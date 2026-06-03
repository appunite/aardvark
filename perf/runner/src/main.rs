use std::collections::BTreeSet;
use std::fs;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use aardvark_core::{
    config::{PyRuntimeConfig, ResetPolicy, WarmState},
    invocation::{FieldDescriptor, InvocationDescriptor},
    outcome::{OutcomeStatus, ResultPayload},
    pool::{PoolConfig, PoolResetMode, PyRuntimePool},
    strategy::{
        JsonInvocationStrategy, RawCtxBindingBuilder, RawCtxInput, RawCtxInvocationStrategy,
        RawCtxMetadata, RawCtxPublishBuilder,
    },
    Bundle, BundleArtifact, BundlePool, CleanupMode, IsolateConfig, PoolOptions, PyRuntime,
};
use anyhow::{anyhow, Context, Result};
use bytes::Bytes;
use clap::{Parser, ValueEnum};
use comfy_table::{presets::UTF8_FULL_CONDENSED, Cell, Table};
use serde::Serialize;
use serde_json::{json, Value as JsonValue};
use which::which;

mod perf;

#[derive(Parser, Debug)]
#[command(
    name = "aardvark-perf",
    about = "Performance harness for Aardvark runtime"
)]
enum Cli {
    /// Run benchmarks for all scenarios (Aardvark + host Python) and emit reports
    All {
        #[arg(long, default_value_t = 10)]
        iterations: usize,
        #[arg(long)]
        json: Option<PathBuf>,
        #[arg(long)]
        csv: Option<PathBuf>,
        #[arg(long, value_enum, ignore_case = true)]
        profile: Option<LoadProfile>,
    },
    /// Run a single scenario/mode combination
    Scenario {
        #[arg(long, value_enum, ignore_case = true)]
        scenario: Scenario,
        #[arg(long, value_enum, ignore_case = true)]
        mode: Mode,
        #[arg(long, default_value_t = 10)]
        iterations: usize,
        #[arg(long, value_enum, ignore_case = true)]
        profile: Option<LoadProfile>,
    },
}

#[derive(Copy, Clone, Debug, Serialize, ValueEnum)]
#[value(rename_all = "kebab-case")]
enum Scenario {
    Echo,
    Numpy,
    Pandas,
    Tensor,
    Matplotlib,
}

#[derive(Copy, Clone, Debug, Serialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
#[value(rename_all = "kebab-case")]
enum LoadProfile {
    None,
    Low,
    Medium,
    High,
}

#[derive(Copy, Clone, Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
enum InvocationKind {
    Json,
    RawCtx,
}

#[derive(Copy, Clone, Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
enum PathKind {
    Cold,
    Warm,
    ResetInPlace,
    Persistent,
    FirstCall,
}

#[derive(Copy, Clone, Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
enum CleanupKind {
    Full,
    SharedBuffersOnly,
    None,
}

impl CleanupKind {
    fn label(self) -> &'static str {
        match self {
            CleanupKind::Full => "full",
            CleanupKind::SharedBuffersOnly => "shared-buffers-only",
            CleanupKind::None => "none",
        }
    }

    fn to_cleanup_mode(self) -> CleanupMode {
        match self {
            CleanupKind::Full => CleanupMode::Full,
            CleanupKind::SharedBuffersOnly => CleanupMode::SharedBuffersOnly,
            CleanupKind::None => CleanupMode::None,
        }
    }
}

#[derive(Copy, Clone, Debug, Serialize, ValueEnum)]
#[value(rename_all = "kebab-case")]
enum Mode {
    AardvarkJsonCold,
    AardvarkJsonWarm,
    AardvarkJsonResetInPlace,
    #[value(alias = "aardvark-json-persistent-full")]
    AardvarkJsonPersistent,
    AardvarkJsonPersistentShared,
    AardvarkJsonPersistentNone,
    #[value(name = "aardvark-rawctx-cold")]
    AardvarkRawCtxCold,
    #[value(name = "aardvark-rawctx-warm")]
    AardvarkRawCtxWarm,
    #[value(name = "aardvark-rawctx-reset-in-place")]
    AardvarkRawCtxResetInPlace,
    #[value(
        name = "aardvark-rawctx-persistent",
        alias = "aardvark-rawctx-persistent-full"
    )]
    AardvarkRawCtxPersistent,
    #[value(name = "aardvark-rawctx-persistent-shared")]
    AardvarkRawCtxPersistentShared,
    #[value(name = "aardvark-rawctx-persistent-none")]
    AardvarkRawCtxPersistentNone,
    #[value(
        name = "host-python-warm",
        alias = "host-python",
        alias = "host",
        alias = "python"
    )]
    HostPythonWarm,
    #[value(name = "host-python-prepare-run")]
    HostPythonPrepareRun,
    #[value(name = "host-python-process")]
    HostPythonProcess,
}

