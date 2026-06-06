# Pyodide Implementation Status

This document describes the current Aardvark Pyodide integration as it exists
today. It is a status note, not a complete upstream conformance claim.

## Upstream Base

- Aardvark targets Pyodide `0.29.4`.
- The local upstream checkout is a git submodule at
  `third_party/pyodide/0.29.4/`.
- That checkout is pinned to Pyodide tag `0.29.4`, commit
  `d178a7e14644eb17dfd7bd91dc21b19078868381`.
- Runtime assets are staged under the ignored local distribution directory:

```text
.aardvark/pyodide-distributions/aardvark-0.1.1-pyodide-v0.29.4-full
```

Restage the full distribution after changing Pyodide-facing JS assets:

```bash
cargo run -p aardvark-cli -- assets stage --variant full --force
```

## Runtime Shape

Aardvark embeds Pyodide inside the in-process V8 runtime. The bootstrap path is:

1. Rust creates a `JsRuntime` isolate.
2. Core Pyodide JS and WASM assets are loaded from the staged distribution.
3. Aardvark injects the Emscripten setup shim and `pyodide_bootstrap.js`.
4. The bootstrap exposes host hooks for package loading, filesystem policy,
   network policy, diagnostics, timers, and reset.
5. Python snippets execute through the loaded Pyodide instance.

Package loading is local-distribution based. Aardvark does not currently treat
CDN downloads, arbitrary wheel downloads, or network-backed `micropip` installs
as part of the supported runtime contract.

## Implemented Compatibility Surface

The local compatibility work covers a Node-shaped subset of upstream Pyodide:

- `runPython` and `runPythonAsync` execution.
- Selenium-shaped JavaScript snippets through the local compat runner.
- Python-to-JS and JS-to-Python proxy behavior represented in the adopted cases.
- Selected upstream asyncio, webloop, stream, filesystem, package loading,
  stdlib, type-conversion, `JsProxy`, and `PyProxy` cases.
- Local package loading for `package` and `cpython_module` entries from
  `pyodide-lock.json`.
- Import checks for every package that declares import names in the lockfile.

Implementation details that matter for current parity:

- `globalThis.pyodide` is set when `loadPyodide` resolves.
- `pyodide.loadedPackages` is updated for packages installed from the staged
  distribution.
- `package_type = "cpython_module"` is accepted by the package loader.
- Delay-aware timers and a small event-loop pump support adopted asyncio and
  webloop cases.
- Emscripten's internal generated `eval(...)` calls are preserved for trusted
  staged package JS. Some upstream packages need this for dynamic-library import
  glue.

## Current Verification

The local Pyodide compatibility harness lives in:

```text
compat-tests/pyodide-node/
```

Current local evidence:

- Adopted runtime corpus:
  `python3 compat-tests/pyodide-node/run_local.py --version 0.29.4 --timeout-seconds 120`
  reports `69` passed cases and no failures.
- Package import matrix:
  `python3 compat-tests/pyodide-node/run_package_imports.py --version 0.29.4 --timeout-seconds 180`
  reports `293` pass, `68` no-import metadata entries, `6` upstream-expected
  xfail entries, and no Aardvark failures.
- Workspace verification has also passed with:

```bash
cargo fmt --all --check
cargo clippy -p aardvark-core -p aardvark-cli -p aardvark-compat-runner --all-targets -- -D warnings
cargo test --workspace
```

The six expected package xfails match upstream or upstream Node behavior:
`cmyt`, `galpy`, `matplotlib-inline`, `numpy-tests`, `soupsieve`, and `yt`.

## What This Proves

We can currently say:

- Aardvark passes the adopted local Pyodide `0.29.4` Node-shaped runtime corpus.
- Aardvark can load the full staged Pyodide `0.29.4` lockfile package set far
  enough to import every package-declared module that upstream marks as
  supported in Node.
- The known package import exceptions are classified against upstream
  expectations.

## What This Does Not Prove

Do not claim that Aardvark passes all upstream Pyodide tests. The current harness
does not prove:

- DOM, canvas, browser, webworker, or service-worker APIs.
- Browser event-loop behavior beyond the local Node-shaped cases.
- CDN package loading, network wheel installs, or unrestricted `micropip`
  behavior.
- Full handwritten upstream package test suites such as `scipy-tests`.
- Importability for package entries that have no declared import names in
  `pyodide-lock.json`.
- Host behavior that conflicts with Aardvark sandbox, filesystem, or network
  policy.

The correct public claim is narrower: Aardvark passes the adopted local
Pyodide Node compatibility corpus and the lockfile-driven package import matrix
for the pinned Pyodide version.

## Updating Pyodide

For a new Pyodide release:

1. Add a new versioned upstream checkout under `third_party/pyodide/<version>/`.
   Do not overwrite the previous pinned checkout.
2. Stage a matching local Aardvark distribution.
3. Regenerate or refresh the upstream inventory:

```bash
python3 compat-tests/pyodide-node/collect_inventory.py --version <version>
```

4. Port or add adopted runtime cases for the new version.
5. Run the runtime corpus:

```bash
python3 compat-tests/pyodide-node/run_local.py --version <version>
```

6. Run the package import matrix:

```bash
python3 compat-tests/pyodide-node/run_package_imports.py --version <version>
```

7. Update expectations only for upstream-known xfails, Node-unsupported
   behavior, or intentionally unsupported Aardvark host behavior.
8. Update this status document with the new pinned version, verification
   results, and any narrowed or expanded contract.
