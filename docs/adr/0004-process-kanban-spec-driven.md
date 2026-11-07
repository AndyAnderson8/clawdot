# ADR-0004: Kanban with spec-driven development

- **Status:** Accepted
- **Date:** 2026-11-07
- **Deciders:** Maintainer

## Context

The project is initially run by one maintainer, with an intent to open-source and attract contributors. Work items vary widely in size. A hard external contract (satellite protocol, resource budget) must be unambiguous for third-party implementers.

## Options considered

1. **Scrum:** fixed sprints, estimation, ceremonies. Overhead outweighs value for a solo or small, irregular-capacity team.
2. **Kanban:** continuous flow, WIP limits, flow metrics. Low ceremony, fits variable capacity.
3. **Ad hoc:** no overhead, but no traceability or quality discipline.

## Decision

Use **Kanban** (GitHub Projects) for flow, combined with **spec-driven development**: accepted specs with stable requirement IDs precede implementation, and tests trace to those IDs. Details are in [docs/process/sdlc.md](../process/sdlc.md).

## Consequences

**Positive:** low ceremony, strong traceability, a credible contribution process, specs usable by porters.
**Negative:** spec-writing is upfront effort; discipline is required to keep specs and code in sync (mitigated by the Definition of Done and CI traceability reporting).
