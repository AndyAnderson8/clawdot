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
    { "name": "kitchen", "address": "dot.local", "psk_env": "KITCHEN_NOISE_KEY", "reply": "whole" }
  ],
  "media_listen": "0.0.0.0:3337",
  "media_url": "http://192.168.1.10:3337"
}
```

- `address` is `host` or `host:port` (port 6053 by default). At most four devices.
- `psk_env` names the environment variable that holds the device's Noise key (base64, 32 bytes, the `api: encryption: key:` value in its ESPHome configuration). Leave it out for a device without encryption. The key is never written to the file or the log. Needs `esphome-noise`.
- `reply` is `"whole"` (the default) or `"streamed"`: whether a spoken reply to a person is delivered once it is complete, or starts on the device with its first sentence. See [Streamed replies](#streamed-replies). Unknown values are errors.
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

## Streamed replies

The pipeline speaks a reply [sentence by sentence](speech-latency.md): `audio-start`, each sentence's chunks as soon as it is made, `audio-stop`. With `"reply": "whole"` the client collects all of that and acts at `audio-stop`, so the first sound waits for every sentence to be synthesised. With `"reply": "streamed"` it acts at `audio-start`, which the session sends when the first sentence is ready. Only replies to a run the device started are affected; announcements (collected, then announced by URL), an abandoned run's reply and the commands are unchanged.

| Device | `whole` | `streamed` |
|---|---|---|
| Has `SPEAKER` (audio over the API) | `TTS_STREAM_START`, all the audio paced in `VoiceAssistantAudio` messages, end marker, `TTS_STREAM_END`, `TTS_END` | `TTS_STREAM_START` at the first sentence, each sentence's audio paced out as it arrives, then the same end marker and events after the last |
| No `SPEAKER` (fetches a URL) | the WAV is stored, then `TTS_END` carries its URL | `TTS_END` carries the URL as soon as the first sentence is ready; the media server answers with a response that is still being written |

**Playback timing.** The reply counts as played (`RUN_END`, then `playback-done`, which opens the follow-up window) only after `audio-stop` and after the audio has had time to play. The clock is a running "plays until" time: each piece sent moves it on by the piece's length from now or from where it was, whichever is later, so a gap while the next sentence is made is a gap in playback and not the end of the reply. The margin (`api_margin`, `url_margin`) is added after the last piece, as before. Nothing is sent to the pipeline before that, so the session keeps the reply open until the last sentence.

**The live file (URL mode).** The response is a WAV whose RIFF and data lengths are `0xFFFFFFFF` (a header cannot know the length), with `Transfer-Encoding: chunked` for an HTTP/1.1 request, or delimited by the end of the connection for HTTP/1.0. It carries no `Content-Length`. It ends with the terminating chunk when the reply ends, when the reply is stopped or fails (what was written stays), when the device's connection to the core is lost, when it has been open 180 s, or when no audio has come for 30 s. If the device closes the download, the connection slot is freed at once. A second `GET` for the same URL gets the audio from the start. The existing limits stand: a 128-bit token in the path, `GET`/`HEAD` only, four connections, four files for two minutes, 4 MiB of audio, the accept back-off. A reply with no media server falls back to the `whole` path (and its error).

**Stop.** A device that ends its run (`VoiceAssistantRequest` stop) drops the rest: the audio not yet sent is discarded, the live file is closed, and `playback-done` goes to the pipeline, which drops the sentence being synthesised and every sentence after it. The client also handles the pipeline's own `stop-playback` during a reply by closing the stream (`VoiceAssistantAudio` end marker and the closing events, or the live file), ending the run and reporting `playback-done`. The native ESPHome path does not use it today (the client turns the stop-while-working capture off), so it is a safeguard.

**Why `whole` is the default.** Whether a real device accepts it is not recorded anywhere in the repository: that a base image plays a WAV with no declared length or a chunked response (the URL formats it accepts are already an [open question](../open-questions.md)), and that its speaker pipeline tolerates a pause in `VoiceAssistantAudio` between sentences. Both modes keep the old behaviour reachable, so `streamed` is a per-device switch to try on the device.

**Trying it on a device.** Set `"reply": "streamed"` for one device and ask a question whose answer is four or five sentences. Listen for: the first sentence starting about one synthesis after the question, not after the whole answer; a clean join between sentences, or only a short pause when the voice is slow (a pause is expected, a restart or a cut-off is not); the device going back to idle, and the follow-up window opening, once after the last sentence and not earlier; for a URL device, the whole answer playing (a device that stops after the first sentence, or at the WAV header, does not accept a stream with no length); pressing the device's stop mid-answer silencing it. If any of those fail, set `"whole"` again and note what happened in [open-questions.md](../open-questions.md).

## Limits and safety

- Frames from a device are capped at 64 KiB; a malformed frame, bad length, or invalid message closes the connection and the client dials again after 1 s, doubling to 30 s. Nothing in the decoder can panic (checked with fixed-seed random inputs).
- One connection holds one frame plus the reply audio in memory (at most 4 MiB; a streamed reply is held in the same bounded buffer). The file server keeps four files for two minutes and serves only `GET`/`HEAD /esphome/<128-bit token>.wav`.
- The plaintext API is for a trusted network. Use Noise where the device supports it.
- Text from a device is untrusted and is clipped and control-character-stripped before it reaches a log line.

## Not implemented

The ESPHome **server** side (ClawDot appearing as a device), entities other than the media player (light, switch, mute, sensors), timers reported by the device, wake-word selection, UDP audio, and Bluetooth/camera features. See [open-questions.md](../open-questions.md) for the points to verify on a real device.

## Testing

`crates/e2e/tests/esphome_satellite.rs` runs the real client and pipeline against a fake ESPHome device (handshake, version and feature refusal, audio to transcript to spoken reply, announcements with a conversation, abort, reconnect, malformed frames, Noise). The `streamed` tests use a speech provider that waits for the test to let each sentence through: the device gets the first sentence (audio, or the URL and its first bytes) after one synthesis, the body arrives in order and ends with the reply, a stop or a synthesis failure ends it cleanly and nothing more is synthesised, and a device that drops its download frees the slot. `crates/core/src/esphome/noise.rs` holds an `#[ignore]` interoperability test against an independent Noise responder; the check was done once against the `noiseprotocol` Python package, the library `aioesphomeapi` uses (see spec 0004 section 7).
