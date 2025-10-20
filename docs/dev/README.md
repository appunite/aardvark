# Developer Guide

This directory collects everything you need to work on Aardvark itself. It is
organised so you can jump straight to the task at hand:

- [`environment.md`](environment.md) – toolchain setup, project structure, and
  local bootstrap steps.
- [`workflow.md`](workflow.md) – day-to-day development loop: building,
  formatting, linting, and running the test matrix.
- [`runtime-internals.md`](runtime-internals.md) – deep dive into the Rust ↔ JS
  bridge, [Pyodide](https://pyodide.org/) boot sequence, and sandbox shims.
- [`release.md`](release.md) – tagging, changelog hygiene, and crates.io publish
  checklist.

These documents mirror the public architecture notes but focus on *how* to make
changes safely, not just on what the runtime does.
