# Environment Setup

Aardvark is a Rust workspace with JavaScript and Python assets embedded into the
runtime. The following steps get you ready to develop locally.

## Prerequisites

- **Rust**: Install via `rustup` and ensure the toolchain matches
  `rust-toolchain.toml` (nightly features are not required).
- **Node.js**: Needed for bundling the [Pyodide](https://pyodide.org/) bootstrap assets. Use `mise` or
  another version manager to match the version pinned in `.mise.toml`.
- **Python 3.11+**: Only required for regenerating [Pyodide](https://pyodide.org/) metadata and running
  certain integration helpers.
- **wasm-pack** *(optional)*: Handy when inspecting [Pyodide](https://pyodide.org/) builds, but not part
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

3. Fetch [Pyodide](https://pyodide.org/) assets. Download the upstream release and copy the contents of
   the requested variant into `.aardvark/pyodide/<version>` so the runtime can
   serve wheel requests locally:

   ```bash
   mkdir -p .aardvark/pyodide/0.29.0
   curl -L -o pyodide-0.29.0.tar.bz2 \
     https://github.com/pyodide/pyodide/releases/download/0.29.0/pyodide-0.29.0.tar.bz2
   echo "85395f34a808cc8852f3c4a5f5d47f906a8a52fa05e5cd70da33be82f4d86a58  pyodide-0.29.0.tar.bz2" | sha256sum --check
   tar -xjf pyodide-0.29.0.tar.bz2
   rsync -a pyodide/pyodide/v0.29.0/full/ .aardvark/pyodide/0.29.0/
   rm -rf pyodide pyodide-0.29.0.tar.bz2
   ```

4. Build the workspace:

   ```bash
   cargo build
   ```

  The build downloads [V8](https://v8.dev/) via `v8-rs` the first time; this may take a while.
  Our `.cargo/config.toml` points `RUSTY_V8_MIRROR` at the PIC-enabled
  Aardvark release of V8 142.0.0 (built with `v8_monolithic=true` and
  `v8_monolithic_for_shared_library=true`). Override `RUSTY_V8_MIRROR` or
  `RUSTY_V8_ARCHIVE` if you need to test alternative builds.

  If you require additional GN tweaks, export `EXTRA_GN_ARGS=force_pic=true`
  (or other options) before rebuilding V8 so the objects match your needs.

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

- `AARDVARK_PYODIDE_PACKAGE_DIR` – path to a [Pyodide](https://pyodide.org/) wheel cache; required for
  package-loading tests.
- `AARDVARK_OVERLAY_CACHE_DIR` – directory used by overlay hydration tests.
- `RUST_LOG` – set to `info` or `debug` to see tracing spans while running the
  CLI or tests.
- `EXTRA_GN_ARGS` – appended to the GN invocation when rebuilding V8. Export
  values like `force_pic=true` to tune the V8 build for downstream needs.
- `RUSTY_V8_MIRROR` – defaults (via `.cargo/config.toml`) to our PIC-friendly
  V8 142.0.0 release. Override to consume a different archive or mirror.
