# 0006: Device Layer Contract

| | |
|---|---|
| **Status** | Draft |
| **Version** | 0.1 |
| **Author(s)** | Maintainer direction, drafted by an agent |
| **Created** | 2026-11-09 |
| **Related** | [ADR-0015](../adr/0015-own-the-device-layer.md) (Proposed), [ADR-0012](../adr/0012-board-support-layer.md), [0001](0001-product-requirements.md) (REQ-BRD, REQ-VOI-013..019), [0002](0002-resource-budget.md), [0003](0003-satellite-protocol.md), [R-11](../research/11-wake-word-spike.md), [R-07](../research/07-echo-dot-gen2-hardware.md) |

> ADR-0015 is still **Proposed** and behind a go/no-go gate (daemon spike and B0). This spec is the contract the daemon is built and tested against on a PC and a Pi first (roadmap B2). Nothing here commits the project to the Dot; if the gate says no-go, the contract still describes what any satellite front end must provide. Figures marked **proposed** are not measured.

## 1. Summary

The device layer is everything between the satellite protocol ([0003](0003-satellite-protocol.md)) and the hardware: audio in and out, tones, LEDs, buttons, mute, local stop, quiet hours, wake word, updates. This spec says what that layer provides on every supported board, which parts are required and which are optional, what a board must do when a capability is missing, and how a board proves it conforms.

The layer is **one portable daemon** (the same code on every board) plus, per device, a **board file** (JSON data, [schema](../../schemas/board/README.md)) and an **image recipe** (kernel, install, updates; outside this repository). The only per-device difference is the board file and the install.

## 2. Scope

**In scope:** the daemon's observable behaviour; the board-file format and validation; the capability model; the minimum hardware profile; the conformance suite's obligations.

**Out of scope:** the brain (agent loop, tools, Spotify control, timers, portal) which is hardware-agnostic; the image recipes and signing keys (a separate repository, to be created by the maintainer); the wire format (0003, with the additions in section 8 to be made there); ESPHome compatibility mode ([0004](0004-esphome-satellite-client.md)).

## 3. Definitions

| Term | Meaning |
|---|---|
| Daemon | `clawdot-device`: the portable device program. No per-device code paths. |
| Board file | JSON data describing one device, validated against [`schemas/board/board-1.schema.json`](../../schemas/board/board-1.schema.json). |
| Image recipe | Per-device kernel, bootloader, install and update procedure. |
| Capability | A hardware or software feature the daemon declares present or absent. |
| Required | A supported board MUST provide it, possibly through the documented equivalent. |
| Optional | A board MAY provide it; absence is declared, never faked. |
| Equivalent | What the user gets instead when a capability is absent (section 5). |
| Logical LED state | One of the fixed set in section 5.2. Boards map states to patterns. |
| Conformance suite | The tests of section 9 that every board must pass. |

## 4. Requirements

IDs are stable. Priorities are MoSCoW; "Ver" is Test, Inspection, Analysis or Demonstration. "M" is the roadmap step (B2 = daemon on PC and Pi, B3 = Dot spike, B5 = image and updates, B6 = Dot device features).

