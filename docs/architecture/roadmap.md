# Roadmap & Targets

This document records the features under active consideration. Dates are intentionally omitted; the goal is to share intent with early adopters (Weasel team and beyond).

## Near-Term (pre-1.0 blockers)

- **Observability** – extend diagnostics with per-request timestamps, richer queue metrics, and tracing integration helpers for non-Rust hosts.
- **Hardening** – process isolation guidance (container profiles), automated crash/heap quarantine recovery, and fuzzer coverage for the JS/Python sandboxes.
- **Streaming payloads** – prototype incremental RawCtx readers/writers so large datasets avoid double-buffering between guest and host.
- **Manifest evolvability** – versioned schema negotiation plus clear guidance once [Pyodide](https://pyodide.org/) upgrades require schema changes.

## Mid-Term

- **JavaScript runtime parity** – Node-compatible module loader, bring-your-own module story spelled out in docs, and broader built-in coverage based on real workloads.
- **Async host adapters** – richer RawCtx adapters that expose async host functions with capability gates.
- **Per-request overrides** – ability to tweak network/FS budgets per invocation without rebuilding bundles.
- **Telemetry export** – optional gRPC/JSON exporter for outcomes so non-Rust hosts can subscribe without embedding the crate.

## Deferred / Experimental

- **Streaming ingest** – revisit raw context streaming once production feedback confirms the demand.
- **Multi-tenant isolation** – investigate multi-process orchestration or VM-per-tenant models if risk appetite requires it.
- **Alternate guest languages** – evaluate WebAssembly components for languages beyond Python/JavaScript after the current two stabilise.

Have an idea or pressing gap? Open an issue or add a note under `docs/dev/roadmap.md` so it can be triaged for the next planning cycle.
