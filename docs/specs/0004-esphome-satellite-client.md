# 0004: ESPHome voice-satellite client

| | |
|---|---|
| **Status** | Draft |
| **Version** | 0.2 |
| **Created** | 2026-11-09 |
| **Related** | [0003](0003-satellite-protocol.md), [0002](0002-resource-budget.md), [R-08](../research/08-existing-echo-dot-projects.md) section 4.2, roadmap B2, [ADR-0014](../adr/0014-language-rust-core-polyglot-edges.md) |

## 1. Summary

Many speakers (a Dot running a base image such as TECHO5 `echod`, the Home Assistant Voice PE, an Atom Echo, a Raspberry Pi running linux-voice-assistant) are **ESPHome servers**: they listen on TCP port 6053 and expect a client, normally Home Assistant, to dial them and drive the voice assistant. This spec defines an optional core feature, `esphome-satellite`, in which ClawDot is that **client**. It lets such a device use ClawDot as its brain without Home Assistant. It does **not** make ClawDot an ESPHome server.

The feature is an adapter: it speaks the ESPHome native API to the device and the ClawDot satellite protocol ([0003](0003-satellite-protocol.md)) to the existing voice pipeline, so wake handling, endpointing, speech-to-text, the assistant, text-to-speech, follow-up listening, and inbox announcements all stay exactly as they are for any other satellite.

## 2. Scope

**In scope:** the plaintext native API framing, a hand-written minimal protobuf codec for the messages listed in section 5, the voice-assistant subscription with audio over the API connection, text-to-speech return, announcements with optional start of a conversation, media-player commands, keepalive and reconnection, optional Noise encryption (a separate cargo feature), and configuration.

**Out of scope:** the ESPHome server side (pretending to be a device); entities other than the media player (light, switch, sensors, Bluetooth proxy, camera); Home Assistant features (state subscription, services); voice-assistant UDP audio; timers reported by the device; wake-word selection (`VoiceAssistantSetConfiguration`); authentication by password (removed from ESPHome in 2026.1).

## 3. Definitions

| Term | Meaning |
|---|---|
| Device | An ESPHome voice satellite that the core dials |
| API audio | Microphone and speaker audio carried inside the API connection as `VoiceAssistantAudio` messages (subscribe flag `API_AUDIO`), 16 kHz, 16-bit little-endian, mono |
| Run | One voice-assistant exchange started by the device (`VoiceAssistantRequest` with `start` true) |
| Feature flags | The `voice_assistant_feature_flags` bits in `DeviceInfoResponse`: `VOICE_ASSISTANT` 1, `SPEAKER` 2, `API_AUDIO` 4, `TIMERS` 8, `ANNOUNCE` 16, `START_CONVERSATION` 32 |

## 4. Requirements

