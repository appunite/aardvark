# Shared-Library Host Integration

Aardvark is designed to be embedded by hosts, but the current public API is a
Rust library API. A non-Rust host should load a Rust `cdylib` owned by that host
integration. That `cdylib` links `aardvark-core`, manages runtime state, and
exports the host-specific ABI.

This guide describes that boundary. It is not a promise that Aardvark already
ships a universal C ABI or ready-made bindings for every host runtime.

## Host Shapes

There are three useful integration shapes:

- **Rust application**: link `aardvark-core` directly and follow
  [`rust-host.md`](rust-host.md).
- **Rust shared object**: build a `cdylib` that links `aardvark-core`, exports a
  C-compatible ABI, and is loaded by another process or language runtime.
- **Host-specific native extension**: build the same Rust shared object, but use
  the native-extension conventions of the host runtime. Python native
  extensions, Ruby native extensions, JVM/JNI libraries, and application plugin
  loaders all fall into this class.

The important contract is the same in the second and third cases: Linux loads an
Aardvark-backed shared object, the shared object starts Pyodide inside V8, and
results cross back through the host boundary.

## Required Inputs

A shared-library host must provide:

- a built Rust shared object that links `aardvark-core`;
- a verified Aardvark Pyodide distribution path;
- bundle bytes or inline source to execute;
- invocation arguments in a format the shared object understands;
- a clear ownership contract for returned buffers, diagnostics, and errors.

For Python workloads that import packages such as `numpy` or `pandas`, use the
`full` Pyodide distribution variant. The `core` variant is only suitable for
workloads that do not need extra Pyodide packages.

The runtime does not download wheels on demand. A host should either set
`AARDVARK_PYODIDE_DIST_DIR` before loading the shared object or pass the
distribution path into the Rust side and configure `PyRuntimeConfig` /
`IsolateConfig` directly.

## Linux `rusty_v8` Archive

On x86_64 Linux, the upstream `rusty_v8` archive for `v8 = "=149.2.0"` does not
link correctly into shared objects. Direct Rust executables are not affected.
The workspace `Cargo.toml` `v8` dependency is the authoritative version pin; the
archive name below must track that pin.

When building a Linux shared object that links Aardvark, set:

```bash
curl -LO https://github.com/appunite/aardvark/releases/download/v149.2.0/librusty_v8_release_x86_64-unknown-linux-gnu-v149.2.0-shared.a
curl -LO https://github.com/appunite/aardvark/releases/download/v149.2.0/librusty_v8_release_x86_64-unknown-linux-gnu-v149.2.0-shared.a.sha256
sha256sum -c librusty_v8_release_x86_64-unknown-linux-gnu-v149.2.0-shared.a.sha256
export RUSTY_V8_ARCHIVE="$PWD/librusty_v8_release_x86_64-unknown-linux-gnu-v149.2.0-shared.a"
cargo build --release
```

Use an archive only for its matching `v8` crate version and target triple.
Maintainers can reproduce and verify the archive with
[`../dev/linux-v8-shared-archive.md`](../dev/linux-v8-shared-archive.md).

## Suggested ABI Shape

Aardvark does not currently prescribe a universal ABI. A host integration should
keep Rust-owned runtime state opaque and expose only simple boundary types.

A typical C-compatible shape is:

```c
typedef struct AardvarkHost AardvarkHost;

int aardvark_host_new(
    const char *pyodide_dist_dir,
    AardvarkHost **out_host,
    char **out_error
);

int aardvark_host_invoke(
    AardvarkHost *host,
    const uint8_t *bundle_bytes,
    size_t bundle_len,
    const uint8_t *input_bytes,
    size_t input_len,
    uint8_t **out_bytes,
    size_t *out_len,
    char **out_error
);

void aardvark_host_free_bytes(uint8_t *ptr, size_t len);
void aardvark_host_free_string(char *ptr);
void aardvark_host_free(AardvarkHost *host);
```

