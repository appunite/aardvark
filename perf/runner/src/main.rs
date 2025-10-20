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
use comfy_table::{presets::UTF8_FULL_CONDENSED, Cell, Table};
use serde::Serialize;
use serde_json::{json, Value as JsonValue};
use structopt::StructOpt;
use which::which;

mod perf;

#[derive(StructOpt, Debug)]
#[structopt(
    name = "aardvark-perf",
    about = "Performance harness for Aardvark runtime"
)]
enum Cli {
    /// Run benchmarks for all scenarios (Aardvark + host Python) and emit reports
    All {
        #[structopt(long, default_value = "10")]
        iterations: usize,
        #[structopt(long)]
        json: Option<PathBuf>,
        #[structopt(long)]
        csv: Option<PathBuf>,
        #[structopt(long, possible_values = LoadProfile::VARIANTS, case_insensitive = true)]
        profile: Option<LoadProfile>,
    },
    /// Run a single scenario/mode combination
    Scenario {
        #[structopt(long, possible_values = Scenario::VARIANTS, case_insensitive = true)]
        scenario: Scenario,
        #[structopt(long, possible_values = Mode::VARIANTS, case_insensitive = true)]
        mode: Mode,
        #[structopt(long, default_value = "10")]
        iterations: usize,
        #[structopt(long, possible_values = LoadProfile::VARIANTS, case_insensitive = true)]
        profile: Option<LoadProfile>,
    },
}

#[derive(Copy, Clone, Debug, Serialize)]
enum Scenario {
    Echo,
    Numpy,
    Pandas,
}

#[derive(Copy, Clone, Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
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

#[derive(Copy, Clone, Debug, Serialize)]
enum Mode {
    AardvarkJsonCold,
    AardvarkJsonWarm,
    AardvarkJsonResetInPlace,
    AardvarkJsonPersistent,
    AardvarkJsonPersistentShared,
    AardvarkJsonPersistentNone,
    AardvarkRawCtxCold,
    AardvarkRawCtxWarm,
    AardvarkRawCtxResetInPlace,
    AardvarkRawCtxPersistent,
    AardvarkRawCtxPersistentShared,
    AardvarkRawCtxPersistentNone,
    HostPython,
}

impl Mode {
    const VARIANTS: &'static [&'static str] = &[
        "aardvark-json-cold",
        "aardvark-json-warm",
        "aardvark-json-reset-in-place",
        "aardvark-json-persistent",
        "aardvark-json-persistent-full",
        "aardvark-json-persistent-shared",
        "aardvark-json-persistent-none",
        "aardvark-rawctx-cold",
        "aardvark-rawctx-warm",
        "aardvark-rawctx-reset-in-place",
        "aardvark-rawctx-persistent",
        "aardvark-rawctx-persistent-full",
        "aardvark-rawctx-persistent-shared",
        "aardvark-rawctx-persistent-none",
        "host-python",
    ];

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
            Mode::HostPython => "host-python",
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
            Mode::HostPython => None,
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
            Mode::HostPython => None,
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
    rss_kib: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cold_total: Option<TimingStats>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cold_prepare: Option<TimingStats>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cold_run: Option<TimingStats>,
}

#[derive(Serialize, serde::Deserialize, Default, Clone)]
struct TimingStats {
    avg_ms: f64,
    min_ms: f64,
    max_ms: f64,
}

struct TimingBuckets<'a> {
    prepare: &'a mut Vec<Duration>,
    run: &'a mut Vec<Duration>,
    total: &'a mut Vec<Duration>,
}

