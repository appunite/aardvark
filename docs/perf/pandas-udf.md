# Pandas UDF Performance (HAV-3465)

Measurements for running a pandas UDF through Aardvark on Pyodide 0.29.4, to
size workers and isolates per node for Python UDFs. Step 1 of the Python UDFs
plan.

## Setup

- Machine: Intel Core i5-10500T (6 cores / 12 threads, 2.3 GHz), 15 GiB RAM,
  Linux 7.0. Release build of `aardvark-perf` at `dfd4789`.
- Distribution: `aardvark-0.2.1-pyodide-v0.29.4-full` (pandas 2.3.3,
  numpy 2.2.5).
- Workload: `perf/fixtures/scenarios/pandas.py`. It builds a DataFrame and does
  a `groupby().agg(mean)` over 128 groups. `medium` = 10 000 rows,
  `high` = 1 000 000 rows. JSON invocation.
- Each row is a separate `aardvark-perf scenario` process, 10 iterations.
  "End RSS" is `/proc/self/statm` after the run. "Peak RSS" is
  `/usr/bin/time -v` max RSS for the whole process.
- The echo bundle was re-measured on the same machine as the baseline. The
  Apple M3 numbers in `report.md` are not comparable: they are about 2x faster
  on CPU, and macOS reports RSS differently. Regenerating `report.md` on this
  machine is left for a follow-up.

Command shape:

```sh
AARDVARK_PYODIDE_DIST_DIR=... target/release/aardvark-perf scenario \
  --scenario pandas --mode <mode> --profile medium --iterations 10 \
  [--setup-pool-desired-size N] [--warm-preimport numpy --warm-preimport pandas]
```

## Results

### Cold start

| Bundle | Profile | Avg ms | P95 ms | Peak RSS MiB |
| --- | --- | --: | --: | --: |
| echo | none | 2 095 | 2 146 | 332 |
| pandas | none | 6 162 | 6 319 | 717 |
| pandas | medium | 6 163 | 6 274 | 666 |
| pandas | high | 6 205 | 6 295 | 719 |

Installing and importing numpy + pandas adds about **4.1 s** on top of the
~2.1 s Pyodide boot. The workload itself barely affects cold time.

### Warm calls (isolate already prepared)

| Path | Profile | Avg ms | P50 ms | P95 ms |
| --- | --- | --: | --: | --: |
| Aardvark persistent warm-call (`aardvark-json-persistent-warm-call-no-stdio`) | medium | 8.5 | 8.7 | 9.2 |
| Aardvark persistent warm-call | high | 54.0 | 53.5 | 55.6 |
| Aardvark registry retained handler (`aardvark-json-registry-retained-handler-no-stdio`) | medium | 10.7 | 8.5 | 21.5 |
| Native CPython 3.13.2, warm handler | medium | 3.0 | 2.8 | 3.9 |
| Native CPython 3.13.2, warm handler | high | 46.7 | 44.4 | 57.3 |
| Native CPython 3.13.2, fresh process per call | medium | 338 | 329 | 377 |
| echo, persistent warm-call | medium | 0.2 | 0.2 | 0.3 |

When an isolate is hot, pandas costs ~3x native on small frames, and is close
to native (+15%) on 1M rows, where the work happens in compiled code.

### Warm snapshot and pool warmup

| Path | Bundle | Avg ms per call / setup |
| --- | --- | --: |
| Restore warm snapshot + run (`aardvark-json-warm`) | echo | 391 |
| Restore warm snapshot + run (`aardvark-json-warm`) | pandas | 4 374 |
| Warmed host, pool of 1 (setup + first live call) | pandas | 4 254 |
| Warmed host, pool of 2 | pandas | 7 925 |
| Warmed host, pool of 4 | pandas | 15 019 |
| Warmed host, pool of 4 | echo | 1 212 |

The warm snapshot today does not include pandas: restore is fast, but each
isolate still installs and imports numpy/pandas on top of it (~4 s).
Pool warmup runs **serially**, about 3.7–4.2 s per pandas isolate.

### Snapshot with pandas pre-imported: currently broken

Every path that captures the warm snapshot after importing a native extension
fails on the first call after restore:

```
RuntimeError: table index is out of bounds
    at wasm://wasm/...:wasm-function[...]
    at callPyObjectKwargs (pyodide.asm.patched.js)
```

- Reproduces with `--warm-preimport numpy` alone.
- A pure-stdlib preimport (`--warm-preimport json`) works.
- Affected modes: `aardvark-json-warm`, `-persistent-warm-call-no-stdio`,
  `-registry-retained-handler-no-stdio` and `-warmed-host-pooled-...`, at
  pool sizes 1, 2 and 4.

The memory image restores, but the WebAssembly function-table entries that the
dynamically loaded `.so` side modules registered do not. So the first call into
numpy C code jumps to a missing table slot. Neither the snapshot code nor
`docs/architecture/packages-and-snapshots.md` covers this case. Without a fix
we cannot measure how much a pandas-preimported snapshot would help. The echo
numbers suggest it could bring per-isolate setup from ~4.2 s down toward
~0.4–1 s.

### Memory per isolate (warmed host pool, medium profile)

| Pool size | pandas end RSS MiB | pandas peak RSS MiB | echo end RSS MiB | echo peak RSS MiB |
| --: | --: | --: | --: | --: |
| 1 | 697 | 998 | 367 | 430 |
| 2 | 778 | 1 210 | 444 | 575 |
| 4 | 1 123 | 1 732 | 556 | 744 |

- Marginal cost of a **pandas isolate: ~140 MiB steady, ~245 MiB at peak**
  ((1 732 − 998) / 3). For echo it is ~65 / ~105 MiB.
- Fixed cost of a worker process with one pandas isolate is ~0.7–1.0 GiB. This
  includes the runtime used to capture the warm state.

## Capacity estimate

Sizing rule for a node, per worker process:

```
RAM ≈ 1.0 GiB + 0.25 GiB × isolates      (peak-based, pandas loaded)
```

Each isolate runs one call at a time on one thread, so throughput comes from the
isolate count, and more isolates than cores adds no throughput. Examples:

| Node | Isolates (= cores) | Est. peak RAM | Serial warmup today |
| --- | --: | --: | --: |
| 4 vCPU / 8 GiB | 4 | ~2 GiB | ~15 s |
| 8 vCPU / 16 GiB | 8 | ~3 GiB | ~30 s |
| 16 vCPU / 32 GiB | 16 | ~5 GiB | ~60 s |

So memory is not the constraint: one worker per node with one isolate per core
fits comfortably. The two real costs are:

1. **Cold path ~6.2 s.** No request should ever see it. Pools must be pre-warmed
   at deploy time, and a crashed or recycled isolate costs ~4 s to replace.
2. **Serial warmup.** It takes ~4 s × isolates before a fresh worker can take its
   full load. Fixing native-extension snapshots, or warming isolates in
   parallel, is the lever here.

Steady-state latency (8.5 ms for 10k rows, 54 ms for 1M rows) is fine for UDF
calls.

## Follow-ups

- Restore dynlib function-table state on warm-snapshot restore, so snapshots
  can pre-import numpy/pandas. Then re-measure the pre-import rows above.
- Warm pool isolates in parallel, or at least measure whether it helps.
- Re-run these measurements on the target production instance type.