| ID | Requirement | Pri | M | Ver |
|---|---|---|---|---|
| REQ-DEV-001 | The device daemon MUST run unchanged on every supported board. All per-device differences MUST be in the board file or the image recipe; the daemon MUST contain no code path selected by a board name. | Must | B2 | T/I |
| REQ-DEV-002 | The daemon MUST implement the satellite protocol (0003) natively, and its `hello` MUST declare every capability it has and none it lacks. | Must | B2 | T |
| REQ-DEV-003 | Audio in MUST yield 16 kHz 16-bit mono PCM in 20 ms frames (REQ-SAT-004, REQ-VOI-013); the board file's channel map selects or mixes the microphone channels and identifies the echo-reference channels. Audio out MUST accept PCM (REQ-SAT-005) with `start`, `stop`, `volume` and `duck`. Backends MUST report device loss, a missing tool, end of input and a stall as distinct errors, never as silence (REQ-VOI-013). | Must | B2 | T |
| REQ-DEV-004 | The logical LED state set is fixed and versioned (section 5.2). The daemon MUST ignore an unknown state, MUST treat a state with no mapping as a no-op, and every state MUST have a documented equivalent on a board with no LEDs (section 5.2). | Must | B2 | T |
| REQ-DEV-005 | Stop and cancel MUST work without the wake word, from a button gesture where the board has a button and from the protocol (`clawdot.stop-playback`, and a cancel of the turn) always. The daemon MUST act locally first (silence the speaker, end capture) and tell the core afterwards, and MUST do so while the core is unreachable. | Must | B2 | T |
| REQ-DEV-006 | Quiet-hours settings MUST be applied locally: LED brightness ceiling, speaker volume ceiling and deferral of unsolicited announcements, even when the core is unreachable. Settings arrive from the core and are kept on the device; with none received the daemon applies no quiet hours. | Must | B2 | T |
| REQ-DEV-007 | Updates MUST be signed. On a board that declares `update.mode = ab`, a failed update MUST roll back without user action; a board that cannot do A/B MUST declare `manual`. The daemon MUST report the mode and the running version in its status. | Must | B5 | T/D |
| REQ-DEV-008 | A board file MUST be validated at startup against the published schema. Unknown keys and impossible values (out of range, wrong type, an unsupported schema version, a claim below the minimum hardware profile) MUST stop startup with a message that names the key. A board file MUST contain data only: no shell command assembled from text, no placeholder other than the documented `{nn}`, no secret (REQ-BRD-005). | Must | B2 | T |
| REQ-DEV-009 | Follow-up listening (reopening the microphone for a bounded window after a reply that expects an answer, REQ-VOI-007) SHOULD be offered on every board. It is driven by the core and requires no board feature. | Should | B2 | T |
| REQ-DEV-010 | Wi-Fi setup without a keyboard SHOULD be offered where the board has Wi-Fi: join a network, report status, forget it. The secret MUST NOT be logged (REQ-BRD-009). | Should | B5 | T/D |
| REQ-DEV-011 | The project MUST provide a device conformance suite (section 9). A board is "supported" only when every Must test passes on it and the run is recorded. The suite MUST run in CI against the desktop board with a fake core, and on real hardware for each supported board. | Must | B2 | T |
| REQ-DEV-012 | A board that is not Linux-capable, has no microphone or no speaker, or cannot fit the minimum hardware profile (section 6) MUST be documented as unsupported. A board that fails the minimum is unsupported, not partially supported. | Must | B2 | I |
| REQ-DEV-013 | Every capability MUST be declared present or absent. For each absent Required capability the daemon MUST provide the documented equivalent in section 5; for each absent Optional capability it MUST do nothing and never report it present. | Must | B2 | T |
| REQ-DEV-014 | Local feedback tones (wake acknowledge, error, timer, inbox chime) MUST be played by the daemon itself, without a round trip to the core, so feedback works on a slow link. Tones are built in; no tone file is read from the network. | Must | B2 | T |
| REQ-DEV-015 | A board MUST report its mute kind as `none`, `software` or `hardware`. The daemon MUST offer software mute on every board, MUST NOT stream or process microphone audio while muted, and MUST NOT state or imply a disconnected microphone unless the board file declares `hardware` (REQ-BRD-008). | Must | B2 | T |
| REQ-DEV-016 | Ducking: while the agent speaks, the daemon MUST lower other audio it controls by the configured level and restore it afterwards, including when the reply is stopped or fails. A board that cannot duck other sources MUST declare `duck.kind = none` and the daemon MUST pause those sources instead (section 5.1). | Must | B2 | T |
| REQ-DEV-017 | Push-to-talk MUST be available on every board as a turn source: a button where one exists, otherwise the protocol or portal. Wake-word detection MUST sit behind the `WakeEngine` seam of section 7 so an engine can be added or replaced without changing the loop. Where `features.on_device_wake` is false, wake detection is core-side (REQ-SAT-013). | Must | B2 | T |
| REQ-DEV-018 | The daemon MUST report clock trust (`unset`, `unverified`, `synced`, REQ-BRD-010) in its status and MUST NOT fire a timer locally on an untrusted clock (REQ-TMR-009). | Should | B5 | T |
| REQ-DEV-019 | The daemon MUST NOT log secrets or audio (REQ-SEC-003) and MUST refuse to read or write any path in the board file's `takeover.forbidden` list or containing a `..` segment. | Must | B2 | T |
| REQ-DEV-020 | The daemon's queues MUST be bounded and drop the oldest event on overflow (REQ-BRD-016, REQ-NFR-110). On disconnect it MUST keep serving local functions (stop, mute, quiet hours, LEDs, tones) and reconnect with backoff (REQ-SAT-008). | Must | B2 | T |
| REQ-DEV-021 | The daemon's default build MUST NOT link C code. Echo cancellation or wake-word code in C is allowed only inside one isolated optional crate that is not part of the default build, with its size and memory cost and licence recorded (ADR-0015 point 6). | Must | B2 | I |
| REQ-DEV-022 | The daemon MUST fit the resource profile of section 6.2 on the reference board; the figures are measured at B3 and recorded in the traceability document. | Must | B3 | T/A |
| REQ-DEV-023 | Bluetooth speaker mode (A2DP sink and source) MAY be offered, with arbitration against announcements defined: an announcement ducks or pauses the Bluetooth source and restores it. | May | B6 | T/D |
| REQ-DEV-024 | A Spotify Connect receiver MAY be offered. It MUST be off by default, MUST show the terms-of-service notice of section 5.4 when enabled, and MUST NOT be a dependency of any other feature. Spotify control through the brain is a software feature and identical on every board. | May | B6 | I/D |

