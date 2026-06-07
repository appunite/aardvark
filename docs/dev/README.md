# Developer Guide

This directory collects everything you need to work on Aardvark itself. It is
organised so you can jump straight to the task at hand:

- [`environment.md`](environment.md) – toolchain setup, project structure, and
  local bootstrap steps.
- [`workflow.md`](workflow.md) – day-to-day development loop: building,
  formatting, linting, and running the test matrix.
- [`runtime-internals.md`](runtime-internals.md) – Rust ↔ JS bridge,
  [Pyodide](https://pyodide.org/) boot sequence, and sandbox shims.
- [`pyodide-implementation-status.md`](pyodide-implementation-status.md) –
  current Pyodide version, local compatibility evidence, and known non-claims.
- [`linux-v8-shared-archive.md`](linux-v8-shared-archive.md) – reproducible
  Linux `rusty_v8` archive builds for shared-library host packaging.

These documents cover contributor workflow and implementation details that do
not belong in the public API reference.
