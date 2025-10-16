# Aardvark Runtime

![Aardvark in the Bushveld, Limpopo](https://upload.wikimedia.org/wikipedia/commons/thumb/f/f0/Orycteropus_afer_175359469.jpg/1039px-Orycteropus_afer_175359469.jpg "By Kelly Abram - https://www.inaturalist.org/photos/175359469, CC BY 4.0, https://commons.wikimedia.org/w/index.php?curid=134253363")

> The aardvark (/ˈɑːrdvɑːrk/ ARD-vark; Orycteropus afer) is a medium-sized, burrowing, nocturnal mammal native to Africa. The aardvark is the only living member of the genus Orycteropus, the family Orycteropodidae and the order Tubulidentata. It has a long proboscis, similar to a pig's snout, which is used to sniff out food.
>
> -- [Wikipedia](https://en.wikipedia.org/wiki/Aardvark)

Embedded multi-language runtime for executing sandboxed bundles inside V8, with hardened resource controls and structured diagnostics. The project takes clear inspiration from Cloudflare Python Workers while pursuing an embeddable library-first design for Rust hosts. **Aardvark is experimental software**: APIs, manifests, and runtime semantics may change without notice, and the system has not been hardened for production traffic yet.

## Why Aardvark?

- **Snapshot-friendly runtimes** – Reuse warm isolates across requests, preload packages, and capture snapshots to keep cold-starts in check.
- **Deterministic sandboxing** – Enforce per-invocation budgets for wall time, CPU, heap, filesystem writes, and outbound network hosts.
- **Self-describing bundles** – Ship code, manifest, and dependency hints together as a ZIP; hosts can honour or override the manifest contract at runtime.
- **First-class telemetry** – Every invocation emits structured diagnostics (stdout/stderr, exceptions, resource usage, policy violations) that hosts can feed into their own observability stack.
- **Runtime pooling** – Amortise startup cost by recycling isolates with predictable reset semantics.
- **Dual-language engine (preview)** – Run JavaScript bundles alongside Python handlers using the same network/filesystem sandboxing. JavaScript support is read-only for now and expects bring-your-own modules.

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

`docs/api/rust-host.md` expands on pooling, invocation strategies, and telemetry export. For JavaScript bundles, pass `language = "javascript"` in the manifest or descriptor and ship a self-contained bundle produced by your JS build tool.

## Documentation

- Architecture guidance lives under `docs/architecture/`. Start with `overview.md` for a top-down explanation, then branch into resource-limits, sandbox internals, and telemetry.
- API reference under `docs/api/` covers the manifest schema, host integration, handler contracts, and diagnostics handling with examples.
- Developer onboarding material is available in `docs/dev/` for contributors extending the project.

## Publishing Notes

The core library is published as `aardvark-core`. Before cutting any experimental build:

- Audit the bundled Pyodide version and rebuild snapshots if needed.
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
- API stability is not guaranteed; expect breaking changes while the runtime matures.

## License

See `LICENSE` for details.
