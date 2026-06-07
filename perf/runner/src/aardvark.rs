use super::report::*;
use super::rss::current_rss_mib;
use super::scenarios::*;
use super::*;

mod validation;

use validation::validate_aardvark_outcome;

pub(super) struct AardvarkBenchOptions<'a> {
    pub(super) iterations: usize,
    pub(super) profile: LoadProfile,
    pub(super) include_samples: bool,
    pub(super) pyodide_profile: Option<&'a str>,
    pub(super) warm_preimports: &'a [String],
    pub(super) manifest_preload_imports: &'a [String],
    pub(super) setup_pool_desired_size: usize,
}

pub(super) fn bench_aardvark(
    scenario: Scenario,
    mode: Mode,
    options: AardvarkBenchOptions<'_>,
) -> Result<BenchResult> {
    let iterations = options.iterations;
    let profile = options.profile;
    let include_samples = options.include_samples;
    let pyodide_profile = options.pyodide_profile;
    let warm_preimports = options.warm_preimports;
    let setup_pool_desired_size = options.setup_pool_desired_size;
    let invocation = mode
        .invocation_kind()
        .ok_or_else(|| anyhow!("mode '{}' is not an Aardvark variant", mode.name()))?;
    let bench_context = AardvarkBenchContext {
        scenario,
        profile,
        invocation,
    };
    let path = mode
        .path_kind()
        .ok_or_else(|| anyhow!("mode '{}' is missing a path kind", mode.name()))?;
    let cleanup_kind = mode.cleanup_kind();
    let mut applied_cleanup = cleanup_kind;

    let python_source = scenario_source(scenario);
    let manifest = scenario_manifest(
        scenario,
        invocation,
        options.pyodide_profile,
        options.manifest_preload_imports,
    );
    let bundle_bytes = build_bundle_bytes(&python_source, manifest.as_bytes())?;
    let bundle = Bundle::from_zip_bytes(&bundle_bytes)?;
    let descriptor = if mode.uses_direct_rawctx_contract() && mode.captures_stdio() {
        None
    } else {
        descriptor_for(scenario, invocation, profile, mode)
    };

    let json_input = json_input_for(scenario, profile);
    let raw_inputs = Arc::new(rawctx_inputs_for(scenario, profile, mode)?);

    let mut prepare = Vec::with_capacity(iterations);
    let mut run = Vec::with_capacity(iterations);
    let mut total = Vec::with_capacity(iterations);
    let mut cold_total_stats: Option<TimingStats> = None;
    let mut cold_prepare_stats: Option<TimingStats> = None;
    let mut cold_run_stats: Option<TimingStats> = None;
    let mut setup_breakdown_buckets = SetupBreakdownBuckets::default();
    let mut has_setup_breakdown = false;

    match path {
        PathKind::Cold => {
            for _ in 0..iterations {
                let mut runtime = PyRuntime::new(runtime_config_for(pyodide_profile)?)?;
                let mut buckets = TimingBuckets {
                    prepare: &mut prepare,
                    run: &mut run,
                    total: &mut total,
                };
                execute_iteration(
                    bench_context,
                    &mut runtime,
                    descriptor.as_ref(),
                    &bundle,
                    json_input.clone(),
                    raw_inputs.as_ref(),
                    &mut buckets,
                )?;
            }
        }
        PathKind::Warm => {
            let (warm_state, cold_total, cold_prepare, cold_run) = capture_warm_state(
                bench_context,
                &bundle,
                descriptor.as_ref(),
                json_input.clone(),
                raw_inputs.as_ref(),
                pyodide_profile,
                warm_preimports,
            )?;
            cold_total_stats = Some(cold_total);
            cold_prepare_stats = Some(cold_prepare);
            cold_run_stats = Some(cold_run);
            let mut config = runtime_config_for(pyodide_profile)?;
            config.warm_state = Some(warm_state);
            for _ in 0..iterations {
                let mut runtime = PyRuntime::new(config.clone())?;
                let mut buckets = TimingBuckets {
                    prepare: &mut prepare,
                    run: &mut run,
                    total: &mut total,
                };
                execute_iteration(
                    bench_context,
                    &mut runtime,
                    descriptor.as_ref(),
                    &bundle,
                    json_input.clone(),
                    raw_inputs.as_ref(),
                    &mut buckets,
                )?;
            }
        }
        PathKind::ResetInPlace => {
            let (warm_state, cold_total, cold_prepare, cold_run) = capture_warm_state(
                bench_context,
                &bundle,
                descriptor.as_ref(),
                json_input.clone(),
                raw_inputs.as_ref(),
                pyodide_profile,
                warm_preimports,
            )?;
            cold_total_stats = Some(cold_total);
            cold_prepare_stats = Some(cold_prepare);
            cold_run_stats = Some(cold_run);
            let mut runtime_config = runtime_config_for(pyodide_profile)?;
            runtime_config.warm_state = Some(warm_state);
            runtime_config.reset_policy = ResetPolicy::Manual;
            let pool = PyRuntimePool::new(PoolConfig {
                max_runtimes: 1,
                runtime_config,
                reset_mode: PoolResetMode::InPlace,
            })?;
            for _ in 0..iterations {
                let mut handle = pool.checkout()?;
                {
                    let runtime = handle.runtime();
                    let mut buckets = TimingBuckets {
                        prepare: &mut prepare,
                        run: &mut run,
                        total: &mut total,
                    };
                    execute_iteration(
                        bench_context,
                        runtime,
                        descriptor.as_ref(),
                        &bundle,
                        json_input.clone(),
                        raw_inputs.as_ref(),
                        &mut buckets,
                    )?;
                }
                drop(handle);
            }
        }
        PathKind::FirstLive => {
            let (warm_state, cold_total, cold_prepare, cold_run) = capture_warm_state(
                bench_context,
                &bundle,
                descriptor.as_ref(),
                json_input.clone(),
                raw_inputs.as_ref(),
                pyodide_profile,
                warm_preimports,
            )?;
            cold_total_stats = Some(cold_total);
            cold_prepare_stats = Some(cold_prepare);
            cold_run_stats = Some(cold_run);

            let bench_cleanup = cleanup_kind.unwrap_or(CleanupKind::SharedBuffersOnly);
            applied_cleanup = Some(bench_cleanup);
            for _ in 0..iterations {
                let mut isolate_config = IsolateConfig {
                    runtime: runtime_config_for(pyodide_profile)?,
                    ..IsolateConfig::default()
                };
                isolate_config.runtime.warm_state = Some(warm_state.clone());
                isolate_config.cleanup = bench_cleanup.to_cleanup_mode();

                if mode.uses_warmed_host_registry() {
                    has_setup_breakdown = true;
                    let setup_start = Instant::now();
                    let registry_start = Instant::now();
                    let descriptor = descriptor
                        .clone()
                        .unwrap_or_else(|| InvocationDescriptor::new("main:entrypoint"));
                    let warmup = match invocation {
                        InvocationKind::Json => {
                            WarmedBundleHostWarmup::json_input(json_input.clone())
                        }
                        InvocationKind::RawCtx => WarmedBundleHostWarmup::rawctx(
                            rawctx_inputs_for_call(scenario, profile, raw_inputs.as_ref(), mode)?,
                        ),
                    };
                    let registry = WarmedBundleHostRegistry::new(
                        WarmedBundleHostOptions::pooled(PoolOptions {
                            isolate: isolate_config,
                            desired_size: setup_pool_desired_size,
                            max_size: setup_pool_desired_size.max(1),
                            telemetry_interval: None,
                            ..PoolOptions::default()
                        })
                        .with_descriptor(descriptor)
                        .with_warmup(warmup),
                    );
                    let host = registry.host_for_bytes(&bundle_bytes)?;
                    setup_breakdown_buckets
                        .registry_init
                        .push(registry_start.elapsed());
                    setup_breakdown_buckets.artifact_parse.push(Duration::ZERO);
                    setup_breakdown_buckets.pool_create.push(Duration::ZERO);
                    setup_breakdown_buckets.handler_prepare.push(Duration::ZERO);

                    let setup_elapsed = setup_start.elapsed();
                    let raw_inputs_for_iteration = if matches!(invocation, InvocationKind::RawCtx) {
                        Some(rawctx_inputs_for_call(
                            scenario,
                            profile,
                            raw_inputs.as_ref(),
                            mode,
                        )?)
                    } else {
                        None
                    };
                    let live_start = Instant::now();
                    let outcome = match invocation {
                        InvocationKind::Json => host.call_json_input(json_input.clone())?,
                        InvocationKind::RawCtx => {
                            host.call_rawctx(raw_inputs_for_iteration.unwrap_or_default())?
                        }
                    };
                    let live_elapsed = live_start.elapsed();

                    validate_aardvark_outcome(scenario, profile, invocation, &outcome)?;
                    prepare.push(setup_elapsed);
                    run.push(live_elapsed);
                    total.push(setup_elapsed + live_elapsed);
                    continue;
                }

                if mode.uses_warmed_host() {
                    has_setup_breakdown = true;
                    let setup_start = Instant::now();

                    setup_breakdown_buckets.registry_init.push(Duration::ZERO);

                    let artifact_start = Instant::now();
                    let artifact = BundleArtifact::from_bytes(&bundle_bytes)?;
                    setup_breakdown_buckets
                        .artifact_parse
                        .push(artifact_start.elapsed());

                    let host_start = Instant::now();
                    let descriptor = descriptor
                        .clone()
                        .unwrap_or_else(|| InvocationDescriptor::new("main:entrypoint"));
                    let host_options = WarmedBundleHostOptions::pooled(PoolOptions {
                        isolate: isolate_config,
                        desired_size: setup_pool_desired_size,
                        max_size: setup_pool_desired_size.max(1),
                        telemetry_interval: None,
                        ..PoolOptions::default()
                    })
                    .with_descriptor(descriptor);
                    let host = WarmedBundleHost::from_artifact(artifact, host_options)?;
                    setup_breakdown_buckets
                        .handler_prepare
                        .push(host_start.elapsed());

                    if mode.uses_pool_wide_warmup() {
                        let warm_all_start = Instant::now();
                        let outcomes = match invocation {
                            InvocationKind::Json => host
                                .warm_all(WarmedBundleHostWarmup::json_input(json_input.clone()))?,
                            InvocationKind::RawCtx => host.warm_all(
                                WarmedBundleHostWarmup::rawctx(rawctx_inputs_for_call(
                                    scenario,
                                    profile,
                                    raw_inputs.as_ref(),
                                    mode,
                                )?),
                            )?,
                        };
                        for outcome in &outcomes {
                            validate_aardvark_outcome(scenario, profile, invocation, outcome)?;
                        }
                        setup_breakdown_buckets
                            .warm_all
                            .push(warm_all_start.elapsed());
                    }

                    let setup_elapsed = setup_start.elapsed();
                    let raw_inputs_for_iteration = if matches!(invocation, InvocationKind::RawCtx) {
                        Some(rawctx_inputs_for_call(
                            scenario,
                            profile,
                            raw_inputs.as_ref(),
                            mode,
                        )?)
                    } else {
                        None
                    };
                    let live_start = Instant::now();
                    let outcome = match invocation {
                        InvocationKind::Json => host.call_json_input(json_input.clone())?,
                        InvocationKind::RawCtx => {
                            host.call_rawctx(raw_inputs_for_iteration.unwrap_or_default())?
                        }
                    };
                    let live_elapsed = live_start.elapsed();

                    validate_aardvark_outcome(scenario, profile, invocation, &outcome)?;
                    prepare.push(setup_elapsed);
                    run.push(live_elapsed);
                    total.push(setup_elapsed + live_elapsed);
                    continue;
                }

                let options = PoolOptions {
                    isolate: isolate_config,
                    desired_size: setup_pool_desired_size,
                    max_size: setup_pool_desired_size.max(1),
                    telemetry_interval: None,
                    ..PoolOptions::default()
                };

                has_setup_breakdown = true;
                let setup_start = Instant::now();

                let registry_start = Instant::now();
                let registry = BundlePoolRegistry::new(options)?;
                setup_breakdown_buckets
                    .registry_init
                    .push(registry_start.elapsed());

                let artifact_start = Instant::now();
                let artifact = BundleArtifact::from_bytes(&bundle_bytes)?;
                setup_breakdown_buckets
                    .artifact_parse
                    .push(artifact_start.elapsed());

                let pool_start = Instant::now();
                let _pool = registry.pool_for_artifact(artifact.clone())?;
                setup_breakdown_buckets
                    .pool_create
                    .push(pool_start.elapsed());

                let handler_start = Instant::now();
                let prepared =
                    registry.prepare_handler_for_artifact(artifact, descriptor.clone())?;
                setup_breakdown_buckets
                    .handler_prepare
                    .push(handler_start.elapsed());

                if mode.uses_pool_wide_warmup() {
                    let warm_all_start = Instant::now();
                    let outcomes = match invocation {
                        InvocationKind::Json => prepared.warm_all_json_input(json_input.clone())?,
                        InvocationKind::RawCtx => prepared.warm_all_rawctx_with(|_| {
                            rawctx_inputs_for_call(scenario, profile, raw_inputs.as_ref(), mode)
                                .map_err(|err| PyRunnerError::Validation(err.to_string()))
                        })?,
                    };
                    for outcome in &outcomes {
                        validate_aardvark_outcome(scenario, profile, invocation, outcome)?;
                    }
                    setup_breakdown_buckets
                        .warm_all
                        .push(warm_all_start.elapsed());
                }
                let setup_elapsed = setup_start.elapsed();

                let raw_inputs_for_iteration = if matches!(invocation, InvocationKind::RawCtx) {
                    Some(rawctx_inputs_for_call(
                        scenario,
                        profile,
                        raw_inputs.as_ref(),
                        mode,
                    )?)
                } else {
                    None
                };
                let live_start = Instant::now();
                let outcome = match invocation {
                    InvocationKind::Json => prepared.call_json_input(json_input.clone())?,
                    InvocationKind::RawCtx => {
                        prepared.call_rawctx(raw_inputs_for_iteration.unwrap_or_default())?
                    }
                };
                let live_elapsed = live_start.elapsed();

                validate_aardvark_outcome(scenario, profile, invocation, &outcome)?;
                prepare.push(setup_elapsed);
                run.push(live_elapsed);
                total.push(setup_elapsed + live_elapsed);
            }
        }
        PathKind::Persistent => {
            let (warm_state, cold_total, cold_prepare, cold_run) = capture_warm_state(
                bench_context,
                &bundle,
                descriptor.as_ref(),
                json_input.clone(),
                raw_inputs.as_ref(),
                pyodide_profile,
                warm_preimports,
            )?;
            cold_total_stats = Some(cold_total);
            cold_prepare_stats = Some(cold_prepare);
            cold_run_stats = Some(cold_run);

            let mut isolate_config = IsolateConfig {
                runtime: runtime_config_for(pyodide_profile)?,
                ..IsolateConfig::default()
            };
            isolate_config.runtime.warm_state = Some(warm_state);
            let bench_cleanup = cleanup_kind.unwrap_or(CleanupKind::Full);
            isolate_config.cleanup = bench_cleanup.to_cleanup_mode();
            applied_cleanup = Some(bench_cleanup);

            let options = PoolOptions {
                isolate: isolate_config,
                telemetry_interval: None,
                ..PoolOptions::default()
            };

            let registry = if mode.uses_registry_pool() {
                Some(BundlePoolRegistry::new(options.clone())?)
            } else {
                None
            };
            let pool = if let Some(registry) = &registry {
                registry.pool_for_bytes(&bundle_bytes)?
            } else {
                let artifact = BundleArtifact::from_bundle(bundle.clone())?;
                BundlePool::from_artifact(artifact, options)?
            };
            let retained_prepared = if mode.uses_registry_retained_handler() {
                let registry = registry.as_ref().ok_or_else(|| {
                    anyhow!(
                        "mode '{}' requires a registry for retained handlers",
                        mode.name()
                    )
                })?;
                Some(registry.prepare_handler_for_bytes(&bundle_bytes, descriptor.clone())?)
            } else {
                None
            };
            let handler = if mode.uses_registry_cached_handler() {
                if let Some(registry) = &registry {
                    let _ =
                        registry.prepare_handler_for_bytes(&bundle_bytes, descriptor.clone())?;
                }
                None
            } else if mode.uses_registry_retained_handler() {
                None
            } else {
                Some(match descriptor.clone() {
                    Some(desc) => pool.prepare_handler(Some(desc))?,
                    None => pool.prepare_default_handler()?,
                })
            };

            if mode.uses_explicit_warm_call() {
                let outcome = if let Some(prepared) = retained_prepared.as_ref() {
                    match invocation {
                        InvocationKind::Json => prepared.call_json_input(json_input.clone())?,
                        InvocationKind::RawCtx => prepared.call_rawctx(rawctx_inputs_for_call(
                            scenario,
                            profile,
                            raw_inputs.as_ref(),
                            mode,
                        )?)?,
                    }
                } else {
                    let handler = handler.as_ref().ok_or_else(|| {
                        anyhow!(
                            "mode '{}' requires a local handler for explicit warm calls",
                            mode.name()
                        )
                    })?;
                    match invocation {
                        InvocationKind::Json => {
                            pool.warm_json_input(handler, json_input.clone())?
                        }
                        InvocationKind::RawCtx => pool.warm_rawctx(
                            handler,
                            rawctx_inputs_for_call(scenario, profile, raw_inputs.as_ref(), mode)?,
                        )?,
                    }
                };
                validate_aardvark_outcome(scenario, profile, invocation, &outcome)?;
            }

            for _ in 0..iterations {
                let raw_inputs_for_iteration = if matches!(invocation, InvocationKind::RawCtx) {
                    Some(rawctx_inputs_for_call(
                        scenario,
                        profile,
                        raw_inputs.as_ref(),
                        mode,
                    )?)
                } else {
                    None
                };
                let start = Instant::now();
                let outcome =
                    if let Some(prepared) = retained_prepared.as_ref() {
                        match invocation {
                            InvocationKind::Json => prepared.call_json_input(json_input.clone())?,
                            InvocationKind::RawCtx => prepared
                                .call_rawctx(raw_inputs_for_iteration.unwrap_or_default())?,
                        }
                    } else if mode.uses_registry_cached_handler() {
                        let registry = registry.as_ref().ok_or_else(|| {
                            anyhow!(
                                "mode '{}' requires a registry for cached handlers",
                                mode.name()
                            )
                        })?;
                        let prepared = registry
                            .prepare_handler_for_bytes(&bundle_bytes, descriptor.clone())?;
                        match invocation {
                            InvocationKind::Json => prepared.call_json_input(json_input.clone())?,
                            InvocationKind::RawCtx => prepared
                                .call_rawctx(raw_inputs_for_iteration.unwrap_or_default())?,
                        }
                    } else {
                        let pool_for_call = if let Some(registry) = &registry {
                            registry.pool_for_bytes(&bundle_bytes)?
                        } else {
                            pool.clone()
                        };
                        let local_handler;
                        let handler_for_call = if mode.prepares_handler_each_call() {
                            local_handler = match descriptor.clone() {
                                Some(desc) => pool_for_call.prepare_handler(Some(desc))?,
                                None => pool_for_call.prepare_default_handler()?,
                            };
                            &local_handler
                        } else {
                            handler.as_ref().ok_or_else(|| {
                                anyhow!(
                                    "mode '{}' requires a persistent prepared handler",
                                    mode.name()
                                )
                            })?
                        };
                        match invocation {
                            InvocationKind::Json => pool_for_call
                                .call_json_input(handler_for_call, json_input.clone())?,
                            InvocationKind::RawCtx => pool_for_call.call_rawctx(
                                handler_for_call,
                                raw_inputs_for_iteration.unwrap_or_default(),
                            )?,
                        }
                    };
                validate_aardvark_outcome(scenario, profile, invocation, &outcome)?;
                let total_elapsed = start.elapsed();
                let prepare_ms = outcome.diagnostics.prepare_ms.unwrap_or(0);
                let cleanup_ms = outcome.diagnostics.cleanup_ms.unwrap_or(0);
                let prepare_duration = Duration::from_millis(prepare_ms);
                let cleanup_duration = Duration::from_millis(cleanup_ms);
                let mut run_duration = total_elapsed
                    .checked_sub(prepare_duration)
                    .unwrap_or_default();
                run_duration = run_duration
                    .checked_sub(cleanup_duration)
                    .unwrap_or_default();

                prepare.push(prepare_duration);
                run.push(run_duration);
                total.push(total_elapsed);
            }
        }
        PathKind::FirstCall => {
            return Err(anyhow!(
                "first-call path is synthesized from setup timings, not benchmarked directly"
            ));
        }
    }

    Ok(BenchResult {
        scenario,
        mode,
        profile,
        invocation: Some(invocation),
        path: Some(path),
        cleanup: applied_cleanup,
        iterations,
        total: timing_stats(&total),
        prepare: Some(timing_stats(&prepare)),
        run: Some(timing_stats(&run)),
        rss_mib: current_rss_mib(),
        cold_total: cold_total_stats,
        cold_prepare: cold_prepare_stats,
        cold_run: cold_run_stats,
        host_python_version: None,
        host_packages: None,
        samples: timing_samples(include_samples, &total, &prepare, &run),
        setup_breakdown: has_setup_breakdown
            .then(|| setup_breakdown_stats(&setup_breakdown_buckets)),
        setup_breakdown_samples: (has_setup_breakdown && include_samples)
            .then(|| setup_breakdown_samples(&setup_breakdown_buckets)),
        setup_pool_desired_size: has_setup_breakdown.then_some(setup_pool_desired_size),
    })
}

