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

### Preparing Pyodide assets

The runtime expects a local Pyodide cache. Use the helper to download and verify the pinned build:

```
cargo aardvark fetch-pyodide --version 0.28.2 --variant core
```

Assets land under `./.aardvark/pyodide/0.28.2/core`. Point `AARDVARK_PYODIDE_PACKAGE_DIR` (or `PyRuntimeConfig::pyodide_version`) at that directory before running the runtime. Swap `--variant full` for the full bundle or add extras like `--extra static-libraries,xbuildenv` when you need development tooling.

Only need the downloader? Install it standalone:

```
cargo install aardvark-cli --no-default-features --features fetcher
```

Prefer a pure shell workflow? Mirror the helper with `curl` and `tar`:

```
curl -L -o pyodide-core-0.28.2.tar.bz2 \
  https://github.com/pyodide/pyodide/releases/download/0.28.2/pyodide-core-0.28.2.tar.bz2
echo "c9f6dd067d119e50850849f7428e3c636ecbc2684a0d2ff992f3bd48a1062b6c  pyodide-core-0.28.2.tar.bz2" | sha256sum --check
tar -xjf pyodide-core-0.28.2.tar.bz2
mkdir -p .aardvark/pyodide/0.28.2
mv pyodide .aardvark/pyodide/0.28.2/core
```

Adjust the URL and checksum if you pin a different version.

### Building CLI release binaries

Use the workspace task runner to produce release artefacts for both CLI variants:

    cargo install cross
    cargo run -p xtask -- release-cli

The task ensures the required Rust targets are installed (`rustup target add …`) before building.

The command writes binaries into `./dist/`, building the full runtime CLI (`aardvark-cli`) and the downloader-only helper (`cargo-aardvark`) for the default targets (`x86_64-apple-darwin` and `x86_64-unknown-linux-gnu`).

Useful flags:

- `--targets <triple[,triple]>` – override the target list.
- `--skip-full` / `--skip-fetcher` – build only one variant.
- `--out-dir <path>` – choose a different output directory.

Prefer to run the underlying cross-compiles yourself?

    cross build -p aardvark-cli --release --target x86_64-unknown-linux-gnu
    cross build -p aardvark-cli --release --no-default-features --features fetcher --target x86_64-unknown-linux-gnu

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
