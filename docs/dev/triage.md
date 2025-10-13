# Issue Triage & Planning

This guide explains how we ingest bug reports, feature requests, and technical
investigations.

## Intake

- New issues land in the "Triage" column of the project board.
- Each issue must have at least:
  - Reproduction steps or failing test case (for bugs)
  - Clear problem statement or user story (for features)
  - Owner (person responsible for next action)

## Classification

1. **Severity**
   - `S1` – production outage or data risk
   - `S2` – broken core functionality or hard failure in supported workflow
   - `S3` – degraded experience or missing guardrails
   - `S4` – polish, tooling, docs-only issues

2. **Area**
   - Runtime (Rust, watchdogs, pooling)
   - JS sandbox (network/filesystem/capabilities)
   - Pyodide packaging & snapshots
   - Host API / diagnostics
   - Tooling & docs

3. **Type**
   - Bug
   - Enhancement
   - Investigation

## Grooming Checklist

- Ensure each issue has acceptance criteria.
- Link related docs (`docs/api`, `docs/architecture`, relevant `docs/dev`
  articles).
- For bugs, capture whether the issue reproduces on current `master`.
- For feature requests, note which sandbox guarantees or telemetry changes are
  required.

## Planning Cadence

- Weekly triage meeting: review new issues, assign owners, and set severity.
- Bi-weekly planning: slot `S1/S2` items into the current milestone and
  negotiate capacity for enhancements.
- Maintain a "Future Ideas" list when scope is unclear.

## Definition of Done

- Code merged with tests (unit, integration, or smoke as appropriate).
- Docs updated: user-facing (`docs/api`/`docs/architecture`) and developer
  references (`docs/dev`).
- Telemetry and diagnostics audited for regressions.
- Release notes entry drafted if the change is user-visible.

## Technical Debt Tracking

- Use labels like `tech-debt`, `refactor`, `cleanup`.
- Debt items should explain the risk (maintenance cost, performance, correctness).
- Revisit debt backlog monthly to prevent runaway build-up.
