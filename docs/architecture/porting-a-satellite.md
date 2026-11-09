# Porting a satellite to new hardware

A satellite is a microphone, a speaker, and some lights. The protocol was designed so that a port is a few hundred
lines in any language ([spec 0003](../specs/0003-satellite-protocol.md)). The reference implementation in
`crates/satellite` is plain blocking Rust and is meant to be read and translated.

## What you implement

In Rust, the `Device` trait. In another language, the same eight things:

| Method | Meaning |
|---|---|
| `read_mic` | Next 20 ms of microphone audio (320 samples, 16 kHz, 16-bit little-endian mono). Only called while the core has asked for audio. |
| `begin_playback`, `play`, `end_playback` | A reply arrives as a start, a run of audio pieces, then an end. Play them in order, gaplessly if you can. |
| `stop_playback` | Stop now. |
| `indicator` | Show `idle`, `wake-detected`, `listening`, `thinking`, `speaking`, `error`, `muted`, or `notification`. Ignore the ones your hardware cannot show. |
| `set_volume` | 0 to 100. |
| `set_mute` | Obey the core's request to mute or unmute capture. |
| `supports_streams`, `play_url`, `pause_stream` | Optional. If your device can fetch and play an audio stream (a Linux board with `mpv`, for example), return true; the core will then send radio URLs and pause/resume commands. Otherwise leave the defaults and the core never sends them. |

Everything else (handshake, keepalive, streaming only when asked, reporting `playback-done`, ignoring events you do
not know) is in `clawdot_satellite::run`.

## The conversation, from the satellite's side

1. Connect over TCP and send `clawdot.hello` (protocol version, a device id, your token if the core wants one,
   and what you can do). Expect `clawdot.welcome`, or `clawdot.reject` with a reason; show the reason to the user.
2. When the wake word fires or the user presses the action button, send `detection` (a wake event). Without a wake
   word, a button is enough; the core cannot detect the wake word for you yet.
3. The core replies with `clawdot.stream-start`. Send `audio-chunk` frames until it sends `clawdot.stream-stop`.
   Send nothing while muted.
4. The core sends `audio-start`, some `audio-chunk`s, and `audio-stop`. Play them, then send
   `clawdot.playback-done`. The core may immediately ask for more audio (the follow-up window).
5. Answer `ping` with `pong`. If the core goes quiet for about 20 seconds, reconnect with backoff.

## Rules that matter

- **Never act on unknown events.** Ignore anything you do not recognise; newer cores will send more.
- **Half-duplex is the default.** The core stops listening while it speaks, so you do not need echo cancellation.
- **Mute must really mute.** If you have a hardware mute, report it with `clawdot.mute-changed` and stop sending audio.
- **Do not store or log audio.** The core does not either.
- **Protocol versions** match exactly while the major version is 0.

## Trying it

With a core configured as in [satellite-server.md](satellite-server.md):

```
clawdot-satellite --core 192.168.1.20:10400 --in question.wav --out answer.wav --token-env CLAWDOT_SATELLITE_TOKEN
```

The command streams the recording as if it were a microphone and saves the reply. It is also the quickest way to
check that a core is set up correctly before any hardware exists.

## Not yet available

- A conformance test suite you can run against a port (REQ-SAT-012). Today the reference satellite and the end to
  end tests in `crates/e2e` are the closest thing.
- Opus downlink and TLS.

## Audio on a desktop: the audio interface

The reference satellite can run real turns. `clawdot_satellite::front::AudioDevice` is a `Device` built from three small
parts ([spec 0001](../specs/0001-product-requirements.md) REQ-VOI-013..018):

| Part | Trait | Backends |
|---|---|---|
| Microphone | `audio::AudioIn`: `start`, `read_frame` (20 ms, 16 kHz mono s16), `stop` | `command::CommandIn` (a capture program's stdout), `audio::WavIn` (a recording) |
| Speaker | `audio::AudioOut`: `begin`, `play`, `finish`, `poll_done`, `stop`, `duck`, `set_volume` | `command::CommandOut` (a player's stdin), `audio::WavOut` (memory or a WAV file) |
| Turn trigger | `front::Trigger`: `poll` returns `Talk` or `Quit` | `LineTrigger` (Enter on the terminal), `Immediate`, `NoTrigger`; a wake-word detector goes here |

The command backends start a program from an argument vector (no shell), pass audio only through its pipes, bound their
buffers, kill the program when stopped or dropped, and report a silent or exited program as `Stalled` or `Eof`, never as
silence. Programs are found on `PATH` (`command::default_capture` and `default_playback`); the order and the Windows
caveats are in [the PC voice loop guide](../guides/pc-voice-loop.md).

The `Device` trait gained four optional methods with defaults, so existing devices are unchanged: `mic_active` (the
core started or stopped asking for audio), `mic_burst` (frames per loop pass, to keep up with a live microphone),
`poll_wake` (called every pass; true sends `wake`) and `playback_idle` (the satellite sends `playback-done` only when
this is true, and keeps answering pings meanwhile). A port to new hardware can ignore all four.

Optional hardware lives in `satellite::hw` and is independent of the core and of the protocol: `Leds` (`LedState`), `Buttons`
(`Talk`, `Mute`, `Stop`), and `AudioOut::capabilities()` (`duck`, `volume`, `stop`). Attach them with
`AudioDevice::with_leds` and `with_buttons`; a board without them gets the `Null` versions. The `Mute` button is defined but
not yet wired (the core owns mute, and reporting a hardware mute needs a `mute-changed` hook in the loop).
