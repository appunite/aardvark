# Pandas UDF Performance (HAV-3465)

Measurements for running a pandas UDF through Aardvark on Pyodide 0.29.4, to
size workers and isolates per node for Python UDFs. Step 1 of the Python UDFs
plan.

## Setup

- Machine: Apple M4 Max (16 cores: 12 performance + 4 efficiency), 48 GiB RAM,
  macOS 26.7.1. Release build of `aardvark-perf` at `d474765`.
- Distribution: `aardvark-0.2.1-pyodide-v0.29.4-full` (pandas 2.3.3,
  numpy 2.2.5).
- Workload: `perf/fixtures/scenarios/pandas.py`. It builds a DataFrame and does
  a `groupby().agg(mean)` over 128 groups. `medium` = 10 000 rows,
  `high` = 1 000 000 rows. JSON invocation.
- Each row is a separate `aardvark-perf scenario` process, 10 iterations.
  "End RSS" is the Mach `resident_size` of the process after the run. "Peak
  RSS" is `/usr/bin/time -l` maximum resident set size for the whole process.
- The echo bundle was re-measured on the same machine as the baseline.
  `report.md` holds the full benchmark matrix from the same machine.

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
| echo | none | 818 | 853 | 276 |
| pandas | none | 2 361 | 2 436 | 596 |
| pandas | medium | 2 361 | 2 420 | 598 |
| pandas | high | 2 420 | 2 500 | 633 |

Installing and importing numpy + pandas adds about **1.5 s** on top of the
~0.8 s Pyodide boot. The workload itself barely affects cold time.

### Warm calls (isolate already prepared)

| Path | Profile | Avg ms | P50 ms | P95 ms |
| --- | --- | --: | --: | --: |
| Aardvark persistent warm-call (`aardvark-json-persistent-warm-call-no-stdio`) | medium | 2.5 | 2.5 | 2.8 |
| Aardvark persistent warm-call | high | 18.3 | 18.2 | 18.9 |
| Aardvark registry retained handler (`aardvark-json-registry-retained-handler-no-stdio`) | medium | 3.9 | 2.4 | 10.6 |
| Native CPython 3.13.2, warm handler | medium | 1.2 | 0.9 | 2.5 |
| Native CPython 3.13.2, warm handler | high | 7.5 | 7.2 | 9.0 |
| Native CPython 3.13.2, fresh process per call | medium | 155 | 155 | 157 |
| echo, persistent warm-call | medium | 0.1 | 0.1 | 0.2 |

When an isolate is hot, pandas costs ~2x native on small frames and ~2.4x on
1M rows.

### Warm snapshot and pool warmup

| Path | Bundle | Avg ms per call / setup |
| --- | --- | --: |
| Restore warm snapshot + run (`aardvark-json-warm`) | echo | 163 |
| Restore warm snapshot + run (`aardvark-json-warm`) | pandas | 1 733 |
| Warmed host, pool of 1 (setup + first live call) | pandas | 1 698 |
| Warmed host, pool of 2 | pandas | 3 125 |
| Warmed host, pool of 4 | pandas | 5 826 |
| Warmed host, pool of 4 | echo | 484 |

The warm snapshot today does not include pandas: restore is fast (~0.18 s), but
each isolate still installs and imports numpy/pandas on top of it (~1.5 s).
Pool warmup runs **serially**, about 1.4–1.7 s per pandas isolate.

### Snapshot with pandas pre-imported: currently broken

Every path that captures the warm snapshot after importing a native extension
fails on the first call after restore:

```
RuntimeError: table index is out of bounds
    at wasm://wasm/...:wasm-function[...]
    at callPyObjectKwargs (pyodide.asm.patched.js)
```

- Reproduces with `--warm-preimport numpy` alone in `aardvark-json-warm`. In
  the registry and pooled modes, numpy alone fails earlier, while the warm
  state is captured (`python snippet execution failed`). numpy + pandas
  reaches the table-index error.
- A pure-stdlib preimport (`--warm-preimport json`) works.
- Affected modes: `aardvark-json-warm`, `-persistent-warm-call-no-stdio`,
  `-registry-retained-handler-no-stdio` and `-warmed-host-pooled-...`, at
  pool sizes 1, 2 and 4.

The memory image restores, but the WebAssembly function-table entries that the
dynamically loaded `.so` side modules registered do not. So the first call into
numpy C code jumps to a missing table slot. Neither the snapshot code nor
`docs/architecture/packages-and-snapshots.md` covers this case. Without a fix
we cannot measure how much a pandas-preimported snapshot would help. The echo
numbers suggest it could bring per-isolate setup from ~1.4 s down toward
~0.1–0.2 s.

### Memory per isolate (warmed host pool, medium profile)

| Pool size | pandas end RSS MiB | pandas peak RSS MiB | echo end RSS MiB | echo peak RSS MiB |
| --: | --: | --: | --: | --: |
| 1 | 637 | 744 | 299 | 337 |
| 2 | 836 | 1 023 | 382 | 447 |
| 4 | 1 201 | 1 541 | 569 | 693 |

