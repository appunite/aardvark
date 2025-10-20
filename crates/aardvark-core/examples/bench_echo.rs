use aardvark_core::{outcome::ResultPayload, Bundle, PyRuntime, PyRuntimeConfig};
use std::io::{Cursor, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use zip::write::FileOptions;
use zip::ZipWriter;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let iterations: usize = args
        .next()
        .map(|value| {
            value
                .parse()
                .expect("iterations must be a positive integer")
        })
        .unwrap_or(100);
    let payload_len: usize = args
        .next()
        .map(|value| {
            value
                .parse()
                .expect("payload length must be a positive integer")
        })
        .unwrap_or(1024);

    println!(
        "bench_echo: iterations={} payload_len={} bytes",
        iterations, payload_len
    );

    let mut config = PyRuntimeConfig::default();
    config.snapshot.save_to = Some(PathBuf::from("target/bench_echo.snapshot"));
    let mut runtime = PyRuntime::new(config)?;
    let bundle = build_echo_bundle(payload_len)?;

    let warm_session = runtime.prepare_session(bundle.clone(), "main:main")?;
    let warm_outcome = runtime.run_session(&warm_session)?;
    assert!(
        warm_outcome.is_success(),
        "warmup run failed: {:?}",
        warm_outcome.status
    );
    runtime.capture_warm_state()?;
    runtime.reset_in_place()?;

    let mut phases = PhaseStats::default();

    for _ in 0..iterations {
        runtime.reset_in_place()?;

        let prepare_start = Instant::now();
        let session = runtime.prepare_session(bundle.clone(), "main:main")?;
        let prepare = prepare_start.elapsed();

        let run_start = Instant::now();
        let outcome = runtime.run_session(&session)?;
        let run = run_start.elapsed();

        if let Some(ResultPayload::Text(_value)) = outcome.payload() {
            // optional inspection: `_value` now ignored.
        }

        phases.record(prepare, run);
    }

    println!(
        "phases: prepare=avg {:.2} ms (min {:.2}, max {:.2}) · run=avg {:.2} ms (min {:.2}, max {:.2}) · total=avg {:.2} ms (min {:.2}, max {:.2})",
        phases.prepare.avg_ms(),
        phases.prepare.min_ms(),
        phases.prepare.max_ms(),
        phases.run.avg_ms(),
        phases.run.min_ms(),
        phases.run.max_ms(),
        phases.total.avg_ms(),
        phases.total.min_ms(),
        phases.total.max_ms()
    );

    Ok(())
}

fn build_echo_bundle(payload_len: usize) -> Result<Bundle, Box<dyn std::error::Error>> {
    let source = format!(
        r#"PAYLOAD = "x" * {payload_len}

def main():
    return PAYLOAD
"#,
        payload_len = payload_len
    );

    let cursor = Cursor::new(Vec::new());
    let mut writer = ZipWriter::new(cursor);
    let options = FileOptions::default();
    writer.start_file("main.py", options)?;
    writer.write_all(source.as_bytes())?;
    let cursor = writer.finish()?;
    let bytes = cursor.into_inner();
    Ok(Bundle::from_zip_bytes(bytes)?)
}

#[derive(Default)]
struct PhaseStats {
    prepare: Stat,
    run: Stat,
    total: Stat,
}

impl PhaseStats {
    fn record(&mut self, prepare: Duration, run: Duration) {
        self.prepare.push(prepare);
        self.run.push(run);
        self.total.push(prepare + run);
    }
}

#[derive(Default)]
struct Stat {
    count: usize,
    sum: Duration,
    min: Duration,
    max: Duration,
}

impl Stat {
    fn push(&mut self, value: Duration) {
        if self.count == 0 {
            self.min = value;
            self.max = value;
        } else {
            if value < self.min {
                self.min = value;
            }
            if value > self.max {
                self.max = value;
            }
        }
        self.count += 1;
        self.sum += value;
    }

    fn avg_ms(&self) -> f64 {
        if self.count == 0 {
            return 0.0;
        }
        (self.sum.as_secs_f64() * 1000.0) / self.count as f64
    }

    fn min_ms(&self) -> f64 {
        self.min.as_secs_f64() * 1000.0
    }

    fn max_ms(&self) -> f64 {
        self.max.as_secs_f64() * 1000.0
    }
}
