# Development Workflow

Aardvark balances Rust, JavaScript, and Python pieces. This guide walks through
the daily workflow.

## Building and Formatting

- **Incremental builds**:
  ```bash
  cargo build -p aardvark-core
  cargo build -p aardvark-cli
  ```
- **Format everything**:
  ```bash
  cargo fmt --all
  ```
- **Clippy**:
  ```bash
  cargo clippy --workspace --all-targets -- -D warnings
  ```

## Testing

### Unit and Integration Tests

- Core library unit tests:
  ```bash
  cargo test -p aardvark-core
  ```
- Sandbox regression tests (network/filesystem/capability coverage):
  ```bash
  cargo test -p aardvark-core --test runtime_pool_and_outcome
  ```
- Workspace tests without the slower integration crate:
  ```bash
  PYODIDE_DIST_DIR="$(find .aardvark/pyodide-distributions -maxdepth 1 -type d -name 'aardvark-*-pyodide-v0.29.4-full' | sort | tail -n 1)"
  test -n "$PYODIDE_DIST_DIR"
  AARDVARK_PYODIDE_DIST_DIR="$PYODIDE_DIST_DIR" \
    cargo test --workspace --exclude integration-tests
  ```
- Overlay/catalog integration tests:
  ```bash
  cargo test -p integration-tests -- --nocapture
  ```
  These will mount real overlay caches. Ensure `AARDVARK_PYODIDE_DIST_DIR`
  points at a verified Aardvark Pyodide distribution (or run
  `cargo run -p aardvark-cli -- assets stage --variant full` beforehand).

- Local Pyodide compatibility harness:
  ```bash
  python3 compat-tests/pyodide-node/run_local.py --version 0.29.4
  python3 compat-tests/pyodide-node/run_package_imports.py --version 0.29.4
  ```
  This is a local Node-shaped Pyodide parity check, not a CI gate and not a
  browser conformance suite.

### Smoke Testing the CLI

```
PYODIDE_DIST_DIR="$(find .aardvark/pyodide-distributions -maxdepth 1 -type d -name 'aardvark-*-pyodide-v0.29.4-full' | sort | tail -n 1)"
test -n "$PYODIDE_DIST_DIR"
AARDVARK_PYODIDE_DIST_DIR="$PYODIDE_DIST_DIR" \
  cargo run -p aardvark-cli -- \
  --bundle example/numpy_bundle.zip --entrypoint main:main --package numpy
```

Set `RUST_LOG=aardvark::telemetry=info` to verify tracing and sandbox telemetry output.

## JS/[Pyodide](https://pyodide.org/) Asset Workflow

- Edit the bootstrap code under `crates/aardvark-core/src/js/`.
- Keep the assets ASCII-only to simplify embedding.
- After changes, rebuild the core crate. JS assets are embedded through
  `include_str!` or copied into generated [Pyodide](https://pyodide.org/) assets by `build.rs`.
- There is no root npm lint/unit harness for the embedded JS assets. Cover
  behavioural changes with Rust regression tests, the CLI smoke path, and the
  local Pyodide compatibility harness when Pyodide-facing behaviour changes.

## Manifest Schema Updates

- Modify `crates/aardvark-core/schemas/aardvark.bundle-manifest.schema.json`.
- Regenerate the Rust manifest structs if new fields are added.
- Update `docs/api/manifest.md` and add tests under
  `crates/aardvark-core/tests/` to cover new validation paths.
- Version the schema thoughtfully. Until 1.0, semver ranges are informally
  enforced; once we commit to stable, update the schema version string and
  document migration impact explicitly.

## Telemetry and Tracing

- All tracing spans use the `aardvark::*` targets. Subscribe with
  `tracing-subscriber` locally if you need to debug budget enforcement. The
  pool reporter logs queue metrics under `aardvark::telemetry`; tweak
  `PoolOptions::telemetry_interval` to manage its cadence.
- When touching diagnostics, update `docs/architecture/telemetry.md` and avoid
  breaking existing telemetry consumers unless the migration is documented.

## Pull Request Checklist

- [ ] `cargo fmt` and `cargo clippy` succeed.
- [ ] Unit tests and integration tests pass.
- [ ] JS shims evaluated or lightly smoke tested via CLI.
- [ ] Docs updated (`docs/api`, `docs/architecture`, and `docs/dev`) when
      behaviour changes.
- [ ] Release notes or changelog updated when published behaviour changes.