## 5. Capability model

A **capability** is declared by the board file (hardware) or by the daemon build (software) and travels to the core in `hello.capabilities` (REQ-SAT-003). Software features are identical everywhere; hardware features appear where the hardware exists.

### 5.1 Required, Should and Optional

| Capability | Class | Declared by | When missing, the equivalent is |
|---|---|---|---|
| Audio in (microphone) | Required | `hardware.microphones >= 1`, `audio.capture` | None: a board without a microphone is unsupported (REQ-DEV-012) |
| Audio out (speaker) | Required | `hardware.speakers >= 1`, `audio.playback` | None: unsupported |
| Tone | Required | daemon (built in) | n/a: always present |
| LED states for agent events | Should | `leds` | The state is shown by a tone (wake, error, timer, inbox), the console line (desktop), and the portal status line (section 5.2). Never silent for `error` |
| Buttons | Should | `buttons`, `actions` | The portal and the protocol carry talk, stop, mute and volume; push-to-talk from the portal or the console |
| Mute | Required (kind may be `none`) | `mute.kind` | Software mute from the portal or the protocol (REQ-DEV-015); the status says `software` |
| Stop and cancel without wake word | Required | a `stop` action on a button; the protocol always | The portal Stop control and `clawdot.stop-playback` from the core, acted on locally first. A board with no button documents this as its stop path |
| Follow-up listening | Should | daemon (core-driven) | Wake word or push-to-talk again for each turn |
| Ducking of the daemon's own audio | Required | `audio.playback.duck` | `software` ducking scales the daemon's own audio |
| Ducking of other sources | Optional | `duck.kind = mixer` | Pause the other source while the agent speaks, resume after |
| Quiet hours hook | Required | daemon | n/a: LED ceiling needs LEDs, the volume ceiling and announcement deferral need nothing |
| Wake word on device | Optional | `features.on_device_wake` | Core-side detection on the uplink stream (REQ-SAT-013), or push-to-talk |
| Echo cancellation | Optional | `features.aec` | Half-duplex: the microphone is closed while the assistant speaks (REQ-VOI-012); barge-in stays off (REQ-VOI-005) |
| Hardware mute | Optional | `mute.kind = hardware` | Software mute; the assistant never says "disconnected" |
| Wi-Fi setup without a keyboard | Should | `wifi` | A wired or pre-configured network; documented in the board's install guide |
| Clock trust | Should | `clock` | The core's `clock_valid` status governs timers (REQ-TMR-009) |
| Updates | Required | `update.mode` | `manual`: declared in status and in the install guide |
| Bluetooth speaker mode | Optional | `features.bluetooth_sink`, `bluetooth_source` | Absent; the speaker plays only the assistant and its streams |
| Spotify Connect receiver | Optional | `features.spotify_connect` | Spotify control through the brain (play, pause, search) still works |
| Display | Optional | capability `display` (0003) | Semantic events are ignored (REQ-SAT-016); the portal shows them |

### 5.2 Logical LED states and their equivalents

