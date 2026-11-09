# Documentation Index

| Area | Document | Purpose |
|---|---|---|
| Process | [sdlc.md](process/sdlc.md) | Kanban board, WIP limits, spec-driven workflow, DoR/DoD, releases |
| Process | [git-workflow.md](process/git-workflow.md) | Git flow, merge commits, gated landing, releases, enforcement layers |
| Process | [runbook.md](process/runbook.md) | Step-by-step commands for humans and agents: work, land, release, hotfix, fixes for common errors |
| Process | [repo-settings.md](process/repo-settings.md) | GitHub rulesets and settings that enforce the git flow server-side |
| Agents | [../AGENTS.md](../AGENTS.md) | Binding rules for AI coding agents (identity, branches, commits) |
| Specs | [specs/](specs/README.md) | What the system must do, with stable requirement IDs |
| Architecture | [architecture/overview.md](architecture/overview.md) | Components, flows, constraints, risks |
| Architecture | [architecture/extensibility.md](architecture/extensibility.md) | Core, packs, sidecars; media backends; trust model |
| Architecture | [architecture/configuration.md](architecture/configuration.md) | The configuration file: keys, defaults, validation |
| Architecture | [architecture/http-tools.md](architecture/http-tools.md) | Declarative HTTP tools: schema, safety rules, confirmation |
| Architecture | [architecture/mcp.md](architecture/mcp.md) | MCP client: pinned revision, safety rules, configuration |
| Architecture | [architecture/media.md](architecture/media.md) | Music and radio: backend shapes, built-in stations, control recipes |
| Architecture | [architecture/voice-pipeline.md](architecture/voice-pipeline.md) | Dialog state machine, endpointing, and the speech turn |
| Architecture | [architecture/satellite-server.md](architecture/satellite-server.md) | How speakers connect: handshake, a spoken turn, mute, limits |
| Architecture | [architecture/board-support.md](architecture/board-support.md) | Board layer: buttons, LED ring, mute, Wi-Fi, clock, audio names; the Echo Dot Gen 2 board file |
| Architecture | [architecture/porting-a-satellite.md](architecture/porting-a-satellite.md) | Implementing a satellite on new hardware; the reference client |
| Decisions | [adr/](adr/README.md) | Why we chose what we chose |
| Quality | [quality/testing-strategy.md](quality/testing-strategy.md) | Test levels, CI gates, review checklist |
| Quality | [quality/traceability.md](quality/traceability.md) | Requirement-to-test map and measured results |
| Security | [security/threat-model.md](security/threat-model.md) | Assets, threats, mitigations |
| Design | [design/brand.md](design/brand.md) | Logo, colour tokens, contrast, usage rules, trademark stance |
| Design | [design/ui-stack.md](design/ui-stack.md) | Proposed: raw JS, esbuild TypeScript, or Preact for the web UI |
| Research | [research/](research/README.md) | Dated findings from external sources, with spec impact |
| Process | [process/dev-environment.md](process/dev-environment.md) | Windows host plus WSL2 setup, toolchain, cross-compilation |
| Privacy | [../PRIVACY.md](../PRIVACY.md) | What data goes where; the no-telemetry promise |
| Planning | [roadmap.md](roadmap.md) | Milestones, feasibility gate, exit criteria, initial backlog |
| Reports | [reports/2026-11-07-overnight-session.md](reports/2026-11-07-overnight-session.md) | Overnight session: what was built, measurements, decisions, open items |
| Planning | [future-scope.md](future-scope.md) | Parked ideas with promotion conditions |

## Document precedence

If documents disagree: **Accepted spec > ADR > architecture overview > roadmap**. Fix the inconsistency in the same PR.

## Glossary

See [spec 0001 §3](specs/0001-product-requirements.md#3-definitions).
