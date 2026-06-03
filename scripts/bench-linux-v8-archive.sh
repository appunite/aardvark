#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO_ROOT=$(cd "$SCRIPT_DIR/.." && pwd)

default_v8_version() {
  awk -F'"' '/^[[:space:]]*v8 = "=/ { gsub(/^=/, "", $2); print $2; exit }' "$REPO_ROOT/Cargo.toml"
}

print_usage() {
  cat <<'EOF'
bench-linux-v8-archive.sh - compare upstream and custom rusty_v8 executable perf.

Usage:
  bench-linux-v8-archive.sh <custom-archive.a> [options]

Options:
  --v8-version VERSION     v8 crate version to benchmark (default: workspace pin)
  --work-dir DIR           scratch directory (default: tmp/v8-linux-shared-archive/bench)
  --out-dir DIR            results directory (default: target/v8-linux-shared-archive/perf)
  --iterations N           measured iterations per scenario (default: 75)
  --warmup N               warmup iterations per scenario (default: 20)
  --max-ratio N            max custom/upstream median ratio before failure (default: 1.05)
  --keep-work-dir          keep scratch directory after the run
  -h, --help               show this help

The benchmark compares normal Linux executable builds. It is a guard against
accidentally accepting a custom V8 archive that is materially slower than the
upstream prebuilt archive for ordinary executable use.
EOF
}

require_option_value() {
  local option="$1"
  local value="${2:-}"

  if [[ -z "$value" ]]; then
    echo "error: $option requires a value" >&2
    exit 1
  fi
}

if [[ $# -gt 0 ]]; then
  case "$1" in
    -h|--help|help)
      print_usage
      exit 0
      ;;
  esac
fi

if [[ $# -lt 1 ]]; then
  print_usage >&2
  exit 1
fi

ARCHIVE="$1"
shift

V8_VERSION=$(default_v8_version)
WORK_DIR="$REPO_ROOT/tmp/v8-linux-shared-archive/bench"
OUT_DIR="$REPO_ROOT/target/v8-linux-shared-archive/perf"
ITERATIONS=75
WARMUP=20
MAX_RATIO=1.05
KEEP_WORK_DIR=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --v8-version)
      require_option_value "$1" "${2:-}"
      V8_VERSION="$2"
      shift 2
      ;;
    --work-dir)
      require_option_value "$1" "${2:-}"
      WORK_DIR="$2"
      shift 2
      ;;
    --out-dir)
      require_option_value "$1" "${2:-}"
      OUT_DIR="$2"
      shift 2
      ;;
    --iterations)
      require_option_value "$1" "${2:-}"
      ITERATIONS="$2"
      shift 2
      ;;
    --warmup)
      require_option_value "$1" "${2:-}"
      WARMUP="$2"
      shift 2
      ;;
    --max-ratio)
      require_option_value "$1" "${2:-}"
      MAX_RATIO="$2"
      shift 2
      ;;
    --keep-work-dir)
      KEEP_WORK_DIR=1
      shift
      ;;
    -h|--help|help)
      print_usage
      exit 0
      ;;
    *)
      echo "error: unknown argument '$1'" >&2
      print_usage >&2
      exit 1
      ;;
  esac
done

if [[ "$(uname -s)" != "Linux" || "$(uname -m)" != "x86_64" ]]; then
  echo "error: this benchmark must run on x86_64 Linux" >&2
  echo "host: $(uname -s) $(uname -m)" >&2
  exit 1
fi

for tool in cargo rustc python3 sha256sum; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "error: required tool '$tool' is missing" >&2
    exit 1
  fi
done

if [[ ! -s "$ARCHIVE" ]]; then
  echo "error: archive does not exist or is empty: $ARCHIVE" >&2
  exit 1
fi

ARCHIVE=$(cd "$(dirname "$ARCHIVE")" && pwd)/$(basename "$ARCHIVE")
mkdir -p "$(dirname "$WORK_DIR")"
WORK_DIR=$(cd "$(dirname "$WORK_DIR")" && pwd)/$(basename "$WORK_DIR")
OUT_DIR=$(mkdir -p "$OUT_DIR" && cd "$OUT_DIR" && pwd)

if ! [[ "$ITERATIONS" =~ ^[0-9]+$ ]]; then
  echo "error: --iterations must be a positive integer" >&2
  exit 1
fi

