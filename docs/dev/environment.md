# Environment Setup

Aardvark is a Rust workspace with JavaScript and Python assets embedded into the
runtime. The following steps get you ready to develop locally.

## Prerequisites

- **Rust**: The workspace pins Rust in `.mise.toml` and `Cargo.toml`
  `rust-version`. Run `mise install`, or install the same version with
  `rustup`; nightly features are not required.
- **Node.js** *(optional)*: Pinned in `.mise.toml` for ad-hoc JavaScript
  inspection. The checked-in build does not have root npm scripts, and `cargo
  build` embeds JavaScript assets directly.
- **Python 3.13**: Used by the perf host runner and helper scripts. `mise
  install` installs the pinned version.
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

3. Stage [Pyodide](https://pyodide.org/) assets. The runtime never downloads
   wheels at execution time, so make sure a local cache exists before running
   tests or integration scenarios. You can:

   - Enable the `aardvark-core/full-pyodide-packages` feature when building.
     The build script downloads the full 0.29.0 release, verifies it, and
     points `PyRuntimeConfig::default()` at the extracted cache.
   - Run the CLI helper: `cargo run -p aardvark-cli -- assets stage` downloads
     and flattens the full variant into `.aardvark/pyodide/0.29.0/` (use
     `--variant core`, `--output <dir>`, or `--force` as needed).
   - Stage manually with the upstream tarball:

     ```bash
     mkdir -p .aardvark/pyodide/0.29.0
     curl -L -o pyodide-0.29.0.tar.bz2 \
       https://github.com/pyodide/pyodide/releases/download/0.29.0/pyodide-0.29.0.tar.bz2
     echo "85395f34a808cc8852f3c4a5f5d47f906a8a52fa05e5cd70da33be82f4d86a58  pyodide-0.29.0.tar.bz2" | sha256sum --check
     tar -xjf pyodide-0.29.0.tar.bz2
     rsync -a pyodide/pyodide/v0.29.0/full/ .aardvark/pyodide/0.29.0/
     rm -rf pyodide pyodide-0.29.0.tar.bz2
     ```

   In all cases, point the runtime at the cache via
   `AARDVARK_PYODIDE_PACKAGE_DIR` or by calling
   `PyRuntimeConfig::set_pyodide_package_dir`.

4. Build the workspace:

   ```bash
   cargo build
   ```

  The build downloads [V8](https://v8.dev/) via `v8-rs` the first time; this may take a while.

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
- `AARDVARK_PYODIDE_ARCHIVE` – local [Pyodide](https://pyodide.org/) archive consumed by
  `crates/aardvark-core/build.rs` instead of downloading one.
- `AARDVARK_PYODIDE_DIR` – local unpacked [Pyodide](https://pyodide.org/) asset directory copied by
  `crates/aardvark-core/build.rs` instead of downloading an archive.
- `AARDVARK_OVERLAY_CACHE_DIR` – directory used by overlay hydration tests.
- `RUST_LOG` – set to `info` or `debug` to see tracing spans while running the
  CLI or tests.