impl Mode {
    fn name(&self) -> &'static str {
        match self {
            Mode::AardvarkJsonCold => "aardvark-json-cold",
            Mode::AardvarkJsonWarm => "aardvark-json-warm",
            Mode::AardvarkJsonResetInPlace => "aardvark-json-reset-in-place",
            Mode::AardvarkJsonPersistent => "aardvark-json-persistent",
            Mode::AardvarkJsonPersistentShared => "aardvark-json-persistent-shared",
            Mode::AardvarkJsonPersistentNone => "aardvark-json-persistent-none",
            Mode::AardvarkRawCtxCold => "aardvark-rawctx-cold",
            Mode::AardvarkRawCtxWarm => "aardvark-rawctx-warm",
            Mode::AardvarkRawCtxResetInPlace => "aardvark-rawctx-reset-in-place",
            Mode::AardvarkRawCtxPersistent => "aardvark-rawctx-persistent",
            Mode::AardvarkRawCtxPersistentShared => "aardvark-rawctx-persistent-shared",
            Mode::AardvarkRawCtxPersistentNone => "aardvark-rawctx-persistent-none",
            Mode::HostPythonWarm => "host-python-warm",
            Mode::HostPythonPrepareRun => "host-python-prepare-run",
            Mode::HostPythonProcess => "host-python-process",
        }
    }

    fn invocation_kind(self) -> Option<InvocationKind> {
        match self {
            Mode::AardvarkJsonCold | Mode::AardvarkJsonWarm | Mode::AardvarkJsonResetInPlace => {
                Some(InvocationKind::Json)
            }
            Mode::AardvarkJsonPersistent
            | Mode::AardvarkJsonPersistentShared
            | Mode::AardvarkJsonPersistentNone => Some(InvocationKind::Json),
            Mode::AardvarkRawCtxCold
            | Mode::AardvarkRawCtxWarm
            | Mode::AardvarkRawCtxResetInPlace => Some(InvocationKind::RawCtx),
            Mode::AardvarkRawCtxPersistent
            | Mode::AardvarkRawCtxPersistentShared
            | Mode::AardvarkRawCtxPersistentNone => Some(InvocationKind::RawCtx),
            Mode::HostPythonWarm | Mode::HostPythonPrepareRun | Mode::HostPythonProcess => None,
        }
    }

    fn path_kind(self) -> Option<PathKind> {
        match self {
            Mode::AardvarkJsonCold | Mode::AardvarkRawCtxCold => Some(PathKind::Cold),
            Mode::AardvarkJsonWarm | Mode::AardvarkRawCtxWarm => Some(PathKind::Warm),
            Mode::AardvarkJsonResetInPlace | Mode::AardvarkRawCtxResetInPlace => {
                Some(PathKind::ResetInPlace)
            }
            Mode::AardvarkJsonPersistent
            | Mode::AardvarkJsonPersistentShared
            | Mode::AardvarkJsonPersistentNone
            | Mode::AardvarkRawCtxPersistent
            | Mode::AardvarkRawCtxPersistentShared
            | Mode::AardvarkRawCtxPersistentNone => Some(PathKind::Persistent),
            Mode::HostPythonWarm | Mode::HostPythonPrepareRun | Mode::HostPythonProcess => None,
        }
    }

    fn cleanup_kind(self) -> Option<CleanupKind> {
        match self {
            Mode::AardvarkJsonPersistent | Mode::AardvarkRawCtxPersistent => {
                Some(CleanupKind::Full)
            }
            Mode::AardvarkJsonPersistentShared | Mode::AardvarkRawCtxPersistentShared => {
                Some(CleanupKind::SharedBuffersOnly)
            }
            Mode::AardvarkJsonPersistentNone | Mode::AardvarkRawCtxPersistentNone => {
                Some(CleanupKind::None)
            }
            _ => None,
        }
    }

    fn aardvark_modes() -> &'static [Mode] {
        &[
            Mode::AardvarkJsonCold,
            Mode::AardvarkJsonWarm,
            Mode::AardvarkJsonResetInPlace,
            Mode::AardvarkJsonPersistent,
            Mode::AardvarkJsonPersistentShared,
            Mode::AardvarkJsonPersistentNone,
            Mode::AardvarkRawCtxCold,
            Mode::AardvarkRawCtxWarm,
            Mode::AardvarkRawCtxResetInPlace,
            Mode::AardvarkRawCtxPersistent,
            Mode::AardvarkRawCtxPersistentShared,
            Mode::AardvarkRawCtxPersistentNone,
        ]
    }

    fn host_modes() -> &'static [Mode] {
        &[
            Mode::HostPythonWarm,
            Mode::HostPythonPrepareRun,
            Mode::HostPythonProcess,
        ]
    }

    fn is_aardvark(self) -> bool {
        self.invocation_kind().is_some()
    }
}

#[derive(Clone, Serialize)]
struct BenchResult {
    scenario: Scenario,
    mode: Mode,
    profile: LoadProfile,
    #[serde(skip_serializing_if = "Option::is_none")]
    invocation: Option<InvocationKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<PathKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cleanup: Option<CleanupKind>,
    iterations: usize,
    total: TimingStats,
    #[serde(skip_serializing_if = "Option::is_none")]
    prepare: Option<TimingStats>,
    #[serde(skip_serializing_if = "Option::is_none")]
    run: Option<TimingStats>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rss_mib: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cold_total: Option<TimingStats>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cold_prepare: Option<TimingStats>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cold_run: Option<TimingStats>,
    #[serde(skip_serializing_if = "Option::is_none")]
    host_python_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    host_packages: Option<Vec<String>>,
}

#[derive(Serialize, serde::Deserialize, Default, Clone)]
struct TimingStats {
    avg_ms: f64,
    min_ms: f64,
    max_ms: f64,
    std_ms: f64,
    p50_ms: f64,
    p95_ms: f64,
    p99_ms: f64,
}

