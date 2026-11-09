# Roadmap

Milestones are releasable increments with exit criteria, not dates. Order may shift as spikes inform risk. Requirement IDs refer to [spec 0001](specs/0001-product-requirements.md). Ideas outside v1.0 live in [future-scope.md](future-scope.md).

**v1.0 definition:** *Wake → ask → tools; timers, alarms, and reminders; an agent inbox with spoken replies; a tiny footprint within the Echo Dot Gen 2 budget by default; desktop and Raspberry Pi satellites.*

**Sequencing principle:** build the walking skeleton, then the differentiator (the agent inbox) before table-stakes features, while the budget-risk spikes de-risk the footprint promise.

## M0: Foundations and feasibility gate
**Goal:** a credible, enforceable skeleton, and proof that the footprint promise is achievable before we build on it.

Foundations
- [ ] Specs 0001 and 0002 reviewed by the maintainer and Accepted; 0003 completed after SPIKE-005
- [ ] ADRs 0002, 0006, 0008 decided (ADR-0007 licence and ADR-0009 name: done)
- [x] `deny.toml` licence allowlist and `license = "Apache-2.0"` in manifests (TASK-002)
- [x] Repo: Cargo workspace, `rust-toolchain.toml`, `justfile`, `.editorconfig`, `.gitattributes`
- [ ] Dev environment validated ([dev-environment.md](process/dev-environment.md), TASK-001)
- [x] Git flow enforcement: hooks, agent guard, landing and release scripts, CI skeleton with the `gate` job ([ADR-0011](adr/0011-git-flow-merge-commits.md)). Rust, deny, cross-armv7, and budget jobs activate with the workspace (TASK-001..003).
- [ ] GitHub repository created; rulesets applied ([repo-settings.md](process/repo-settings.md)); signed commits once keys exist; Project board configured
  - *Status:* the repository exists with an `origin` remote and the CI `gate` job; `dev` was published through `scripts/land.sh`. Rulesets, signing and the Project board are not verified from the repo.
- [x] SPIKE-001 (desk phase): specs sourced, no known unlock, see [R-04](research/04-echo-dot-gen2.md). Hands-on teardown is opportunistic.

