# A live voice loop on a Windows PC

Say the wake word (or press Enter), ask a question, hear the answer, and it listens again. Everything runs on this PC except the language-model request, which goes to OpenRouter (or to a local model). Speech to text and text to speech are local and free ([local-speech.md](local-speech.md)). Requirements: REQ-VOI-013..018 ([spec 0001](../specs/0001-product-requirements.md)) and REQ-DEV-025..030 ([spec 0006](../specs/0006-device-layer-contract.md)).

**Status.** Built and tested without audio hardware: the always-on microphone, the wake word, half-duplex, mute and stall recovery run in CI against fake microphones and a scripted engine (`crates/e2e/tests/pc_loop.rs`, `crates/satellite/src/front.rs`), and the real wake engine wakes the daemon from golden audio (`crates/device/tests/daemon.rs`). On 2026-11-09 the whole path ran on the maintainer's PC with a **recording standing in for the microphone** (see "Rehearse without a microphone"). **Not yet run with a real microphone and speakers**: the sandbox that built this has none, and `ffmpeg` was not installed. The first person to run it should expect to adjust the microphone name, and should record the result in [open-questions.md](../open-questions.md).

## What runs where

```
 Windows                                                       WSL2 (Ubuntu)
 microphone ─► ffmpeg ─► clawdot-device.exe ══ TCP 127.0.0.1:10400 ══► clawdot (core)
   (always open)          ├ wake word (okay nabu)                      ├ end of speech, dialog
 speakers   ◄─ ffplay ◄─  ├ push-to-talk, mute, stop keys              ├ language model  ──► OpenRouter
                          └ status lines                               └ speech ──► 127.0.0.1:8082
                                                                          ├ whisper.cpp  (speech to text)
                                                                          └ Piper        (text to speech)
```

* The device program keeps the microphone open between turns. While idle it feeds 20 ms frames to the wake engine and throws them away otherwise. When it hears the word (or you press Enter) it sends `wake`; the core opens listening, the device sends the audio it kept since the word (at most 2 s) and then live audio, and **the core finds the end of your speech** by silence (about a second) and stops the stream.
* Half-duplex is kept: while the assistant speaks the microphone is open but its audio is discarded and the engine is not fed, so it never hears itself; after the reply the engine restarts afresh and the word is ignored for half a second.
* Mute releases the microphone for real (`m` then Enter, or a mute command from the core). Push-to-talk (Enter) always works, with or without a wake engine.
* If the microphone stalls or the capture program dies, a line says so and it is reopened after 1 s, 2 s, 4 s, up to 10 s. A program that is not installed is reported once and not retried.

## One-time setup

1. **WSL2 with the ClawDot build tools** (Rust, Zig): [dev-environment.md](../process/dev-environment.md), `scripts/setup-wsl.sh`. Also `rustup target add x86_64-pc-windows-gnu` is done for you by the build script. Windows needs **no Rust and no Visual Studio**: the Windows program is cross-built in WSL2.
2. **FFmpeg on Windows**, for the microphone and the speaker: `winget install Gyan.FFmpeg`, then open a new terminal. (`ffmpeg` and `ffplay` must be on `PATH`.)
3. **Your microphone's name:**
   ```powershell
   .\scripts\dev\pc-voice.ps1 -ListDevices
   ```
   Copy the audio device name, for example `Microphone (USB Audio Device)`. Check it by itself first (this is the same kind of command ClawDot runs):
   ```powershell
   ffmpeg -hide_banner -f dshow -i "audio=Microphone (USB Audio Device)" -t 3 -ac 1 -ar 16000 test.wav
   ffplay -nodisp -autoexit test.wav
   ```
4. **A language model.** `OPENROUTER_API_KEY` is already a Windows user variable; the launcher passes it to WSL2 for you (no `setx WSLENV` needed). The sample configuration uses `openai/gpt-4o-mini` with a one-dollar daily cap; edit `clawdot-pc.json` (created on first run from [examples/local-speech.json](examples/local-speech.json), and git-ignored) for another model. A language model that runs on this PC works too: point `llm.url` at its OpenAI-compatible address.
5. **The wake word model.** The launcher uses `crates\wake\tests\golden\models\okay_nabu.tflite` from your checkout: the official **Okay Nabu** microWakeWord model, Apache-2.0, already in the repository for its tests. To use another **official** model, download the `.tflite` and the `.json` with the same name from <https://github.com/esphome/micro-wake-word-models/tree/main/models/v2> (Apache-2.0): `okay_nabu`, `hey_mycroft`, `hey_jarvis` (the Jarvis name is a trademark question left open) and `alexa` (a trademark; not offered). Put both files in one folder and pass `-WakeModel C:\path\hey_mycroft.tflite`. Models from anywhere else are not covered by the licence recorded in the program: give their real licence with `--wake-licence` when running the program by hand. Their training data carries a non-commercial caveat for personal use; see the open-questions entry.