struct TimingBuckets<'a> {
    prepare: &'a mut Vec<Duration>,
    run: &'a mut Vec<Duration>,
    total: &'a mut Vec<Duration>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli {
        Cli::All {
            iterations,
            json,
            csv,
            profile,
        } => {
            let mut results = Vec::new();
            let profiles: Vec<LoadProfile> = match profile {
                Some(p) => vec![p],
                None => vec![
                    LoadProfile::None,
                    LoadProfile::Low,
                    LoadProfile::Medium,
                    LoadProfile::High,
                ],
            };
            for profile in profiles {
                for scenario in [
                    Scenario::Echo,
                    Scenario::Numpy,
                    Scenario::Pandas,
                    Scenario::Tensor,
                    Scenario::Matplotlib,
                ] {
                    for mode in Mode::aardvark_modes() {
                        results.push(bench_aardvark(scenario, *mode, iterations, profile)?);
                    }
                    for mode in host_modes_for_scenario(scenario) {
                        results.push(bench_host(scenario, *mode, iterations, profile)?);
                    }
                }
            }
            let expanded = expand_results(&results);
            if let Some(path) = json {
                write_json(&path, &expanded)?;
            }
            if let Some(path) = csv {
                write_csv(&path, &expanded)?;
            }
            print_summary(&expanded);
        }
        Cli::Scenario {
            scenario,
            mode,
            iterations,
            profile,
        } => {
            let profile = profile.unwrap_or(LoadProfile::None);
            let result = if mode.is_aardvark() {
                bench_aardvark(scenario, mode, iterations, profile)?
            } else {
                bench_host(scenario, mode, iterations, profile)?
            };
            let expanded = expand_results(std::slice::from_ref(&result));
            println!("{}", serde_json::to_string_pretty(&expanded)?);
        }
    }
    Ok(())
}

fn bench_aardvark(
    scenario: Scenario,
    mode: Mode,
    iterations: usize,
    profile: LoadProfile,
) -> Result<BenchResult> {
    let invocation = mode
        .invocation_kind()
        .ok_or_else(|| anyhow!("mode '{}' is not an Aardvark variant", mode.name()))?;
    let path = mode
        .path_kind()
        .ok_or_else(|| anyhow!("mode '{}' is missing a path kind", mode.name()))?;
    let cleanup_kind = mode.cleanup_kind();
    let mut applied_cleanup = cleanup_kind;

    let python_source = scenario_source(scenario);
    let manifest = scenario_manifest(scenario, invocation);
    let bundle = build_bundle(&python_source, manifest.as_bytes())?;
    let descriptor = descriptor_for(scenario, invocation, profile);

    let json_payload = json_payload_for(scenario, profile);
    let raw_inputs = Arc::new(rawctx_inputs_for(scenario, profile)?);

    let mut prepare = Vec::with_capacity(iterations);
    let mut run = Vec::with_capacity(iterations);
    let mut total = Vec::with_capacity(iterations);
    let mut cold_total_stats: Option<TimingStats> = None;
    let mut cold_prepare_stats: Option<TimingStats> = None;
    let mut cold_run_stats: Option<TimingStats> = None;

    match path {
        PathKind::Cold => {
            for _ in 0..iterations {
                let mut runtime = PyRuntime::new(PyRuntimeConfig::default())?;
                let mut buckets = TimingBuckets {
                    prepare: &mut prepare,
                    run: &mut run,
                    total: &mut total,
                };
                execute_iteration(
                    &mut runtime,
                    invocation,
                    descriptor.as_ref(),
                    &bundle,
                    json_payload.clone(),
                    raw_inputs.as_ref(),
                    &mut buckets,
                )?;
            }
        }
        PathKind::Warm => {
            let (warm_state, cold_total, cold_prepare, cold_run) = capture_warm_state(
                invocation,
                &bundle,
                descriptor.as_ref(),
                json_payload.clone(),
                raw_inputs.as_ref(),
            )?;
            cold_total_stats = Some(cold_total);
            cold_prepare_stats = Some(cold_prepare);
            cold_run_stats = Some(cold_run);
            let config = PyRuntimeConfig {
                warm_state: Some(warm_state),
                ..PyRuntimeConfig::default()
            };
            for _ in 0..iterations {
                let mut runtime = PyRuntime::new(config.clone())?;
                let mut buckets = TimingBuckets {
                    prepare: &mut prepare,
                    run: &mut run,
                    total: &mut total,
                };
                execute_iteration(
                    &mut runtime,
                    invocation,
                    descriptor.as_ref(),
                    &bundle,
                    json_payload.clone(),
                    raw_inputs.as_ref(),
                    &mut buckets,
                )?;
            }
        }
        PathKind::ResetInPlace => {
            let (warm_state, cold_total, cold_prepare, cold_run) = capture_warm_state(
                invocation,
                &bundle,
                descriptor.as_ref(),
                json_payload.clone(),
                raw_inputs.as_ref(),
            )?;
            cold_total_stats = Some(cold_total);
            cold_prepare_stats = Some(cold_prepare);
            cold_run_stats = Some(cold_run);
            let runtime_config = PyRuntimeConfig {
                warm_state: Some(warm_state),
                reset_policy: ResetPolicy::Manual,
                ..PyRuntimeConfig::default()
            };
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
                        runtime,
                        invocation,
                        descriptor.as_ref(),
                        &bundle,
                        json_payload.clone(),
                        raw_inputs.as_ref(),
                        &mut buckets,
                    )?;
                }
                drop(handle);
            }
        }
        PathKind::Persistent => {
            let (warm_state, cold_total, cold_prepare, cold_run) = capture_warm_state(
                invocation,
                &bundle,
                descriptor.as_ref(),
                json_payload.clone(),
                raw_inputs.as_ref(),
            )?;
            cold_total_stats = Some(cold_total);
            cold_prepare_stats = Some(cold_prepare);
            cold_run_stats = Some(cold_run);

            let mut isolate_config = IsolateConfig::default();
            isolate_config.runtime.warm_state = Some(warm_state);
            let bench_cleanup = cleanup_kind.unwrap_or(CleanupKind::Full);
            isolate_config.cleanup = bench_cleanup.to_cleanup_mode();
            applied_cleanup = Some(bench_cleanup);

            let options = PoolOptions {
                isolate: isolate_config,
                telemetry_interval: None,
                ..PoolOptions::default()
            };

            let artifact = BundleArtifact::from_bundle(bundle.clone())?;
            let pool = BundlePool::from_artifact(artifact, options)?;
            let handle = pool.handle();
            let handler = match descriptor.clone() {
                Some(desc) => handle.prepare_handler(Some(desc)),
                None => handle.prepare_default_handler(),
            };

            for _ in 0..iterations {
                let start = Instant::now();
                let outcome = match invocation {
                    InvocationKind::Json => pool.call_json(&handler, json_payload.clone())?,
                    InvocationKind::RawCtx => pool.call_rawctx(&handler, (*raw_inputs).clone())?,
                };
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
        PathKind::FirstCall => unreachable!("first-call path is synthesized post-run"),
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
    })
}

