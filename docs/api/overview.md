# API Overview

Aardvark exposes a Rust-centric API for hosts and a manifest format for bundle authors. This directory maps the entry points you need most often:

- [`manifest.md`](manifest.md) – Bundle manifest schema, validation rules, and examples.
- [`rust-host.md`](rust-host.md) – Embedding the runtime in host applications, including pooling and invocation patterns.
- [`python-handlers.md`](python-handlers.md) – Expectations for entrypoint functions and how descriptor metadata affects argument decoding.
- [`diagnostics.md`](diagnostics.md) – Reading `ExecutionOutcome`, telemetry, and handling policy failures.

The CLI (`aardvark-cli`) remains a development aid. Production hosts should link `aardvark-core` directly.
