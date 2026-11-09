# Roadmap

Milestones are releasable increments with exit criteria, not dates. Order may shift as spikes inform risk. Requirement IDs refer to [spec 0001](specs/0001-product-requirements.md). Ideas outside v1.0 live in [future-scope.md](future-scope.md).

**v1.0 definition:** *Wake → ask → tools; timers, alarms, and reminders; an agent inbox with spoken replies; a tiny footprint within the Echo Dot Gen 2 budget by default; desktop and Raspberry Pi satellites.*

**Sequencing principle:** build the walking skeleton, then the differentiator (the agent inbox) before table-stakes features, while the budget-risk spikes de-risk the footprint promise.

## M0: Foundations and feasibility gate
**Goal:** a credible, enforceable skeleton, and proof that the footprint promise is achievable before we build on it.

Foundations
- [ ] Specs 0001 and 0002 reviewed by the maintainer and Accepted; 0003 completed after SPIKE-005
- [ ] ADRs 0002, 0006, 0008 decided (ADR-0007 licence and ADR-0009 name: done)
- [ ] `deny.toml` licence allowlist and `license = "Apache-2.0"` in manifests (TASK-002)
- [x] Repo: Cargo workspace, `rust-toolchain.toml`, `justfile`, `.editorconfig`, `.gitattributes`
- [ ] Dev environment validated ([dev-environment.md](process/dev-environment.md), TASK-001)
- [x] Git flow enforcement: hooks, agent guard, landing and release scripts, CI skeleton with the `gate` job ([ADR-0011](adr/0011-git-flow-merge-commits.md)). Rust, deny, cross-armv7, and budget jobs activate with the workspace (TASK-001..003).
- [ ] GitHub repository created; rulesets applied ([repo-settings.md](process/repo-settings.md)); signed commits once keys exist; Project board configured
- [x] SPIKE-001 (desk phase): specs sourced, no known unlock, see [R-04](research/04-echo-dot-gen2.md). Hands-on teardown is opportunistic.

Feasibility spikes (each ends with a written finding)
- [ ] SPIKE-003: TLS + HTTP client/server footprint on armv7 (x86_64 musl with rustls measured: idle 2.8 MiB; armv7 run still to do)
- [ ] SPIKE-002: wake-word runtime footprint on armv7
- [ ] SPIKE-006: audio capture and playback on armv7 Linux
- [ ] SPIKE-005: Wyoming-compatible framing with token `hello` and TLS; client code size
- [x] Baseline budget measurement: x86_64 musl 2.35 MiB binary, 3.8 MiB peak RSS under load, armv7 binary 1.89 MiB (armv7 memory and CPU unmeasured; see the [overnight report](reports/2026-11-07-overnight-session.md))

### Feasibility gate (go/no-go before M1)
Thresholds are provisional and set at the maintainer's review. Outcomes: **Go** (proceed), **Investigate** (adjust the approach, rerun), **Revise** (amend spec 0002 and/or ADR-0003 through an ADR *before* M1).

| Spike | Measurement (armv7, qemu-user, confirmed on real hardware where possible) | Go | Investigate | Revise |
|---|---|---|---|---|
| SPIKE-003 | Idle RSS of an empty async binary with rustls HTTPS client + HTTP server | ≤ 12 MB | 12-20 MB | > 20 MB |
| SPIKE-002 | Wake-word runtime: RSS delta; idle CPU as share of one core | ≤ 8 MB; ≤ 15% | up to 2× | beyond 2× |
| SPIKE-006 | Audio capture + playback at 16 kHz: RSS delta; underruns over 10 min | ≤ 4 MB; none | ≤ 8 MB; rare | > 8 MB or unstable |
| SPIKE-005 | Satellite client implementation size and RAM | ≤ 500 lines core logic; ≤ 1 MB | 2× | Fall back to WebSocket decision |

**Exit:** CI green on an empty-but-real workspace; budget gate running; specs Accepted; feasibility gate passed or budget formally revised.

## M1: Walking skeleton
**Goal:** end-to-end voice turn on desktop, in budget.
REQ-VOI-001..004, 006..010, 012, REQ-AST-001..003, 006, 007, 009, REQ-CFG-007, REQ-NFR-001/003/005/007, REQ-SEC-001..003, 006, 007
- Desktop satellite; in-process transport; remote transport
- Wake (existing licence-clean pretrained word), VAD, batch STT, streaming LLM, sentence-level TTS, half-duplex
- Config file and logging
- First budget measurements on armv7/qemu; **budget ratchet** (REQ-NFR-114) applied from the measured skeleton

