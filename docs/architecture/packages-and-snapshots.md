# Packages, Bundles, and Snapshots

This note explains how code and dependencies arrive in the runtime and how repeated invocations avoid cold-start penalties.

## Bundles

- Bundles are standard ZIP archives. `bundle.rs` rejects absolute paths, parent-directory escapes, and non-UTF8 names to keep mounting predictable.
- `aardvark.manifest.json` is optional but recommended. When missing, hosts must provide an `InvocationDescriptor` and package list manually.
- Python modules should live under `/app`. Supporting data files can be included as additional entries and accessed via `importlib.resources` or direct paths.

```mermaid
graph TD
    BundleZIP[Bundle ZIP] -->|extract| Manifest[aardvark.manifest.json]
    BundleZIP --> Code["/app modules"]
    BundleZIP --> Data[static assets]
    Manifest --> Runtime[PyRuntime]
    Code --> Pyodide
    Data --> Pyodide
```

## Manifest-driven packages

- The manifest’s `packages` field lists [Pyodide](https://pyodide.org/) packages to preload. Names are normalised (trimmed, deduplicated, lowercase for comparisons).
- During session preparation the JS bootstrap resolves dependencies using [Pyodide](https://pyodide.org/)’s lockfile from the staged Aardvark Pyodide distribution and installs wheels from `AARDVARK_PYODIDE_DIST_DIR` (or the path set via `PyRuntimeConfig::with_pyodide_dist_dir`). The distribution manifest verifies the runtime files, package files, adapter version, lockfile hash, and compatibility fingerprint before use.
- Dynamic libraries required by those packages are preloaded immediately after installation so they remain available during snapshot capture and execution.

```mermaid
flowchart LR
    ManifestPackages --> Resolver[Resolve dependencies]
    Resolver --> Distribution{Wheel exists in distribution?}
    Distribution -- yes --> Install[Install into /session/site-packages]
    Distribution -- no --> FailFast[error: distribution missing package]
    Install --> Dynlib[Preload dynlibs]
    Dynlib --> Ready[Packages ready]
```

**Limitations**

- Manifests only support the bundled [Pyodide](https://pyodide.org/) version (currently `314.0.7`). Requests for a different version fail fast.
- Package installation still hits the local filesystem. Ensure hosts point `AARDVARK_PYODIDE_DIST_DIR` at a prepared distribution.

## Snapshots

- Passing `snapshot.load_from` in `PyRuntimeConfig` hydrates [Pyodide](https://pyodide.org/) from a previously captured memory snapshot, skipping import time.
- When `snapshot.save_to` is set, `PyRuntime::prepare_session_with_descriptor` writes a new snapshot after the first load, including overlay metadata.
- Snapshot exports generate:
  - the main memory image,
  - a snapshot sidecar with the active Pyodide compatibility fingerprint,
  - a content-addressed TarFS blob containing `/session/site-packages` and `/usr/lib` deltas,
  - a JSON index describing the overlay contents, preload instructions, and compatibility fingerprint.

```mermaid
flowchart TD
    Execute[First warm run] --> SnapshotTrigger{snapshot.save_to set?}
    SnapshotTrigger -- yes --> Capture[Capture memory image]
    Capture --> Overlay[Write TarFS overlay]
    Overlay --> Index[Update overlay index]
    Index --> SnapshotArtifacts[Snapshot bundle]
    SnapshotTrigger -- no --> End[Skip capture]
```

### Native extensions (`.so` side modules)

Packages such as numpy and pandas load WebAssembly side modules with `dlopen`.
Each side module owns a wasm instance and a range of indirect-function-table
slots, and neither is part of the heap image. The heap still holds pointers
into both: table indices for C function pointers, and data addresses relative
to each module's memory base. A restore that only copies the heap leaves those
pointers dangling (`table index is out of bounds` or `null function`).

So a snapshot also records the dynamic-linking state:

- every side module in load order, with its memory base, table base, sizes and
  `RTLD_GLOBAL` flag;
- the `dlopen` handles that point at each module;
- the GOT (resolved data addresses and function-table indices) and, for each
  function entry, which module exports the function;
- the function-table length.

It travels inside the snapshot header next to Pyodide's own hiwire state.

On restore:

1. Right after the heap is copied in, the table is grown to its recorded
   length and the recorded GOT values are preset. Nothing allocated later can
   take a slot that the heap already points at.
2. Once the overlay has put the `.so` files back on the filesystem, every side
   module is re-instantiated at its recorded memory and table base, in the
   original order. The patched Emscripten loader keeps the restored heap bytes
   over the module's initial data, and skips relocations and constructors,
   which already ran before the snapshot.
3. `dlopen` handles are re-attached, and any GOT function slot that is still
   empty is refilled from its exporting module.

The loader hooks are part of the patched Pyodide glue that staging writes, so
this requires a distribution staged with adapter version 2 or later. A replay
that can't find a module's file, or finds a module whose size changed, fails
the restore instead of running with a broken heap.

**Limitations**

- Overlay hydration currently assumes a single overlay catalog per overlay cache root. Stale entries are ignored on fingerprint mismatch, but hosts must still coordinate manual pruning when deleting blobs out of band.
- Snapshots are architecture-specific. Do not share them across mismatched CPU features.
- Explicit snapshot loads, cached snapshot bytes, and `WarmState` restores fail when the compatibility fingerprint is missing or does not match the active distribution. Stale overlay catalog entries are ignored and rebuilt.

## Using Multiple Bundles

- `BundlePoolRegistry` routes each bundle/profile pair to its own warmed pool.
  Size `PoolOptions::desired_size` and `PoolOptions::max_size` for the
  concurrency each dependency set needs.
- Runtimes are not multi-tenant: only one session runs at a time within an isolate. Use additional pool capacity to parallelise.

## Alternative Loader Paths

- Hosts needing bespoke package resolution can skip the manifest packages and install requirements manually by calling into [Pyodide](https://pyodide.org/) through a custom strategy before handing control to user code.
- LangChain-style bundles with large optional dependencies can be shipped as separate snapshots; the manifest can reference the minimal package set and rely on the host to choose the right snapshot per request.
