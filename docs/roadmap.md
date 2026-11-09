# Roadmap

Steps are releasable increments with exit criteria, not dates. Order may shift as spikes inform risk. Requirement IDs refer to [spec 0001](specs/0001-product-requirements.md). Ideas outside v1.0 live in [future-scope.md](future-scope.md).

**v1.0 definition:** *Wake → ask → tools; timers, alarms, and reminders; an agent inbox with spoken replies; a tiny footprint within the Echo Dot Gen 2 budget by default; desktop and Raspberry Pi satellites.*

## Principles

1. **The end goal is the Echo Dot Gen 2** (the primary device), but the core stays hardware-agnostic behind the board layer ([ADR-0012](adr/0012-board-support-layer.md)). Device paths, key codes and driver names never enter the core or the reference satellites.
2. **Prove everything on a PC first.** Each capability works end to end on a desktop before any device work starts, so a device problem is never confused with a logic problem.
3. **Agentic workflows are the differentiator.** The agent inbox (spoken announcements of what your coding agents did, and spoken replies back) is what ClawDot is for. Commodity audio parts (wake word, speech to text, text to speech, music control) are reused, not rebuilt.
4. **The resource budget is a feature** ([spec 0002](specs/0002-resource-budget.md)). The default configuration must fit Echo Dot Gen 2 class limits; every new dependency is justified by its size and memory cost.

**Honesty note:** no audio has run anywhere yet. Everything voice-related below is tested against fakes only. The one real-hardware result so far is that the binary starts and idles on an Echo Dot (see the baseline in A0).

## Structure and mapping from the old milestones

Work is now in three phases: **A** (PC loop, hardware-agnostic core), **B** (board layer and the Echo Dot Gen 2), **C** (hardening and v1.0). Other documents (specs, research, reports, ADRs) still say M0 to M6; this table maps the old numbers to the new steps.

| Old | New | Note |
|---|---|---|
| M0 Foundations and feasibility gate | A0 (below); SPIKE-002 and SPIKE-006 now run inside A3 and A2 | The gate table stays here; SPIKE-003 and SPIKE-005 findings remain open in A0 |
| M1 Walking skeleton | A2 + A3 + A4 | Exit AC-1, AC-10 is A4's exit; the budget ratchet moves to C |
| M2 Agent inbox | A5 (voice loop); the inbox itself is built | Exit AC-3, AC-6; ingress fuzzing moves to C |
| M3 Time and local intelligence | A7 (offline mode, eval set, persona); scheduler is built | AC-2, AC-4, AC-7, AC-8, AC-9 are checked in A7 |
| M4 Web UI | A6 | Device auto-detection moves to B1 |
| M5 Tools and music | A7 (radio, recipes) | Radio and MCP/HTTP tools are built |
| M6 Hardening and Raspberry Pi | C; the Pi item is B7 | |
| (new) | A1 process and safety; B0 to B8 device layer and Echo Dot Gen 2 | |

