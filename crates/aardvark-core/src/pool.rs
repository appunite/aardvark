//! Simple runtime pool with reset and tracing hooks.

use crate::config::{PyRuntimeConfig, ResetPolicy};
use crate::error::Result;
use crate::runtime::PyRuntime;
use parking_lot::{Condvar, Mutex};
use std::collections::VecDeque;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use tracing::{info, info_span, warn};

/// Pool configuration.
#[derive(Clone)]
pub struct PoolConfig {
    /// Maximum number of runtimes allowed in the pool at any given time.
    pub max_runtimes: usize,
    /// Baseline configuration applied to every newly-created runtime.
    pub runtime_config: PyRuntimeConfig,
    /// How the pool resets runtimes when they are returned.
    pub reset_mode: PoolResetMode,
}

impl PoolConfig {
    /// Create a configuration with explicit capacity and runtime config.
    pub fn new(max_runtimes: usize, runtime_config: PyRuntimeConfig) -> Self {
        Self {
            max_runtimes,
            runtime_config,
            reset_mode: PoolResetMode::RecreateEngine,
        }
    }
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            max_runtimes: 4,
            runtime_config: PyRuntimeConfig::default(),
            reset_mode: PoolResetMode::RecreateEngine,
        }
    }
}

/// Reset strategy to use when returning runtimes to the pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PoolResetMode {
    /// Drop the language engine and recreate it from scratch (`reset_to_snapshot`).
    RecreateEngine,
    /// Keep the existing isolate/context and rebuild it in place (`reset_in_place`).
    InPlace,
}

/// Runtime pool managing reusable PyRuntime instances.
pub struct PyRuntimePool {
    inner: Arc<PoolInner>,
}

struct PoolInner {
    config: PoolConfig,
    state: Mutex<PoolState>,
    condvar: Condvar,
    next_id: AtomicU64,
}

struct PoolState {
    queue: VecDeque<ManagedRuntime>,
    total: usize,
}

struct ManagedRuntime {
    id: String,
    runtime: PyRuntime,
}

impl PyRuntimePool {
    #[allow(clippy::arc_with_non_send_sync)]
    /// Construct a pool that lazily creates runtimes up to the configured limit.
    pub fn new(config: PoolConfig) -> Result<Self> {
        Ok(Self {
            inner: Arc::new(PoolInner {
                next_id: AtomicU64::new(1),
                config,
                state: Mutex::new(PoolState {
                    queue: VecDeque::new(),
                    total: 0,
                }),
                condvar: Condvar::new(),
            }),
        })
    }

    /// Check out a runtime handle, blocking until one is available.
    pub fn checkout(&self) -> Result<PooledRuntime> {
        let mut state = self.inner.state.lock();
        loop {
            if let Some(managed) = state.queue.pop_front() {
                drop(state);
                return Ok(PooledRuntime::new(self.inner.clone(), managed));
            }

            if state.total < self.inner.config.max_runtimes {
                state.total += 1;
                let runtime_id =
                    format!("rt-{}", self.inner.next_id.fetch_add(1, Ordering::Relaxed));
                drop(state);

                let mut runtime = PyRuntime::new(self.inner.config.runtime_config.clone())?;
                runtime.set_runtime_id(runtime_id.clone());
                info!(
                    target: "aardvark::runtime",
                    runtime_id = runtime_id.as_str(),
                    "runtime.new"
                );
                return Ok(PooledRuntime::new(
                    self.inner.clone(),
                    ManagedRuntime {
                        id: runtime_id,
                        runtime,
                    },
                ));
            }

            self.inner.condvar.wait(&mut state);
        }
    }
}

/// Handle returned by the pool; on drop the runtime is reset (if needed) and returned.
pub struct PooledRuntime {
    inner: Arc<PoolInner>,
    managed: Option<ManagedRuntime>,
}

impl PooledRuntime {
    fn new(inner: Arc<PoolInner>, managed: ManagedRuntime) -> Self {
        let span = info_span!(
            target: "aardvark::runtime",
            "runtime.pool.checkout",
            runtime_id = managed.id.as_str()
        );
        let _guard = span.enter();
        info!(
            target: "aardvark::runtime",
            runtime_id = managed.id.as_str(),
            "checkout"
        );
        Self {
            inner,
            managed: Some(managed),
        }
    }

    /// Provides mutable access to the underlying runtime.
    ///
    /// Callers typically prepare a session, run it, and then let the handle drop
    /// so the runtime can be reset and returned to the pool.
    pub fn runtime(&mut self) -> &mut PyRuntime {
        &mut self
            .managed
            .as_mut()
            .expect("pooled runtime already returned")
            .runtime
    }

    /// Returns the runtime identifier used for tracing spans.
    pub fn runtime_id(&self) -> &str {
        self.managed
            .as_ref()
            .map(|managed| managed.id.as_str())
            .unwrap_or("<returned>")
    }
}

impl Drop for PooledRuntime {
    fn drop(&mut self) {
        if let Some(mut managed) = self.managed.take() {
            let runtime_id = managed.id.clone();
            let reset_needed = matches!(
                self.inner.config.runtime_config.reset_policy,
                ResetPolicy::Manual
            );

            if reset_needed {
                let span = info_span!(
                    target: "aardvark::runtime",
                    "runtime.reset",
                    runtime_id = runtime_id.as_str(),
                    mode = ?self.inner.config.reset_mode
                );
                let _guard = span.enter();
                let reset_result = match self.inner.config.reset_mode {
                    PoolResetMode::RecreateEngine => managed.runtime.reset_to_snapshot(),
                    PoolResetMode::InPlace => managed.runtime.reset_in_place(),
                };
                if let Err(err) = reset_result {
                    warn!(
                        target: "aardvark::runtime",
                        runtime_id = runtime_id.as_str(),
                        error = %err,
                        "reset failed; dropping runtime"
                    );
                    // Drop this runtime and reduce total count.
                    let mut state = self.inner.state.lock();
                    state.total = state.total.saturating_sub(1);
                    self.inner.condvar.notify_one();
                    return;
                }
                info!(
                    target: "aardvark::runtime",
                    runtime_id = runtime_id.as_str(),
                    mode = ?self.inner.config.reset_mode,
                    "reset complete"
                );
            }

            let span = info_span!(
                target: "aardvark::runtime",
                "runtime.pool.return",
                runtime_id = runtime_id.as_str()
            );
            let _guard = span.enter();
            info!(
                target: "aardvark::runtime",
                runtime_id = runtime_id.as_str(),
                "returning runtime to pool"
            );

            let mut state = self.inner.state.lock();
            state.queue.push_back(ManagedRuntime {
                id: runtime_id,
                runtime: managed.runtime,
            });
            drop(state);
            self.inner.condvar.notify_one();
        }
    }
}