fn execute_iteration(
    runtime: &mut PyRuntime,
    invocation: InvocationKind,
    descriptor: Option<&InvocationDescriptor>,
    bundle: &Bundle,
    json_payload: Option<JsonValue>,
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
    let outcome = match invocation {
        InvocationKind::Json => {
            let mut strategy = JsonInvocationStrategy::new(json_payload);
            runtime.run_session_with_strategy(&session, &mut strategy)?
        }
        InvocationKind::RawCtx => {
            let mut strategy = RawCtxInvocationStrategy::new(raw_inputs.to_vec());
            runtime.run_session_with_strategy(&session, &mut strategy)?
        }
    };
    let run_elapsed = run_start.elapsed();

    if !outcome.is_success() {
        return Err(anyhow!("handler failed: {:?}", outcome.status));
    }

    if matches!(invocation, InvocationKind::RawCtx)
        && !matches!(
            outcome.status,
            OutcomeStatus::Success(ResultPayload::SharedBuffers(_))
        )
    {
        return Err(anyhow!("rawctx run did not return shared buffers"));
    }

    timings.prepare.push(prep_elapsed);
    timings.run.push(run_elapsed);
    timings.total.push(prep_elapsed + run_elapsed);

    Ok(())
}

fn capture_warm_state(
    invocation: InvocationKind,
    bundle: &Bundle,
    descriptor: Option<&InvocationDescriptor>,
    json_payload: Option<JsonValue>,
    raw_inputs: &[RawCtxInput],
) -> Result<(WarmState, TimingStats, TimingStats, TimingStats)> {
    let mut baseline_runtime = PyRuntime::new(PyRuntimeConfig::default())?;
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
            &mut baseline_runtime,
            invocation,
            descriptor,
            bundle,
            json_payload.clone(),
            raw_inputs,
            &mut buckets,
        )?;
    }
    drop(baseline_runtime);

    let mut warm_config = PyRuntimeConfig::default();
    warm_config.snapshot.save_to = Some(PathBuf::from("target/perf/bench_warm_snapshot.bin"));
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

fn bench_host(
    scenario: Scenario,
    mode: Mode,
    iterations: usize,
    profile: LoadProfile,
) -> Result<BenchResult> {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/run_host.py");
    let uv = which("uv")
        .context("`uv` command not found on PATH. Install from https://docs.astral.sh/uv/ or ensure it is available before running the perf suite.")?;
    let host_plan = host_runtime_plan(scenario)?;
    let mut cmd = Command::new(uv);
    cmd.arg("run");
    cmd.arg(format!("--python={}", host_plan.python_version));
    for pkg in &host_plan.packages {
        cmd.arg(format!("--with={pkg}"));
    }
    cmd.arg("python");
    cmd.arg(script);
    cmd.arg("--scenario");
    cmd.arg(scenario.name());
    cmd.arg("--iterations");
    cmd.arg(iterations.to_string());
    cmd.arg("--profile");
    cmd.arg(profile.name());
    cmd.arg("--host-mode");
    cmd.arg(host_mode_name(mode)?);

    let output = cmd
        .output()
        .with_context(|| "failed to run host python benchmark")?;
    if !output.status.success() {
        return Err(anyhow!(
            "host benchmark failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let result: HostResult = serde_json::from_slice(&output.stdout)
        .with_context(|| "failed to parse host benchmark output")?;
    Ok(BenchResult {
        scenario,
        mode,
        profile,
        invocation: None,
        path: None,
        cleanup: None,
        iterations,
        total: result.total,
        prepare: result.prepare,
        run: result.run,
        rss_mib: Some(result.rss_mib),
        cold_total: None,
        cold_prepare: None,
        cold_run: None,
        host_python_version: result.python_version.or(Some(host_plan.python_version)),
        host_packages: Some(host_plan.packages),
    })
}

fn scenario_source(scenario: Scenario) -> String {
    match scenario {
        Scenario::Echo => perf::echo_script().to_owned(),
        Scenario::Numpy => perf::numpy_script().to_owned(),
        Scenario::Pandas => perf::pandas_script().to_owned(),
        Scenario::Tensor => perf::tensor_script().to_owned(),
        Scenario::Matplotlib => perf::matplotlib_script().to_owned(),
    }
}

fn scenario_manifest(scenario: Scenario, invocation: InvocationKind) -> String {
    let packages = scenario_packages(scenario);
    let mut manifest = json!({
        "schemaVersion": "1.0",
        "entrypoint": "main:entrypoint",
        "packages": packages,
    });
    if matches!(invocation, InvocationKind::RawCtx) {
        manifest["resources"] = json!({
            "hostCapabilities": ["rawctx_buffers"],
        });
    }
    manifest.to_string()
}

fn descriptor_for(
    scenario: Scenario,
    invocation: InvocationKind,
    _profile: LoadProfile,
) -> Option<InvocationDescriptor> {
    match invocation {
        InvocationKind::Json => None,
        InvocationKind::RawCtx => {
            let mut descriptor = InvocationDescriptor::new("main:entrypoint");
            let metadata = match scenario {
                Scenario::Echo => RawCtxPublishBuilder::new("echo-output")
                    .transform("memoryview")
                    .metadata(json!({"scenario": "echo", "profile": _profile.name()}))
                    .build(),
                Scenario::Numpy => RawCtxPublishBuilder::new("numpy-output")
                    .transform("memoryview")
                    .metadata(json!({"scenario": "numpy", "profile": _profile.name()}))
                    .build(),
                Scenario::Pandas => RawCtxPublishBuilder::new("pandas-output")
                    .transform("memoryview")
                    .metadata(json!({
                        "format": "i32_f64_pairs",
                        "fields": ["category", "value_mean"],
                        "profile": _profile.name(),
                    }))
                    .build(),
                Scenario::Tensor => RawCtxPublishBuilder::new("tensor-output")
                    .transform("memoryview")
                    .metadata(json!({
                        "format": "f32_le",
                        "profile": _profile.name(),
                    }))
                    .build(),
                Scenario::Matplotlib => RawCtxPublishBuilder::new("matplotlib-output")
                    .transform("memoryview")
                    .metadata(json!({
                        "format": "u64_le",
                        "profile": _profile.name(),
                    }))
                    .build(),
            };
            descriptor.outputs.push(FieldDescriptor {
                name: "result".to_owned(),
                type_tag: None,
                metadata: Some(metadata),
            });
            if matches!(scenario, Scenario::Tensor) {
                descriptor.inputs.push(FieldDescriptor {
                    name: "tensor".to_owned(),
                    type_tag: None,
                    metadata: Some(
                        RawCtxBindingBuilder::new()
                            .raw_arg("tensor_payload")
                            .optional(true)
                            .build(),
                    ),
                });
            }
            Some(descriptor)
        }
    }
}

fn scenario_packages(scenario: Scenario) -> &'static [&'static str] {
    match scenario {
        Scenario::Echo => &[],
        Scenario::Numpy => &["numpy"],
        Scenario::Pandas => &["numpy", "pandas"],
        Scenario::Tensor => &["numpy"],
        Scenario::Matplotlib => &["numpy", "matplotlib"],
    }
}

fn build_bundle(source: &str, manifest: &[u8]) -> Result<Bundle> {
    use zip::write::SimpleFileOptions;
    use zip::CompressionMethod;

    let mut buffer = Vec::new();
    {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut buffer));
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        writer.start_file("main.py", options)?;
        writer.write_all(source.as_bytes())?;
        writer.start_file("aardvark.manifest.json", options)?;
        writer.write_all(manifest)?;
        writer.finish()?;
    }
    Ok(Bundle::from_zip_bytes(buffer)?)
}