if ! [[ "$WARMUP" =~ ^[0-9]+$ ]]; then
  echo "error: --warmup must be a non-negative integer" >&2
  exit 1
fi

if [[ "$ITERATIONS" -lt 1 ]]; then
  echo "error: --iterations must be at least 1" >&2
  exit 1
fi

if [[ "$WARMUP" -lt 0 ]]; then
  echo "error: --warmup must be non-negative" >&2
  exit 1
fi

rm -rf "$WORK_DIR"
mkdir -p "$WORK_DIR/v8_bench/src" "$OUT_DIR"

if [[ "$KEEP_WORK_DIR" -eq 0 ]]; then
  trap 'rm -rf "$WORK_DIR"' EXIT
fi

cat > "$WORK_DIR/Cargo.toml" <<EOF
[workspace]
members = ["v8_bench"]
resolver = "2"

[workspace.package]
edition = "2021"
rust-version = "1.96"
EOF

cat > "$WORK_DIR/v8_bench/Cargo.toml" <<EOF
[package]
name = "v8_bench"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true

[dependencies]
v8 = "=$V8_VERSION"
EOF

cat > "$WORK_DIR/v8_bench/src/main.rs" <<'EOF'
use std::sync::OnceLock;
use std::time::Instant;

static V8_PLATFORM: OnceLock<v8::SharedRef<v8::Platform>> = OnceLock::new();

struct Scenario {
    name: &'static str,
    source: &'static str,
}

fn init_v8() {
    V8_PLATFORM.get_or_init(|| {
        let platform = v8::new_default_platform(0, false);
        let shared = platform.make_shared();
        v8::V8::initialize_platform(shared.clone());
        v8::V8::initialize();
        shared
    });
}

fn run_script(source_text: &str) -> Result<i64, String> {
    let create_params = v8::CreateParams::default().array_buffer_allocator(v8::new_default_allocator());
    let mut isolate = v8::Isolate::new(create_params);
    let result = {
        v8::scope!(let scope, &mut isolate);
        let context = v8::Context::new(scope, v8::ContextOptions::default());
        let mut context_scope = v8::ContextScope::new(scope, context);
        let scope = &mut context_scope;
        let source = v8::String::new(scope, source_text)
            .ok_or_else(|| "source string failed".to_string())?;
        let script = v8::Script::compile(scope, source, None)
            .ok_or_else(|| "compile failed".to_string())?;
        let value = script.run(scope).ok_or_else(|| "run failed".to_string())?;
        value.integer_value(scope).ok_or_else(|| "integer conversion failed".to_string())?
    };
    Ok(result)
}

fn percentile(sorted: &[u128], percent: usize) -> u128 {
    let index = ((sorted.len() - 1) * percent) / 100;
    sorted[index]
}