fn execute_iteration(
    bench_context: AardvarkBenchContext,
    runtime: &mut PyRuntime,
    descriptor: Option<&InvocationDescriptor>,
    bundle: &Bundle,
    json_input: Option<JsonInput>,
    raw_inputs: &[RawCtxInput],
    timings: &mut TimingBuckets<'_>,
) -> Result<()> {
    let prep_start = Instant::now();
    let (session, _) = match descriptor {
        Some(desc) => {
            runtime.prepare_session_with_manifest_and_descriptor(bundle.clone(), desc.clone())?
        }
        None => runtime.prepare_session_with_manifest(bundle.clone())?,
    };
    let prep_elapsed = prep_start.elapsed();

    let run_start = Instant::now();
    let outcome = match bench_context.invocation {
        InvocationKind::Json => {
            let mut strategy = JsonInvocationStrategy::with_input(json_input);
            runtime.run_session_with_strategy(&session, &mut strategy)?
        }
        InvocationKind::RawCtx => {
            let mut strategy = RawCtxInvocationStrategy::new(raw_inputs.to_vec());
            runtime.run_session_with_strategy(&session, &mut strategy)?
        }
    };
    let run_elapsed = run_start.elapsed();

    if !outcome.is_success() {
        return Err(anyhow!(
            "handler failed: {:?}; diagnostics: {:?}",
            outcome.status,
            outcome.diagnostics
        ));
    }

    validate_aardvark_outcome(
        bench_context.scenario,
        bench_context.profile,
        bench_context.invocation,
        &outcome,
    )?;

    timings.prepare.push(prep_elapsed);
    timings.run.push(run_elapsed);
    timings.total.push(prep_elapsed + run_elapsed);

    Ok(())
}

