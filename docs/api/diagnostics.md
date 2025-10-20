# Diagnostics and Error Handling

Hosts interact with invocations through the `ExecutionOutcome` struct. This guide shows how to interpret results and surface them to operators.

## Inspecting the status

```rust
match &outcome.status {
    aardvark_core::OutcomeStatus::Success(payload) => {
        tracing::info!(kind = payload.kind(), "python completed successfully");
    }
    aardvark_core::OutcomeStatus::Failure(kind) => match kind {
        aardvark_core::FailureKind::PythonException(exc) => {
            tracing::warn!(?exc, stderr = %outcome.diagnostics.stderr, "handler failed");
        }
        aardvark_core::FailureKind::TimeoutExceeded { requested_ms } => {
            tracing::error!(requested_ms, "wall-clock limit hit");
        }
        aardvark_core::FailureKind::CpuLimitExceeded { used_ms, .. } => {
            tracing::error!(used_ms, "cpu budget exceeded");
        }
        aardvark_core::FailureKind::HeapLimitExceeded { requested_mb } => {
            tracing::error!(requested_mb, "heap limit exceeded");
        }
        aardvark_core::FailureKind::AdapterError { message } => {
            tracing::error!(%message, "strategy or decoding failure");
        }
        aardvark_core::FailureKind::Other { message } => {
            tracing::error!(%message, "runtime internal error");
        }
    }
}
```

Treat adapter and other failures as infrastructure incidents. Policy violations (`TimeoutExceeded`, `CpuLimitExceeded`, `HeapLimitExceeded`) should be surfaced to bundle owners.

## Reading diagnostics

- `stdout`/`stderr` capture everything printed by Python; include them in logs.
- `exception` carries Python exception type/value/traceback when available.
- `cpu_ms_used`, `filesystem_bytes_written`, and the network/violation arrays provide the sandbox perspective.

## Telemetry helper

```rust
let telemetry = outcome.sandbox_telemetry();
if let Some(cpu_ms) = telemetry.cpu_ms_used {
    metrics::histogram!("aardvark.cpu_ms", cpu_ms as f64);
}
if let Some(wait_ms) = telemetry.queue_wait_ms {
    metrics::histogram!("aardvark.queue_wait_ms", wait_ms as f64);
}
if let Some(rss_after) = telemetry.memory.rss_kib_after {
    metrics::gauge!("aardvark.rss_kib", rss_after as f64);
}
for denied in telemetry.network.blocked.iter() {
    metrics::counter!("aardvark.network.denied", 1,
        "host" => denied.host.clone(),
        "https" => denied.https_required.to_string(),
        "reason" => denied.reason.clone());
}
```

`SandboxTelemetry::has_policy_violations()` is a quick guard for alerting. Pair it with `PoolTelemetry::from(&pool.stats())` if you want to publish aggregate queue metrics and guard-rail counters (total quarantines, heap-triggered quarantines, RSS-triggered quarantines, and scale-down events).

## Handling policy breaches gracefully

- Python handlers may catch the `RuntimeError` raised by the sandbox to provide fallback behaviour (e.g., skip a network call when denied).
- Even when caught, the runtime still records the violation, enabling operators to audit after the fact.

## Integrating with tracing

Subscribe to the `tracing` spans emitted by the runtime to gain more context:

```rust
tracing_subscriber::fmt()
    .with_env_filter("aardvark::runtime=info,aardvark::diagnostics=info")
    .init();
```

Tracing metadata includes `runtime_id`, entrypoint name, and the limits applied.

## Serialising outcomes

`ExecutionOutcome` implements `Serialize`/`Deserialize`. You can forward the entire struct over message queues or persist it as JSON:

```rust
let json = serde_json::to_string(&outcome)?;
let round_trip: aardvark_core::ExecutionOutcome = serde_json::from_str(&json)?;
```

When serialising, note that shared buffer payloads drop in-process data and only keep metadata (`length`, `id`, optional `metadata`). The consumer is expected to fetch the actual bytes via an out-of-band channel.

## Known gaps

- Sandboxed stdout/stderr are unbounded. Large outputs may increase memory pressure; consider enforcing limits at the descriptor level.
- Diagnostics do not yet include precise timestamps for when an invocation started/finished. Add wrapper-level timing if you need that today.
