use crate::bundle::BundleFingerprint;
use crate::error::{PyRunnerError, Result};
use crate::invocation::InvocationDescriptor;
use crate::persistent::{
    BundleArtifact, BundleHandle, HandlerSession, IsolateConfig, PythonIsolate,
};
use crate::strategy::{JsonInput, RawCtxInput};

mod api;
mod inner;
mod registry;
use hdrhistogram::Histogram;
use parking_lot::{Condvar, Mutex};
use serde_json::Value as JsonValue;
use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use tracing::{error, info, info_span, warn};

#[cfg(target_os = "linux")]
use std::fs::File;
#[cfg(target_os = "linux")]
use std::io::Read;

/// Queue backpressure strategy.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum QueueMode {
    #[default]
    Block,
    FailFast,
}

pub type IsolateId = u64;

/// Configuration for bundle pools.
#[derive(Clone)]
pub struct PoolOptions {
    /// Baseline isolate options (Pyodide version, warm snapshot hooks, etc.).
    pub isolate: IsolateConfig,
    /// Preferred number of isolates to keep hot. Use zero for lazy pools.
    pub desired_size: usize,
    /// Upper bound on isolates that may be spawned when demand spikes.
    pub max_size: usize,
    /// Optional maximum number of queued calls awaiting an idle isolate.
    pub max_queue: Option<usize>,
    /// Behaviour when the queue is full (`Block` vs `FailFast`).
    pub queue_mode: QueueMode,
    /// Optional lifecycle callbacks invoked around isolate/call events.
    pub lifecycle_hooks: Option<LifecycleHooks>,
    /// RSS guard rail in KiB; isolates exceeding it are quarantined.
    pub memory_limit_kib: Option<u64>,
    /// Pyodide heap guard rail in KiB; isolates exceeding it are quarantined.
    pub heap_limit_kib: Option<u64>,
    /// Interval for the periodic telemetry reporter (set to `None` to disable).
    pub telemetry_interval: Option<Duration>,
}

impl Default for PoolOptions {
    fn default() -> Self {
        Self {
            isolate: IsolateConfig::default(),
            desired_size: 1,
            max_size: 1,
            max_queue: Some(64),
            queue_mode: QueueMode::Block,
            lifecycle_hooks: None,
            memory_limit_kib: None,
            heap_limit_kib: None,
            telemetry_interval: Some(Duration::from_millis(250)),
        }
    }
}

impl PoolOptions {
    fn validate(&self) -> Result<()> {
        if self.max_size == 0 {
            return Err(PyRunnerError::Validation(
                "pool max_size must be at least 1".to_string(),
            ));
        }
        if self.desired_size > self.max_size {
            return Err(PyRunnerError::Validation(format!(
                "desired_size ({}) cannot exceed max_size ({})",
                self.desired_size, self.max_size
            )));
        }
        Ok(())
    }
}

type IsolateStartCallback = Arc<dyn Fn(IsolateId, &IsolateConfig) + Send + Sync>;
type IsolateRecycleCallback = Arc<dyn Fn(IsolateId, &RecycleReason) + Send + Sync>;
type CallStartedCallback = Arc<dyn Fn(&CallContext) + Send + Sync>;
type CallFinishedCallback = Arc<dyn for<'a> Fn(&CallContext, CallOutcome<'a>) + Send + Sync>;

/// Lifecycle hooks invoked during pool operations.
#[derive(Clone, Default)]
pub struct LifecycleHooks {
    /// Called when a new isolate starts (after warm state application).
    pub on_isolate_started: Option<IsolateStartCallback>,
    /// Called when an isolate leaves active service (idle/quarantined/etc.).
    pub on_isolate_recycled: Option<IsolateRecycleCallback>,
    /// Called right before a call is handed to an isolate.
    pub on_call_started: Option<CallStartedCallback>,
    /// Called after a call completes (success or failure).
    pub on_call_finished: Option<CallFinishedCallback>,
}

/// Reason describing why an isolate left active service.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecycleReason {
    /// The isolate completed work and returned to the idle pool.
    ReturnedToIdle,
    /// The isolate exceeded a guard rail and was quarantined.
    Quarantined {
        exceeded_heap: bool,
        exceeded_rss: bool,
    },
    /// The pool scaled down and explicitly dropped this isolate.
    ScaledDown,
    /// The pool shut down and is releasing all isolates.
    Shutdown,
}