Version 1 of the set (the board file's `leds.states` keys). Unknown names MUST be ignored. A state's equivalent applies on a board with no LEDs, and the first column of equivalents (the tone) is the daemon's built-in tone.

| State | Meaning | Source | Equivalent without LEDs |
|---|---|---|---|
| `idle` | Ready | `set-indicator idle` | Nothing; the portal shows "ready" |
| `wake` | Wake word or button heard | `set-indicator wake-detected`, local | Acknowledge tone |
| `listening` | Capturing speech | `set-indicator listening` | Acknowledge tone; portal "listening" |
| `thinking` | Waiting for the assistant | `set-indicator thinking` | Portal "thinking"; no sound |
| `speaking` | Reply playing | `set-indicator speaking` | The reply itself |
| `muted` | Microphone muted | `set-indicator muted`, local | A short mute tone on change; portal "muted" |
| `error` | Something failed | `set-indicator error`, local | Error tone; portal error |
| `offline` | Core unreachable | local (connection state) | Error tone once, then portal "offline" |
| `timer` | A timer is running or fired | core (addition, section 8) | Timer tone when it fires; portal list |
| `alarm` | An alarm is ringing | core (addition) | The alarm sound itself |
| `inbox` | An agent inbox item waits | `set-indicator notification` | Inbox chime; portal badge |
| `volume` | Volume changing (`{nn}` is the step) | local (button) | A short level tone |
| `updating` | An update is installing | local (image) | Portal status |
| `setup` | Waiting for Wi-Fi setup | local (image) | Spoken or tone prompt where audio works, otherwise the install guide |

Precedence when several apply (highest first): `updating`, `setup`, `offline`, `error`, `volume` (transient), `muted`, `alarm`, `timer`, `wake`, `listening`, `speaking`, `thinking`, `inbox`, `idle`. The daemon implements this once; boards do not reorder it.

### 5.3 Missing-capability rule

A capability the board file does not declare is absent. The daemon does not fake it (no LED claims on a board without LEDs, no "disconnected" mute). The documented equivalent is part of the contract and is exercised by the conformance suite on a board profile that lacks the capability (section 9).

### 5.4 Spotify Connect notice

Free Connect receivers rely on unofficial access to Spotify, which may breach its terms or stop working without notice. The text shown on enabling must say so. It is never a dependency of the core experience.

## 6. Minimum hardware profile

### 6.1 Supported boards

A board is supported by the daemon only if all of these hold, and the board file's `hardware` section states them:

| Item | Minimum | Notes |
|---|---|---|
| Operating system | Linux | Kernel with ALSA and `evdev` where buttons exist |
| CPU architecture | armv7, aarch64, x86_64 (riscv64 accepted by the file, not built) | 32-bit ARM is the reference (Echo Dot 2) |
| RAM | 256 MB total | The Dot has 512 MB with about 290 MB free beside the stock stack; the base OS plus the daemon must fit the budget of [0002](0002-resource-budget.md) |
| Microphone | at least 1 | More improve wake word and AEC; they are not required |
| Speaker | at least 1 | |
| Network | Wi-Fi or Ethernet to the core | The daemon needs the core for the brain; local functions work without it |
| Storage | enough for the image and 16 MB of state (REQ-NFR-106) | |

ESP32-class parts are not supported by the daemon. They may attach in compatibility mode ([0004](0004-esphome-satellite-client.md)). A board below the minimum is documented as unsupported (REQ-DEV-012) and is not partially supported.

### 6.2 Daemon resource profile (proposed, to be measured at B3)

| Resource | Proposed limit on the reference board | Basis |
|---|---|---|
| Daemon RSS, idle, wake word and AEC running | at most 16 MB | Leaves headroom inside the 48 MB design target when the daemon runs beside a core on one device; separate from the core's own budget when they run on different devices |
| Daemon CPU, idle, wake word and AEC running | at most 35 % of one Cortex-A53 core | R-08 measured 23 to 30 % for wake word alone on the Dot |
| Binary without the C DSP crate | at most 3 MB stripped | Size of the daemon crate is reported at each build; the figure here is a target, not a measurement |
| Cold start to ready | at most 5 s | Same as the core's budget |

These numbers are placeholders for the maintainer to set; they are logged in [open-questions](../open-questions.md).

## 7. The wake-engine seam

The daemon exposes the seam recommended by [R-11 section 9](../research/11-wake-word-spike.md). It is a Rust trait; this spec fixes its behaviour, not its spelling.

1. **Frame in, detection out.** The engine receives 16 kHz mono signed 16-bit PCM in frames of any length (10 ms is the natural unit) and appends zero or more detections, each with a word id, a score from 0.0 to 1.0 and the stream offset in samples where the phrase ended. No audio leaves the engine and none is stored.
2. **Per-word sensitivity.** One normalised value per word, 0.0 strict to 1.0 loose; 0.5 is the model's published cutoff.
3. **Refractory period** inside the engine (default 2 s, configurable): one utterance yields one detection.
4. **Muted means not fed.** The daemon does not call the engine while muted and resets it on unmute and after a turn.
5. **Provenance.** Each word reports its phrase, model hash and licence so status can show them and REQ-VOI-009 can be checked at startup.
6. **Push-to-talk is another source of the same event** (a reserved word id, score 1.0), so there is one entry point.

The daemon ships the seam and push-to-talk, not an engine. An engine is a separate step (A3 on the PC, B3 on the Dot).

## 8. Wire additions this contract needs (to be made in spec 0003)

Not applied here. Each is a minor, ignorable `clawdot.` addition (REQ-SAT-009, REQ-SAT-011), proposed for spec 0003 version 0.4:

| Addition | Direction | Why |
|---|---|---|
| Extended `clawdot.set-indicator` states: `timer`, `alarm` (the rest of the set in 5.2 is local to the device) | core to satellite | Timer and alarm LED states |
| `clawdot.play-tone` with a tone name from a fixed list | core to satellite | Core-requested feedback tones; local tones need no wire message |
| `clawdot.cancel-turn` | satellite to core | Tell the core a local cancel happened (REQ-DEV-005) |
| `clawdot.set-quiet-hours` | core to satellite | Deliver quiet-hours settings (REQ-DEV-006) |
| `clawdot.device-status` | satellite to core | Board name, daemon version, update mode, mute kind, clock trust |
| Update messages (`clawdot.update-available`, `clawdot.update-status`) | both | Updates, with the image recipe (REQ-DEV-007) |

Until they land, the daemon uses the messages that exist (stop, indicator, mute, volume) and keeps the rest local.

## 9. Conformance

The procedure for running it and for adding a board is written when the suite exists (slice 3 of the daemon work); this section is the contract.

1. **Fake core.** The suite runs the daemon (or any board implementation) against a scripted core built on the protocol decoder, as [0003 section 9](0003-satellite-protocol.md) does for satellites.
2. **Every Must is a test.** Test names start with the requirement, `req_dev_005_...`. A requirement that cannot be tested off hardware is marked in the report as "needs hardware" and is run on the unit.
3. **Capability profiles.** The suite runs once for a full profile and again for a profile with no LEDs, no buttons and no hardware mute, and checks the equivalents of section 5.
4. **CI.** The desktop board runs the whole suite in CI with command audio replaced by WAV files. A pull request that breaks a Must fails.
5. **A new board proves itself** by supplying its board file, running the suite on the unit with the same daemon binary built for its architecture, and recording the result (date, daemon version, board file hash, tests passed and skipped with reason). It needs no daemon change; if it does, the contract is wrong and this spec is amended first.
6. **Schema.** The board-file schema and the loader are kept in step by a test that validates the shipped examples and a set of malformed files (section 10).

## 10. Board file

The format is defined by [`schemas/board/board-1.schema.json`](../../schemas/board/board-1.schema.json) (JSON Schema draft 2020-12), documented in [`schemas/board/README.md`](../../schemas/board/README.md). Examples shipped in [`boards/`](../../boards/): `echo-dot2.json`, `desktop.json`, `raspberry-pi.json`.

- Closed objects: an unknown key is an error naming the key. The loader reports the first problem as a dotted path (`audio.capture.rate`).
- Versioned by the `schema` integer and the schema file name. A breaking change makes `board-2.schema.json`; a loader refuses a version it does not know.
- Data only (REQ-DEV-008): program arguments are arrays of fixed text, with no placeholder except `{nn}` in an LED pattern name; paths are absolute and contain no `..`.
- Board files are JSON because the configuration and ADR-0012 chose JSON; the cost is no comments, so notes live in `description` and in [board-support](../architecture/board-support.md).

## 11. Open questions

| # | Question | Status |
|---|---|---|
| 1 | Resource figures in 6.2 | Placeholders; measure at B3 |
| 2 | Wire additions in section 8 | Proposed; land with spec 0003 0.4 and code |
| 3 | Whether `alsa` backends are native Rust (ioctl needs `unsafe`) or a spawned tool | Open; command backends first, `alsa` kind reserved |
| 4 | Where the daemon's update agent lives (daemon or image) | Open; REQ-DEV-007 states the behaviour only |

## 12. Changelog

| Version | Date | Change |
|---|---|---|
| 0.1 | 2026-11-09 | First draft from ADR-0015's proposed text, refined: REQ-DEV-001..024, capability model, minimum hardware profile, board-file schema 1. |
