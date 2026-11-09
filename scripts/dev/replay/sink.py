"""A stand-in speaker: appends the raw audio it receives on stdin to a file.

    python sink.py out.raw
"""

import sys

with open(sys.argv[1], "ab") as f:
    while True:
        block = sys.stdin.buffer.read(4096)
        if not block:
            break
        f.write(block)
        f.flush()
