#!/usr/bin/env bash
# Free, local speech servers for ClawDot on Linux or WSL2: speech to text (whisper.cpp on CLAWDOT_STT_PORT, internal)
# and text to speech (Piper behind a tiny OpenAI-compatible gateway on CLAWDOT_TTS_PORT, which also forwards
# transcriptions, so ClawDot's one `speech.url` is http://127.0.0.1:8082/v1). Nothing here calls a paid service.
# Guide: docs/guides/local-speech.md.
#   scripts/dev/local-speech.sh setup     download the programs, models and voice (about 400 MB) into $CLAWDOT_SPEECH_DIR
#   scripts/dev/local-speech.sh start     start both servers in the background
#   scripts/dev/local-speech.sh status    say whether each answers
#   scripts/dev/local-speech.sh stop      stop them
#   scripts/dev/local-speech.sh test      speak a sentence and transcribe it back (needs both running)
# Environment: CLAWDOT_SPEECH_DIR (default ~/clawdot-speech), CLAWDOT_STT_PORT (8081), CLAWDOT_TTS_PORT (8082),
# CLAWDOT_VOICE (en_US-joe-medium), CLAWDOT_STT_MODEL (base.en), CLAWDOT_STT_THREADS (4).
set -euo pipefail

dir="${CLAWDOT_SPEECH_DIR:-$HOME/clawdot-speech}"
stt_port="${CLAWDOT_STT_PORT:-8081}"
tts_port="${CLAWDOT_TTS_PORT:-8082}"
voice="${CLAWDOT_VOICE:-en_US-joe-medium}"
model="${CLAWDOT_STT_MODEL:-base.en}"
threads="${CLAWDOT_STT_THREADS:-4}"
# Pinned: a whisper.cpp build, the Piper wheel. Hashes of the downloads are printed by `setup` and kept in the guide.
whisper_tag="b5454"
whisper_sha256="a72becf15d7917f990f6313867a52638b82b7f9ef237fb0c980dac56a135781c"
piper_version="1.8.0"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
export PATH="$HOME/.local/bin:$PATH"

die() { echo "local-speech: $*" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || die "$1 is needed ($2)"; }
pidfile() { echo "$dir/$1.pid"; }
alive() { [ -f "$(pidfile "$1")" ] && kill -0 "$(cat "$(pidfile "$1")")" 2>/dev/null; }

