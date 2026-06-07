# JSON vs. RawCtx Payloads

Aardvark offers two primary invocation strategies. Both expose the same
sandboxing and watchdog behaviour, but they optimise for different trade-offs.
Use this guide to decide which path matches your workload.

## JSON Strategy

The JSON strategy (`JsonInvocationStrategy`) serialises inputs/outputs with
Serde on the host side and the standard library on the guest side for ordinary
payloads. Typed side-channel inputs are opt-in through `JsonInput` variants so
hot hosts can avoid rebuilding giant generic JSON values when the payload shape
is already known.

**When to choose JSON**

- Small payloads: numbers, strings, lists, maps, and ordinary request/response
  objects.
- Plain JSON request/response handlers that read `builtins.__aardvark_input`,
  or descriptor-bound handlers when positional arguments are required.
- Cross-language integrations where clients already speak JSON.
- Situations where readability and schema evolution matter more than raw
  throughput.

**Operational notes**

- Python JSON inputs are exposed as `builtins.__aardvark_input` for the duration
  of the call. They are not passed as positional handler arguments unless the
  host uses descriptor-bound inputs instead of the JSON strategy side channel.
- JSON inputs passed through `JsonInvocationStrategy::new` keep their JSON
  shape. Python receives strings as `str`, arrays as `list`, objects as `dict`,
  and scalars as their ordinary decoded Python values regardless of payload
  size.
- Bytes-like handler results are returned as `ResultPayload::SharedBuffers` with
  metadata (`format: "bytes"`).
- Hot hosts that already know the payload shape should use prepared typed inputs
  instead of rebuilding large `serde_json::Value`s on every call.
  `JsonInput::F32LeBytes` delivers a Python `memoryview` over little-endian
  `float32` bytes, `JsonInput::Utf8Bytes` delivers a Python `str`, and
  `JsonInput::Bytes` delivers Python `bytes`.
- Large numeric array results can also return as `ResultPayload::SharedBuffers`
  with typed metadata such as `format: "f32_le"`.
- Diagnostics still record sandbox telemetry (CPU, filesystem, network, reset),
  and hosts should release or consume shared-buffer JSON payloads the same way
  they handle RawCtx outputs.
- If a JSON handler is a hot data-plane function and printed output is not part
  of the contract, set `InvocationDescriptor::with_capture_stdio(false)` to
  skip stdout/stderr interception for that handler. Keep the default capture
  mode for scripts, debugging, and warning-heavy code.
- Larger payloads incur extra copies and UTF-8 validation; watch
  `ExecutionOutcome::diagnostics.cpu_ms_used` if you push multi-megabyte bodies.

## RawCtx Strategy

RawCtx is a structured binary channel that keeps buffers in shared memory and
lets descriptors describe the shape of the data. It can publish multiple
outputs, materialise columnar tables, and avoid JSON parsing entirely.

RawCtx has two usage levels:

- The descriptor auto-wrapper binds inputs/outputs into a normal Python
  function signature. This is easier to integrate and keeps schemas explicit.
- The direct contract skips the auto-wrapper. Python reads
  `builtins.__aardvark_rawctx_inputs` and publishes buffers with
  `builtins.__aardvark_publish_buffer(id, data, metadata)`. This is the lowest
  overhead path for bundles that already own the binary protocol.

**When to choose RawCtx**

- High-throughput analytics workloads (NumPy/Pandas tensors, Arrow-like
  records) where copying gigabytes of data would dominate request time.
- Handlers that need access to binary data (images, ML features) without
  round-tripping through base64.
- Multi-output scenarios (e.g., return both summary JSON and a buffer) where
  manifest-defined publishers keep everything deterministic.

**Operational notes**

- Descriptor-bound inputs surface in Python as `memoryview`s or decoded scalars
  depending on the binding. Direct RawCtx inputs surface under
  `builtins.__aardvark_rawctx_inputs` as `{name: {"data": memoryview,
  "metadata": ...}}`.
- For hot input buffers, construct `RawCtxInput` from owned bytes, for example
  `RawCtxInput::from_vec(...)`. If the host repeatedly clones shared `Bytes`,
  the runtime may have to copy into a V8 backing store instead of transferring
  the owned allocation.
- Direct RawCtx handlers should omit input metadata on hot buffers unless they
  actually read it. Descriptor-bound RawCtx uses metadata to drive schema
  binding, but direct handlers already own the binary protocol and can avoid
  per-call metadata serialisation when the metadata is redundant.
- Direct RawCtx handlers that never read input metadata can also opt into
  `InvocationDescriptor::with_rawctx_flat_input_buffers(true)`. This exposes
  `builtins.__aardvark_rawctx_inputs` as `{name: memoryview}` instead of
  `{name: {"data": memoryview, "metadata": ...}}`, removing the nested record
  allocation on the Python side. Do not enable it for handlers that expect the
  metadata-bearing record shape.
- Outputs become `ResultPayload::SharedBuffers` and carry metadata so the host
  knows how to interpret each buffer. Descriptor-bound handlers can use output
  metadata/publishers; direct handlers call
  `builtins.__aardvark_publish_buffer(...)` themselves.
- If a direct hot path has a fixed output protocol and the host already knows how
  to interpret buffer ids, disable output metadata materialization with
  `InvocationDescriptor::with_rawctx_output_metadata(false)`. This keeps the
  bytes/id path intact and returns `metadata: None`, avoiding per-call output
  metadata conversion work.
- Shared buffers require the host capability `rawctx_buffers`. Denying that
  capability hard-fails the invocation; the integration tests assert this
  behaviour (`javascript_rawctx_requires_capability`).
- Diagnostics capture the same lifecycle data as JSON plus the manifest-derived
  table metadata. Memory accounting includes the shared buffer sizes so you can
  alert on runaway payloads.
- Direct RawCtx hot handlers can also disable stdio capture with
  `InvocationDescriptor::with_capture_stdio(false)` when stdout/stderr are not
  host-observable data. This avoids per-call Python stream interception while
  preserving the same sandbox, capability gates, and shared-buffer result path.
- If a RawCtx handler publishes every successful response through shared
  buffers, opt into
  `InvocationDescriptor::with_rawctx_shared_buffer_only_success(true)`. The
  handler still executes in Pyodide/V8 and exceptions still return structured
  failure diagnostics, but successful calls skip the full JSON execution
  envelope. Do not enable this for handlers whose successful scalar/JSON return
  value is observable host data.

## Choosing a strategy

| Consideration        | JSON                              | RawCtx                                  |
|---------------------|-----------------------------------|-----------------------------------------|
| Ease of use         | High (serde/`json` module)        | Medium (descriptor metadata required)   |
| Copy behaviour      | Ordinary JSON for `Value`; explicit side channels for typed inputs | Zero-copy via shared buffers             |
| Data types          | Typical JSON structures; opt-in bytes/typed-array inputs and shared-buffer outputs | Binary blobs, columnar tables, typed scalars |
| Capability gates    | None beyond defaults              | Requires `rawctx_buffers`                |
| Tooling             | Works with any JSON client        | Best when host can consume shared buffers|

Start with JSON unless you know you are ingesting or emitting large binary
payloads, or you need the table/typed metadata features. Switching later is
straightforward: update the manifest (or descriptor) to include RawCtx bindings
and grant the host capability. The rest of the runtime—pooling, telemetry, and
policy enforcement—behaves identically.
