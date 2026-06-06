# Local Pyodide Node Compatibility Harness

This directory contains local-only tooling for comparing Aardvark against the
upstream Pyodide Node test surface. It intentionally does not define a CI gate.

## Upstream Source

The upstream Pyodide checkout lives at:

```text
third_party/pyodide/0.29.4/
```

It is pinned to the Pyodide `0.29.4` release commit used by Aardvark's staged
distribution. Future Pyodide updates should add a new versioned checkout instead
of overwriting this one.

## Commands

Generate a static inventory of upstream tests:

```bash
python3 compat-tests/pyodide-node/collect_inventory.py --version 0.29.4
```

Run the first local Aardvark compatibility corpus:

```bash
python3 compat-tests/pyodide-node/run_local.py --version 0.29.4
```

Run Pyodide's lockfile-driven package import matrix:

```bash
python3 compat-tests/pyodide-node/run_package_imports.py --version 0.29.4
```

By default the runner resets the Aardvark runtime before each adopted case,
which matches the upstream function-scoped Selenium fixture more closely. Use
`--reuse-runtime` only for manual debugging of stateful failures.

Package-backed cases use the staged full distribution. Restage it after changing
runtime JS assets such as `crates/aardvark-core/src/js/pyodide_bootstrap.js`:

```bash
cargo run -p aardvark-cli -- assets stage --variant full --force
python3 compat-tests/pyodide-node/run_local.py \
  --version 0.29.4 \
  --timeout-seconds 120 \
  --dist-dir .aardvark/pyodide-distributions/aardvark-0.1.1-pyodide-v0.29.4-full
```

## Contract

For each adopted upstream test id, Aardvark should either:

- match upstream Pyodide under Node,
- be tracked as an Aardvark parity gap, or
- be classified as an intentional difference or out-of-contract host feature.

The expectations file for this version is:

```text
compat-tests/pyodide-node/expectations/0.29.4.toml
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
compat-tests/pyodide-node/expectations/0.29.4-package-imports.toml
```

Package entries without declared imports are reported as `no_imports`; they are
not failures, but they also do not prove runtime importability.
