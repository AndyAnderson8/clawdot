# ADR-0015: Own the device layer: one portable device daemon, per-device board files, per-device image recipes

- **Status:** Proposed (go/no-go gate after the daemon spike and B0)
- **Date:** 2026-11-09
- **Deciders:** Maintainer
- **Refines:** [ADR-0005](0005-satellite-core-separation.md), [ADR-0012](0012-board-support-layer.md), [ADR-0014](0014-language-rust-core-polyglot-edges.md)
- **Replaces (as a plan):** the "ADR-0013, Dot front end base" proposal named in [R-08](../research/08-existing-echo-dot-projects.md) section 10 and the roadmap B0 exit. ADR-0013 is taken by first-run admin setup, so that proposal is replaced by this one.

## Context

[R-08](../research/08-existing-echo-dot-projects.md) found that TECHO5 Dot (T5D, MIT) already drives the Dot's audio, LED ring, buttons, Wi-Fi and clock, and exposes the speaker as an ESPHome voice satellite. Its recommendation, and the current Phase B, is to treat that image as an unmodified third-party front end and attach to it with an ESPHome client in the core (B2). [R-10](../research/10-libreecho-evaluation.md) evaluated LibreEcho as a third candidate.

The maintainer weighed that plan against the alternatives on 2026-11-09 and rejected "unmodified third-party daemon, permanently". His reasons:

- The brain must be hardware-agnostic, and the **same experience** must be under ClawDot's control on every device: Spotify through Connect, LED states for agent events, buttons, wake word, quiet hours, stop without the wake word, and updates.
- Designing the product around what a third-party daemon exposes is "designing the car around the wheel". Features would be limited to what an upstream author builds, in his order and on his schedule.
- He accepts the extra effort and wants the result maintainable and professional.

His final direction is a stronger form than "fork T5D": the device layer is **one portable daemon that runs on every Linux board**, and a device is described by **data plus an install recipe**, not by code. The goal in his words: *develop once, it works; the only per-device difference is how you install it.*

Facts that shape the decision:

- T5D is MIT, Go, small, and updated almost daily ([R-08](../research/08-existing-echo-dot-projects.md) sections 2 and 6). Its OS image (kernel, rootfs, installer, update flow) and its hardware findings (mixer routes, LED format, button codes, channel map) are reusable under MIT with notices; its `echod` is the thing we would replace.
- The satellite protocol ([spec 0003](../specs/0003-satellite-protocol.md)) already carries audio, wake, buttons, mute, indicator and volume (REQ-SAT-006/007), is Wyoming-compatible (REQ-SAT-011) and has capability negotiation (REQ-SAT-003, REQ-SAT-013).
- Echo cancellation, a wake-word engine and ALSA capture on a 32-bit Cortex-A53 are the hard parts. [R-08](../research/08-existing-echo-dot-projects.md) measured about 23 to 30 percent of one core for wake word on the Dot; the engine and the CPU fit are the **main unknowns**.
- The mature audio DSP is C (WebRTC audio processing, SpeexDSP). The workspace forbids `unsafe` ([ADR-0002](0002-implementation-language-rust.md)), so that code must be isolated.
- Hardware bring-up on the Dot is possible only with the maintainer present: every device write needs his go ([B0 trial plan](../guides/b0-techo5-trial-plan.md)).

## Decision drivers

- One experience on every board, owned by ClawDot, not by a third party's roadmap.
- A single code path to test and maintain: the same daemon on a PC, a Raspberry Pi and a Dot, so "works on the PC" means something.
- A core that stays hardware-agnostic and Rust, with device concerns behind a stable contract.
- One toolchain for the whole family (Rust), with unsafe C isolated in one crate.
- Legal cleanliness: MIT notices kept, no Amazon files, no unlock tooling redistributed, no non-commercial models bundled.
- A maintenance burden one maintainer can carry, and a cheap exit.

## Options considered

1. **Unmodified third-party daemon behind an ESPHome adapter, permanently (previous plan).** Least code. Rejected as the end state: each ClawDot feature (LED states for agent events, quiet hours, stop without the wake word, Spotify Connect, our own updates) would be an upstream request or a workaround, and the protocol in between has no ClawDot concepts. **Kept as a compatibility mode** for third-party satellites.
2. **Fork `echod` (Go) into a separate device repository and extend it.** The first draft of this ADR. Fast to start and legally simple, but it ties the design to one board's daemon, adds a second language and toolchain permanently, and gives a PC and a Pi a different code path from the Dot, so the contract would not be exercised until late. **Kept as the fallback** if the Rust daemon proves infeasible on the Dot.
3. **LibreEcho or EchoMuse as the base.** LibreEcho ships a GPL-2.0 kernel and platform tree, an FPGA blob with no stated licence, a non-commercial default wake-word model ([R-10](../research/10-libreecho-evaluation.md) section 6), a small community and a heavier install. EchoMuse splits work between a Go device layer and a Python controller tied to its controller ([R-08](../research/08-existing-echo-dot-projects.md) section 6). Neither is a better base than T5D; both remain research references and B0 fallbacks.
4. **Bare metal or ESP32 as the device.** Cheap, but it is not the Echo Dot Gen 2, and ESP32-class parts cannot run the brain. An ESP32 may act as a satellite through the protocol or the ESPHome compatibility mode, but it is **not supported by the device daemon** (see the minimum contract below).
5. **One portable Rust device daemon, board files, image recipes (chosen).**

