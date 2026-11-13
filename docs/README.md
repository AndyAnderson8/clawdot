# Documentation index

Start with the [top-level README](../README.md) for what ClawDot is and its status, then the [architecture overview](architecture/overview.md) for how it is built.

If documents disagree, the order is: accepted spec, then ADR, then architecture documents, then roadmap. Fix the disagreement in the same change. The glossary is in [spec 0001 section 3](specs/0001-product-requirements.md#3-definitions).

## Using ClawDot

| Document | What it is |
|---|---|
| [guides/quick-start.md](guides/quick-start.md) | Start the program on a computer, choose a model, hear it answer |
| [guides/local-speech.md](guides/local-speech.md) | Free local speech-to-text and text-to-speech servers |
| [guides/speech-providers.md](guides/speech-providers.md) | ElevenLabs or Deepgram instead of an OpenAI-style server |
| [guides/realtime-voice.md](guides/realtime-voice.md) | Optional speech-to-speech voice: build, cost, what is not yet proven |
| [guides/pc-voice-loop.md](guides/pc-voice-loop.md) | Manual push-to-talk test on a computer |
| [guides/announce.md](guides/announce.md) | Make the speaker say something from a script or CI (`POST /api/announce`, `clawdot say`) |
| [guides/hermes-mcp.md](guides/hermes-mcp.md) | Let an agent harness speak and ask you things through the MCP server |
| [guides/memory-and-skills.md](guides/memory-and-skills.md) | Notes, how they are selected, and importing skills |
| [guides/updates.md](guides/updates.md) | How updates are checked, verified and installed; how a release is signed |
| [guides/forgot-password.md](guides/forgot-password.md) | Recover the administrator password |
| [guides/hive.md](guides/hive.md) | Optional device identity and discovery of other speakers on the network |
| [recipes/README.md](recipes/README.md) | Ready configurations: [web search](recipes/web-search.md), [Spotify](recipes/spotify.md), [Home Assistant](recipes/home-assistant.md), [GitHub](recipes/github.md), [calendar](recipes/calendar.md), [Claude Code hook](recipes/claude-code-hook.md) |
| [../bridges/README.md](../bridges/README.md) | ntfy and Discord bridges that make the speaker say messages |
| [api/README.md](api/README.md) | The HTTP API as an OpenAPI description ([openapi.yaml](api/openapi.yaml)) |

## How it is built

| Document | What it is |
|---|---|
| [architecture/overview.md](architecture/overview.md) | Context, crates, the core's structure, where state lives, the resource budget |
| [architecture/agent-loop.md](architecture/agent-loop.md) | One multi-step turn: tasks, limits, speech, questions, cancel, memory |
| [architecture/action-safety.md](architecture/action-safety.md) | Risk tiers, host-held confirmation, untrusted turns, skill allow-lists |
| [architecture/configuration.md](architecture/configuration.md) | The configuration file, saved settings, the portal's first run |
| [architecture/voice-pipeline.md](architecture/voice-pipeline.md) | Dialog state machine, end of speech, the spoken turn |
| [architecture/speech-latency.md](architecture/speech-latency.md) | Time from the end of speech to the first sound: the reply played sentence by sentence as it is made (built), and speaking while the model is still writing (design) |
| [architecture/satellite-server.md](architecture/satellite-server.md) | How speakers connect to the core |
| [architecture/esphome-satellite.md](architecture/esphome-satellite.md) | Optional client for ESPHome voice devices |
| [architecture/extensibility.md](architecture/extensibility.md) | Tools, packs, sidecars, media backends, trust model |
| [architecture/http-tools.md](architecture/http-tools.md) | Declarative HTTP tools |
| [architecture/mcp.md](architecture/mcp.md) | The MCP client |
| [architecture/media.md](architecture/media.md) | Music and radio |
| [architecture/evals.md](architecture/evals.md) | The scenario evaluation harness |

## Devices

| Document | What it is |
|---|---|
| [architecture/porting-a-satellite.md](architecture/porting-a-satellite.md) | Implement a satellite on new hardware |
| [architecture/board-support.md](architecture/board-support.md) | Board files: buttons, lights, mute, audio names |
| [architecture/device-daemon.md](architecture/device-daemon.md) | The portable device daemon |
| [architecture/device-conformance.md](architecture/device-conformance.md) | The conformance suite a board must pass |
| [architecture/wake-word-runtime.md](architecture/wake-word-runtime.md) | The wake-word runtime and its measurements |
| [guides/dot-ssh.md](guides/dot-ssh.md) | Pushing builds to an Echo Dot over SSH (for developers) |
| [guides/b0-techo5-trial-plan.md](guides/b0-techo5-trial-plan.md) | Trial plan for a Linux base image on an Echo Dot (historical) |

## Specs, decisions, plans

| Document | What it is |
|---|---|
| [specs/](specs/README.md) | What the system must do, with stable requirement IDs: [product requirements](specs/0001-product-requirements.md), [resource budget](specs/0002-resource-budget.md), [satellite protocol](specs/0003-satellite-protocol.md), [ESPHome client](specs/0004-esphome-satellite-client.md), [device layer](specs/0006-device-layer-contract.md), [wake word](specs/0007-wake-word-runtime.md) |
| [adr/](adr/README.md) | Why decisions were made |
| [roadmap.md](roadmap.md) | Milestones, what is done and what is next |
| [future-scope.md](future-scope.md) | Parked ideas with the condition for promoting each |
| [open-questions.md](open-questions.md) | Questions for the maintainer, with the default chosen meanwhile |
| [design/agentic-scope.md](design/agentic-scope.md) | The design behind the agent loop (proposal, partly built) |
| [research/](research/README.md) | Dated findings from outside sources |
| [reports/](reports/) | Dated reviews: what was built, measured and proven ([latest review](reports/2026-11-11-architecture-review.md), [proving pass](reports/2026-11-11-proving-pass.md)) |

## Security and privacy

| Document | What it is |
|---|---|
| [security/threat-model.md](security/threat-model.md) | Assets, threats, mitigations |
| [../SECURITY.md](../SECURITY.md) | How to report a vulnerability |
| [../PRIVACY.md](../PRIVACY.md) | What is stored, what leaves the device, to whom |

## Process and quality

| Document | What it is |
|---|---|
| [../CONTRIBUTING.md](../CONTRIBUTING.md) | How to contribute |
| [process/dev-environment.md](process/dev-environment.md) | Toolchain, Windows with WSL, cross-compiling |
| [process/sdlc.md](process/sdlc.md) | Board, work limits, spec-driven workflow, definitions of ready and done |
| [process/git-workflow.md](process/git-workflow.md) | Branches, merge commits, gated landing, releases |
| [process/runbook.md](process/runbook.md) | Step-by-step commands and fixes for common errors |
| [process/repo-settings.md](process/repo-settings.md) | Server-side rules that enforce the git flow |
| [process/personal-data.md](process/personal-data.md) | What must not appear in files, and the guard |
| [../AGENTS.md](../AGENTS.md) | Binding rules for AI coding agents |
| [quality/testing-strategy.md](quality/testing-strategy.md) | Test levels, gates, review checklist |
| [quality/traceability.md](quality/traceability.md) | Requirement-to-test map |

## Design

| Document | What it is |
|---|---|
| [design/portal.md](design/portal.md) | The web portal: tabs, layout, principles |
| [design/portal-v2.md](design/portal-v2.md) | The reasoning, palette and wireframes behind the current portal |
| [design/ui-stack.md](design/ui-stack.md) | The UI stack proposal (partly superseded by ADR-0016) |
| [design/brand.md](design/brand.md) | Logo, colour tokens, usage rules |
