# Host Integration (Rust)

This guide shows how to embed `aardvark-core` in a Rust service. It covers runtime setup, bundle execution, pooling, and error handling. Everything here is **experimental** and likely to change; use it for prototypes rather than production traffic.

## Adding the dependency

```toml
[dependencies]
aardvark-core = { path = "crates/aardvark-core" }
```

For crates.io you will depend on the published version instead of the workspace path.

## Preparing Pyodide assets

Before initialising the runtime you need the pinned Pyodide bundle on disk. The recommended path is

```
cargo aardvark fetch-pyodide --version 0.28.2 --variant core
```

which creates `./.aardvark/pyodide/0.28.2/core`. Export `AARDVARK_PYODIDE_PACKAGE_DIR` (or configure `PyRuntimeConfig`) to that directory. Use `--variant full` or `--extra static-libraries,xbuildenv` when you need the larger bundles.

Need a shell-only alternative? Download manually:

```
curl -L -o pyodide-core-0.28.2.tar.bz2 \
  https://github.com/pyodide/pyodide/releases/download/0.28.2/pyodide-core-0.28.2.tar.bz2
echo "c9f6dd067d119e50850849f7428e3c636ecbc2684a0d2ff992f3bd48a1062b6c  pyodide-core-0.28.2.tar.bz2" | sha256sum --check
tar -xjf pyodide-core-0.28.2.tar.bz2
mkdir -p .aardvark/pyodide/0.28.2
mv pyodide .aardvark/pyodide/0.28.2/core
```

Point the environment variable at the resulting directory and mirror the URL/hash when you upgrade Pyodide.

## Creating a runtime

```rust
use aardvark_core::{PyRuntime, PyRuntimeConfig};

fn build_runtime() -> anyhow::Result<PyRuntime> {
    let mut config = PyRuntimeConfig::default();
    config.reset_policy = aardvark_core::config::ResetPolicy::AfterInvocation;
    config.snapshot.load_from = Some("/srv/snapshots/pandas.bin".into());
    let runtime = PyRuntime::new(config)?;
    Ok(runtime)
}
```

Key configuration knobs:

- `snapshot.load_from` – optional warm snapshot path.
- `snapshot.save_to` – capture a new snapshot after the first load.
- Snapshots are cached in memory after the first read; call `config.snapshot.clear_cache()` if you regenerate the file at runtime.
- `budget_override` – clamp descriptor limits globally (e.g., enforce platform-wide CPU ceilings).
- `host_capabilities` – capability allowlist applied to every session unless the manifest narrows it further.
- `default_language` – fallback guest language when descriptors/manifests omit one (defaults to `python`; set to `javascript` to prefer the preview engine).

## Preparing a session

```rust
use aardvark_core::{Bundle, PyRuntime};

fn load_bundle(bytes: &[u8]) -> anyhow::Result<Bundle> {
    // Parse once and keep the value around — cloning `Bundle` is cheap.
    Bundle::from_zip_bytes(bytes)
}

fn prepare(runtime: &mut PyRuntime, bundle: Bundle) -> anyhow::Result<aardvark_core::PySession> {
    let (session, manifest_opt) = runtime.prepare_session_with_manifest(bundle)?;
    if let Some(manifest) = manifest_opt {
        tracing::info!(packages = ?manifest.packages(), "manifest applied");
    }
    Ok(session)
}
```

If you need full control, create an `InvocationDescriptor` and call `prepare_session_with_descriptor` instead. The descriptor lets you pin the language per invocation via `descriptor.language = Some(RuntimeLanguage::JavaScript);`.

## Running the session

```rust
use aardvark_core::{ExecutionOutcome, FailureKind};

fn invoke(runtime: &mut PyRuntime, session: &aardvark_core::PySession) -> anyhow::Result<()> {
    let outcome = runtime.run_session(session)?;
    if outcome.is_success() {
        let payload = outcome.payload();
        println!("handler returned {:?}", payload);
    } else if let FailureKind::PythonException(exc) = &outcome.status {
        eprintln!("python raised: {:?}\nstdout:{}\nstderr:{}",
            exc, outcome.diagnostics.stdout, outcome.diagnostics.stderr);
    }

    let telemetry = outcome.sandbox_telemetry();
    tracing::info!(cpu_ms = ?telemetry.cpu_ms_used, "cpu usage");
    Ok(())
}
```

All payload types are supported: text, JSON, binary, and shared buffers. Use pattern matching to unwrap the one you expect.