## Decision

### The layers

```
ClawDot brain (Rust, this repository)
        ^  ClawDot satellite protocol (spec 0003)
portable device daemon (Rust): ALSA in/out, echo cancellation, wake word,
        LEDs and buttons driven by the board file; the same code on every board
        ^  board file (JSON per device: mixer routes, LED format, button codes, mic channel map)
image recipe per device: kernel, bootloader, install, signed A/B updates
```

1. **The brain stays in this repository**, Rust, hardware-agnostic, seeing devices only through the satellite interface (ADR-0005). The ESPHome adapter (B2) stays as an optional **compatibility mode** for third-party satellites; it is no longer the primary path to our own devices.
2. **The portable device daemon** is Rust and is part of the ClawDot repository family so there is one toolchain, one CI and one set of protocol crates. It runs on any Linux board that meets the minimum contract. It implements the satellite protocol **natively** and owns audio capture and playback, echo cancellation, the wake-word engine, LED and button handling, tones, ducking, quiet-hours behaviour and local stop. It contains **no per-device code paths**: everything that differs between boards is read from the board file.
3. **Board files** are JSON data per device (extending the format of [ADR-0012](0012-board-support-layer.md): key codes, LED state-to-pattern mapping, forbidden paths) with the daemon-level additions: ALSA device names and parameters, **mixer routes**, the **LED frame format**, **button codes** and the **mic channel map** (which channel is microphone, which is reference). They are reviewable and testable without hardware.
4. **Image recipes** are per device and hold everything that cannot be portable: kernel, bootloader, install procedure, partitioning, and **signed A/B updates with rollback**. They live in a separate repository, working name `clawdot-os`, **to be created by the maintainer** (this repository does not create it). The Echo Dot 2 recipe starts from **T5D's OS image (kernel, rootfs, install and update flow, MIT, notices kept) with our daemon in place of `echod`**, and a board file built from T5D's findings. Further recipes (a Pi image, a smart-calendar-class device) follow.
5. **Where the daemon's source lives** (a workspace in this repository, or a sibling repository inside the ClawDot family) is left to the maintainer; this ADR requires only that it be Rust, share the protocol crates and carry no per-device code. Proposed default: crates in this workspace behind their own package set so the core's default build and budget are unaffected (REQ-NFR-115).
6. **Language.** The daemon is **Rust**. Proven C DSP (WebRTC audio processing or SpeexDSP for echo cancellation and noise suppression) is wrapped in **one isolated crate** that alone is allowed `unsafe`, linked statically, with the notice in `NOTICE`. T5D's Go source is a **reference, not a target**. A fork of `echod` (Option 2) remains **only a fallback** if the Rust daemon proves infeasible on the Dot.
7. **Upstream and licences.** T5D and `echod` (MIT) are credited in `NOTICE` for anything copied or ported (project, commit, files, copyright holder). We never redistribute Amazon files or unlock tooling; we do not redistribute a boot image containing Amazon's kernel (publish the recipe and the GPL sources as T5D does). Hardware facts we learn are contributed back upstream where generic.
8. **Go/no-go gate** after the daemon spike and B0 (see the end). This ADR stays **Proposed** until it passes; nothing is forked or created before.

### Same software, hardware where it exists

- **Software features are identical on every board:** agent loop, tools, MCP, skills, inbox, Spotify control, timers, quiet hours, the portal, updates. They live in the brain or in the daemon's common code, so no board can lack them.
- **Hardware features appear where the hardware exists**, through capability negotiation (REQ-SAT-003). Each has a **documented equivalent elsewhere**: the LED ring's states map to a single LED, to a status line in the portal, or to a tone on a board with no LED; a display shows semantic events (REQ-SAT-016) where present and the portal elsewhere.

### Minimum contract for support

A board is supported by the daemon only if it is Linux-capable, has enough RAM for the daemon plus the base OS inside the budget ([spec 0002](../specs/0002-resource-budget.md)), and has at least one microphone and one speaker. **ESP32-class parts are not supported for the daemon** (they can still attach as ESPHome or Wyoming satellites in compatibility mode). A board that fails the minimum contract is unsupported, not partially supported.

### The device layer contract

