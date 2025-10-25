# Bundle Manifest

`aardvark.manifest.json` lives at the root of the bundle and must conform to schema version `1.0`.

## Top-level fields

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `schemaVersion` | string | yes | Must equal `"1.0"`. The runtime rejects other versions.
| `entrypoint` | string | yes | `module:export` pointing at the handler exported by the bundle.
| `packages` | array(string) | no | [Pyodide](https://pyodide.org/) packages to preload. Names are normalised (trimmed, case-insensitive dedupe).
| `runtime` | object | no | Runtime selection and language-specific constraints (language defaults to `python`).
| `resources` | object | no | Resource policies for CPU, network, filesystem, and host capabilities.

### `runtime`

```
"runtime": {
  "language": "javascript",
"pyodide": {"version": "0.29.0"}
}
```

- `language` – Optional runtime override. Accepts `"python"` (default) or `"javascript"`.
- `pyodide.version` is optional and only valid when `language` is `"python"`. When present it must match the version bundled with the runtime.

> JavaScript runtime support is available as a preview: bundles run with a read-only filesystem
> and manifest `packages` are ignored. Ship fully bundled JavaScript code; the runtime does not
> resolve npm dependencies.

### `resources`

```
"resources": {
  "cpu": {"defaultLimitMs": 5000},
  "network": {
    "allow": ["api.example.com", "*.internal.example.com"],
    "httpsOnly": true
  },
  "filesystem": {
    "mode": "readWrite",
    "quotaBytes": 1048576
  },
  "hostCapabilities": ["rawctx_buffers"]
}
```

- `cpu.defaultLimitMs` – Optional per-invocation CPU budget in milliseconds (>0).
- `network.allow` – Optional host allowlist. Entries may include `:<port>`; wildcards must start with `*.`. When absent, all network access is denied.
- `network.httpsOnly` – Defaults to `true`. Set to `false` to allow HTTP (only if you understand the risk).
- `filesystem.mode` – `"read"` or `"readWrite"`. Defaults to read-only.
- `filesystem.quotaBytes` – Optional positive integer limiting bytes written when writable.
- `hostCapabilities` – Optional list of capability strings. Entries are trimmed, deduplicated, and compared case-insensitively.

## Example manifest

```json
{
  "schemaVersion": "1.0",
  "entrypoint": "service:handler",
  "packages": ["numpy", "pandas"],
  "runtime": {
    "language": "python",
    "pyodide": {"version": "0.29.0"}
  },
  "resources": {
    "cpu": {"defaultLimitMs": 8000},
    "network": {
      "allow": ["api.data.example", "*.analytics.example"],
      "httpsOnly": true
    },
    "filesystem": {
      "mode": "readWrite",
      "quotaBytes": 5_000_000
    },
    "hostCapabilities": ["rawctx_buffers"]
  }
}
```

### JavaScript example

```json
{
  "schemaVersion": "1.0",
  "entrypoint": "handler:fetch",
  "runtime": {"language": "javascript"},
  "resources": {
    "network": {"allow": ["api.example"], "httpsOnly": true}
  }
}
```

> Ship JavaScript bundles that already include dependencies (via esbuild, webpack, etc.).
> The runtime does not resolve `node_modules` paths or download packages at invocation time.

## Validation rules

- Entrypoints must include both module and function names separated by `:`. Whitespace is trimmed automatically.
- Empty strings in `packages`, `network.allow`, or `hostCapabilities` cause the manifest to be rejected.
- `quotaBytes` and `defaultLimitMs` must be positive when provided.
- Mixing [Pyodide](https://pyodide.org/) versions is not supported. Upgrade the runtime or regenerate bundles when [Pyodide](https://pyodide.org/) changes.
- When `runtime.language` is `"javascript"`, omit `runtime.pyodide` and leave `packages` empty.

## When to skip the manifest

- Infrastructure that already ships descriptors, packages, and limits through another channel can omit the manifest entirely.
- The runtime still expects the entrypoint to follow the `module:export` convention even without the manifest; hosts pass it via `InvocationDescriptor`.
- Bundles without a manifest cannot use manifest-only features such as built-in package installation or manifest-defined network policies.

## Known gaps

- Schema versioning is manual. We plan to add explicit backwards-compatibility ranges when [Pyodide](https://pyodide.org/) upgrades demand changes.
- There is no manifest field for describing output payloads beyond the descriptor metadata; hosts should rely on descriptors for now.