## Run it

```powershell
.\scripts\dev\pc-voice.ps1 -Mic "Microphone (USB Audio Device)"
```

The first run builds the Windows programs and the core (a few minutes), downloads the speech models (about 400 MB) and starts everything. Then:

| You do | It does |
|---|---|
| Say "okay nabu", then ask | Wake, listen, answer, listen again for the next wake word |
| Press **Enter** | Talk now (push-to-talk), same as the wake word |
| `m` then Enter | Mute or unmute (the microphone is released) |
| `s` then Enter | Stop speaking |
| `q` then Enter, or Ctrl+C | Leave; the core and the speech servers are stopped |

Options: `-Sensitivity 0.7` (0 strict, 0.5 as published, 1 loose), `-NoWake` (push-to-talk only), `-WakeModel PATH`, `-Config PATH`, `-NoSpeechServers` (you run another speech provider; edit `speech` in the configuration), `-KeepServers` (leave the speech servers running on exit), `-Rebuild`, `-Turns N` (leave after N replies), `-Board PATH` (your own board file). The core's log is `clawdot-core.log` in the repository folder.

### The status lines

The device prints one short line per change, and never audio or what you said:

| Line | Meaning |
|---|---|
| `wake word: "Okay Nabu" (model sha256 0689abe1912a95a3, licence Apache-2.0)` | At start: which word is loaded, the model's hash and its recorded licence |
| `microphone: open` | The capture program started |
| `ready: say "okay nabu" or press Enter to talk; q then Enter to quit` | Idle and listening for the word |
| `wake word heard: "okay nabu"` | The engine fired |
| `listening: speak now` / `thinking...` / `speaking` | The core's states |
| `(nothing heard, or something went wrong)` | The core heard no speech, or a provider failed; read `clawdot-core.log` |
| `muted: the microphone is released` / `unmuted` | Mute changed |
| `microphone: stopped; reopening in 2 s` / `microphone: back` | A fault and its recovery |
| `led: ...` | The same states as LED names (the console is the LED on a PC) |

### Tuning the wake word

If it fires on background talk, lower `-Sensitivity` (0.3); if you have to repeat yourself, raise it (0.7). Each step changes the probability cutoff the model must reach (0.5 is the published cutoff, 0.97 for Okay Nabu; the curve is documented in [wake-word-runtime.md](../architecture/wake-word-runtime.md)). Say the whole phrase, close to the microphone, at a normal pace. A word is ignored for 2 s after it fires.

### Resource use

The wake engine measured 0.24 % of one core of a Ryzen AI 9 365 per word (release build) and about 75 KB of working memory; the device program is 0.8 MB. whisper.cpp (base.en) and Piper take roughly 300 MB each while running ([local-speech.md](local-speech.md)).

## What the launcher does (to do it by hand)

```powershell
# 1. build (in WSL2): Windows programs, the core
wsl -d Ubuntu -- bash -lc ". ~/.clawdot-env.sh && cd /mnt/c/path/to/clawdot && scripts/dev/build-windows.sh && cargo build --release -p clawdot-core"
# 2. speech servers and core (in WSL2; OPENROUTER_API_KEY reaches WSL2 through WSLENV)
wsl -d Ubuntu -- bash -lc "cd /mnt/c/path/to/clawdot && scripts/dev/local-speech.sh setup && scripts/dev/local-speech.sh start"
$env:WSLENV = "OPENROUTER_API_KEY/u:" + $env:WSLENV
wsl -d Ubuntu -- bash -lc "~/.cache/clawdot-target/release/clawdot --config /mnt/c/path/to/clawdot/clawdot-pc.json --no-console --echo-speech"
# 3. the device (Windows), in the window where you type the keys
.\target\windows\clawdot-device.exe --board desktop --core 127.0.0.1:10400 --mic-device "Microphone (USB Audio Device)" `
    --wake-model .\crates\wake\tests\golden\models\okay_nabu.tflite --wake-sensitivity 0.5
