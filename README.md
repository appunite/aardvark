# Aardvark Python Runner

![Aardvark in the Bushveld, Limpopo](https://upload.wikimedia.org/wikipedia/commons/thumb/f/f0/Orycteropus_afer_175359469.jpg/1039px-Orycteropus_afer_175359469.jpg "By Kelly Abram - https://www.inaturalist.org/photos/175359469, CC BY 4.0, https://commons.wikimedia.org/w/index.php?curid=134253363")

> The aardvark (/ˈɑːrdvɑːrk/ ARD-vark; Orycteropus afer) is a medium-sized, burrowing, nocturnal mammal native to Africa. The aardvark is the only living member of the genus Orycteropus, the family Orycteropodidae and the order Tubulidentata. It has a long proboscis, similar to a pig's snout, which is used to sniff out food.
>
> -- [Wikipedia](https://en.wikipedia.org/wiki/Aardvark)

Embedded Pyodide runtime for executing Python bundles inside V8, with hardened resource controls and structured diagnostics. Inspired by Cloudflare Python Workers, implemented in Rust for host applications that need tight integration and predictable performance.

## Why Aardvark?

- **Snapshot-friendly Pyodide** – Reuse warm isolates across requests, preload packages, and capture snapshots to keep cold-starts in check.
- **Deterministic sandboxing** – Enforce per-invocation budgets for wall time, CPU, heap, filesystem writes, and outbound network hosts.
- **Self-describing bundles** – Ship code, manifest, and dependency hints together as a ZIP; hosts can honour or override the manifest contract at runtime.
- **First-class telemetry** – Every invocation emits structured diagnostics (stdout/stderr, exceptions, resource usage, policy violations) that hosts can feed into their own observability stack.
- **Runtime pooling** – Amortise Pyodide startup cost by recycling isolates with predictable reset semantics.

## Quick Start (CLI)

The CLI is intended for local smoke tests and debugging; production setups should embed the library directly.

```
cargo run -p aardvark-cli -- \
  --bundle hello_bundle.zip \
  --entrypoint main:main
```

To preload packages, point the runtime at an unpacked Pyodide cache:

```
AARDVARK_PYODIDE_PACKAGE_DIR=tmp/pyodide \
  cargo run -p aardvark-cli -- \
  --bundle example/pandas_numpy_bundle.zip \
  --manifest
```

The manifest bundled with the example instructs the runtime to install `numpy` and `pandas` before executing the handler.

## Embedding in Rust

```rust
use aardvark_core::{Bundle, PyRuntime, PyRuntimeConfig};

fn execute(bytes: &[u8]) -> anyhow::Result<()> {
    let mut runtime = PyRuntime::new(PyRuntimeConfig::default())?;
    let (session, _manifest) = runtime.prepare_session_with_manifest(Bundle::from_zip_bytes(bytes)?)?;
    let outcome = runtime.run_session(&session)?;
    println!("status: {:?}", outcome.status);
    println!("stdout: {}", outcome.diagnostics.stdout);
    Ok(())
}
```

`docs/api/rust-host.md` expands on pooling, custom descriptors, and telemetry export.

## Documentation

- Architecture: `docs/architecture/overview.md` (start here), plus deeper dives into lifecycle, sandboxing, packages, and telemetry.
- API reference: `docs/api/` covers the manifest schema, host integration, Python handler expectations, and diagnostics handling with examples.

## Publishing Notes

The crate exposes the library as `aardvark-core`. When publishing to crates.io:

- Audit the bundled Pyodide version and rebuild snapshots if needed.
- Decide whether to ship the CLI (`aardvark-cli`) alongside or keep it as a workspace-only helper.
- Ensure `AARDVARK_PYODIDE_PACKAGE_DIR` points at a cache available on the target system; the crate does not download wheels at runtime.

## Limitations and Open Work

- Windows builds are not supported or tested.
- Shared buffer handles currently copy data; zero-copy transports are planned but not implemented.
- Network sandboxing is allowlist-based per session; there is no per-request override yet.
- Filesystem quota enforcement only covers the `/session` tree.
- Streaming outputs and incremental logs are not available; handlers must return a single payload.

## License

See `LICENSE` for details.
