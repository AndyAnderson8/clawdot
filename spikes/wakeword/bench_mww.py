"""SPIKE-002 harness: microWakeWord (TFLite, int8) through pymicro-wakeword.

Usage: python -I bench_mww.py CLIP_DIR [model ...]
Models are the esphome/micro-wake-word-models v2 files bundled in pymicro-wakeword (Apache-2.0 incl. weights).
Feeds 10 ms chunks; a detection is the library's own rule (mean of the sliding window >= probability_cutoff).
Reports CPU seconds per audio second (process time, includes the TFLite C library and the C micro frontend).
"""
import sys, time, glob, os, wave
import numpy as np
import psutil
from pymicro_wakeword import MicroWakeWord, MicroWakeWordFeatures, Model

clips = sys.argv[1]
want_models = sys.argv[2:] or ["hey_jarvis", "alexa", "hey_mycroft", "okay_nabu"]
proc = psutil.Process()
rss0 = proc.memory_info().rss
enum = {m.value: m for m in Model}
mods = {n: MicroWakeWord.from_builtin(enum[n]) for n in want_models}
rss1 = proc.memory_info().rss
feats = MicroWakeWordFeatures()
print("cutoffs:", {n: (m.probability_cutoff, m.sliding_window_size) for n, m in mods.items()})

def load(p):
    with wave.open(p) as w:
        assert w.getframerate() == 16000 and w.getnchannels() == 1
        return w.readframes(w.getnframes())

rows = []
audio_s = 0.0
cpu0 = time.process_time()
for p in sorted(glob.glob(os.path.join(clips, "*.wav"))):
    b = load(p)
    b = bytes(32000) + b + bytes(32000)
    feats.reset() if hasattr(feats, "reset") else None
    for m in mods.values():
        m.reset() if hasattr(m, "reset") else None
    fired = {n: 0.0 for n in mods}  # max sliding-window mean probability over the clip
    for i in range(0, len(b) - 320 + 1, 320):
        for f in feats.process_streaming(b[i:i + 320]):
            for n, m in mods.items():
                pr = m.process_streaming_prob(f)
                if pr is not None:
                    fired[n] = max(fired[n], float(pr))
    audio_s += len(b) / 32000
    rows.append((os.path.basename(p), fired))
cpu = time.process_time() - cpu0
rss2 = proc.memory_info().rss

def report(scale):
    tp = fn = fp = npos = nneg = 0
    for f, mx in rows:
        if f.startswith("pos__"):
            w = f.split("__")[1]
            if w not in mods:
                continue
            npos += 1
            ok = mx[w] >= mods[w].probability_cutoff * scale
            tp += ok; fn += (not ok)
        else:
            nneg += 1
            if any(v >= mods[k].probability_cutoff * scale for k, v in mx.items()):
                fp += 1
    print(f"cutoff x{scale:.2f}: positives {tp}/{npos} detected (FRR {fn/max(npos,1):.0%}); negative clips with a false accept {fp}/{nneg}")

for sc in (1.0, 0.95, 0.9, 0.8, 0.7):
    report(sc)
print(f"audio {audio_s:.0f} s; CPU {cpu:.2f} s -> {cpu/audio_s*100:.1f}% of one core with {len(mods)} model(s); RSS before {rss0/1e6:.0f} MB, after load {rss1/1e6:.0f} MB, after run {rss2/1e6:.0f} MB")
