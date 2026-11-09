"""SPIKE-002 harness: openWakeWord (ONNX) on a few synthetic clips.

Usage: python -I bench_oww.py CLIP_DIR [threshold]
Clips are 16 kHz mono 16-bit WAVs named pos__<model>__N.wav or neg__N.wav (see gen_clips.ps1).
Models are the openWakeWord pretrained ones (CC BY-NC-SA 4.0): used for local measurement only, never committed.
Reports CPU seconds per audio second (process time, one thread), RSS, and per-clip max score.
"""
import sys, time, glob, os, wave
import numpy as np
import psutil
import openwakeword
from openwakeword.model import Model

clips = sys.argv[1]
thr = float(sys.argv[2]) if len(sys.argv) > 2 else 0.5
names = sys.argv[3].split(",") if len(sys.argv) > 3 else ["hey_jarvis", "alexa", "hey_mycroft"]
proc = psutil.Process()
rss0 = proc.memory_info().rss
m = Model(wakeword_models=list(names), inference_framework="onnx")
rss1 = proc.memory_info().rss

def load(p):
    with wave.open(p) as w:
        assert w.getframerate() == 16000 and w.getnchannels() == 1
        return np.frombuffer(w.readframes(w.getnframes()), dtype=np.int16)

res = []
tot_audio = 0.0
cpu0 = time.process_time()
for p in sorted(glob.glob(os.path.join(clips, "*.wav"))):
    a = load(p)
    a = np.concatenate([np.zeros(16000, np.int16), a, np.zeros(16000, np.int16)])  # 1 s lead and tail
    m.reset()
    best = {n: 0.0 for n in names}
    for i in range(0, len(a) - 1280 + 1, 1280):  # 80 ms frames
        sc = m.predict(a[i:i + 1280])
        for k, v in sc.items():
            for n in names:
                if k.startswith(n):
                    best[n] = max(best[n], float(v))
    tot_audio += len(a) / 16000
    res.append((os.path.basename(p), best))
cpu = time.process_time() - cpu0
rss2 = proc.memory_info().rss

tp = fn = fp = 0
npos = nneg = 0
for f, best in res:
    if f.startswith("pos__"):
        want = f.split("__")[1]
        if want not in names:
            continue  # no pretrained openWakeWord model for this phrase
        npos += 1
        if best[want] >= thr: tp += 1
        else: fn += 1
        print(f"{f:40s} want {want:12s} score {best[want]:.2f}  (others max {max([v for k,v in best.items() if k!=want] or [0]):.2f})")
    else:
        nneg += 1
        mx = max(best.values())
        if mx >= thr: fp += 1
        print(f"{f:40s} neg                  max score {mx:.2f}")
print(f"\nthreshold {thr}: positives {tp}/{npos} detected (FRR {fn/npos:.0%}); negatives {fp}/{nneg} false accepts")
print(f"audio {tot_audio:.0f} s; CPU {cpu:.2f} s -> {cpu/tot_audio*100:.1f}% of one core; python+ort RSS before {rss0/1e6:.0f} MB, after load {rss1/1e6:.0f} MB, after run {rss2/1e6:.0f} MB")