/// Outcome provided to hook callbacks once a call completes.
pub enum CallOutcome<'a> {
    Success(&'a crate::ExecutionOutcome),
    Error(&'a PyRunnerError),
}

/// Snapshot describing an invocation being processed by the pool.
pub struct CallContext {
    /// Identifier for the isolate serving the call.
    pub isolate_id: IsolateId,
    /// Fingerprint of the bundle currently mounted.
    pub bundle_fingerprint: BundleFingerprint,
    /// Entrypoint being executed (module:function).
    pub entrypoint: String,
    /// Milliseconds the call spent waiting in the queue before dispatch.
    pub queue_wait_ms: u64,
}

impl CallContext {
    fn new(
        isolate_id: IsolateId,
        bundle_fingerprint: BundleFingerprint,
        entrypoint: String,
        queue_wait_ms: u64,
    ) -> Self {
        Self {
            isolate_id,
            bundle_fingerprint,
            entrypoint,
            queue_wait_ms,
        }
    }

    pub fn isolate_id(&self) -> IsolateId {
        self.isolate_id
    }

    pub fn bundle_fingerprint(&self) -> BundleFingerprint {
        self.bundle_fingerprint
    }

    pub fn bundle_fingerprint_hex(&self) -> u64 {
        self.bundle_fingerprint.as_u64()
    }

    pub fn entrypoint(&self) -> &str {
        &self.entrypoint
    }

    pub fn queue_wait_ms(&self) -> u64 {
        self.queue_wait_ms
    }
}

/// Snapshot of current pool state.
pub struct PoolStats {
    pub total: usize,
    pub idle: usize,
    pub busy: usize,
    pub waiting: usize,
    pub invocations: u64,
    pub average_queue_wait_ms: f64,
    pub queue_wait_p50_ms: Option<f64>,
    pub queue_wait_p95_ms: Option<f64>,
    pub quarantine_events: u64,
    pub quarantine_heap_hits: u64,
    pub quarantine_rss_hits: u64,
    pub scaledown_events: u64,
}

/// Bundle-scoped pool managing a reusable isolate.
pub struct BundlePool {
    inner: Arc<BundlePoolInner>,
}

/// Stable key used by [`BundlePoolRegistry`] for warmed bundle pools.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct BundlePoolKey {
    fingerprint: BundleFingerprint,
    pyodide_distribution_profile: Option<String>,
}

/// Host-side registry that routes bundles to profile-aware warmed pools.
#[derive(Clone)]
pub struct BundlePoolRegistry {
    inner: Arc<BundlePoolRegistryInner>,
}

/// Prepared pool/handler pair returned by [`BundlePoolRegistry`].
#[derive(Clone)]
pub struct PreparedBundleHandler {
    pool: BundlePool,
    handler: Arc<HandlerSession>,
}

struct BundlePoolRegistryInner {
    options: PoolOptions,
    artifacts: Mutex<HashMap<[u8; 32], Arc<BundleArtifact>>>,
    pools: Mutex<HashMap<BundlePoolKey, RegistryPoolSlot>>,
    handlers: Mutex<HashMap<BundleHandlerKey, PreparedBundleHandler>>,
    condvar: Condvar,
}

enum RegistryPoolSlot {
    Creating,
    Ready(BundlePool),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct BundleHandlerKey {
    pool: BundlePoolKey,
    descriptor: String,
}

struct BundlePoolInner {
    artifact: Arc<BundleArtifact>,
    options: Mutex<PoolOptions>,
    state: Mutex<PoolState>,
    prewarmed_handlers: Mutex<Vec<InvocationDescriptor>>,
    condvar: Condvar,
    stats: Arc<PoolStatsTracker>,
    metrics: Arc<PoolSharedMetrics>,
    hooks: LifecycleHooks,
    isolate_seq: AtomicU64,
    telemetry: Mutex<Option<TelemetryHandle>>,
}

struct PoolStatsTracker {
    invocations: AtomicU64,
    queue_wait_ns: AtomicU64,
    queue_wait_hist: Mutex<Histogram<u64>>,
}

struct PoolSharedMetrics {
    active: AtomicUsize,
    idle: AtomicUsize,
    waiting: AtomicUsize,
    quarantine_total: AtomicU64,
    quarantine_heap: AtomicU64,
    quarantine_rss: AtomicU64,
    scaledown_total: AtomicU64,
}

impl PoolSharedMetrics {
    fn new() -> Self {
        Self {
            active: AtomicUsize::new(0),
            idle: AtomicUsize::new(0),
            waiting: AtomicUsize::new(0),
            quarantine_total: AtomicU64::new(0),
            quarantine_heap: AtomicU64::new(0),
            quarantine_rss: AtomicU64::new(0),
            scaledown_total: AtomicU64::new(0),
        }
    }

