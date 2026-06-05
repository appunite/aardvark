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
  "language": "python",
  "pyodide": {
    "version": "0.29.4",
    "profile": "blas",
    "preloadImports": ["main"]
  }
}
```

- `language` – Optional runtime override. Accepts `"python"` (default) or `"javascript"`.
- `pyodide.version` is optional and only valid when `language` is `"python"`. When present it must match the version bundled with the runtime.
- `pyodide.profile` is optional and only valid when `language` is `"python"`. It is a host-defined distribution profile such as `"default"` or `"blas"`. Hosts map profiles to staged Aardvark Pyodide distribution directories before constructing a warmed isolate or `BundlePool`.
- `pyodide.preloadImports` is optional and only valid when `language` is `"python"`. During `capture_warm_state()`, the runtime imports these modules immediately before the Pyodide snapshot is captured. Use this for pure Python app modules that are safe to restore from a Pyodide snapshot; do not use it for wasm-extension packages such as NumPy unless that exact bundle/distribution has been verified.

Distribution profiles are selected before Pyodide/V8 isolate creation. They are not a per-call switch: warm states, overlays, and package caches are fingerprint-bound to the selected distribution. Use separate warmed pools for materially different profiles.

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
    "pyodide": {"version": "0.29.4", "profile": "default"}
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
- `runtime.pyodide.profile` is trimmed, lowercased, and must contain only ASCII letters, digits, `_`, or `-`.
- `runtime.pyodide.preloadImports` entries are trimmed and deduplicated case-insensitively. Empty entries cause the manifest to be rejected.
- `quotaBytes` and `defaultLimitMs` must be positive when provided.
- Mixing [Pyodide](https://pyodide.org/) versions is not supported. Upgrade the runtime or regenerate bundles when [Pyodide](https://pyodide.org/) changes.
- When `runtime.language` is `"javascript"`, omit `runtime.pyodide` and leave `packages` empty.

## When to skip the manifest

- Infrastructure that already ships descriptors, packages, and limits through another channel can omit the manifest entirely.
- The runtime still expects the entrypoint to follow the `module:export` convention even without the manifest; hosts pass it via `InvocationDescriptor`.
- Bundles without a manifest cannot use manifest-only features such as built-in package installation or manifest-defined network policies.

## Known gaps

- Schema versioning is exact today: the runtime accepts `schemaVersion: "1.0"`.
  Future schema versions require explicit parser and validation changes.
- There is no manifest field for describing output payloads beyond the descriptor metadata; hosts should rely on descriptors for now.