Feasibility spikes (each ends with a written finding)
- [ ] SPIKE-003: TLS + HTTP client/server footprint on armv7 (x86_64 musl with rustls measured: idle 2.8 MiB; armv7 run still to do; the full binary has since run on a real Echo Dot at about 1.4 MB idle RSS, see the baseline below, but that is not a spike finding)
- [ ] SPIKE-002: wake-word runtime footprint on armv7
- [ ] SPIKE-006: audio capture and playback on armv7 Linux
- [ ] SPIKE-005: Wyoming-compatible framing with token `hello` and TLS; client code size
- [x] Baseline budget measurement: x86_64 musl 2.35 MiB binary, 3.8 MiB peak RSS under load, armv7 binary 1.89 MiB (see the [overnight report](reports/2026-11-07-overnight-session.md))
  - *Status, full feature set (`scripts/budget.sh`, x86_64 musl):* binary 2.62 MiB, idle RSS 3.0 MiB, peak 4.3 MiB under load, armv7 binary 2.10 MiB. The ratchet limits have not been tightened yet.
  - *Status, real hardware (Echo Dot, maintainer's 2026-11 test; not yet written up in `docs/reports/`):* about 1.4 MB idle RSS, 12 MB virtual peak, 2.1 MB binary. The write-up is still to do.

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

*Status: partly built.* Done and tested against fakes: in-process and remote (TCP) transport, voice dialog state machine, energy-based endpointing, STT/TTS clients, streaming LLM client, half-duplex, JSON config file and the reference satellite (`crates/core/src/{voice,vad,speech,satellite,transport,config}.rs`, `crates/satellite`, [voice-pipeline](architecture/voice-pipeline.md)). Not done: real microphone and speaker I/O, wake-word detection, a real STT/TTS provider run, and the ratchet. AC-1 and AC-10 are not demonstrated on a device.

## M2: Agent inbox (the differentiator)
REQ-INB-001..018, 020, REQ-SEC-004, REQ-EXT-004, 006
- Minimal HTTP ingress, endpoints and tokens (config-file managed), queue, announce/dismiss/snooze/reply
- Format adapters (Claude Code native `http` hook first), filtering and priority, reference agent bridge
- SSE/poll sources; "what are my agents doing?" query
- Integration snippets (Claude Code hooks, shell, GitHub Actions); fuzzing of ingress

**Exit:** AC-3 and AC-6 pass; a desktop demo of "Claude finishes → speaker announces → voice reply resumes the session".

*Status: mostly built.* Ingress, endpoints with hashed tokens, queue, replies over SSE, adapters (`claude-code`, `generic`), summaries, the bridge that resumes Claude Code, and per-endpoint snippets exist (`crates/core/src/inbox/`, `crates/bridge`), with the headline loop tested over real sockets (`crates/e2e/tests/agent_loop.rs`, `satellite_loop.rs`). Not done: fuzzing of ingress (no fuzz targets in the repo), and the real `claude` CLI resume has not been verified. A live desktop demo is not recorded.

## M3: Time and local intelligence
REQ-TMR-001..008, REQ-AST-004, 010..013, REQ-NFR-008, 009, 111
- Scheduler with fake-clock test suite; atomic JSON store; restart-safety tests
- Local fast-path intent matcher; usage limits; persona config
- Tool-calling eval set and tested-models list; offline degraded mode

**Exit:** AC-2, AC-4, AC-7, AC-8, and AC-9 pass.

*Status: partly built.* Scheduler (timers, alarms, reminders, DST-aware), atomic versioned JSON store, fast-path matcher, usage limits and a clock-unset guard exist and are tested (`crates/core/src/{sched,store,fastpath,usage}.rs`; commits 9ce871d, db8655f). Not verified or not done: the tool-calling eval set and tested-models list, offline degraded mode and persona config as specified, and the AC pass/fail status.

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

*Status: partly built.* Internet radio by voice with ducking, the media-source model, the MCP client over Streamable HTTP and declarative HTTP tools with confirmation exist (`crates/core/src/{media,mcp,httptools}.rs`; [media](architecture/media.md), [mcp](architecture/mcp.md), [http-tools](architecture/http-tools.md)). Tested only against fake servers and players. Not done: the Spotify, Home Assistant and Hermes recipes (untested, sources secondary).

## M6: Hardening and Raspberry Pi
REQ-NFR-004, 010, 108; opt-in REQ-VOI-005, REQ-VOI-011, REQ-NFR-002 tuning
- **Raspberry Pi satellite** (documented, supported)
- 30-day soak; budget optimisation and ratchet tightening; upgrade procedure and schema migrations
- Opt-in barge-in/AEC evaluation (SPIKE-004); latency tuning

**Exit:** real-hardware run recorded; Pi satellite passes the conformance suite.

*Status: not started* (no soak, Pi satellite or conformance suite). A first real-hardware run on an Echo Dot exists (see the M0 baseline) but is not yet recorded in `docs/reports/`.

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

*Status:* none of these is fully met. Licence (`LICENSE`, `NOTICE`) and `PRIVACY.md` exist; the budget gate runs as a script, not yet as a required CI job, and the first real-hardware measurement is not yet recorded as a run; fuzzing, signed releases with SBOM, the Raspberry Pi satellite and the quick start are not done. `v0.1.0` is prepared (`CHANGELOG.md`) but no tag has been published.

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

**Backlog status (as of 2026-11-08).** Built, with tests: TASK-008 (`crates/bridge`), TASK-010 (`fastpath.rs`), TASK-011 and TASK-012 (`inbox/adapter.rs`, `inbox/reply.rs`, `inbox/summary.rs`), TASK-013 (`media.rs`), FEAT-001 (in-process transport, `crates/satellite`), FEAT-002 (`llm.rs`). Partly built: TASK-001 (workspace exists; dev environment validation open), TASK-002 (CI `gate`, Rust and deny jobs run; cross-armv7 still a TODO in `ci.yml`), TASK-003 (`scripts/budget.sh` measures size and RSS; no qemu harness, not a CI gate), TASK-004 (spec 0003 is a skeleton; typed messages in `crates/protocol`), TASK-009 and TASK-015 (snippets are generated per endpoint; the full Connect-an-agent UI flow is not), TASK-014 (HTTP tools built; Home Assistant and Hermes recipes untested). Not found in the repo: SPIKE-002..008 findings, EPIC-001, TASK-005 (conformance harness), TASK-006 (traceability tool), TASK-007 (board and labels not verifiable). SPIKE-001 desk phase is done.
