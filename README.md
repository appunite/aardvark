# Aardvark Runtime

![Aardvark in the Bushveld, Limpopo](https://upload.wikimedia.org/wikipedia/commons/thumb/f/f0/Orycteropus_afer_175359469.jpg/1039px-Orycteropus_afer_175359469.jpg "By Kelly Abram - https://www.inaturalist.org/photos/175359469, CC BY 4.0, https://commons.wikimedia.org/w/index.php?curid=134253363")

> The aardvark (/ˈɑːrdvɑːrk/ ARD-vark; Orycteropus afer) is a medium-sized, burrowing, nocturnal mammal native to Africa. The aardvark is the only living member of the genus Orycteropus, the family Orycteropodidae and the order Tubulidentata. It has a long proboscis, similar to a pig's snout, which is used to sniff out food.
>
> -- [Wikipedia](https://en.wikipedia.org/wiki/Aardvark)

Embedded multi-language runtime for executing sandboxed bundles inside [V8](https://v8.dev/), with hardened resource controls and structured diagnostics. The project takes clear inspiration from Cloudflare Python Workers while pursuing an embeddable library-first design for Rust hosts and host-owned shared-library adapters. **Aardvark is experimental software**: APIs, manifests, and runtime semantics may change without notice, and the system has not been hardened for production traffic yet.

## Why Aardvark?

- **Persistent isolates** – Keep Python warm between calls, reuse shared buffers, and avoid remounting bundles unless the code changes.
- **Snapshot-friendly runtimes** – Reuse warm isolates across requests, carry overlay metadata with snapshots, and keep cold starts predictable.
- **Deterministic sandboxing** – Enforce per-invocation budgets for wall time, CPU, heap, filesystem writes, and outbound network hosts.
- **Self-describing bundles** – Ship code, manifest, and dependency hints together as a ZIP; hosts can honour or override the manifest contract at runtime.
- **First-class telemetry** – Every invocation emits structured diagnostics (stdout/stderr, exceptions, resource usage, policy violations, reset timings) that hosts can feed into their own observability stack.
- **Runtime pooling** – Amortise startup cost by recycling isolates with predictable reset semantics.
- **Dual-language engine (preview)** – Run JavaScript bundles alongside Python handlers using the same network/filesystem sandboxing. JavaScript support is currently read-only and expects bring-your-own modules.

## Quick Start (CLI)

The CLI is intended for local smoke tests and debugging; production setups should embed the library directly.

```
cargo run -p aardvark-cli -- assets stage --variant full
PYODIDE_DIST_DIR="$(find .aardvark/pyodide-distributions -maxdepth 1 -type d -name 'aardvark-*-pyodide-v0.29.4-full' | sort | tail -n 1)"
test -n "$PYODIDE_DIST_DIR"
AARDVARK_PYODIDE_DIST_DIR="$PYODIDE_DIST_DIR" \
  cargo run -p aardvark-cli -- \
  --bundle example/numpy_bundle.zip
```

Each example bundle includes `aardvark.manifest.json`, so the CLI can read the
entrypoint and package list from the ZIP. To smoke a larger package set, point
the runtime at the same staged distribution and run another manifest-backed
bundle:

```
AARDVARK_PYODIDE_DIST_DIR="$PYODIDE_DIST_DIR" \
  cargo run -p aardvark-cli -- \
  --bundle example/pandas_numpy_bundle.zip
```

Use `--entrypoint` or `--package` only as local debugging overrides for
manifest-less bundles or one-off experiments. Production bundles should carry
their entrypoint and package requirements in the manifest.

### Preparing [Pyodide](https://pyodide.org/) assets

The runtime never downloads wheels on demand. Package loading uses a staged
Aardvark Pyodide distribution, which combines the upstream Pyodide 0.29.4
release with Aardvark adapter scripts and a manifest containing file hashes plus
a compatibility fingerprint.

Use the CLI helper:

```
cargo run -p aardvark-cli -- assets stage --variant full
PYODIDE_DIST_DIR="$(find .aardvark/pyodide-distributions -maxdepth 1 -type d -name 'aardvark-*-pyodide-v0.29.4-full' | sort | tail -n 1)"
test -n "$PYODIDE_DIST_DIR"
cargo run -p aardvark-cli -- assets verify "$PYODIDE_DIST_DIR"
```

Then set `AARDVARK_PYODIDE_DIST_DIR` or configure
`PyRuntimeConfig::with_pyodide_dist_dir(...)` / `set_pyodide_dist_dir(...)`.
The `core` variant is only suitable for scenarios that do not install additional
Pyodide packages.

### Building CLI release binaries

Use the workspace task runner to produce release artefacts for the CLI:

    cargo install cross
    cargo run -p xtask -- release-cli

The task ensures the required Rust targets are installed (`rustup target add …`)
before building. Binaries land in `./dist/` (default targets:
`x86_64-apple-darwin` and `x86_64-unknown-linux-gnu`).

Useful flags:

- `--targets <triple[,triple]>` – override the target list.
- `--out-dir <path>` – choose a different output directory.

Prefer to run the underlying cross-compiles yourself?

    cross build -p aardvark-cli --release --target x86_64-unknown-linux-gnu

## Embedding in Rust

### Quick handler execution with `PythonIsolate`

```rust
use aardvark_core::{
    persistent::{BundleArtifact, BundleHandle, HandlerSession, PythonIsolate},
    ExecutionOutcome, IsolateConfig, PyRuntimeConfig,
};

fn execute(
    bundle_bytes: &[u8],
    pyodide_dist_dir: impl Into<std::path::PathBuf>,
) -> anyhow::Result<ExecutionOutcome> {
    // Parse once; cloning `BundleArtifact` is cheap because entries are shared internally.
    let artifact = BundleArtifact::from_bytes(bundle_bytes)?;

    let mut isolate = PythonIsolate::new(IsolateConfig {
        runtime: PyRuntimeConfig::default().with_pyodide_dist_dir(pyodide_dist_dir),
        ..IsolateConfig::default()
    })?;
    // Optionally pre-load the bundle so imports and package setup happen before the first call.
    let handle = BundleHandle::from_artifact(artifact.clone());
    isolate.load_bundle(&handle)?;

    // Prepare a handler session (reuse it across invocations).
    let handler: HandlerSession = handle.prepare_default_handler();
    Ok(handler.invoke(&mut isolate)?)
}
```

`HandlerSession` exposes `invoke`, `invoke_json`, `invoke_rawctx`, and `invoke_async` adapters. `PythonIsolate` also provides `run_inline_python`/`run_inline_python_with_options` for one-off scripts and exposes the underlying `PyRuntime` when you need low-level control.

### Pooling with `BundlePool`

```rust
use std::sync::Arc;

use aardvark_core::persistent::{
    BundleArtifact, BundlePool, LifecycleHooks, PoolOptions, QueueMode,
};
use aardvark_core::{IsolateConfig, PyRuntimeConfig};

fn pool_example(
    bundle_bytes: &[u8],
    pyodide_dist_dir: impl Into<std::path::PathBuf>,
) -> anyhow::Result<()> {
    let artifact = BundleArtifact::from_bytes(bundle_bytes)?;
    let pool = BundlePool::from_artifact(
        artifact.clone(),
        PoolOptions {
            isolate: IsolateConfig {
                runtime: PyRuntimeConfig::default().with_pyodide_dist_dir(pyodide_dist_dir),
                ..IsolateConfig::default()
            },
            desired_size: 2,
            max_size: 4,
            queue_mode: QueueMode::Block,
            heap_limit_kib: Some(256 * 1024),
            memory_limit_kib: Some(512 * 1024),
            lifecycle_hooks: Some(LifecycleHooks {
                on_isolate_started: Some(Arc::new(|id, _cfg| tracing::debug!(isolate_id = id, "started"))),
                on_isolate_recycled: Some(Arc::new(|id, reason| {
                    tracing::debug!(isolate_id = id, ?reason, "recycled")
                })),
                ..Default::default()
            }),
            ..PoolOptions::default()
        },
    )?;

    let handler = pool.prepare_default_handler()?;

    let outcome = pool.call_default(&handler)?;
    tracing::info!(
        queue_wait_ms = outcome.diagnostics.queue_wait_ms,
        heap_kib = outcome.diagnostics.py_heap_kib,
    );

    let stats = pool.stats();
    tracing::info!(
        invocations = stats.invocations,
        avg_wait_ms = stats.average_queue_wait_ms,
        p95_wait_ms = stats.queue_wait_p95_ms,
        quarantine_events = stats.quarantine_events,
        scaledown_events = stats.scaledown_events,
    );
    Ok(())
}
```

The pool now manages multiple isolates, exposes queue behaviour (`QueueMode` + `max_queue`), and enforces per-isolate heap/RSS guard rails. Lifecycle hooks let hosts observe isolate churn and per-call outcomes without instrumenting the runtime directly.

Need to change concurrency after startup? Call `pool.set_desired_size(new_size)` to pre-warm or shed isolates, or `pool.resize(max_size)` to adjust the upper bound.

### Still need the raw runtime?

`PyRuntime` remains available for hosts that prefer to manage bundles and resets manually. When a bundle has a manifest, construct the runtime from the bundle so profile requirements such as `runtime.pyodide.profile` are applied before the Pyodide isolate starts:

```rust
use aardvark_core::{Bundle, PyRuntime, PyRuntimeConfig};

fn execute_with_runtime(bytes: &[u8]) -> anyhow::Result<()> {
    let bundle = Bundle::from_zip_bytes(bytes)?;
    let mut runtime = PyRuntime::new_for_bundle(PyRuntimeConfig::default(), &bundle)?;
    let (session, _manifest) = runtime.prepare_session_with_manifest(bundle)?;
    let outcome = runtime.run_session(&session)?;
    println!("status: {:?}", outcome.status);
    Ok(())
}
```

### Warm Snapshots

Capture a fully-initialised [Pyodide](https://pyodide.org/) instance (including packages) and reuse it for future runtimes:

```rust
use aardvark_core::{Bundle, PyRuntime, PyRuntimeConfig};

fn build_warm_state(bytes: &[u8]) -> anyhow::Result<(PyRuntimeConfig, Bundle)> {
    let bundle = Bundle::from_zip_bytes(bytes)?;
    let mut config = PyRuntimeConfig::default();
    config.apply_bundle_manifest(bundle.manifest()?.as_ref())?;
    let mut runtime = PyRuntime::new(config.clone())?;
    let (_session, _manifest) = runtime.prepare_session_with_manifest(bundle.clone())?;
    // Optional: run warm-up imports or pre-populate globals here.
    let warm = runtime.capture_warm_state()?;

    config.warm_state = Some(warm);
    Ok((config, bundle))
}
```

Any new runtime (or pool) constructed with that `PyRuntimeConfig` skips the heavy [Pyodide](https://pyodide.org/) bootstrap and restores directly from the warm snapshot. Warm states are tied to the active Pyodide distribution fingerprint; a runtime will reject a warm state captured with a different distribution. Captured warm states carry overlay metadata and normally re-import that overlay on restore. Only use `WarmState::with_overlay_preloaded` or `WarmState::into_overlay_preloaded` when you have already baked the overlay into the snapshot image.

## Benchmarking the runtime

For a quick sanity check of `prepare` versus `run` timings, use the built-in bench example:

```
cargo run -p aardvark-core --example bench_echo -- 100 1024
```

Arguments are `[iterations] [payload_len]` (both optional). The harness warms the runtime, captures a warm snapshot, and prints avg/min/max milliseconds for `prepare`, `run`, and `total`. Use it to correlate host-side measurements with the core runtime.

`docs/api/rust-host.md` expands on persistent isolates, pooling semantics, invocation strategies, and telemetry export. For JavaScript bundles, pass `language = "javascript"` in the manifest or descriptor and ship a self-contained bundle produced by your JS build tool.

For non-Rust hosts, load a Rust `cdylib` that links `aardvark-core` and owns the
host-specific ABI. Start with `docs/api/shared-library-host.md`; the Linux
`rusty_v8` archive rebuild procedure in `docs/dev/linux-v8-shared-archive.md`
is maintainer documentation, not the integration entry point.

## Documentation

- Architecture guidance lives under `docs/architecture/`. Start with `overview.md` for a top-down explanation, then branch into resource-limits, lifecycle/sandbox internals, and telemetry. The current feature plan is in `roadmap.md`.
- API reference under `docs/api/` covers the manifest schema, Rust host integration, shared-library host integration, handler contracts, and diagnostics handling with examples.
- Developer onboarding material is available in `docs/dev/` for contributors extending the project.
- Performance notes and benchmark workflow live in `docs/perf/overview.md`.
- The included `Makefile` has helpers (`make perf-all`, `make perf-md`). It
  honours `PYODIDE_DIST_DIR` when wiring up the perf harness.

## Publishing Notes

The core library is published as `aardvark-core`. Before cutting any experimental build:

- Audit the bundled [Pyodide](https://pyodide.org/) version and rebuild snapshots if needed.
- Decide whether to ship the CLI (`aardvark-cli`) alongside or keep it workspace-only.
- Ensure `AARDVARK_PYODIDE_DIST_DIR` points at a verified distribution available on the target system; the crate never downloads wheels at runtime.
- Regenerate the bundle manifest schema if new fields were added.

## Limitations and Open Work

- Windows builds are not supported or tested.
- Shared buffer handles expose zero-copy slices; request an owned copy only if you need to mutate or persist the data.
- JavaScript support expects pre-bundled modules and does not resolve npm packages or access the filesystem for node_modules.
- Network sandboxing is allowlist-based per session; there is no per-request override yet.
- Filesystem quota enforcement only covers the `/session` tree.
- Streaming outputs and incremental logs are not available; handlers must return a single payload.
- Warm snapshots are tied to the [Pyodide](https://pyodide.org/) distribution fingerprint used when you captured them; changing the distribution requires baking a new snapshot. When you assemble warm states manually, pass the matching fingerprint and only flag them as `overlay_preloaded` when the overlay was baked into the snapshot.
- Runtime pool resets still execute synchronously on the thread that next checks out a runtime; there is no background reset worker yet.
- API stability is not guaranteed; expect breaking changes while the runtime matures.

## License

See `LICENSE` for details.