What the daemon and image together provide on every supported board. Absent hardware features are declared, never faked.

| Area | Requirement | Where it lives |
|---|---|---|
| Audio in | 16 kHz 16-bit mono PCM in 20 ms frames (REQ-SAT-004); the channel map selects or mixes the microphone channels and the reference | Daemon + board file |
| Echo cancellation and noise suppression | Software AEC against the playback reference where the board can afford it; otherwise declared absent and barge-in stays off (REQ-VOI-005) | Daemon (isolated C crate) |
| Audio out | PCM and Opus (REQ-SAT-005), start/stop, volume, **ducking** (lower other audio while the agent speaks, restore after) | Daemon + board file (mixer routes) |
| Tone | Built-in short tones (wake acknowledge, error, timer, inbox chime) played locally, so feedback works on a slow link | Daemon |
| LED states | Fixed logical set: idle, listening, thinking, speaking, inbox waiting, muted, error, timer, updating, setup. Mapped by the board file; a board without LEDs uses the documented equivalent | Daemon + board file |
| Buttons | Logical buttons and gestures (press, long press, double press) as events (REQ-SAT-006) | Daemon + board file (button codes) |
| Mute | A mute control with an honest kind: none, software, hardware (REQ-BRD-008); never claim "disconnected" for software mute | Daemon + board file |
| Stop / cancel | **Stop playback and cancel the turn without the wake word**: a button gesture and a protocol command, acted on locally first | Daemon |
| Follow-up listening | Reopen the microphone for a bounded window after a reply that expects an answer | Daemon |
| Quiet hours | Hook applied locally: LED dimming, volume ceiling, announcement deferral, even with the core unreachable | Daemon |
| Wake word | On-device engine where the CPU allows, otherwise core-side (REQ-SAT-013); models with recorded provenance and a compatible licence (REQ-VOI-009) | Daemon |
| Bluetooth speaker mode | Optional: A2DP sink and source; arbitration with announcements defined | Daemon + image |
| Spotify Connect receiver | Optional, off by default. **Terms-of-service caveat:** free Connect receivers rely on unofficial access to Spotify, which may breach its terms or stop working without notice; shown on enabling and never a dependency of the core experience. Spotify *control* (play, pause, search through the brain) is a software feature and is identical everywhere | Daemon or sidecar |
| Updates | **Signed A/B updates with rollback**: verify against a project key, install to the inactive slot, boot once, confirm or roll back automatically; a board that cannot do A/B declares `update: manual` | Image recipe |
| Wi-Fi setup | Join a network without a keyboard (setup access point or Bluetooth page), status, forget | Image recipe + daemon |
| Clock | Clock trust (unset, unverified, synced) reported; no timers fire on an untrusted clock (TMR-009) | Daemon |
| Transport | Satellite protocol with token authentication (REQ-SAT-010) and TLS where the network is untrusted; reconnect with backoff (REQ-SAT-008) | Daemon |
| Safety | Never log secrets or audio; forbidden-paths list from the board file (REQ-BRD-013) | Daemon + board file |

#### Proposed spec text (to land with a spec amendment)

Spec files are being edited elsewhere in parallel, so the text lives here and moves into a new spec (working name `0006-device-layer-contract.md`) when this ADR is accepted. Area code `DEV`.

- REQ-DEV-001 (Must): the device daemon MUST run unchanged on every supported board; all per-device differences MUST be in the board file or the image recipe.
- REQ-DEV-002 (Must): the daemon MUST implement the satellite protocol natively and declare every capability it lacks.
- REQ-DEV-003 (Must): audio in and out, tone, volume and ducking as in the table.
- REQ-DEV-004 (Must): the logical LED state set is fixed and versioned; unknown states MUST be ignored; every state MUST have a documented equivalent on a board with no LEDs.
- REQ-DEV-005 (Must): stop and cancel MUST work without the wake word and MUST act locally before the core confirms.
- REQ-DEV-006 (Must): quiet-hours settings MUST be applied locally.
- REQ-DEV-007 (Must): updates MUST be signed, and a failed update MUST roll back without user action where the board supports A/B; otherwise the board declares `update: manual`.
- REQ-DEV-008 (Must): a board file MUST be validated against a published JSON Schema and MUST NOT contain commands assembled from text.
- REQ-DEV-009 (Should): follow-up listening, Bluetooth speaker mode, Wi-Fi setup and clock trust as in the table.
- REQ-DEV-010 (May): Spotify Connect receiver, off by default, with the terms-of-service notice.
- REQ-DEV-011 (Must): the project MUST provide a device conformance suite that runs against the daemon with a fake board on a PC and, for each supported board, on real hardware; a board is "supported" only when every Must test passes and the result is recorded.
- REQ-DEV-012 (Must): a board that is not Linux-capable, lacks a microphone or speaker, or cannot fit the budget MUST be documented as unsupported.

