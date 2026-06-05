# Telemetry and Observability

Aardvark emphasises structured diagnostics so hosts can make admission and billing decisions without scraping logs.

## ExecutionOutcome

Every invocation returns an `ExecutionOutcome` containing:

- `status` – `Success` with a payload (text, JSON, binary, shared buffers) or `Failure` with a specific `FailureKind`.
- `diagnostics` – stdout, stderr, optional Python exception (`type`, `value`, `traceback`), plus sandbox metrics.

Hosts should persist the full diagnostics blob for debugging. For latency-sensitive workflows, prefer extracting telemetry summaries as described below.

```mermaid
sequenceDiagram
    participant Runtime
    participant Engine
    participant JS
    participant Python
    participant Host

    Runtime->>Engine: run_session()
    Engine->>JS: execute handler
    JS->>Python: call entrypoint
    Python-->>JS: stdout/stderr/result
    JS-->>Engine: ExecutionOutput + sandbox events
    Engine-->>Runtime: collected data
    Runtime-->>Host: ExecutionOutcome
    Runtime-->>Host: SandboxTelemetry
```

## SandboxTelemetry

`ExecutionOutcome::sandbox_telemetry()` produces a `SandboxTelemetry` struct:

- `queue_wait_ms` – Milliseconds spent waiting for a pooled isolate (if the call went through `BundlePool`).
- `queue_wait_p50_ms` / `queue_wait_p95_ms` – Rolling queue wait percentiles produced by the pool.
- `cpu_ms_used` – CPU milliseconds consumed by the Python thread (if available).
- `prepare_ms` / `cleanup_ms` – Host-visible timings for the runtime’s prepare and cleanup phases.
- `filesystem.bytes_written` – Bytes written under `/session` during the invocation.
- `filesystem.violations` – Any attempts that breached filesystem policy.
- `network.allowed` / `network.blocked` – Lists of contacted hosts and denied requests, including port, HTTPS flag, and reason codes.
- `py_heap_kib` – Python heap usage at the end of the invocation (KiB).
- `rss_kib_before` / `rss_kib_after` – Process RSS snapshots (Linux and macOS; other targets fall back to `None`).

The telemetry snapshot is cheap to clone and is intended for metrics pipelines (Prometheus, statsd, etc.).

```mermaid
flowchart LR
    Outcome[ExecutionOutcome] -->|to_telemetry| Telemetry[SandboxTelemetry]
    Telemetry --> CPU[cpu_ms_used]
    Telemetry --> FS[filesystem bytes + violations]
    Telemetry --> NetAllowed[network.allowed]
    Telemetry --> NetBlocked[network.blocked]
    Telemetry --> Alert{has_policy_violations?}
    Alert -- yes --> Warn[raise alert]
    Alert -- no --> Metrics[emit metrics only]
```

## Tracing

The runtime ships with tracing instrumentation (`tracing` crate):

- Runtime lifecycle: `runtime.new`, `runtime.pool.checkout`, `runtime.pool.return`, `runtime.reset`.
- Pool execution: `aardvark.call` spans annotate isolate id, bundle fingerprint, entrypoint, and queue wait duration for every `BundlePool::call_*` invocation. Persistent pools also emit `aardvark::telemetry` events at the interval configured by `PoolOptions::telemetry_interval` (queue depth, P50/P95 wait, isolate counts).
- Budgeting: `aardvark::budget` spans outline limits and enforcement results.
- Diagnostics: `aardvark::diagnostics` logs CPU usage, filesystem writes, and network decisions.

Subscribe via `tracing-subscriber` (or bridge into OpenTelemetry) to feed these spans into your observability stack. Enable `RUST_LOG=aardvark::telemetry=info` to witness the pool reporter in action; lower levels (`debug`/`trace`) surface the finer-grained diagnostics.

## FailureKinds

Failures differentiate between:

- `PythonException` – Python raised and was not caught.
- `AdapterError` – Invocation strategy failed before Python executed (e.g., decoder error).
- `TimeoutExceeded`, `CpuLimitExceeded`, `HeapLimitExceeded` – policy breaches.
- `Other` – unrecoverable runtime errors (JS engine failures, reset issues).

Hosts should treat adapter and other failures as infrastructure incidents; policy failures belong to the bundle author.

## Diagnostics Roadmap

The following telemetry gaps remain open:

- Heap telemetry is sampled at completion; peak usage during the call is still unknown. Capturing peaks remains on the backlog.
- Filesystem telemetry does not list the filenames written; only aggregate bytes and violation messages are provided.
- There is no streaming log channel; all stdout/stderr is buffered until completion.

Host SDKs should leave room to expose additional counters without breaking
existing consumers.
