# Architecture Decision Records

An ADR captures one significant decision, its context, and its consequences, so future contributors understand *why*. Format follows [MADR](https://adr.github.io/madr/), shortened.

| ADR | Title | Status |
|---|---|---|
| [0001](0001-record-architecture-decisions.md) | Record architecture decisions | Accepted |
| [0002](0002-implementation-language-rust.md) | Implementation language: Rust | Proposed |
| [0003](0003-reference-hardware-budget.md) | Echo Dot Gen 2 as the reference resource budget | Accepted |
| [0004](0004-process-kanban-spec-driven.md) | Kanban with spec-driven development | Accepted |
| [0005](0005-satellite-core-separation.md) | Separate satellite and core via a protocol | Accepted |
| [0006](0006-inbox-transport.md) | Inbox transport: push-first with device-initiated fallback | Proposed |
| [0007](0007-license.md) | Project license: Apache-2.0 | Accepted |
| [0008](0008-wyoming-compatible-framing.md) | Wyoming-compatible framing for the satellite protocol (refines 0005) | Proposed |
| [0009](0009-project-name.md) | Project name: ClawDot | Accepted |
| [0010](0010-extension-model.md) | Extension model: no loadable code; MCP, HTTP tools, packs, sidecars | Proposed |
| [0011](0011-git-flow-merge-commits.md) | Git flow with merge commits and gated landing | Accepted |
| [0012](0012-first-run-admin-setup.md) | First-run administrator setup instead of a printed token | Proposed |

## Rules
- ADRs are immutable once Accepted. To change a decision, write a new ADR that **supersedes** it.
- Statuses: Proposed → Accepted → (Deprecated | Superseded by ADR-NNNN).
- Write an ADR when a decision is hard to reverse, affects multiple components, or involves a real trade-off.
- Start from [`0000-template.md`](0000-template.md).