fn capture_warm_state(
    bench_context: AardvarkBenchContext,
    bundle: &Bundle,
    descriptor: Option<&InvocationDescriptor>,
    json_input: Option<JsonInput>,
    raw_inputs: &[RawCtxInput],
    pyodide_profile: Option<&str>,
    warm_preimports: &[String],
) -> Result<(WarmState, TimingStats, TimingStats, TimingStats)> {
    let mut baseline_runtime = PyRuntime::new(runtime_config_for(pyodide_profile)?)?;
    let mut cold_prepare = Vec::with_capacity(1);
    let mut cold_run = Vec::with_capacity(1);
    let mut cold_total = Vec::with_capacity(1);
    {
        let mut buckets = TimingBuckets {
            prepare: &mut cold_prepare,
            run: &mut cold_run,
            total: &mut cold_total,
        };
        execute_iteration(
            bench_context,
            &mut baseline_runtime,
            descriptor,
            bundle,
            json_input.clone(),
            raw_inputs,
            &mut buckets,
        )?;
    }
    drop(baseline_runtime);

    let mut warm_config = runtime_config_for(pyodide_profile)?;
    warm_config.snapshot.save_to = Some(PathBuf::from("target/perf/bench_warm_snapshot.bin"));
    if !warm_preimports.is_empty() {
        let scripts = warm_preimport_scripts(warm_preimports)?;
        warm_config.hooks.before_warm_snapshot = Some(Arc::new(move |runtime| {
            for script in scripts.iter() {
                runtime.js_runtime().run_python_snippet(script)?;
            }
            Ok(())
        }));
    }
    let mut runtime = PyRuntime::new(warm_config)?;
    if let Some(desc) = descriptor {
        runtime.prepare_session_with_manifest_and_descriptor(bundle.clone(), desc.clone())?;
    } else {
        runtime.prepare_session_with_manifest(bundle.clone())?;
    }
    let warm_state = runtime.capture_warm_state()?;
    Ok((
        warm_state,
        timing_stats(&cold_total),
        timing_stats(&cold_prepare),
        timing_stats(&cold_run),
    ))
}

fn warm_preimport_scripts(modules: &[String]) -> Result<Arc<Vec<String>>> {
    let mut scripts = Vec::with_capacity(modules.len());
    for module in modules {
        let module = module.trim();
        if module.is_empty() {
            continue;
        }
        let literal = serde_json::to_string(module)
            .map_err(|err| anyhow!("failed to encode warm preimport module {module}: {err}"))?;
        scripts.push(format!("__import__({literal})"));
    }
    Ok(Arc::new(scripts))
}

fn runtime_config_for(pyodide_profile: Option<&str>) -> Result<PyRuntimeConfig> {
    let mut config = PyRuntimeConfig::default();
    if let Some(profile) = pyodide_profile {
        config.set_pyodide_distribution_profile(profile)?;
    }
    Ok(config)
}
