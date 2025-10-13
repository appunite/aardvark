# Environment Setup

Aardvark is a Rust workspace with JavaScript and Python assets embedded into the
runtime. The following steps get you ready to develop locally.

## Prerequisites

- **Rust**: Install via `rustup` and ensure the toolchain matches
  `rust-toolchain.toml` (nightly features are not required).
- **Node.js**: Needed for bundling the Pyodide bootstrap assets. Use `mise` or
  another version manager to match the version pinned in `.mise.toml`.
- **Python 3.11+**: Only required for regenerating Pyodide metadata and running
  certain integration helpers.
- **wasm-pack** *(optional)*: Handy when inspecting Pyodide builds, but not part
  of the default build.

## Bootstrapping

1. Clone the repository and enter it:

   ```bash
   git clone git@github.com:your-org/aardvark.git
   cd aardvark
   ```

2. Sync tool versions with `mise` (optional but recommended):

   ```bash
   mise install
   ```

3. Fetch Pyodide assets. The build script expects a cached tarball inside
   `tmp/`. Use the helper script:

   ```bash
   scripts/fetch-pyodide.sh   # downloads the version pinned in assets.rs
   ```

4. Build the workspace:

   ```bash
   cargo build
   ```

   The build downloads V8 via `v8-rs` the first time; this may take a while.

## Project Layout

- `crates/aardvark-core/` – the runtime library and the bulk of the logic.
- `crates/aardvark-cli/` – developer CLI wrapper around the core library.
- `integration-tests/` – slow tests that exercise TarFS overlays and runtime
  pooling.
- `docs/` – public and developer documentation.
- `internal_docs/` – historical research notes (ignored in git).
- `scripts/` – utility scripts for cache maintenance and asset syncing.

## IDE Hints

- Enable `rust-analyzer` proc-macro support.
- Configure the TypeScript/JS tools to understand ES modules inside
  `crates/aardvark-core/src/js/`.
- If you use VS Code, add a task that runs `cargo fmt && cargo clippy` before
  committing.

## Common Environment Variables

- `AARDVARK_PYODIDE_PACKAGE_DIR` – path to a Pyodide wheel cache; required for
  package-loading tests.
- `AARDVARK_OVERLAY_CACHE_DIR` – directory used by overlay hydration tests.
- `RUST_LOG` – set to `info` or `debug` to see tracing spans while running the
  CLI or tests.
