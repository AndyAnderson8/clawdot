# ESPHome voice satellites

An optional core feature lets ClawDot be the brain of any speaker that is an **ESPHome voice-satellite server**: a Dot running a base image such as TECHO5 `echod`, a Home Assistant Voice PE, an Atom Echo, or a Raspberry Pi running linux-voice-assistant. ClawDot dials the device (the *client* side of the ESPHome native API), so Home Assistant is not needed. Requirements: [spec 0004](../specs/0004-esphome-satellite-client.md). Background: [R-08](../research/08-existing-echo-dot-projects.md) section 4.2.

It is **off by default** and adds nothing to a default build. Turn it on at build time:

```
cargo build --release -p clawdot-core --features esphome-satellite   # plaintext API
cargo build --release -p clawdot-core --features esphome-noise       # plus Noise encryption (uses ring)
```

## Configuration

```json
"esphome": {
  "devices": [
    { "name": "kitchen", "address": "dot.local", "psk_env": "KITCHEN_NOISE_KEY" }
  ],
  "media_listen": "0.0.0.0:3337",
  "media_url": "http://192.168.1.10:3337"
}
```

- `address` is `host` or `host:port` (port 6053 by default). At most four devices.
- `psk_env` names the environment variable that holds the device's Noise key (base64, 32 bytes, the `api: encryption: key:` value in its ESPHome configuration). Leave it out for a device without encryption. The key is never written to the file or the log. Needs `esphome-noise`.
- `media_listen` and `media_url` start a small file server. The device downloads the spoken announcement (and, for devices that cannot take audio over the API connection, the spoken reply) from `media_url`, which must be an address the device can reach. Without them, announcements are declined and logged; spoken replies to a person still work over the API connection.
- The section needs the `speech` section, as `satellite.listen` does. The follow-up window is 8 seconds unless `satellite.follow_up_secs` is set (which needs `satellite.listen`).

A build without the feature refuses a non-empty `esphome` section with a message naming the feature.

## How it works

For each device the core opens a TCP connection, says hello, reads the device's feature flags, lists its entities (it needs only the media player), and subscribes to voice requests with audio over the API connection. It then runs an ordinary [satellite session](satellite-server.md) on one end of an in-memory pipe and translates on the other end, so the whole voice pipeline is reused:

| Device (ESPHome) | Pipeline (satellite protocol) |
|---|---|
| `VoiceAssistantRequest` start | `detection` (wake), unless the follow-up window is already open |
| `VoiceAssistantAudio` (16 kHz mono PCM) | `audio-chunk` |
| `VoiceAssistantRequest` stop | the run is abandoned; listening is cancelled |
| events `RUN_START`, `STT_START`, `STT_VAD_END`, `STT_END`, `INTENT_START`, `INTENT_END`, `TTS_START`, `TTS_STREAM_START`, `TTS_STREAM_END`, `TTS_END`, `RUN_END` (or `ERROR`) | `stream-start`, `stream-stop`, indicators, `audio-start`/`audio-chunk`/`audio-stop` |
| spoken reply as `VoiceAssistantAudio` (or a URL in `TTS_END` for a device without an API speaker) | the reply audio; `playback-done` is sent after its duration |
| `VoiceAssistantAnnounceRequest` (media URL, `start_conversation`) | an inbox announcement or alarm; `playback-done` on `VoiceAssistantAnnounceFinished` |
| `MediaPlayerCommandRequest` | `set-volume`, `stop-playback`, `play-url`, `media-control` |

An inbox event is spoken like this: the pipeline synthesises it, the client stores it as a WAV file, sends `VoiceAssistantAnnounceRequest` with its URL and `start_conversation` (when the follow-up window is on and the device supports it), and waits for `VoiceAssistantAnnounceFinished`. The device then starts a run, and the user's answer continues the open follow-up window (the instruction goes to the agent bridge as for any satellite). A device that cannot announce is given a media-player command instead.

## Limits and safety

- Frames from a device are capped at 64 KiB; a malformed frame, bad length, or invalid message closes the connection and the client dials again after 1 s, doubling to 30 s. Nothing in the decoder can panic (checked with fixed-seed random inputs).
- One connection holds one frame plus the reply audio in memory (at most 4 MiB). The file server keeps four files for two minutes and serves only `GET`/`HEAD /esphome/<128-bit token>.wav`.
- The plaintext API is for a trusted network. Use Noise where the device supports it.
- Text from a device is untrusted and is clipped and control-character-stripped before it reaches a log line.

## Not implemented

The ESPHome **server** side (ClawDot appearing as a device), entities other than the media player (light, switch, mute, sensors), timers reported by the device, wake-word selection, UDP audio, and Bluetooth/camera features. See [open-questions.md](../open-questions.md) for the points to verify on a real device.

## Testing

`crates/e2e/tests/esphome_satellite.rs` runs the real client and pipeline against a fake ESPHome device (handshake, version and feature refusal, audio to transcript to spoken reply, announcements with a conversation, abort, reconnect, malformed frames, Noise). `crates/core/src/esphome/noise.rs` holds an `#[ignore]` interoperability test against an independent Noise responder; the check was done once against the `noiseprotocol` Python package, the library `aioesphomeapi` uses (see spec 0004 section 7).
