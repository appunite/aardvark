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
- `mise` (or any toolchain manager capable of installing Python 3.12) – we use
  it in documentation for reproducible instructions.
- Pyodide assets already downloaded (see [Host Integration – Preparing Pyodide
  assets](../api/rust-host.md#preparing-pyodide-assets)).

Ensure a matching CPython is available (Pyodide 0.28.2 targets Python 3.12):

```sh
mise install python@3.12
mise exec python@3.12 -- python --version
```

## Running the Benchmarks

The harness reads Pyodide wheels from the directory referenced by
`AARDVARK_PYODIDE_PACKAGE_DIR` (the `Makefile` forwards `PYODIDE_DIR`, which
defaults to `./.aardvark/pyodide/0.28.2`). Stage the upstream release and copy
the contents of `pyodide/v0.28.2/full/` (or `core/`) into that directory so the
runtime can serve requests like `pyodide/v0.28.2/full/numpy-….whl` from a flat
layout.

For the cold and warm paths each iteration spins up a fresh runtime, installs
the requested packages from the local cache, prepares the bundle, and executes
the entrypoint. The persistent rows keep a `BundlePool` isolate hot between
calls (`CleanupMode::Full`), highlighting the latency win from skipping the
hydration step.

From the repository root:

```sh
make perf-all ITERATIONS=10
```

By default the harness iterates through every workload and load profile,
printing a combined table. Sample console output (2 iterations per profile on
an M2 Max):

```
┌──────────┬──────────┬──────────────────────────────┬────────────┬──────────────┬─────────┬─────────┬─────────┬───────────┐
│ Scenario │ Profile  │ Mode                         │ Invocation │ Path         │ Avg ms  │ Min ms  │ Max ms  │ RSS (KiB) │
╞══════════╪══════════╪══════════════════════════════╪════════════╪══════════════╪═════════╪═════════╪═════════╪═══════════╡
│ echo     │ none     │ aardvark-json-persistent     │ json       │ persistent   │ 38.5    │ 3.8     │ 76.2    │ 620800    │
│ echo     │ medium   │ aardvark-json-persistent     │ json       │ persistent   │ 80.7    │ 5.9     │ 155.6   │ 642448    │
│ echo     │ high     │ aardvark-json-persistent     │ json       │ persistent   │ 35.1    │ 3.8     │ 157.5   │ 652928    │
│ numpy    │ medium   │ aardvark-json-persistent     │ json       │ persistent   │ 246.0   │ 25.9    │ 466.1   │ 940160    │
│ numpy    │ high     │ aardvark-rawctx-persistent   │ rawctx     │ persistent   │ 129.3   │ 37.6    │ 493.6   │ 965312    │
│ pandas   │ medium   │ aardvark-json-persistent     │ json       │ persistent   │ 438.2   │ 72.8    │ 1897.8  │ 1765808   │
│ pandas   │ high     │ aardvark-rawctx-persistent   │ rawctx     │ persistent   │ 450.3   │ 84.7    │ 1911.0  │ 1970048   │
│ numpy    │ high     │ host-python                  │ -          │ -            │ 0.96    │ 0.23    │ 1.99    │ 38976     │
└──────────┴──────────┴──────────────────────────────┴────────────┴──────────────┴─────────┴─────────┴─────────┴───────────┘
```

The JSON/CSV artefacts live under `target/perf/` and include the same
information (one row per scenario/profile/path/mode combination).


### Single Scenario

To benchmark one combination:

```sh
cargo run -p aardvark-perf -- scenario \
  --scenario pandas \
  --mode aardvark-json-persistent \
  --profile medium \
  --iterations 25
```

## Host Python Runner

The harness shells out to:

```sh
uv run --python 3.12 --with numpy --with pandas \
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
