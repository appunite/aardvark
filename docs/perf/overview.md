# Performance Benchmarks

This suite measures a few representative workloads across the Aardvark runtime
and a native CPython interpreter:

- **Echo** – returns a fixed 1 KB string.
- **NumPy** – multiplies two 200×200 matrices using `numpy`.
- **Pandas** – aggregates a 50 000‑row DataFrame.

Each workload is executed through Aardvark (warm snapshot, in-place resets) and
through the host Python interpreter. The harness records average/min/max
wall-clock latency per invocation and the peak RSS reported by the OS.

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

From the repository root:

```sh
make perf-all ITERATIONS=25
```

Sample console output:

```
| Scenario | Mode | Avg ms | Min ms | Max ms | RSS (KiB) |
|----------|------|--------|--------|--------|-----------|
| echo     | aardvark | 128.55 | 123.12 | 135.44 | 215000 |
| echo     | host-python | 1.42 | 1.30 | 1.71 | 10234 |
...
```

The JSON/CSV files contain the same data for further analysis and live under `target/perf/`.

### Single Scenario

To benchmark one combination:

```sh
cargo run -p aardvark-perf -- scenario \
  --scenario pandas \
  --mode aardvark \
  --iterations 50
```

## Host Python Runner

The harness shells out to:

```sh
uv run --python 3.12 --with numpy --with pandas \
  python perf/fixtures/run_host.py --scenario pandas --iterations 25
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

- Add new Python workloads under `perf/fixtures/scenarios/` and register them in
  `SCENARIOS`.
- Update `Scenario` in `perf/runner/src/main.rs` with the matching metadata
  (packages, manifest).
- For more granular metrics (per-phase timings, CPU, warm snapshot size), extend
  the `BenchResult` struct and add the necessary instrumentation in
  `bench_aardvark`. Keep a note in the internal diary when introducing new
  metrics so we can track follow-up work.
