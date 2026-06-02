# What This Runtime Actually Does

New to Aardvark? Start here. This doc gives you the mental model for how a bundle of Python or JavaScript ends up running safely inside your service.

## Big Picture

At runtime we embed [V8](https://v8.dev/) (Google’s JavaScript engine) inside a Rust library. [Pyodide](https://pyodide.org/) (a WebAssembly build of CPython) runs *inside* that V8 instance so Python code can execute without shipping a browser.

```mermaid
graph LR
    Host[Your Rust host
    application] -->|calls| Core["aardvark-core
    (Rust)"]
    Core -->|configures| V8[V8 Runtime]
    V8 -->|creates| Iso[V8 Isolate]
    Iso -->|loads| WASM["(Pyodide WASM + JS shims)"]
    WASM -->|boots| Py[CPython interpreter]
    Py -->|executes| Handler["Bundle entrypoint
    (Python or JS)"]
    Handler -->|stdout, payload,
    telemetry| Core
    Core --> Host
```

### Layers in plain English

1. **Host (you)** – Link `aardvark-core` and decide when to prepare/run bundles.
2. **Core runtime** – Rust code that builds sessions, enforces policies, and collects telemetry.
3. **[V8](https://v8.dev/)** – Acts as the sandbox shell; each isolate keeps guest state contained.
4. **[Pyodide](https://pyodide.org/) WASM + shims** – WebAssembly binary plus JavaScript glue that exposes filesystem/network guards to the interpreter.
5. **CPython** – The actual Python VM running inside Pyodide.
6. **Bundle entrypoint** – The function from your ZIP bundle (`module:function`) returning JSON, RawCtx buffers, or nothing.

## Lifecycle in 30 Seconds

```mermaid
sequenceDiagram
    participant Host
    participant Core as aardvark-core
    participant V8 as V8 Isolate
    participant WASM as Pyodide WASM
    participant Py as CPython

    Host->>Core: prepare_session(bundle)
    Core->>V8: ensure isolate + policies
    V8->>WASM: load Pyodide module
    WASM->>Py: import bundle, run setup
    Host->>Core: run_session(session)
    Core->>Py: call entrypoint via strategy
    Py-->>Core: result / exception
    Core-->>Host: ExecutionOutcome + diagnostics
    Core->>V8: reset or recycle isolate
```

## Why This Encapsulation Works

- **Single process, many guards** – Even though everything runs inside one process, [V8](https://v8.dev/) isolates, WebAssembly sandboxes, and our JS shims combine to keep guest code deterministic.
- **Warm isolation** – Pooling isolates means you reuse the heavy parts ([Pyodide](https://pyodide.org/) init, imports) without violating sandbox rules.
- **Policy enforcement** – Filesystem/network/hook checks live in the JS shims *and* are mirrored in Rust diagnostics so hosts can alert/kill misbehaving bundles.

## What You Need To Deploy It

- Ship bundles as ZIP files with an optional `aardvark.manifest.json`.
- Provide a staged Aardvark Pyodide distribution on disk when Python bundles need packages – the runtime never downloads wheels on the fly.
- Decide on invocation strategy: JSON (serde-friendly) or RawCtx (zero-copy buffers).
- Monitor `ExecutionOutcome.diagnostics` for policy hits; guard rails are surfaced there first.

When you’re ready for more depth, head back to `docs/architecture/overview.md` and `docs/architecture/lifecycle.md`.