- Marginal cost of a **pandas isolate: ~190 MiB steady, ~265 MiB at peak**
  ((1 541 − 744) / 3). For echo it is ~90 / ~120 MiB.
- Fixed cost of a worker process with one pandas isolate is ~0.6–0.75 GiB. This
  includes the runtime used to capture the warm state.

## Pyodide 314 re-measurement (HAV-3466)

Same machine, method and rows as above, after upgrading to Pyodide 314.0.7
(Python 3.14.2, Emscripten 5.0.3). Release build of `aardvark-perf` at
`173b717`, distribution `aardvark-0.3.0-pyodide-v314.0.7-full` (pandas 3.0.2,
numpy 2.4.6). Native rows use CPython 3.14.2 with the same pinned packages.

| Row | 0.29.4 | 314.0.7 | Change |
| --- | --: | --: | --: |
| Cold, echo (avg ms) | 818 | 913 | +12% |
| Cold, pandas `none` (avg ms) | 2 361 | 2 630 | +11% |
| Cold, pandas `high` (avg ms) | 2 420 | 2 671 | +10% |
| Cold, pandas peak RSS (MiB) | 596 | 715 | +20% |
| Persistent warm-call, `medium` (avg ms) | 2.5 | 3.1 | +24% |
| Persistent warm-call, `high` (avg ms) | 18.3 | 20.3 | +11% |
| Registry retained handler, `medium` (avg / p95 ms) | 3.9 / 10.6 | 5.2 / 14.1 | +33% |
| Native CPython warm, `medium` (avg ms) | 1.2 | 1.5 | noise |
| Native CPython warm, `high` (avg ms) | 7.5 | 8.5 | +13% |
| Native CPython fresh process, `medium` (avg ms) | 155 | 164 | +6% |
| echo persistent warm-call (avg ms) | 0.1 | 0.1 | = |
| Restore warm snapshot + run, echo (avg ms) | 163 | 131 | −20% |
| Restore warm snapshot + run, pandas (avg ms) | 1 733 | 1 802 | +4% |
| Warmed host pool of 1, pandas (ms) | 1 698 | 1 761 | +4% |
| Warmed host pool of 2, pandas (ms) | 3 125 | 3 299 | +6% |
| Warmed host pool of 4, pandas (ms) | 5 826 | 6 352 | +9% |
| Warmed host pool of 4, echo (ms) | 484 | 422 | −13% |
| Pool of 1 / 2 / 4, pandas peak RSS (MiB) | 744 / 1 023 / 1 541 | 871 / 1 144 / 1 624 | +80–130 |

Takeaways:

- 314 is about 10% slower to boot and import pandas, and warm pandas calls
  are 10–25% slower. On 1M rows native CPython 3.14 + pandas 3.0 slowed by a
  similar 13%, so that delta tracks the Python/pandas upgrade. On 10k rows
  Aardvark slowed more than native (+0.6 ms). Absolute warm latency (3 ms /
  20 ms) is still fine for UDFs.
- Snapshot restore itself got faster (echo 163 → 131 ms).
- The fixed cost per worker grew by ~120 MiB. The marginal pandas isolate
  is about the same: ~250 MiB at peak ((1 624 − 871) / 3), ~165 MiB steady.
  The capacity estimate below still holds with the fixed term at ~0.85 GiB.
- Pyodide 314 no longer sizes the heap from `INITIAL_MEMORY`, which broke
  restoring any warm snapshot larger than the default heap (every pandas
  snapshot row). Fixed in this upgrade by growing memory before the restore.
- Native-extension preimport into the warm snapshot still fails on 314 with
  the same `table index is out of bounds` error. The follow-up below still
  applies.

## Capacity estimate

Sizing rule for a node, per worker process:

```
RAM ≈ 0.75 GiB + 0.27 GiB × isolates      (peak-based, pandas loaded)
```

Each isolate runs one call at a time on one thread, so throughput comes from the
isolate count, and more isolates than cores adds no throughput. Examples:

| Node | Isolates (= cores) | Est. peak RAM | Serial warmup today |
| --- | --: | --: | --: |
| 4 vCPU / 8 GiB | 4 | ~2 GiB | ~6 s |
| 8 vCPU / 16 GiB | 8 | ~3 GiB | ~11 s |
| 16 vCPU / 32 GiB | 16 | ~5 GiB | ~22 s |

So memory is not the constraint: one worker per node with one isolate per core
fits comfortably. The two real costs are:

1. **Cold path ~2.4 s.** No request should ever see it. Pools must be pre-warmed
   at deploy time, and a crashed or recycled isolate costs ~1.4–1.7 s to
   replace.
2. **Serial warmup.** It takes ~1.4 s × isolates before a fresh worker can take
   its full load. Fixing native-extension snapshots, or warming isolates in
   parallel, is the lever here.

Steady-state latency (2.5 ms for 10k rows, 18 ms for 1M rows) is fine for UDF
calls.

## Follow-ups

- Restore dynlib function-table state on warm-snapshot restore, so snapshots
  can pre-import numpy/pandas. Then re-measure the pre-import rows above.
- Warm pool isolates in parallel, or at least measure whether it helps.
- Re-run these measurements on the target production instance type.
