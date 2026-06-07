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
   wheels at execution time, so make sure a local Aardvark Pyodide distribution
   exists before running package-loading tests or integration scenarios:

   ```bash
   cargo run -p aardvark-cli -- assets stage --variant full
   PYODIDE_DIST_DIR="$(find .aardvark/pyodide-distributions -maxdepth 1 -type d -name 'aardvark-*-pyodide-v0.29.4-full' | sort | tail -n 1)"
   test -n "$PYODIDE_DIST_DIR"
   cargo run -p aardvark-cli -- assets verify "$PYODIDE_DIST_DIR"
   ```

   Point the runtime at that directory with `AARDVARK_PYODIDE_DIST_DIR` or by
   calling `PyRuntimeConfig::set_pyodide_dist_dir`. Use `--variant core` only
   for scenarios that do not need the full wheel set.

4. Build the workspace:

   ```bash
   cargo build
   ```

  The build downloads [V8](https://v8.dev/) via `v8-rs` the first time; this may take a while.

## Project Layout

- `crates/aardvark-core/` – the runtime library and the bulk of the logic.
- `crates/aardvark-cli/` – developer CLI wrapper around the core library.
- `integration-tests/` – slow tests for snapshot overlays and the overlay
  catalog. Pool and warmed-host coverage lives mostly under
  `crates/aardvark-core/tests/`.
- `docs/` – public and developer documentation.
- `internal_docs/` – historical research notes (ignored in git).
- `scripts/` – utility scripts for asset and overlay maintenance.

## IDE Hints

- Enable `rust-analyzer` proc-macro support.
- Configure the TypeScript/JS tools to understand ES modules inside
  `crates/aardvark-core/src/js/`.
- If you use VS Code, add a task that runs `cargo fmt && cargo clippy` before
  committing.

## Common Environment Variables

- `AARDVARK_PYODIDE_DIST_DIR` – path to a staged Aardvark Pyodide distribution;
  required for package-loading tests that use external assets.
- `AARDVARK_PYODIDE_ARCHIVE` – local [Pyodide](https://pyodide.org/) archive consumed by
  `crates/aardvark-core/build.rs` instead of downloading one.
- `AARDVARK_PYODIDE_DIR` – local unpacked [Pyodide](https://pyodide.org/) asset directory copied by
  `crates/aardvark-core/build.rs` instead of downloading an archive. This is a
  build-time contributor override, not the runtime package contract.
- `AARDVARK_OVERLAY_CACHE_DIR` – directory used by overlay hydration tests.
- `RUST_LOG` – set to `info` or `debug` to see tracing spans while running the
  CLI or tests.
