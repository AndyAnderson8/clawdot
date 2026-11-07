# Satellite server

The core accepts speakers and other audio devices ("satellites") over the satellite protocol
([spec 0003](../specs/0003-satellite-protocol.md)). A satellite is a microphone and speaker that knows nothing
about assistants; the core does the thinking.

```
satellite ──hello──► core ──welcome──►
satellite ──wake───► core ──indicator: listening, stream-start──►
satellite ══audio══► core    (endpointing → speech-to-text → assistant → text-to-speech)
          ◄══audio══ core ──indicator: speaking──►
satellite ──playback-done──► core ──(follow-up window) stream-start──►
```

## Turning it on

```json
"speech": { "url": "https://api.openai.com/v1", "key_env": "OPENAI_API_KEY",
            "stt_model": "whisper-1", "tts_model": "tts-1", "voice": "alloy" },
"satellite": { "listen": "0.0.0.0:10400", "token_env": "CLAWDOT_SATELLITE_TOKEN", "follow_up_secs": 8 }
```

- `satellite.listen` needs the `speech` section, because the core transcribes and speaks for the satellite.
- A **shared token** (the value of the environment variable `token_env` names) is required unless `listen` is a
  loopback address. The satellite sends it in `hello`; only its hash is kept in memory. Wrong or missing tokens get
  a `reject` with the reason `unauthorized` and nothing else.
- There is no TLS on this port yet. Use a trusted network or a VPN, as with the rest of the device.

## What the core does for a satellite

| Satellite does | Core does |
|---|---|
| `hello` with protocol version and capabilities | Checks the version (exact match while the major is 0) and the token, replies `welcome` or `reject`. |
| `wake`, or the `action` button | Opens listening: `set-indicator listening`, `stream-start`. |
| Streams 16 kHz mono 16-bit `audio-chunk`s | Finds the end of the utterance; sends `stream-stop` and `set-indicator thinking`. |
| (waits) | Transcribes, asks the assistant, speaks the reply a sentence at a time: `set-indicator speaking`, `audio-start`, `audio-chunk`s, `audio-stop`. |
| `playback-done` | Opens the follow-up window (`stream-start`) so the user can answer without the wake word. |
| `mute-changed` or the `mute` button | Stops capture, shows `muted`, ignores the wake word until unmuted. The `mute` button toggles and sends `set-mute`. |
| Nothing for 20 s | Drops the connection. The core pings every 5 s; the satellite should reconnect with backoff. |

Other rules: unknown events are ignored so newer satellites keep working; audio in any format other than 16 kHz
mono 16-bit is reported once with an `error` event and dropped; up to four satellites connect at once and further
connections are closed immediately; frames are size-limited by the protocol decoder, so a hostile peer cannot make
the core allocate without bound; `hello` must arrive within 10 s.

## Announcements

When a satellite is connected, inbox announcements and fired timers or alarms are spoken on it as well as printed.
A satellite accepts one only when it is idle and unmuted, so an announcement never interrupts a conversation or
plays into a muted room. If every satellite is busy or muted, the event stays queued and is retried a second later
(nothing is marked announced until a satellite actually spoke it). Fired timers are best effort: a busy satellite
skips them, the console still shows them. After an announcement plays, the follow-up window opens, so the user can
answer ("keep going and add tests") without the wake word, and the instruction goes to the agent bridge.

## Assistant effects

Replies can carry effects. Volume changes (`VolumeSet`, `VolumeUp`, `VolumeDown`) and `StopPlayback` are sent as
`set-volume` and `stop-playback`. **Radio streams and pause/resume are not sent yet**: the protocol has no way to
tell a satellite to play a URL, so music needs a decision in spec 0003 (the core relaying the stream as audio, or
a `play-url` command) before it works on a satellite.

## Not built yet

- A Raspberry Pi satellite and the conformance suite (REQ-SAT-012, REQ-SAT-015); the desktop reference satellite exists, see [porting-a-satellite.md](porting-a-satellite.md).
- Wake-word detection in the core for satellites that cannot do it themselves (REQ-SAT-013); today those need a
  button, or the satellite must send `wake`.
- Opus downlink (REQ-SAT-005), TLS, per-satellite tokens, and displaying inbox announcements on a satellite.