**Exit:** AC-1 and AC-10 pass; budget gate green; ratchet set.

## M2: Agent inbox (the differentiator)
REQ-INB-001..018, 020, REQ-SEC-004, REQ-EXT-004, 006
- Minimal HTTP ingress, endpoints and tokens (config-file managed), queue, announce/dismiss/snooze/reply
- Format adapters (Claude Code native `http` hook first), filtering and priority, reference agent bridge
- SSE/poll sources; "what are my agents doing?" query
- Integration snippets (Claude Code hooks, shell, GitHub Actions); fuzzing of ingress

**Exit:** AC-3 and AC-6 pass; a desktop demo of "Claude finishes → speaker announces → voice reply resumes the session".

## M3: Time and local intelligence
REQ-TMR-001..008, REQ-AST-004, 010..013, REQ-NFR-008, 009, 111
- Scheduler with fake-clock test suite; atomic JSON store; restart-safety tests
- Local fast-path intent matcher; usage limits; persona config
- Tool-calling eval set and tested-models list; offline degraded mode

**Exit:** AC-2, AC-4, AC-7, AC-8, and AC-9 pass.

## M4: Web UI
REQ-CFG-001..006, 008, 009, 012, 013, REQ-INB-019, REQ-NFR-006
- Embedded static UI; **first-run setup**: choose a single admin password (hashed, light parameters) or deliberately leave the UI open on a private network, with an "unprotected" banner and a `--reset-admin` CLI recovery; webhook tokens stay required either way; no multi-user accounts; provider keys, voice and persona; endpoint management; history; test speak; health
- **Connect an agent** flow (generates per-harness snippets, token, bridge command); mDNS (`clawdot.local`); headless Wi-Fi docs for Pi

**Exit:** a new user can set up from the UI alone.

## M5: Tools and music
REQ-MUS-001..006, REQ-AST-005, 008, 014, REQ-CFG-010, REQ-EXT-005
- Media-source interface (playback and control shapes); internet radio / HTTP streams; ducking
- MCP client (Streamable HTTP); declarative HTTP tools; confirm policy
- Recipes: Spotify control, Home Assistant, Hermes (verified against their docs at this point)

## M6: Hardening and Raspberry Pi
REQ-NFR-004, 010, 108; opt-in REQ-VOI-005, REQ-VOI-011, REQ-NFR-002 tuning
- **Raspberry Pi satellite** (documented, supported)
- 30-day soak; budget optimisation and ratchet tightening; upgrade procedure and schema migrations
- Opt-in barge-in/AEC evaluation (SPIKE-004); latency tuning

**Exit:** real-hardware run recorded; Pi satellite passes the conformance suite.

## Beyond v1.0 (planned, in rough priority order)
Release early and often: tag `v0.x` releases as M2, M3, M4 land, so the differentiator ships long before v1.0. Items below are *planned for*, meaning v1.0 designs must not preclude them (see [future-scope.md](future-scope.md) for conditions).

| # | Item | Why it's next | Designed-for in v1.0 by |
|---|---|---|---|
| 1 | **Board support layer** (`crates/board`: a small `Board` trait for LED ring, buttons, mic-mute, Wi-Fi status/join and clock checks; `NullBoard` and desktop boards; per-device TOML for button and LED mappings) | Keeps the core device-agnostic while allowing real hardware features; one repo, no forks | Satellite protocol capabilities; config layering; optional Cargo features keep the default build in budget |
| 2 | **Echo Dot Gen 2 target** (`dot2` build profile and `scripts/build-dot2.sh`, a board implementation, a shipped `boards/echo-dot2.toml`, headless Wi-Fi and clock setup docs) | The primary device deserves a first-class build, not just a budget check | Board layer; SPIKE-006; hardware findings on `/sys/class/leds`, input devices and Wi-Fi tooling |
| 3 | **Agent connect kits** (Claude Code plugin bundling the hook and a skill; Codex, Gemini CLI, opencode adapters) in a separate `clawdot-agent-kit` repo | One-step setup is the adoption hook | Harness-agnostic webhook/SSE; pluggable bridge adapters (INB-020); the Connect-an-agent flow |
| 4 | **ClawDot MCP server** (`announce`, `ask_user`) so agents like Hermes can speak and ask you | Two-way agent interaction | Event `kind` and reply types (INB-003/017) |
| 5 | **Multiple assistant profiles** (e.g. Hermes vs a plain model, each with its own voice) chosen by voice routing, not extra wake words | Matches the "talk to Hermes / Claude / OpenAI" use case | Persona and voice config (AST-013); provider abstraction |
| 6 | **Installable packs** with a permission-review UI, plus an optional curated index | Community-driven integrations | Adapters already use the pack data format (EXT-002/004) |
| 7 | **Wi-Fi provisioner** (captive portal; BLE for ESP32) | Makes it an appliance | Portal is network-independent; mDNS |
| 8 | **Home Assistant HACS integration** and **Wyoming bridge** | HA community reach | Generic webhook, HTTP tools, Wyoming-compatible framing (ADR-0008) |
| 9 | **`/display` page** and semantic display events for screen speakers | Screens, kiosks | Display capability and `clawdot.display.*` reserved (SAT-016) |
| 10 | **Spotify** (control of a Connect device first; own-speaker receiver only if viable), podcasts and audiobooks (resume positions) | Music breadth | Media-source interface (MUS-003/005) |
| 11 | ESP32-S3 satellite; routines (e.g. morning briefing); long-term memory | Platform and feature breadth | Protocol; scheduler; bounded stores |

