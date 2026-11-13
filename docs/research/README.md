# Research Notes

Short, dated notes that capture what we learned from external sources and what it means for the specs. They are **inputs to decisions, not decisions**: conclusions that change a contract become spec amendments or ADRs.

## Conventions
- Each note ends with *Spec impact* (recommended changes) and a *Confidence* line.
- Confidence: **high** = primary source read in full for the claim; **medium** = several consistent secondary sources; **low** = single source, vendor claim, or forum report.
- Notes are not edited after the fact except to add a dated correction. New findings go in new notes.

## Index

| Note | Topic | Feeds |
|---|---|---|
| [R-01](01-agent-integration.md) | Agent hooks, session resume, MCP | INB requirements, ADR-0006 |
| [R-02](02-providers.md) | OpenRouter, STT, TTS | VOI/AST requirements, NFR-002 |
| [R-03](03-wake-word.md) | openWakeWord vs microWakeWord, licensing | ADR-0002, SPIKE-002 |
| [R-04](04-echo-dot-gen2.md) | Echo Dot Gen 2 specs and unlock status | ADR-0003, spec 0002 |
| [R-05](05-wyoming-and-local-intents.md) | Wyoming protocol, hassil, local intents | ADR-0005, spec 0003 |
| [R-06](06-naming.md) | Name collision check | Public release |
| [R-07](07-echo-dot-gen2-hardware.md) | Echo Dot Gen 2 hands-on hardware findings (buttons, LED ring, audio, Wi-Fi, clock, services) | ADR-0012, spec 0001 §4.10, board support |
| [R-11](11-wake-word-spike.md) | SPIKE-002 PC finding: wake-word engines, licences, custom-word training, measurements, the engine seam | roadmap A3, ADR-0002, REQ-VOI-001/009 |
| [R-12](12-own-linux-layer.md) | ClawDot's own Linux layer for the Dot: inventory of the TECHO5 image, options, milestones, risks, install and update experience | roadmap B4b, ADR-0015, spec 0006 (REQ-DEV-032..037) |
| [R-08](08-existing-echo-dot-projects.md) | Prior art for the Dot: EchoMuse, TECHO5 Dot, NEO (hardware drive, ESPHome link, licences, Phase B recommendation) | roadmap Phase B, ADR-0012, spec 0003 |
| [R-12](12-agent-harness-brains.md) | Hermes Agent and opencode as an optional brain: how each is driven, three link designs, safety, loop improvements | REQ-EXT-006, REQ-INB-020, future-scope |
| [R-14](14-hive.md) | The hive: several speakers sharing memories, skills, settings and a spend cap with no server; discovery, pairing, trust, offline behaviour, who answers; options and milestones | roadmap item 20, future scope, a future `HIV` spec and ADR |

An earlier survey of two reference repositories and the wider landscape (make-a-smart-speaker, chatgpt-openai-smart-speaker, Home Assistant, OpenVoiceOS, Willow, Pipecat, LiveKit) was done in conversation; its conclusions are the eight "ideas to add" reflected below.

## Prior art

Projects that already put the Echo Dot to work, studied in [R-08](08-existing-echo-dot-projects.md) (all MIT-licensed; none is standalone):

- [EchoMuse](https://github.com/wilbowes/EchoMuse): own Linux userspace (emOS) or rooted Fire OS, on-device openWakeWord, a controller that impersonates ESPHome satellites for Home Assistant.
- [TECHO5 Dot](https://github.com/HuskerMinion/techo5-dot): Alpine Linux image with signed A/B updates; the Dot itself speaks the ESPHome native API.
- [NEO](https://github.com/techmiguel/neo-robot): ESP32-S3 assistant; useful only for ideas (streaming TTS, intents before the LLM).

Their link to a "brain" is the ESPHome native API, not Wyoming. Reuse rules (what may be copied, depended on, or only learned from) are in R-08 section 6. Unlock tooling (`amonet`, `firebreak`) is credited and linked, never redistributed.

## Consolidated spec-change proposals

| # | Change | Source | Target |
|---|---|---|---|
| 1 | Inbox format adapters (Claude Code native HTTP hook, Codex, Gemini, generic) | R-01 | REQ-INB-003/013 |
| 2 | Agent bridge as reply target; device talks only to a pre-configured bridge | R-01 | REQ-INB-008, threat T4 |
| 3 | Event filtering and priority mapping (`Stop` noise, needs-input > completed) | R-01 | new REQ-INB-014 |
| 4 | MCP client: Streamable HTTP only; pin spec revision | R-01 | REQ-AST-005 |
| 5 | `Stt` batch and streaming modes; OpenAI-style HTTP provider interface | R-02 | REQ-VOI-003, providers |
| 6 | Latency target conditional on STT mode; measure in M1 | R-02 | REQ-NFR-002 |
| 7 | Cost and usage caps | R-02, repo review | new REQ-AST-011 |
| 8 | Wake-word licence/provenance rule; microWakeWord as default candidate | R-03 | new REQ-VOI-009, ADR-0002 |
| 9 | Wake-word model workstream epic | R-03 | roadmap |
| 10 | Reference device table sourced; Dot port downgraded to opportunistic | R-04 | spec 0002 §2, roadmap M7 |
| 11 | Software AEC/beamforming as declared capability | R-04 | spec 0003, SPIKE-004 |
| 12 | Wyoming-compatible framing with extension layer (decide in M0) | R-05 | ADR-0005, spec 0003 |
| 13 | Local fast-path intents (bounded-time, hassil-style subset) | R-05, repo review | new REQ-AST-010 |
| 14 | Latency earcon / filler; adaptive end-of-turn detection | repo review | VOI requirements |
| 15 | Rename before public release | R-06 | README, repo |

Process: these are applied in a single `spec` change set, reviewed like code (see [SDLC §5](../process/sdlc.md#5-spec-driven-workflow)).

**Status (2026-11-07):** items 1-14 applied in spec 0001 v0.2, 0002 v0.2, 0003 v0.2, ADR-0006 (amended while Proposed), ADR-0008 (new), the threat model, testing strategy, and roadmap. Item 12 is Proposed pending SPIKE-005. Item 15 was decided by the maintainer: keep the name, risk accepted ([ADR-0009](../adr/0009-project-name.md)).
