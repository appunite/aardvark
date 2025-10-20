# Authoring Python Handlers

Bundle authors expose a handler via the manifest entrypoint (`module:function`). The runtime imports the module from `/app` and calls the function using the descriptor metadata.

## Function signature

- Default strategy calls `handler(*args)` where `args` are decoded in the order declared by the descriptor inputs.
- Keyword arguments are not currently supported; use metadata bindings (RawCtx) when you need named channels.
- Handlers may return `None`, strings, JSON-serialisable objects, `bytes`, or `memoryview` instances depending on the descriptor outputs.

## Decoding inputs

Descriptor `FieldDescriptor.metadata` can request built-in decoders:

```json
{
  "inputs": [
    {"name": "payload", "metadata": {"aardvark": {"decoder": "utf8"}}}
  ]
}
```

The default decoder passes raw bytes. Available decoders include `utf8`, `json`, numeric conversions, and RawCtx helpers for zero-copy buffers.

## Returning outputs

Outputs mirror inputs: metadata influences post-processing. If no decoder/transform is specified, returning native Python objects is fine:

- Return a string for `ResultPayload::Text`.
- Return a `dict`/`list` for `ResultPayload::Json`.
- Return `bytes` or `memoryview` for binary payloads.

When using RawCtx publishers, the runtime copies the buffer metadata into the outcome so hosts can interpret shared buffers consistently.

## Example handler

```python
# app/main.py
import json

# Expect descriptor input with decoder="json"
def handler(event):
    total = sum(item["value"] for item in event["records"])
    return {
        "count": len(event["records"]),
        "total": total,
    }
```

## Using RawCtx for advanced IO

RawCtx exposes structured columnar data and shared-memory buffers. Example descriptor snippet:

```json
{
  "inputs": [
    {
      "name": "table",
      "metadata": {
        "aardvark": {
          "rawctx": {
            "binding": {
              "arg": "table",
              "decoder": "json",
              "table": {
                "columns": [
                  {"name": "city", "dtype": "utf8"},
                  {"name": "temperature", "dtype": "float64"}
                ]
              }
            }
          }
        }
      }
    }
  ]
}
```

Python can access the derived metadata via the `meta` argument if the descriptor requests it. RawCtx is useful for high-volume data ingestion because it avoids per-row Python decoding.

For zero-copy outputs, allocate buffers via `builtins.__aardvark_output_buffer(size, *, id=None, metadata=None)`. The helper returns a `memoryview` backed by the runtime's `SharedArrayBuffer`, so filling it in-place and returning it avoids extra copies when `transform="memoryview"`.

## Error handling inside handlers

- Raise exceptions to signal failure. The runtime captures `type`, `value`, and traceback.
- For policy violations (network deny, filesystem quota) catch the `RuntimeError` thrown by the shim if you prefer to degrade gracefully.
- Print to stdout/stderr for debugging; both streams are collected and returned in diagnostics.

## Testing handlers locally

- Use the CLI to load your bundle: `cargo run -p aardvark-cli -- --bundle my_bundle.zip --manifest`.
- Provide `--descriptor` when testing descriptor-only bundles.
- For unit tests, load the same module under CPython and invoke the handler with representative payloads. Ensure any Pyodide-specific APIs are guarded behind runtime checks.

## Known gaps

- Streaming output is not supported; handlers must return a single payload.
- There is no built-in virtualenv simulation. The Pyodide environment may differ from CPython (especially around file IO and native extensions).
- Persistent filesystem state is wiped after each invocation in pooled runtimes; do not rely on local cache between runs.