fn timing_stats(samples: &[Duration]) -> TimingStats {
    if samples.is_empty() {
        return TimingStats::default();
    }
    let mut ms: Vec<f64> = samples.iter().map(|d| d.as_secs_f64() * 1000.0).collect();
    let avg_ms = ms.iter().sum::<f64>() / ms.len() as f64;
    let min_ms = ms.iter().fold(f64::INFINITY, |acc, &val| acc.min(val));
    let max_ms = ms.iter().fold(f64::NEG_INFINITY, |acc, &val| acc.max(val));
    let variance = ms
        .iter()
        .map(|value| {
            let diff = value - avg_ms;
            diff * diff
        })
        .sum::<f64>()
        / ms.len() as f64;
    ms.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let percentile = |pct: f64| -> f64 {
        if ms.len() == 1 {
            return ms[0];
        }
        let position = pct * (ms.len() as f64 - 1.0);
        let lower = position.floor() as usize;
        let upper = position.ceil() as usize;
        if lower == upper {
            ms[lower]
        } else {
            let weight = position - lower as f64;
            ms[lower] * (1.0 - weight) + ms[upper] * weight
        }
    };

    TimingStats {
        avg_ms,
        min_ms,
        max_ms,
        std_ms: variance.sqrt(),
        p50_ms: percentile(0.50),
        p95_ms: percentile(0.95),
        p99_ms: percentile(0.99),
    }
}

fn expand_results(results: &[BenchResult]) -> Vec<BenchResult> {
    let mut expanded = Vec::new();
    for result in results {
        if let (Some(cold_total), Some(cold_prepare), Some(cold_run)) =
            (&result.cold_total, &result.cold_prepare, &result.cold_run)
        {
            let mut first = result.clone();
            first.path = Some(PathKind::FirstCall);
            first.iterations = 1;
            first.total = cold_total.clone();
            first.prepare = Some(cold_prepare.clone());
            first.run = Some(cold_run.clone());
            first.cold_total = None;
            first.cold_prepare = None;
            first.cold_run = None;
            expanded.push(first);
        }

        let mut steady = result.clone();
        steady.cold_total = None;
        steady.cold_prepare = None;
        steady.cold_run = None;
        expanded.push(steady);
    }
    expanded
}

fn write_json(path: &Path, results: &[BenchResult]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let mut file =
        File::create(path).with_context(|| format!("failed to write {}", path.display()))?;
    file.write_all(serde_json::to_string_pretty(results)?.as_bytes())?;
    Ok(())
}

