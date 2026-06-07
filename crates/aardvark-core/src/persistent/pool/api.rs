use super::*;

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

    /// Prepares handler-specific caches on every currently-created isolate.
    ///
    /// This intentionally moves import resolution and adapter installation into
    /// pool warmup so the first live request does not pay that setup cost.
    pub fn prewarm_handler(&self, handler: &HandlerSession) -> Result<()> {
        self.inner.prewarm_handler(handler)
    }

    /// Prepares and prewarms the bundle's default handler for hot invocation.
    ///
    /// Prefer this for latency-sensitive pools when startup/preload cost can be
    /// paid before serving live requests.
    pub fn prepare_default_handler(&self) -> Result<HandlerSession> {
        self.prepare_handler(None)
    }

    /// Prepares and prewarms a handler using an optional descriptor override.
    pub fn prepare_handler(
        &self,
        descriptor: Option<InvocationDescriptor>,
    ) -> Result<HandlerSession> {
        let handler = self.handle().prepare_handler(descriptor);
        self.prewarm_handler(&handler)?;
        Ok(handler)
    }

    /// Invokes a handler using JSON adapters.
    pub fn call_json(
        &self,
        handler: &HandlerSession,
        input: Option<JsonValue>,
    ) -> Result<crate::ExecutionOutcome> {
        self.call_json_input(handler, input.map(JsonInput::Value))
    }

    /// Invokes a handler using a prepared JSON adapter input.
    pub fn call_json_input(
        &self,
        handler: &HandlerSession,
        input: Option<JsonInput>,
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

    /// Executes a JSON handler during explicit startup warmup.
    ///
    /// This is intentionally separate from `prepare_handler`: it runs caller
    /// code and should only be used when startup side effects are acceptable.
    pub fn warm_json(
        &self,
        handler: &HandlerSession,
        input: Option<JsonValue>,
    ) -> Result<crate::ExecutionOutcome> {
        self.call_json(handler, input)
    }

    /// Executes a JSON handler with a prepared input during explicit startup warmup.
    pub fn warm_json_input(
        &self,
        handler: &HandlerSession,
        input: Option<JsonInput>,
    ) -> Result<crate::ExecutionOutcome> {
        self.call_json_input(handler, input)
    }

    /// Executes a JSON warmup once on every currently-created idle isolate.
    ///
    /// The pool must be idle. This is intended for deploy-time warmup when the
    /// host can pay representative handler execution before accepting traffic.
    pub fn warm_all_json(
        &self,
        handler: &HandlerSession,
        input: Option<JsonValue>,
    ) -> Result<Vec<crate::ExecutionOutcome>> {
        self.warm_all_json_input(handler, input.map(JsonInput::Value))
    }

    /// Executes a JSON warmup with a prepared input once on every currently-created idle isolate.
    pub fn warm_all_json_input(
        &self,
        handler: &HandlerSession,
        input: Option<JsonInput>,
    ) -> Result<Vec<crate::ExecutionOutcome>> {
        self.warm_all_with(handler, |_| Ok(CallInvocation::Json(input.clone())))
    }

    /// Executes a RawCtx handler during explicit startup warmup.
    ///
    /// This is intentionally separate from `prepare_handler`: it runs caller
    /// code and should only be used when startup side effects are acceptable.
    pub fn warm_rawctx(
        &self,
        handler: &HandlerSession,
        inputs: Vec<RawCtxInput>,
    ) -> Result<crate::ExecutionOutcome> {
        self.call_rawctx(handler, inputs)
    }

    /// Executes a RawCtx warmup once on every currently-created idle isolate.
    ///
    /// Inputs are cloned for each isolate. Prefer `warm_all_rawctx_with` when
    /// the host can cheaply build owned buffers per isolate.
    pub fn warm_all_rawctx(
        &self,
        handler: &HandlerSession,
        inputs: Vec<RawCtxInput>,
    ) -> Result<Vec<crate::ExecutionOutcome>> {
        self.warm_all_rawctx_with(handler, |_| Ok(inputs.clone()))
    }

    /// Executes a RawCtx warmup once on every currently-created idle isolate.
    pub fn warm_all_rawctx_with<F>(
        &self,
        handler: &HandlerSession,
        mut inputs_for_isolate: F,
    ) -> Result<Vec<crate::ExecutionOutcome>>
    where
        F: FnMut(usize) -> Result<Vec<RawCtxInput>>,
    {
        self.warm_all_with(handler, |index| {
            Ok(CallInvocation::RawCtx(inputs_for_isolate(index)?))
        })
    }

    /// Executes a default-strategy handler during explicit startup warmup.
    ///
    /// This is intentionally separate from `prepare_handler`: it runs caller
    /// code and should only be used when startup side effects are acceptable.
    pub fn warm_default(&self, handler: &HandlerSession) -> Result<crate::ExecutionOutcome> {
        self.call_default(handler)
    }

    /// Executes a default-strategy warmup once on every currently-created idle isolate.
    pub fn warm_all_default(
        &self,
        handler: &HandlerSession,
    ) -> Result<Vec<crate::ExecutionOutcome>> {
        self.warm_all_with(handler, |_| Ok(CallInvocation::Default))
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

    /// Adjusts the maximum pool size.
    ///
    /// It fails if any isolate that would have to be removed is busy.
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
    ///
    /// Setting this to zero makes the pool lazy.
    pub fn set_desired_size(&self, desired_size: usize) -> Result<()> {
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
        let (guard, wait_duration) = self.inner.acquire_slot()?;
        self.call_acquired(guard, wait_duration, handler, invocation)
    }

    fn warm_all_with<F>(
        &self,
        handler: &HandlerSession,
        mut invocation_for_isolate: F,
    ) -> Result<Vec<crate::ExecutionOutcome>>
    where
        F: FnMut(usize) -> Result<CallInvocation>,
    {
        let desired = { self.inner.options.lock().desired_size.max(1) };
        self.inner.ensure_min_isolates(desired)?;
        let guards = self.inner.acquire_all_idle_slots()?;
        let mut outcomes = Vec::with_capacity(guards.len());
        for (index, guard) in guards.into_iter().enumerate() {
            let invocation = invocation_for_isolate(index)?;
            outcomes.push(self.call_acquired(guard, Duration::ZERO, handler, invocation)?);
        }
        Ok(outcomes)
    }

    fn call_acquired(
        &self,
        mut guard: SlotGuard,
        wait_duration: Duration,
        handler: &HandlerSession,
        invocation: CallInvocation,
    ) -> Result<crate::ExecutionOutcome> {
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
                CallInvocation::Json(input) => handler.invoke_json_input(&mut isolate, input),
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
    Json(Option<JsonInput>),
    RawCtx(Vec<RawCtxInput>),
}

#[cfg(target_os = "linux")]
fn current_rss_kib() -> Option<u64> {
    let mut file = File::open("/proc/self/statm").ok()?;
    let mut contents = String::new();
    file.read_to_string(&mut contents).ok()?;
    let mut parts = contents.split_whitespace();
    parts.next()?; // skip total
    let resident_pages: u64 = parts.next()?.parse().ok()?;
    // SAFETY: `sysconf(_SC_PAGESIZE)` has no pointer arguments and does not
    // impose Rust-side aliasing or initialization requirements.
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as u64;
    Some(resident_pages.saturating_mul(page_size) / 1024)
}

#[cfg(target_os = "macos")]
fn current_rss_kib() -> Option<u64> {
    use std::mem::MaybeUninit;
    unsafe extern "C" {
        #[link_name = "mach_task_self_"]
        static MACH_TASK_SELF: libc::mach_port_t;
    }
    let mut info = MaybeUninit::<libc::mach_task_basic_info>::uninit();
    let task = unsafe {
        // SAFETY: `MACH_TASK_SELF` is provided by macOS libSystem as the
        // current task port value and is read-only from Rust's perspective.
        MACH_TASK_SELF
    };
    let mut count = libc::MACH_TASK_BASIC_INFO_COUNT;
    // SAFETY: `info` points to writable storage for `mach_task_basic_info`;
    // `count` is initialized as required by `task_info`, and `assume_init`
    // happens only after `KERN_SUCCESS`.
    unsafe {
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