#### Relation to existing documents

- **ADR-0012:** the `Board` trait, `NullBoard`, the desktop board, gesture and mapping code, auto-detection and the JSON file format remain, and become part of the daemon's common code. The `board-dot2` code module (button readers, `ledctrl` routes) is never built: the Dot is a board file plus an image recipe. The ADR-0012 amendment of 2026-11-09 stands.
- **Spec 0003:** gains the ClawDot extensions the contract needs as minor, ignorable `clawdot.` additions (logical LED states in `clawdot.set-indicator`, a local tone command, quiet-hours and update messages) per REQ-SAT-009 and REQ-SAT-011. Proposed in the spec amendment, not here.

## Consequences

**Positive:**
- The product experience is specified and tested by us and identical across boards; no dependency on an upstream author's roadmap.
- "Works on the PC" is real evidence: the Pi and PC run the same daemon, so the contract is forced to be real before any Dot work.
- One toolchain (Rust) and the existing protocol crates; board work is mostly data.
- The conformance suite makes a new board a bounded job: a board file, an image recipe, a passing run.

**Negative / costs and risks:**
- **The main unknowns are technical.** The wake-word engine and the CPU fit on the Dot (research measured about 23 to 30 percent of one core) and echo cancellation cost on a Cortex-A53 are not proven. A spike decides them before commitment (roadmap B2).
- **Maintenance burden.** A daemon, an image repository, signing keys, image builds, and a stream of upstream (T5D) changes to read and port. Mitigations: board files as data, a small daemon, a conformance suite in CI, upstream followed on a schedule.
- **AEC and wake word ownership.** There is no hardware AEC on the Dot; barge-in stays opt-in. The DSP is C inside one isolated `unsafe` crate: it needs its own review, a build for armv7 musl, and a size and memory measurement (ADR-0002 budget).
- **Hardware bring-up needs the maintainer present.** Each flash, recovery and listening test on the unit is his; CI cannot cover it (spec 0002 section 5).
- **Bus factor.** One maintainer plus agents; mitigated by documentation (rebuild recipe, recovery guide) and a small codebase.
- **Licences.** Kernel and busybox are GPL-2.0: separate programs, source offered, never linked into the Apache-2.0 daemon; no image of Amazon's kernel redistributed. **openWakeWord pretrained models are CC-BY-NC-SA-4.0 and non-commercial**: never bundled. WebRTC (BSD-3) and SpeexDSP (BSD-3) need `NOTICE` entries. Piper voices carry their own licences. Spotify Connect receivers carry terms-of-service risk.
- **Dependence on the unlock.** The Dot path rests on a rooted, unlocked unit and community tooling we do not own.
- **Effort lands before features.** The daemon, recipes and signing pipeline precede the first device-only feature.

**Follow-ups:**
- Run the daemon spike and B0; record the finding against the gate.
- If Go: the maintainer creates `clawdot-os`; draft `0006-device-layer-contract.md`, the spec 0003 amendment and the board-file schema.
- After acceptance, amend ADR-0012 and the R-08 recommendation text to point here; add `NOTICE` entries when code is first ported.

## Exit criteria and go/no-go gate

Evaluated by the maintainer after the daemon spike (B2) and the TECHO5 trial (B0) are written up.

**Go** requires all of:

1. **The daemon runs the PC and Pi voice loop** through a board file and passes the conformance suite against a fake board.
2. **A wake-word engine and echo canceller fit the Dot:** measured on the unit, the daemon (capture, AEC, wake word, playback) uses at most the CPU and RSS that spec 0002 leaves for it, with the base OS idle figures from B0; the wake-word engine has a licence-clean model and a Rust or isolated-C runtime.
3. **T5D boots on the maintainer's unit and the restore to rooted Fire OS is proved**, and we understand its kernel, rootfs, install and update flow well enough to rebuild it and to design a signed A/B update on this partition layout (or name the realistic alternative).
4. **A board file built from T5D's findings** (mixer routes, LED format, button codes, channel map) is judged sufficient to drive audio, ring and buttons from our daemon, by reading the source and a small experiment, using no Amazon files.
5. The maintainer judges the ongoing effort acceptable.

**No-go, fall back:** if the wake word or echo canceller does not fit, the first fallback is Option 2 (fork `echod` in the image repository, keep the contract); if the unit cannot be restored safely or the maintainer judges the effort too high, fall back to Option 1 (unmodified base through the ESPHome adapter, B3) with missing features listed as documented gaps, or to EchoMuse or LibreEcho if B0 data favours them. Nothing in this repository's default build depends on the daemon until the gate, so a no-go costs this ADR, the roadmap revision and the contract document.

**Reconsider later** if upstream T5D stops or changes licence, if the maintenance burden exceeds the time the maintainer sets, or if the board file approach cannot express a second device.
