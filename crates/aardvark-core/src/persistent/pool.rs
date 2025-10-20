use crate::bundle::BundleFingerprint;
use crate::error::{PyRunnerError, Result};
use crate::persistent::{
    BundleArtifact, BundleHandle, HandlerSession, IsolateConfig, PythonIsolate,
};
use crate::strategy::RawCtxInput;
use hdrhistogram::Histogram;
use parking_lot::{Condvar, Mutex};
use serde_json::Value as JsonValue;
use std::collections::HashSet;
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueMode {
    Block,
    FailFast,
}

impl Default for QueueMode {
    fn default() -> Self {
        Self::Block
    }
}

pub type IsolateId = u64;

/// Configuration for bundle pools.
#[derive(Clone)]
pub struct PoolOptions {
    pub isolate: IsolateConfig,
    pub desired_size: usize,
    pub max_size: usize,
    pub max_queue: Option<usize>,
    pub queue_mode: QueueMode,
    pub lifecycle_hooks: Option<LifecycleHooks>,
    pub memory_limit_kib: Option<u64>,
    pub heap_limit_kib: Option<u64>,
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
        if self.desired_size == 0 {
            return Err(PyRunnerError::Validation(
                "pool desired_size must be at least 1".to_string(),
            ));
        }
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
    pub on_isolate_started: Option<IsolateStartCallback>,
    pub on_isolate_recycled: Option<IsolateRecycleCallback>,
    pub on_call_started: Option<CallStartedCallback>,
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
    pub isolate_id: IsolateId,
    pub bundle_fingerprint: BundleFingerprint,
    pub entrypoint: String,
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

struct BundlePoolInner {
    artifact: Arc<BundleArtifact>,
    options: Mutex<PoolOptions>,
    state: Mutex<PoolState>,
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

impl BundlePool {
    /// Constructs a pool from bundle bytes and options.
    pub fn from_bytes(bytes: impl AsRef<[u8]>, options: PoolOptions) -> Result<Self> {
        let artifact = BundleArtifact::from_bytes(bytes)?;
        Self::from_artifact(artifact, options)
    }

    /// Constructs a pool from a pre-parsed artifact and options.
    pub fn from_artifact(artifact: Arc<BundleArtifact>, options: PoolOptions) -> Result<Self> {
        let inner = BundlePoolInner::new(artifact, options)?;
        Ok(Self { inner })
    }

    #[doc(hidden)]
    pub fn test_acquire_guard(&self) -> Result<TestLease> {
        let (guard, _) = self.inner.acquire_slot()?;
        Ok(TestLease { guard: Some(guard) })
    }

    /// Returns the shared bundle artifact.
    pub fn artifact(&self) -> Arc<BundleArtifact> {
        Arc::clone(&self.inner.artifact)
    }

    /// Returns a handle that can be used to prepare handler sessions.
    pub fn handle(&self) -> BundleHandle {
        BundleHandle::from_artifact(self.artifact())
    }

    /// Invokes a handler using JSON adapters.
    pub fn call_json(
        &self,
        handler: &HandlerSession,
        input: Option<JsonValue>,
    ) -> Result<crate::ExecutionOutcome> {
        self.call_with(handler, CallInvocation::Json(input))
    }

    /// Invokes a handler using RawCtx adapters.
    pub fn call_rawctx(
        &self,
        handler: &HandlerSession,
        inputs: Vec<RawCtxInput>,
    ) -> Result<crate::ExecutionOutcome> {
        self.call_with(handler, CallInvocation::RawCtx(inputs))
    }

    /// Invokes a handler using the default strategy.
    pub fn call_default(&self, handler: &HandlerSession) -> Result<crate::ExecutionOutcome> {
        self.call_with(handler, CallInvocation::Default)
    }