| ID | Requirement | Pri | Ver |
|---|---|---|---|
| REQ-ESP-001 | The ESPHome client MUST be an optional cargo feature `esphome-satellite` that is **off by default**. A default build MUST contain none of its code and gain no dependency. Noise encryption (REQ-ESP-014) is a second feature, `esphome-noise`, that implies the first. | Must | T |
| REQ-ESP-002 | Configuration MUST be an `esphome` section with a `devices` list. Each device has `name` (required, used as the satellite `device_id`), `address` (`host` or `host:port`, port 6053 by default), and optionally `psk_env`, the **name of an environment variable** holding the base64 32-byte Noise key. The key MUST NOT be stored in the file, logged, or shown in the UI. Unknown keys are errors. A build without the feature MUST refuse a non-empty `esphome` section with a message that names the feature, rather than ignoring it. | Must | T |
| REQ-ESP-003 | Plaintext framing MUST be: one byte `0x00`, the payload length as a protobuf varint, the message type as a varint, then the payload. A received frame whose declared payload exceeds 64 KiB, whose varints are malformed or over 5 bytes, or whose first byte is not `0x00` (or `0x01` when Noise is in use) is a fatal framing error: the connection is closed and the reason logged. A first byte of `0x01` on a plaintext connection MUST be reported as "the device requires Noise encryption; set `psk_env`". | Must | T |
| REQ-ESP-004 | The protobuf codec MUST accept only well-formed input within bounds: every length is checked against the remaining bytes, varints are at most 10 bytes, wire types 3 and 4 and unknown wire types are errors, unknown fields are skipped, and text fields MUST be valid UTF-8. No input, however malformed, may panic or allocate beyond the frame limit. | Must | T |
| REQ-ESP-005 | Connection setup MUST be: `HelloRequest`, check that the device's API major version is 1, `DeviceInfoRequest`, then `ListEntitiesRequest` until `ListEntitiesDoneResponse`. A different major version, a device without the `VOICE_ASSISTANT` flag, or a device without the `API_AUDIO` flag MUST end the attempt with a message that says why, and the client retries per REQ-ESP-012. | Must | T |
| REQ-ESP-006 | After setup the client MUST send `SubscribeVoiceAssistantRequest` with `subscribe` true and flag `API_AUDIO`. It MUST answer `PingRequest` with `PingResponse` and `DisconnectRequest` with `DisconnectResponse`, send its own `PingRequest` every 10 s, and treat 30 s without any frame from the device as a dead connection. | Must | T |
| REQ-ESP-007 | On `VoiceAssistantRequest` with `start` true the client MUST reply `VoiceAssistantResponse` with `port` 0, open listening in the voice pipeline as if the device had sent a wake event, and feed each `VoiceAssistantAudio` payload to the pipeline as 16 kHz mono audio. It MUST send `VoiceAssistantEventResponse` events in this order: `RUN_START`, `STT_START`, then when the pipeline closes the microphone `STT_VAD_END` and `STT_END`, then `INTENT_START`, `INTENT_END`, `TTS_START`, the speech events of REQ-ESP-008, and finally `RUN_END`. A failed turn sends `ERROR` then `RUN_END`. | Must | T |
| REQ-ESP-008 | A spoken reply MUST reach the device by API audio when the device has the `SPEAKER` and `API_AUDIO` flags: `TTS_STREAM_START`, `VoiceAssistantAudio` messages of at most 1024 bytes of 16 kHz mono PCM, a final message with `end` true, `TTS_STREAM_END`. Otherwise it MUST be served as a WAV file by the media server (REQ-ESP-010) and announced in `TTS_END` data as `url`. The pipeline is told playback is done after the audio's duration plus a short margin, because the API gives no end-of-playback signal for a turn. | Must | T |
| REQ-ESP-009 | A `VoiceAssistantRequest` with `start` false from the device MUST abandon the run: listening stops and any reply produced for it is discarded. | Must | T |
| REQ-ESP-010 | An **announcement** (inbox event, alarm) spoken by the pipeline when no run is active MUST be served as a WAV file by the media server and sent as `VoiceAssistantAnnounceRequest` (`media_id` the URL, `start_conversation` true when the follow-up window is on and the device has the `START_CONVERSATION` flag) when the device has the `ANNOUNCE` flag, otherwise as `MediaPlayerCommandRequest` with `announcement` true on the first media-player entity. `VoiceAssistantAnnounceFinished` MUST be what tells the pipeline playback is done (with a timeout of the audio duration plus 10 s). The media server MUST bind only to the configured address, serve `GET /esphome/<random 128-bit token>.wav` and nothing else, keep at most 4 files of at most 4 MiB for at most 120 s, and answer anything malformed with an error and a closed connection. Without a configured media URL, announcements are declined and logged. | Must | T |
| REQ-ESP-011 | A conversation started by an announcement MUST reach the pipeline: after the announcement the device starts a run (REQ-ESP-007) which continues the already-open follow-up window instead of opening a second one. If the device starts no run within the follow-up window, the window MUST close, so that later announcements are not blocked. | Must | T |
| REQ-ESP-012 | A lost or refused connection MUST be retried with exponential backoff from 1 s doubling to 30 s, and each new connection starts a fresh pipeline session with no state carried over. A reply that was being produced for the old connection MUST be discarded. | Must | T |
| REQ-ESP-013 | Pipeline commands MUST map to the media-player entity when one exists: `set-volume` to `MediaPlayerCommandRequest` with volume, `stop-playback` to STOP, `play-url` to a play command with the URL, `media-control` to PAUSE or PLAY. Without a media player they are ignored and the satellite declares no `media_url` capability. Other indicators are ignored. | Should | T |
| REQ-ESP-014 | With the `esphome-noise` feature, a device with a `psk_env` MUST be dialled with `Noise_NNpsk0_25519_ChaChaPoly_SHA256`, the client as initiator, using the ESPHome prologue `NoiseAPIInit\x00\x00` and the framing in section 5.4, and built only on crates already in the dependency tree. A wrong key, a tampered record, or a failed handshake MUST end the attempt without panic. | Should | T |
| REQ-ESP-015 | Everything received from a device (names, text, audio) MUST be treated as untrusted. Each connection MUST use bounded buffers: one frame (64 KiB) plus the audio held for a reply. At most 4 devices MAY be configured. | Must | T |
| REQ-ESP-016 | The project MUST provide a fake ESPHome voice-satellite server in `crates/e2e` and tests that cover handshake, version mismatch, a missing feature flag, an audio stream through to a transcript and a spoken reply, an announcement with start of a conversation, device disconnect and reconnect, and malformed frames. | Must | T |
| REQ-ESP-017 | Spec 0002 MUST be met with the feature on or off. The measured binary-size difference between the default build and the feature builds MUST be recorded in section 7. | Must | A |

