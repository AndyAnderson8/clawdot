#!/usr/bin/env python3
"""A tiny OpenAI-compatible text-to-speech server over Piper, for local ClawDot tests.

It answers the one call ClawDot's speech client makes:

    POST /v1/audio/speech   {"model": "...", "voice": "...", "input": "text", "response_format": "pcm"}

and replies with raw signed 16-bit little-endian mono samples ("pcm") at the voice's own sample rate
(22,050 Hz for the medium voices, so set `speech.tts_sample_rate` to 22050), or a WAV file ("wav").
`GET /v1/models` and `GET /health` exist so health checks and OpenAI SDKs work, and `GET /v1/audio/voices` lists the
installed voices so ClawDot's portal can offer them as a choice.

With `--stt-url http://127.0.0.1:8081` it also forwards `POST /v1/audio/transcriptions` unchanged to a
whisper.cpp server, so ClawDot's single `speech.url` can point here for both directions.

It listens on 127.0.0.1 only, reads at most 64 KiB of a speech request (32 MiB of audio to forward), and never
logs the text it is given.
It needs `pip install piper-tts` (GPL-3.0; this file only imports it, it is not part of ClawDot's build).

    python piper_openai_shim.py --voice-dir ~/clawdot-speech/voices --voice en_US-joe-medium --port 8082
"""

import argparse
import io
import json
import re
import sys
import threading
import urllib.error
import urllib.request
import wave
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

from piper import PiperVoice

MAX_BODY = 64 * 1024
MAX_AUDIO = 32 * 1024 * 1024
MAX_TEXT = 4000
VOICE_NAME = re.compile(r"^[A-Za-z0-9_.-]{1,80}$")


class Voices:
    """Loads voices from a directory on first use; one synthesis at a time."""

    def __init__(self, directory: Path, default: str):
        self.directory = directory
        self.default = default
        self.loaded = {}
        self.lock = threading.Lock()
        self.get(default)  # fail at start, not on the first request

    def get(self, name: str) -> PiperVoice:
        if not VOICE_NAME.match(name):
            name = self.default
        path = self.directory / f"{name}.onnx"
        if not path.is_file():
            name = self.default
            path = self.directory / f"{name}.onnx"
        if name not in self.loaded:
            self.loaded[name] = PiperVoice.load(path)
        return self.loaded[name]

    def synthesize(self, name: str, text: str) -> tuple[bytes, int]:
        with self.lock:
            voice = self.get(name)
            chunks = list(voice.synthesize(text))
        if not chunks:
            return b"", voice.config.sample_rate
        return b"".join(c.audio_int16_bytes for c in chunks), chunks[0].sample_rate


