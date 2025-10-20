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
  cargo fmt
  npm run lint:js   # if you have local npm scripts; otherwise run prettier/eslint manually
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
- JS shim tests (run under Node with mocked Pyodide):
  ```bash
  node crates/aardvark-core/tests/js/run-tests.mjs
  ```
- Integration tests:
  ```bash
  cargo test -p integration-tests -- --nocapture
  ```
  These will mount real overlay caches. Ensure `AARDVARK_PYODIDE_PACKAGE_DIR`
  points at an unpacked cache.

### Smoke Testing the CLI

```
AARDVARK_PYODIDE_PACKAGE_DIR=.aardvark/pyodide/0.28.2 \
  cargo run -p aardvark-cli -- \
  --bundle hello_bundle.zip --entrypoint main:main --manifest
```

Set `RUST_LOG=aardvark::telemetry=info` to verify tracing and sandbox telemetry output.

## JS/Pyodide Asset Workflow

- Edit the bootstrap code under `crates/aardvark-core/src/js/`.
- Keep the assets ASCII-only to simplify embedding.
- After changes, rebuild the core crate. The JS is bundled into the Rust
  binary via `include_str!` and hashed for cache busting.

## Manifest Schema Updates

- Modify `crates/aardvark-core/schemas/aardvark.bundle-manifest.schema.json`.
- Regenerate the Rust manifest structs if new fields are added.
- Update `docs/api/manifest.md` and add tests under
  `crates/aardvark-core/tests/` to cover new validation paths.
- Version the schema thoughtfully. Until 1.0, semver ranges are informally
  enforced; once we commit to stable, update the schema version string and keep
  old versions compatible when possible.

## Telemetry and Tracing

- All tracing spans use the `aardvark::*` targets. Subscribe with
  `tracing-subscriber` locally if you need to debug budget enforcement. The
  pool reporter logs queue metrics under `aardvark::telemetry`; tweak
  `PoolOptions::telemetry_interval` to manage its cadence.
- When touching diagnostics, update `docs/architecture/telemetry.md` and keep
  telemetry structs backwards compatible.

## Pull Request Checklist

- [ ] `cargo fmt` and `cargo clippy` succeed.
- [ ] Unit tests and integration tests pass.
- [ ] JS shims evaluated or lightly smoke tested via CLI.
- [ ] Docs updated (`docs/api`, `docs/architecture`, and `docs/dev`) when
      behaviour changes.
- [ ] Changelog entry added (see `release.md`).
