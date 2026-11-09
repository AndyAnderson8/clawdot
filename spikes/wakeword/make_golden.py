#!/usr/bin/env python
"""Generate the golden data for crates/wake (spec 0007, REQ-WAK-011) from the reference implementation.

Reference: pymicro-wakeword (TensorFlow Lite C library) and pymicro-features (TFLite Micro audio frontend),
the code R-11 measured. Run it in a scratch virtual environment outside the repository:

    python -m venv venv && venv/bin/pip install pymicro-wakeword numpy scipy
    venv/bin/python -I make_golden.py MODEL_DIR OUT_DIR

MODEL_DIR holds okay_nabu.tflite/.json and hey_mycroft.tflite/.json from esphome/micro-wake-word-models
(Apache-2.0). OUT_DIR is crates/wake/tests/golden. All audio here is synthesised by this script from a seeded
random generator: no recording of a person, no third-party audio. The outputs are:

    <clip>.s16                  the clip, 16 kHz mono signed 16-bit little-endian
    <clip>.features.u16         frontend output, unsigned 16-bit little-endian, 40 values per frame
    <model>.<clip>.txt          per feature frame: raw model output (0..255, or -1 when no inference ran that
                                frame) and the sliding-window mean the reference returned
    <model>.qstream.i8 / .txt   int8 model inputs (40 per row) and the reference outputs for them
    frames.s16, frames.features.txt   independent 480-sample frames and the reference's first feature frame

Licence of the generated data: the same as this repository (Apache-2.0); it derives only from this script.
"""
import os
import sys

import numpy as np
from scipy.signal import lfilter

from pymicro_features import MicroFrontend
from pymicro_wakeword import MicroWakeWord
import pymicro_wakeword

SR = 16000


def resonator(x, f, bw):
    r = np.exp(-np.pi * bw / SR)
    theta = 2 * np.pi * f / SR
    a = [1, -2 * r * np.cos(theta), r * r]
    return lfilter([1 - r], a, x)


def vowel(rng, dur, f0a, f0b, formants):
    n = int(dur * SR)
    f0 = np.linspace(f0a, f0b, n)
    phase = np.cumsum(f0 / SR)
    src = np.zeros(n)
    for h in range(1, 40):
        src += np.sin(2 * np.pi * h * phase) / h
    out = np.zeros(n)
    for f, bw, g in formants:
        out += g * resonator(src, f, bw)
    env = np.hanning(n) ** 0.5
    return out * env


def fricative(rng, dur, lo, hi):
    n = int(dur * SR)
    x = rng.standard_normal(n)
    y = resonator(x, (lo + hi) / 2, hi - lo)
    return y * np.hanning(n)


def speech_like(seed, seconds):
    rng = np.random.default_rng(seed)
    vowels = [
        [(700, 90, 1.0), (1200, 110, 0.6), (2600, 160, 0.3)],
        [(300, 70, 1.0), (2300, 120, 0.5), (3000, 170, 0.3)],
        [(500, 80, 1.0), (1000, 100, 0.7), (2500, 150, 0.3)],
        [(350, 70, 1.0), (800, 90, 0.5), (2400, 150, 0.2)],
    ]
    pieces = [np.zeros(int(0.2 * SR))]
    t = 0.0
    while t < seconds - 0.4:
        v = vowels[rng.integers(len(vowels))]
        d = rng.uniform(0.12, 0.28)
        f0 = rng.uniform(100, 180)
        pieces.append(vowel(rng, d, f0, f0 * rng.uniform(0.85, 1.2), v) * rng.uniform(0.5, 1.0))
        t += d
        if rng.random() < 0.5:
            fd = rng.uniform(0.05, 0.14)
            pieces.append(fricative(rng, fd, 3000, 6000) * 0.15)
            t += fd
        else:
            g = rng.uniform(0.02, 0.08)
            pieces.append(np.zeros(int(g * SR)))
            t += g
    x = np.concatenate(pieces)[: int(seconds * SR)]
    x = np.pad(x, (0, int(seconds * SR) - len(x)))
    x = x / np.abs(x).max() * 9000.0
    x += rng.standard_normal(len(x)) * 40.0
    return np.clip(x, -32768, 32767).astype(np.int16)