Hardware notes: the Echo Dot Gen 2 **hardware baseline** is normative from M1 onward (every CI run checks the default configuration against its resource envelope, spec 0002). The Dot **firmware port** is opportunistic only: no known unlock ([R-04](research/04-echo-dot-gen2.md)).
## v1.0 criteria
- All Must requirements Implemented and traced
- Default configuration green on the budget gate for armv7 under Dot-class limits (RAM, CPU, storage), plus a recorded run on the closest available real hardware (a 512 MB, quad-core, 32-bit userland board, or a Pi constrained to match)
- Desktop and Raspberry Pi satellites supported and conformance-tested
- Threat model reviewed; fuzzing in CI; signed releases with SBOM
- Docs: quick start, protocol guide for porters, agent integration guide, upgrade procedure, [PRIVACY.md](../PRIVACY.md)
- Licence and governance files in place

## Initial backlog (to be created as issues)

| Key | Type | Title |
|---|---|---|
| SPIKE-001 | spike | Hands-on Echo Dot Gen 2 baseline (opportunistic; desk phase done) |
| SPIKE-002 | spike | Wake-word runtime footprint on armv7 |
| SPIKE-003 | spike | TLS stack memory and connection strategy |
| SPIKE-004 | spike | Software AEC options and cost (opt-in barge-in) |
| SPIKE-005 | spike | Wyoming-compatible framing prototype with token hello and TLS |
| SPIKE-006 | spike | Audio capture and playback on armv7 Linux |
| TASK-001 | task | Cargo workspace skeleton, toolchain pin, dev environment validation |
| TASK-002 | task | CI pipeline (lint, test, deny, cross build) |
| TASK-003 | task | Budget gate tooling (qemu RSS harness) |
| TASK-004 | task | Complete satellite protocol spec and JSON Schemas |
| TASK-005 | task | Conformance harness (fake core/fake satellite) |
| TASK-006 | task | Traceability report tool |
| TASK-007 | task | GitHub Project board and labels |
| TASK-008 | task | Reference agent bridge for Claude Code (headless resume) |
| TASK-009 | task | Inbox format adapters and integration snippets |
| TASK-010 | task | Fast-path intent grammar and matcher |
| FEAT-001 | feature | Desktop satellite with in-process transport |
| FEAT-002 | feature | Streaming provider traits and OpenAI-compatible LLM client |
| EPIC-001 | epic | Wake-word model workstream: engine integration, eval set, licence provenance (custom training is post-1.0) |
| TASK-011 | task | Inbox adapter engine with declarative mappings; `claude-code` and `generic` adapters |
| TASK-012 | task | Reply stream (SSE) and reply types; spoken-summary pipeline with `summary_model` |
| TASK-013 | task | Media-source interface; HTTP stream backend; control-shape example |
| TASK-014 | task | Declarative HTTP tools and the Home Assistant / Hermes recipes |
| TASK-015 | task | "Connect an agent" flow and per-harness snippet generator |
| SPIKE-007 | spike | Verify Codex / Gemini CLI / opencode hook and resume mechanisms from primary docs |
| SPIKE-008 | spike | Verify Home Assistant conversation API / MCP server and Hermes gateway from primary docs |