    /// Returns current pool statistics.
    pub fn stats(&self) -> PoolStats {
        let snapshot = self.inner.stats.snapshot();
        let state = self.inner.state.lock();
        let total = state.active;
        let idle = state.idle.len();
        let waiting = state.waiting;
        let busy = total.saturating_sub(idle);
        let (quarantine_events, quarantine_heap_hits, quarantine_rss_hits) =
            self.inner.metrics.quarantine_counts();
        let scaledown_events = self.inner.metrics.scaledown_count();
        PoolStats {
            total,
            idle,
            busy,
            waiting,
            invocations: snapshot.invocations,
            average_queue_wait_ms: snapshot.average_queue_wait_ms,
            queue_wait_p50_ms: snapshot.queue_wait_p50_ms,
            queue_wait_p95_ms: snapshot.queue_wait_p95_ms,
            quarantine_events,
            quarantine_heap_hits,
            quarantine_rss_hits,
            scaledown_events,
        }
    }

    /// Adjusts the maximum pool size (no-op for the single-isolate pool).
    pub fn resize(&self, new_max_size: usize) -> Result<()> {
        if new_max_size == 0 {
            return Err(PyRunnerError::Validation(
                "pool size must be at least 1".to_string(),
            ));
        }

        self.inner.shrink_to(new_max_size)?;

        let desired = {
            let mut opts = self.inner.options.lock();
            if opts.desired_size > new_max_size {
                opts.desired_size = new_max_size;
            }
            opts.max_size = new_max_size;
            opts.desired_size
        };

        self.inner.ensure_min_isolates(desired)?;
        Ok(())
    }

    /// Sets the desired steady-state isolate count.
    pub fn set_desired_size(&self, desired_size: usize) -> Result<()> {
        if desired_size == 0 {
            return Err(PyRunnerError::Validation(
                "pool desired_size must be at least 1".to_string(),
            ));
        }

        {
            let max_size = { self.inner.options.lock().max_size };
            if desired_size > max_size {
                return Err(PyRunnerError::Validation(format!(
                    "desired_size {desired_size} exceeds max_size {max_size}",
                )));
            }
        }

        {
            let mut opts = self.inner.options.lock();
            opts.desired_size = desired_size;
        }

        self.inner.ensure_min_isolates(desired_size)?;
        self.inner.shrink_to(desired_size)?;
        Ok(())
    }