# Vowel-and-consonant sequences in the rough shape of the two phrases (formant 1, 2, 3, seconds, gain). They
# are formant-synthesised sounds, not speech of anyone; the models respond to them because they are voiced
# syllable sequences, which makes the output pass through the middle of its range (a useful test).
PHRASES = {
    "phrase_a": ([[500, 900, 2500, 0.18, 1.0], [700, 2000, 2800, 0.16, 0.9], [300, 1000, 2400, 0.08, 0.5],
                  [750, 1200, 2600, 0.15, 1.0], [250, 800, 2300, 0.07, 0.4], [300, 800, 2300, 0.2, 0.9]], 120, 0.95, 0.3),
    "phrase_b": ([[614, 1763, 2869, 0.289, 1.23], [478, 758, 2730, 0.204, 0.45], [965, 1863, 2363, 0.214, 0.37],
                  [220, 1125, 2192, 0.095, 0.19], [324, 850, 1699, 0.087, 0.47], [679, 820, 2511, 0.195, 1.40],
                  [300, 1026, 2565, 0.098, 0.23]], 160, 0.9, 0.5),
}


def phrase(name, seed, amp):
    segs, f0, glide, burst = PHRASES[name]
    rng = np.random.default_rng(seed)
    parts = [np.zeros(int(0.25 * SR))]
    for f1, f2, f3, d, g in segs:
        parts.append(vowel(rng, d, f0, f0 * glide, [(f1, 80, 1.0), (f2, 110, 0.6), (f3, 160, 0.3)]) * g)
        if rng.random() < burst:
            parts.append(fricative(rng, 0.03, 1500, 4000) * 0.2)
    parts.append(np.zeros(int(0.35 * SR)))
    x = np.concatenate(parts)
    x = x / np.abs(x).max() * amp
    x = x + rng.standard_normal(len(x)) * 30
    return np.clip(x, -32768, 32767).astype(np.int16)


def tones(seed, seconds):
    rng = np.random.default_rng(seed)
    n = int(seconds * SR)
    t = np.arange(n) / SR
    x = 3000 * np.sin(2 * np.pi * 440 * t) * (0.5 + 0.5 * np.sin(2 * np.pi * 3 * t))
    x += 2000 * np.sin(2 * np.pi * 1800 * t + 3 * np.sin(2 * np.pi * 5 * t))
    x += 600 * rng.standard_normal(n)
    a = int(0.35 * n)
    x[a:a + int(0.12 * SR)] = 0
    b = int(0.7 * n)
    x[b:b + int(0.05 * SR)] *= 8
    x[b + 100:b + 140] = -32768
    return np.clip(x, -32768, 32767).astype(np.int16)


def frames(seed, count):
    """Independent 480-sample frames of very different level and shape (the first-frame test)."""
    rng = np.random.default_rng(seed)
    out = []
    for k in range(count):
        amp = 10 ** rng.uniform(0.5, 4.5)
        t = np.arange(480) / SR
        kind = k % 3
        if kind == 0:
            x = amp * rng.standard_normal(480)
        elif kind == 1:
            x = amp * np.sin(2 * np.pi * rng.uniform(100, 7800) * t) + 0.1 * amp * rng.standard_normal(480)
        else:
            x = amp * (np.sin(2 * np.pi * rng.uniform(100, 900) * t) + 0.5 * np.sin(2 * np.pi * rng.uniform(1000, 4000) * t)) * rng.uniform(0.2, 1)
        out.append(np.clip(x, -32768, 32767).astype(np.int16))
    return np.concatenate(out)


def features_of(pcm):
    fe = MicroFrontend()
    b = pcm.tobytes()
    rows = []
    for i in range(0, len(b) - 319, 320):
        r = fe.process_samples(b[i:i + 320])
        if r.features:
            # The library returns the unsigned frontend output divided by 25.6 (as 1/256 steps).
            rows.append(np.round(np.array(r.features) * 25.6).astype(np.uint16))
    return np.array(rows)


