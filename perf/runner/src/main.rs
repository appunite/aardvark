use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use aardvark_core::{Bundle, PyRuntime, PyRuntimeConfig};
use anyhow::{anyhow, Context, Result};
use serde::Serialize;
use structopt::StructOpt;

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
    },
    /// Run a single scenario/mode combination
    Scenario {
        #[structopt(long, possible_values = Scenario::VARIANTS, case_insensitive = true)]
        scenario: Scenario,
        #[structopt(long, possible_values = Mode::VARIANTS, case_insensitive = true)]
        mode: Mode,
        #[structopt(long, default_value = "10")]
        iterations: usize,
    },
}

#[derive(Copy, Clone, Debug, Serialize)]
enum Scenario {
    Echo,
    Numpy,
    Pandas,
}

#[derive(Copy, Clone, Debug, Serialize)]
enum Mode {
    Aardvark,
    HostPython,
}

#[derive(Serialize)]
struct BenchResult {
    scenario: Scenario,
    mode: Mode,
    iterations: usize,
    total: TimingStats,
    prepare: Option<TimingStats>,
    run: Option<TimingStats>,
    rss_kib: Option<u64>,
}

#[derive(Serialize, serde::Deserialize, Default, Clone)]
struct TimingStats {
    avg_ms: f64,
    min_ms: f64,
    max_ms: f64,
}

fn main() -> Result<()> {
    let cli = Cli::from_args();
    match cli {
        Cli::All {
            iterations,
            json,
            csv,
        } => {
            let mut results = Vec::new();
            for scenario in [Scenario::Echo, Scenario::Numpy, Scenario::Pandas] {
                results.push(bench_aardvark(scenario, iterations)?);
                results.push(bench_host(scenario, iterations)?);
            }
            if let Some(path) = json {
                write_json(&path, &results)?;
            }
            if let Some(path) = csv {
                write_csv(&path, &results)?;
            }
            print_summary(&results);
        }
        Cli::Scenario {
            scenario,
            mode,
            iterations,
        } => {
            let result = match mode {
                Mode::Aardvark => bench_aardvark(scenario, iterations)?,
                Mode::HostPython => bench_host(scenario, iterations)?,
            };
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
    }
    Ok(())
}

fn bench_aardvark(scenario: Scenario, iterations: usize) -> Result<BenchResult> {
    let python_source = scenario_source(scenario);
    let manifest = scenario_manifest(scenario);
    let bundle = build_bundle(python_source, manifest.as_bytes())?;

    let snapshot_path = bundle_snapshot_path(scenario)?;
    let mut config = PyRuntimeConfig::default();
    if snapshot_path.exists() {
        config.snapshot.load_from = Some(snapshot_path.clone());
    }
    config.snapshot.save_to = Some(snapshot_path);

    let mut runtime = PyRuntime::new(config)?;

    // Warm-up run to install packages and capture warm state.
    let (session, _) = runtime.prepare_session_with_manifest(bundle.clone())?;
    runtime.run_session(&session)?;
    runtime.capture_warm_state()?;
    runtime.reset_in_place()?;

    let mut prepare = Vec::with_capacity(iterations);
    let mut run = Vec::with_capacity(iterations);
    let mut total = Vec::with_capacity(iterations);

    for _ in 0..iterations {
        runtime.reset_in_place()?;
        let prep_start = std::time::Instant::now();
        let (session, _) = runtime.prepare_session_with_manifest(bundle.clone())?;
        let prep_elapsed = prep_start.elapsed();

        let run_start = std::time::Instant::now();
        let outcome = runtime.run_session(&session)?;
        assert!(outcome.is_success(), "handler failed: {:?}", outcome.status);
        let run_elapsed = run_start.elapsed();

        prepare.push(prep_elapsed);
        run.push(run_elapsed);
        total.push(prep_elapsed + run_elapsed);
    }

    Ok(BenchResult {
        scenario,
        mode: Mode::Aardvark,
        iterations,
        total: timing_stats(&total),
        prepare: Some(timing_stats(&prepare)),
        run: Some(timing_stats(&run)),
        rss_kib: max_rss_kib(),
    })
}

fn bench_host(scenario: Scenario, iterations: usize) -> Result<BenchResult> {
    let script = Path::new("perf/fixtures/run_host.py");
    let mut cmd = Command::new("uv");
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
        iterations,
        total: result.total,
        prepare: None,
        run: None,
        rss_kib: Some(result.rss_kib),
    })
}

fn scenario_source(scenario: Scenario) -> &'static str {
    match scenario {
        Scenario::Echo => include_str!("../../fixtures/scenarios/echo.py"),
        Scenario::Numpy => include_str!("../../fixtures/scenarios/numpy_case.py"),
        Scenario::Pandas => include_str!("../../fixtures/scenarios/pandas_case.py"),
    }
}

fn scenario_manifest(scenario: Scenario) -> String {
    let packages = scenario_packages(scenario);
    serde_json::json!({
        "schemaVersion": "1.0",
        "entrypoint": "main:main",
        "packages": packages,
    })
    .to_string()
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

fn write_json(path: &Path, results: &[BenchResult]) -> Result<()> {
    let mut file =
        File::create(path).with_context(|| format!("failed to write {}", path.display()))?;
    file.write_all(serde_json::to_string_pretty(results)?.as_bytes())?;
    Ok(())
}

fn write_csv(path: &Path, results: &[BenchResult]) -> Result<()> {
    let mut file =
        File::create(path).with_context(|| format!("failed to write {}", path.display()))?;
    writeln!(
        file,
        "scenario,mode,iterations,avg_ms,min_ms,max_ms,rss_kib,prepare_avg_ms,run_avg_ms"
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
        writeln!(
            file,
            "{},{},{},{:.2},{:.2},{:.2},{},{},{}",
            result.scenario.name(),
            result.mode.name(),
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
    println!("| Scenario | Mode | Avg ms | Min ms | Max ms | RSS (KiB) |");
    println!("|----------|------|--------|--------|--------|-----------|");
    for r in results {
        println!(
            "| {} | {} | {:.2} | {:.2} | {:.2} | {} |",
            r.scenario.name(),
            r.mode.name(),
            r.total.avg_ms,
            r.total.min_ms,
            r.total.max_ms,
            r.rss_kib.unwrap_or_default()
        );
    }
}

fn bundle_snapshot_path(scenario: Scenario) -> Result<PathBuf> {
    let dir = PathBuf::from("target/perf_snapshots");
    std::fs::create_dir_all(&dir)?;
    let mut path = dir;
    path.push(format!("{}.bin", scenario.name()));
    Ok(path)
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

impl Mode {
    fn name(&self) -> &'static str {
        match self {
            Mode::Aardvark => "aardvark",
            Mode::HostPython => "host-python",
        }
    }
}

impl Mode {
    const VARIANTS: &'static [&'static str] = &["aardvark", "host-python"];
}

impl std::str::FromStr for Mode {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "aardvark" => Ok(Mode::Aardvark),
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
