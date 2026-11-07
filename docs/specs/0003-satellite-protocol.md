# 0003: Satellite Protocol

| | |
|---|---|
| **Status** | Draft (skeleton) |
| **Protocol version** | 0.1 (unstable) |
| **Created** | 2026-11-07 |
| **Related** | ADR-0005, [0001](0001-product-requirements.md) |

> This is a skeleton that fixes structure and open decisions. Wire details are completed in milestone M0/M1 before implementation starts.

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

## 4. Requirements (draft)

| ID | Requirement | Pri | Ver |
|---|---|---|---|
| REQ-SAT-001 | Framing MUST be a single-line JSON header terminated by `\n`, followed by an optional JSON data block and an optional length-prefixed binary payload (Wyoming-compatible), carried over TCP with TLS available; an in-process transport MUST implement the same message semantics. **Pending ADR-0008**; WebSocket remains the fallback if rejected. | Must | T |
| REQ-SAT-002 | The first message MUST be a `hello` from the satellite carrying protocol version, device id, and capabilities. The core replies `welcome` or rejects with a reason. | Must | T |
| REQ-SAT-003 | Capabilities MUST include: audio in/out formats, on-device wake word, LEDs, buttons, display, hardware mute, and audio front-end features (AEC, noise suppression, VAD, beamforming), each declared as present/absent so the core can supply what the satellite lacks. | Must | T |
| REQ-SAT-004 | Uplink audio MUST be 16 kHz, 16-bit little-endian mono PCM in fixed-size frames (default 20 ms). | Must | T |
| REQ-SAT-005 | Downlink audio MUST support PCM and Opus; the format is negotiated in the handshake. | Must | T |
| REQ-SAT-006 | Events satellite→core MUST include: `wake`, `button`, `mute_changed`, `playback_done`, `error`. | Must | T |
| REQ-SAT-007 | Commands core→satellite MUST include: `play_audio`, `stop_playback`, `set_volume`, `set_indicator`, `set_mute`. | Must | T |
| REQ-SAT-008 | Either side MUST send keepalives; the peer MUST be considered gone after a configurable timeout, and the satellite MUST reconnect with backoff. | Must | T |
| REQ-SAT-009 | Protocol versions MUST follow `MAJOR.MINOR`: minor = backward-compatible additions (unknown fields/events ignored); major = breaking. The core SHOULD support the previous major for one release. | Must | T |
| REQ-SAT-010 | Authentication MUST use a pre-shared token (per satellite) for remote connections. | Must | T |
| REQ-SAT-011 | Core Wyoming event names and fields SHOULD be used unchanged where they fit (audio, wake, satellite, describe/info). ClawDot additions MUST live under a `clawdot.` event namespace and be ignorable by non-ClawDot peers. | Should | T |
| REQ-SAT-012 | The project MUST provide a conformance test harness (a fake core and a fake satellite) and JSON Schemas for all control messages. | Must | T |
| REQ-SAT-013 | A satellite MUST work with only the mandatory capability set (PCM in/out, no wake word, no LEDs), with the core performing wake-word detection when the satellite cannot. | Must | T |
| REQ-SAT-014 | The project SHOULD provide a bridge or compatibility mode so stock Wyoming satellites can connect for the core subset (audio in/out, wake). | Should | T |
| REQ-SAT-015 | The project MUST ship reference satellites for desktop (development) and Raspberry Pi-class devices, both passing the conformance suite. | Must | T |
| REQ-SAT-016 | The core SHOULD emit **semantic display events** under `clawdot.display.*` (state, transcript, now-playing, timers, inbox cards) when a satellite declares a display capability. The protocol MUST NOT assume a screen is absent, and a satellite without a display MUST be able to ignore these events. | Should | T |

## 5. Message catalogue (to be specified)

| Direction | Message | Purpose |
|---|---|---|
| S→C | `hello` | Version, id, capabilities, token |
| C→S | `welcome` / `reject` | Accepted config / refusal reason |
| S→C | audio frame (binary) | Mic audio, when streaming |
| S→C | `wake`, `button`, `mute_changed`, `playback_done`, `error` | Events |
| C→S | `stream_start` / `stream_stop` | Tell the satellite to start/stop sending mic audio |
| C→S | audio frame (binary) | TTS/tones/music |
| C→S | `play_audio`, `stop_playback`, `set_volume`, `set_indicator`, `set_mute` | Commands |
| C→S | `clawdot.play-url` | Play an audio stream (radio) from a URL. Only sent to satellites that declared `media_url`. The satellite decodes it. |
| C→S | `clawdot.media-control` | `pause` or `resume` stream playback. The core pauses music while the assistant speaks and resumes it afterwards (REQ-MUS-004). |
| both | `ping` / `pong` | Keepalive |

## 6. Open questions

| # | Question | Owner |
|---|---|---|
| 1 | Wyoming-compatible framing plus `clawdot.*` extensions vs WebSocket: see [ADR-0008](../adr/0008-wyoming-compatible-framing.md) and [R-05](../research/05-wyoming-and-local-intents.md). Current leaning: Wyoming-compatible. Decide after SPIKE-005. | Maintainer |
| 2 | Binary frame header layout and sequence numbers/timestamps for AEC alignment. | Maintainer |
| 3 | Do we need a secondary UDP/RTP path for latency on lossy Wi-Fi? Defer unless measured. | Maintainer |
| 4 | How is playback reference audio provided for software AEC? | Maintainer |

## 7. Changelog

| Version | Date | Change |
|---|---|---|
| 0.1 | 2026-11-07 | Skeleton |
| 0.2 | 2026-11-07 | Framing changed to Wyoming-compatible (pending ADR-0008); audio front-end capabilities; Wyoming bridge requirement |
| 0.3 | 2026-11-07 | `clawdot.play-url` and `clawdot.media-control` commands and the `media_url` capability for internet radio on satellites with a player (additive; unknown events are still ignored). |