```

`clawdot-device --help` lists every option (`--always-on` keeps the microphone open even without a wake model; `--wake-sensitivity WORD=0.7` sets one word; `--wake-refractory-ms`; `--speaker-device`). Windows reaches a core in WSL2 on `127.0.0.1` through WSL's localhost forwarding; if the device cannot connect, check `wsl --status`.

## Rehearse without a microphone

This is what was actually run on 2026-11-09. A recording plays as the microphone (a wake word clip, then your question), the speaker is a file, the language model is a free stand-in, and everything else is real: the wake engine, the core, whisper.cpp and Piper.

```powershell
wsl -d Ubuntu -- bash -lc "cd /mnt/c/path/to/clawdot && scripts/dev/local-speech.sh start && scripts/dev/voice-loop-rehearsal.sh /path/to/question.wav"
```

Result on the maintainer's PC with the whisper.cpp sample `jfk.wav` as the question:

```
wake word: "Okay Nabu" (model sha256 0689abe1912a95a3, licence Apache-2.0)
microphone: open
wake word heard: "okay nabu"
listening: speak now / thinking... / speaking
ready: say "okay nabu" or press Enter to talk; q then Enter to quit
--- the language model was asked: And so, my fellow Americans!
--- reply audio: 168668 bytes of 16 kHz mono
```

(The question was cut at its first pause, as the endpointer is meant to do. The wake clip is synthetic and scores low after a second of silence, so the rehearsal loosens that word to 0.8; a real voice does not need that.) The same run through `pc-voice.ps1` with a Windows board file in place of the microphone also worked, which exercised the cross-built `clawdot-device.exe` and the WSL2 core together.

## Without the launcher, or with only part of it

* **A recording through the satellite.** `clawdot-satellite --core 127.0.0.1:10400 --live --in question.wav --out answer.wav` runs one turn from a file; `clawdot --config clawdot-pc.json --voice-in question.wav --voice-out answer.wav` answers a recording with no satellite at all. The speech servers must be up for both ([local-speech.md](local-speech.md)).
* **Push-to-talk with the plain satellite** (no wake word, microphone opened at the press): `clawdot-satellite --core 127.0.0.1:10400 --ptt --mic-device "Microphone (USB Audio Device)"`. It is built for Windows by `scripts/dev/build-windows.sh` too.
* **Everything free, no audio:** `scripts/cargo.sh test -p clawdot-satellite -p clawdot-device -p clawdot-e2e`.
* **Other audio programs.** The default is `ffmpeg` and `ffplay`; SoX works for both directions with no microphone name. Override either with `--mic-cmd` and `--speaker-cmd` on the satellite, or `audio.capture.command` in a board file ([board-support.md](../architecture/board-support.md)).

| System | Capture (first found) | Playback (first found) |
|---|---|---|
| Windows | `ffmpeg` (only with a device name; DirectShow has no default), then `sox` | `ffplay`, then `sox` |
| Linux | `arecord`, then `sox`, then `ffmpeg` | `aplay`, then `sox`, then `ffplay` |
| macOS | `ffmpeg`, then `sox` | `ffplay`, then `sox` |

## Troubleshooting

| Symptom | Cause and fix |
|---|---|
| `ffmpeg is not installed` | `winget install Gyan.FFmpeg`, then a **new** terminal so `PATH` updates. |
| `-Mic is needed` | Run with `-ListDevices` and pass the name exactly, quotes included. |
| `microphone: not available; the wake word is off` | The capture program is missing or cannot start; fix it and restart (a missing program is not retried). |
| `microphone: stopped; reopening in N s` repeats | Another program holds the microphone, it was unplugged, or Windows privacy settings block desktop apps (Settings > Privacy > Microphone > "Let desktop apps access your microphone"). Run the `ffmpeg ... test.wav` command above to see the real message. |
| `ffmpeg` runs but the wake word never fires | Check the level: record with the test command and listen. Raise `-Sensitivity` to 0.7. The word must be the whole phrase. |
| Fires by itself | Lower `-Sensitivity` to 0.3; speakers close to the microphone can trigger it with other people's voices. The reply itself cannot: audio is discarded while it plays. |
| `wake word model not found` | Pass `-WakeModel` with the `.tflite`; its `.json` must sit beside it. |
| `the core did not open 127.0.0.1:10400` | Read `clawdot-core.log.err` in the repository folder. A first build can take over a minute; the launcher waits 60 s after it. |
| Device connects, `(nothing heard, or something went wrong)` | The speech servers are down or the language model failed: `wsl -d Ubuntu -- bash -lc "cd /mnt/c/path/to/clawdot && scripts/dev/local-speech.sh status"`; check `OPENROUTER_API_KEY` and the model name. |
| No sound from the speaker | `ffplay` failed: run `ffplay -nodisp -autoexit test.wav`. `ffplay` always uses the default output device. |
| The first reply starts late | A new `ffplay` is started per reply (a short gap); the first speech call also loads the voice. |
| Cannot connect from Windows to the core | WSL2 localhost forwarding is off: `wsl --status`; or set `networkingMode=mirrored` in `%USERPROFILE%\.wslconfig` and `wsl --shutdown`. |

## What this does not do

* No barge-in: you cannot interrupt a reply by speaking (a wake word while the assistant is *thinking* is passed to the core, which uses it for "stop"). No echo cancellation, so the speaker should not be pointed at the microphone more than necessary; discarding audio during replies is what prevents self-triggering.
* The wake word is one of the official English models; no custom word ([R-11](../research/11-wake-word-spike.md)).
* Reply audio starts after the whole first sentence is synthesised and a new player starts per reply.
* Windows only is documented. The same program builds natively on Linux and macOS, untested here.
