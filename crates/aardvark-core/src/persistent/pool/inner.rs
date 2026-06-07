use super::*;

impl BundlePoolInner {
    #[allow(clippy::arc_with_non_send_sync)]
    pub(super) fn new(
        artifact: Arc<BundleArtifact>,
        mut options: PoolOptions,
    ) -> Result<Arc<Self>> {
        options.validate()?;
        options
            .isolate
            .runtime
            .apply_manifest_pyodide_distribution_profile(artifact.pyodide_distribution_profile())?;
        let hooks = options.lifecycle_hooks.clone().unwrap_or_default();
        let inner = Arc::new(Self {
            artifact,
            options: Mutex::new(options),
            state: Mutex::new(PoolState::new()),
            prewarmed_handlers: Mutex::new(Vec::new()),
            condvar: Condvar::new(),
            stats: Arc::new(PoolStatsTracker::new()?),
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

    pub(super) fn ensure_min_isolates(&self, target: usize) -> Result<()> {
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

    pub(super) fn shrink_to(&self, target: usize) -> Result<()> {
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
            isolates.sort_by_key(|item| Reverse(item.0));

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

    pub(super) fn start_telemetry(self: &Arc<Self>) {
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

    pub(super) fn prewarm_handler(&self, handler: &HandlerSession) -> Result<()> {
        let slots: Vec<Arc<IsolateSlot>> = {
            let state = self.state.lock();
            state
                .isolates
                .iter()
                .filter_map(|slot| slot.as_ref().cloned())
                .collect()
        };

        for slot in slots {
            let mut isolate = slot.isolate.lock();
            isolate.prewarm_handler(handler)?;
        }

        self.register_prewarmed_handler(handler.descriptor())?;
        Ok(())
    }

    pub(super) fn register_prewarmed_handler(
        &self,
        descriptor: &InvocationDescriptor,
    ) -> Result<()> {
        let key = descriptor_registry_key(descriptor)?;
        let mut handlers = self.prewarmed_handlers.lock();
        for existing in handlers.iter() {
            if descriptor_registry_key(existing)? == key {
                return Ok(());
            }
        }
        handlers.push(descriptor.clone());
        Ok(())
    }

    pub(super) fn prewarmed_handler_descriptors(&self) -> Vec<InvocationDescriptor> {
        self.prewarmed_handlers.lock().clone()
    }

    pub(super) fn acquire_slot(self: &Arc<Self>) -> Result<(SlotGuard, Duration)> {
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
                    .ok_or_else(|| {
                        PyRunnerError::Internal(format!(
                            "pool idle index {index} did not reference a live isolate"
                        ))
                    })?
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

    pub(super) fn acquire_all_idle_slots(self: &Arc<Self>) -> Result<Vec<SlotGuard>> {
        let mut state = self.state.lock();
        if state.shutdown {
            return Err(PyRunnerError::PoolShuttingDown);
        }
        if state.creating > 0 {
            return Err(PyRunnerError::Validation(format!(
                "cannot warm all isolates while {} isolates are still starting",
                state.creating
            )));
        }
        let busy = state.active.saturating_sub(state.idle.len());
        if busy > 0 {
            return Err(PyRunnerError::Validation(format!(
                "cannot warm all isolates while {busy} isolates are busy",
            )));
        }

        let mut indices = std::mem::take(&mut state.idle);
        indices.sort_unstable();
        let mut guards = Vec::with_capacity(indices.len());
        for index in indices {
            let slot = state
                .isolates
                .get(index)
                .and_then(|slot| slot.as_ref())
                .ok_or_else(|| {
                    PyRunnerError::Internal(format!(
                        "pool idle index {index} did not reference a live isolate"
                    ))
                })?
                .clone();
            self.metrics.dec_idle();
            guards.push(SlotGuard::new(self.clone(), index, slot));
        }
        Ok(guards)
    }

    pub(super) fn release_slot(&self, index: usize) {
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
    pub(super) fn spawn_isolate(&self, add_to_idle: bool) -> Result<SlotEntry> {
        let options_snapshot = { self.options.lock().clone() };

        let reserved_index = {
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
            for descriptor in self.prewarmed_handler_descriptors() {
                let handler = handle.prepare_handler(Some(descriptor));
                isolate.prewarm_handler(&handler)?;
            }
            Ok(isolate)
        })();

        match creation {
            Ok(isolate) => {
                let isolate_id = self.isolate_seq.fetch_add(1, Ordering::Relaxed);
                let slot = Arc::new(IsolateSlot::new(isolate_id, isolate));

                let active_after = {
                    let mut state = self.state.lock();
                    state.creating = state.creating.saturating_sub(1);
                    state.isolates[reserved_index] = Some(slot.clone());
                    state.active += 1;
                    self.metrics.inc_active();
                    if add_to_idle {
                        state.idle.push(reserved_index);
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
                    index: reserved_index,
                    slot,
                })
            }
            Err(err) => {
                let mut state = self.state.lock();
                state.creating = state.creating.saturating_sub(1);
                if reserved_index + 1 == state.isolates.len() {
                    state.isolates.pop();
                } else {
                    state.isolates[reserved_index] = None;
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

impl BundlePoolInner {
    pub(super) fn call_hook_isolate_started(&self, isolate_id: IsolateId, config: &IsolateConfig) {
        if let Some(callback) = &self.hooks.on_isolate_started {
            callback(isolate_id, config);
        }
    }

    pub(super) fn call_hook_isolate_recycled(&self, isolate_id: IsolateId, reason: &RecycleReason) {
        if let Some(callback) = &self.hooks.on_isolate_recycled {
            callback(isolate_id, reason);
        }
    }

    pub(super) fn call_hook_call_started(&self, context: &CallContext) {
        if let Some(callback) = &self.hooks.on_call_started {
            callback(context);
        }
    }

    pub(super) fn call_hook_call_finished<'a>(
        &self,
        context: &CallContext,
        outcome: CallOutcome<'a>,
    ) {
        if let Some(callback) = &self.hooks.on_call_finished {
            callback(context, outcome);
        }
    }

    pub(super) fn current_limits(&self) -> (Option<u64>, Option<u64>) {
        let opts = self.options.lock();
        (opts.memory_limit_kib, opts.heap_limit_kib)
    }

    pub(super) fn ensure_desired_isolates(&self) {
        let desired = { self.options.lock().desired_size };
        if let Err(err) = self.ensure_min_isolates(desired) {
            warn!(target: "aardvark::pool", error = %err, "failed to replenish isolates after quarantine");
        }
    }

    pub(super) fn quarantine_slot(&self, index: usize, reason: RecycleReason) -> Option<IsolateId> {
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
