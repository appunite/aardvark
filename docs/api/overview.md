# API Overview

Aardvark exposes a Rust-centric API for hosts and a manifest format for bundle authors. This directory maps the entry points you need most often:

- [`manifest.md`](manifest.md) – Bundle manifest schema, validation rules, and examples.
- [`rust-host.md`](rust-host.md) – Embedding the runtime in host applications, including pooling and invocation patterns.
- [`shared-library-host.md`](shared-library-host.md) – Host-agnostic guidance for embedding Aardvark through a Rust `cdylib`.
- [`python-handlers.md`](python-handlers.md) – Expectations for entrypoint functions and how descriptor metadata affects argument decoding.
- [`diagnostics.md`](diagnostics.md) – Reading `ExecutionOutcome`, telemetry, and handling policy failures.
- [`json-vs-rawctx.md`](json-vs-rawctx.md) – Trade-offs between the JSON and RawCtx strategies and when to pick each one.

The CLI (`aardvark-cli`) remains a development aid. Rust production hosts should
link `aardvark-core` directly. Non-Rust hosts should load a host-owned Rust
`cdylib` that links `aardvark-core`.

Looking for the bigger picture? Architecture notes live under `../architecture/` (start with `overview.md`, `lifecycle.md`, and `sandboxing.md`). Future-facing work is tracked in `../architecture/roadmap.md`.