fn main() {
    init_v8();

    let iterations: usize = std::env::var("AARDVARK_V8_BENCH_ITERATIONS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(75);
    let warmup: usize = std::env::var("AARDVARK_V8_BENCH_WARMUP")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(20);

    let scenarios = [
        Scenario {
            name: "startup_add",
            source: "21 * 2",
        },
        Scenario {
            name: "arithmetic_loop",
            source: "let acc = 0; for (let i = 0; i < 750000; i++) { acc = (acc + ((i * 13) % 97)) | 0; } acc;",
        },
        Scenario {
            name: "object_alloc",
            source: "const xs = []; for (let i = 0; i < 70000; i++) { xs.push({i, v: i % 17}); } xs.length;",
        },
        Scenario {
            name: "json_parse_stringify",
            source: "const text = '[' + Array.from({length: 18000}, (_, i) => '{\"id\":' + i + ',\"v\":' + (i % 31) + '}').join(',') + ']'; const parsed = JSON.parse(text); JSON.stringify(parsed).length;",
        },
    ];

    println!("scenario,iterations,warmup,median_ns,p95_ns,min_ns,max_ns");
    for scenario in scenarios {
        for _ in 0..warmup {
            let _ = run_script(scenario.source).expect("warmup failed");
        }

        let mut samples = Vec::with_capacity(iterations);
        for _ in 0..iterations {
            let started = Instant::now();
            let _ = run_script(scenario.source).expect("scenario failed");
            samples.push(started.elapsed().as_nanos());
        }
        samples.sort_unstable();
        let median = percentile(&samples, 50);
        let p95 = percentile(&samples, 95);
        let min = samples[0];
        let max_value = *samples.last().unwrap();
        println!(
            "{},{},{},{},{},{},{}",
            scenario.name, iterations, warmup, median, p95, min, max_value
        );
    }
}
EOF

echo "Benchmarking upstream and custom V8 archives"
echo "  custom archive=$ARCHIVE"
echo "  custom sha256=$(sha256sum "$ARCHIVE" | awk '{print $1}')"
echo "  v8=$V8_VERSION"
echo "  iterations=$ITERATIONS"
echo "  warmup=$WARMUP"
echo "  max ratio=$MAX_RATIO"

UPSTREAM_V8_ENV_UNSETS=(
  -u RUSTY_V8_ARCHIVE
  -u RUSTY_V8_MIRROR
  -u RUSTY_V8_SRC_BINDING_PATH
  -u V8_FROM_SOURCE
  -u GN_ARGS
  -u EXTRA_GN_ARGS
  -u PRINT_GN_ARGS
  -u V8_FORCE_DEBUG
)

CUSTOM_V8_ENV_UNSETS=(
  -u RUSTY_V8_MIRROR
  -u RUSTY_V8_SRC_BINDING_PATH
  -u V8_FROM_SOURCE
  -u GN_ARGS
  -u EXTRA_GN_ARGS
  -u PRINT_GN_ARGS
  -u V8_FORCE_DEBUG
)

(
  cd "$WORK_DIR"
  env "${UPSTREAM_V8_ENV_UNSETS[@]}" CARGO_TARGET_DIR="$WORK_DIR/target-upstream" cargo build -p v8_bench --release
  env "${CUSTOM_V8_ENV_UNSETS[@]}" RUSTY_V8_ARCHIVE="$ARCHIVE" CARGO_TARGET_DIR="$WORK_DIR/target-custom" cargo build -p v8_bench --release
  AARDVARK_V8_BENCH_ITERATIONS="$ITERATIONS" AARDVARK_V8_BENCH_WARMUP="$WARMUP" \
    "$WORK_DIR/target-upstream/release/v8_bench" > "$OUT_DIR/upstream.csv"
  AARDVARK_V8_BENCH_ITERATIONS="$ITERATIONS" AARDVARK_V8_BENCH_WARMUP="$WARMUP" \
    "$WORK_DIR/target-custom/release/v8_bench" > "$OUT_DIR/custom.csv"
)

python3 - "$OUT_DIR/upstream.csv" "$OUT_DIR/custom.csv" "$OUT_DIR/comparison.csv" "$MAX_RATIO" <<'PY'
import csv
import sys
from pathlib import Path

upstream_path = Path(sys.argv[1])
custom_path = Path(sys.argv[2])
comparison_path = Path(sys.argv[3])
max_ratio = float(sys.argv[4])

def load(path):
    with path.open(newline="") as handle:
        return {row["scenario"]: row for row in csv.DictReader(handle)}

upstream = load(upstream_path)
custom = load(custom_path)

failed = []
with comparison_path.open("w", newline="") as handle:
    fieldnames = [
        "scenario",
        "upstream_median_ns",
        "custom_median_ns",
        "custom_to_upstream_ratio",
        "status",
    ]
    writer = csv.DictWriter(handle, fieldnames=fieldnames)
    writer.writeheader()
    for scenario in sorted(upstream):
        if scenario not in custom:
            failed.append((scenario, "missing custom result"))
            continue
        upstream_median = int(upstream[scenario]["median_ns"])
        custom_median = int(custom[scenario]["median_ns"])
        ratio = custom_median / upstream_median if upstream_median else float("inf")
        status = "pass" if ratio <= max_ratio else "fail"
        if status == "fail":
            failed.append((scenario, f"ratio {ratio:.4f} > {max_ratio:.4f}"))
        writer.writerow(
            {
                "scenario": scenario,
                "upstream_median_ns": upstream_median,
                "custom_median_ns": custom_median,
                "custom_to_upstream_ratio": f"{ratio:.6f}",
                "status": status,
            }
        )

print(comparison_path.read_text(), end="")
if failed:
    for scenario, reason in failed:
        print(f"perf guard failed: {scenario}: {reason}", file=sys.stderr)
    sys.exit(1)
PY

echo "Perf comparison written to $OUT_DIR/comparison.csv"
