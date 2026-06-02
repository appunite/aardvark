# Performance Benchmarks

This suite measures a few representative workloads across the Aardvark runtime
and a native CPython interpreter. Every workload is executed at four **load
profiles** so we can observe how latency scales with input size:

- **None** – no explicit input; the handler uses its baked-in defaults.
- **Low** – roughly 10² logical items (16 bytes for `echo`, 64 scalars for
  `numpy`, 128 rows for `pandas`, 256 tensor elements).
- **Medium** – roughly 10³–10⁴ logical items (1 000 bytes / 4 096 scalars /
  10 000 rows / 16 384 tensor elements).
- **High** – roughly 10⁶ logical items (1 MB / 1 000 000 scalars /
  1 000 000 rows / 262 144 tensor elements).

Workloads:

- **Echo** – echoes the provided payload.
- **NumPy** – applies deterministic sine transforms and matrix multiplies based
  on the requested size and returns a scalar aggregate.
- **Pandas** – aggregates a deterministic DataFrame with repeatable groups and
  returns a JSON summary.
- **Tensor** – consumes dense `float32` tensors, applying transcendental
  transforms and publishing the result through RawCtx as a zero-copy binary
  buffer. The JSON path exercises the same computation but serialises the
  tensor as a list of floats, highlighting the bandwidth cost of JSON.

Each workload/profile pair is exercised through four Aardvark paths—cold
start, warm snapshot, reset-in-place pooling, and the persistent isolate
pool—and through the host Python interpreter. The harness records
average/min/max wall-clock latency per invocation plus the peak RSS reported by
the OS.

## Requirements

- `uv` for ephemeral Python environments: <https://docs.astral.sh/uv/>
- `mise` (or any toolchain manager capable of installing Python 3.13) – we use
  it in documentation for reproducible instructions.
- [Pyodide](https://pyodide.org/) assets already staged (see [Host Integration – Preparing Pyodide assets](../api/rust-host.md#preparing-pyodide-assets)).

Ensure a matching CPython is available ([Pyodide](https://pyodide.org/) 0.29.4 targets Python 3.13.2):

```sh
mise install python@3.13
mise exec python@3.13 -- python --version
```

## Running the Benchmarks

The harness reads [Pyodide](https://pyodide.org/) runtime files and wheels from the
directory referenced by `AARDVARK_PYODIDE_DIST_DIR` (the `Makefile` forwards
`PYODIDE_DIST_DIR`, which defaults to the full staged distribution for the
pinned Aardvark/Pyodide versions). A quick setup looks like:

```sh
cargo run -p aardvark-cli -- assets stage --variant full
export PYODIDE_DIST_DIR="$PWD/.aardvark/pyodide-distributions/aardvark-0.1.1-pyodide-v0.29.4-full"
cargo run -p aardvark-cli -- assets verify "$PYODIDE_DIST_DIR"
```

The top-level `Makefile` now exposes helper targets. Run `make help` to see the
available commands and the current environment defaults:

```
$ make help
Available targets:
  make perf-all     Run the full perf suite (JSON/CSV artefacts).
  make perf-md      Generate Markdown summary (runs perf-all first).
  make setup-python Install Python 3.13 via mise (used by host runner).
Variables:
  PYODIDE_VERSION=0.29.4
  PYODIDE_DIST_DIR=/.../.aardvark/pyodide-distributions/aardvark-0.1.1-pyodide-v0.29.4-full
  ITERATIONS=25
```

Run `make setup-python` once per machine to install the host-side Python
interpreter used for the comparative baseline.

`make perf-all` executes the full matrix (`aardvark-perf all …`) and writes both
JSON and CSV artefacts under `target/perf/`. To inspect the Markdown table
directly call `make perf-md`, which invokes the same run and pipes the JSON into
`perf/scripts/render_markdown.py`.

For the cold and warm paths each iteration spins up a fresh runtime, installs
the requested packages from the staged distribution, prepares the bundle, and executes
the entrypoint. The persistent rows keep a `BundlePool` isolate hot between
calls (`CleanupMode::Full` unless noted), highlighting the latency win from
skipping the hydration step.

Sample console output (abbreviated; numbers will vary with hardware and
iteration count):

```
┌──────────┬──────────┬──────────────────────────────┬────────────┬──────────────┬───────────────┬──────┬────────┬────────┬────────┬────────┬────────┬────────┬──────────┐
│ Scenario │ Profile  │ Mode                         │ Invocation │ Path         │ Cleanup       │ Iter │ Avg ms │ Min ms │ Max ms │ Std ms │ P50 ms │ P95 ms │ RSS (MiB)│
├──────────┼──────────┼──────────────────────────────┼────────────┼──────────────┼───────────────┼──────┼────────┼────────┼────────┼────────┼────────┼────────┼──────────┤
│ echo     │ low      │ aardvark-json-persistent     │ json       │ persistent   │ full          │ 25   │  42.10 │   3.95 │  88.42 │  19.32 │   5.92 │  76.73 │   607.12 │
│ numpy    │ medium   │ aardvark-rawctx-persistent   │ rawctx     │ persistent   │ shared-buffers-only │ 25   │ 210.54 │  38.22 │ 454.11 │ 123.47 │  68.09 │ 392.77 │   942.65 │
│ pandas   │ high     │ aardvark-json-cold           │ json       │ cold         │ -             │ 25   │ 790.31 │ 765.42 │ 812.55 │   9.88 │ 789.77 │ 803.91 │  2184.40 │
│ pandas   │ high     │ host-python                  │ -          │ -            │ -             │ 25   │   1.12 │   0.18 │   2.02 │   0.39 │   0.96 │   1.78 │    37.88 │
└──────────┴──────────┴──────────────────────────────┴────────────┴──────────────┴───────────────┴──────┴────────┴────────┴────────┴────────┴────────┴────────┴──────────┘
```

The JSON/CSV artefacts live under `target/perf/` and include the same
information (one row per scenario/profile/path/mode combination).


### Single Scenario

To benchmark one combination set `AARDVARK_PYODIDE_DIST_DIR` (or export
`PYODIDE_DIST_DIR` before invoking make) and run:

```sh
AARDVARK_PYODIDE_DIST_DIR=$PYODIDE_DIST_DIR cargo run -p aardvark-perf -- scenario \
  --scenario pandas \
  --mode aardvark-json-persistent \
  --profile medium \
  --iterations 25
```

## Host Python Runner

The harness shells out to:

```sh
uv run --python 3.13 --with numpy --with pandas \
  python perf/fixtures/run_host.py --scenario pandas --profile medium --iterations 25
```

`uv` ensures the requested packages are available without modifying the user’s
environment.

## Generating Markdown Tables

A helper script converts the JSON output into a Markdown table for reports:

```sh
python perf/scripts/render_markdown.py target/perf/results.json > target/perf/results.md
```

Or, if you prefer the Makefile wrapper:

```sh
make perf-md
```

The script reads the JSON emitted by `aardvark-perf` and prints a table grouped
by scenario.

## Extending the Suite

- Add new Python workloads under `perf/fixtures/scenarios/` (one module per
  workload/profile) and list them in `perf/runner/src/perf/mod.rs`.
- Update `Scenario` in `perf/runner/src/main.rs` with the matching metadata
  (packages, manifest).
- For more granular metrics (per-phase timings, CPU, warm snapshot size), extend
  the `BenchResult` struct and add the necessary instrumentation in
  `bench_aardvark`. Keep a note in the internal diary when introducing new
  metrics so we can track follow-up work.