fn write_csv(path: &Path, results: &[BenchResult]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let mut file =
        File::create(path).with_context(|| format!("failed to write {}", path.display()))?;
    writeln!(
        file,
        "scenario,profile,mode,invocation,path,cleanup,iterations,avg_ms,min_ms,max_ms,std_ms,p50_ms,p95_ms,p99_ms,rss_mib,prepare_avg_ms,run_avg_ms"
    )?;
    for result in results {
        let prepare_avg = result
            .prepare
            .as_ref()
            .map(|s| format!("{:.2}", s.avg_ms))
            .unwrap_or_default();
        let run_avg = result
            .run
            .as_ref()
            .map(|s| format!("{:.2}", s.avg_ms))
            .unwrap_or_default();
        let path = result
            .path
            .map(|mode| match mode {
                PathKind::Cold => "cold",
                PathKind::Warm => "warm",
                PathKind::ResetInPlace => "reset-in-place",
                PathKind::Persistent => "persistent",
                PathKind::FirstCall => "first-call",
            })
            .unwrap_or("-");
        let cleanup = result.cleanup.map(|kind| kind.label()).unwrap_or("-");
        writeln!(
            file,
            "{},{},{},{},{},{},{},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{},{}",
            result.scenario.name(),
            result.profile.name(),
            result.mode.name(),
            result
                .invocation
                .map(|kind| match kind {
                    InvocationKind::Json => "json",
                    InvocationKind::RawCtx => "rawctx",
                })
                .unwrap_or("-"),
            path,
            cleanup,
            result.iterations,
            result.total.avg_ms,
            result.total.min_ms,
            result.total.max_ms,
            result.total.std_ms,
            result.total.p50_ms,
            result.total.p95_ms,
            result.total.p99_ms,
            result.rss_mib.unwrap_or_default(),
            prepare_avg,
            run_avg,
        )?;
    }
    Ok(())
}

fn print_summary(results: &[BenchResult]) {
    let mut table = Table::new();
    table.load_preset(UTF8_FULL_CONDENSED);
    table.set_header([
        "Scenario",
        "Profile",
        "Mode",
        "Invocation",
        "Path",
        "Cleanup",
        "Iter",
        "Avg ms",
        "Min ms",
        "Max ms",
        "Std ms",
        "P50 ms",
        "P95 ms",
        "P99 ms",
        "RSS (MiB)",
    ]);

    for r in results {
        let invocation = r
            .invocation
            .map(|kind| match kind {
                InvocationKind::Json => "json",
                InvocationKind::RawCtx => "rawctx",
            })
            .unwrap_or("-");
        let path = r
            .path
            .map(|kind| match kind {
                PathKind::Cold => "cold",
                PathKind::Warm => "warm",
                PathKind::ResetInPlace => "reset-in-place",
                PathKind::Persistent => "persistent",
                PathKind::FirstCall => "first-call",
            })
            .unwrap_or("-");
        let cleanup = r
            .cleanup
            .map(|kind| kind.label().to_string())
            .unwrap_or_else(|| "-".to_string());

        let rss = r
            .rss_mib
            .map(|value| format!("{value:.2}"))
            .unwrap_or_else(|| "-".to_string());

        table.add_row(vec![
            Cell::new(r.scenario.name()),
            Cell::new(r.profile.name()),
            Cell::new(r.mode.name()),
            Cell::new(invocation),
            Cell::new(path),
            Cell::new(cleanup),
            Cell::new(r.iterations.to_string()),
            Cell::new(format!("{:.2}", r.total.avg_ms)),
            Cell::new(format!("{:.2}", r.total.min_ms)),
            Cell::new(format!("{:.2}", r.total.max_ms)),
            Cell::new(format!("{:.2}", r.total.std_ms)),
            Cell::new(format!("{:.2}", r.total.p50_ms)),
            Cell::new(format!("{:.2}", r.total.p95_ms)),
            Cell::new(format!("{:.2}", r.total.p99_ms)),
            Cell::new(rss.clone()),
        ]);
    }

    println!("{}", table);
}

#[derive(Serialize, serde::Deserialize)]
struct HostResult {
    total: TimingStats,
    #[serde(default)]
    prepare: Option<TimingStats>,
    #[serde(default)]
    run: Option<TimingStats>,
    rss_mib: f64,
    #[serde(default)]
    python_version: Option<String>,
}

struct HostRuntimePlan {
    python_version: String,
    packages: Vec<String>,
}

fn host_mode_name(mode: Mode) -> Result<&'static str> {
    match mode {
        Mode::HostPythonWarm => Ok("warm-handler"),
        Mode::HostPythonPrepareRun => Ok("prepare-run"),
        Mode::HostPythonProcess => Ok("process"),
        other => Err(anyhow!(
            "mode '{}' is not a host Python variant",
            other.name()
        )),
    }
}

fn host_modes_for_scenario(scenario: Scenario) -> &'static [Mode] {
    match scenario {
        // Pyodide 0.29.4 pins matplotlib 3.8.4 for Python 3.13.2. That exact
        // native CPython package set is not generally wheel-installable, so the
        // default full matrix avoids producing a misleading or host-dependent row.
        Scenario::Matplotlib => &[],
        _ => Mode::host_modes(),
    }
}

fn host_runtime_plan(scenario: Scenario) -> Result<HostRuntimePlan> {
    let lock_path = pyodide_lock_path()?;
    let lock_text = fs::read_to_string(&lock_path)
        .with_context(|| format!("failed to read {}", lock_path.display()))?;
    let lock: JsonValue = serde_json::from_str(&lock_text)
        .with_context(|| format!("failed to parse {}", lock_path.display()))?;
    let python_version = lock
        .get("info")
        .and_then(|info| info.get("python"))
        .and_then(JsonValue::as_str)
        .map(str::to_owned)
        .ok_or_else(|| anyhow!("{} does not declare info.python", lock_path.display()))?;

    let lock_packages = lock
        .get("packages")
        .and_then(JsonValue::as_object)
        .ok_or_else(|| anyhow!("{} does not declare packages", lock_path.display()))?;
    let mut visited = BTreeSet::new();
    let mut packages = Vec::new();
    for name in scenario_packages(scenario) {
        collect_host_package(name, lock_packages, &mut visited, &mut packages, &lock_path)?;
    }

    Ok(HostRuntimePlan {
        python_version,
        packages,
    })
}