## Using the runtime pool

```rust
use aardvark_core::{PoolConfig, PyRuntimePool, PyRuntimeConfig};

fn pool_example() -> anyhow::Result<()> {
    let pool = PyRuntimePool::new(PoolConfig {
        max_runtimes: 8,
        runtime_config: PyRuntimeConfig::default(),
    })?;
    let mut handle = pool.checkout()?;
    let bundle = Bundle::from_zip_bytes(include_bytes!("../../hello_bundle.zip"))?;
    let session = handle.runtime().prepare_session_with_manifest(bundle)?.0;
    let outcome = handle.runtime().run_session(&session)?;
    drop(handle); // returns runtime to the pool; reset happens lazily in the background queue
    assert!(outcome.is_success());
    Ok(())
}
```

Pool handles implement `Drop`; always let them go out of scope to return the runtime. If reset fails, the runtime is discarded and capacity decreases until a new runtime is created.

Returned runtimes are marked dirty and scrubbed the next time the pool needs additional capacity. That keeps the hand-off path fast while still ensuring every checkout observes a clean snapshot.

## Warm Snapshots for Faster Cold Starts

If you want Cloudflare-style deploy-time hydration, capture a warm snapshot once and reuse it:

```rust
use aardvark_core::{Bundle, PyRuntime, PyRuntimeConfig, WarmState};

fn bake_warm_state(bytes: &[u8]) -> anyhow::Result<(WarmState, Bundle)> {
    let mut runtime = PyRuntime::new(PyRuntimeConfig::default())?;
    let bundle = Bundle::from_zip_bytes(bytes)?;
    runtime.prepare_session_with_manifest(bundle.clone())?;
    // Optional: execute warm-up imports or other setup work here.
    let warm = runtime.capture_warm_state()?;
    Ok((warm, bundle))
}

fn host_with_warm_state(warm: WarmState) -> anyhow::Result<PyRuntime> {
    let mut config = PyRuntimeConfig::default();
    config.warm_state = Some(warm);
    PyRuntime::new(config)
}
```

The saved `WarmState` bundles a Pyodide memory snapshot with its overlay. Runtimes constructed with it skip package installation and restore the filesystem/DLLs immediately. Call `config.snapshot.clear_cache()` or set `config.warm_state = None` if you regenerate the warm state at runtime.

## Custom strategies

```rust
use aardvark_core::{DefaultInvocationStrategy, PyInvocationStrategy};

let mut strategy = DefaultInvocationStrategy::default();
let outcome = runtime.run_session_with_strategy(&session, &mut strategy)?;
```

Implement `PyInvocationStrategy` when you need bespoke argument decoding or multi-phase execution. Strategies receive an `InvocationContext` with access to the JS runtime for advanced orchestration.

## Error handling

- `PyRunnerError` covers infrastructure failures (bad bundles, JS init issues). Treat them as deployment problems.
- `ExecutionOutcome::failure` indicates the handler ran (or was attempted) but finished unsuccessfully; inspect `FailureKind` for the root cause.
- Always read `diagnostics.stderr` even on success; Python warnings are printed there.

## Diagnostics export

```rust
use aardvark_core::SandboxTelemetry;

fn record(outcome: &ExecutionOutcome) {
    let telemetry: SandboxTelemetry = outcome.sandbox_telemetry();
    metrics::histogram!("aardvark.cpu_ms", telemetry.cpu_ms_used.unwrap_or(0) as f64);
    if telemetry.has_policy_violations() {
        tracing::warn!(?telemetry, "policy violation");
    }
}
```

`SandboxTelemetry` implements `Clone` so you can send it to background workers without keeping the original outcome alive.

## Known gaps

- There is no async API; integrate with async runtimes by wrapping the blocking calls in thread pools.
- Shared buffers expose zero-copy views via `SharedBufferHandle::as_slice()`; call `into_bytes()` only if you need an owned copy.
- JavaScript bundles are “bring your own modules”: package resolution is not performed at runtime, so ship a single self-contained bundle produced by your JS bundler.
- Manifest-driven package caches must be prepared out of band. The core crate does not download wheels from the network.

## Stability & Release Readiness

- Neither runtime path is production hardened. Expect breaking changes to manifests, descriptors, and configuration while we iterate.
- The manifest schema is currently versioned as `1.0` but should be treated as provisional; schema bumps may happen without backwards compatibility.
- When we approach a stable release we will publish migration guides and follow semantic versioning.