Verification: **T**est, **I**nspection, **A**nalysis, **D**emonstration.

## 5. Behaviour

### 5.1 Messages used

Message types and field numbers are protocol facts taken from the published ESPHome `api.proto` (see section 8 for the licence position); the client writes its own encoder and decoder for just these.

| Type | Message | Direction | Fields used |
|---|---|---|---|
| 1 | `HelloRequest` | to device | `client_info` 1, `api_version_major` 2, `api_version_minor` 3 |
| 2 | `HelloResponse` | from device | `api_version_major` 1, `api_version_minor` 2, `server_info` 3, `name` 4 |
| 5, 6 | `DisconnectRequest`, `DisconnectResponse` | both | none |
| 7, 8 | `PingRequest`, `PingResponse` | both | none |
| 9 | `DeviceInfoRequest` | to device | none |
| 10 | `DeviceInfoResponse` | from device | `name` 2, `esphome_version` 4, `friendly_name` 13, `voice_assistant_feature_flags` 17 |
| 11 | `ListEntitiesRequest` | to device | none |
| 19 | `ListEntitiesDoneResponse` | from device | none |
| 63 | `ListEntitiesMediaPlayerResponse` | from device | `object_id` 1, `key` 2 (fixed32), `name` 3 |
| 65 | `MediaPlayerCommandRequest` | to device | `key` 1 (fixed32), `has_command` 2, `command` 3, `has_volume` 4, `volume` 5 (float), `has_media_url` 6, `media_url` 7, `has_announcement` 8, `announcement` 9 |
| 89 | `SubscribeVoiceAssistantRequest` | to device | `subscribe` 1, `flags` 2 |
| 90 | `VoiceAssistantRequest` | from device | `start` 1, `conversation_id` 2, `flags` 3, `wake_word_phrase` 5 |
| 91 | `VoiceAssistantResponse` | to device | `port` 1, `error` 2 |
| 92 | `VoiceAssistantEventResponse` | to device | `event_type` 1, `data` 2 (repeated `name` 1, `value` 2) |
| 106 | `VoiceAssistantAudio` | both | `data` 1, `end` 2, `data2` 3 |
| 119 | `VoiceAssistantAnnounceRequest` | to device | `media_id` 1, `text` 2, `preannounce_media_id` 3, `start_conversation` 4 |
| 120 | `VoiceAssistantAnnounceFinished` | from device | `success` 1 |

Any other message type from the device is read, bounded, and ignored.

### 5.2 Mapping to the satellite protocol

For each device the client opens an in-memory pipe, runs a normal satellite session ([0003](0003-satellite-protocol.md)) on one end, and plays the satellite on the other end. It sends `hello` with the device name and the capabilities `wake_word` (the device detects the wake word) and `media_url` (only when a media player exists). A device `VoiceAssistantRequest` becomes `detection`; audio becomes `audio-chunk`; the pipeline's `stream-stop`, indicator, `audio-start`/`audio-chunk`/`audio-stop` messages become the events and audio of REQ-ESP-007 and REQ-ESP-008; `playback-done` is sent as REQ-ESP-008 and REQ-ESP-010 describe. Nothing in the pipeline is aware that the satellite is an ESPHome device.

