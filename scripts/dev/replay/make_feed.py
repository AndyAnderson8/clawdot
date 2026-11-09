"""Builds the rehearsal microphone recording: a second of silence, a wake word clip, a pause, the question, and
three seconds of silence. All inputs are 16 kHz mono signed 16-bit: the clip as raw samples (.s16), the question
as a WAV file.

    python make_feed.py wake_clip.s16 question.wav feed.s16
"""

import sys
import wave


def silence(seconds: float) -> bytes:
    return b"\x00\x00" * int(16000 * seconds)


with wave.open(sys.argv[2]) as w:
    if (w.getframerate(), w.getnchannels(), w.getsampwidth()) != (16000, 1, 2):
        sys.exit("the question must be a mono 16-bit 16 kHz WAV")
    question = w.readframes(w.getnframes())
wake = open(sys.argv[1], "rb").read()
feed = silence(1.0) + wake + silence(0.4) + question + silence(3.0)
open(sys.argv[3], "wb").write(feed)
print(f"{len(feed) / 32000:.1f} seconds of microphone")