def load_model(model_dir, name):
    import glob
    lib = glob.glob(os.path.join(os.path.dirname(pymicro_wakeword.__file__), "lib", "*tensorflowlite_c.*"))[0]
    return MicroWakeWord.from_config(os.path.join(model_dir, name + ".json"), libtensorflowlite_c_path=lib)


def run_reference(m, float_feats):
    """Return (raw uint8 output or -1, window mean) per feature frame."""
    m.reset()
    res = []
    for f in float_feats:
        mean = m.process_streaming_prob(f.reshape(1, 1, 40))
        ran = len(m._features) == 0  # the stride filled and an inference ran on this frame
        raw = int(round(m._probabilities[-1] * 256)) if ran and len(m._probabilities) else -1
        res.append((raw, mean))
    return res


def main():
    model_dir, out = sys.argv[1], sys.argv[2]
    os.makedirs(out, exist_ok=True)
    clips = {
        "speech": speech_like(2026, 1.6),
        "tones": tones(7, 1.2),
        "phrase_a": phrase("phrase_a", 1002, 9000),
        "phrase_b": phrase("phrase_b", 1144, 25000),
    }
    for name, pcm in clips.items():
        pcm.tofile(os.path.join(out, name + ".s16"))
        feats = features_of(pcm)
        feats.astype("<u2").tofile(os.path.join(out, name + ".features.u16"))
        print(name, pcm.shape, feats.shape)
    fr = frames(11, 30)
    fr.tofile(os.path.join(out, "frames.s16"))
    first = []
    for k in range(30):
        fe = MicroFrontend()
        x = fr[k * 480:(k + 1) * 480].tobytes()
        r = None
        for i in range(0, 960, 320):
            r = fe.process_samples(x[i:i + 320])
        first.append(np.round(np.array(r.features) * 25.6).astype(int))
    np.savetxt(os.path.join(out, "frames.features.txt"), np.array(first), fmt="%d")

    for name in ("okay_nabu", "hey_mycroft"):
        m = load_model(model_dir, name)
        scale, zp = m.input_scale, m.input_zero_point
        for cname in clips:
            feats = np.fromfile(os.path.join(out, cname + ".features.u16"), dtype="<u2").reshape(-1, 40)
            res = run_reference(m, feats.astype(np.float64) / 25.6)
            with open(os.path.join(out, f"{name}.{cname}.txt"), "w") as f:
                for raw, mean in res:
                    f.write(f"{raw} {mean:.9f}\n")
            raws = [r for r, _ in res if r >= 0]
            print(name, cname, "max raw", max(raws), "mid-range outputs", sum(1 for r in raws if 8 <= r <= 247),
                  "max window mean", round(max(mean for _, mean in res), 4))
        # Random int8 streams at several levels, for the kernels; the model output is mostly saturated for such
        # input, so keep the stream with the most outputs away from 0.
        best = None
        for seed in range(6):
            base = np.random.default_rng(1000 + seed).standard_normal((240, 40))
            for level in np.geomspace(3, 90, 24):
                for off in (-60, -30, 0, 20):
                    q = np.clip(np.round(zp + off + level * base), -128, 127).astype(np.int8)
                    r = run_reference(m, (q.astype(np.float64) - zp) * scale)
                    mids = sum(1 for raw, _ in r if 1 <= raw <= 254)
                    if best is None or mids > best[0]:
                        best = (mids, q, r)
        mids, q, res = best
        q.tofile(os.path.join(out, name + ".qstream.i8"))
        with open(os.path.join(out, name + ".qstream.txt"), "w") as f:
            for raw, mean in res:
                f.write(f"{raw} {mean:.9f}\n")
        print(name, "random stream outputs between 1 and 254:", mids, "scale", scale, "zp", zp,
              "cutoff", m.probability_cutoff, "window", m.sliding_window_size)


if __name__ == "__main__":
    main()
