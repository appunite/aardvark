use std::collections::BTreeSet;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use aardvark_core::{
    config::{PyRuntimeConfig, ResetPolicy, WarmState},
    invocation::{FieldDescriptor, InvocationDescriptor},
    outcome::ResultPayload,
    pool::{PoolConfig, PoolResetMode, PyRuntimePool},
    strategy::{
        JsonInput, JsonInvocationStrategy, RawCtxBindingBuilder, RawCtxInput,
        RawCtxInvocationStrategy, RawCtxMetadata, RawCtxPublishBuilder,
    },
    Bundle, BundleArtifact, BundlePool, BundlePoolRegistry, IsolateConfig, PoolOptions,
    PyRunnerError, PyRuntime, WarmedBundleHost, WarmedBundleHostOptions, WarmedBundleHostRegistry,
    WarmedBundleHostWarmup,
};
use anyhow::{anyhow, Context, Result};
use bytes::Bytes;
use clap::Parser;
use comfy_table::{presets::UTF8_FULL_CONDENSED, Cell, Table};
use serde::Serialize;
use serde_json::{json, Value as JsonValue};
use which::which;

mod aardvark;
mod host;
mod perf;
mod report;
mod rss;
mod scenarios;

use aardvark::{bench_aardvark, AardvarkBenchOptions};
use host::{aardvark_modes_for_scenario, bench_host, host_modes_for_scenario};
use report::{expand_results, print_summary, write_csv, write_json};

mod model;

use model::*;

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
    #[serde(skip_serializing_if = "Option::is_none")]
    samples: Option<TimingSamples>,
    #[serde(skip_serializing_if = "Option::is_none")]
    setup_breakdown: Option<SetupBreakdownStats>,
    #[serde(skip_serializing_if = "Option::is_none")]
    setup_breakdown_samples: Option<SetupBreakdownSamples>,
    #[serde(skip_serializing_if = "Option::is_none")]
    setup_pool_desired_size: Option<usize>,
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

#[derive(Clone, Serialize)]
struct TimingSamples {
    total_ms: Vec<f64>,
    prepare_ms: Vec<f64>,
    run_ms: Vec<f64>,
}

#[derive(Clone, Serialize)]
struct SetupBreakdownStats {
    registry_init: TimingStats,
    artifact_parse: TimingStats,
    pool_create: TimingStats,
    handler_prepare: TimingStats,
    #[serde(skip_serializing_if = "Option::is_none")]
    warm_all: Option<TimingStats>,
}

#[derive(Clone, Serialize)]
struct SetupBreakdownSamples {
    registry_init_ms: Vec<f64>,
    artifact_parse_ms: Vec<f64>,
    pool_create_ms: Vec<f64>,
    handler_prepare_ms: Vec<f64>,
    warm_all_ms: Vec<f64>,
}

#[derive(Default)]
struct SetupBreakdownBuckets {
    registry_init: Vec<Duration>,
    artifact_parse: Vec<Duration>,
    pool_create: Vec<Duration>,
    handler_prepare: Vec<Duration>,
    warm_all: Vec<Duration>,
}

struct TimingBuckets<'a> {
    prepare: &'a mut Vec<Duration>,
    run: &'a mut Vec<Duration>,
    total: &'a mut Vec<Duration>,
}

#[derive(Clone, Copy)]
struct AardvarkBenchContext {
    scenario: Scenario,
    profile: LoadProfile,
    invocation: InvocationKind,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli {
        Cli::All {
            iterations,
            json,
            csv,
            profile,
            samples,
            pyodide_profile,
            warm_preimports,
            manifest_preload_imports,
            setup_pool_desired_size,
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
                    Scenario::NumpyMatmul,
                    Scenario::Pandas,
                    Scenario::ScipySgemm,
                    Scenario::Tensor,
                    Scenario::Matplotlib,
                ] {
                    for mode in aardvark_modes_for_scenario(scenario) {
                        results.push(bench_aardvark(
                            scenario,
                            *mode,
                            AardvarkBenchOptions {
                                iterations,
                                profile,
                                include_samples: samples,
                                pyodide_profile: pyodide_profile.as_deref(),
                                warm_preimports: &warm_preimports,
                                manifest_preload_imports: &manifest_preload_imports,
                                setup_pool_desired_size,
                            },
                        )?);
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
            samples,
            pyodide_profile,
            warm_preimports,
            manifest_preload_imports,
            setup_pool_desired_size,
        } => {
            let profile = profile.unwrap_or(LoadProfile::None);
            let result = if mode.is_aardvark() {
                bench_aardvark(
                    scenario,
                    mode,
                    AardvarkBenchOptions {
                        iterations,
                        profile,
                        include_samples: samples,
                        pyodide_profile: pyodide_profile.as_deref(),
                        warm_preimports: &warm_preimports,
                        manifest_preload_imports: &manifest_preload_imports,
                        setup_pool_desired_size,
                    },
                )?
            } else {
                bench_host(scenario, mode, iterations, profile)?
            };
            let expanded = expand_results(std::slice::from_ref(&result));
            println!("{}", serde_json::to_string_pretty(&expanded)?);
        }
    }
    Ok(())
}
