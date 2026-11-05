# Local Pyodide Node Compatibility Harness

This directory contains local-only tooling for comparing Aardvark against the
upstream Pyodide Node test surface. It intentionally does not define a CI gate.

## Upstream Source

The upstream Pyodide checkout lives at:

```text
third_party/pyodide/314.0.7/
```

It is pinned to the Pyodide `314.0.7` release commit used by Aardvark's staged
distribution. When upgrading Pyodide, replace the checkout and rename the versioned
`cases/`, `expectations/` and `inventory/` files to match.

## Commands

Generate a static inventory of upstream tests:

```bash
python3 compat-tests/pyodide-node/collect_inventory.py --version 314.0.7
```

Run the first local Aardvark compatibility corpus:

```bash
python3 compat-tests/pyodide-node/run_local.py --version 314.0.7
```

Run Pyodide's lockfile-driven package import matrix:

```bash
python3 compat-tests/pyodide-node/run_package_imports.py --version 314.0.7
```

By default the runner resets the Aardvark runtime before each adopted case,
which matches the upstream function-scoped Selenium fixture more closely. Use
`--reuse-runtime` only for manual debugging of stateful failures.

Package-backed cases use the staged full distribution. Restage it after changing
runtime JS assets such as `crates/aardvark-core/src/js/pyodide_bootstrap.js`:

```bash
cargo run -p aardvark-cli -- assets stage --variant full --force
python3 compat-tests/pyodide-node/run_local.py \
  --version 314.0.7 \
  --timeout-seconds 120
```

The runner derives the default full-distribution path from the workspace package
version and requested Pyodide version:

```text
.aardvark/pyodide-distributions/aardvark-<aardvark-version>-pyodide-v<pyodide-version>-full
```

## Contract

For each adopted upstream test id, Aardvark should either:

- match upstream Pyodide under Node,
- be tracked as an Aardvark parity gap, or
- be classified as an intentional difference or out-of-contract host feature.

The expectations file for this version is:

```text
compat-tests/pyodide-node/expectations/314.0.7.toml
```

No skipped or failing adopted case should remain unclassified.

The local `runJs` adapter is intentionally Selenium-shaped: snippets run in the
active Pyodide context with `pyodide`, `self`, and small `assert`,
`assertThrows`, and `assertThrowsAsync` helpers in scope, and may use `return`
plus top-level `await`. The same helpers are installed on `globalThis` for
upstream cases that call JavaScript from Python via `pyodide.code.run_js`.

Cases may use either `code` or `codeLines`; `codeLines` is joined with newlines
before execution so larger upstream-derived snippets stay readable. Cases that
expect a guest exception should use `expectErrorContains`, which is treated as a
passing compatibility assertion when all listed substrings appear in the runner
response.

The package import runner mirrors upstream
`packages/_tests/test_packages_common.py`: it reads the staged
`pyodide-lock.json`, keeps entries whose `package_type` is `package` or
`cpython_module`, calls Aardvark `loadPackage` for each package, and imports
every declared module name. Packages Pyodide marks xfail or unsupported on Node
are classified in:

```text
compat-tests/pyodide-node/expectations/314.0.7-package-imports.toml
```

Package entries without declared imports are reported as `no_imports`; they are
not failures, but they also do not prove runtime importability.
