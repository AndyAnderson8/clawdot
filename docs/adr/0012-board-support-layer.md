# ADR-0012: Board support layer: a small `Board` trait, optional boards, per-device data

- **Status:** Proposed (maintainer review)
- **Date:** 2026-11-08
- **Deciders:** Maintainer

## Context

ClawDot is hardware-agnostic through the satellite protocol ([ADR-0005](0005-satellite-core-separation.md)), and the Echo Dot Gen 2 is the resource baseline ([ADR-0003](0003-reference-hardware-budget.md)). The protocol carries audio and a few events; it says nothing about how a particular speaker turns a button press into an event, shows `thinking` on its LED ring, learns that the clock is wrong, or joins a Wi-Fi network. Those are device facts.

Hands-on inspection of a Dot ([R-07](../research/07-echo-dot-gen2-hardware.md)) shows that every such facility is reachable from userspace as plain files and sockets: evdev button nodes, an ISSI IS31FL3236 LED driver (12 RGB LEDs) with a vendor `ledctrl` command, the `wpa_supplicant` control socket, ALSA character devices, and the system clock. It also shows the real difficulty: Amazon's daemons own those devices today (`ledcontroller`, `acebuttond`, `mixer`, `wifisvc`, `sntp`), the stock clock restores to a stale-but-plausible time after reboot, microphone mute is a kernel privacy driver whose electrical effect we cannot prove, and at least one sysfs file panics the kernel when read.

The maintainer wants real hardware features on the Dot (LED feedback, buttons, mute, headless Wi-Fi and clock setup) without forking, without a second copy of the core, and without breaking the 48 MB design target ([spec 0002](../specs/0002-resource-budget.md)).

## Decision drivers

- One repository; the core and the reference satellites stay device-agnostic.
- The default build must be byte-for-byte unaffected by boards that are not selected (REQ-NFR-113, REQ-NFR-115).
- Capabilities are uneven across devices and some are not achievable, so the interface must express absence and honesty (mute kind, clock trust).
- No new unsafe code and no mandatory dependencies (the workspace forbids `unsafe_code`).
- Per-device differences should be data, reviewable and testable, not code.
- Verified against the real device, not assumed.

## Options considered