def make_handler(voices: Voices, stt_url: str | None):
    class Handler(BaseHTTPRequestHandler):
        server_version = "clawdot-piper-shim"

        def log_message(self, fmt, *args):  # no request text, only the line
            sys.stderr.write("piper-shim: %s\n" % (fmt % args))

        def reply(self, code: int, ctype: str, body: bytes, extra=None):
            self.send_response(code)
            self.send_header("Content-Type", ctype)
            self.send_header("Content-Length", str(len(body)))
            for k, v in (extra or {}).items():
                self.send_header(k, v)
            self.end_headers()
            self.wfile.write(body)

        def error(self, code: int, message: str):
            body = json.dumps({"error": {"message": message}}).encode()
            self.reply(code, "application/json", body)

        def do_GET(self):
            if self.path in ("/health", "/v1/health"):
                self.reply(200, "text/plain", b"ok")
            elif self.path == "/v1/models":
                body = json.dumps({"object": "list", "data": [{"id": "piper", "object": "model"}]})
                self.reply(200, "application/json", body.encode())
            elif self.path == "/v1/audio/voices":
                # The voices that are installed, so the portal can offer a choice instead of a name to type.
                names = sorted(p.stem for p in voices.directory.glob("*.onnx"))
                self.reply(200, "application/json", json.dumps({"voices": names}).encode())
            else:
                self.error(404, "not found")

        def forward_transcription(self):
            if not stt_url:
                return self.error(404, "no --stt-url was given, so transcription is not served here")
            try:
                length = int(self.headers.get("Content-Length", "0"))
            except ValueError:
                return self.error(400, "bad Content-Length")
            if length <= 0 or length > MAX_AUDIO:
                return self.error(400, "audio missing or too large")
            body = self.rfile.read(length)
            req = urllib.request.Request(
                stt_url + "/v1/audio/transcriptions",
                data=body,
                headers={"Content-Type": self.headers.get("Content-Type", "application/octet-stream")},
                method="POST",
            )
            try:
                with urllib.request.urlopen(req, timeout=120) as r:
                    return self.reply(r.status, r.headers.get("Content-Type", "application/json"), r.read())
            except urllib.error.HTTPError as e:
                return self.reply(e.code, "application/json", e.read())
            except OSError:
                return self.error(502, "the speech-to-text server did not answer")

        def do_POST(self):
            if self.path == "/v1/audio/transcriptions":
                return self.forward_transcription()
            if self.path != "/v1/audio/speech":
                return self.error(404, "only /v1/audio/speech and /v1/audio/transcriptions are served")
            try:
                length = int(self.headers.get("Content-Length", "0"))
            except ValueError:
                return self.error(400, "bad Content-Length")
            if length <= 0 or length > MAX_BODY:
                return self.error(400, "request body missing or too large")
            try:
                req = json.loads(self.rfile.read(length))
            except (ValueError, UnicodeDecodeError):
                return self.error(400, "body is not JSON")
            if not isinstance(req, dict):
                return self.error(400, "body must be a JSON object")
            text = req.get("input")
            if not isinstance(text, str) or not text.strip() or len(text) > MAX_TEXT:
                return self.error(400, "`input` must be non-empty text of at most 4000 characters")
            fmt = req.get("response_format", "mp3")
            if fmt not in ("pcm", "wav"):
                return self.error(400, "response_format must be pcm or wav (no encoder is installed)")
            voice = req.get("voice")
            try:
                pcm, rate = voices.synthesize(voice if isinstance(voice, str) else "", text)
            except Exception as e:  # a bad voice file or engine failure must not kill the server
                sys.stderr.write("piper-shim: synthesis failed: %s\n" % type(e).__name__)
                return self.error(500, "synthesis failed")
            if fmt == "pcm":
                return self.reply(200, "audio/pcm", pcm, {"X-Sample-Rate": str(rate)})
            buf = io.BytesIO()
            with wave.open(buf, "wb") as w:
                w.setnchannels(1)
                w.setsampwidth(2)
                w.setframerate(rate)
                w.writeframes(pcm)
            self.reply(200, "audio/wav", buf.getvalue())

    return Handler


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    p.add_argument("--voice-dir", type=Path, required=True, help="folder holding NAME.onnx and NAME.onnx.json")
    p.add_argument("--voice", required=True, help="default voice name, e.g. en_US-joe-medium")
    p.add_argument("--host", default="127.0.0.1")
    p.add_argument("--port", type=int, default=8082)
    p.add_argument("--stt-url", help="forward /v1/audio/transcriptions to this whisper.cpp server, e.g. http://127.0.0.1:8081")
    a = p.parse_args()
    voices = Voices(a.voice_dir, a.voice)
    srv = ThreadingHTTPServer((a.host, a.port), make_handler(voices, a.stt_url.rstrip("/") if a.stt_url else None))
    print(f"piper-shim: voice {a.voice}, listening on http://{a.host}:{a.port}/v1", flush=True)
    try:
        srv.serve_forever()
    except KeyboardInterrupt:
        pass
    return 0


if __name__ == "__main__":
    sys.exit(main())