    fn call_with(
        &self,
        handler: &HandlerSession,
        invocation: CallInvocation,
    ) -> Result<crate::ExecutionOutcome> {
        let (mut guard, wait_duration) = self.inner.acquire_slot()?;
        let queue_wait_ms = wait_duration.as_millis().min(u128::from(u64::MAX)) as u64;
        let rss_before = current_rss_kib();
        let context = CallContext::new(
            guard.isolate().id(),
            handler.artifact().fingerprint(),
            handler.descriptor().entrypoint().to_owned(),
            queue_wait_ms,
        );
        let bundle_hex = format!("{:016x}", context.bundle_fingerprint_hex());
        let call_span = info_span!(
            target: "aardvark::telemetry",
            "aardvark.call",
            isolate_id = context.isolate_id(),
            bundle = bundle_hex.as_str(),
            entrypoint = context.entrypoint(),
            queue_wait_ms = queue_wait_ms
        );
        let _call_guard = call_span.enter();
        info!(
            target: "aardvark::telemetry",
            isolate_id = context.isolate_id(),
            bundle = bundle_hex.as_str(),
            entrypoint = context.entrypoint(),
            queue_wait_ms,
            "call.start"
        );
        self.inner.call_hook_call_started(&context);

        let result = {
            let mut isolate = guard.isolate().isolate.lock();
            match invocation {
                CallInvocation::Default => handler.invoke(&mut isolate),
                CallInvocation::Json(input) => handler.invoke_json(&mut isolate, input),
                CallInvocation::RawCtx(inputs) => handler.invoke_rawctx(&mut isolate, inputs),
            }
        };
        let rss_after = current_rss_kib();
        self.inner.stats.record_invocation(wait_duration);
        let (memory_limit_kib, heap_limit_kib) = self.inner.current_limits();
        match result {
            Ok(mut outcome) => {
                info!(
                    target: "aardvark::telemetry",
                    isolate_id = context.isolate_id(),
                    bundle = bundle_hex.as_str(),
                    status = ?outcome.status,
                    queue_wait_ms,
                    heap_kib = outcome.diagnostics.py_heap_kib,
                    rss_after = rss_after,
                    "call.success"
                );
                outcome.diagnostics.queue_wait_ms = Some(queue_wait_ms);
                if outcome.diagnostics.rss_kib_before.is_none() {
                    outcome.diagnostics.rss_kib_before = rss_before;
                }
                if outcome.diagnostics.rss_kib_after.is_none() {
                    outcome.diagnostics.rss_kib_after = rss_after;
                }

                let mut exceeded_heap = false;
                let mut exceeded_rss = false;
                if let Some(limit) = heap_limit_kib {
                    if outcome
                        .diagnostics
                        .py_heap_kib
                        .filter(|heap| *heap > limit)
                        .is_some()
                    {
                        exceeded_heap = true;
                    }
                }
                if let Some(limit) = memory_limit_kib {
                    if rss_after.filter(|rss| *rss > limit).is_some() {
                        exceeded_rss = true;
                    }
                }

                if exceeded_heap || exceeded_rss {
                    let reason = RecycleReason::Quarantined {
                        exceeded_heap,
                        exceeded_rss,
                    };
                    if let Some(id) = self.inner.quarantine_slot(guard.index(), reason.clone()) {
                        warn!(
                            target: "aardvark::pool",
                            isolate_id = id,
                            bundle = bundle_hex.as_str(),
                            exceeded_heap,
                            exceeded_rss,
                            "quarantining isolate after exceeding memory limits"
                        );
                        guard.suppress_release();
                        drop(guard);
                        self.inner.ensure_desired_isolates();
                        self.inner
                            .call_hook_call_finished(&context, CallOutcome::Success(&outcome));
                        return Ok(outcome);
                    }
                }

                drop(guard);
                self.inner
                    .call_hook_call_finished(&context, CallOutcome::Success(&outcome));
                Ok(outcome)
            }
            Err(err) => {
                error!(
                    target: "aardvark::telemetry",
                    isolate_id = context.isolate_id(),
                    bundle = bundle_hex.as_str(),
                    error = %err,
                    "call.error"
                );
                drop(guard);
                self.inner
                    .call_hook_call_finished(&context, CallOutcome::Error(&err));
                Err(err)
            }
        }
    }
}

impl Clone for BundlePool {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

enum CallInvocation {
    Default,
    Json(Option<JsonValue>),
    RawCtx(Vec<RawCtxInput>),
}

impl PoolStatsTracker {
    fn new() -> Self {
        Self {
            invocations: AtomicU64::new(0),
            queue_wait_ns: AtomicU64::new(0),
            queue_wait_hist: Mutex::new(Histogram::new(3).expect("histogram init")),
        }
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

impl BundlePoolInner {
    #[allow(clippy::arc_with_non_send_sync)]
    fn new(artifact: Arc<BundleArtifact>, options: PoolOptions) -> Result<Arc<Self>> {
        options.validate()?;
        let hooks = options.lifecycle_hooks.clone().unwrap_or_default();
        let inner = Arc::new(Self {
            artifact,
            options: Mutex::new(options),
            state: Mutex::new(PoolState::new()),
            condvar: Condvar::new(),
            stats: Arc::new(PoolStatsTracker::new()),
            metrics: Arc::new(PoolSharedMetrics::new()),
            hooks,
            isolate_seq: AtomicU64::new(1),
            telemetry: Mutex::new(None),
        });

        let desired = {
            let opts = inner.options.lock();
            opts.desired_size
        };
        inner.ensure_min_isolates(desired)?;
        inner.start_telemetry();
        Ok(inner)
    }

    fn ensure_min_isolates(&self, target: usize) -> Result<()> {
        loop {
            {
                let state = self.state.lock();
                if state.active + state.creating >= target {
                    return Ok(());
                }
            }
            self.spawn_isolate(true)?;
        }
    }

    fn shrink_to(&self, target: usize) -> Result<()> {
        let mut removed = Vec::new();
        {
            let mut state = self.state.lock();
            if state.active <= target {
                return Ok(());
            }
            let removable = state.active.saturating_sub(target);
            let idle_available = state.idle.len();
            if removable > idle_available {
                let busy = state.active.saturating_sub(idle_available);
                return Err(PyRunnerError::Validation(format!(
                    "cannot shrink pool below {target} isolates while {busy} isolates are busy",
                    busy = busy,
                )));
            }

            let idle_set: HashSet<usize> = state.idle.iter().copied().collect();
            let mut isolates: Vec<(IsolateId, usize, bool)> = state
                .isolates
                .iter()
                .enumerate()
                .filter_map(|(index, slot)| {
                    slot.as_ref().map(|slot| {
                        let is_idle = idle_set.contains(&index);
                        (slot.id(), index, is_idle)
                    })
                })
                .collect();
            isolates.sort_by(|a, b| b.0.cmp(&a.0));

            let mut indices_to_remove = Vec::with_capacity(removable);
            for (id, index, is_idle) in isolates {
                if indices_to_remove.len() == removable {
                    break;
                }
                if !is_idle {
                    return Err(PyRunnerError::Validation(format!(
                        "cannot shrink pool below {target} isolates while isolate {id} is busy",
                    )));
                }
                indices_to_remove.push(index);
            }

            if indices_to_remove.len() < removable {
                let busy = state.active.saturating_sub(state.idle.len());
                return Err(PyRunnerError::Validation(format!(
                    "cannot shrink pool below {target} isolates while {busy} isolates are busy",
                    busy = busy,
                )));
            }

            let remove_set: HashSet<usize> = indices_to_remove.iter().copied().collect();
            state.idle.retain(|index| !remove_set.contains(index));

            for index in indices_to_remove {
                if let Some(slot) = state.isolates[index].take() {
                    removed.push(slot);
                }
                state.active = state.active.saturating_sub(1);
                self.metrics.dec_active();
                self.metrics.dec_idle();
            }

            while matches!(state.isolates.last(), Some(None)) {
                state.isolates.pop();
            }
        }

        if removed.is_empty() {
            return Ok(());
        }

        self.metrics.add_scaledown(removed.len());

        let reason = RecycleReason::ScaledDown;
        for slot in removed {
            let id = slot.id();
            self.call_hook_isolate_recycled(id, &reason);
            drop(slot);
        }

        Ok(())
    }

    fn start_telemetry(self: &Arc<Self>) {
        let interval = {
            let opts = self.options.lock();
            opts.telemetry_interval
        };

        let Some(interval) = interval else {
            return;
        };

        if interval.is_zero() {
            return;
        }

        let mut slot = self.telemetry.lock();
        if slot.is_some() {
            return;
        }

        if let Some(handle) =
            TelemetryHandle::spawn(Arc::clone(&self.stats), Arc::clone(&self.metrics), interval)
        {
            *slot = Some(handle);
        }
    }

    fn acquire_slot(self: &Arc<Self>) -> Result<(SlotGuard, Duration)> {
        let start = Instant::now();
        loop {
            let (max_size, queue_mode, max_queue) = {
                let opts = self.options.lock();
                (opts.max_size, opts.queue_mode, opts.max_queue)
            };

            let mut state = self.state.lock();
            if state.shutdown {
                return Err(PyRunnerError::PoolShuttingDown);
            }

            if let Some(index) = state.idle.pop() {
                self.metrics.dec_idle();
                let slot = state.isolates[index]
                    .as_ref()
                    .expect("idle slot must exist")
                    .clone();
                drop(state);
                let wait_duration = start.elapsed();
                return Ok((SlotGuard::new(self.clone(), index, slot), wait_duration));
            }

            if state.active + state.creating < max_size {
                drop(state);
                let entry = self.spawn_isolate(false)?;
                let wait_duration = start.elapsed();
                return Ok((
                    SlotGuard::new(self.clone(), entry.index, entry.slot),
                    wait_duration,
                ));
            }

            if matches!(queue_mode, QueueMode::FailFast) {
                return Err(PyRunnerError::PoolAtCapacity {
                    active: state.active,
                    max_size,
                });
            }

            if let Some(limit) = max_queue {
                if state.waiting >= limit {
                    return Err(PyRunnerError::PoolQueueFull {
                        queue_length: state.waiting + 1,
                        limit,
                    });
                }
            }

            state.waiting += 1;
            self.metrics.inc_waiting();
            self.condvar.wait(&mut state);
            state.waiting = state.waiting.saturating_sub(1);
            self.metrics.dec_waiting();
        }
    }

    fn release_slot(&self, index: usize) {
        let isolate_id = {
            let mut state = self.state.lock();
            if state.shutdown {
                return;
            }
            debug_assert!(index < state.isolates.len());
            let id = state
                .isolates
                .get(index)
                .and_then(|slot| slot.as_ref().map(|slot| slot.id()));
            state.idle.push(index);
            self.metrics.inc_idle();
            self.condvar.notify_one();
            id
        };
        if let Some(id) = isolate_id {
            let reason = RecycleReason::ReturnedToIdle;
            self.call_hook_isolate_recycled(id, &reason);
            info!(
                target: "aardvark::pool",
                isolate_id = id,
                reason = ?reason,
                "isolate.idle"
            );
        }
    }

    #[allow(clippy::arc_with_non_send_sync)]
    fn spawn_isolate(&self, add_to_idle: bool) -> Result<SlotEntry> {
        let options_snapshot = { self.options.lock().clone() };

        let placeholder_index = {
            let mut state = self.state.lock();
            if state.shutdown {
                return Err(PyRunnerError::PoolShuttingDown);
            }
            if state.active + state.creating >= options_snapshot.max_size {
                return Err(PyRunnerError::PoolAtCapacity {
                    active: state.active,
                    max_size: options_snapshot.max_size,
                });
            }
            state.isolates.push(None);
            state.creating += 1;
            state.isolates.len() - 1
        };

        let artifact = self.artifact.clone();
        let creation = (|| -> Result<PythonIsolate> {
            let mut isolate = PythonIsolate::new(options_snapshot.isolate.clone())?;
            let handle = BundleHandle::from_artifact(artifact);
            isolate.load_bundle(&handle)?;
            Ok(isolate)
        })();

        match creation {
            Ok(isolate) => {
                let isolate_id = self.isolate_seq.fetch_add(1, Ordering::Relaxed);
                let slot = Arc::new(IsolateSlot::new(isolate_id, isolate));

                let active_after = {
                    let mut state = self.state.lock();
                    state.creating = state.creating.saturating_sub(1);
                    state.isolates[placeholder_index] = Some(slot.clone());
                    state.active += 1;
                    self.metrics.inc_active();
                    if add_to_idle {
                        state.idle.push(placeholder_index);
                        self.condvar.notify_one();
                        self.metrics.inc_idle();
                    }
                    state.active
                };

                self.call_hook_isolate_started(isolate_id, &options_snapshot.isolate);
                info!(
                    target: "aardvark::pool",
                    isolate_id,
                    active_isolates = active_after,
                    "isolate.started"
                );

                Ok(SlotEntry {
                    index: placeholder_index,
                    slot,
                })
            }
            Err(err) => {
                let mut state = self.state.lock();
                state.creating = state.creating.saturating_sub(1);
                if placeholder_index + 1 == state.isolates.len() {
                    state.isolates.pop();
                } else {
                    state.isolates[placeholder_index] = None;
                }
                Err(err)
            }
        }
    }
}

impl Drop for BundlePoolInner {
    fn drop(&mut self) {
        {
            let telemetry = self.telemetry.lock().take();
            drop(telemetry);
        }
        self.metrics.active.store(0, Ordering::Relaxed);
        self.metrics.idle.store(0, Ordering::Relaxed);
        self.metrics.waiting.store(0, Ordering::Relaxed);
        let mut state = self.state.lock();
        state.shutdown = true;
        state.idle.clear();
        let mut recycled = Vec::new();
        while let Some(entry) = state.isolates.pop() {
            if let Some(slot) = entry {
                recycled.push(slot);
            }
        }
        drop(state);
        let reason = RecycleReason::Shutdown;
        for slot in recycled {
            let id = slot.id();
            self.call_hook_isolate_recycled(id, &reason);
            drop(slot);
        }
    }
}

#[cfg(target_os = "linux")]
fn current_rss_kib() -> Option<u64> {
    let mut file = File::open("/proc/self/statm").ok()?;
    let mut contents = String::new();
    file.read_to_string(&mut contents).ok()?;
    let mut parts = contents.split_whitespace();
    parts.next()?; // skip total
    let resident_pages: u64 = parts.next()?.parse().ok()?;
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as u64;
    Some(resident_pages.saturating_mul(page_size) / 1024)
}

#[cfg(target_os = "macos")]
fn current_rss_kib() -> Option<u64> {
    use std::mem::MaybeUninit;
    unsafe {
        let mut info = MaybeUninit::<libc::mach_task_basic_info>::uninit();
        #[allow(deprecated)]
        let task = libc::mach_task_self();
        let mut count = libc::MACH_TASK_BASIC_INFO_COUNT;
        let result = libc::task_info(
            task,
            libc::MACH_TASK_BASIC_INFO,
            info.as_mut_ptr() as *mut libc::integer_t,
            &mut count,
        );
        if result != libc::KERN_SUCCESS {
            return None;
        }
        let info = info.assume_init();
        Some(info.resident_size / 1024)
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn current_rss_kib() -> Option<u64> {
    None
}

impl BundlePoolInner {
    fn call_hook_isolate_started(&self, isolate_id: IsolateId, config: &IsolateConfig) {
        if let Some(callback) = &self.hooks.on_isolate_started {
            callback(isolate_id, config);
        }
    }

    fn call_hook_isolate_recycled(&self, isolate_id: IsolateId, reason: &RecycleReason) {
        if let Some(callback) = &self.hooks.on_isolate_recycled {
            callback(isolate_id, reason);
        }
    }

    fn call_hook_call_started(&self, context: &CallContext) {
        if let Some(callback) = &self.hooks.on_call_started {
            callback(context);
        }
    }

    fn call_hook_call_finished<'a>(&self, context: &CallContext, outcome: CallOutcome<'a>) {
        if let Some(callback) = &self.hooks.on_call_finished {
            callback(context, outcome);
        }
    }

    fn current_limits(&self) -> (Option<u64>, Option<u64>) {
        let opts = self.options.lock();
        (opts.memory_limit_kib, opts.heap_limit_kib)
    }

    fn ensure_desired_isolates(&self) {
        let desired = { self.options.lock().desired_size };
        if let Err(err) = self.ensure_min_isolates(desired) {
            warn!(target: "aardvark::pool", error = %err, "failed to replenish isolates after quarantine");
        }
    }

    fn quarantine_slot(&self, index: usize, reason: RecycleReason) -> Option<IsolateId> {
        let (removed_id, removed_slot) = {
            let mut state = self.state.lock();
            if index >= state.isolates.len() {
                return None;
            }
            let removed = state.isolates[index].take();
            let removed_id = removed.as_ref().map(|slot| slot.id());
            if removed.is_some() {
                state.active = state.active.saturating_sub(1);
                self.metrics.dec_active();
                let idle_before = state.idle.len();
                state.idle.retain(|&i| i != index);
                if state.idle.len() < idle_before {
                    self.metrics.dec_idle();
                }
            }
            while matches!(state.isolates.last(), Some(None)) {
                state.isolates.pop();
            }
            self.condvar.notify_one();
            (removed_id, removed)
        };
        if let Some(id) = removed_id {
            if let RecycleReason::Quarantined {
                exceeded_heap,
                exceeded_rss,
            } = &reason
            {
                self.metrics.inc_quarantine(*exceeded_heap, *exceeded_rss);
            }
            self.call_hook_isolate_recycled(id, &reason);
            info!(
                target: "aardvark::pool",
                isolate_id = id,
                reason = ?reason,
                "isolate.quarantined"
            );
        }
        drop(removed_slot);
        removed_id
    }
}