setup() {
  need curl "sudo apt install curl"
  mkdir -p "$dir/whisper/models" "$dir/voices"
  if [ ! -x "$dir/whisper/bin/whisper-bin-ubuntu-x64/whisper-server" ]; then
    echo "downloading whisper.cpp $whisper_tag (10 MB)"
    curl -fL --progress-bar -o "$dir/whisper/w.tgz" \
      "https://github.com/ggml-org/whisper.cpp/releases/download/$whisper_tag/whisper-bin-ubuntu-x64.tar.gz"
    echo "$whisper_sha256  $dir/whisper/w.tgz" | sha256sum -c - || die "the whisper.cpp download does not match the pinned hash"
    mkdir -p "$dir/whisper/bin"
    tar xzf "$dir/whisper/w.tgz" -C "$dir/whisper/bin"
    rm -f "$dir/whisper/w.tgz"
  fi
  if ! ldconfig -p 2>/dev/null | grep -q 'libgomp[.]so[.]1' && [ ! -e "$dir/lib/libgomp.so.1" ]; then
    echo "libgomp1 is missing; fetching it without root (or run: sudo apt install libgomp1)"
    mkdir -p "$dir/lib" "$dir/debs"
    (cd "$dir/debs" && apt-get download libgomp1) || die "install it with: sudo apt install libgomp1"
    dpkg -x "$dir"/debs/libgomp1_*.deb "$dir/debs/x"
    cp "$dir"/debs/x/usr/lib/*/libgomp.so.1* "$dir/lib/"
    rm -rf "$dir/debs"
  fi
  if [ ! -f "$dir/whisper/models/ggml-$model.bin" ]; then
    echo "downloading the speech-to-text model ggml-$model.bin (base.en is 142 MB)"
    curl -fL --progress-bar -o "$dir/whisper/models/ggml-$model.bin" \
      "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-$model.bin"
  fi
  if ! command -v uv >/dev/null 2>&1; then
    echo "installing uv (a Python package manager, about 40 MB) into ~/.local/bin"
    curl -LsSf https://astral.sh/uv/install.sh | sh
  fi
  if [ ! -x "$dir/piper-venv/bin/python" ]; then
    uv venv "$dir/piper-venv" --python 3.12
    uv pip install --python "$dir/piper-venv/bin/python" "piper-tts==$piper_version"
  fi
  if [ ! -f "$dir/voices/$voice.onnx" ]; then
    echo "downloading the voice $voice (about 61 MB)"
    "$dir/piper-venv/bin/python" -m piper.download_voices --download-dir "$dir/voices" "$voice"
  fi
  echo "setup done in $dir"
}

start() {
  [ -x "$dir/whisper/bin/whisper-bin-ubuntu-x64/whisper-server" ] || die "run setup first"
  if alive stt; then echo "speech to text already running"; else
    LD_LIBRARY_PATH="$dir/lib:${LD_LIBRARY_PATH:-}" nohup "$dir/whisper/bin/whisper-bin-ubuntu-x64/whisper-server" \
      -m "$dir/whisper/models/ggml-$model.bin" --host 127.0.0.1 --port "$stt_port" -t "$threads" \
      --inference-path /v1/audio/transcriptions >"$dir/stt.log" 2>&1 &
    echo $! >"$(pidfile stt)"
  fi
  if alive tts; then echo "text to speech already running"; else
    nohup "$dir/piper-venv/bin/python" "$here/piper_openai_shim.py" \
      --voice-dir "$dir/voices" --voice "$voice" --port "$tts_port" \
      --stt-url "http://127.0.0.1:$stt_port" >"$dir/tts.log" 2>&1 &
    echo $! >"$(pidfile tts)"
  fi
  for _ in $(seq 1 60); do
    if status_quiet; then status; return 0; fi
    sleep 1
  done
  status || true
  die "a server did not come up in 60 seconds; read $dir/stt.log and $dir/tts.log"
}

status_quiet() {
  curl -fs -m 2 -o /dev/null "http://127.0.0.1:$stt_port/health" &&
    curl -fs -m 2 -o /dev/null "http://127.0.0.1:$tts_port/health"
}

status() {
  local rc=0
  if curl -fs -m 2 -o /dev/null "http://127.0.0.1:$stt_port/health"; then
    echo "speech to text: up   http://127.0.0.1:$stt_port/v1 (ggml-$model)"
  else echo "speech to text: DOWN (log: $dir/stt.log)"; rc=1; fi
  if curl -fs -m 2 -o /dev/null "http://127.0.0.1:$tts_port/health"; then
    echo "text to speech: up   ($voice); ClawDot's speech.url is http://127.0.0.1:$tts_port/v1 (both directions)"
  else echo "text to speech: DOWN (log: $dir/tts.log)"; rc=1; fi
  return $rc
}

stop() {
  for n in stt tts; do
    if alive "$n"; then kill "$(cat "$(pidfile "$n")")" 2>/dev/null || true; echo "stopped $n"; fi
    rm -f "$(pidfile "$n")"
  done
}

test_roundtrip() {
  local wav="$dir/test.wav" pcm="$dir/test16k.wav"
  curl -fs -X POST "http://127.0.0.1:$tts_port/v1/audio/speech" -H 'Content-Type: application/json' \
    -d '{"model":"piper","voice":"x","input":"Hello from ClawDot. The weather is fine.","response_format":"wav"}' -o "$wav"
  echo "spoke $(wc -c <"$wav") bytes of WAV to $wav"
  "$dir/piper-venv/bin/python" - "$wav" "$pcm" <<'PY'
import sys, wave
src, dst = sys.argv[1:3]
r = wave.open(src)
data = r.readframes(r.getnframes())
rate = r.getframerate()
samples = [int.from_bytes(data[i:i + 2], "little", signed=True) for i in range(0, len(data) - 1, 2)]
n = int(len(samples) * 16000 / rate)
out = bytearray()
for i in range(n):
    s = samples[min(int(i * rate / 16000), len(samples) - 1)]
    out += int(s).to_bytes(2, "little", signed=True)
w = wave.open(dst, "wb")
w.setnchannels(1)
w.setsampwidth(2)
w.setframerate(16000)
w.writeframes(bytes(out))
w.close()
PY
  echo "heard back:"
  curl -fs "http://127.0.0.1:$tts_port/v1/audio/transcriptions" -F "file=@$pcm" -F model=whisper-1 -F response_format=json
  echo
}

case "${1:-}" in
  setup) setup ;;
  start) start ;;
  status) status ;;
  stop) stop ;;
  test) test_roundtrip ;;
  *) sed -n '2,12p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 2 ;;
esac
