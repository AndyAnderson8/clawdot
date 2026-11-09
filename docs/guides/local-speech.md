# Free local speech on a PC: whisper.cpp and Piper

ClawDot's speech client needs two OpenAI-compatible HTTP calls, `POST {url}/audio/transcriptions` and `POST {url}/audio/speech` ([voice-pipeline.md](../architecture/voice-pipeline.md), `crates/core/src/speech.rs`). An OpenRouter key does not provide them. This guide sets up both on your own PC, for no money and with no CUDA, so a spoken turn costs only the language-model call (or nothing, with a local model).

**Status: run on the maintainer's PC (Windows 11, WSL2 Ubuntu) on 2026-11-09**, both natively on Windows and inside WSL2. Every command below was run; the results are in "What was measured". Not run: a microphone (the sandbox has none), a GPU, any other CPU.

## The recommended pair

| Direction | Program | Why | Licence |
|---|---|---|---|
| Speech to text | [whisper.cpp](https://github.com/ggml-org/whisper.cpp) `whisper-server`, model `ggml-base.en.bin` | One prebuilt CPU zip or tarball (9 MB), no Python, no CUDA. The server takes `--inference-path`, which makes it answer on `/v1/audio/transcriptions` and reply `{"text": "..."}`, exactly what the client reads. | Program MIT; the model is OpenAI Whisper converted to ggml, MIT ([model card](https://huggingface.co/ggerganov/whisper.cpp)) |
| Text to speech | [Piper](https://github.com/OHF-Voice/piper1-gpl) 1.8.0 (`pip install piper-tts`), voice `en_US-joe-medium`, behind `scripts/dev/piper_openai_shim.py` | Fast on a CPU (tens of times real time on this PC), 61 MB voice, no network after the download. Piper has no OpenAI-style endpoint of its own (its HTTP server is `POST /synthesize`), so a small Python shim (about 190 lines, standard library plus `piper`) translates; it also forwards transcriptions, so ClawDot's single `speech.url` serves both directions. | Piper is GPL-3.0 and is only run as a separate program, never linked into ClawDot. The voice `en_US-joe-medium` is **CC0** |

ClawDot has one `speech.url`. The shim listens on port 8082 and is that URL; it forwards transcription requests to `whisper-server` on 8081, which nothing else needs to reach.

### Voices and their licences

Piper's own documentation says: "Piper is intended for personal use and text to speech research only", and that each voice's `MODEL_CARD` holds its licence. Read the card of any voice you change to. Checked on 2026-11-09:

| Voice | Licence (from its `MODEL_CARD`) | Size | Use |
|---|---|---|---|
| `en_US-joe-medium` | CC0 | 61 MB | **Default.** No attribution needed. |
| `en_GB-alba-medium` | CC BY 4.0 | 61 MB | Attribute the University of Edinburgh dataset (Scottish-accented British English). |
| `en_US-libritts_r-medium` | CC BY 4.0 | 79 MB | Many speakers (`speaker` ids); attribute LibriTTS-R. |
| `en_US-kristin-medium` | public domain | 61 MB | LibriVox reader. |
| `en_US-lessac-medium` | Blizzard 2013 licence: research purposes only, **no commercial use**, no redistribution | 61 MB | Popular, but avoid if the device may ever be sold or shipped. |
| `en_US-ryan-medium`, `en_US-hfc_female-medium`, `en_US-hfc_male-medium` | CC BY-NC-SA 4.0 | 61 MB | Non-commercial, share-alike. |
| `en_US-amy-medium` | "See URL" (mimic3-voices), unclear | 61 MB | Avoid until clarified. |

ClawDot ships no voice and no model; these are downloaded to your disk, outside the repository. Change voice with `CLAWDOT_VOICE=en_GB-alba-medium` before `setup` and `start`, and the same name in `speech.voice`.

### Speech-to-text models

| Model file | Size | Notes on this CPU |
|---|---|---|
| `ggml-tiny.en.bin` | 75 MiB | Fastest, more mistakes |
| `ggml-base.en.bin` | 142 MiB | **Default.** English only. An 11 s clip took about 0.8 to 1.2 s. |
| `ggml-small.en.bin` | 466 MiB | Better with accents and names; roughly three times slower than base |

Set `CLAWDOT_STT_MODEL=small.en` to use another (files come from `https://huggingface.co/ggerganov/whisper.cpp`). Names such as "ClawDot" are heard as "Quad" or "Quadop" by the base model; that is a recognition limit, not a bug.

## Install and run

Pick where the core runs, because the core must reach the speech URL on `127.0.0.1`.

* **Core in WSL2 (what [pc-voice-loop.md](pc-voice-loop.md) uses).** Run the speech servers in WSL2 too, with the bash script. WSL2 in its default NAT mode cannot reach servers bound to Windows `127.0.0.1`.
* **Core elsewhere on Windows, or WSL2 in mirrored networking mode** (`networkingMode=mirrored` in `%USERPROFILE%\.wslconfig`, then `wsl --shutdown`; not tried here): run the servers on Windows with the PowerShell script.

Disk: about 410 MB in `~/clawdot-speech` (programs 23 MB, base.en model 142 MB, voice 61 MB, a 167 MB Python environment) plus about 105 MB for the `uv` Python 3.12 the bash script installs. Nothing goes in the repository.

### In WSL2 (or Linux)

```bash
scripts/dev/local-speech.sh setup     # about 400 MB of downloads; the whisper.cpp download is checked against a pinned SHA-256
scripts/dev/local-speech.sh start     # both servers, in the background; logs in ~/clawdot-speech/*.log
scripts/dev/local-speech.sh status
scripts/dev/local-speech.sh test      # speaks a sentence, then transcribes it back
scripts/dev/local-speech.sh stop
```

`setup` installs [uv](https://docs.astral.sh/uv/) (from `astral.sh`) to get a Python with `venv` and `pip`, because Ubuntu's own Python often lacks them and needs `sudo`. It also needs `libgomp1` for whisper.cpp; if it is not installed it fetches the package with `apt-get download` and unpacks it under `~/clawdot-speech/lib` without root (or run `sudo apt install libgomp1`).

### On native Windows

Needs Python 3.9 or newer (`winget install Python.Python.3.12`) and PowerShell 5.1 or 7. From the repository:

```powershell
.\scripts\dev\local-speech.ps1 setup
.\scripts\dev\local-speech.ps1 start
.\scripts\dev\local-speech.ps1 status
.\scripts\dev\local-speech.ps1 test
.\scripts\dev\local-speech.ps1 stop
```

Files go to `%USERPROFILE%\clawdot-speech`. If the first run says execution of scripts is disabled, run it as `powershell -ExecutionPolicy Bypass -File .\scripts\dev\local-speech.ps1 setup`. Both servers bind to `127.0.0.1` only. **Do not start `whisper-server` from Git Bash with an argument such as `--inference-path /v1/audio/transcriptions`:** MSYS rewrites it to a Windows path (`C:/Program Files/Git/v1/...`) and every request returns 404. Use PowerShell, or `MSYS_NO_PATHCONV=1`.

### By hand (what the scripts run)

```bash
# speech to text, port 8081
whisper-server -m ggml-base.en.bin --host 127.0.0.1 --port 8081 -t 4 --inference-path /v1/audio/transcriptions
# text to speech and the single speech URL, port 8082
python scripts/dev/piper_openai_shim.py --voice-dir ./voices --voice en_US-joe-medium --port 8082 --stt-url http://127.0.0.1:8081
```

Downloads: `https://github.com/ggml-org/whisper.cpp/releases/download/b5454/whisper-bin-x64.zip` (Windows, SHA-256 `6ba69e34...816ccb`) or `whisper-bin-ubuntu-x64.tar.gz` (Linux, `a72becf1...81c`); the model `https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin` (SHA-256 `a03779c8...d002`); `pip install piper-tts==1.8.0`; `python -m piper.download_voices --download-dir voices en_US-joe-medium`. (`b5454` is whisper.cpp's build tag for release v1.9.5; the `v1.9.5` release page itself has no binaries.) Full hashes are in the scripts.

## ClawDot configuration

[`examples/local-speech.json`](examples/local-speech.json), with the model names, `voice` and `tts_sample_rate: 22050` (the medium Piper voices are 22,050 Hz; the client resamples to 16 kHz):

```json
"speech": {
  "url": "http://127.0.0.1:8082/v1",
  "stt_model": "whisper-1",
  "tts_model": "piper",
  "voice": "en_US-joe-medium",
  "language": "en",
  "tts_sample_rate": 22050
}
```

No `key_env`: the servers need no key and plain `http` is accepted for localhost. Whisper.cpp ignores `model`, and the shim ignores `tts_model` and falls back to its default voice for names it does not have (the OpenAI voice `alloy`, say). The client asks for `response_format: "pcm"`, which the shim returns raw at the voice's rate.

## Verification checklist

1. **Servers up.** `local-speech.sh status` (or `.ps1`) prints `up` twice.
2. **STT.** Any 16 kHz mono 16-bit WAV (the whisper.cpp repository has `samples/jfk.wav`, public domain):
   ```bash
   curl -s http://127.0.0.1:8082/v1/audio/transcriptions -F file=@jfk.wav -F model=whisper-1 -F response_format=json -F language=en
   ```
   Expect `{"text":" And so my fellow Americans, ask not what your country can do for you, ask what you can do for your country.\n"}`. (Port 8081 answers the same request directly.)
3. **TTS, then listen.**
   ```bash
   curl -s -X POST http://127.0.0.1:8082/v1/audio/speech -H 'Content-Type: application/json' \
     -d '{"model":"piper","voice":"x","input":"Hello from ClawDot.","response_format":"wav"}' -o hello.wav
   ```
   Play `hello.wav` (on Windows: `start hello.wav`, or `ffplay -nodisp -autoexit hello.wav`). `response_format` `pcm` returns raw samples; other formats are refused with a 400 because no encoder is installed.
4. **Round trip.** `local-speech.sh test` speaks a sentence and prints what the STT heard.
5. **The core on a recording, no microphone.** With the language model of your choice in the config (OpenRouter needs `OPENROUTER_API_KEY` and spends a little; a local model does not):
   ```
   clawdot --config clawdot-pc.json --voice-in question.wav --voice-out answer.wav --no-console
   ```
   It prints `heard> ...` and `speaker> ...` and writes `answer.wav` (16 kHz mono). Then the satellite on the same recording: `clawdot-satellite --core 127.0.0.1:10400 --live --in question.wav --out answer.wav`.

## What was measured

Machine: AMD Ryzen AI 9 365 (20 threads), Windows 11, WSL2 Ubuntu, 4 whisper threads.

| Measure | Result |
|---|---|
| STT latency, 11 s clip, base.en | 0.8 to 1.2 s (about 10 times real time), same natively and in WSL2 |
| TTS latency, 3.7 s of speech | about 1.0 s for the first request after start (model load included in the process, warm-up in the first call), 0.1 to 0.2 s for later ones |
| Memory | `whisper-server` base.en about 260 to 300 MB resident; Piper shim about 300 MB resident |
| Disk | 410 MB (see above) |
| Whole path through the real core | `clawdot --voice-in jfk.wav --voice-out reply.wav` with these servers and a stub language model on localhost took 1.2 s, and the transcript `heard>` was word-exact; the reply was synthesised and written as 135,047 samples at 16 kHz |
| Round trip text to speech to text | "Hello from ClawDot. The weather is fine." came back as "Hello from Quad. The weather is fine." |

These are one machine's numbers. A slower CPU scales roughly with single-thread speed; `small.en` will not be real time on a laptop with two cores.

## Other options (not run here)

* **[speaches](https://github.com/speaches-ai/speaches)** (MIT, formerly faster-whisper-server): one OpenAI-compatible server for both directions (faster-whisper for STT; Kokoro and Piper for TTS), CPU image `ghcr.io/speaches-ai/speaches:latest-cpu` on port 8000, started with `docker compose` per [its installation page](https://speaches.ai/installation/). Its documented TTS formats are `mp3` and `wav`; whether it answers `response_format: "pcm"`, which ClawDot sends, was **not verified**, so it may need a client change. Docker Desktop is installed on this PC but Docker was not used.
* **[Kokoro-FastAPI](https://github.com/remsky/Kokoro-FastAPI)** (Apache-2.0): a better-sounding voice (Kokoro-82M) behind an OpenAI-style `/v1/audio/speech`. Not evaluated.
* **OpenAI** (`whisper-1`, `tts-1`): the paid choice in [pc-voice-loop.md](pc-voice-loop.md).

## Troubleshooting

| Symptom | Cause and fix |
|---|---|
| `whisper-server` answers 404 on every path | The path argument was rewritten by Git Bash; see above. |
| `libgomp.so.1: cannot open shared object file` | `sudo apt install libgomp1`, or rerun `setup` (it fetches the library). |
| The core says the speech provider failed | `local-speech.sh status`; read `stt.log` and `tts.log`. The core must be able to reach `127.0.0.1:8082`: from WSL2 NAT mode, a server on Windows is unreachable. |
| Reply speaks at the wrong speed or pitch | `tts_sample_rate` does not match the voice. Medium voices are 22050, low are 16000 (see the voice's `.onnx.json`). |
| Odd words in transcripts | Use `small.en`, and set `speech.language` to `en`. |
| `No module named pip` or `venv` in WSL | Expected on Ubuntu without `python3-venv`; the script uses `uv`, so run `setup` again. |
