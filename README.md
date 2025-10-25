# Aardvark Runtime

![Aardvark in the Bushveld, Limpopo](https://upload.wikimedia.org/wikipedia/commons/thumb/f/f0/Orycteropus_afer_175359469.jpg/1039px-Orycteropus_afer_175359469.jpg "By Kelly Abram - https://www.inaturalist.org/photos/175359469, CC BY 4.0, https://commons.wikimedia.org/w/index.php?curid=134253363")

> The aardvark (/ˈɑːrdvɑːrk/ ARD-vark; Orycteropus afer) is a medium-sized, burrowing, nocturnal mammal native to Africa. The aardvark is the only living member of the genus Orycteropus, the family Orycteropodidae and the order Tubulidentata. It has a long proboscis, similar to a pig's snout, which is used to sniff out food.
>
> -- [Wikipedia](https://en.wikipedia.org/wiki/Aardvark)

Embedded multi-language runtime for executing sandboxed bundles inside [V8](https://v8.dev/), with hardened resource controls and structured diagnostics. The project takes clear inspiration from Cloudflare Python Workers while pursuing an embeddable library-first design for Rust hosts. **Aardvark is experimental software**: APIs, manifests, and runtime semantics may change without notice, and the system has not been hardened for production traffic yet.

> [!IMPORTANT]
> Aardvark bundles prebuilt PIC-enabled V8 142.0.0 archives that we compiled from the upstream tag with `v8_monolithic=true` and `v8_monolithic_for_shared_library=true`. The workspace’s `.cargo/config.toml` points `RUSTY_V8_MIRROR` at our GitHub release so `cargo build` uses those artifacts by default. If you package your own cdylib (for example, an Elixir NIF) you can keep this mirror, or override `RUSTY_V8_MIRROR` / `RUSTY_V8_ARCHIVE` to supply a different build. The mirror is still experimental—expect churn and let us know if you hit linker surprises.

## Why Aardvark?

- **Persistent isolates** – Keep Python warm between calls, reuse shared buffers, and avoid remounting bundles unless the code changes.
- **Snapshot-friendly runtimes** – Reuse warm isolates across requests, bake overlays into warm snapshots, and keep cold starts predictable.
- **Deterministic sandboxing** – Enforce per-invocation budgets for wall time, CPU, heap, filesystem writes, and outbound network hosts.
- **Self-describing bundles** – Ship code, manifest, and dependency hints together as a ZIP; hosts can honour or override the manifest contract at runtime.
- **First-class telemetry** – Every invocation emits structured diagnostics (stdout/stderr, exceptions, resource usage, policy violations, reset timings) that hosts can feed into their own observability stack.
- **Runtime pooling** – Amortise startup cost by recycling isolates with predictable reset semantics.
- **Dual-language engine (preview)** – Run JavaScript bundles alongside Python handlers using the same network/filesystem sandboxing. JavaScript support is read-only for now and expects bring-your-own modules.

## Quick Start (CLI)

The CLI is intended for local smoke tests and debugging; production setups should embed the library directly.

```
cargo run -p aardvark-cli -- \
  --bundle hello_bundle.zip \
  --entrypoint main:main
```

To preload packages, point the runtime at an unpacked [Pyodide](https://pyodide.org/) cache:

```
AARDVARK_PYODIDE_PACKAGE_DIR=.aardvark/pyodide/0.29.0 \
  cargo run -p aardvark-cli -- \
  --bundle example/pandas_numpy_bundle.zip \
  --manifest
```

The manifest bundled with the example instructs the runtime to install `numpy` and `pandas` before executing the handler.

### Preparing [Pyodide](https://pyodide.org/) assets

The runtime expects a local [Pyodide](https://pyodide.org/) cache and never downloads wheels on demand.
Stage the upstream release yourself and flatten it into
`./.aardvark/pyodide/<version>` so every asset sits directly under that
directory:

```
mkdir -p .aardvark/pyodide/0.29.0
curl -L -o pyodide-0.29.0.tar.bz2 \
  https://github.com/pyodide/pyodide/releases/download/0.29.0/pyodide-0.29.0.tar.bz2
echo "85395f34a808cc8852f3c4a5f5d47f906a8a52fa05e5cd70da33be82f4d86a58  pyodide-0.29.0.tar.bz2" | sha256sum --check
tar -xjf pyodide-0.29.0.tar.bz2
rsync -a pyodide/pyodide/v0.29.0/full/ .aardvark/pyodide/0.29.0/
rm -rf pyodide pyodide-0.29.0.tar.bz2
```

Swap the archive name for `pyodide-core-0.29.0.tar.bz2` if you only need the
core subset. Once the files are in place, set
`AARDVARK_PYODIDE_PACKAGE_DIR=.aardvark/pyodide/0.29.0` (or configure
`PyRuntimeConfig::pyodide_version`). When [Pyodide](https://pyodide.org/) requests
`pyodide/v0.29.0/full/numpy-*.whl`, the runtime will serve
`.aardvark/pyodide/0.29.0/numpy-*.whl` straight from disk.

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
    IsolateConfig,
};

fn execute(bundle_bytes: &[u8]) -> anyhow::Result<String> {
    // Parse once; cloning `BundleArtifact` is cheap because entries are shared internally.
    let artifact = BundleArtifact::from_bytes(bundle_bytes)?;

    let mut isolate = PythonIsolate::new(IsolateConfig::default())?;
    // Optionally pre-load the bundle so packages are cached before the first call.
    let handle = BundleHandle::from_artifact(artifact.clone());
    isolate.load_bundle(&handle)?;

    // Prepare a handler session (reuse it across invocations).
    let handler: HandlerSession = handle.prepare_default_handler();
    let outcome = handler.invoke(&mut isolate)?;
    Ok(outcome
        .payload()
        .and_then(|payload| match payload {
            aardvark_core::ResultPayload::Text(text) => Some(text.clone()),
            _ => None,
        })
        .unwrap_or_default())
}
```

`HandlerSession` exposes `invoke`, `invoke_json`, `invoke_rawctx`, and `invoke_async` adapters. `PythonIsolate` also provides `run_inline_python`/`run_inline_python_with_options` for one-off scripts and exposes the underlying `PyRuntime` when you need low-level control.

### Pooling with `BundlePool`

```rust
use std::sync::Arc;

use aardvark_core::persistent::{
    BundleArtifact, BundlePool, LifecycleHooks, PoolOptions, QueueMode,
};

fn pool_example(bundle_bytes: &[u8]) -> anyhow::Result<()> {
    let artifact = BundleArtifact::from_bytes(bundle_bytes)?;
    let pool = BundlePool::from_artifact(
        artifact.clone(),
        PoolOptions {
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

    let handle = pool.handle();
    let handler = handle.prepare_default_handler();

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

`PyRuntime` remains available for hosts that prefer to manage bundles and resets manually. The legacy example is below for completeness:

```rust
use aardvark_core::{Bundle, PyRuntime, PyRuntimeConfig};

fn execute_with_runtime(bytes: &[u8]) -> anyhow::Result<()> {
    let mut runtime = PyRuntime::new(PyRuntimeConfig::default())?;
    let bundle = Bundle::from_zip_bytes(bytes)?;
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
    let mut runtime = PyRuntime::new(PyRuntimeConfig::default())?;
    let bundle = Bundle::from_zip_bytes(bytes)?;
    let (_session, _manifest) = runtime.prepare_session_with_manifest(bundle.clone())?;
    // Optional: run warm-up imports or pre-populate globals here.
    let warm = runtime.capture_warm_state()?;

    let mut config = PyRuntimeConfig::default();
    config.warm_state = Some(warm);
    Ok((config, bundle))
}
```

Any new runtime (or pool) constructed with that `PyRuntimeConfig` skips the heavy [Pyodide](https://pyodide.org/) bootstrap and restores directly from the warm snapshot. Snapshots captured inside the runtime automatically mark their overlays as preloaded so in-place resets avoid re-importing site-packages.

## Benchmarking the runtime

For a quick sanity check of `prepare` versus `run` timings, use the built-in bench example:

```
cargo run -p aardvark-core --example bench_echo -- 100 1024
```

Arguments are `[iterations] [payload_len]` (both optional). The harness warms the runtime, captures a warm snapshot, and prints avg/min/max milliseconds for `prepare`, `run`, and `total`. Use it to correlate host-side measurements with the core runtime.

`docs/api/rust-host.md` expands on persistent isolates, pooling semantics, invocation strategies, and telemetry export. For JavaScript bundles, pass `language = "javascript"` in the manifest or descriptor and ship a self-contained bundle produced by your JS build tool.

## Documentation

- Architecture guidance lives under `docs/architecture/`. Start with `overview.md` for a top-down explanation, then branch into resource-limits, lifecycle/sandbox internals, and telemetry. The current feature plan is in `roadmap.md`.
- API reference under `docs/api/` covers the manifest schema, host integration, handler contracts, and diagnostics handling with examples.
- Developer onboarding material is available in `docs/dev/` for contributors extending the project.
- Performance notes and benchmark workflow live in `docs/perf/overview.md`.
- The included `Makefile` has helpers (`make perf-all`, `make perf-md`). It
  honours `PYODIDE_DIR` (default `./.aardvark/pyodide/0.29.0`) when wiring up
  the perf harness.

## Publishing Notes

The core library is published as `aardvark-core`. Before cutting any experimental build:

- Audit the bundled [Pyodide](https://pyodide.org/) version and rebuild snapshots if needed.
- Decide whether to ship the CLI (`aardvark-cli`) alongside or keep it workspace-only.
- Ensure `AARDVARK_PYODIDE_PACKAGE_DIR` points at a cache available on the target system; the crate never downloads wheels at runtime.
- Regenerate the bundle manifest schema if new fields were added.

## Limitations and Open Work

- Windows builds are not supported or tested.
- Shared buffer handles expose zero-copy slices; request an owned copy only if you need to mutate or persist the data.
- JavaScript support expects pre-bundled modules and does not resolve npm packages or access the filesystem for node_modules.
- Network sandboxing is allowlist-based per session; there is no per-request override yet.
- Filesystem quota enforcement only covers the `/session` tree.
- Streaming outputs and incremental logs are not available; handlers must return a single payload.
- Warm snapshots are tied to the [Pyodide](https://pyodide.org/) build and manifest used when you captured them; changing either requires baking a new snapshot. When you assemble warm states manually, remember to flag them as `overlay_preloaded` so resets avoid redundant imports.
- Runtime pool resets still execute synchronously on the thread that next checks out a runtime; there is no background reset worker yet.
- API stability is not guaranteed; expect breaking changes while the runtime matures.

## License

See `LICENSE` for details.
