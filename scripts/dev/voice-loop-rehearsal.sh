#!/usr/bin/env bash
# Rehearse the whole voice loop with no microphone, no speaker and no paid service, on Linux or WSL2:
# a recording is played as the microphone (a wake word clip, then your question), the real wake engine, the
# real core, the local speech servers (scripts/dev/local-speech.sh start) and a free stand-in language model run,
# and the spoken reply is written to a file.
#   scripts/dev/voice-loop-rehearsal.sh question.wav          (mono 16-bit 16 kHz WAV)
# Prints the device's status lines, the question the model was asked and the size of the reply audio.
set -euo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
question="${1:?usage: voice-loop-rehearsal.sh question.wav}"
speech="${CLAWDOT_SPEECH_DIR:-$HOME/clawdot-speech}"
work="$(mktemp -d)"
trap 'kill $(jobs -p) 2>/dev/null || true; rm -rf "$work"' EXIT
export PATH="$HOME/.local/bin:$PATH"
py="$speech/piper-venv/bin/python"
[ -x "$py" ] || py=python3

curl -fs -m 2 -o /dev/null "http://127.0.0.1:${CLAWDOT_TTS_PORT:-8082}/health" ||
  { echo "rehearsal: start the speech servers first: scripts/dev/local-speech.sh start" >&2; exit 1; }

echo "rehearsal: building (first time takes minutes)"
(cd "$repo" && cargo build -q --release -p clawdot-core && cargo build -q --release -p clawdot-device --features mww)
target="${CARGO_TARGET_DIR:-$repo/target}/release"

wake="$repo/crates/wake/tests/golden/models/okay_nabu.tflite"
"$py" "$repo/scripts/dev/replay/make_feed.py" "$repo/crates/wake/tests/golden/phrase_a.s16" "$question" "$work/feed.s16"

cat >"$work/board.json" <<EOF
{
  "schema": 1, "board": "rehearsal",
  "description": "Paced recording as the microphone, a file as the speaker.",
  "hardware": { "arch": "x86_64", "ram_mb": 4096, "microphones": 1, "speakers": 1 },
  "audio": {
    "capture": { "kind": "command", "command": ["$py", "$repo/scripts/dev/replay/pace.py", "$work/feed.s16"], "rate": 16000, "format": "S16_LE", "channels": 1, "mic_channels": [0] },
    "playback": { "kind": "command", "command": ["$py", "$repo/scripts/dev/replay/sink.py", "$work/reply.raw"], "rate": 16000, "format": "S16_LE", "channels": 1, "duck": { "kind": "software", "level_pct": 30 } }
  },
  "buttons": [ { "name": "action", "source": { "kind": "key", "key": "t" } } ],
  "actions": { "action.press": "action" },
  "leds": { "driver": "console" },
  "mute": { "kind": "software" },
  "features": { "aec": false, "on_device_wake": true },
  "update": { "mode": "manual" }
}
EOF
cat >"$work/core.json" <<EOF
{
  "data_dir": "$work/data",
  "llm": { "url": "http://127.0.0.1:8083/v1", "model": "stand-in" },
  "speech": { "url": "http://127.0.0.1:${CLAWDOT_TTS_PORT:-8082}/v1", "stt_model": "whisper-1", "tts_model": "piper", "voice": "${CLAWDOT_VOICE:-en_US-joe-medium}", "language": "en", "tts_sample_rate": 22050 },
  "satellite": { "listen": "127.0.0.1:10400", "follow_up_secs": 0 }
}
EOF

"$py" "$repo/scripts/dev/replay/fake_llm.py" 8083 >"$work/llm.log" 2>&1 &
"$target/clawdot" --config "$work/core.json" --no-console >"$work/core.log" 2>&1 &
sleep 2
# The stand-in microphone is a synthetic wake clip, which scores low after silence: loosen the word.
(sleep 30 | timeout 30 "$target/clawdot-device" --board "$work/board.json" --core 127.0.0.1:10400 \
  --wake-model "$wake" --wake-sensitivity 0.8 --turns 1) 2>&1 || true
echo "--- the language model was asked:"
cat "$work/llm.log"
echo "--- reply audio: $(wc -c <"$work/reply.raw" 2>/dev/null || echo 0) bytes of 16 kHz mono (32000 bytes = 1 s)"