Keep these details explicit:

- who owns each pointer;
- which function frees returned memory;
- whether an `AardvarkHost` can be used concurrently;
- whether calls execute on the host thread or a Rust worker thread;
- how stdout, stderr, status, and diagnostics are returned;
- whether the host gets raw bytes, JSON, or a custom envelope.

For most hosts, the Rust side should own `PythonIsolate` or `BundlePool` state
and expose opaque handles. Do not pass Rust references or V8/Pyodide internals
across the ABI.

## Minimal Rust Boundary

The shared object should be a normal Rust crate:

```toml
[lib]
crate-type = ["cdylib"]

[dependencies]
aardvark-core = "0.1.1"
anyhow = "1"
```

Inside the crate, use the same runtime APIs as a direct Rust host:

```rust
use aardvark_core::{
    persistent::{BundleArtifact, BundleHandle, HandlerSession, PythonIsolate},
    IsolateConfig, PyRuntimeConfig, ResultPayload,
};

struct HostRuntime {
    isolate: PythonIsolate,
}

impl HostRuntime {
    fn new(pyodide_dist_dir: &str) -> anyhow::Result<Self> {
        let config = IsolateConfig {
            runtime: PyRuntimeConfig::default().with_pyodide_dist_dir(pyodide_dist_dir),
            ..Default::default()
        };
        Ok(Self {
            isolate: PythonIsolate::new(config)?,
        })
    }

    fn invoke_default(&mut self, bundle_bytes: &[u8]) -> anyhow::Result<Vec<u8>> {
        let artifact = BundleArtifact::from_bytes(bundle_bytes)?;
        let handle = BundleHandle::from_artifact(artifact);
        self.isolate.load_bundle(&handle)?;

        let handler: HandlerSession = handle.prepare_default_handler();
        let outcome = handler.invoke(&mut self.isolate)?;
        if !outcome.is_success() {
            anyhow::bail!(
                "aardvark invocation failed: status={:?}; stdout={}; stderr={}",
                outcome.status,
                outcome.diagnostics.stdout,
                outcome.diagnostics.stderr
            );
        }

        let bytes = match outcome.payload() {
            Some(ResultPayload::None) | None => Vec::new(),
            Some(ResultPayload::Text(text)) => text.as_bytes().to_vec(),
            Some(ResultPayload::Json(value)) => value.to_string().into_bytes(),
            Some(ResultPayload::Binary(bytes)) => bytes.clone(),
            Some(ResultPayload::SharedBuffers(_)) => {
                anyhow::bail!("shared-buffer payloads need a host-specific export path")
            }
        };
        Ok(bytes)
    }
}
```

The exact exported functions are host-specific. The important part is that the
host boundary stays outside `aardvark-core`; Aardvark supplies the runtime, not a
one-size-fits-all foreign-function interface.

## Verification

Before shipping a shared-library integration, verify all of the following on the
target OS and architecture:

- the shared object links with `RUSTY_V8_ARCHIVE` when required;
- the host loads the shared object through its actual loader;
- the shared object starts Aardvark and Pyodide;
- a bundle that imports `numpy` or `pandas` runs when the `full` distribution is
  configured;
- results and diagnostics cross the host boundary with the documented ownership
  rules;
- forbidden Linux TLS relocations are absent from the resulting shared object;
- runtime performance is checked against the upstream executable baseline when
  accepting a custom V8 archive.

The maintainer archive verifier covers the generic `cdylib`/`dlopen` case. It
does not replace the host-specific smoke for the integration you are actually
shipping.

## Current Limitations

- Aardvark does not yet publish a stable universal C ABI.
- Host-specific bindings are outside the current `aardvark-core` crate.
- Windows shared-library packaging is not supported or tested.
- Linux shared-object builds require the custom `rusty_v8` archive for the
  pinned `v8` version.
- API stability is not guaranteed while the runtime remains experimental.
