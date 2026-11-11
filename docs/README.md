# Documentation Index

| Area | Document | Purpose |
|---|---|---|
| Process | [sdlc.md](process/sdlc.md) | Kanban board, WIP limits, spec-driven workflow, DoR/DoD, releases |
| Process | [git-workflow.md](process/git-workflow.md) | Git flow, merge commits, gated landing, releases, enforcement layers |
| Process | [runbook.md](process/runbook.md) | Step-by-step commands for humans and agents: work, land, release, hotfix, fixes for common errors |
| Process | [repo-settings.md](process/repo-settings.md) | GitHub rulesets and settings that enforce the git flow server-side |
| Process | [personal-data.md](process/personal-data.md) | What must not appear in examples, and the hashed-denylist guard |
| Agents | [../AGENTS.md](../AGENTS.md) | Binding rules for AI coding agents (identity, branches, commits) |
| Specs | [specs/](specs/README.md) | What the system must do, with stable requirement IDs |
| API | [api/README.md](api/README.md) | The HTTP API as an OpenAPI 3.1 description ([openapi.yaml](api/openapi.yaml)): portal calls and webhook endpoints, with a curl session |
| Architecture | [architecture/overview.md](architecture/overview.md) | Components, flows, constraints, risks |
| Architecture | [architecture/extensibility.md](architecture/extensibility.md) | Core, packs, sidecars; media backends; trust model |
| Architecture | [architecture/configuration.md](architecture/configuration.md) | The configuration file: keys, defaults, validation |
| Architecture | [architecture/http-tools.md](architecture/http-tools.md) | Declarative HTTP tools: schema, safety rules, confirmation |
| Architecture | [architecture/mcp.md](architecture/mcp.md) | MCP client: pinned revision, safety rules, configuration |
| Architecture | [architecture/action-safety.md](architecture/action-safety.md) | How actions are held, tiered and scoped: host-held confirmation, risk tiers, untrusted turns, skill allow-lists |
| Architecture | [architecture/agent-loop.md](architecture/agent-loop.md) | One multi-step turn: limits, plan and progress speech, questions, cancel, memory and notes, long-result summaries, error recovery |
| Architecture | [architecture/evals.md](architecture/evals.md) | The scenario eval harness: format, replay gate, live mode with a spend cap, the shipped scenario set |
| Architecture | [architecture/media.md](architecture/media.md) | Music and radio: backend shapes, built-in stations, control recipes |
| Architecture | [architecture/voice-pipeline.md](architecture/voice-pipeline.md) | Dialog state machine, endpointing, and the speech turn |
| Architecture | [architecture/satellite-server.md](architecture/satellite-server.md) | How speakers connect: handshake, a spoken turn, mute, limits |
| Architecture | [architecture/board-support.md](architecture/board-support.md) | Board layer: buttons, LED ring, mute, Wi-Fi, clock, audio names; the Echo Dot Gen 2 board file |
| Specs | [specs/0006-device-layer-contract.md](specs/0006-device-layer-contract.md) | Device layer contract: REQ-DEV, capability model, minimum hardware, conformance; board-file schema in [../schemas/board/](../schemas/board/README.md) |
| Specs | [specs/0007-wake-word-runtime.md](specs/0007-wake-word-runtime.md) | Wake-word runtime: REQ-WAK, accepted operators, limits, agreement with the reference |
| Architecture | [architecture/wake-word-runtime.md](architecture/wake-word-runtime.md) | The wake-word runtime: parts, validation against the reference, measurements |
| Architecture | [architecture/device-daemon.md](architecture/device-daemon.md) | The portable device daemon: modules, what exists and what does not, running it, size |
| Architecture | [architecture/device-conformance.md](architecture/device-conformance.md) | The device conformance suite: cases, running it, how a new board proves itself |
| Architecture | [architecture/esphome-satellite.md](architecture/esphome-satellite.md) | Optional client for ESPHome voice satellites (a Dot base image, Voice PE): config, mapping, limits |
| Architecture | [architecture/porting-a-satellite.md](architecture/porting-a-satellite.md) | Implementing a satellite on new hardware; the reference client |
| Guides | [guides/pc-voice-loop.md](guides/pc-voice-loop.md) | Manual smoke test: a push-to-talk spoken turn on a PC (microphone in, speaker out) |
| Decisions | [adr/](adr/README.md) | Why we chose what we chose |
| Quality | [quality/testing-strategy.md](quality/testing-strategy.md) | Test levels, CI gates, review checklist |
| Quality | [quality/traceability.md](quality/traceability.md) | Requirement-to-test map and measured results |
| Security | [security/threat-model.md](security/threat-model.md) | Assets, threats, mitigations |
| Design | [design/brand.md](design/brand.md) | Logo, colour tokens, contrast, usage rules, trademark stance |
| Design | [design/portal.md](design/portal.md) | The tabbed web portal: palette, type, layout, tab names, principles |
| Design | [design/ui-stack.md](design/ui-stack.md) | Proposed: raw JS, esbuild TypeScript, or Preact for the web UI |
| Research | [research/](research/README.md) | Dated findings from external sources, with spec impact |
| Recipes | [recipes/README.md](recipes/README.md) | The default toolset as tested configurations: web search, Spotify (with its policy problem), Home Assistant, GitHub, calendar, Claude Code hook |
| Guides | [guides/announce.md](guides/announce.md) | Make the speaker say something from a script, Home Assistant or CI: `POST /api/announce` and `clawdot say`, with snippets and failure modes |
| Guides | [guides/hermes-mcp.md](guides/hermes-mcp.md) | Let an agent harness such as Hermes speak and ask you things: the ClawDot MCP server, `announce` and `ask_user` |
| Bridges | [../bridges/README.md](../bridges/README.md) | ntfy and Discord sidecars that make the speaker say messages, with filters, rate limits and no raw markup |
| Guides | [guides/quick-start.md](guides/quick-start.md) | Start ClawDot on a computer, choose a language model, hear it answer, and keep it updated |
| Guides | [guides/updates.md](guides/updates.md) | How ClawDot checks for, verifies and installs updates, and how the maintainer signs a release |
| Guides | [guides/realtime-voice.md](guides/realtime-voice.md) | Optional realtime (speech-to-speech) voice: how to build it in, what it costs, what stays in ClawDot, and what is not yet verified against the real service |
| Guides | [guides/local-speech.md](guides/local-speech.md) | Free local speech to text (whisper.cpp) and text to speech (Piper) with an OpenAI-compatible URL, for the PC voice loop |
| Guides | [guides/memory-and-skills.md](guides/memory-and-skills.md) | How memories are kept, selected for each request and searched, and how skills in the open SKILL.md format are imported, reviewed and kept from running anything |
| Guides | [guides/speech-providers.md](guides/speech-providers.md) | Choose ElevenLabs or Deepgram instead of an OpenAI-style speech server: setup, keys, cloned voices and their consent, and what is not yet verified against the real services |
| Guides | [guides/forgot-password.md](guides/forgot-password.md) | Forgot the administrator password: the 10 second button hold, the desktop key, the command line, and the Echo Dot |
| Guides | [guides/b0-techo5-trial-plan.md](guides/b0-techo5-trial-plan.md) | B0 trial of the TECHO5 Dot base image on the maintainer's unit: backups, install, measurements, rollback, go/no-go |
| Process | [process/dev-environment.md](process/dev-environment.md) | Windows host plus WSL2 setup, toolchain, cross-compilation |
| Privacy | [../PRIVACY.md](../PRIVACY.md) | What data goes where; the no-telemetry promise |
| Planning | [open-questions.md](open-questions.md) | Questions for the maintainer, each with the default chosen meanwhile |
| Planning | [roadmap.md](roadmap.md) | Milestones, feasibility gate, exit criteria, initial backlog |
| Reports | [reports/2026-11-07-overnight-session.md](reports/2026-11-07-overnight-session.md) | Overnight session: what was built, measurements, decisions, open items |
| Planning | [future-scope.md](future-scope.md) | Parked ideas with promotion conditions |

## Document precedence

If documents disagree: **Accepted spec > ADR > architecture overview > roadmap**. Fix the inconsistency in the same PR.

## Glossary

See [spec 0001 §3](specs/0001-product-requirements.md#3-definitions).
