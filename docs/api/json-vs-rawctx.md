# JSON vs. RawCtx Payloads

Aardvark offers two primary invocation strategies. Both expose the same
sandboxing and watchdog behaviour, but they optimise for different trade-offs.
Use this guide to decide which path matches your workload.

## JSON Strategy

The JSON strategy (`JsonInvocationStrategy`) serialises inputs/outputs with
Serde on the host side and the standard library on the guest side.

**When to choose JSON**

- Rapid prototyping and simple payloads (numbers, strings, small objects).
- Handlers written to look like normal web functions (`def handler(event): ...`).
- Cross-language integrations where clients already speak JSON.
- Situations where readability and schema evolution matter more than raw
  throughput.

**Operational notes**

- JSON payloads land in `ResultPayload::Json` or `ResultPayload::Text`.
- Diagnostics still record sandbox telemetry (CPU, filesystem, network, reset),
  but there are no shared buffers to clean up.
- Larger payloads incur extra copies and UTF-8 validation; watch
  `ExecutionOutcome::diagnostics.cpu_ms_used` if you push multi-megabyte bodies.

## RawCtx Strategy

RawCtx is a structured binary channel that keeps buffers in shared memory and
lets descriptors describe the shape of the data. It can publish multiple
outputs, materialise columnar tables, and avoid JSON parsing entirely.

**When RawCtx shines**

- High-throughput analytics workloads (NumPy/Pandas tensors, Arrow-like
  records) where copying gigabytes of data would dominate request time.
- Handlers that need access to binary data (images, ML features) without
  round-tripping through base64.
- Multi-output scenarios (e.g., return both summary JSON and a buffer) where
  manifest-defined publishers keep everything deterministic.

**Operational notes**

- Inputs surface in Python as `memoryview`s or decoded scalars depending on the
  binding. Outputs become `ResultPayload::SharedBuffers` and carry metadata so
  the host knows how to interpret each buffer.
- Shared buffers require the host capability `rawctx_buffers`. Denying that
  capability hard-fails the invocation; the integration tests assert this
  behaviour (`javascript_rawctx_requires_capability`).
- Diagnostics capture the same lifecycle data as JSON plus the manifest-derived
  table metadata. Memory accounting includes the shared buffer sizes so you can
  alert on runaway payloads.

## Choosing a strategy

| Consideration        | JSON                              | RawCtx                                  |
|---------------------|-----------------------------------|-----------------------------------------|
| Ease of use         | High (serde/`json` module)        | Medium (descriptor metadata required)   |
| Copy behaviour      | Host ↔ guest copies each way      | Zero-copy via shared buffers             |
| Data types          | Text + typical JSON structures    | Binary blobs, columnar tables, typed scalars |
| Capability gates    | None beyond defaults              | Requires `rawctx_buffers`                |
| Tooling             | Works with any JSON client        | Best when host can consume shared buffers|

Start with JSON unless you know you are ingesting or emitting large binary
payloads, or you need the table/typed metadata features. Switching later is
straightforward: update the manifest (or descriptor) to include RawCtx bindings
and grant the host capability. The rest of the runtime—pooling, telemetry, and
policy enforcement—behaves identically.