fn collect_host_package(
    name: &str,
    lock_packages: &serde_json::Map<String, JsonValue>,
    visited: &mut BTreeSet<String>,
    packages: &mut Vec<String>,
    lock_path: &Path,
) -> Result<()> {
    let package_key = pyodide_package_key(lock_packages, name).ok_or_else(|| {
        anyhow!(
            "{} does not declare Pyodide package '{}'",
            lock_path.display(),
            name
        )
    })?;
    let canonical = canonical_package_name(package_key);
    if !visited.insert(canonical) {
        return Ok(());
    }

    let package = lock_packages
        .get(package_key)
        .expect("package key must come from lock package map");
    let package_name = package
        .get("name")
        .and_then(JsonValue::as_str)
        .unwrap_or(package_key);
    let version = package
        .get("version")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| {
            anyhow!(
                "{} does not declare a version for Pyodide package '{}'",
                lock_path.display(),
                package_name
            )
        })?;
    packages.push(format!("{package_name}=={version}"));

    let depends = package
        .get("depends")
        .and_then(JsonValue::as_array)
        .into_iter()
        .flatten();
    for dependency in depends {
        let dependency = dependency.as_str().ok_or_else(|| {
            anyhow!(
                "{} declares a non-string dependency for Pyodide package '{}'",
                lock_path.display(),
                package_name
            )
        })?;
        collect_host_package(dependency, lock_packages, visited, packages, lock_path)?;
    }

    Ok(())
}

fn pyodide_package_key<'a>(
    lock_packages: &'a serde_json::Map<String, JsonValue>,
    requested: &str,
) -> Option<&'a str> {
    if let Some((key, _)) = lock_packages.get_key_value(requested) {
        return Some(key.as_str());
    }

    let requested = canonical_package_name(requested);
    lock_packages
        .iter()
        .find(|(key, package)| {
            canonical_package_name(key) == requested
                || package
                    .get("name")
                    .and_then(JsonValue::as_str)
                    .map(canonical_package_name)
                    .is_some_and(|name| name == requested)
        })
        .map(|(key, _)| key.as_str())
}

fn canonical_package_name(name: &str) -> String {
    let mut normalized = String::with_capacity(name.len());
    let mut last_was_separator = false;
    for ch in name.chars() {
        if matches!(ch, '-' | '_' | '.') {
            if !last_was_separator {
                normalized.push('-');
                last_was_separator = true;
            }
        } else {
            normalized.push(ch.to_ascii_lowercase());
            last_was_separator = false;
        }
    }
    normalized
}

fn pyodide_lock_path() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("AARDVARK_PYODIDE_DIST_DIR") {
        return Ok(PathBuf::from(dir).join("pyodide-lock.json"));
    }
    if let Some(dir) = std::env::var_os("PYODIDE_DIST_DIR") {
        return Ok(PathBuf::from(dir).join("pyodide-lock.json"));
    }

    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| anyhow!("failed to resolve workspace root"))?;
    let default_path = workspace_root
        .join(".aardvark/pyodide-distributions")
        .join("aardvark-0.1.1-pyodide-v0.29.4-full")
        .join("pyodide-lock.json");
    if default_path.exists() {
        return Ok(default_path);
    }

    Err(anyhow!(
        "host Python benchmarks require a staged Pyodide distribution so Python and package versions can be pinned; set AARDVARK_PYODIDE_DIST_DIR or PYODIDE_DIST_DIR"
    ))
}

trait ScenarioExt {
    fn name(&self) -> &'static str;
}

impl ScenarioExt for Scenario {
    fn name(&self) -> &'static str {
        match self {
            Scenario::Echo => "echo",
            Scenario::Numpy => "numpy",
            Scenario::Pandas => "pandas",
            Scenario::Tensor => "tensor",
            Scenario::Matplotlib => "matplotlib",
        }
    }
}

impl LoadProfile {
    fn name(&self) -> &'static str {
        match self {
            LoadProfile::None => "none",
            LoadProfile::Low => "low",
            LoadProfile::Medium => "medium",
            LoadProfile::High => "high",
        }
    }
}

fn json_payload_for(scenario: Scenario, profile: LoadProfile) -> Option<JsonValue> {
    match scenario {
        Scenario::Echo => perf::echo_payload(profile).map(|bytes| {
            let text = std::str::from_utf8(bytes)
                .expect("echo fixtures must be valid utf-8")
                .to_owned();
            JsonValue::String(text)
        }),
        Scenario::Numpy => perf::numpy_size(profile).map(|size| json!({"size": size})),
        Scenario::Pandas => perf::pandas_rows(profile).map(|rows| json!({"rows": rows})),
        Scenario::Tensor => {
            let data = perf::tensor_data(profile);
            if data.is_empty() {
                None
            } else {
                let values = data.into_iter().map(JsonValue::from).collect::<Vec<_>>();
                Some(JsonValue::Array(values))
            }
        }
        Scenario::Matplotlib => {
            perf::matplotlib_points(profile).map(|points| json!({"points": points}))
        }
    }
}

fn rawctx_inputs_for(scenario: Scenario, profile: LoadProfile) -> Result<Vec<RawCtxInput>> {
    match scenario {
        Scenario::Echo => {
            let Some(bytes) = perf::echo_payload(profile) else {
                return Ok(Vec::new());
            };
            let data = Bytes::from_static(bytes);
            let meta = RawCtxMetadata::new("binary");
            Ok(vec![RawCtxInput::new("payload", data, Some(meta))?])
        }
        Scenario::Numpy => {
            let Some(size) = perf::numpy_size(profile) else {
                return Ok(Vec::new());
            };
            let data = Bytes::copy_from_slice(&size.to_le_bytes());
            Ok(vec![RawCtxInput::new("control", data, None)?])
        }
        Scenario::Pandas => {
            let Some(rows) = perf::pandas_rows(profile) else {
                return Ok(Vec::new());
            };
            let data = Bytes::copy_from_slice(&rows.to_le_bytes());
            Ok(vec![RawCtxInput::new("control", data, None)?])
        }
        Scenario::Tensor => {
            let bytes = perf::tensor_bytes(profile);
            if bytes.is_empty() {
                return Ok(Vec::new());
            }
            let length = bytes.len() / std::mem::size_of::<f32>();
            let metadata = RawCtxMetadata::new("binary")
                .with_shape(vec![length])
                .with_extra(json!({"format": "f32_le"}))?;
            Ok(vec![RawCtxInput::new(
                "tensor",
                Bytes::from(bytes),
                Some(metadata),
            )?])
        }
        Scenario::Matplotlib => {
            let Some(points) = perf::matplotlib_points(profile) else {
                return Ok(Vec::new());
            };
            let data = Bytes::copy_from_slice(&points.to_le_bytes());
            Ok(vec![RawCtxInput::new("control", data, None)?])
        }
    }
}

