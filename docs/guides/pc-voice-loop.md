# A push-to-talk voice turn on a PC

A manual smoke test for the audio interface ([spec 0001](../specs/0001-product-requirements.md) REQ-VOI-013..018). It runs a real spoken turn: you press Enter, speak, and the reply plays on your speakers. Nothing in CI does this, because CI has no audio device and must never spend money.

**Status: not yet run on real hardware.** The code is tested with scripted child processes and recordings only (`crates/satellite`, `crates/e2e/tests/audio_turn.rs`). The commands below are written from the tools' documentation. The first person to run this should expect to adjust a device name, and should record the result in [open-questions.md](../open-questions.md) or the roadmap.

## What runs where

```
 microphone ─► ffmpeg ─► clawdot-satellite ══ TCP 127.0.0.1:10400 ══► clawdot (core)
 speaker    ◄─ ffplay ◄─ clawdot-satellite ◄══                         ├─ endpointing, dialog
                                                                       ├─ speech to text   (speech section)
                                                                       ├─ language model   (llm section, OpenRouter)
                                                                       └─ text to speech   (speech section)
```

The satellite only moves samples. The programs it starts are `ffmpeg` (capture) and `ffplay` (playback), found on `PATH`; no audio library is linked.

## What you need

| Item | Detail |
|---|---|
| FFmpeg | Provides `ffmpeg` and `ffplay`. `winget install Gyan.FFmpeg`, then open a new terminal. Alternatively SoX (`sox`) works for both directions with no microphone name; see "Backends" below. |
| A Rust toolchain | To build `clawdot` (the core) and `clawdot-satellite`. The core is built and run in WSL2 per [dev-environment.md](../process/dev-environment.md); the satellite is plain Rust and builds natively on Windows with the MSVC toolchain (`cargo build --release -p clawdot-satellite`). Building the core natively on Windows has not been tried. |
| `OPENROUTER_API_KEY` | Already set as a Windows user variable. It pays for the language model only. |
| A speech provider | **Needed in addition.** See the next section. |

### Speech to text and text to speech

The core's speech client calls the OpenAI-compatible endpoints `POST {url}/audio/transcriptions` and `POST {url}/audio/speech` ([voice-pipeline.md](../architecture/voice-pipeline.md)). **An OpenRouter key does not by itself provide these**, and ClawDot does not send it to the speech provider. Pick one:

1. **OpenAI**: `url` `https://api.openai.com/v1`, models `whisper-1` and `tts-1`, key in a second environment variable (for example `OPENAI_API_KEY`). This costs money; the daily spend cap below applies to the language model only, so keep the session short.
2. **A local server on this PC** that speaks the same two endpoints (a Whisper server and a Piper or Kokoro wrapper, for example). Use `http://localhost:PORT/v1`; plain `http` is accepted for localhost only. No key and no cost. The model and voice names are whatever that server expects.

If you have neither, you can still exercise the text and file paths (next to last section).

## 1. Check the audio tools by themselves (free)

In PowerShell, find your microphone's exact name:

```powershell
ffmpeg -hide_banner -list_devices true -f dshow -i dummy
```

Copy the name of the audio device, for example `Microphone (USB Audio Device)`. Record three seconds, then play it back:

```powershell
ffmpeg -hide_banner -f dshow -i "audio=Microphone (USB Audio Device)" -t 3 -ac 1 -ar 16000 test.wav
ffplay -nodisp -autoexit test.wav
```

If you hear yourself, both tools work. If not, fix this first; ClawDot runs exactly this kind of command.

## 2. Write the core's configuration

Save as `clawdot-pc.json` (the model name is an example; use one your OpenRouter account allows):

```json
{
  "data_dir": "./clawdot-data",
  "llm": {
    "url": "https://openrouter.ai/api/v1",
    "model": "openai/gpt-4o-mini",
    "key_env": "OPENROUTER_API_KEY"
  },
  "limits": { "max_requests_per_day": 50, "daily_spend_usd": 1 },
  "speech": {
    "url": "https://api.openai.com/v1",
    "key_env": "OPENAI_API_KEY",
    "stt_model": "whisper-1",
    "tts_model": "tts-1",
    "voice": "alloy",
    "language": "en"
  },
  "satellite": { "listen": "127.0.0.1:10400", "follow_up_secs": 0 }
}
```

- `satellite.listen` on a loopback address needs no token. Do not bind it to a network address without `token_env` ([satellite-server.md](../architecture/satellite-server.md)).
- `follow_up_secs: 0` turns off the follow-up window for the first test, so one press is one turn. Remove it later for conversations.
- For a local speech server, replace the `speech` block with `"url": "http://localhost:PORT/v1"`, no `key_env`, and the model and voice names that server uses.
- The spend cap can also be given as a flag. A flag overrides the file: `--daily-spend-usd 1`.

