# Roadmap

Steps are releasable increments with exit criteria, not dates. Order may shift as spikes inform risk. Requirement IDs refer to [spec 0001](specs/0001-product-requirements.md). Ideas outside v1.0 live in [future-scope.md](future-scope.md).

**v1.0 definition:** *Wake → ask → tools; timers, alarms, and reminders; an agent inbox with spoken replies; a tiny footprint within the Echo Dot Gen 2 budget by default; desktop and Raspberry Pi satellites.*

**Status as of 2026-11-09** (checked against `origin/dev` at `7fcc110`; the per-step notes below carry the evidence). "Proven" means checked by automated tests and gates that run in CI. Nothing here is proven on a real microphone, a real speaker, a real language-model session or a real Echo Dot beyond the one idle measurement noted in A0.

- **Built and proven (by tests and gates, no live dependency):** faster commit checks, the integration-branch workflow and `land.sh` hardening; action safety (host-held confirmation, risk tiers, the event-turn rule, taint, skill allow-lists, spoken read-back redaction, audit log, daily caps, dry-run); the eval harness in replay mode as a `check.sh` gate (81 scenarios, 24 of them injection); the agent loop's logic (limits, plan, progress, `ask_user`, cancel, notes, summaries, errors); the TypeScript and Preact portal (10 tabs, editable settings, activity, notes, tiers, diagnostics); the announce API, `clawdot say`, the stopwatch, and the ntfy and Discord bridges; device contract spec 0006, the board schema, the `clawdot-device` daemon and the conformance suite (PC side only); the wake-word runtime numerically against its reference implementation on synthetic golden clips.
- **Built but unproven live (works against fakes, recordings or a scripted engine; never on real audio, a real model session or a device):** the audio interface, command backends and push-to-talk; the microWakeWord runtime (`crates/wake`) and its device adapter; the PC voice loop with always-on capture and local whisper and Piper (rehearsed on 2026-11-09 with a recording as the microphone, never with a real microphone); the spoken half of the agent loop and of the inbox (the real `claude` resume is unverified); eval live mode (fake provider only); the ESPHome client (tested only against a fake device); the recipes (written and used as eval fixtures; none run against its real server; Spotify is blocked by the provider's policy); radio, MCP and HTTP tools (fake servers and players only).
- **Not started:** the B0 trial (a plan only, [guide](guides/b0-techo5-trial-plan.md); nothing has been run), B3 to B7 on a device, the Pi run, rules ("do X when Y"), background tasks with ids, the armv7 spike findings (SPIKE-002, 003, 006 on armv7), the budget ratchet and CI budget job, fuzzing, signed releases with SBOM, the 30-day soak, the quick start and upgrade guide, and offline degraded mode.

- **Since then (2026-11-10, [report](reports/2026-11-10-portal-v3-updates-dot.md)):** the portal was redesigned (sidebar, Home numbers and chart, Clock, Bluetooth placeholder, Features, one card pattern; 34 browser flows pass locally), the recovery code was removed, updates were built (daily and on-demand check, confirmed signed install; tested against fakes only, no release or key exists yet), and the new build runs on the Dot, which came back by itself after a reboot (31 seconds). Not seen: a spoken answer from the new build, CI on the portal batch, a real signed update.

## Not doing

Stated up front so the plan stays small. Each of these is either out of scope for v1.0 or deliberately declined; ideas that might return are in [future-scope.md](future-scope.md).

- **No OS or audio engine for third-party boards.** We ship one portable device daemon, a board file and an image recipe per device ([ADR-0015](adr/0015-own-the-device-layer.md)); we do not build a general operating system or audio stack for other people's hardware.
- **No smart-home hub.** Home Assistant and similar systems are optional tools the agent can call, not something ClawDot becomes or depends on.
- **No local speech models on small devices.** Speech to text and text to speech run off-device (or on a PC); the Dot-class budget does not hold them.
- **No ESP32 brain.** An ESP32-class chip can be a satellite later; the agent loop runs on a PC, a Pi or a Dot.
- **No multiroom audio, calls, cameras or screens** in v1.0. Screen devices and Zigbee are planned after it (see Beyond v1.0, items 8, 11 and 12).

## Principles

1. **The end goal is the Echo Dot Gen 2** (the primary device), but the core stays hardware-agnostic behind the board layer ([ADR-0012](adr/0012-board-support-layer.md)). Device paths, key codes and driver names never enter the core or the reference satellites.
2. **Prove everything on a PC first.** Each capability works end to end on a desktop before any device work starts, so a device problem is never confused with a logic problem.
3. **Agentic workflows are the product:** plan, act with tools, report back, under a safety model. The agent inbox (spoken announcements of what your coding agents did, and spoken replies back) is one input among several. Commodity audio parts (wake word, speech to text, text to speech, music control) are reused, not rebuilt. See [agentic-scope.md](design/agentic-scope.md) (Proposed; the spec amendment it lists is not yet applied).
4. **The resource budget is a feature** ([spec 0002](specs/0002-resource-budget.md)). The default configuration must fit Echo Dot Gen 2 class limits; every new dependency is justified by its size and memory cost.

**Honesty note:** no real microphone and no real speaker has run ClawDot yet. Everything voice-related below is tested against fakes, scripted engines and recordings. The closest run is the 2026-11-09 rehearsal on the maintainer's PC, where a recording played as the microphone and the real wake engine, core, whisper.cpp and Piper handled it ([pc-voice-loop.md](guides/pc-voice-loop.md)); the language model there was a free stand-in. The one real-hardware result so far is that the binary starts and idles on an Echo Dot (see the baseline in A0).

## Structure and mapping from the old milestones

Work is now in three phases: **A** (PC loop, hardware-agnostic core), **B** (board layer and the Echo Dot Gen 2), **C** (hardening and v1.0). Other documents (specs, research, reports, ADRs) still say M0 to M6; this table maps the old numbers to the new steps.

| Old | New | Note |
|---|---|---|
| M0 Foundations and feasibility gate | A0 (below); SPIKE-002 and SPIKE-006 now run inside A3 and A2 | The gate table stays here; SPIKE-003 and SPIKE-005 findings remain open in A0 |
| M1 Walking skeleton | A2 + A3 + A4 | Exit AC-1, AC-10 is A4's exit; the budget ratchet moves to C |
| M2 Agent inbox | A5 (voice loop); the inbox itself is built and tested with fakes | Exit AC-3, AC-6; ingress fuzzing moves to C |
| M3 Time and local intelligence | A7 (offline mode, persona); scheduler is built; the eval set moved to A1c and is built | AC-2, AC-4, AC-7, AC-8, AC-9 are checked in A7 |
| M4 Web UI | A6 | Device auto-detection moves to B1 |
| M5 Tools and music | A7 (radio, announce, bridges); recipes in A5b | Radio and MCP/HTTP tools are built (fakes only); recipes are written, not run against real servers |
| M6 Hardening and Raspberry Pi | C; the Pi item is B7 | |
| (new) | A1 process and safety, A1b action safety, A1c eval harness, A3b wake-word runtime (inside A3), A5b agent loop; B0 to B8 device layer and Echo Dot Gen 2 | A1b, A1c, A5b follow the reordering proposed in [agentic-scope.md](design/agentic-scope.md) section 7 |

Release mapping in [git-workflow.md](process/git-workflow.md#7-versioning-and-tags) uses the old milestone names; read it through this table.

## A0. Foundations and feasibility gate (was M0)
**Goal:** a credible, enforceable skeleton, and proof that the footprint promise is achievable before we build on it.

Foundations
- [ ] Specs 0001 and 0002 reviewed by the maintainer and Accepted; 0003 completed after SPIKE-005
- [ ] ADRs 0002, 0006, 0008 decided (ADR-0007 licence and ADR-0009 name: done; ADR-0003 Accepted). Still Proposed (checked in `docs/adr/` on 2026-11-09, along with ADR-0002, 0006 and 0008): ADR-0012 (board layer), ADR-0013 (first-run admin setup, implemented), ADR-0014 (Rust core, polyglot edges), ADR-0015 (own the device layer, gated on B3) and ADR-0016 (TypeScript portal stack, implemented). All six specs under `docs/specs/` are still Draft
- [x] `deny.toml` licence allowlist and `license = "Apache-2.0"` in manifests (TASK-002)
- [x] Repo: Cargo workspace, `rust-toolchain.toml`, `justfile`, `.editorconfig`, `.gitattributes`
- [ ] Dev environment validated ([dev-environment.md](process/dev-environment.md), TASK-001)
  - *Status:* the WSL2 setup built and ran the project on the maintainer's PC (the PC voice loop rehearsal, 2026-11-09), but no validation record is written, so this stays open.
- [x] Git flow enforcement: hooks, agent guard, landing and release scripts, CI skeleton with the `gate` job ([ADR-0011](adr/0011-git-flow-merge-commits.md)). Since then: integration branches (`scripts/new-integration.sh`, `land.sh --into` and `--update`; merged as `chore/integration-branches`, `49b6d6b`; [git-workflow.md](process/git-workflow.md) section 5a) and `land.sh` robustness fixes (`chore/land-sh-robustness`, `43bdb5c`; the `integration/recipes-and-portal` batch landed as `7fcc110`). The Rust, deny and portal jobs run in `ci.yml`; the cross-armv7 and budget jobs are still a `TODO(TASK-003)` there.
- [ ] GitHub repository created; rulesets applied ([repo-settings.md](process/repo-settings.md)); signed commits once keys exist; Project board configured
  - *Status:* the repository exists with an `origin` remote and the CI `gate` job; `dev` was published through `scripts/land.sh`. Rulesets, signing and the Project board are not verified from the repo.
- [x] SPIKE-001 (desk phase): specs sourced, see [R-04](research/04-echo-dot-gen2.md). The hands-on part has since happened: [R-07](research/07-echo-dot-gen2-hardware.md) records an adb-root Dot.

Feasibility spikes (each ends with a written finding)
- [ ] SPIKE-003: TLS + HTTP client/server footprint on armv7 (x86_64 musl with rustls measured: idle 2.8 MiB; armv7 run still to do; the full binary has since run on a real Echo Dot at about 1.4 MB idle RSS, see the baseline below, but that is not a spike finding)
- [ ] SPIKE-002: wake-word runtime footprint on armv7. The PC part is written up ([R-11](research/11-wake-word-spike.md): Go for the approach, one open item) and measured in [wake-word-runtime.md](architecture/wake-word-runtime.md); the armv7 and device figures are not taken, so this stays open
- [ ] SPIKE-006: audio capture and playback on armv7 Linux (PC in A2 first, device in B3, through the portable daemon)
- [ ] SPIKE-005: Wyoming-compatible framing with token `hello` and TLS; client code size. The frame JSON Schemas and the conformance harness now exist (TASK-004, TASK-005, below), which narrows what is left to the size and TLS measurements
- [x] Baseline budget measurement: x86_64 musl 2.35 MiB binary, 3.8 MiB peak RSS under load, armv7 binary 1.89 MiB (see the [overnight report](reports/2026-11-07-overnight-session.md))
  - *Status, full feature set (`scripts/budget.sh`, x86_64 musl):* binary 2.62 MiB, idle RSS 3.0 MiB, peak 4.3 MiB under load, armv7 binary 2.10 MiB. The ratchet limits have not been tightened yet.
  - *Status, real hardware (Echo Dot, maintainer's 2026-11 test; not yet written up in `docs/reports/`):* about 1.4 MB idle RSS, 12 MB virtual peak, 2.1 MB binary. The write-up is still to do. It shows the process starts and idles; it says nothing about audio.

### Feasibility gate (go/no-go before the device phase)
Thresholds are provisional and set at the maintainer's review. Outcomes: **Go** (proceed), **Investigate** (adjust the approach, rerun), **Revise** (amend spec 0002 and/or ADR-0003 through an ADR *before* Phase B).

| Spike | Measurement (armv7, qemu-user, confirmed on real hardware where possible) | Go | Investigate | Revise |
|---|---|---|---|---|
| SPIKE-003 | Idle RSS of an empty async binary with rustls HTTPS client + HTTP server | ≤ 12 MB | 12-20 MB | > 20 MB |
| SPIKE-002 | Wake-word runtime: RSS delta; idle CPU as share of one core | ≤ 8 MB; ≤ 15% | up to 2× | beyond 2× |
| SPIKE-006 | Audio capture + playback at 16 kHz: RSS delta; underruns over 10 min | ≤ 4 MB; none | ≤ 8 MB; rare | > 8 MB or unstable |
| SPIKE-005 | Satellite client implementation size and RAM | ≤ 500 lines core logic; ≤ 1 MB | 2× | Fall back to WebSocket decision |

**Exit:** CI green on an empty-but-real workspace; budget gate running; specs Accepted; feasibility gate passed or budget formally revised.

---

# Phase A: PC loop (hardware-agnostic core)

Everything in this phase runs on a desktop. Nothing here may depend on a particular speaker.

## v1 exit (Phase A and the v1.0 line in one place)

Phase A is done, and a `v0.x` release can claim "ClawDot v1 behaviour", when all of these hold:

1. **One release, two machines:** the same release runs on a PC and on a Raspberry Pi (Pi parity is proved in B7; before then, the same binary and config on a PC and a Pi-class build).
2. **A spoken turn with 2-3 default tools** (see "Default toolset for v1" below) works end to end: ask, the agent calls tools, the answer is spoken.
3. **An agent event is spoken with its source**, and a **voice reply resumes the session** (AC-3, AC-6).
4. **Actions by voice are confirmed by the host:** a risky call runs only after a later user turn confirms the exact action; a flag supplied by the model never releases a tool (A1b).
5. **The offline eval gate passes:** the replay-mode scenario set runs in `scripts/check.sh` and CI, including the injection scenarios (A1c).

Status (2026-11-09): **2 of 5 met, 2 built but unproven, 1 not met.**

- **Item 4: met.** The host-held confirmation (`bugfix/host-held-confirmation`, `83f0f80`, REQ-SEC-011) ignores the model's `confirmed` flag; the replay set holds scenarios for it (for example `inj-model-forges-the-confirmed-flag`, `uc1-instruct-needs-exact-confirmation`). It is a logic property, shown by tests and replay, not by audio.
- **Item 5: met.** `scripts/check.sh` runs `clawdot-eval -- replay` over 81 scenarios (24 injection) with no network and no spend (`feature/eval-harness`, `9d80cf6`, REQ-AST-016..018). Live mode and the "once against a real model" part of the A1c exit are not done, but item 5 asks only for the replay gate.
- **Item 2: built, unproven.** The tool loop, the timers and the stopwatch run in the replay set and in the end-to-end tests with fakes; the PC voice loop was rehearsed with a recording as the microphone, a local whisper and Piper, and a free stand-in model (`feature/pc-voice-loop`, `f5cce39`). No real microphone, speaker or default-tool server has been used.
- **Item 3: built, unproven.** The inbox, the spoken announcement and the reply path run over real sockets in `crates/e2e/tests/agent_loop.rs`; the real `claude` CLI resume has not been verified and nothing was spoken on a real speaker.
- **Item 1: not met.** `v0.1.0` is prepared but no release has been published, and no Raspberry Pi has run the build (B7). A Pi board file exists (`boards/raspberry-pi.json`) and passes the conformance suite against a fake board only.

### Default toolset for v1

The default toolset is a set of **recipes** (configuration that the portal can add, with the config shown), not hard-coded behaviour. Each is optional and the core runs with none of them (REQ-AGT-008 as proposed).

- **Timers, alarms, reminders and a stopwatch:** built in. *Built and tested*: the scheduler, and the stopwatch (`feature/stopwatch-and-timers-polish`, `54fe711`, REQ-TMR-010..014; scenarios `uc6-stopwatch-*`).
- **Web search:** through a declarative HTTP tool or an MCP server, chosen by the user. *Recipe written, not run against a real server:* [recipes/](recipes/README.md) (SearXNG, Tavily, Brave MCP), REQ-EXT-007..010, `feature/default-tool-recipes` (`01565f9`); covered by the fake-tool scenarios `rcp-*` (`rcp-web-search-text-cannot-switch-off-lights`).
- **Spotify control** through the Spotify Web API (HTTP tool or MCP). *Recipe written ([recipes/spotify.md](recipes/spotify.md)) but blocked: Spotify's Developer Policy section III.3 (effective 2025-05-15) restricts voice-controlled apps, so it is documented as not permitted to rely on; decision pending. Nothing was run against Spotify.* Receiver options: a Spotify Connect device elsewhere, or the opt-in Connect receiver on the device (B6). Audio does not play on ClawDot's speaker unless a receiver runs there.
- **Home Assistant tools:** optional, via its API or MCP server; never required. *Recipe written ([recipes/home-assistant.md](recipes/home-assistant.md)), untested against a real Home Assistant.*
- **Coding-agent bridge:** the Claude Code bridge (resume today, job start later) as the agentic centrepiece. *Bridge and hook recipe ([recipes/claude-code-hook.md](recipes/claude-code-hook.md)) built; job start not built; real `claude` resume unverified.*

## A1. Process and safety
**Goal:** a faster, safer base before more features pile on.

- [x] Faster commit checks (`chore/faster-commit-checks`, merged as `dcbe581`): the commit hook runs only the fast checks, and the full set runs in `scripts/check.sh` and at landing, so no gate is weakened ([git-workflow.md](process/git-workflow.md); commits `5574c54`, `f6f76b3`)
- [x] Findings of the [ingress security review](reports/2026-11-08-security-review-ingress.md), each in a small branch with a regression test:
  - **F1** (High): the unconfirmed `instruct` reply and unfenced `details` text are fixed in the inbox (`bugfix/inbox-instruct-confirmation`). The wider fix, where confirmation is held by the host for every tool, is A1b and has landed (`bugfix/host-held-confirmation`, `83f0f80`)
  - **F3** done: `title` and `status` go through the prompt fence and are spoken with their source
  - **F4** done: the LLM client no longer accepts plain `http://` to arbitrary hosts (private hosts only, REQ-SEC-001)
  - **F2** done: per-source pre-auth caps and body stall timeouts (also F9)
  - **F6 / F7** done: state files created with mode 0600; no agent text and no one-time tokens on stdout
- Not in this step, tracked in the report: F5 (persistence fsync under lock), F8, F10 to F15

**Exit:** the listed findings fixed and traced; `scripts/check.sh` fast enough that nobody is tempted to skip it.

*Status: done. Findings F1 (inbox, then host-held for every tool), F2, F3, F4, F6, F7 landed with regression tests, and faster commit checks landed. The listed exit is met; F5, F8 and F10 to F15 stay tracked in the report, outside this step.*

## A1b. Action safety
**Goal:** a model can never act on its own say-so. Lands before more write-capable tools are connected. Design: [agentic-scope.md](design/agentic-scope.md) sections 3 and 5 (G1 to G3, G6); proposed requirements REQ-SEC-010..016 (not yet in the spec; amend the spec before or with the code).

- **Host-held confirmation:** a pending-action ledger; risky calls are read back and run only after a later user turn confirms the exact action; the model's own `confirmed` flag is ignored. *Built* (`bugfix/host-held-confirmation`, `83f0f80`, REQ-SEC-011)
- **Risk tiers** on every tool (T0 read-only to T3 destructive), set by the administrator, default T2 for unclassified tools, server hints cannot lower a tier; the portal Tools tab gets a tier selector. *Built* (REQ-SEC-010; the administrator sets a tier in `5885862`, the portal Tools tab lists and sets it in `84d9da0`, `feature/portal-tool-tiers`)
- **Event-turn read-only rule:** turns started by events, schedules or rules run in no-act mode (T0 and propose-only). *Built* (REQ-SEC-012; scenario `uc2-event-turn-proposes-only`, `inj-event-cannot-set-a-timer`)
- **Untrusted-content rule:** text from any tool result is fenced as data, and untrusted text in a turn's context forces full confirmation on T2 and T3 calls. *Built* as taint (REQ-SEC-012, [action-safety.md](architecture/action-safety.md); scenario `inj-lowered-tool-held-after-outside-text`)
- **Enforced skill allow-lists:** a skill's tool list narrows the tool set when that skill is the active scope (amends REQ-CFG-036; it was advisory). *Built* (REQ-SEC-013)
- **Audit log:** a bounded private record of every tool call, confirmation, refusal and cap hit, with a portal view. *Built* (`feature/audit-log`, REQ-SEC-014): JSON lines, rotation, redaction, `GET /api/audit`; see [action-safety.md](architecture/action-safety.md)
- **Spoken read-back redaction:** a held action is read back without secrets. *Built* (`bugfix/redact-spoken-readback`, `2aa6624`; scenario `pol-spoken-readback-hides-secrets`)
- **Daily caps and dry-run:** a per-day cap on actions and a stricter one on costly actions, and a dry-run mode that describes instead of running. *Built* (REQ-SEC-015, REQ-SEC-016), configured in the file or by `--dry-run`; the portal now has the daily-limit settings (`feature/portal-agent-settings`, `1bad4c5`); scenarios `pol-daily-cap-stops-actions`, `pol-dry-run-describes-instead-of-acting`

**Exit:** the injection and confirmation scenarios pass (A1c); AC-14 to AC-16 as proposed hold; every write-capable tool has a tier.

*Status: built and tested with fakes and in the replay set; none of it has run against a live model or by voice on real audio. Every item above landed (host-held confirmation `83f0f80`, audit log `9e71771`, read-back redaction `2aa6624`, tiers in the portal on `7fcc110`). The exit holds in replay mode: the injection and confirmation scenarios pass in `scripts/check.sh`. AC-14 to AC-16 are not recorded as run. "Every write-capable tool has a tier" holds by default, since an unclassified tool is T2; the recipes' default tiers are listed in [recipes/README.md](recipes/README.md).*

## A1c. Eval harness
**Goal:** every change to the agent loop is measured, offline and for free, before it merges. Replaces the "eval set" bullet that used to sit in A7.

- **Scenario format:** a small declarative file per scenario (user words, fake tool results, expected calls, forbidden calls, expected speech); about 30 scenarios to start (the use cases in agentic-scope plus an injection set)
- **Replay mode:** runs in CI and `scripts/check.sh` against recorded or scripted model turns; no network, no spend; a gate
- **Live mode:** run on demand against a real model with a **spend cap**; produces the tested-models list (REQ-AST-012)

**Exit:** replay mode is a `check.sh` gate with the injection set at 100 percent; live mode runs once against the default recommended model and the result is recorded.

*Status: replay mode is a `check.sh` gate and the scenario set is built (`feature/eval-harness`, `9d80cf6`, REQ-AST-016..018; [evals.md](architecture/evals.md)). `ls evals/scenarios` lists 81 scenarios on `origin/dev`, 24 of them injection scenarios (`inj-*`; the others are 21 agent-loop `agt-*`, 6 policy `pol-*`, 4 recipe `rcp-*` and 26 use-case `uc*`). `scripts/check.sh` runs `clawdot-eval -- replay` over all of them (step "agent-loop scenarios"), and CI runs the same. Live mode is built and tested against a fake provider only. Exit: the replay half is met; "live mode runs once against the default recommended model" is still open and needs the maintainer's go and an API key.*

## A2. Audio interface
**Goal:** one small audio in/out trait, so real sound reaches the core on a PC and later on the Dot without changing the core.

- A trait for capture (16 kHz mono PCM frames) and playback, with **command-backed backends**: on the PC, existing tools (ffmpeg, sox, arecord-like); on the Dot later, `tinycap` and `tinyplay` ([R-07](research/07-echo-dot-gen2-hardware.md) section 6). Spawning tools keeps `unsafe_code = "forbid"` intact and adds no dependency
- A fake backend for tests; underrun and device-loss behaviour defined
- **SPIKE-006 on the PC**: latency, underruns over 10 minutes, RSS delta
- Spec impact: amend REQ-VOI as needed before code

**Exit:** a microphone-to-file and file-to-speaker round trip on a PC through the trait, with a written SPIKE-006 PC finding. (Not yet claimed anywhere: no audio has run.)

*Status: built, not yet run on real audio.* The traits (`AudioIn`, `AudioOut`), the command backend (arecord, aplay, sox, ffmpeg, ffplay from `PATH`), the WAV backend and push-to-talk are in `crates/satellite` (REQ-VOI-013..018), tested with scripted child processes. The manual smoke test is [guides/pc-voice-loop.md](guides/pc-voice-loop.md). Still open: the first real run, and the SPIKE-006 PC measurements (latency, underruns over 10 minutes, RSS delta). The `tinycap`/`tinyplay` presets for the Dot are not written. The 2026-11-09 rehearsal ([pc-voice-loop.md](guides/pc-voice-loop.md)) played a recording through the stack and wrote the reply to a file, so the real `ffmpeg` capture and `ffplay` playback programs on a real microphone and speaker were still not exercised.

## A3. Wake word
**Goal:** hands-free start, reusing an existing model.

- SPIKE-002 on the PC: evaluate existing engines and pretrained models (for example openWakeWord); record licence, model size, RSS and idle CPU. Justify any dependency against the budget; prefer a sidecar or opt-in feature if it does not fit by default
- **Push-to-talk / hotkey fallback** so the loop is usable without a wake word, and as the Dot's button path later
- Custom "ClawDot" word training stays out of v1.0 (EPIC-001 covers integration and the evaluation set only)

**Exit:** a written SPIKE-002 PC finding; wake or hotkey starts a turn on a PC.

*Status: built and tested without real audio; the wake word has never heard a real voice.* Push-to-talk exists on the desktop satellite (`--ptt`, Enter to talk). The seam is `front::Trigger` (REQ-VOI-017). The PC part of SPIKE-002 is written up ([R-11](research/11-wake-word-spike.md), `docs/wake-word-spike` merged as `9a42b51`: Go for the approach with one open item). A3b is built: `clawdot-wake` (`crates/wake`) runs the official microWakeWord models in pure Rust ([spec 0007](specs/0007-wake-word-runtime.md), [runtime note](architecture/wake-word-runtime.md), `feature/wakeword-runtime`, `7c08fad`), checked against the reference implementation on synthetic golden clips, and its device adapter (`MwwEngine` in `crates/device`, feature `mww`) feeds always-on capture (the microphone stays open between turns; `crates/satellite/src/front.rs`, `feature/pc-voice-loop`, `f5cce39`). The whole path was rehearsed with a recording as the microphone. Exit: the SPIKE-002 PC finding is written; "wake or hotkey starts a turn on a PC" is shown only with a recording and fake microphones. Open: an evaluation set of real voices (false accepts and rejects), the armv7 and device measurements, and licence provenance for any model beyond the official ones.

## A4. End-to-end voice turn on a PC (old M1 exit)
REQ-VOI-001..004, 006..010, 012, REQ-AST-001..003, 006, 007, 009, REQ-CFG-007, REQ-NFR-001/003/005/007, REQ-SEC-001..003, 006, 007

- Wake or hotkey, VAD, batch STT, streaming LLM, sentence-level TTS, half-duplex, through the real A2 audio backends and a real provider
- First budget measurements with the real pipeline

**Exit:** AC-1 and AC-10 pass on a PC.

*Status: partly built; the real-provider pieces have run only on a recording.* Done and tested against fakes: in-process and remote (TCP) transport, voice dialog state machine, energy-based endpointing, STT/TTS clients, streaming LLM client, half-duplex, JSON config file and the reference satellite (`crates/core/src/{voice,vad,speech,satellite,transport,config}.rs`, `crates/satellite`, [voice-pipeline](architecture/voice-pipeline.md)). The satellite frame JSON Schemas (`schemas/satellite`, TASK-004) and the conformance harness (TASK-005) are done. Since the last update: local whisper.cpp and Piper are wired in ([local-speech.md](guides/local-speech.md), `b24b181`) and ran in the 2026-11-09 rehearsal with a recording as the microphone and a free stand-in model; the real wake engine woke the daemon in that run. Not done: a run with a real microphone and speaker, a run against the recommended hosted model as part of the loop, and the ratchet. AC-1 and AC-10 are not demonstrated anywhere.

## A5. Agent loop by voice with Claude Code (old M2 exit)
REQ-INB-001..018, 020, REQ-SEC-004, REQ-EXT-004, 006

- Minimal HTTP ingress, endpoints and tokens, queue, announce/dismiss/snooze/reply; format adapters (Claude Code native `http` hook first), filtering and priority, reference agent bridge; SSE/poll sources; "what are my agents doing?"; integration snippets (Claude Code hooks, shell, GitHub Actions)
- The remaining work is the spoken half: a real Claude Code session finishes, the PC speaker announces it, a spoken reply resumes the session

**Exit:** AC-3 and AC-6 pass; a PC demo of "Claude finishes → speaker announces → voice reply resumes the session". Proposed additions (agentic-scope section 7): the job-start path (the bridge starts a new job as a T2 tool with exact-text confirmation) and the failure case (a job that fails is announced and can be retried).

*Status: mostly built, voice half missing.* Ingress, endpoints with hashed tokens, queue, replies over SSE, adapters (`claude-code`, `generic`), summaries, the bridge that resumes Claude Code, and per-endpoint snippets exist (`crates/core/src/inbox/`, `crates/bridge`), with the headline loop tested over real sockets (`crates/e2e/tests/agent_loop.rs`, `satellite_loop.rs`). Not done: the real `claude` CLI resume has not been verified, and there is no live demo. Fuzzing of ingress moves to Phase C. Since then the A1b dependency has landed (host-held confirmation, `83f0f80`). The Claude Code hook recipe is written ([recipes/claude-code-hook.md](recipes/claude-code-hook.md)); the one-way announce path and the ntfy and Discord bridges (A7) give two more ways in. A5 depends on the A1 fixes (F1 in particular) and on A1b before it is enabled by default.

## A5b. Agent loop
**Goal:** a multi-step voice request is a bounded, audible, cancellable task, not a silent wait. Design: [agentic-scope.md](design/agentic-scope.md) (build list P1-1 to P1-9, proposed REQ-AGT-001..008). Depends on A1b and A1c; most of it is testable on the text console and with fakes, without audio.

- **Limits:** bound rounds (default 8), calls (12) and wall-clock time (60 s) per turn, with per-tool timeouts and per-tier daily caps; hitting a limit stops new calls and says what was done and what remains
- **Spoken plan:** a one-line plan before a request that needs three or more steps; ask a clarifying question rather than guess when an argument is missing or a target is ambiguous
- **Progress speech:** a short phrase before any tool call expected to exceed 3 s (needs a streaming `run_turn`)
- **Cancel:** "stop" and "cancel" end the running turn cooperatively and the assistant says what was and was not done; stop-while-working on audio depends on the open question about half-duplex (feeds B4/B6)
- **Memory scope:** history stays bounded and in memory; a recent-actions list for follow-ups such as "undo that"; persistent notes only as opt-in, bounded and manageable in the portal
- **Result summaries:** long tool results are summarised for speech by the summary model (which cannot call tools), with the full result kept for follow-ups
- **Rules ("do X when Y"):** moved up from Beyond v1.0. First version is tell-me and remind-me actions only, listed in the portal, starting as drafts until confirmed, running in a no-act scope; rules that act need a threat review first
- **Recipes for the default toolset** (see "Default toolset for v1"): real-server trials for web search, Spotify control, optional Home Assistant, and the coding-agent bridge (including job start), one recipe each, also used as eval fixtures

**Exit:** the v1 exit items 2, 3 and 4 pass in replay mode and once live; the default toolset is shipped as recipes and verified against real servers where the server's terms allow.

*Status: the loop is built and tested with fakes and in the replay set; not started: rules, background tasks with ids, and any live-model or device check.* It landed as `feature/agent-loop` (`d4cd9d3`; REQ-AST-019 to REQ-AST-025, REQ-VOI-020, REQ-SEC-017; [architecture/agent-loop.md](architecture/agent-loop.md)). Covered by 21 `agt-*` scenarios plus the injection set: limits and their spoken account, the spoken plan and progress phrases (sentence-level, not token streaming), clarifying questions and `ask_user`, cooperative cancel (button, or wake word and a stop phrase while the assistant is silent), recent actions and opt-in notes, long-result summaries with the full text in the audit record, and error recovery. The portal screens for these settings have landed (agent settings, daily limits and quiet hours in Settings, the Notes tab and the Activity tab; `fe71e74`, `ed71345`). The recipes for the default toolset are written ([recipes/](recipes/README.md), `01565f9`) and used as eval fixtures, but not run against real servers. Exit: items 2, 3 and 4 pass in replay mode (4 does; 2 and 3 are covered by fake tools); "once live" and "verified against real servers" are open. Not built: stop over the assistant's own voice (needs echo cancellation, B3), a local stop-word check, and job start for the coding-agent bridge.

## A6. Web UI (old M4)
REQ-CFG-001..006, 008, 009, 012, 013, REQ-INB-019, REQ-NFR-006

Landed already: first-run admin setup (choose an admin password or deliberately leave the UI open on a private network, with an "unprotected" banner; private-address-only setup, no setup code; `--reset-admin` and a button-opened reset window for recovery, no recovery code; REQ-CFG-018..024, [ADR-0013](adr/0013-first-run-admin-setup.md)), sign-in with rate limiting, the UI theme and logo set, and the default listen address `127.0.0.1:3335` (REQ-CFG-014, chosen so as not to collide with Amazon's `oobed` on 8080). Also landed: **editable settings** saved to `settings.json` in a layer between the configuration file and flags, with per-group save and a build-time gzip copy of the page (REQ-CFG-037..043), and the **tabbed portal**, now a **TypeScript and Preact** app built by esbuild into one committed `crates/core/src/ui.html` (`feature/portal-typescript`, `3529b61`; ESLint and Prettier gates, `chore/portal-lint-format`, `ad88ca2`; a `portal` job in CI; [ADR-0016](adr/0016-portal-typescript-stack.md), still Proposed; [design/ui-stack.md](design/ui-stack.md)). `portal/src/App.tsx` lists **10 tabs**: Home, Reminders, Notes, Agents, Activity, Tools, Connections, Skills, Webhooks, Settings (REQ-CFG-026), plus an About view with diagnostics. Tested by the Rust UI tests (`crates/core/src/ui/tests.rs`) and the portal's own tests; the portal has not been used by a new user on a real device.

Landed since the last update, from the "still to do" list:
- **Activity view and tier selector** for A1b: the Activity tab shows the audit log with filters and paging (`GET /api/audit`, REQ-CFG-060, `ed71345`); the Tools tab lists and sets each tool's risk tier (`f592297`)
- **Notes tab** (`a758c3b`, REQ-CFG-044), agent limits, daily caps and **quiet hours** in Settings (`feature/portal-agent-settings`, `fe71e74`); the core holds non-urgent announcements in a device-wide window (`0abbe9f`). The device applies it locally in B6, which is not built
- **Settings page gaps**: a board setting (`auto` or `none`, REQ-CFG-037; the list picker with detection evidence waits for B1) and a "restart needed" note listing the saved values that are not running yet
- **Log viewer, diagnostics bundle and settings export** (`GET /api/logs`, `/api/diagnostics`, `/api/settings/export`; `feature/portal-about`, `7e743b6`; the diagnostics test asserts no secret is included)
- **First-run checklist** on Home: clock set, admin password, language model, a speaker connected, an agent connected, speaker test (no Wi-Fi item)

Still to do:
- **Settings import** (export exists; secrets are excluded from it)
- **Connect an agent** flow: per-endpoint snippets exist and the Agents tab and Home link to it; the full guided flow with the bridge command is not built
- Endpoint management (Webhooks tab) and test speak (Home) exist; per-endpoint history and health were not verified
- **mDNS (`clawdot.local`)** and headless Wi-Fi docs for Pi: not started (no code found)
- Webhook tokens stay required whether or not the UI is protected; no multi-user accounts

Hardware auto-detection and the board picker are in B1; the settings page has only the `auto` or `none` board setting until B1 is done.

**Exit:** a new user can set up from the UI alone. *Not demonstrated; no first-time user has set up a device from the portal.*

## A7. Remaining local intelligence and integrations (old M3 and M5)
REQ-TMR-001..008, REQ-AST-004, 010..013, REQ-NFR-008, 009, 111; REQ-MUS-001..006, REQ-AST-005, 008, 014, REQ-CFG-010, REQ-EXT-005

Local intelligence (old M3)
- Offline degraded mode; persona config as specified (REQ-AST-013). The tool-calling eval set and tested-models list moved to A1c
- *Status: partly built.* Scheduler (timers, alarms, reminders, DST-aware), atomic versioned JSON store, fast-path matcher, usage limits and a clock-unset guard exist and are tested (`crates/core/src/{sched,store,fastpath,usage}.rs`; commits 9ce871d, db8655f). The stopwatch landed later (`54fe711`, REQ-TMR-010..014; scenarios `uc6-stopwatch-*`, including the unset-clock case). Offline degraded mode (the LLM unreachable) was not found in the code, so it is not started. The AC pass/fail status is not recorded.

Announce and integrations
- A **one-way announce/say API and CLI** (speak this text, no reply expected), the simplest integration for scripts and automations. It reuses the inbox's untrusted-text handling (A1). *Built and tested* (`feature/announce-api`, `bd6fb10`, REQ-INB-021..027): `POST /api/announce`, `clawdot say`, [guide](guides/announce.md); scenarios `inj-announce-text-cannot-run-a-tool` and `inj-announce-text-cannot-say-yes`. Not spoken on a real speaker
- **Off-device bridges as sidecars**: Discord and ntfy, kept out of the core binary so the default build stays in budget. *Built* (`feature/bridges`, `39ef546`, REQ-INB-028..031, [bridges/](../bridges/README.md)); Python sidecars whose standard-library tests and `ruff` run in the gate (`ac69d2f`) against fake ClawDot and ntfy servers. **Not tested** against a real ntfy server or a real Discord gateway ([bridges README](../bridges/README.md))
- Recipes: the real-server recipes (Spotify control, Home Assistant, web search, coding agent) moved to A5b and are written ([recipes/](recipes/README.md)), not run. **Spotify control** controls a Connect device (audio plays there, not on ClawDot's speaker unless the B6 receiver runs) and is blocked by Spotify's Developer Policy section III.3 (effective 2025-05-15); no Hermes recipe exists in `docs/recipes/`
- **Internet radio** and ducking, media-source interface, MCP client (Streamable HTTP), declarative HTTP tools with confirmation. *Status: partly built.* Radio by voice with ducking, the media-source model, the MCP client and HTTP tools exist (`crates/core/src/{media,mcp,httptools}.rs`; [media](architecture/media.md), [mcp](architecture/mcp.md), [http-tools](architecture/http-tools.md)), tested only against fake servers and players. The Spotify and Home Assistant recipes are untested against their real services
- **Willow and Home Assistant / Wyoming satellites as optional front ends**: they interoperate through the satellite protocol and the bridge; we do not fork them (see Beyond v1.0)

**Exit:** AC-2, AC-4, AC-7, AC-8 and AC-9 pass; one announce path and one music path demonstrated on a PC. *Not met: no AC pass is recorded, and neither path has been demonstrated with real sound.*

---

# Phase B: Board layer and the Echo Dot Gen 2

*Reworked 2026-11-09 from R-08 section 9, then revised the same day for the maintainer's decision to own the device layer ([ADR-0015](adr/0015-own-the-device-layer.md), Proposed): one portable device daemon on every Linux board, a board file and an image recipe per device. The earlier plan (own audio and LEDs on Fire OS) is superseded; see the note at the end of this phase. Nothing here is decided beyond what the maintainer approves; the open points are in [open-questions.md](open-questions.md).*

Starts after the PC loop works (A4 and A5 at least) and the feasibility gate for the device spikes is Go. The sequence: finish the PC voice loop; build the portable daemon on PC and Pi (B2) so the contract is real; run the TECHO5 trial (B0, which needs the maintainer and can run beside B2) to learn the Dot; spike the wake word and echo canceller on the Dot (B3) and pass the [ADR-0015](adr/0015-own-the-device-layer.md) gate; then bring up the Dot on T5D's OS image with our daemon (B4) and add images per board. Hardware facts are in [R-07](research/07-echo-dot-gen2-hardware.md), the unlock history in [R-09](research/09-echo-unlock-history.md), and the existing Dot projects in R-08. The device layer decision is ADR-0015; the board design is [ADR-0012](adr/0012-board-support-layer.md), [board-support](architecture/board-support.md) and spec REQ-BRD (all Proposed). **Every step that changes the device, and every single device write inside a step, needs the maintainer's explicit go. Stop and ask before each one.**

The maintainer's unit is a Dot 2 that is already unlocked: rooted Fire OS with TWRP. It is the test unit for B0 and the entry state for B3.

**Phase B status (2026-11-09):** the PC-side half is built; nothing in Phase B has run on a device. Built and tested on a PC: the device contract ([spec 0006](specs/0006-device-layer-contract.md), Draft), the board-file JSON Schema (`schemas/board/`), the `clawdot-device` daemon (`crates/device`), three board files (`boards/desktop.json`, `raspberry-pi.json`, `echo-dot2.json`) and the conformance suite (`crates/device-conformance`; `feature/device-conformance`, `daecd81`), which runs all three board files against fakes. Not started: B0 (a written plan only), B3, B4, B5, B6, B7 and the echo canceller. ADR-0012 and ADR-0015 are still Proposed.

## B0. Trial of the TECHO5 Dot base image (spike)
Goal: learn the hardware and the image and update process from a working Dot image (TECHO5), so ClawDot's own daemon and board file can be built from its findings. B0 is the gate input for [ADR-0015](adr/0015-own-the-device-layer.md); it is learning from unmodified TECHO5, not choosing a permanent front end.

- Trial the TECHO5 Dot (T5D) base image on the maintainer's unit, which is rooted Fire OS with TWRP. Installing it writes to the recovery partition and replaces the system, so **each device write waits for the maintainer's explicit go**
- Before any write: full partition backups taken and checked, and the installer's `--dry-run` read first
- **Rollback path, recorded before the trial:** recovery is the XDA procedure ([Unlock, root, TWRP, unbrick Echo Dot 2nd gen "biscuit"](https://xdaforums.com/t/unlock-root-twrp-unbrick-amazon-echo-dot-2nd-gen-2016-biscuit.4761416/)) with the TWRP backup and the partition backups from this step, restoring the rooted Fire OS state ([R-09](research/09-echo-unlock-history.md)). Restoring is proved on paper before the first write and written into the spike finding
- Measure idle RSS, CPU and boot time; check whether the base's `echod` serves an ESPHome client that is not Home Assistant (voice subscribe, announce, ring entity); check how the Noise key is handled
- This trial is also how Alexa gets wiped: **removing Alexa is achieved by installing the base image, not by our own staged removal on Fire OS**, and every step of it is gated on the maintainer's go

*Status: not run.* B0 is runbook-only: [guides/b0-techo5-trial-plan.md](guides/b0-techo5-trial-plan.md) (`docs/b0-techo5-trial-plan`, `4f005fb`) is a plan for review, and no agent or person has run any step of it on the device.

**Exit:** a written spike finding that records mixer routes, LED format, button codes, channel map, the image and update flow, and idle figures, as input to the B3 gate. (The "Dot front end base" ADR proposed in R-08 is replaced by ADR-0015.) If T5D cannot be installed or restored safely, evaluate EchoMuse or LibreEcho instead.

## B1. Board crate (reduced)
REQ-BRD-001..014

- `crates/board`: the `Board` trait, `NullBoard`, a desktop board, shared gesture and mapping code, the JSON data-file schema (board files are JSON, decided 2026-11-09)
- **Hardware auto-detection:** `board = "auto"` fingerprints the hardware against each compiled-in board's `detect` rules (device tree, `/proc/cpuinfo`, sysfs, `/proc/idme/device_type_id` on the Dot). A single confident match selects it. **With no confident match the board stays unset and the settings page asks the user to pick from a list, showing the detection evidence.** Users never edit mapping files unless they want to
- **The Dot board is data only until the base trial (B0) is done:** names, key codes, the LED `frame` format and a `forbidden` list of paths the board must never touch, including `power_button_state` and `calibrated_lux`. No Dot code that opens audio, LED or button devices
- ADR-0012 moved from Proposed to **Accepted**; the settings page gets its board section
- Default build unaffected (REQ-NFR-115); a CI feature matrix (`default`, `desktop`)

*Status: partly built, under a different shape.* The `Board` trait, `NullBoard`, `ConfiguredBoard`, the embedded board files and the strict board-file loader live in `crates/device` (`board.rs`, `board_file.rs`), not in a separate `crates/board`; the desktop board drives the PC loop through the daemon (REQ-BRD-001..014 are not individually re-traced here). The Dot board is data only (`boards/echo-dot2.json`). Not done: `board = "auto"` detection against real hardware, the settings list picker, the CI feature matrix, and ADR-0012 stays Proposed.

**Exit:** the desktop board drives the core on a PC through the trait; ADR-0012 Accepted; budget impact measured, not estimated.

## B2. Device contract and the portable daemon, on PC and Pi first
REQ-DEV-001..012 (proposed in [ADR-0015](adr/0015-own-the-device-layer.md); drafted as [spec 0006](specs/0006-device-layer-contract.md), with the board-file schema in `schemas/board/`)

- The **device layer contract** as a spec, the **board-file JSON Schema** (mixer routes, LED frame format, button codes, mic channel map, forbidden paths) and a **device conformance suite** that runs against a fake board on a PC
- The **portable device daemon** (Rust): ALSA in and out, tones, ducking, buttons and LED states driven only from the board file, local stop without the wake word, quiet-hours hook, native satellite protocol. **No per-device code paths**
- Echo cancellation and noise suppression behind **one isolated crate** that wraps a proven C library (WebRTC audio processing or SpeexDSP); it is the only place allowed `unsafe`; notices in `NOTICE`
- Runs first on the PC and then on a Raspberry Pi through board files, so the contract is exercised before any Dot work. Spec 0003 amendments (logical LED states, tone, quiet hours, update messages) as minor `clawdot.` additions
- The default core build and budget are unaffected (REQ-NFR-115)

*Status: PC side built, Pi and echo canceller not.* Spec 0006 (Draft), the schema, the daemon and the conformance suite exist and run in CI against a fake board for all three board files ([device-daemon.md](architecture/device-daemon.md), [device-conformance.md](architecture/device-conformance.md)). The PC voice loop runs through the daemon with always-on capture ([pc-voice-loop.md](guides/pc-voice-loop.md)); it was rehearsed with a recording only. Not built: the `alsa` audio kind, evdev buttons and the `ledctrl`, `frame` and `sysfs` LED drivers (accepted by the schema, reported as absent), mixer routes, ducking of other sources, the isolated echo-cancellation crate, quiet-hours settings over the wire, and a local-cancel message to the core. `device-daemon.md` still says the wake engine is not fed; the PC loop now feeds it, so that note is stale.

**Exit:** the PC and the Pi run the full voice loop through the daemon and a board file, and the conformance suite passes against a fake board. *The conformance half is met against fakes; the PC loop is rehearsed; the Pi has not run.*

## B3. Daemon feasibility spike on the Dot (the main unknowns)
- Measure on the Dot (maintainer's go for every device write): wake-word engine CPU and RSS (research says about 23 to 30 percent of one core), echo-canceller cost on the Cortex-A53, capture-to-wake latency, with the base OS idle figures from B0
- Pick the wake-word runtime: pure Rust, then an engine already in the base, then a statically linked C runtime behind the isolated crate (ADR-0014 point 4). Models only with recorded licence provenance; no pretrained openWakeWord models are bundled
- Decide whether AEC fits at all on the Dot; if not, barge-in stays off and the board file says so

*Status: not started.* No figure has been taken on the Dot beyond the idle measurement in A0.

**Exit:** a written spike finding and the **go/no-go gate of [ADR-0015](adr/0015-own-the-device-layer.md)**. Go moves the ADR toward Accepted; no-go falls back to a fork of `echod` (kept in the image repository) or to the unmodified-base route in B8.

## B4. Image repository and Dot bring-up
- **The maintainer creates the image repository** (working name `clawdot-os`); no agent creates repositories. This step starts only after the gate in B3 is Go
- The **Echo Dot 2 recipe** starts from T5D's OS image (kernel, rootfs, install and update flow, MIT, notices kept) with **our daemon in place of `echod`**, and a board file built from T5D's findings (mixer routes, LED format, button codes, channel map). Strip what we do not want; brand it
- Track upstream T5D; send generic fixes back; `NOTICE` credit; **never redistribute Amazon files or unlock tooling**, and never a boot image containing Amazon's kernel (publish the recipe)
- **docs/guides/install-on-echo-dot-2.md** plus a guide template for other devices; starts from the already-unlocked state, links the unlock procedure, pins the firebreak version, credits the people involved (R-09 section 9), and keeps the restore path from B0

*Status: not started* (blocked on the B3 gate; the image repository does not exist).

**Exit:** a fresh unlocked Dot reaches a voice turn through our daemon following only the guide.

## B5. Image build and signed A/B update pipeline
REQ-DEV-007

- Reproducible image build, a project signing key (held by the maintainer) and a **signed A/B update with automatic rollback** on the Dot partition layout (or the documented `update: manual` alternative, if the layout allows no A/B)
- The conformance suite gains update tests (bad signature rejected, failed boot rolls back)
- Wi-Fi setup without a keyboard and clock trust on the image

*Status: not started.*

**Exit:** an update is signed, installed and confirmed on the Dot, and a deliberately broken update rolls back on its own; recorded run.

## B6. Device features through the daemon
- LED states for agent events on the ring (`inbox`, `listening`, `thinking`, `speaking`, `error`, `timer`, `updating`), the documented equivalent on boards without a ring
- Buttons and gestures, mute with its honest kind, **stop without the wake word**, follow-up listening, **quiet hours** applied locally
- **Wake word** with licence-clean models and recorded provenance (EPIC-001); **Bluetooth speaker mode** (A2DP sink and source, arbitration with announcements measured in B0)
- **Spotify Connect receiver** as an opt-in with the terms-of-service notice; Spotify *control* through the brain is a software feature and needs no device work
- Anything the hardware cannot do is a documented gap, not a faked capability

*Status: not started on a device.* The daemon's LED state machine, gestures, tones and quiet-hours policy are built and tested on a PC (console LEDs and terminal keys only).

**Exit:** the Dot shows the agent states, honours stop and quiet hours without the brain, and the recorded conformance run on the unit is green for every Must.

## B7. Pi and PC parity, and more boards
- Raspberry Pi image recipe and install guide from the B4 template; PC satellite (the daemon in a desktop package). Both pass the same conformance suite with the same daemon logic and only a different board file and recipe
- A third board (for example a smart-calendar-class device) is the test of the claim "the only per-device difference is how you install it": a board file, a recipe, a passing run, and no daemon change. Boards below the minimum contract (not Linux-capable, too little RAM, no microphone or speaker) are documented as unsupported

*Status: not started on hardware.* The Raspberry Pi board file passes the suite against a fake board, which is evidence for the "no daemon change" claim but not a real-hardware run.

**Exit:** Pi and PC satellites pass the conformance suite; real-hardware run recorded; the third board needs no daemon code change.

## B8. ESPHome native-API client (compatibility mode)
Goal: third-party ESPHome voice satellites (stock T5D or LibreEcho images, a Pi running linux-voice-assistant) can use ClawDot as their brain without Home Assistant. Not the path for our own devices. Spec-level item: see the [future-scope](future-scope.md) row and R-08 sections 4.2 and 4.3.

- A new optional core feature (`esphome-satellite`) behind the existing satellite interface: frame layer, minimal hand-written protobuf for about 20 messages, voice session, announce-then-listen for the inbox, media player play, optional light command. Estimate about 1,100 to 1,300 lines; not in the default Dot budget
- Optional Noise encryption on `ring`, with no `snow` and no `prost`; tested against a fake ESPHome server on a PC first, then `aioesphomeapi`
- **Status (2026-11-09):** built as an optional compatibility feature (`feature/esphome-satellite-client`, `52cd1ca`) behind the cargo features `esphome-satellite` and `esphome-noise` ([spec 0004](specs/0004-esphome-satellite-client.md), [architecture/esphome-satellite.md](architecture/esphome-satellite.md)) and tested only against the fake device in `crates/e2e/tests/esphome_satellite.rs`. The Noise handshake was also checked once against an independent responder (`noiseprotocol`). Still to do: run against `aioesphomeapi`'s own voice client and then a real satellite (blocked on the B0 trial and the maintainer's go for any device)

**Exit:** a real ESPHome satellite wakes, a voice turn runs through ClawDot's pipeline, and an inbox announcement gets a spoken reply.

**What changed (2026-11-09).** The earlier plan (own audio and LED ring on Fire OS, staged Alexa removal) was replaced by an unmodified third-party base. The maintainer then decided to own the device layer: one portable daemon, per-device board files and image recipes ([ADR-0015](adr/0015-own-the-device-layer.md)). The ESPHome client is now B8, the install guide is part of B4, "ClawDot on the Dot itself" is the daemon on the image, and the Pi board is B7. The Fire OS route stays a developer path only; the hazards in R-07 section 10 (for example the sysfs file that panics the kernel) still apply to any work on the stock system.

---

# Phase C: Hardening and v1.0 (old M6)
REQ-NFR-004, 010, 108; opt-in REQ-VOI-005, REQ-VOI-011, REQ-NFR-002 tuning

- **30-day soak** on real hardware
- **Budget ratchet** (REQ-NFR-114): hard gates re-set to about 1.5x the measured values, and a **CI budget job** (today a script, not a required job)
- **Fuzzing** of ingress and frames in CI (no fuzz targets exist yet)
- **Signed releases with SBOM**
- Quick start, upgrade procedure and schema migrations, protocol guide for porters
- Opt-in barge-in/AEC evaluation (SPIKE-004); latency tuning
- v1.0 criteria below

*Status: not started* (no soak, ratchet, fuzzing, or release signing; the budget job and the cross-armv7 build are still a `TODO(TASK-003)` in `.github/workflows/ci.yml`). Quick start and upgrade guides do not exist; the protocol guide for porters exists as [architecture/porting-a-satellite.md](architecture/porting-a-satellite.md).

## Beyond v1.0 (planned, in rough priority order)
Release early and often: tag `v0.x` releases as steps land, so the differentiator ships long before v1.0. Items below are *planned for*, meaning v1.0 designs must not preclude them (see [future-scope.md](future-scope.md) for conditions). The board layer and the Echo Dot target, listed here before, are now Phase B.

| # | Item | Why it's next | Designed-for in v1.0 by |
|---|---|---|---|
| 1 | **Agent connect kits** (Claude Code plugin bundling the hook and a skill; Codex, Gemini CLI, opencode adapters) in a separate `clawdot-agent-kit` repo | One-step setup is the adoption hook | Harness-agnostic webhook/SSE; pluggable bridge adapters (INB-020); the Connect-an-agent flow |
| 2 | **ClawDot MCP server** (`announce`, `ask_user`) so agents like Hermes can speak and ask you | Two-way agent interaction | Event `kind` and reply types (INB-003/017) |
| 3 | **Multiple assistant profiles** (e.g. Hermes vs a plain model, each with its own voice) chosen by voice routing, not extra wake words | Matches the "talk to Hermes / Claude / OpenAI" use case | Persona and voice config (AST-013); provider abstraction |
| 4 | **Installable packs** with a permission-review UI, plus an optional curated index | Community-driven integrations | Adapters already use the pack data format (EXT-002/004) |
| 5 | **Wi-Fi provisioner** (captive portal; BLE for ESP32) | Makes it an appliance | Portal is network-independent; mDNS |
| 6 | **Home Assistant HACS integration** and **Wyoming bridge** | HA community reach | Generic webhook, HTTP tools, Wyoming-compatible framing (ADR-0008) |
| 7 | **Willow-compatible front end** (let a Willow device talk to ClawDot) | Reuses an existing open voice front end instead of building one | Satellite protocol; Wyoming-compatible framing; no fork |
| 7b | **Try third-party Echo front ends as satellites** (for example echo-dot-assist's `hassmic`, which makes a Fire OS Echo Dot, Echo or Echo Show look like an ESPHome device): check whether ClawDot's ESPHome client can be their brain without Home Assistant. Untested; their README only describes Home Assistant. Would also cover Echo Show screens | Reuses existing device support, including screens | ESPHome native-API client (B8); display capability (SAT-016) |
| 8 | **`/display` page** and semantic display events for screen speakers | Screens, kiosks | Display capability and `clawdot.display.*` reserved (SAT-016) |
| 9 | **librespot Connect receiver sidecar (research)**: ClawDot's speaker as a Spotify Connect device, as a separate process | Music playing on the device itself | Media-source interface (MUS-003/005). Research only: reverse-engineered library, budget and terms-of-service concerns |
| 10 | Podcasts and audiobooks (resume positions) | Music breadth | Media-source interface |
| 11 | **Zigbee for devices that have a radio** (a Dot-class or Pi board with a Zigbee chip, or a USB dongle): pair and control lights, plugs and sensors directly, as optional tools, without a hub | Local smart-home control without a cloud or Home Assistant | Tools and rules; the portal's Lights and Media pages leave room; no hub (see Not doing) |
| 12 | **Screen devices**: speakers with a display (smart-display class) as first-class satellites, showing the transcript, now playing, timers and cards; touch to stop or pick | Screens are a common target for this kind of assistant | Display capability and `clawdot.display.*` reserved (SAT-016); item 8 is the first step |
| 13 | **Media page in the portal**: music, podcasts and radio providers chosen in one place, with Bluetooth on or off beside them (the page exists as a stub; providers are built later, with items 9 and 10) | One place for everything that plays | Media-source interface (MUS-003/005) |
| 14 | **Realtime voice (optional)**: a speech-to-speech service (a provider's realtime API) as an alternative to the separate listen, think and speak steps, for lower delay. Tools, skills, memory and the ask-first checks stay in ClawDot (the service calls them). Best where ClawDot runs on a PC or Pi, or through a small relay, because the Dot's memory budget is a poor place to hold a streaming connection; billed by audio time, so the spend cap must cover it | Faster, more natural turns | Speech provider abstraction; the portal's Voice page; off unless chosen, never required |
| 15 | **Your own voice (optional, through a provider)**: a voice made from a short recording by a service that offers voice cloning, picked from the voice list like any other (an adapter for services whose list is not OpenAI-style). Training or serving a voice model locally is not planned; it is too rare a case to carry | A voice that is yours or a family member's, with their consent | Voice is already a setting and a list from the speech server |
| 16 | ESP32-S3 satellite (a satellite only, never the brain); scheduled routines such as a morning briefing; rules that act | Platform and feature breadth | Protocol; scheduler; bounded stores |

Moved into Phase A on 2026-11-09: standing "do X when Y" rules (first version tell-me and remind-me only) and opt-in bounded notes are in A5b. Item 2 (the ClawDot MCP server) follows the rules work, because it needs the no-act and scope machinery first.

Hardware notes: the Echo Dot Gen 2 **hardware baseline** is normative (every CI run checks the default configuration against its resource envelope, spec 0002). A Dot with an adb-root or unlocked bootloader runs ClawDot as a normal process beside the stock stack ([R-07](research/07-echo-dot-gen2-hardware.md)); Phase B turns that into a supported install.

## v1.0 criteria
- All Must requirements Implemented and traced
- Default configuration green on the budget gate for armv7 under Dot-class limits (RAM, CPU, storage), plus a recorded run on the closest available real hardware, now the Echo Dot Gen 2 itself
- Desktop and Raspberry Pi satellites supported and conformance-tested
- Threat model reviewed; fuzzing in CI; signed releases with SBOM
- Docs: quick start, protocol guide for porters, agent integration guide, upgrade procedure, [PRIVACY.md](../PRIVACY.md)
- Licence and governance files in place

*Status (2026-11-09): 1 of 6 met (criterion 6), 2 partly met (4, 5), 1 built but unproven (3), 2 not met (1, 2).*

| # | Criterion | State |
|---|---|---|
| 1 | All Must requirements Implemented and traced | Not met. Many Musts are built and tested, but the traceability audit ([report](reports/2026-11-08-traceability-audit.md)) predates this work and every spec is still Draft |
| 2 | Budget gate on armv7 plus a recorded run on real hardware | Not met. `scripts/budget.sh` measures size and RSS; the budget job and cross-armv7 build are a `TODO(TASK-003)` in `ci.yml`. The Echo Dot idle figure (about 1.4 MB RSS) is in A0 but not written up as a run |
| 3 | Desktop and Raspberry Pi satellites supported and conformance-tested | Built but unproven. The conformance suite passes for the desktop, Pi and Dot board files against fakes; no Pi or Dot has run it |
| 4 | Threat model reviewed; fuzzing in CI; signed releases with SBOM | Partly. The [ingress security review](reports/2026-11-08-security-review-ingress.md) is done and its findings fixed (A1); no fuzz targets, no signing, no SBOM |
| 5 | Docs: quick start, protocol guide for porters, agent integration guide, upgrade procedure, PRIVACY.md | Partly. `PRIVACY.md`, the porters guide ([porting-a-satellite.md](architecture/porting-a-satellite.md)), the recipes and the announce, local-speech and PC voice loop guides exist; a quick start and an upgrade procedure do not |
| 6 | Licence and governance files in place | Met. `LICENSE`, `NOTICE`, `SECURITY.md`, `CONTRIBUTING.md`, `AGENTS.md`; the rulesets and signing are not verified from the repo (A0) |

`v0.1.0` is prepared (`CHANGELOG.md`) but no tag has been published.

## Initial backlog (to be created as issues)

| Key | Type | Title |
|---|---|---|
| SPIKE-001 | spike | Hands-on Echo Dot Gen 2 baseline (desk phase done; first hands-on findings in R-07) |
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

**Backlog status (as of 2026-11-09, checked against `origin/dev` at `7fcc110`).** Built, with tests: TASK-004 (`schemas/satellite`, `crates/protocol`), TASK-005 (conformance harness in `crates/e2e`, documented in the satellite spec), TASK-008 (`crates/bridge`; the real `claude` resume is unverified), TASK-010 (`fastpath.rs`), TASK-011 and TASK-012 (`inbox/adapter.rs`, `inbox/reply.rs`, `inbox/summary.rs`), TASK-013 (`media.rs`; fake players only), FEAT-001 (in-process transport, `crates/satellite`), FEAT-002 (`llm.rs`). Partly built: TASK-001 (workspace exists; dev environment validation open), TASK-002 (CI `gate`, Rust, deny and portal jobs run; cross-armv7 still a TODO in `ci.yml`), TASK-003 (`scripts/budget.sh` measures size and RSS; no qemu harness, not a CI gate), TASK-009 and TASK-015 (snippets are generated per endpoint; the Agents tab links to them; the full guided Connect-an-agent flow is not built), TASK-014 (HTTP tools built; the Home Assistant recipe is written ([recipes/home-assistant.md](recipes/home-assistant.md)) but untested against a real server; no Hermes recipe exists). Spikes: SPIKE-002 has its PC finding in [R-11](research/11-wake-word-spike.md) (armv7 open); SPIKE-001's hands-on findings are in R-07 but not closed as a spike. Not found in the repo: SPIKE-003 (armv7), SPIKE-004, SPIKE-005 (size and TLS), SPIKE-006 and SPIKE-007 and SPIKE-008 findings, EPIC-001 (the runtime is built; the evaluation set and provenance workstream are not), TASK-006 (traceability tool; only the one-off audit report exists), TASK-007 (board and labels not verifiable).
