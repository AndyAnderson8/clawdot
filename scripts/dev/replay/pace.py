"""A stand-in microphone: plays a raw 16 kHz mono signed 16-bit recording to stdout in real time (one 20 ms frame
every 20 ms), then silence forever, so the voice loop can be rehearsed with no audio hardware.

    python pace.py feed.s16
"""

import sys
import time

data = open(sys.argv[1], "rb").read()
out = sys.stdout.buffer
step = 640
t0 = time.monotonic()
i = 0
while True:
    chunk = data[i * step:(i + 1) * step]
    if len(chunk) < step:
        chunk = b"\x00" * step
    try:
        out.write(chunk)
        out.flush()
    except (BrokenPipeError, OSError):
        break
    i += 1
    delay = t0 + i * 0.02 - time.monotonic()
    if delay > 0:
        time.sleep(delay)