Release mapping in [git-workflow.md](process/git-workflow.md#7-versioning-and-tags) uses the old milestone names; read it through this table.

## A0. Foundations and feasibility gate (was M0)
**Goal:** a credible, enforceable skeleton, and proof that the footprint promise is achievable before we build on it.

Foundations
- [ ] Specs 0001 and 0002 reviewed by the maintainer and Accepted; 0003 completed after SPIKE-005
- [ ] ADRs 0002, 0006, 0008 decided (ADR-0007 licence and ADR-0009 name: done). ADR-0012 (board layer) is Proposed; ADR-0013 (first-run admin setup) exists
- [x] `deny.toml` licence allowlist and `license = "Apache-2.0"` in manifests (TASK-002)
- [x] Repo: Cargo workspace, `rust-toolchain.toml`, `justfile`, `.editorconfig`, `.gitattributes`
- [ ] Dev environment validated ([dev-environment.md](process/dev-environment.md), TASK-001)
- [x] Git flow enforcement: hooks, agent guard, landing and release scripts, CI skeleton with the `gate` job ([ADR-0011](adr/0011-git-flow-merge-commits.md)). Rust, deny, cross-armv7, and budget jobs activate with the workspace (TASK-001..003).
- [ ] GitHub repository created; rulesets applied ([repo-settings.md](process/repo-settings.md)); signed commits once keys exist; Project board configured
  - *Status:* the repository exists with an `origin` remote and the CI `gate` job; `dev` was published through `scripts/land.sh`. Rulesets, signing and the Project board are not verified from the repo.
- [x] SPIKE-001 (desk phase): specs sourced, see [R-04](research/04-echo-dot-gen2.md). The hands-on part has since happened: [R-07](research/07-echo-dot-gen2-hardware.md) records an adb-root Dot.

Feasibility spikes (each ends with a written finding)
- [ ] SPIKE-003: TLS + HTTP client/server footprint on armv7 (x86_64 musl with rustls measured: idle 2.8 MiB; armv7 run still to do; the full binary has since run on a real Echo Dot at about 1.4 MB idle RSS, see the baseline below, but that is not a spike finding)
- [ ] SPIKE-002: wake-word runtime footprint on armv7 (PC evaluation in A3 first)
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

## A1. Process and safety
**Goal:** a faster, safer base before more features pile on.

- [ ] Faster commit checks (branch `chore/faster-commit-checks`, in flight): keep hooks quick without weakening any gate
- [ ] Fix the findings of the [ingress security review](reports/2026-11-08-security-review-ingress.md), in small branches, each with a regression test and a spec line where the report says one is needed:
  - **F1** (High): an unconfirmed `instruct` reply to a shell-capable agent, and unfenced `details` text. Needs confirmation plus fencing and untrusted-text labelling in tool results (a branch `bugfix/inbox-instruct-confirmation` exists, not landed)
  - **F3**: `title` and `status` bypass the prompt fence and are spoken without naming the source
  - **F4**: the LLM client accepts plain `http://` to any host
  - **F2**: idle-socket exhaustion (per-source pre-auth caps) and body stall timeouts (also F9)
  - **F6 / F7**: state files created with mode 0600; no agent text and no one-time tokens on stdout
- Not in this step, tracked in the report: F5 (persistence fsync under lock), F8, F10 to F15

**Exit:** the listed findings fixed and traced; `scripts/check.sh` fast enough that nobody is tempted to skip it.

## A2. Audio interface
**Goal:** one small audio in/out trait, so real sound reaches the core on a PC and later on the Dot without changing the core.

- A trait for capture (16 kHz mono PCM frames) and playback, with **command-backed backends**: on the PC, existing tools (ffmpeg, sox, arecord-like); on the Dot later, `tinycap` and `tinyplay` ([R-07](research/07-echo-dot-gen2-hardware.md) section 6). Spawning tools keeps `unsafe_code = "forbid"` intact and adds no dependency
- A fake backend for tests; underrun and device-loss behaviour defined
- **SPIKE-006 on the PC**: latency, underruns over 10 minutes, RSS delta
- Spec impact: amend REQ-VOI as needed before code

**Exit:** a microphone-to-file and file-to-speaker round trip on a PC through the trait, with a written SPIKE-006 PC finding. (Not yet claimed anywhere: no audio has run.)

*Status: built, not yet run on real audio.* The traits (`AudioIn`, `AudioOut`), the command backend (arecord, aplay, sox, ffmpeg, ffplay from `PATH`), the WAV backend and push-to-talk are in `crates/satellite` (REQ-VOI-013..018), tested with scripted child processes. The manual smoke test is [guides/pc-voice-loop.md](guides/pc-voice-loop.md). Still open: the first real run, and the SPIKE-006 PC measurements (latency, underruns over 10 minutes, RSS delta). The `tinycap`/`tinyplay` presets for the Dot are not written.

## A3. Wake word
**Goal:** hands-free start, reusing an existing model.

- SPIKE-002 on the PC: evaluate existing engines and pretrained models (for example openWakeWord); record licence, model size, RSS and idle CPU. Justify any dependency against the budget; prefer a sidecar or opt-in feature if it does not fit by default
- **Push-to-talk / hotkey fallback** so the loop is usable without a wake word, and as the Dot's button path later
- Custom "ClawDot" word training stays out of v1.0 (EPIC-001 covers integration and the evaluation set only)

**Exit:** a written SPIKE-002 PC finding; wake or hotkey starts a turn on a PC.

*Status:* push-to-talk exists on the desktop satellite (`--ptt`, Enter to talk). The seam for a wake word is `front::Trigger` (REQ-VOI-017): a detector that implements `poll` and reads frames from the same `AudioIn` replaces `LineTrigger`. What remains is everything else in A3: the engine evaluation, a detector, keeping the microphone open between turns (today it opens at the press and closes after the utterance), and the SPIKE-002 finding.

## A4. End-to-end voice turn on a PC (old M1 exit)
REQ-VOI-001..004, 006..010, 012, REQ-AST-001..003, 006, 007, 009, REQ-CFG-007, REQ-NFR-001/003/005/007, REQ-SEC-001..003, 006, 007

- Wake or hotkey, VAD, batch STT, streaming LLM, sentence-level TTS, half-duplex, through the real A2 audio backends and a real provider
- First budget measurements with the real pipeline

**Exit:** AC-1 and AC-10 pass on a PC.

*Status: partly built.* Done and tested against fakes: in-process and remote (TCP) transport, voice dialog state machine, energy-based endpointing, STT/TTS clients, streaming LLM client, half-duplex, JSON config file and the reference satellite (`crates/core/src/{voice,vad,speech,satellite,transport,config}.rs`, `crates/satellite`, [voice-pipeline](architecture/voice-pipeline.md)). The satellite frame JSON Schemas (`schemas/satellite`, TASK-004) and the conformance harness that checks the reference satellite against a fake core (TASK-005) are done. Real microphone and speaker I/O now exists through the desktop satellite (A2 status) but has not run on real audio. Not done: wake-word detection (A3), a real STT/TTS provider run, and the ratchet. AC-1 and AC-10 are not demonstrated anywhere.

## A5. Agent loop by voice with Claude Code (old M2 exit)
REQ-INB-001..018, 020, REQ-SEC-004, REQ-EXT-004, 006

- Minimal HTTP ingress, endpoints and tokens, queue, announce/dismiss/snooze/reply; format adapters (Claude Code native `http` hook first), filtering and priority, reference agent bridge; SSE/poll sources; "what are my agents doing?"; integration snippets (Claude Code hooks, shell, GitHub Actions)
- The remaining work is the spoken half: a real Claude Code session finishes, the PC speaker announces it, a spoken reply resumes the session

**Exit:** AC-3 and AC-6 pass; a PC demo of "Claude finishes → speaker announces → voice reply resumes the session".

*Status: mostly built, voice half missing.* Ingress, endpoints with hashed tokens, queue, replies over SSE, adapters (`claude-code`, `generic`), summaries, the bridge that resumes Claude Code, and per-endpoint snippets exist (`crates/core/src/inbox/`, `crates/bridge`), with the headline loop tested over real sockets (`crates/e2e/tests/agent_loop.rs`, `satellite_loop.rs`). Not done: the real `claude` CLI resume has not been verified, and there is no live demo. Fuzzing of ingress moves to Phase C. A5 depends on the A1 fixes (F1 in particular) before it is enabled by default.

## A6. Web UI (old M4)
REQ-CFG-001..006, 008, 009, 012, 013, REQ-INB-019, REQ-NFR-006

Landed already: first-run admin setup (choose an admin password or deliberately leave the UI open on a private network, with an "unprotected" banner; private-address-only setup, no setup code; `--reset-admin` recovery; REQ-CFG-018..024, [ADR-0013](adr/0013-first-run-admin-setup.md)), sign-in with rate limiting, the UI theme and logo set, and the default listen address `127.0.0.1:3335` (REQ-CFG-014, chosen so as not to collide with Amazon's `oobed` on 8080). The embedded UI is one hand-written file plus a theme CSS; a TypeScript stack is only a [proposal](design/ui-stack.md).

Still to do:
- **Settings page** for everything now set by flags or config files: time zone (pre-filled from the browser, not from IP geolocation), language-model URL, key and models, persona and voice, listen address and public URL, usage limits, admin password, and board overrides. Secrets are write-only in the UI; changes apply or say plainly that a restart is needed
- **Connect an agent** flow (per-harness snippets, token, bridge command)
- **First-run checklist** on the dashboard: Wi-Fi and clock OK, admin set, language model reachable, first agent connected, test speak, each with a one-click fix or link
- Endpoint management, history, test speak, health
- mDNS (`clawdot.local`); headless Wi-Fi docs for Pi
- Webhook tokens stay required whether or not the UI is protected; no multi-user accounts

Hardware auto-detection and the board picker are in B1; the settings page only gets a board section once B1 exists.

**Exit:** a new user can set up from the UI alone.

## A7. Remaining local intelligence and integrations (old M3 and M5)
REQ-TMR-001..008, REQ-AST-004, 010..013, REQ-NFR-008, 009, 111; REQ-MUS-001..006, REQ-AST-005, 008, 014, REQ-CFG-010, REQ-EXT-005

Local intelligence (old M3)
- Offline degraded mode; tool-calling eval set and tested-models list; persona config as specified (REQ-AST-013)
- *Status: partly built.* Scheduler (timers, alarms, reminders, DST-aware), atomic versioned JSON store, fast-path matcher, usage limits and a clock-unset guard exist and are tested (`crates/core/src/{sched,store,fastpath,usage}.rs`; commits 9ce871d, db8655f). The AC pass/fail status is not recorded.

Announce and integrations
- A **one-way announce/say API and CLI** (speak this text, no reply expected), the simplest integration for scripts and automations. It reuses the inbox's untrusted-text handling (A1)
- **Off-device bridges as sidecars**: Discord and ntfy, kept out of the core binary so the default build stays in budget
- Recipes: **Spotify control** (a Connect device is controlled; audio plays there, not on ClawDot's speaker) and **Home Assistant**; Hermes recipe verified against its docs at this point
- **Internet radio** and ducking, media-source interface, MCP client (Streamable HTTP), declarative HTTP tools with confirmation. *Status: partly built.* Radio by voice with ducking, the media-source model, the MCP client and HTTP tools exist (`crates/core/src/{media,mcp,httptools}.rs`; [media](architecture/media.md), [mcp](architecture/mcp.md), [http-tools](architecture/http-tools.md)), tested only against fake servers and players. The Spotify, Home Assistant and Hermes recipes are untested
- **Willow and Home Assistant / Wyoming satellites as optional front ends**: they interoperate through the satellite protocol and the bridge; we do not fork them (see Beyond v1.0)

**Exit:** AC-2, AC-4, AC-7, AC-8 and AC-9 pass; one announce path and one music path demonstrated on a PC.

---

# Phase B: Board layer and the Echo Dot Gen 2

*Reworked 2026-11-09 from R-08 section 9, then revised the same day for the maintainer's decision to own the device layer ([ADR-0015](adr/0015-own-the-device-layer.md), Proposed): one portable device daemon on every Linux board, a board file and an image recipe per device. The earlier plan (own audio and LEDs on Fire OS) is superseded; see the note at the end of this phase. Nothing here is decided beyond what the maintainer approves; the open points are in [open-questions.md](open-questions.md).*

Starts after the PC loop works (A4 and A5 at least) and the feasibility gate for the device spikes is Go. The sequence: finish the PC voice loop; build the portable daemon on PC and Pi (B2) so the contract is real; run the TECHO5 trial (B0, which needs the maintainer and can run beside B2) to learn the Dot; spike the wake word and echo canceller on the Dot (B3) and pass the [ADR-0015](adr/0015-own-the-device-layer.md) gate; then bring up the Dot on T5D's OS image with our daemon (B4) and add images per board. Hardware facts are in [R-07](research/07-echo-dot-gen2-hardware.md), the unlock history in [R-09](research/09-echo-unlock-history.md), and the existing Dot projects in R-08. The device layer decision is ADR-0015; the board design is [ADR-0012](adr/0012-board-support-layer.md), [board-support](architecture/board-support.md) and spec REQ-BRD (all Proposed). **Every step that changes the device, and every single device write inside a step, needs the maintainer's explicit go. Stop and ask before each one.**

The maintainer's unit is a Dot 2 that is already unlocked: rooted Fire OS with TWRP. It is the test unit for B0 and the entry state for B3.

## B0. Trial of the TECHO5 Dot base image (spike)
Goal: learn the hardware and the image and update process from a working Dot image (TECHO5), so ClawDot's own daemon and board file can be built from its findings. B0 is the gate input for [ADR-0015](adr/0015-own-the-device-layer.md); it is learning from unmodified TECHO5, not choosing a permanent front end.

- Trial the TECHO5 Dot (T5D) base image on the maintainer's unit, which is rooted Fire OS with TWRP. Installing it writes to the recovery partition and replaces the system, so **each device write waits for the maintainer's explicit go**
- Before any write: full partition backups taken and checked, and the installer's `--dry-run` read first
- **Rollback path, recorded before the trial:** recovery is the XDA procedure ([Unlock, root, TWRP, unbrick Echo Dot 2nd gen "biscuit"](https://xdaforums.com/t/unlock-root-twrp-unbrick-amazon-echo-dot-2nd-gen-2016-biscuit.4761416/)) with the TWRP backup and the partition backups from this step, restoring the rooted Fire OS state ([R-09](research/09-echo-unlock-history.md)). Restoring is proved on paper before the first write and written into the spike finding
- Measure idle RSS, CPU and boot time; check whether the base's `echod` serves an ESPHome client that is not Home Assistant (voice subscribe, announce, ring entity); check how the Noise key is handled
- This trial is also how Alexa gets wiped: **removing Alexa is achieved by installing the base image, not by our own staged removal on Fire OS**, and every step of it is gated on the maintainer's go

**Exit:** a written spike finding that records mixer routes, LED format, button codes, channel map, the image and update flow, and idle figures, as input to the B3 gate. (The "Dot front end base" ADR proposed in R-08 is replaced by ADR-0015.) If T5D cannot be installed or restored safely, evaluate EchoMuse or LibreEcho instead.

## B1. Board crate (reduced)
REQ-BRD-001..014

- `crates/board`: the `Board` trait, `NullBoard`, a desktop board, shared gesture and mapping code, the JSON data-file schema (board files are JSON, decided 2026-11-09)
- **Hardware auto-detection:** `board = "auto"` fingerprints the hardware against each compiled-in board's `detect` rules (device tree, `/proc/cpuinfo`, sysfs, `/proc/idme/device_type_id` on the Dot). A single confident match selects it. **With no confident match the board stays unset and the settings page asks the user to pick from a list, showing the detection evidence.** Users never edit mapping files unless they want to
- **The Dot board is data only until the base trial (B0) is done:** names, key codes, the LED `frame` format and a `forbidden` list of paths the board must never touch, including `power_button_state` and `calibrated_lux`. No Dot code that opens audio, LED or button devices
- ADR-0012 moved from Proposed to **Accepted**; the settings page gets its board section
- Default build unaffected (REQ-NFR-115); a CI feature matrix (`default`, `desktop`)

**Exit:** the desktop board drives the core on a PC through the trait; ADR-0012 Accepted; budget impact measured, not estimated.

## B2. Device contract and the portable daemon, on PC and Pi first
REQ-DEV-001..012 (proposed in [ADR-0015](adr/0015-own-the-device-layer.md); a new spec `0006-device-layer-contract.md` lands with this step)

- The **device layer contract** as a spec, the **board-file JSON Schema** (mixer routes, LED frame format, button codes, mic channel map, forbidden paths) and a **device conformance suite** that runs against a fake board on a PC
- The **portable device daemon** (Rust): ALSA in and out, tones, ducking, buttons and LED states driven only from the board file, local stop without the wake word, quiet-hours hook, native satellite protocol. **No per-device code paths**
- Echo cancellation and noise suppression behind **one isolated crate** that wraps a proven C library (WebRTC audio processing or SpeexDSP); it is the only place allowed `unsafe`; notices in `NOTICE`
- Runs first on the PC and then on a Raspberry Pi through board files, so the contract is exercised before any Dot work. Spec 0003 amendments (logical LED states, tone, quiet hours, update messages) as minor `clawdot.` additions
- The default core build and budget are unaffected (REQ-NFR-115)

**Exit:** the PC and the Pi run the full voice loop through the daemon and a board file, and the conformance suite passes against a fake board.

## B3. Daemon feasibility spike on the Dot (the main unknowns)
- Measure on the Dot (maintainer's go for every device write): wake-word engine CPU and RSS (research says about 23 to 30 percent of one core), echo-canceller cost on the Cortex-A53, capture-to-wake latency, with the base OS idle figures from B0
- Pick the wake-word runtime: pure Rust, then an engine already in the base, then a statically linked C runtime behind the isolated crate (ADR-0014 point 4). Models only with recorded licence provenance; no pretrained openWakeWord models are bundled
- Decide whether AEC fits at all on the Dot; if not, barge-in stays off and the board file says so

**Exit:** a written spike finding and the **go/no-go gate of [ADR-0015](adr/0015-own-the-device-layer.md)**. Go moves the ADR toward Accepted; no-go falls back to a fork of `echod` (kept in the image repository) or to the unmodified-base route in B8.

## B4. Image repository and Dot bring-up
- **The maintainer creates the image repository** (working name `clawdot-os`); no agent creates repositories. This step starts only after the gate in B3 is Go
- The **Echo Dot 2 recipe** starts from T5D's OS image (kernel, rootfs, install and update flow, MIT, notices kept) with **our daemon in place of `echod`**, and a board file built from T5D's findings (mixer routes, LED format, button codes, channel map). Strip what we do not want; brand it
- Track upstream T5D; send generic fixes back; `NOTICE` credit; **never redistribute Amazon files or unlock tooling**, and never a boot image containing Amazon's kernel (publish the recipe)
- **docs/guides/install-on-echo-dot-2.md** plus a guide template for other devices; starts from the already-unlocked state, links the unlock procedure, pins the firebreak version, credits the people involved (R-09 section 9), and keeps the restore path from B0

**Exit:** a fresh unlocked Dot reaches a voice turn through our daemon following only the guide.

## B5. Image build and signed A/B update pipeline
REQ-DEV-007

- Reproducible image build, a project signing key (held by the maintainer) and a **signed A/B update with automatic rollback** on the Dot partition layout (or the documented `update: manual` alternative, if the layout allows no A/B)
- The conformance suite gains update tests (bad signature rejected, failed boot rolls back)
- Wi-Fi setup without a keyboard and clock trust on the image

**Exit:** an update is signed, installed and confirmed on the Dot, and a deliberately broken update rolls back on its own; recorded run.

## B6. Device features through the daemon
- LED states for agent events on the ring (`inbox`, `listening`, `thinking`, `speaking`, `error`, `timer`, `updating`), the documented equivalent on boards without a ring
- Buttons and gestures, mute with its honest kind, **stop without the wake word**, follow-up listening, **quiet hours** applied locally
- **Wake word** with licence-clean models and recorded provenance (EPIC-001); **Bluetooth speaker mode** (A2DP sink and source, arbitration with announcements measured in B0)
- **Spotify Connect receiver** as an opt-in with the terms-of-service notice; Spotify *control* through the brain is a software feature and needs no device work
- Anything the hardware cannot do is a documented gap, not a faked capability

**Exit:** the Dot shows the agent states, honours stop and quiet hours without the brain, and the recorded conformance run on the unit is green for every Must.

## B7. Pi and PC parity, and more boards
- Raspberry Pi image recipe and install guide from the B4 template; PC satellite (the daemon in a desktop package). Both pass the same conformance suite with the same daemon logic and only a different board file and recipe
- A third board (for example a smart-calendar-class device) is the test of the claim "the only per-device difference is how you install it": a board file, a recipe, a passing run, and no daemon change. Boards below the minimum contract (not Linux-capable, too little RAM, no microphone or speaker) are documented as unsupported

**Exit:** Pi and PC satellites pass the conformance suite; real-hardware run recorded; the third board needs no daemon code change.

## B8. ESPHome native-API client (compatibility mode)
Goal: third-party ESPHome voice satellites (stock T5D or LibreEcho images, a Pi running linux-voice-assistant) can use ClawDot as their brain without Home Assistant. Not the path for our own devices. Spec-level item: see the [future-scope](future-scope.md) row and R-08 sections 4.2 and 4.3.

- A new optional core feature (`esphome-satellite`) behind the existing satellite interface: frame layer, minimal hand-written protobuf for about 20 messages, voice session, announce-then-listen for the inbox, media player play, optional light command. Estimate about 1,100 to 1,300 lines; not in the default Dot budget
- Optional Noise encryption on `ring`, with no `snow` and no `prost`; tested against a fake ESPHome server on a PC first, then `aioesphomeapi`
- **Status (2026-11-09):** the client is built behind the cargo features `esphome-satellite` and `esphome-noise` ([spec 0004](specs/0004-esphome-satellite-client.md), [architecture/esphome-satellite.md](architecture/esphome-satellite.md)) and tested against the fake device in `crates/e2e`. The Noise handshake was also checked once against an independent responder (`noiseprotocol`). Still to do: run against `aioesphomeapi`'s own voice client and then a real satellite (blocked on the B0 trial and the maintainer's go for any device)

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

*Status: not started* (no soak, ratchet, fuzzing, or release signing).

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
| 8 | **`/display` page** and semantic display events for screen speakers | Screens, kiosks | Display capability and `clawdot.display.*` reserved (SAT-016) |
| 9 | **librespot Connect receiver sidecar (research)**: ClawDot's speaker as a Spotify Connect device, as a separate process | Music playing on the device itself | Media-source interface (MUS-003/005). Research only: reverse-engineered library, budget and terms-of-service concerns |
| 10 | Podcasts and audiobooks (resume positions) | Music breadth | Media-source interface |
| 11 | ESP32-S3 satellite; routines (e.g. morning briefing); long-term memory | Platform and feature breadth | Protocol; scheduler; bounded stores |

Hardware notes: the Echo Dot Gen 2 **hardware baseline** is normative (every CI run checks the default configuration against its resource envelope, spec 0002). A Dot with an adb-root or unlocked bootloader runs ClawDot as a normal process beside the stock stack ([R-07](research/07-echo-dot-gen2-hardware.md)); Phase B turns that into a supported install.

## v1.0 criteria
- All Must requirements Implemented and traced
- Default configuration green on the budget gate for armv7 under Dot-class limits (RAM, CPU, storage), plus a recorded run on the closest available real hardware, now the Echo Dot Gen 2 itself
- Desktop and Raspberry Pi satellites supported and conformance-tested
- Threat model reviewed; fuzzing in CI; signed releases with SBOM
- Docs: quick start, protocol guide for porters, agent integration guide, upgrade procedure, [PRIVACY.md](../PRIVACY.md)
- Licence and governance files in place

*Status:* none of these is fully met. Licence (`LICENSE`, `NOTICE`) and `PRIVACY.md` exist; the budget gate runs as a script, not yet as a required CI job, and the first real-hardware measurement is not yet recorded as a run; fuzzing, signed releases with SBOM, the Raspberry Pi satellite and the quick start are not done. `v0.1.0` is prepared (`CHANGELOG.md`) but no tag has been published.

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

**Backlog status (as of 2026-11-09).** Built, with tests: TASK-004 (`schemas/satellite`, `crates/protocol`), TASK-005 (conformance harness in `crates/e2e`, documented in the satellite spec), TASK-008 (`crates/bridge`), TASK-010 (`fastpath.rs`), TASK-011 and TASK-012 (`inbox/adapter.rs`, `inbox/reply.rs`, `inbox/summary.rs`), TASK-013 (`media.rs`), FEAT-001 (in-process transport, `crates/satellite`), FEAT-002 (`llm.rs`). Partly built: TASK-001 (workspace exists; dev environment validation open), TASK-002 (CI `gate`, Rust and deny jobs run; cross-armv7 still a TODO in `ci.yml`), TASK-003 (`scripts/budget.sh` measures size and RSS; no qemu harness, not a CI gate), TASK-009 and TASK-015 (snippets are generated per endpoint; the full Connect-an-agent UI flow is not), TASK-014 (HTTP tools built; Home Assistant and Hermes recipes untested). Not found in the repo: SPIKE-002..008 findings, EPIC-001, TASK-006 (traceability tool), TASK-007 (board and labels not verifiable). SPIKE-001's hands-on findings are in R-07 but not closed as a spike.
