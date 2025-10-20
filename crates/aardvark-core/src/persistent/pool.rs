use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};

use crate::error::Result;
use crate::persistent::{
    BundleArtifact, BundleHandle, HandlerSession, IsolateConfig, PythonIsolate,
};
use crate::strategy::RawCtxInput;
use serde_json::Value as JsonValue;

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

/// Configuration for bundle pools.
#[derive(Clone)]
pub struct PoolOptions {
    pub isolate: IsolateConfig,
    pub desired_size: usize,
    pub max_size: usize,
    pub max_queue: Option<usize>,
    pub queue_mode: QueueMode,
}

impl Default for PoolOptions {
    fn default() -> Self {
        Self {
            isolate: IsolateConfig::default(),
            desired_size: 1,
            max_size: 1,
            max_queue: Some(64),
            queue_mode: QueueMode::Block,
        }
    }
}

/// Snapshot of current pool state.
pub struct PoolStats {
    pub total: usize,
    pub idle: usize,
    pub waiting: usize,
    pub invocations: u64,
    pub average_queue_wait_ms: f64,
}

/// Bundle-scoped pool managing a reusable isolate.
pub struct BundlePool {
    inner: BundlePoolInner,
}

struct BundlePoolInner {
    artifact: Arc<BundleArtifact>,
    isolate: Mutex<PythonIsolate>,
    stats: PoolStatsTracker,
}

struct PoolStatsTracker {
    invocations: AtomicU64,
    queue_wait_ns: AtomicU64,
}

impl BundlePool {
    /// Constructs a pool from bundle bytes and options.
    pub fn from_bytes(bytes: impl AsRef<[u8]>, options: PoolOptions) -> Result<Self> {
        let artifact = BundleArtifact::from_bytes(bytes)?;
        Self::from_artifact(artifact, options)
    }

    /// Constructs a pool from a pre-parsed artifact and options.
    pub fn from_artifact(artifact: Arc<BundleArtifact>, options: PoolOptions) -> Result<Self> {
        let mut isolate = PythonIsolate::new(options.isolate)?;
        let handle = BundleHandle::from_artifact(artifact.clone());
        isolate.load_bundle(&handle)?;
        let _ = (
            options.desired_size,
            options.max_size,
            options.max_queue,
            options.queue_mode,
        );
        Ok(Self {
            inner: BundlePoolInner {
                artifact,
                isolate: Mutex::new(isolate),
                stats: PoolStatsTracker {
                    invocations: AtomicU64::new(0),
                    queue_wait_ns: AtomicU64::new(0),
                },
            },
        })
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
        let invocations = self.inner.stats.invocations.load(Ordering::Relaxed);
        let queue_wait_ns = self.inner.stats.queue_wait_ns.load(Ordering::Relaxed);
        let average_queue_wait_ms = if invocations == 0 {
            0.0
        } else {
            (queue_wait_ns as f64 / invocations as f64) / 1_000_000.0
        };
        PoolStats {
            total: 1,
            idle: 1,
            waiting: 0,
            invocations,
            average_queue_wait_ms,
        }
    }

    /// Adjusts the maximum pool size (no-op for the single-isolate pool).
    pub fn resize(&self, _new_max_size: usize) {}

    fn call_with(
        &self,
        handler: &HandlerSession,
        invocation: CallInvocation,
    ) -> Result<crate::ExecutionOutcome> {
        let start = Instant::now();
        let mut isolate = self.inner.isolate.lock().unwrap();
        let wait_duration = start.elapsed();
        let mut outcome = match invocation {
            CallInvocation::Default => handler.invoke(&mut isolate)?,
            CallInvocation::Json(input) => handler.invoke_json(&mut isolate, input)?,
            CallInvocation::RawCtx(inputs) => handler.invoke_rawctx(&mut isolate, inputs)?,
        };
        self.inner.stats.record_invocation(wait_duration);
        outcome.diagnostics.queue_wait_ms = Some(wait_duration.as_millis() as u64);
        Ok(outcome)
    }
}

enum CallInvocation {
    Default,
    Json(Option<JsonValue>),
    RawCtx(Vec<RawCtxInput>),
}

impl PoolStatsTracker {
    fn record_invocation(&self, wait: Duration) {
        self.invocations.fetch_add(1, Ordering::Relaxed);
        self.queue_wait_ns
            .fetch_add(wait.as_nanos() as u64, Ordering::Relaxed);
    }
}
