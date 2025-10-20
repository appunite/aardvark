# Architecture Overview

This document introduces Aardvark’s execution model from the host’s point of view. It is written for engineers who need to embed the runtime, reason about the layering, or evaluate whether the current features satisfy a production workload.

## System Goals

- Provide deterministic, embeddable runtimes (Python via Pyodide today, JavaScript via a V8 embed) without shipping a browser.
- Allow hosts to preload dependencies (via manifests, overlays, and snapshots) so cold-start cost stays predictable.
- Enforce resource limits inside the same process: CPU, wall time, heap, filesystem writes, and outbound network access.
- Keep the host-facing API small enough for Rust, but expose structured diagnostics so other host languages can wrap it later.
- Surface reset timings and sandbox telemetry so pooling strategies remain observable without tracing every call.

## Layers at a Glance

1. **Host integration (`aardvark-core`)** – public Rust API exposing persistent isolates (`PythonIsolate`), the serial pool (`BundlePool`), and low-level access to `PyRuntime`.
2. **Runtime coordinator (`runtime.rs`)** – orchestrates session preparation, policy wiring, invocation strategies, and telemetry collection.
3. **Language engines (`runtime/python.rs`, `runtime/javascript.rs`)** – embed V8, load language-specific bootstraps, manage network/filesystem shims, and emit low-level traces.
4. **Guest bootstraps (`pyodide_bootstrap.js`, `js_runtime_bootstrap.js`)** – configure the interpreter, install packages (Python only), and enforce sandbox rules inside the WASM VM.
5. **User code** – Python bundles (Pyodide) or JavaScript modules executed under the same sandbox contract.

The layers are intentionally narrow: the host only talks to the coordinator, which in turn controls the JS engine. Python never reaches directly into host resources.

```mermaid
graph TD
  Host[Host Service] -->|Bundles + Config| Core[aardvark-core API]
  Core -->|Prepare/Run| Runtime[Runtime Coordinator]
  Runtime -->|Embed| Engine[V8 Engine Wrapper]
  Engine -->|Boot + Policies| Bootstrap[Language Bootstrap]
  Bootstrap -->|Executes| Guest["Guest Handler (Python/JS)"]
  Guest -->|Stdout/Stderr/Result| Runtime
  Runtime -->|Diagnostics + Outcome| Core
  Core -->|Telemetry + Payload| Host
```

## Execution Flow (Top to Bottom)

1. **Bundle ingestion** – Hosts create a `Bundle` (or `BundleArtifact`) from a ZIP archive. `bundle.rs` normalises paths and (optionally) extracts `aardvark.manifest.json`.
2. **Session negotiation** – `PythonIsolate` forwards to `PyRuntime::prepare_session_with_manifest`, which validates the manifest, derives descriptor limits, applies manifest resource policies, and loads any listed packages before mounting the bundle at `/app`.
3. **Strategy selection** – Unless overridden, `DefaultInvocationStrategy` serializes the descriptor and dispatches to the configured guest language. JavaScript reuses the same contract through `JavaScriptInvocationStrategy`.
4. **Watchdogs** – Wall-clock and CPU watchdogs arm before calling into the guest. Heap usage is checked both before and after execution.
5. **Sandbox enforcement** – The JS layer enforces network allowlists (HTTPS by default), filesystem mode/quota, and host capability gates for native bridges. Violations are raised back to Rust.
6. **Outcome synthesis** – Captured stdout/stderr, console messages, payloads, sandbox telemetry, and policy violations are combined into `ExecutionOutcome`.
7. **Reset** – Depending on `ResetPolicy`, runtimes either rebuild the engine (`reset_to_snapshot`) or scrub it in place (`reset_in_place`). Warm states captured inside the runtime bake the overlay into the snapshot, so in-place resets reuse site-packages without rehydrating tarballs. `BundlePool` currently processes calls serially, so resets happen synchronously when the pool guard drops.

The same flow is used whether the runtime comes from a pool or is standalone. Pooling only changes lifecycle management around steps 2 and 7.

```mermaid
sequenceDiagram
    participant Host
    participant Core as PyRuntime
    participant Engine as JsRuntime
    participant Pyodide
    participant Handler as Python Handler

    Host->>Core: prepare_session_with_manifest(bundle)
    Core->>Engine: ensure_pyodide_module()
    Core->>Engine: set policies
    Core->>Engine: mount bundle / load packages
    Core->>Host: PySession
    Host->>Core: run_session(session)
    Core->>Engine: arm watchdogs
    Engine->>Pyodide: execute entrypoint
    Pyodide->>Handler: module:function(*args)
    Handler-->>Pyodide: payload/exception
    Pyodide-->>Engine: stdout/stderr/events
    Engine-->>Core: ExecutionOutput + telemetry
    Core-->>Host: ExecutionOutcome
```

## Design Notes

- **Pyodide inside V8** – Running Pyodide in V8 lets us reuse the same WASM module across invocations, keep snapshots small, and apply V8’s tooling (heap statistics, isolate limits) to Python workloads. The JavaScript engine reuses the same embed without the Pyodide layer, making cross-language behaviour consistent.
- **Descriptor-first contract** – Manifests are optional at runtime. Hosts can provide `InvocationDescriptor`s directly when they need to override limits or use fully dynamic pipelines. Manifests exist to make bundles self-describing for less opinionated hosts.
- **“Everything is a bundle”** – Packages, manifests, and entrypoints travel together inside a single ZIP. This keeps the host API simple and avoids filesystem mutation outside the sandbox when code is deployed.
- **Telemetry as a first-class product** – Diagnostics always attach CPU, filesystem, network telemetry, queue wait durations (if pooled), reset summaries, and heap stats even when the invocation fails. Hosts can surface policy violations without parsing logs.
- **Reset visibility** – Each invocation records how the runtime was reset (recreate vs in-place), how long it took, and which engine generation served the handler, making pool behaviour observable without diving into logs.

## Current Limitations

- Only Linux/macOS targets are exercised. Windows builds are untested and expected to fail.
- Shared buffer handles present zero-copy views backed by the runtime; the host may still materialize owned copies when required.
- JavaScript bundles must be fully self-contained. Ship pre-bundled modules (e.g., via esbuild/webpack) because the runtime does not resolve npm packages or fetch external scripts.
- Snapshot overlays assume Pyodide 0.28.2. Future Pyodide upgrades will require regenerating overlay metadata and schema version bumping. When hosts build warm states out of band they must flag them as overlay-preloaded to avoid redundant imports.
- Network sandboxing is allowlist-based per session. There is no per-request override yet, and DNS leakage is not mitigated beyond host matching.
- Filesystem quota enforcement only tracks writes within the virtual session directory. If code escapes to other WASM-visible mounts it will currently fail closed but without detailed accounting.
- `BundlePool` is single-isolate for now. Use it for serial reuse and telemetry, but do not rely on it for parallelism until the multi-isolate design lands.