## 3. Start the core (WSL2)

The core runs in WSL2, so the variables must be passed through from Windows. Set this once in PowerShell, then open a new terminal:

```powershell
setx WSLENV "OPENROUTER_API_KEY/u:OPENAI_API_KEY/u"
```

Then, from the repository in WSL (or through `scripts/cargo.sh` from Git Bash):

```
cargo run --release -p clawdot-core -- --config clawdot-pc.json --daily-spend-usd 1 --echo-speech
```

It prints `clawdot: satellites connect to 127.0.0.1:10400`. Leave it running. Windows reaches a WSL2 listener on `127.0.0.1` through WSL's localhost forwarding; if the satellite cannot connect, check `wsl --status` for that setting.

## 4. Run the satellite (Windows)

```powershell
cargo run --release -p clawdot-satellite -- --core 127.0.0.1:10400 --ptt --mic-device "Microphone (USB Audio Device)"
```

It prints which programs it chose (`microphone: ffmpeg`, `speaker: ffplay`) and then:

```
push-to-talk: press Enter to talk, then speak; q then Enter to quit
```

Press **Enter**, say "what time is it" (or any question), and stop. The core finds the end of your speech by silence (about a second), then thinks, then speaks. You should see `listening: speak now`, `thinking...`, `speaking`, and hear the reply. `q` then Enter leaves. Add `--turns 1` to leave after one reply.

Things to look for, and what they mean:

| You see | Meaning |
|---|---|
| `microphone: ... was not found; install it` | The program is not on `PATH`. Open a new terminal after installing, or name it with `--mic-cmd`. |
| `ffmpeg is installed but needs a microphone name` | On Windows `ffmpeg` has no default device; pass `--mic-device`. SoX needs none. |
| `microphone source ended` right away | `ffmpeg` exited: usually a wrong device name. Run the step 1 command by hand to see its message. |
| `audio stalled: the capture program produced no audio` | The device is open but silent (muted in Windows, or another app holds it). |
| `(nothing heard, or something went wrong)` | The core heard no speech, or a provider failed. Look at the core's console. |
| Reply text in the core, no sound | The player failed; see `speaker:` lines. Try `--speaker-cmd` with another player. |

## Backends

The default program is the first of these found on `PATH`; nothing is run to probe it. Override with `--mic-cmd PROGRAM [--mic-arg ARG ...]` and `--speaker-cmd PROGRAM [--speaker-arg ARG ...]` (a capture program must write raw 16 kHz mono signed 16-bit little-endian samples to standard output; a playback program must read the same from standard input; add `--speaker-wav` if it needs a WAV header first). No shell is involved, so quoting is never an issue.

| System | Capture (first found) | Playback (first found) |
|---|---|---|
| Windows | `ffmpeg` (only with `--mic-device`; DirectShow has no default), then `sox` (`waveaudio default`) | `ffplay` (default output, WAV on stdin), then `sox` |
| Linux | `arecord`, then `sox`, then `ffmpeg` (`alsa`) | `aplay`, then `sox`, then `ffplay` |
| macOS | `ffmpeg` (`avfoundation :default`), then `sox` | `ffplay`, then `sox` |
| Tests, headless | a WAV file (`--in question.wav`) | a WAV file (`--out answer.wav`) |

`--mic-device` and `--speaker-device` select a device for the chosen program (`ffplay` always uses the system default output).

## Without a microphone, or without paid providers

- **A recording through the live path.** Record `question.wav` (step 1 does this), then `clawdot-satellite --core 127.0.0.1:10400 --live --in question.wav --out answer.wav` runs one turn from the file and writes the reply to `answer.wav`. The core still calls its providers.
- **The older file mode.** `clawdot-satellite --core ... --in question.wav --out answer.wav` (no `--live`) does the same through the original development device.
- **The core alone.** `clawdot --config clawdot-pc.json --voice-in question.wav --voice-out answer.wav` answers a recording with no satellite at all.
- **Text only.** Start the core without `satellite` or `speech` and type to its console: the language model, tools and fast path run with no audio and no speech provider.
- **Everything free.** `scripts/cargo.sh test -p clawdot-satellite -p clawdot-e2e` runs the scripted-process and recording tests against the real satellite server with fake providers.

## What this does not do yet

- No wake word: Enter is the trigger. The satellite asks a `Trigger` for presses once per loop pass, so a detector replaces `LineTrigger` without touching anything else ([roadmap](../roadmap.md) A3).
- Half-duplex only: the microphone is closed while the reply plays, so there is no barge-in and no echo problem.
- Capture starts when you press Enter (and when the core re-opens listening for a follow-up), so a follow-up answer given in the first moments may lose its first word while `ffmpeg` starts.
- One playback program is started per reply, so there can be a short start-up gap before the first sound.
