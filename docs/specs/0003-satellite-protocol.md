# 0003: Satellite Protocol

| | |
|---|---|
| **Status** | Draft |
| **Protocol version** | 0.1 (unstable) |
| **Created** | 2026-11-07 |
| **Related** | ADR-0005, [ADR-0008](../adr/0008-wyoming-compatible-framing.md), [0001](0001-product-requirements.md) |

> Sections 5 to 8 were written from the working implementation (`crates/protocol`, the core's satellite server, and the reference satellite in `crates/satellite`). Where the code settles a question, this spec says so; what is still open is listed in section 10. Machine-readable schemas for every control message are in [`schemas/satellite/`](../../schemas/satellite/README.md).

## 1. Summary

The satellite protocol is the **only contract** a hardware port must implement. It carries audio and events between a satellite (mic, speaker, optional wake word, LEDs, buttons) and the core. It must be implementable in a few hundred lines of C, Rust, or Go on a constrained device, and must not require knowledge of LLMs, tools, or providers.

## 2. Scope

**In scope:** transport, handshake and capability negotiation, audio framing and formats, events and commands, keepalive and reconnection, versioning and compatibility, authentication.

**Out of scope:** anything inside the core (providers, tools, scheduler).

## 3. Design goals

1. **Small:** minimal implementation burden; no exotic codecs or frameworks.
2. **Dumb endpoints:** the satellite is nearly stateless. Logic lives in the core.
3. **Robust:** handles Wi-Fi drops, reconnects, and partial failure without wedging.
4. **Evolvable:** explicit versioning with clear rules.
5. **Testable:** a language-agnostic **conformance suite** lets port authors validate their satellite.

## 4. Requirements

| ID | Requirement | Pri | Ver |
|---|---|---|---|
| REQ-SAT-001 | Framing MUST be a single-line JSON header terminated by `\n`, followed by an optional JSON data block and an optional length-prefixed binary payload (Wyoming-compatible), carried over TCP with TLS available; an in-process transport MUST implement the same message semantics. Implemented in `protocol::frame`; **ADR-0008 is still Proposed**, and WebSocket remains the fallback if it is rejected. See section 5. | Must | T |
| REQ-SAT-002 | The first message MUST be a `hello` from the satellite carrying protocol version, device id, and capabilities. The core replies `welcome` or rejects with a reason. | Must | T |
| REQ-SAT-003 | Capabilities MUST include: audio in/out formats, on-device wake word, LEDs, buttons, display, hardware mute, and audio front-end features (AEC, noise suppression, VAD, beamforming), each declared as present/absent so the core can supply what the satellite lacks. | Must | T |
| REQ-SAT-004 | Uplink audio MUST be 16 kHz, 16-bit little-endian mono PCM in fixed-size frames (default 20 ms). | Must | T |
| REQ-SAT-005 | Downlink audio MUST support PCM and Opus; the format is negotiated in the handshake. | Must | T |
| REQ-SAT-006 | Events satellite→core MUST include wake (`detection`), `clawdot.button`, `clawdot.mute-changed`, `clawdot.playback-done`, and `error`. The names `wake`, `button`, `mute_changed`, `playback_done` used in earlier drafts mean these events; the wire names are in section 6. | Must | T |
| REQ-SAT-007 | Commands core→satellite MUST include: playing audio (`audio-start`, `audio-chunk`, `audio-stop`), `clawdot.stop-playback`, `clawdot.set-volume`, `clawdot.set-indicator`, `clawdot.set-mute`. There is no single `play_audio` message: a spoken reply is an audio stream (section 6). | Must | T |
| REQ-SAT-008 | Either side MUST send keepalives (`ping`, answered by `pong`); a peer that has sent nothing for the dead-peer timeout (default 20 s) MUST be considered gone, and the satellite MUST reconnect with backoff. The core pings every 5 s by default. | Must | T |
| REQ-SAT-009 | Protocol versions MUST follow `MAJOR.MINOR`: minor = backward-compatible additions (unknown fields/events ignored); major = breaking. While `MAJOR` is 0 the format is unstable and peers MUST match exactly. The core SHOULD support the previous major for one release. | Must | T |
| REQ-SAT-010 | Authentication MUST use a pre-shared token for remote connections. Tokens are per core today (the core accepts a list of token hashes, so each satellite can be given its own); a token bound to one `device_id` is not defined yet. | Must | T |
| REQ-SAT-011 | Core Wyoming event names and fields SHOULD be used unchanged where they fit (audio, wake, satellite, describe/info). ClawDot additions MUST live under a `clawdot.` event namespace and be ignorable by non-ClawDot peers. | Should | T |
| REQ-SAT-012 | The project MUST provide a conformance test harness (a fake core and a fake satellite) and JSON Schemas for all control messages. | Must | T |
| REQ-SAT-013 | A satellite MUST work with only the mandatory capability set (PCM in/out, no wake word, no LEDs), with the core performing wake-word detection when the satellite cannot. | Must | T |
| REQ-SAT-014 | The project SHOULD provide a bridge or compatibility mode so stock Wyoming satellites can connect for the core subset (audio in/out, wake). | Should | T |
| REQ-SAT-015 | The project MUST ship reference satellites for desktop (development) and Raspberry Pi-class devices, both passing the conformance suite. | Must | T |
| REQ-SAT-016 | The core SHOULD emit **semantic display events** under `clawdot.display.*` (state, transcript, now-playing, timers, inbox cards) when a satellite declares a display capability. The protocol MUST NOT assume a screen is absent, and a satellite without a display MUST be able to ignore these events. | Should | T |
| REQ-SAT-017 | A receiver MUST bound what it buffers: header line at most 16 KiB, data block at most 64 KiB, payload at most 1 MiB by default. A framing violation (oversized part, invalid JSON, a header that is not an object, missing `type`, bad length) is fatal: the receiver closes the connection and does not try to resynchronise. | Must | T |
| REQ-SAT-018 | A connection that does not send `hello` within 10 s is dropped. A `hello` with an incompatible version, a wrong or missing token (when the core requires one), or a first message that is not `hello` gets a `clawdot.reject` with a reason, and the core closes the connection. A token failure uses the reason `unauthorized` and reveals nothing else. | Must | T |

| REQ-SAT-019 | Speakers that are ESPHome voice-satellite servers (a Dot running a base image, the Home Assistant Voice PE, linux-voice-assistant) SHOULD attach through the optional adapter in [spec 0004](0004-esphome-satellite-client.md), which plays the satellite role on this protocol toward the pipeline. REQ-SAT-014 (Wyoming bridge) covers stock Wyoming satellites only. | Should | T |
| REQ-SAT-020 | **Administrator reset request.** A satellite MAY send `clawdot.admin-reset` when its owner held the action button for 10 seconds (REQ-DEV-031). The core MUST answer every request with `clawdot.admin-reset-window`. It MUST honour the request, and open the local administrator reset window (REQ-CFG-069), only when the satellite's address is loopback or the satellite presented a valid token in `hello` (so a satellite that anyone on the network can start is not enough), when the administrator setting is in password mode, and when the cooldown of REQ-CFG-069 allows; otherwise it answers with `open_secs` 0 and a short `reason`. Neither message carries a secret. | Should | T |

## 5. Wire format

One message is one **frame**:

```
{"type":"audio-chunk","data":{"rate":16000,"width":2,"channels":1},"payload_length":640}\n
<640 bytes of payload>
```

1. **Header:** one line of compact JSON ending in `\n`. Fields: `type` (string, required, the event name), `data` (object, optional, the event's fields), `data_length` (integer, optional), `payload_length` (integer, optional).
2. **Data block:** if `data_length` is given, that many bytes of a JSON object follow the newline. Its fields are merged over `data` from the header. This is how stock Wyoming peers send data. **Senders in this project always put the fields inline in `data`**, so a receiver may support only the inline form and still talk to ClawDot peers; receivers of stock Wyoming traffic need both.
3. **Payload:** if `payload_length` is given, that many raw bytes follow (after the data block, if any).
4. A header whose `data` is not an object, whose lengths are negative or fractional, or that exceeds the limits in REQ-SAT-017 is a framing error.
5. Unknown header fields are ignored. Unknown `data` fields are ignored (REQ-SAT-009).

The default transport is a TCP connection to the core's `satellite.listen` address (the reference port is 10400). TLS is **not specified yet** (section 10).

## 6. Message catalogue

Fields are in `data` unless noted. Types and ranges are normative in the JSON Schemas.

### 6.1 Handshake

| Dir | `type` | Fields |
|---|---|---|
| S→C | `clawdot.hello` | `protocol` `{major, minor}` (required); `device_id` string (required); `name` string; `token` string; `capabilities` object, every key optional and defaulting to absent |
| C→S | `clawdot.welcome` | `protocol`; `session_id` string; `audio_out` `"pcm"` or `"opus"`, the codec the core will use for downlink audio |
| C→S | `clawdot.reject` | `reason` string; the core then closes the connection |

`capabilities` keys: booleans `wake_word`, `leds`, `buttons`, `display`, `hardware_mute`, `aec`, `noise_suppression`, `vad`, `beamforming`, `media_url`; and `audio_out`, an array of `"pcm"`/`"opus"` the satellite can play (empty or absent means PCM only). A satellite that sends `{}` or omits `capabilities` gets the mandatory set (REQ-SAT-013).

Until `welcome` arrives a satellite sends nothing else. After `welcome`, a second `hello` or `welcome` is ignored.

### 6.2 Audio

| Dir | `type` | Fields |
|---|---|---|
| S→C | `audio-chunk` | `rate` 16000, `width` 2, `channels` 1; `timestamp` integer milliseconds into the stream (optional); payload = little-endian PCM, 640 bytes for 20 ms |
| C→S | `audio-start` | `rate`, `width`, `channels` (the reference core sends 16000/2/1) |
| C→S | `audio-chunk` | same fields; payload = PCM (the core uses chunks of up to a few hundred milliseconds; players must not assume 20 ms) |
| C→S | `audio-stop` | none |
| C→S | `clawdot.stream-start` / `clawdot.stream-stop` | none. The satellite streams microphone `audio-chunk`s only between these two, and never while muted |

Uplink audio in any other format is dropped by the core after one `error` with code `bad-audio-format`. The mic stream has no `audio-start`/`audio-stop` framing: `stream-start` and `stream-stop` delimit it. Opus downlink is negotiated through `welcome.audio_out` but **not implemented**: the core always answers `pcm`.

### 6.3 Events, satellite to core

| `type` | Fields | Meaning |
|---|---|---|
| `detection` | `name` string; `timestamp_ms` integer ms (optional; stock Wyoming calls it `timestamp`) | Wake word heard, or a button pressed as a stand-in. Opens listening. |
| `clawdot.button` | `name` string (`action` opens listening, `mute` toggles mute; other names are ignored); `long` bool, default false | A button was pressed |
| `clawdot.mute-changed` | `muted` bool | A mute switch changed on the device |
| `clawdot.admin-reset` | none | The owner held the action button for 10 seconds; asks the core to open the local administrator reset window (REQ-SAT-020) |
| `clawdot.playback-done` | `id` string (optional) | The satellite finished playing the last `audio-stop`ped reply |
| `error` | `text` string; `code` string (optional) | A fault report; the core logs and ignores it |
| `ping` / `pong` | none | Keepalive |

Note on field names: `detection` carries `timestamp_ms` but `audio-chunk` carries `timestamp`, and stock Wyoming uses `timestamp` for both. The schemas record what is on the wire today; unifying them is an open question (section 10).

### 6.4 Commands, core to satellite

| `type` | Fields | Meaning |
|---|---|---|
| `clawdot.stop-playback` | none | Stop playing now |
| `clawdot.set-volume` | `level` integer 0 to 100 | Speaker volume |
| `clawdot.set-indicator` | `state` one of `idle`, `wake-detected`, `listening`, `thinking`, `speaking`, `error`, `muted`, `notification` | Show a state; ignore states the hardware cannot show |
| `clawdot.set-mute` | `muted` bool | Mute or unmute the microphone |
| `clawdot.play-url` | `url` `http(s)://` address, at most 2048 bytes, no whitespace; `title` string, at most 200 characters | Play an audio stream (radio), replacing what was playing. Only sent to satellites that declared `media_url` |
| `clawdot.media-control` | `action` `pause` or `resume` | Pause or resume stream playback (REQ-MUS-004) |
| `clawdot.admin-reset-window` | `open_secs` integer, 0 when refused; `reason` string (optional, plain words) | The answer to `clawdot.admin-reset`: how long the reset window is open, so the device can show it |
| `ping` | none | Keepalive; answer with `pong` |

A known event whose fields are missing or out of range (for example `level` of 101) is a protocol error: the receiver ends the session. An **unknown** event name is always ignored (REQ-SAT-009, REQ-SAT-011). A satellite that never declared `media_url` ignores `clawdot.play-url` and `clawdot.media-control`.

## 7. Session behaviour

1. TCP connect, then `hello` within 10 s. The core checks, in this order: the first message is `hello`; the version is compatible; the token (when the core requires one). Failure sends `reject` and closes.
2. `welcome`. The satellite is idle.
3. **Spoken turn.** `detection` (or `button` `action`) → the core sends `set-indicator listening` and `stream-start` → the satellite streams `audio-chunk`s → the core finds the end of speech and sends `stream-stop` and `set-indicator thinking` → the core sends `set-indicator speaking`, `audio-start`, `audio-chunk`s, `audio-stop` → the satellite plays the reply and sends `playback-done` → the core may send `stream-start` again (a follow-up window without the wake word).
4. **Mute.** `mute-changed` from the device, or the `mute` button (the core then answers `set-mute`). A muted satellite sends no mic audio and the core ignores the wake word.
5. **Keepalive.** The core pings every 5 s; the satellite answers `pong`. Either side may also ping. Silence for 20 s ends the session; the satellite reconnects with exponential backoff (the reference client starts at 1 s, doubles, and caps at 30 s).
6. **Reconnection** starts a fresh session with a new `hello`; no state survives it.

## 8. Versioning

`hello.protocol` and `welcome.protocol` carry `{major, minor}`. The current version is 0.1. Peers are compatible when `major` matches and, while `major` is 0, `minor` matches too. A mismatch is a `reject` whose reason names both versions.

## 9. Conformance

- **Schemas:** `schemas/satellite/frame-0.1.schema.json`, checked against the Rust types by `crates/protocol/tests/schema.rs`.
- **Harness:** `crates/e2e` provides `FakeSatellite` (drives a core) and `FakeCore` (drives a satellite), both built on the protocol decoder. `tests/conformance_core.rs` and `tests/conformance_satellite.rs` hold the scenarios; each test name starts with the requirement it covers (`req_sat_008_...`). Run them with `cargo test -p clawdot-e2e`.
- **A port** passes by running the satellite scenarios with its own launcher in place of `spawn_satellite`, pointed at `FakeCore::addr()`. The harness is Rust; a language-agnostic runner (recorded conversations a port replays) is not built yet.
- **Not covered yet:** REQ-SAT-014 (Wyoming bridge), REQ-SAT-015 (Raspberry Pi satellite), REQ-SAT-016 (display events), TLS, and Opus; each is absent from the implementation.

## 10. Open questions

| # | Question | Status |
|---|---|---|
| 1 | Wyoming-compatible framing plus `clawdot.*` extensions vs WebSocket. | **Settled in code, not in the record.** Wyoming-compatible framing is implemented and the reference client is about 150 lines of protocol logic over `std`, inside the SPIKE-005 target of 500 lines. [ADR-0008](../adr/0008-wyoming-compatible-framing.md) is still Proposed and the maintainer must accept it. SPIKE-005 also called for a TLS-wrapped hello, which has not been built. |
| 2 | Binary frame header layout and sequence numbers/timestamps for AEC alignment. | **Partly settled.** The layout is the Wyoming one (section 5). `audio-chunk` has an optional millisecond `timestamp`, which neither side fills in yet. No sequence numbers; add them only with AEC work. |
| 3 | Do we need a secondary UDP/RTP path for latency on lossy Wi-Fi? | Open. Defer unless measured. |
| 4 | How is playback reference audio provided for software AEC? | Open. The default is half-duplex, so no reference is needed. |
| 5 | TLS: how is it negotiated, and what does a satellite verify? | Open. Today: plaintext TCP, trusted network or VPN only (REQ-SEC-006 needs a decision). |
| 6 | Opus downlink (REQ-SAT-005) | Negotiation fields exist; the core always sends PCM and the reference satellite plays PCM only. Open until a codec is chosen against the budget. |
| 7 | Unify the timestamp field name across `detection` and `audio-chunk` | Open (see the note in section 6.3). Breaking, so it needs a minor bump while the major is 0. |
| 8 | Per-satellite tokens bound to a `device_id` (REQ-SAT-010) | Open. |
| 9 | Wake-word detection in the core for satellites without it (REQ-SAT-013) | Open: such a satellite must send `detection` from a button today. |

## 11. Changelog

| Version | Date | Change |
|---|---|---|
| 0.1 | 2026-11-07 | Skeleton |
| 0.2 | 2026-11-07 | Framing changed to Wyoming-compatible (pending ADR-0008); audio front-end capabilities; Wyoming bridge requirement |
| 0.3 | 2026-11-07 | `clawdot.play-url` and `clawdot.media-control` commands and the `media_url` capability for internet radio on satellites with a player (additive; unknown events are still ignored). |
| 0.3.1 | 2026-11-08 | Wire format, field tables, session behaviour, and limits written down from the implementation; event names corrected to the wire names; REQ-SAT-017 and REQ-SAT-018 added; JSON Schemas and the conformance harness added (section 9). No change to the protocol version number: the wire is unchanged. |
| 0.3.2 | 2026-11-09 | REQ-SAT-019: ESPHome devices attach through the optional adapter of spec 0004; REQ-SAT-014 is for stock Wyoming satellites. No change to the wire or the protocol version. |
| 0.3.3 | 2026-11-09 | REQ-SAT-020: `clawdot.admin-reset` and `clawdot.admin-reset-window` for the hardware-assisted administrator password reset (additive; unknown events are still ignored, no protocol version change). |