fn main() -> Result<()> {
    let cli = Cli::from_args();
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
                for scenario in [Scenario::Echo, Scenario::Numpy, Scenario::Pandas] {
                    for mode in Mode::aardvark_modes() {
                        results.push(bench_aardvark(scenario, *mode, iterations, profile)?);
                    }
                    results.push(bench_host(scenario, iterations, profile)?);
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
                bench_host(scenario, iterations, profile)?
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
        rss_kib: max_rss_kib(),
        cold_total: cold_total_stats,
        cold_prepare: cold_prepare_stats,
        cold_run: cold_run_stats,
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

fn bench_host(scenario: Scenario, iterations: usize, profile: LoadProfile) -> Result<BenchResult> {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/run_host.py");
    let uv = which("uv")
        .context("`uv` command not found on PATH. Install from https://docs.astral.sh/uv/ or ensure it is available before running the perf suite.")?;
    let mut cmd = Command::new(uv);
    cmd.arg("run");
    cmd.arg(format!("--python={}", host_python_version()));
    for pkg in scenario_packages(scenario) {
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
        mode: Mode::HostPython,
        profile,
        invocation: None,
        path: None,
        cleanup: None,
        iterations,
        total: result.total,
        prepare: None,
        run: None,
        rss_kib: Some(result.rss_kib),
        cold_total: None,
        cold_prepare: None,
        cold_run: None,
    })
}

fn scenario_source(scenario: Scenario) -> String {
    match scenario {
        Scenario::Echo => perf::echo_script().to_owned(),
        Scenario::Numpy => perf::numpy_script().to_owned(),
        Scenario::Pandas => perf::pandas_script().to_owned(),
    }
}

fn scenario_manifest(scenario: Scenario, invocation: InvocationKind) -> String {
    let packages = scenario_packages(scenario);
    let mut manifest = json!({
        "schemaVersion": "1.0",
        "entrypoint": "main:main",
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
    profile: LoadProfile,
) -> Option<InvocationDescriptor> {
    match invocation {
        InvocationKind::Json => None,
        InvocationKind::RawCtx => {
            let mut descriptor = InvocationDescriptor::new("main:main");
            let metadata = match scenario {
                Scenario::Echo => RawCtxPublishBuilder::new("echo-output")
                    .transform("memoryview")
                    .metadata(json!({"kind": "echo", "profile": profile.name()}))
                    .build(),
                Scenario::Numpy => RawCtxPublishBuilder::new("numpy-output")
                    .transform("memoryview")
                    .metadata(json!({"dtype": "float64_le", "profile": profile.name()}))
                    .build(),
                Scenario::Pandas => RawCtxPublishBuilder::new("pandas-output")
                    .transform("memoryview")
                    .metadata(json!({"encoding": "utf8", "profile": profile.name()}))
                    .build(),
            };
            descriptor.outputs.push(FieldDescriptor {
                name: "result".to_owned(),
                type_tag: None,
                metadata: Some(metadata),
            });
            match scenario {
                Scenario::Echo => descriptor.inputs.push(FieldDescriptor {
                    name: "payload".to_owned(),
                    type_tag: None,
                    metadata: Some(
                        RawCtxBindingBuilder::new()
                            .raw_arg("payload")
                            .optional(true)
                            .build(),
                    ),
                }),
                Scenario::Numpy => descriptor.inputs.push(FieldDescriptor {
                    name: "control".to_owned(),
                    type_tag: None,
                    metadata: Some(
                        RawCtxBindingBuilder::new()
                            .raw_arg("payload")
                            .optional(true)
                            .build(),
                    ),
                }),
                Scenario::Pandas => descriptor.inputs.push(FieldDescriptor {
                    name: "control".to_owned(),
                    type_tag: None,
                    metadata: Some(
                        RawCtxBindingBuilder::new()
                            .raw_arg("payload")
                            .optional(true)
                            .build(),
                    ),
                }),
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
    }
}

fn build_bundle(source: &str, manifest: &[u8]) -> Result<Bundle> {
    use zip::write::FileOptions;
    use zip::CompressionMethod;

    let mut buffer = Vec::new();
    {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut buffer));
        let options = FileOptions::default().compression_method(CompressionMethod::Deflated);
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
    let min = samples
        .iter()
        .map(|d| d.as_secs_f64())
        .fold(f64::INFINITY, f64::min);
    let max = samples
        .iter()
        .map(|d| d.as_secs_f64())
        .fold(f64::NEG_INFINITY, f64::max);
    let sum: f64 = samples.iter().map(|d| d.as_secs_f64()).sum();
    let avg = sum / samples.len() as f64;
    TimingStats {
        avg_ms: avg * 1000.0,
        min_ms: min * 1000.0,
        max_ms: max * 1000.0,
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
        "scenario,profile,mode,invocation,path,cleanup,iterations,avg_ms,min_ms,max_ms,rss_kib,prepare_avg_ms,run_avg_ms"
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
            "{},{},{},{},{},{},{},{:.2},{:.2},{:.2},{},{},{}",
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
            result.rss_kib.unwrap_or_default(),
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
        "RSS (KiB)",
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
            .rss_kib
            .map(|value| value.to_string())
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
            Cell::new(rss.clone()),
        ]);
    }

    println!("{}", table);
}

fn host_python_version() -> &'static str {
    "3.12"
}

#[derive(Serialize, serde::Deserialize)]
struct HostResult {
    total: TimingStats,
    rss_kib: u64,
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
        }
    }
}

impl Scenario {
    const VARIANTS: &'static [&'static str] = &["echo", "numpy", "pandas"];
}

impl LoadProfile {
    const VARIANTS: &'static [&'static str] = &["none", "low", "medium", "high"];

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
    }
}

impl std::str::FromStr for Scenario {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "echo" => Ok(Scenario::Echo),
            "numpy" => Ok(Scenario::Numpy),
            "pandas" => Ok(Scenario::Pandas),
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
            "host-python" | "host" | "python" => Ok(Mode::HostPython),
            other => Err(format!("unknown mode '{other}'")),
        }
    }
}

#[cfg(unix)]
fn max_rss_kib() -> Option<u64> {
    unsafe {
        let mut usage: libc::rusage = std::mem::zeroed();
        if libc::getrusage(libc::RUSAGE_SELF, &mut usage) != 0 {
            return None;
        }
        #[cfg(target_os = "macos")]
        {
            Some((usage.ru_maxrss as u64) / 1024)
        }
        #[cfg(not(target_os = "macos"))]
        {
            Some(usage.ru_maxrss as u64)
        }
    }
}

#[cfg(not(unix))]
fn max_rss_kib() -> Option<u64> {
    None
}