### 5.3 Announcement with conversation

```
inbox event -> pipeline speaks (no run active)
client: WAV to media server -> VoiceAssistantAnnounceRequest(url, start_conversation)
device: plays, sends VoiceAssistantAnnounceFinished -> client sends playback-done
pipeline: opens the follow-up window
device: sends VoiceAssistantRequest(start) -> client continues the open window
user answers -> normal run (REQ-ESP-007)
```

### 5.4 Noise framing (feature `esphome-noise`)

Handshake: the client sends an empty frame (`0x01 0x00 0x00`), then a frame `0x01`, length (16 bits, big endian), `0x00`, and the first Noise message (`psk, e`). The device answers with a frame holding its hello (`0x01`, then its name and a zero byte, optionally followed by the MAC address and a zero byte) and then a frame whose payload starts with `0x00` for success (any other value is an error text) followed by the second Noise message (`e, ee`). After the handshake every frame is `0x01`, a 16-bit big-endian length, and one ChaCha20-Poly1305 ciphertext of: message type (16 bits big endian), payload length (16 bits big endian), payload. The nonce is 4 zero bytes then the 64-bit little-endian message counter; counters are per direction and a record that fails authentication ends the connection. The all-zero key that some devices accept when unprovisioned is not used.

## 6. Open questions

Recorded in [open-questions.md](../open-questions.md): the end-of-playback signal for turn audio, the audio format a given base image accepts as a URL, `data2` in `VoiceAssistantAudio`, and behaviour of TECHO5 `echod` with a non-Home-Assistant client.

## 7. Size record (REQ-ESP-017)

Release builds for `armv7-unknown-linux-musleabihf` (opt-level `z`, LTO, stripped), measured 2026-11-09 against `dev` at `ab05588`:

| Build | Size (bytes) | Difference from `dev` |
|---|---|---|
| `dev` before this work | 2,513,840 | none |
| Default features (this work merged) | 2,513,592 | -248 (none; link-order noise) |
| `--features esphome-satellite` | 2,600,592 | +86,752 (+3.5 %) |
| `--features esphome-noise` | 2,612,752 | +98,912 (+3.9 %); Noise itself is +12,160 |

All are far inside the 8 MiB limit of spec 0002. Memory was not measured on a device: a connection holds one frame (at most 64 KiB), at most 4 MiB of reply audio while it is being played, and the pipeline session that a satellite connection already costs. The default build is unchanged because the `esphome` configuration section is kept as raw JSON there and refused unless empty (REQ-ESP-002).

### Verification against independent code

The Noise handshake and the first records were checked once, by hand, against the `noiseprotocol` Python package (the library `aioesphomeapi` uses) acting as the device: handshake, `PingRequest`/`PingResponse`, and `HelloRequest`/`HelloResponse` all interoperated (`crates/core/src/esphome/noise.rs`, `interop_with_an_independent_responder`, `#[ignore]`, with `CLAWDOT_NOISE_PEER` and `CLAWDOT_NOISE_PSK` set). The ChaCha20-Poly1305 transport is checked against RFC 8439 section 2.8.2. Everything else is tested against the fake device in `crates/e2e`; no real ESPHome device or `aioesphomeapi` voice client has been used yet.

## 8. Licensing of the protocol definition

Findings are in [NOTICE](../../NOTICE) and [open-questions.md](../open-questions.md). No file from the ESPHome projects is copied into this repository; the message numbers and types above are facts about the wire format, and the encoder and decoder are written independently.

## 9. Changelog

| Version | Date | Change |
|---|---|---|
| 0.1 | 2026-11-09 | First draft: REQ-ESP-001 to REQ-ESP-017 |
| 0.2 | 2026-11-09 | Implemented behind `esphome-satellite` and `esphome-noise`; size record and verification notes (section 7); REQ-ESP-002 clarified: a default build keeps the section unparsed and refuses it unless empty |
