# Runtime Lifecycle

This note walks through how the runtime starts, runs, and tears down guest sessions. It highlights the checkpoints that enforce isolation and what the host can expect to observe in telemetry.

## Session States

1. **Bundle staging** – the host parses a bundle into a `BundleArtifact`, calculates its fingerprint, and optionally snapshots a warm state ([Pyodide](https://pyodide.org/) only).
2. **Session preparation** – the runtime validates the manifest or descriptor, applies resource policies (CPU, network, filesystem, host capabilities), mounts the bundle at `/app`, and primes the guest interpreter.
3. **Invocation** – watchdogs arm (wall + CPU), the language engine receives the invocation strategy (JSON or RawCtx), and user code runs inside the sandbox.
4. **Outcome synthesis** – stdout/stderr/console output, structured results, and sandbox telemetry are captured and surfaced via `ExecutionOutcome`.
5. **Reset** – the runtime either recreates the isolate (`ResetMode::RecreateEngine`) or performs an in-place scrub (`ResetMode::InPlace`). Pools recycle isolates or quarantine them if guard rails are exceeded.

Every transition records timings in diagnostics (`prepare_ms`, `queue_wait_ms`, `cleanup_ms`) so hosts can correlate spikes or guard-rail hits with the relevant phase.

## Isolation Mitigations

- **Process confinement** – Python and JavaScript share a [V8](https://v8.dev/) isolate with WebAssembly sandboxes. There is no native code execution from bundles.
- **Network policy** – allowlist + HTTPS guard. Denied calls raise `RuntimeError` and populate diagnostics with host/reason pairs.
- **Filesystem policy** – virtual FS hooks enforce read-only or read-write + quota modes, reject path escapes from `/session`, and wipe modified files on reset.
- **CPU budget** – thread-level CPU counters enforce `defaultLimitMs`; overruns mark the outcome as failure and quarantine pooled isolates.
- **Memory guard rails** – optional heap/RSS thresholds quarantine isolates and trigger snapshot rebuilds.
- **Host capabilities** – native bridges (`rawctx_buffers`, future capabilities) require explicit grant per session. Missing capability -> deterministic failure.
- **Crash containment** – WASM traps or fatal interpreter errors mark the isolate unhealthy; pooled runtimes drop the isolate and spin a replacement.

## Known Gaps

- **Single-process isolation** – there is no process sandbox today. A malicious guest that exploits the WASM/V8 embedding could compromise the host. Run in a container/VM if a stronger boundary is required.
- **DNS visibility** – policies operate on the requested host. DNS lookups themselves are not audited.
- **Streaming payloads** – strategies marshal payloads eagerly. Running very large RawCtx datasets still copies into host buffers.
- **Snapshot poisoning** – warm snapshots assume trusted preparation code. If the warm phase runs untrusted code, the snapshot may smuggle state.
- **Capability enumeration** – only `rawctx_buffers` exists. Additional host APIs must be paired with new capabilities before exposure.
- **Windows** – the runtime is untested on Windows; guard rails that rely on `thread_cpu_time_ns` and `/proc` are absent there.

Track these gaps in operational runbooks. Outcomes expose enough diagnostics to triage failures, but we expect follow-up hardening after the first external dogfooding cycle.

## Failure Signals

- **Policy violations** emit `FailureKind::PythonException` backed by sandbox errors (`RuntimeError`); diagnostics flag the denial.
- **Resource overruns** (`cpu_ms_used`, filesystem quota) switch `OutcomeStatus` to `Failure` with specific kinds.
- **Engine crashes** return `FailureKind::Other` with a reason, mark the isolate quarantined, and advise the host to recreate pools.
- **Manifest validation errors** surface before invocation (`PyRunnerError::Validation`); hosts should bubble these back to deployment tooling.

When in doubt, enable `tracing` for the `aardvark::runtime` and `aardvark::telemetry` spans to observe lifecycle events in real time.