    fn inc_active(&self) {
        self.active.fetch_add(1, Ordering::Relaxed);
    }

    fn dec_active(&self) {
        let _ = self
            .active
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_sub(1)
            });
    }

    fn inc_idle(&self) {
        self.idle.fetch_add(1, Ordering::Relaxed);
    }

    fn dec_idle(&self) {
        let _ = self
            .idle
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_sub(1)
            });
    }

    fn inc_waiting(&self) {
        self.waiting.fetch_add(1, Ordering::Relaxed);
    }

    fn dec_waiting(&self) {
        let _ = self
            .waiting
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_sub(1)
            });
    }

    fn inc_quarantine(&self, exceeded_heap: bool, exceeded_rss: bool) {
        self.quarantine_total.fetch_add(1, Ordering::Relaxed);
        if exceeded_heap {
            self.quarantine_heap.fetch_add(1, Ordering::Relaxed);
        }
        if exceeded_rss {
            self.quarantine_rss.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn add_scaledown(&self, count: usize) {
        if count == 0 {
            return;
        }
        self.scaledown_total
            .fetch_add(count as u64, Ordering::Relaxed);
    }

    fn quarantine_counts(&self) -> (u64, u64, u64) {
        (
            self.quarantine_total.load(Ordering::Relaxed),
            self.quarantine_heap.load(Ordering::Relaxed),
            self.quarantine_rss.load(Ordering::Relaxed),
        )
    }

    fn scaledown_count(&self) -> u64 {
        self.scaledown_total.load(Ordering::Relaxed)
    }
}

struct StatsSnapshot {
    invocations: u64,
    average_queue_wait_ms: f64,
    queue_wait_p50_ms: Option<f64>,
    queue_wait_p95_ms: Option<f64>,
}

struct PoolState {
    isolates: Vec<Option<Arc<IsolateSlot>>>,
    idle: Vec<usize>,
    waiting: usize,
    creating: usize,
    active: usize,
    shutdown: bool,
}

struct IsolateSlot {
    id: IsolateId,
    isolate: Mutex<PythonIsolate>,
}

struct TelemetryHandle {
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl TelemetryHandle {
    fn spawn(
        stats: Arc<PoolStatsTracker>,
        metrics: Arc<PoolSharedMetrics>,
        interval: Duration,
    ) -> Option<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let handle = thread::Builder::new()
            .name("aardvark-pool-telemetry".into())
            .spawn(move || {
                let mut last_invocations = 0u64;
                while !thread_stop.load(Ordering::Relaxed) {
                    let snapshot = stats.snapshot();
                    let total = metrics.active.load(Ordering::Relaxed);
                    let idle = metrics.idle.load(Ordering::Relaxed);
                    let waiting = metrics.waiting.load(Ordering::Relaxed);
                    let busy = total.saturating_sub(idle);
                    let (quarantine_total, quarantine_heap, quarantine_rss) =
                        metrics.quarantine_counts();
                    let scaledown = metrics.scaledown_count();
                    let invocations = snapshot.invocations;
                    if (invocations != last_invocations || waiting > 0)
                        && tracing::enabled!(tracing::Level::INFO)
                    {
                        info!(
                            target: "aardvark::telemetry",
                            total_isolates = total,
                            idle_isolates = idle,
                            busy_isolates = busy,
                            waiting_calls = waiting,
                            invocations,
                            avg_queue_wait_ms = snapshot.average_queue_wait_ms,
                            queue_wait_p50_ms = snapshot.queue_wait_p50_ms,
                            queue_wait_p95_ms = snapshot.queue_wait_p95_ms,
                            quarantine_events = quarantine_total,
                            quarantine_heap_hits = quarantine_heap,
                            quarantine_rss_hits = quarantine_rss,
                            scaledown_events = scaledown,
                            "pool.telemetry"
                        );
                    }
                    last_invocations = invocations;
                    thread::sleep(interval);
                }
            });

        match handle {
            Ok(thread) => Some(Self {
                stop,
                thread: Some(thread),
            }),
            Err(err) => {
                warn!(
                    target: "aardvark::pool",
                    error = %err,
                    "failed to spawn telemetry reporter"
                );
                None
            }
        }
    }
}

impl Drop for TelemetryHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

impl IsolateSlot {
    fn new(id: IsolateId, isolate: PythonIsolate) -> Self {
        Self {
            id,
            isolate: Mutex::new(isolate),
        }
    }

    fn id(&self) -> IsolateId {
        self.id
    }
}

struct SlotGuard {
    pool: Arc<BundlePoolInner>,
    index: usize,
    slot: Arc<IsolateSlot>,
    release_on_drop: bool,
}

impl SlotGuard {
    fn new(pool: Arc<BundlePoolInner>, index: usize, slot: Arc<IsolateSlot>) -> Self {
        Self {
            pool,
            index,
            slot,
            release_on_drop: true,
        }
    }

    fn isolate(&self) -> &Arc<IsolateSlot> {
        &self.slot
    }

    fn index(&self) -> usize {
        self.index
    }

    fn suppress_release(&mut self) {
        self.release_on_drop = false;
    }
}

impl Drop for SlotGuard {
    fn drop(&mut self) {
        if self.release_on_drop {
            self.pool.release_slot(self.index);
        }
    }
}

struct SlotEntry {
    index: usize,
    slot: Arc<IsolateSlot>,
}

#[doc(hidden)]
pub struct TestLease {
    guard: Option<SlotGuard>,
}

impl Drop for TestLease {
    fn drop(&mut self) {
        if let Some(guard) = self.guard.take() {
            drop(guard);
        }
    }
}

impl PoolState {
    fn new() -> Self {
        Self {
            isolates: Vec::new(),
            idle: Vec::new(),
            waiting: 0,
            creating: 0,
            active: 0,
            shutdown: false,
        }
    }
}

impl PoolStatsTracker {
    fn new() -> Result<Self> {
        Ok(Self {
            invocations: AtomicU64::new(0),
            queue_wait_ns: AtomicU64::new(0),
            queue_wait_hist: Mutex::new(Histogram::new(3).map_err(|err| {
                PyRunnerError::Internal(format!("failed to initialize queue wait histogram: {err}"))
            })?),
        })
    }

    fn record_invocation(&self, wait: Duration) {
        self.invocations.fetch_add(1, Ordering::Relaxed);
        self.queue_wait_ns
            .fetch_add(wait.as_nanos() as u64, Ordering::Relaxed);
        let wait_ms = wait.as_millis().min(u128::from(u64::MAX)) as u64;
        if let Some(mut hist) = self.queue_wait_hist.try_lock() {
            let _ = hist.record(wait_ms);
        } else {
            let mut hist = self.queue_wait_hist.lock();
            let _ = hist.record(wait_ms);
        }
    }

    fn snapshot(&self) -> StatsSnapshot {
        let invocations = self.invocations.load(Ordering::Relaxed);
        let queue_wait_ns = self.queue_wait_ns.load(Ordering::Relaxed);
        let average_queue_wait_ms = if invocations == 0 {
            0.0
        } else {
            (queue_wait_ns as f64 / invocations as f64) / 1_000_000.0
        };
        let hist = self.queue_wait_hist.lock();
        let (p50, p95) = if hist.is_empty() {
            (None, None)
        } else {
            (
                Some(hist.value_at_quantile(0.5) as f64),
                Some(hist.value_at_quantile(0.95) as f64),
            )
        };
        StatsSnapshot {
            invocations,
            average_queue_wait_ms,
            queue_wait_p50_ms: p50,
            queue_wait_p95_ms: p95,
        }
    }
}

fn descriptor_registry_key(descriptor: &InvocationDescriptor) -> Result<String> {
    serde_json::to_string(descriptor).map_err(|err| {
        PyRunnerError::Descriptor(format!("failed to serialize invocation descriptor: {err}"))
    })
}

fn handler_descriptor_for_artifact(
    artifact: &BundleArtifact,
    descriptor: Option<InvocationDescriptor>,
) -> InvocationDescriptor {
    let mut descriptor = descriptor.unwrap_or_else(|| artifact.default_descriptor());
    descriptor.language = descriptor.language.or(Some(artifact.language()));
    descriptor
}

#[cfg(test)]
mod tests;
