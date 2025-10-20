# Repository Guidelines

## Project Structure & Module Organization
- `crates/aardvark-core/`: primary runtime library embedding [Pyodide](https://pyodide.org/) and managing sandbox policies. Submodules cover assets (`src/js/`, `src/py/`), manifest parsing, pooling, and invocation strategies.
- `crates/aardvark-cli/`: developer CLI that exercises the core library end to end.
- `integration-tests/`: slower overlay and pooling scenarios that rely on prepared caches.
- `docs/`: public architecture/API references and `docs/dev/` for contributor workflow notes.
- `scripts/` and `.aardvark/`: helper tooling plus developer-managed caches ([Pyodide](https://pyodide.org/) wheels, overlays). The `.aardvark/pyodide/<version>` directory is ignored by git and should contain the flattened [Pyodide](https://pyodide.org/) release you stage locally.

## Build, Test, and Development Commands
- `cargo build -p aardvark-core`: compile the runtime library; run before editing JS shims to confirm bindings.
- `cargo test --workspace`: run unit tests across all crates.
- `cargo test -p integration-tests -- --nocapture`: run overlay hydration and pooling tests; requires `AARDVARK_PYODIDE_PACKAGE_DIR`.
- `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings`: enforce formatting and lint rules prior to commits.

## Coding Style & Naming Conventions
- Rust: standard `rustfmt` (4-space indentation). Prefer descriptive module names (`runtime_lifecycle.rs` over `rt.rs`) and snake_case for files/functions.
- JavaScript assets: ES modules targeting the bundled [V8](https://v8.dev/); keep ASCII-only strings for `include_str!` compatibility.
- Documentation: use Markdown headings, concise paragraphs, and place developer-facing notes in `docs/dev/`.

## Testing Guidelines
- Rust tests use `cargo test`; place new unit tests beside implementation files and integration tests under `integration-tests/tests/` with descriptive filenames (e.g., `runtime_pool_and_outcome.rs`).
- When modifying JS sandboxing, add or update assertions in `crates/aardvark-core/tests/js/` and rerun the Node harness.
- Ensure new features expose telemetry or policy changes via tests before merging.

## Commit & Pull Request Guidelines
- Follow the existing imperative tone (e.g., "Add manifest parser", "Document developer workflow"). Group related doc and code edits together.
- Each PR should include: summary of changes, testing evidence (`cargo test` output or notes), updated docs when behaviour changes, and references to tracked issues or feature requests if applicable.
- Squash merge by default; release commits should be tagged (see `docs/dev/release.md`).