1. **`cfg` branches for the Dot inside the core and satellite.** Fastest first step; spreads device paths and key codes through shared code and cannot be tested off-device. Rejected.
2. **A fork or a second repository per device.** Clean separation, but two histories for a one-person project, and fixes drift. Rejected (the maintainer's constraint is one repo, not a fork).
3. **Express everything in the satellite protocol.** Buttons, indicator and mute already exist there (REQ-SAT-006/007); but Wi-Fi join, clock trust and audio device names are local to the device, not wire concepts, and a protocol change would burden every port. Used for what fits (see below), not for the rest.
4. **A small `Board` trait in its own crate, boards behind Cargo features, per-device mapping in data (chosen).**

## Decision

Add a workspace crate `crates/board`. It defines the `Board` interface, a `NullBoard`, the shared gesture and mapping code, and the data-file schema. Concrete boards are modules of that crate behind Cargo features (`desktop`, `dot2`), so they share one tested copy of gesture timing and file parsing. Core and satellite binaries expose matching features (`board-desktop`, `board-dot2`). The satellite protocol stays unchanged; a board is the **device side** of events the protocol already has.

### The interface (what the hardware actually exposes)

| Area | Operations | Notes from the hardware |
|---|---|---|
| Capabilities | Report which areas are present | Everything optional (REQ-BRD-002) |
| Events | A bounded stream of `Button { id, down/up/repeat, time }`, `MuteChanged`, `WifiChanged` | Dot: three evdev nodes; the kernel gives down/up/repeat, long-press timing is ours (REQ-BRD-007) |
| LEDs | `set_state(LedState)`, optional `set_brightness(0..100)` | Dot: 12 RGB LEDs; states map to `ledctrl` pattern names (REQ-BRD-006) |
| Mic mute | `mute_state() -> { muted, kind: none/software/hardware }`; optional `set_mute` | Dot: key 113 plus a kernel privacy driver; whether it is electrical is unverified, so the Dot board declares `software` until proven (REQ-BRD-008) |
| Wi-Fi | `wifi_status()`, `wifi_join(name, secret)` | Dot: `wpa_cli`/control socket on `wlan0`; Amazon managers may fight a join (REQ-BRD-009) |
| Clock | `clock_status() -> unset / unverified / synced`; optional `set_clock` | Dot: restored from a saved value (hours stale) until `sntp` syncs (REQ-BRD-010) |
| Audio | `audio_devices()`: capture and playback names, rate, format, channels, mic vs reference channels, volume control | Dot: capture `pcmC0D24c` 9 ch 16 kHz S24_3LE, playback `pcmC0D23p` stereo 48 kHz S16_LE (REQ-BRD-011) |
| Takeover | `prepare() -> report`, `restore()` | Dot: services to stop for audio; reported, restored on exit (REQ-BRD-012) |

The trait is synchronous and channel-based (a bounded `std` channel fed by one or two small threads), so it does not force an async runtime or `tokio` features on the crate and costs a few KB of stack. Gesture detection (press, long press, double press) is shared code driven by raw edges and a monotonic clock, so it is unit-testable on a desktop.

How it meets the protocol: in the all-in-one build the core calls the board directly. For a remote satellite on a board-equipped device, the satellite process owns the board and translates: raw buttons and gestures become `button` events, `mute_changed` carries mute, `set_indicator` selects the LED state, `set_mute` and `set_volume` are applied through the board. Wi-Fi status/join and clock trust are local to the device and appear in that process's status page and the core status as `clock_valid` (TMR-009), not on the wire.

### Selection and configuration

- **Compile time:** Cargo features decide which boards exist. The default build contains `NullBoard` only; the board crate's code for other boards is not compiled (REQ-BRD-003, REQ-NFR-115).
- **Run time:** a `board` setting in the existing JSON configuration: `none` (default), `auto` (probe only among compiled-in boards: for the Dot, the I2C device `0-003f` named `is31fl3236` and the `biscuit_puffin` product), or a name. A named board that is missing stops startup with a message naming the key (REQ-BRD-004). An optional `board_file` overrides the shipped data file.
- **Per-device data:** `boards/echo-dot2.json` (sketched in [board-support](../architecture/board-support.md)) maps: raw keys to logical buttons; gestures to actions; logical LED states to device patterns; the `takeover` list; and a `forbidden` list of paths the board must never touch (REQ-BRD-005, 013). It is data only: no commands assembled from text.

### Echo Dot Gen 2 board

- Feature `board-dot2`, file `boards/echo-dot2.json`, target `armv7-unknown-linux-musleabihf` (REQ-BRD-014).
- Buttons: read `/dev/input/event1` and `/dev/input/event2` with plain file reads (no grab, so Amazon's daemons keep working).
- LEDs, route A first: ask the running `ledcontroller` with `ledctrl -s/-u/-c/-b`. Route B (stop `ledcontroller`, write `0-003f/frame`) is a later option behind the same logical states, once the `frame` format is verified.
- Wi-Fi: the supplicant control socket at `/data/misc/wifi/sockets/wlan0` (a datagram socket, `std` only) or `wpa_cli`; DHCP stays with `dhcpcd`.
- Clock: read the system clock; report `unverified` until the board sees evidence of a sync; optionally set the clock from a verified source.
- Audio: describe the devices; takeover is explicit. How the process actually opens ALSA (spawn `tinycap`/`tinyplay`, or a small reviewed module that is allowed `unsafe` for ioctls) is deferred to SPIKE-006 on the device.
- Build: `[profile.dot2]` inheriting `release` (same size flags, `panic = "abort"`, LTO), and `scripts/build-dot2.sh`, which builds with `--profile dot2 --target armv7-unknown-linux-musleabihf --features board-dot2`, checks the size gate, and stages `clawdot` and `echo-dot2.json`. Deployment to a device stays manual.

### Dependencies and the budget

| Item | Cost | Justification |
|---|---|---|
| Board code (`NullBoard`, gestures, mapping, Dot 2 module) | Estimated well under 100 KB of code and under 1 MB RSS (two reader threads, fixed queues). **Estimate, to be measured** and recorded per REQ-NFR-115 | No new dependency required |
| Board file format | JSON, parsed by `serde_json` which the core already links: no new dependency. Chosen by the maintainer over TOML (which would have added `basic-toml` or `toml`) | Decided |
| Everything else | `std` only: files, `UnixDatagram`, `Command`, threads, bounded channels | Keeps `unsafe_code = "forbid"` intact |

Resource picture on the device: the stock stack leaves about 290 MB free and ClawDot's hard gate is 64 MB, so memory is not the limit. Device ownership is.

## What this does not and cannot do

- **Prove the microphone mute is electrical.** The Dot board declares `software` mute (discard capture, show the `muted` pattern) until a test with the audio stack stopped shows the path is cut; the UI and speech must never say "disconnected" before then (REQ-BRD-008).
- **Hardware AEC or beamforming.** None exists; the stock firmware does both in software. Barge-in stays opt-in (REQ-VOI-005).
- **Amazon's wake word.** Proprietary. ClawDot uses its own wake word on the captured stream.
- **Arbitrary LED colours while `ledcontroller` runs.** Route A reaches only preloaded patterns (about 230, including solid colours); custom frames need route B.
- **A platform "clock is synced" signal.** None found; the board can report `unverified` and rely on a core-side HTTPS `Date` check.
- **Autostart at boot.** The system partition is read-only and the boot hook is unknown (R-07 unknown 9). Until resolved, ClawDot is started by hand or by an `adb` script.
- **Coexisting with the voice stack for audio.** Capture and playback devices are single-open and held by `mixer`; owning audio means stopping Amazon services, which is untested and may trip a watchdog.
- **Reading every sysfs file safely.** `amz_privacy/power_button_state` panics the kernel; the shared code refuses paths on the board's `forbidden` list (REQ-BRD-013).

## Consequences

**Positive:**
- The core and satellites stay free of device paths; the Dot board is reviewable data plus a small module, testable off-device through the shared gesture and mapping code and fake files.
- The default build and budget are untouched; the Dot gets a first-class, size-gated build.
- The same interface serves the Raspberry Pi (GPIO buttons, a ring), an ESP32 satellite, and the desktop.

**Negative / costs:**
- A new crate, a new feature matrix to keep green in CI (`default`, `desktop`, `dot2`), and one more data format to version.
- The Dot board depends on Amazon's `ledctrl` and on stock partitions staying as they are, and its audio story needs service takeover that is not yet proven safe.
- Boards cannot be validated on CI hardware; real-device checks stay manual per release (spec 0002 §5).

**Follow-ups:**
- Maintainer decisions: board files are JSON (decided 2026-11-09); LED route A first; `software` mute declaration; whether the board crate lives in this workspace as proposed.
- Resolve the unknowns in [R-07](../research/07-echo-dot-gen2-hardware.md) §11 (buttons with real presses, `frame` format, audio takeover, autostart) before the `board-dot2` milestone.
- Update the satellite-porting guide when the first board lands.
- Roadmap items 1 and 2 under "Beyond v1.0" carry the implementation.

## Amendment 2026-11-09: Dot board is data only; takeover plan superseded

The text above is kept as written. Since it was written, [R-08](../research/08-existing-echo-dot-projects.md) found two active projects (TECHO5 Dot and EchoMuse) that already solve Dot audio, LED ring, buttons, Wi-Fi and clock, and recommended against repeating that work on Fire OS. Consequences for this ADR:

- **The Dot board is data only until the base-image trial (roadmap B0) is done.** `boards/echo-dot2.json` records names, key codes, the LED `frame` format and the `forbidden` paths (including `power_button_state` and `calibrated_lux`). The `board-dot2` code module (button readers, `ledctrl` route A or B, Wi-Fi and clock control) is not built until B0 shows the base cannot supply those features.
- **The audio and LED takeover plan is superseded by R-08.** The `Takeover` row of the interface table, the Dot audio takeover, the LED routes and the staged Alexa removal describe the old plan. Alexa is removed by installing the base image (B0), with each device write gated on the maintainer's go.
- The `Board` trait, `NullBoard`, the desktop board, gesture and mapping code, JSON data files and auto-detection (roadmap B1) are unchanged. A board may later be realised remotely through ESPHome entities; the REQ-BRD wording for that is a follow-up (R-08 section 10).
- Status stays Proposed; B1 still moves it to Accepted.