impl std::str::FromStr for Scenario {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "echo" => Ok(Scenario::Echo),
            "numpy" => Ok(Scenario::Numpy),
            "pandas" => Ok(Scenario::Pandas),
            "tensor" => Ok(Scenario::Tensor),
            "matplotlib" => Ok(Scenario::Matplotlib),
            other => Err(format!("unknown scenario '{other}'")),
        }
    }
}

impl std::str::FromStr for LoadProfile {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "none" => Ok(LoadProfile::None),
            "low" => Ok(LoadProfile::Low),
            "medium" => Ok(LoadProfile::Medium),
            "high" => Ok(LoadProfile::High),
            other => Err(format!("unknown profile '{other}'")),
        }
    }
}

impl std::str::FromStr for Mode {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "aardvark-json-cold" => Ok(Mode::AardvarkJsonCold),
            "aardvark-json-warm" => Ok(Mode::AardvarkJsonWarm),
            "aardvark-json-reset-in-place" => Ok(Mode::AardvarkJsonResetInPlace),
            "aardvark-json-persistent" | "aardvark-json-persistent-full" => {
                Ok(Mode::AardvarkJsonPersistent)
            }
            "aardvark-json-persistent-shared" => Ok(Mode::AardvarkJsonPersistentShared),
            "aardvark-json-persistent-none" => Ok(Mode::AardvarkJsonPersistentNone),
            "aardvark-rawctx-cold" => Ok(Mode::AardvarkRawCtxCold),
            "aardvark-rawctx-warm" => Ok(Mode::AardvarkRawCtxWarm),
            "aardvark-rawctx-reset-in-place" => Ok(Mode::AardvarkRawCtxResetInPlace),
            "aardvark-rawctx-persistent" | "aardvark-rawctx-persistent-full" => {
                Ok(Mode::AardvarkRawCtxPersistent)
            }
            "aardvark-rawctx-persistent-shared" => Ok(Mode::AardvarkRawCtxPersistentShared),
            "aardvark-rawctx-persistent-none" => Ok(Mode::AardvarkRawCtxPersistentNone),
            "host-python" | "host-python-warm" | "host" | "python" => Ok(Mode::HostPythonWarm),
            "host-python-prepare-run" => Ok(Mode::HostPythonPrepareRun),
            "host-python-process" => Ok(Mode::HostPythonProcess),
            other => Err(format!("unknown mode '{other}'")),
        }
    }
}

#[cfg(unix)]
#[cfg(target_os = "macos")]
#[allow(deprecated)]
fn current_rss_mib() -> Option<f64> {
    unsafe {
        let mut info: libc::mach_task_basic_info = std::mem::zeroed();
        let mut count = (std::mem::size_of::<libc::mach_task_basic_info>()
            / std::mem::size_of::<libc::integer_t>())
            as libc::mach_msg_type_number_t;
        let result = libc::task_info(
            libc::mach_task_self(),
            libc::MACH_TASK_BASIC_INFO,
            (&mut info as *mut _) as *mut libc::integer_t,
            &mut count,
        );
        if result == libc::KERN_SUCCESS {
            Some(info.resident_size as f64 / (1024.0 * 1024.0))
        } else {
            None
        }
    }
}

#[cfg(target_os = "linux")]
fn current_rss_mib() -> Option<f64> {
    let contents = fs::read_to_string("/proc/self/statm").ok()?;
    let resident_pages: f64 = contents.split_whitespace().nth(1)?.parse().ok()?;
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as f64;
    Some(resident_pages * page_size / (1024.0 * 1024.0))
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn current_rss_mib() -> Option<f64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_package_closure_includes_transitive_lock_dependencies() {
        let mut lock_packages = serde_json::Map::new();
        lock_packages.insert(
            "root-pkg".to_owned(),
            json!({
                "name": "root-pkg",
                "version": "1.0.0",
                "depends": ["mid_pkg"],
            }),
        );
        lock_packages.insert(
            "mid-pkg".to_owned(),
            json!({
                "name": "mid-pkg",
                "version": "2.0.0",
                "depends": ["leaf.pkg"],
            }),
        );
        lock_packages.insert(
            "leaf-pkg".to_owned(),
            json!({
                "name": "leaf-pkg",
                "version": "3.0.0",
                "depends": [],
            }),
        );

        let mut visited = BTreeSet::new();
        let mut packages = Vec::new();
        collect_host_package(
            "root_pkg",
            &lock_packages,
            &mut visited,
            &mut packages,
            Path::new("pyodide-lock.json"),
        )
        .unwrap();

        assert_eq!(
            packages,
            vec!["root-pkg==1.0.0", "mid-pkg==2.0.0", "leaf-pkg==3.0.0"]
        );
    }

    #[test]
    fn full_matrix_omits_default_matplotlib_host_rows() {
        assert!(host_modes_for_scenario(Scenario::Matplotlib).is_empty());
        assert_eq!(host_modes_for_scenario(Scenario::Pandas).len(), 3);
    }
}
