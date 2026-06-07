use super::*;

impl AardvarkRuntime {
    /// Runs a prepared session using the default invocation strategy for the selected language.
    pub fn run_session(&mut self, session: &PySession) -> Result<ExecutionOutcome> {
        let language = session
            .descriptor()
            .language
            .unwrap_or(self.config.default_language);
        match language {
            RuntimeLanguage::Python => {
                let mut strategy = DefaultInvocationStrategy;
                self.run_session_with_strategy(session, &mut strategy)
            }
            RuntimeLanguage::JavaScript => {
                let mut strategy = JavaScriptInvocationStrategy;
                self.run_session_with_strategy(session, &mut strategy)
            }
        }
    }

    /// Runs a prepared session with a caller-provided invocation strategy.
    pub fn run_session_with_strategy<S: PyInvocationStrategy>(
        &mut self,
        session: &PySession,
        strategy: &mut S,
    ) -> Result<ExecutionOutcome> {
        self.run_session_with_strategy_and_cleanup(session, strategy, RuntimeCleanupMode::Full)
    }

    pub(crate) fn run_session_with_strategy_and_cleanup<S: PyInvocationStrategy>(
        &mut self,
        session: &PySession,
        strategy: &mut S,
        cleanup_mode: RuntimeCleanupMode,
    ) -> Result<ExecutionOutcome> {
        let descriptor = session.descriptor();
        let language = descriptor.language.unwrap_or(self.config.default_language);
        let invocation_start = Instant::now();
        let entrypoint_owned = descriptor.entrypoint().to_owned();
        let cleanup_entrypoint = if language == RuntimeLanguage::Python {
            Some(entrypoint_owned.as_str())
        } else {
            None
        };
        let limits = self.effective_limits(descriptor);
        info!(
            target: "aardvark::budget",
            runtime_id = self.runtime_id_str(),
            entrypoint = descriptor.entrypoint(),
            limits.wall_ms = limits.wall_ms.unwrap_or(0),
            limits.heap_mb = limits.heap_mb.unwrap_or(0),
            limits.cpu_ms = limits.cpu_ms.unwrap_or(0),
            strategy = strategy.name(),
            "applying descriptor limits"
        );

        let heap_limit_bytes = limits.heap_mb.map(bytes_from_mib);
        if let (Some(limit_bytes), Some(limit_mb)) = (heap_limit_bytes, limits.heap_mb) {
            let used_before = self.engine_mut().js_mut().heap_used_bytes();
            if used_before > limit_bytes {
                warn!(
                    target: "aardvark::budget",
                    runtime_id = self.runtime_id_str(),
                    heap.used_bytes = used_before,
                    heap.limit_bytes = limit_bytes,
                    "heap usage already exceeds descriptor limit before execution"
                );
                self.cleanup_filesystem();
                return Err(PyRunnerError::HeapLimitExceeded {
                    requested_mb: limit_mb,
                });
            }
        }

        {
            let js = self.engine_mut().js_mut();
            js.clear_network_contacts();
            js.clear_network_denied();
            js.clear_filesystem_events();
        }

        let mut watchdog = self.arm_watchdog(limits.wall_ms);
        let cpu_start_ns = thread_cpu_time_ns();

        let invoke_start = Instant::now();
        let prepare_duration = invoke_start.duration_since(invocation_start);
        let runtime_id_owned = self.runtime_id_str().to_owned();
        let strategy_result = {
            let js = self.engine_mut().js_mut();
            let mut ctx = InvocationContext::new(session, js, language);
            Self::execute_strategy(strategy, &mut ctx, &runtime_id_owned)?
        };

        let timeout_triggered = if let Some(guard) = watchdog.take() {
            guard.complete(self.engine_mut().js_mut())
        } else {
            false
        };

        let cpu_end_ns = thread_cpu_time_ns();
        let cpu_ms_used = match (cpu_start_ns, cpu_end_ns) {
            (Some(start), Some(end)) => Some(ns_to_ms(end.saturating_sub(start))),
            _ => None,
        };
        let (
            filesystem_bytes_written,
            network_contacts_raw,
            network_denied_raw,
            filesystem_violations_raw,
            heap_used_bytes,
        ) = {
            let js = self.engine_mut().js_mut();
            let usage = js.filesystem_usage_bytes().ok();
            let contacts = js.drain_network_contacts();
            let denied = js.drain_network_denied();
            let fs_violations = js.drain_filesystem_violations();
            let heap = js.heap_used_bytes() as u64;
            (usage, contacts, denied, fs_violations, heap)
        };
        let network_hosts_contacted: Vec<NetworkHostContact> = network_contacts_raw
            .into_iter()
            .map(|record| NetworkHostContact {
                host: record.host,
                port: record.port,
                https: record.https,
            })
            .collect();
        let network_hosts_blocked: Vec<NetworkDeniedHost> = network_denied_raw
            .into_iter()
            .map(|record| NetworkDeniedHost {
                host: record.host,
                port: record.port,
                https_required: record.https_required,
                reason: record.reason,
            })
            .collect();
        let filesystem_violations: Vec<FilesystemViolation> = filesystem_violations_raw
            .into_iter()
            .map(|record| FilesystemViolation {
                path: record.path,
                message: record.message,
            })
            .collect();
        let mut collected = CollectedDiagnostics {
            cpu_ms_used,
            filesystem_bytes_written,
            network_hosts_contacted,
            network_hosts_blocked,
            filesystem_violations,
            reset_summary: self.pending_reset_summary.take(),
            queue_wait_ms: None,
            prepare_ms: None,
            cleanup_ms: None,
            py_heap_kib: Some(heap_used_bytes / 1024),
            rss_kib_before: None,
            rss_kib_after: None,
        };
        collected.prepare_ms = Some(prepare_duration.as_millis() as u64);

        Self::emit_diagnostics_events(&collected, self.runtime_id_str(), descriptor.entrypoint());

        if timeout_triggered {
            warn!(
                target: "aardvark::budget",
                runtime_id = self.runtime_id_str(),
                entrypoint = descriptor.entrypoint(),
                limits.wall_ms = limits.wall_ms.unwrap_or(0),
                "wall-clock limit exceeded"
            );
            let diagnostics = Self::make_diagnostics(
                strategy_result.as_ref().ok().map(|res| &res.execution),
                &collected,
            );
            return self.finish_with_cleanup(
                ExecutionOutcome::failure(
                    FailureKind::TimeoutExceeded {
                        requested_ms: limits.wall_ms.unwrap_or_default(),
                    },
                    diagnostics,
                ),
                cleanup_entrypoint,
                cleanup_mode,
            );
        }
        if let Some(limit_ms) = limits.cpu_ms {
            if let Some(used_ms) = collected.cpu_ms_used {
                if used_ms > limit_ms {
                    warn!(
                        target: "aardvark::budget",
                        runtime_id = self.runtime_id_str(),
                        entrypoint = descriptor.entrypoint(),
                        limits.cpu_ms = limit_ms,
                        cpu.used_ms = used_ms,
                        "cpu limit exceeded"
                    );
                    let diagnostics = Self::make_diagnostics(
                        strategy_result.as_ref().ok().map(|res| &res.execution),
                        &collected,
                    );
                    return self.finish_with_cleanup(
                        ExecutionOutcome::failure(
                            FailureKind::CpuLimitExceeded {
                                requested_ms: limit_ms,
                                used_ms,
                            },
                            diagnostics,
                        ),
                        cleanup_entrypoint,
                        cleanup_mode,
                    );
                } else {
                    info!(
                        target: "aardvark::budget",
                        runtime_id = self.runtime_id_str(),
                        entrypoint = descriptor.entrypoint(),
                        limits.cpu_ms = limit_ms,
                        cpu.used_ms = used_ms,
                        "cpu usage recorded"
                    );
                }
            }
        }
        if let (Some(limit_bytes), Some(limit_mb)) = (heap_limit_bytes, limits.heap_mb) {
            let used_after = self.engine_mut().js_mut().heap_used_bytes();
            if used_after > limit_bytes {
                warn!(
                    target: "aardvark::budget",
                    runtime_id = self.runtime_id_str(),
                    entrypoint = descriptor.entrypoint(),
                    heap.used_bytes = used_after,
                    heap.limit_bytes = limit_bytes,
                    "heap usage exceeded"
                );
                let diagnostics = Self::make_diagnostics(
                    strategy_result.as_ref().ok().map(|res| &res.execution),
                    &collected,
                );
                return self.finish_with_cleanup(
                    ExecutionOutcome::failure(
                        FailureKind::HeapLimitExceeded {
                            requested_mb: limit_mb,
                        },
                        diagnostics,
                    ),
                    cleanup_entrypoint,
                    cleanup_mode,
                );
            }
        }

        let mut outcome = match strategy_result {
            Ok(result) => Self::finalize_success(result, descriptor, &collected),
            Err(err) => {
                let diagnostics = Self::make_diagnostics(None, &collected);
                ExecutionOutcome::failure(
                    FailureKind::AdapterError {
                        message: err.to_string(),
                    },
                    diagnostics,
                )
            }
        };

        if matches!(self.config.reset_policy, ResetPolicy::AfterInvocation) {
            let span = info_span!(
                target: "aardvark::runtime",
                "runtime.reset",
                runtime_id = self.runtime_id_str()
            );
            let _guard = span.enter();
            match self.reset_to_snapshot() {
                Ok(_) => {
                    info!(
                        target: "aardvark::runtime",
                        runtime_id = self.runtime_id_str(),
                        "reset complete"
                    );
                }
                Err(err) => {
                    warn!(
                        target: "aardvark::runtime",
                        runtime_id = self.runtime_id_str(),
                        error = %err,
                        "reset failed"
                    );
                    outcome = ExecutionOutcome::failure(
                        FailureKind::Other {
                            message: format!("runtime reset failed: {err}"),
                        },
                        outcome.diagnostics.clone(),
                    );
                }
            }
        }

        let cleanup_start = Instant::now();
        let mut outcome = self.finish_with_cleanup(outcome, cleanup_entrypoint, cleanup_mode)?;
        let cleanup_duration = cleanup_start.elapsed();
        outcome.diagnostics.cleanup_ms = Some(cleanup_duration.as_millis() as u64);
        if outcome.diagnostics.prepare_ms.is_none() {
            outcome.diagnostics.prepare_ms = collected.prepare_ms;
        }
        if outcome.diagnostics.py_heap_kib.is_none() {
            outcome.diagnostics.py_heap_kib = collected.py_heap_kib;
        }
        Ok(outcome)
    }
}
