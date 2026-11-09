# R-11: Wake-word spike (SPIKE-002, PC part of A3)

Date: 2026-11-09 · Feeds: roadmap A3 and the feasibility gate, ADR-0002 (Proposed), REQ-VOI-001 and REQ-VOI-009, spec 0002 section 7 question 2 · Refines: [R-03](03-wake-word.md), [R-08](08-existing-echo-dot-projects.md) section 6

No device was touched. Everything measured here ran on a PC with synthetic speech. The sample is tiny (section 5.1), so read the accuracy numbers as smoke tests, not as quality evidence.

## 1. Summary

1. **Recommended engine: microWakeWord** (streaming int8 models, 40 mel features every 10 ms, one inference per 30 ms). Code and the four official models are Apache-2.0, a model is 52-81 KB, its tensor arena is 22,860 bytes, and one inference is 22-36 thousand multiply-accumulates.
2. **Recommended runtime: a small purpose-built Rust runtime** (microfrontend port plus an interpreter for the 12 operators these models use), not TFLite over FFI and not tract. tract cannot load these models (its TFLite loader panics, section 5.4). No such runtime exists yet; writing and verifying it is the next spike step (section 8).
3. **Fallback and second option: openWakeWord on tract** (pure Rust, measured working, section 5.4), opt-in only. Its code is Apache-2.0 but **its pretrained models are CC BY-NC-SA 4.0**, so we may never ship them. It is only for users who train their own model.
4. **Baseline that always works: push-to-talk** (button, hotkey, or a portal "listen" action) feeding the same detection event. Energy/VAD gating is an optimisation in front of an engine, not a wake word.
5. **Rustpotter** is pure Rust and Apache-2.0 but unmaintained since 2023-10-01 and self-described as not production grade: rejected. **Porcupine** is rejected: closed models, AccessKey validated online, and its free tier is reported discontinued (section 3.4).
6. **Model source for v1.0:** ship no custom word (as the roadmap says). Offer the official microWakeWord models `okay_nabu` and `hey_mycroft` with recorded provenance. Avoid `alexa` (trademark) and be careful with `hey_jarvis` (fictional product name). A residual licence-provenance risk exists even for these (section 4).
7. **Custom "Hey Claw" / "ClawDot":** trainable with the microWakeWord pipeline, but **the pipeline's own notebook says models trained on its default data are for non-commercial personal use only**, and the pre-built negative features are CC BY-NC 4.0. A shippable custom word needs a licence-clean dataset manifest first. That is the "wake-word model workstream" already proposed in R-03.
8. **Measured on a Ryzen AI 9 PC, one thread, 5.6 minutes of audio:** openWakeWord ONNX 3.9% of a core (one word, Python plus ONNX Runtime) and 2.4% (pure Rust tract); microWakeWord 0.5% (one word, Python plus the TFLite C library) and 1.5% for four words at once.
9. **Feasibility gate (SPIKE-002): Go for the approach, with one explicit open item.** The microWakeWord path is far inside both thresholds by every proxy we can measure on a PC, but the Rust runtime that would carry it does not exist yet, so the armv7 RSS and CPU numbers are estimates until it does. The tract/openWakeWord path is measured end to end and fails on size and memory (8.6-13 MB binary, up to about 40 MB RSS under emulation) and is borderline on CPU, so it is an opt-in extra, not the default (sections 5.5 and 7).
10. **The seam** is a `WakeEngine` trait: 16 kHz mono S16 samples in, `Detection { word, score, end_sample }` out, per-word sensitivity, a built-in refractory period, and a table that routes each word to a profile (section 9).

## 2. What was asked and how it was answered

| Question | Where |
|---|---|
| Licence of code and of models, maintenance, size, language, armv7 musl, dependency weight, per candidate | Section 3 and the matrix in section 6 |
| How a custom word is trained and what the data licences imply | Section 4 |
| Measurements on this PC | Section 5 |
| Extrapolation to a Cortex-A53 at about 1.2 GHz | Section 7 |
| Decision, seam, gate, what remains for the Dot | Sections 6, 8, 9, 10 |

Method notes. Licences and dates come from primary sources read on 2026-11-09 (repository pages, GitHub and crates.io APIs, licence files); where a page did not state something, this note says "not stated" rather than guessing. The harnesses are in `spikes/wakeword/` (not a workspace member, not built by CI). No model or audio file is committed.

## 3. Candidates

### 3.1 openWakeWord (OWW) and a pure-Rust ONNX path

| | Finding | Source |
|---|---|---|
| Code licence | Apache-2.0 | repository README, GitHub licence field |
| **Model licence** | **CC BY-NC-SA 4.0** for the pretrained models, "due to the inclusion of datasets with unknown or restrictive licensing as part of the training data"; permissive models only "on request" | README, "License" |
| Maintenance | Last release v0.6.0, 2024-02-11. Last push 2025-12-30. About 2.9k stars. Single maintainer | GitHub API |
| Size | Per word on disk: classifier 0.85-1.27 MB (`alexa` 854 KB, `hey_mycroft` 858 KB, `hey_jarvis` 1.27 MB) plus shared `melspectrogram.onnx` 1.09 MB and `embedding_model.onnx` 1.33 MB: **about 3.3-3.7 MB for one word**. Measured, in the PyPI package | files on disk |
| Architecture | mel spectrogram model, then Google's speech-embedding model (96 dimensions, 76 mel frames), then a small per-word classifier over 16 embeddings; 80 ms frames | README, model graphs |
| Language and runtime | Python; onnxruntime, or tflite-runtime on Linux. A basic C++ port exists (R-03). Pure Rust: **tract-onnx loads all three ONNX models and runs the whole pipeline** (section 5.4) | README, measured |
| armv7 musl | tract-onnx cross-built and run (under QEMU) for `armv7-unknown-linux-musleabihf` (section 5.5). onnxruntime has no armv7 musl wheel in the usual channels; not attempted | measured |
| Dependency weight | Python path: 150 MB RSS before any model is loaded (Python 3.14 plus numpy plus onnxruntime), unusable. Rust path: tract adds about 120 crates (127 in the lock file including TFLite and hound) | measured |
| Accuracy claims | Target under 5% false reject and under 0.5 false accepts per hour, measured by the author on small corpora (Dinner Party Corpus about 5.5 h) | README |
| Training | Colab notebook, "basic model in under 1 hour", synthetic speech only, about 30,000 hours of negative data of mixed licence | README |

### 3.2 microWakeWord (mWW) and what a Rust runtime takes

| | Finding | Source |
|---|---|---|
| Code licence | Apache-2.0 (`OHF-Voice/micro-wake-word`, also `kahrendt/microWakeWord`) | GitHub API, README |
| Model licence | `esphome/micro-wake-word-models`: Apache-2.0 for the whole repository, "no separate or additional terms for the model binaries". **But** the training data's licences are not documented there (section 4) | repository README |
| Maintenance | Active: pushes 2026-07-06 (OHF-Voice) and 2026-07-09 (kahrendt); `pymicro-wakeword` 2.5.0 released 2026-10-27; README calls it an "early release" and says a model trained with the basic notebook "will most likely not be usable" | GitHub API, READMEs |
| Size | v2 models: `hey_jarvis` 52,272 B, `alexa` 55,856 B, `hey_mycroft` 57,248 B, `okay_nabu` 60,264 B in the official repo (80,824 B in `pymicro-wakeword` 2.5.0). **Tensor arena 22,860 B.** Weights about 24-40 KB. Measured | files, model JSON |
| Architecture | MixConv depthwise-separable streaming CNN; spectrogram features every 10 ms; model input is 3 frames x 40 features (30 ms); output is a sigmoid; detection is the mean of the last 5 outputs against a per-model cutoff (0.85-0.97 for the four official models) | README, model JSON |
| Operators | Counted from the model files: `CONV_2D`, `DEPTHWISE_CONV_2D`, `FULLY_CONNECTED`, `LOGISTIC`, `QUANTIZE`, `RESHAPE`, `CONCATENATION`, `STRIDED_SLICE`, `SPLIT_V` (one model), and the stateful `CALL_ONCE`, `VAR_HANDLE`, `READ_VARIABLE`, `ASSIGN_VARIABLE`. **12-13 operator kinds in total.** 21,630-36,480 multiply-accumulates per inference, which is 0.7-1.2 MMAC per second of audio. Measured | `spikes/wakeword/dump_ops.py` |
| Runtimes that exist | TFLite for Microcontrollers (C++); the TFLite C library via `pymicro-wakeword` (a 4.5 MB DLL on Windows); a Rust crate `micro_wakeword` 0.1.3 (MIT, 2026-10-22) over `tflite-c-rs`, x86_64 Linux only listed; a pure-Go port `zserge/microwakeword` (MIT, 1 star, last push 2026-03-30) that R-08 reports at about 30% of one core on the Dot | docs.rs, GitHub API, R-08 |
| tract | **Cannot load it.** `tract-tflite` 0.23.10 panics on `hey_jarvis.tflite` (`flatbuffers ... assertion failed: idx < self.len()`), as expected given the TFLite variable operators. Measured | section 5.4 |
| armv7 musl | A custom Rust runtime has no native dependency, so cross builds are trivial. TFLite C over FFI needs a C++ cross build and `unsafe` (our crates `forbid` unsafe, so it would live in a separate `-sys` crate) | reasoning |

**What a Rust runtime for it would take.** Two parts, both small and both with Apache-2.0 reference code:

1. The **microfrontend** (the TensorFlow Lite Micro audio frontend: 30 ms window, 10 ms step, 512-point FFT, 40 mel channels, 125-7500 Hz, noise reduction and per-channel gain control, log scale, scaled to the model's input range). It is a few hundred lines of C.
2. A **tiny interpreter** for the operators above, with int8 requantisation (fixed-point multiplier and shift) and a few byte arrays for the variable state. A flat-buffer reader for the `.tflite` file is also needed, about 200 lines without a code generator.

Estimated size: **1,000-1,500 lines of Rust, no dependencies beyond `core`/`alloc` (a flatbuffer reader and an FFT written by hand, or `rustfft`)**, 60-100 KB of code. A port must carry the Apache-2.0 notice of the microfrontend and the MIT and BSD notices R-08 lists for the Go port if it borrows from it. The bit-exact check is easy: `pymicro-wakeword` prints the probability for every window, and the Rust runtime must match it.

### 3.3 Rustpotter and other Rust wake-word crates

| Crate | Licence | Maintenance | Notes |
|---|---|---|---|
| `rustpotter` 3.0.2 | Apache-2.0 | **Last crate release and last push 2023-10-01** (3 years ago); 155 stars; 80k downloads | Pure Rust (`candle-core` 0.2.2, `rustfft`, `rubato`, `hound`). Two methods: DTW over MFCCs from 3 to 8 recordings ("references"), or a small trained model (320 KB to 3.1 MB). Author: "not intended to be a production-grade tool"; accuracy claims are qualitative only. Training uses the `rustpotter-cli`. Bundled demo model licences not stated. armv7/musl not mentioned |
| `livekit-wakeword` 0.1.3 | repository Apache-2.0; model licence not stated | New (2026); 312 stars | Rust crate on `ort-tract` (pure Rust ONNX), loads LiveKit-trained classifiers and embeds its own mel/embedding models. **Not openWakeWord compatible** (LiveKit says models from the older OWW-based implementation are not compatible). Training needs LiveKit Inference TTS (a cloud account; the free plan is said to work) and a CUDA GPU or an Apple silicon Mac. Worth re-checking in six months, not a base today |
| `oww-rs` 0.3.3 | not checked | | OWW inference over the `ort` crate (native ONNX Runtime): not pure Rust |
| `micro_wakeword` 0.1.3 | MIT | 2026-10-22 | mWW over the TFLite C library; see 3.2 |

### 3.4 Porcupine and other proprietary options

| | Finding |
|---|---|
| Code | `Picovoice/porcupine` is shown as Apache-2.0 (LICENSE file is the Apache text, "Copyright 2018 Picovoice Inc."). The licence text does not mention model files |
| Keys | Every SDK needs an **AccessKey** from the Picovoice Console. Picovoice's FAQ (found through search, not read in full) says engines call home to validate the key and check plan limits. Picovoice's own reply, relayed on the Home Assistant forum on 2026-06-04: "The SDK does not run without a valid key", "There is no non-commercial tier planned" and free-tier keys stop working on **2026-06-30**. That is a forum report of a vendor email, not a Picovoice page I could read; **verify before relying on it** |
| Custom words | Trained in the closed Picovoice Console; the resulting file is tied to the account and plan |
| Platforms | Raspberry Pi Zero, 3, 4, 5, Cortex-M, desktop, mobile, browsers. armv7 is not named. A Rust SDK was added in v2.0.0 but is reported unmaintained since 2025-07-15 (search result, unverified) |
| Verdict | **Not acceptable.** An open-source device daemon cannot depend on a vendor key check, a closed model format, an account for custom words and a plan that can change or end (REQ-VOI-001 wants detection with no outside dependency; REQ-VOI-009 wants models with a known licence). It can only be a user-supplied plugin, if ever |

Other proprietary engines (Sensory, Snowboy and similar) were not evaluated: the same objections apply to closed-model engines, and Snowboy is understood to be discontinued (from memory, not checked).

### 3.5 Energy, keyword spotting and push-to-talk

| Option | What it is | Verdict |
|---|---|---|
| **Push-to-talk** | Button, hotkey, portal action, or an HA/ESPHome `API_AUDIO` start; produces the same detection event | **Required baseline** (roadmap A3). Zero CPU, zero false accepts. It is also the Dot's button path later |
| Energy / VAD gate | Run the neural engine only when the frame energy or a VAD says speech may be present | An optimisation. The mWW repo ships a 34 KB `vad.tflite` and Silero VAD is a 1.8 MB ONNX (bundled in the OWW package); the engine already runs at under 1% on a PC, so gating only matters for the heavier OWW path |
| Energy trigger as a wake word | Start a turn on any loud sound | Not a wake word: unacceptable false accepts and a privacy problem (audio goes to STT on noise) |
| STT-based keyword spotting | Run a small streaming STT continuously and string-match | Far more CPU and RAM than either engine; no |
| Template matching (DTW), Rustpotter style | A few recordings of the user | Cheap and needs no training pipeline, but accuracy is speaker dependent and the crate is unmaintained. Keep as an idea for a no-training personal word, not a base |

## 4. Custom wake words: training and licence implications

| Engine | How "Hey Claw" or "ClawDot" is trained | Data and licence consequences |
|---|---|---|
| openWakeWord | Colab or local notebook: synthetic positives from TTS, about 30,000 h of negative audio and room impulse responses, small classifier over a frozen shared embedding. "Under 1 hour" for a basic result | Code and the Google embedding model are Apache-2.0 (README). The shipped OWW models are CC BY-NC-SA only because of their training data. **A model we train on licence-clean data would be ours to license as we choose**, but OWW names no clean negative set; we would have to build one |
| microWakeWord | `basic_training_notebook.ipynb`: Piper sample generator (`en_US-libritts_r-medium`) for positives; MIT room impulse responses, AudioSet (`bal_train09`) and a Free Music Archive subset for augmentation; pre-generated negative spectrograms from Hugging Face (`kahrendt/microwakeword`, 9.73 GB) built from CHiME6, DiPCo, FMA-medium, FSD50K, WHAM, LibriSpeech and VOiCES; Colab T4, 10,000 steps | **The notebook says the downloaded data "has a mixture of difference licenses and usage restrictions" and that models trained with it "should be considered as appropriate for non-commercial personal use only". The negative-feature dataset is CC BY-NC 4.0.** The official Apache-2.0 models were presumably trained on a similar mix, but their data card says nothing: that is the residual risk behind REQ-VOI-009 |
| Piper (the synthetic voice) | Needed by both pipelines | `rhasspy/piper` is archived (2025-08-26); its successor `OHF-Voice/piper1-gpl` is **GPL-3.0**. Each voice has its own model card and the Piper docs say some voices carry restrictive licences. Speech *generated* by a tool is not a derivative of the tool's code, but the voice's data licence can still bind. Choose voices with explicit permissive cards and record them |
| Rustpotter | Record 3-8 clips, or label wav files; `rustpotter-cli` | The user's own recordings: no third-party data. Weakest accuracy |
| Porcupine | Type the phrase in the Console | Proprietary and account-bound (section 3.4) |

**Implications for us**

- A shippable "Hey Claw" model needs a **dataset manifest with a licence per source**, a recorded TTS voice licence, and a reproducible training script kept outside the daemon repository's size budget. Candidate permissive negative sources must each be verified (I did not verify any beyond those quoted above); Common Voice, LibriSpeech and similar public corpora are the obvious starting points.
- A **personal** model trained by the user on the default notebook is fine for that user. We must not redistribute it.
- The wake phrase matters as much as the data: "Hey Claw" is two short syllables after "Hey" and will collide with "hey call", "hey clock"; "ClawDot" has no leading interjection. Prefer a phrase of four or more syllables ("Hey ClawDot") and test it against a hard negative list before training. Trademark and confusion checks go in R-06's process.
- The synthetic-only training of both engines is also why my TTS-based test below is not as unfair as it looks: both were trained on TTS.

## 5. Measurements on this PC

### 5.1 Setup, and why the numbers are only smoke tests

- Machine: AMD Ryzen AI 9 365 (Windows 11, and WSL2 Ubuntu for the Rust builds), 20 logical CPUs, 31 GB. Every figure is the CPU time of one thread (`process_time`) divided by the seconds of audio, with 1 s of silence before and after each clip.
- Audio: 75 clips, 338 seconds in total (5.6 minutes), generated locally with the Windows SAPI voices *David*, *Zira* and *Mark* at 16 kHz mono: 30 positives (10 per voice, phrases "hey jarvis", "alexa", "hey mycroft", "okay nabu", each alone and inside a short sentence) and 45 negatives (15 sentences per voice, including deliberate near-misses such as "a jar of marmalade", "the lexicon of modern english", "hey judy", "jarvis is not here today"). Nothing was downloaded as audio and nothing is committed. `spikes/wakeword/gen_clips.ps1` regenerates them.
- **This sample cannot measure false accepts per hour, any accent, any noise, any room, or a real microphone.** 5.6 minutes of clean synthetic speech is 0.09 hours. The positive set has 24-30 clips from three voices. Treat 4% versus 40% below as "something to look at", not as a ranking.
- Models used for measurement: the OWW pretrained models (CC BY-NC-SA, downloaded by the OWW tool into a scratch virtual environment, never committed, used for local evaluation only) and the official mWW v2 models bundled in `pymicro-wakeword` 2.5.0 (Apache-2.0).

### 5.2 openWakeWord, ONNX Runtime, Python (`spikes/wakeword/bench_oww.py`)

| Run | Positives detected | Clips with a false accept (of 45) | CPU per second of audio | RSS |
|---|---|---|---|---|
| 3 words (`hey_jarvis`, `alexa`, `hey_mycroft`), threshold 0.5 | 23 of 24 (FRR 4%) | 3 | **4.4% of one core** | 150 MB before load, 193 MB after load, 233 MB after the run (Python 3.14, numpy, onnxruntime 1.31) |
| 1 word (`hey_jarvis`), threshold 0.5 | 11 of 12 (FRR 8%) | 1 | **3.9% of one core** (three runs: 3.9, 4.0, 4.0) | same |

All three false accepts are sentences I put in as near-misses on purpose ("a jar of marmalade" scored 0.88; "the lexicon of modern english" scored 0.52 and 0.92 in two voices; the library output names the best score, not the word, but these are the phonetic neighbours of `hey_jarvis` and `alexa`). The mWW run produced one false accept on a plain sentence ("the quick brown fox...", voice *Mark*, `hey_mycroft`). The 3.9% for one word against 4.4% for three shows that **the shared mel and embedding stage costs about 3.7% and each extra word about 0.3%**. Memory in this run is dominated by the Python and ONNX Runtime environment and says nothing about a Rust build.

### 5.3 microWakeWord, TFLite C library, Python (`spikes/wakeword/bench_mww.py`)

Default probability cutoffs (0.85-0.97) and the library's 5-window average:

| Cutoff scale | Positives detected (30: `hey_jarvis` 12, the other three 6 each) | Clips with a false accept (of 45) |
|---|---|---|
| 1.00 (shipped cutoffs) | 18 of 30 (FRR 40%) | 1 |
| 0.90 | 18 of 30 | 2 |
| 0.80 | 19 of 30 | 2 |
| 0.70 | 20 of 30 | 3 |

CPU: **0.5% of one core for one model, 1.5% for four models** (so about 0.33% per extra model once the 10 ms frontend is shared). RSS in the Python harness: 35 MB before, 39 MB after (almost all Python, numpy and the 4.5 MB TFLite C DLL).

**Reading the accuracy rows.** The official mWW cutoffs are tuned for very few false accepts per hour on household audio, and these models were trained on Piper voices, not SAPI voices; my robotic SAPI voices, and the sentence-embedded phrases, fall below the cutoff more often. On the three words both engines cover, OWW detected 23 of 24 positives and mWW 16 of 24 (`hey_jarvis` 6 of 12, `alexa` 4 of 6, `hey_mycroft` 6 of 6; `okay_nabu`, which OWW does not have, 2 of 6), with comparable false accepts on this tiny set. **I would not conclude that OWW is more accurate.** I would conclude that neither engine is validated for us yet and that the evaluation set promised in R-03 (real voices, a real room, hours of household audio) is still the gating work for any engine.

### 5.4 Pure Rust: tract (`spikes/wakeword/tract-probe`)

A 170-line Rust program (tract 0.23.10, `tract-onnx`, `hound`) running the three OWW models as one streaming pipeline (1,760-sample windows every 1,280 samples, mel scaled `x / 10 + 2`, 76-frame embedding window, 16-embedding classifier).

| Result | Value |
|---|---|
| Models load and optimise | 78-104 ms for three models (x86) |
| Scores on clips | `hey_jarvis` clips 0.997, 0.993, 0.935; a TTS sentence with no wake word 0.000; the "jar of marmalade" near-miss 0.996 (the Python run flagged the same clip class). Scores agree in kind with the Python run but **not clip for clip**: the Python library keeps extra warm-up state that my port does not replicate (for example clip 56: 0.935 here, 0.36 in Python). So the port is functionally working, not bit-identical, and not a replacement for an accuracy test |
| CPU, x86 (WSL, 1 word, tract f32 kernels) | **2.4-2.5% of one core**, 3.6-4.8 s clips |
| `tract-tflite` on the mWW model | **Fails**: panic `flatbuffers ... vector.rs:96 assertion failed: idx < self.len()` |

### 5.5 Binary size (armv7 musl and x86_64)

The probe (tract-onnx plus `hound`, `panic = "abort"`, LTO, stripped) cross-built with Zig as linker for `armv7-unknown-linux-musleabihf`, static:

| Build | Size |
|---|---|
| `opt-level = 3` | **13.0 MB** (13,046,628 bytes) |
| `opt-level = "s"` | **8.6 MB** (8,641,652 bytes) |
| x86_64 glibc, `opt-level = 3` | 20.4 MB (20,366,832 bytes); 21.4 MB (21,350,400) in the earlier build that also linked `tract-tflite` |

This one pipeline alone is **larger than the whole 8 MB release-binary budget (REQ-NFR-103) even at `opt-level = "s"`**, before any ClawDot code. The size is tract's generic graph optimiser and operator set, so it will not shrink much by choosing different models. A custom runtime estimated at 60-100 KB (section 3.2) is two orders of magnitude smaller. This is the main reason tract is an opt-in sidecar or feature at best.

The two armv7 binaries were also run, unmodified, on `linux/arm/v7` under QEMU user-mode emulation (Docker Desktop, Alpine 3.20): they load the three models and produce the same score on the same clip (0.997) as the x86 build, so the armv7 musl build is functionally correct. Peak RSS reported by the process was **39.6 MB (`opt-level = "s"`) and 42.9 MB (`opt-level = 3`)**. That number includes the QEMU emulator's own memory (the process reads QEMU's `/proc`), so it is an upper bound for the tract path, not a measurement of it, and it is already above the 24 MB idle-RSS requirement before the rest of ClawDot loads. **Timing under emulation is meaningless** (490-540% of a core) and is not used anywhere in this note.

## 6. Decision matrix

Scores: **+** good, **0** acceptable, **-** problem, **x** blocking. "Dot fit" is against spec 0002 (idle RSS 24 MB, binary 8 MB, idle CPU 15% of one core including wake word).

| Criterion | OWW + tract (Rust) | OWW + ONNX Runtime | mWW + custom Rust runtime | mWW + TFLite C (FFI) | Rustpotter | Porcupine | Push-to-talk |
|---|---|---|---|---|---|---|---|
| Code licence | + Apache-2.0 | + Apache-2.0 | + Apache-2.0 (+ MIT/BSD notices if a port borrows) | + Apache-2.0 | + Apache-2.0 | 0 Apache text, but key-gated | + ours |
| **Model licence** | **x** pretrained CC BY-NC-SA; own models only | **x** same | + Apache-2.0 declared; 0 training-data provenance undocumented | same | 0 unstated | x closed | n/a |
| Maintenance | 0 single maintainer, last release 2024-02 | 0 | + active, OHF-backed (2026-07..10) | + | x last release 2023-10 | 0 vendor, free tier reported ended | n/a |
| Model size per word | - 3.3-3.7 MB | - same | + 52-81 KB | + | 0 0.3-3.1 MB | 0 | + 0 |
| Measured PC CPU (one word) | 0 2.4% (Rust) | - 3.9% (with Python) | + 0.5% (with Python; Rust expected lower) | + 0.5% | not measured | not measured | + 0 |
| Estimated A53 CPU (section 7) | 0 about 10-20% | x no armv7 musl build | + about 1-5% | 0 about 1-5% | not measured | 0 | + 0 |
| RSS delta on the Dot | - up to about 40 MB under emulation (upper bound, includes QEMU); to be measured | - | + well under 1 MB | 0 TFLite C runtime a few MB | 0 | 0 | + 0 |
| Binary size | x 8.6-13 MB for the pipeline alone (section 5.5) | - | + 60-100 KB estimate | - TFLite C++ static build | 0 | 0 | + 0 |
| armv7 musl, no `unsafe` | + builds (measured) | x | + trivial, pure Rust | - needs a `-sys` crate and a C++ cross build | + (untested) | - | + |
| Dependency weight | - about 120 crates | - Python | + near zero | 0 | 0 | - | + |
| Custom word effort | 0 notebook, data licence needed | 0 | 0 notebook, data licence needed, harder to tune ("most likely not usable") | 0 | + 3-8 recordings | + Console | n/a |
| Accuracy evidence | 0 author claims; tiny local smoke test | same | 0 none independent; weaker on my tiny SAPI test | same | - none | 0 vendor claims | + |
| **Overall** | **Optional feature / sidecar for users with their own model** | Reject for the daemon | **Recommended default (after the Rust runtime is verified)** | Fallback if the Rust runtime proves hard | Reject | Reject | **Always on** |

## 7. Extrapolation to a Cortex-A53 at about 1.2 GHz, and the feasibility gate

### 7.1 Assumptions (all of them uncertain)

- R-08 reports **23% of one A53 core for openWakeWord with ONNX Runtime** (EchoMuse, one word, with `embedding_stream.onnx`) and **about 30% for the pure-Go microWakeWord port** on the T5D bench unit. Those are the projects' own numbers, not mine.
- My PC measurement of the same OWW stack (ONNX Runtime, one word) is 3.9%. That gives a **PC-to-A53 factor of about 6** for this kind of workload (23 / 3.9). The Ryzen core is much faster than an in-order A53 (higher clock, much higher IPC, better SIMD), so a factor between 6 and 15 is plausible; I use 6-12 as the range.
- The factor is not constant. tract's armv7 kernels are not as tuned as ONNX Runtime's on x86 or ARM. I treat the tract number as "PC figure times 6 to 12, and possibly worse".
- Scaling for mWW is by operation count, not by the PC factor, because the PC runs measured Python overhead and a C library.

### 7.2 Estimates

| Path | PC measure | Estimate on a 1.2 GHz A53 | Against the SPIKE-002 gate (RSS delta at most 8 MB, idle CPU at most 15%) |
|---|---|---|---|
| OWW, ONNX Runtime | 3.9% | 23% (R-08, their measurement) | CPU: Investigate (about 1.5x). No armv7 musl runtime |
| **OWW, tract (Rust)** | 2.4% | **about 14-29%** (2.4 x 6-12) | CPU: Go to Investigate boundary; RSS to measure |
| mWW, a purpose-built Rust runtime | model 22-36 K MAC per 30 ms = 0.7-1.2 MMAC/s; frontend about 100 FFTs of 512 points per second | **about 1-5%**: the arithmetic is about 1 MMAC/s plus about 3-5 MFLOP/s for the FFT and mel stages, which a scalar A53 does at tens to hundreds of MFLOP/s; the range allows for naive code and interpreter overhead | CPU: Go. RSS: Go (arena 23 KB, weights 24-40 KB, frontend tables a few KB) |
| mWW, pure Go (R-08) | n/a | 30% (their measurement) | Investigate. The gap to my 1-5% estimate is exactly why the Rust runtime must be measured: it says the Go interpreter, not the model, is the cost |

### 7.3 Gate

| Spike | Threshold (Go / Investigate / Revise) | Result |
|---|---|---|
| SPIKE-002 RSS delta | at most 8 MB / up to 2x / beyond | mWW Rust runtime: **Go** (estimate, well under 1 MB). OWW tract: **Revise-class on the evidence we have**: 40 MB peak under emulation including QEMU's own memory (section 5.5), and a 8.6-13 MB binary; even if half of that is QEMU, it is beyond 2x the 8 MB limit |
| SPIKE-002 idle CPU | at most 15% / up to 2x / beyond | mWW Rust runtime: **Go** (estimate 1-5%). OWW tract: **Go to Investigate** (estimate 14-29%) |

**Verdict: Go for the approach (microWakeWord with a Rust runtime, push-to-talk always available, OWW on tract as an opt-in), not yet a measured Go.** I deliberately do not claim a measured Go: the recommended runtime does not exist, so its armv7 figures are estimates derived from operation counts and from the OWW calibration above. Treat the first measurement of the Rust runtime under qemu and on the Dot as the real gate. If it came back above 15% the outcome would be Investigate (optimise, or fall back to OWW on tract with its larger footprint), not Revise, because the mWW arithmetic leaves more than a 3x margin even if the first implementation is naive. The OWW-on-tract path is the one that would be Revise (size and memory), which is why it is not the default.

## 8. Recommendation

| Item | Recommendation |
|---|---|
| Engine | microWakeWord (v2 streaming models) |
| Runtime | New small crate (name suggestion `wake-mww`): microfrontend port + int8 interpreter for the 12 operators + `.tflite` reader. `unsafe_code = forbid`, no dependencies except possibly an FFT. Lives behind a cargo feature so a build without wake word (push-to-talk only) is unaffected. Verify against `pymicro-wakeword` probabilities on the same clips (tolerance 0.02), then measure RSS and CPU under qemu-user armv7 and on the Dot |
| Model source | Official `esphome/micro-wake-word-models` `okay_nabu` and `hey_mycroft` (Apache-2.0), downloaded or bundled with a provenance record (URL, commit, SHA-256, licence text), per REQ-VOI-009. Do not ship `alexa`; treat `hey_jarvis` as a trademark question for the maintainer |
| Licence path | v1.0: only official Apache-2.0 models plus a "bring your own model" setting. v1.x: a licence-clean "ClawDot" model from a recorded dataset manifest (section 4). Never bundle OWW's pretrained models or the community collection R-08 rejected |
| Fallback | (a) Push-to-talk, always. (b) Optional `oww-tract` feature, off by default, for users who bring their own OWW model; measured working here; 8.6-13 MB of binary and tens of MB of RAM, so it does not fit the Dot's default budget and belongs in PC and Pi builds or a sidecar. (c) If the Rust runtime is harder than estimated, a separate `-sys` crate over the TFLite C library on PC and Pi targets only |
| Not recommended | Rustpotter, Porcupine, STT keyword spotting, energy triggers |

## 9. The seam the daemon should expose

The core already has a satellite protocol with `detection` events (spec 0003); the engine below sits inside the audio layer, before that. This is a proposal for A3; I did not touch `crates/core` or any spec.

```rust
/// 16 kHz mono signed 16-bit PCM. Callers feed frames of any length; 160 samples (10 ms) is the natural unit.
pub trait WakeEngine: Send {
    /// Words this engine currently listens for, in a stable order.
    fn words(&self) -> &[WordInfo];
    /// Feed audio. Appends zero or more detections to `out`. No allocation after construction.
    fn process(&mut self, pcm: &[i16], out: &mut Vec<Detection>);
    /// 0.0 (strict, fewest false accepts) to 1.0 (loose, fewest misses); mapped per model onto its cutoff.
    fn set_sensitivity(&mut self, word: WordId, sensitivity: f32);
    /// Drop all streaming state (after mute, after a turn, after a long gap).
    fn reset(&mut self);
}

pub struct WordInfo { pub id: WordId, pub phrase: String, pub model_sha256: [u8; 32], pub licence: String }
pub struct Detection {
    pub word: WordId,
    pub score: f32,        // engine confidence, 0.0 to 1.0
    pub end_sample: u64,   // stream offset (samples since start) where the phrase ended
}
```

Rules that go with it:

1. **Frame in, event out.** No audio leaves the engine and none is stored by it. The audio layer keeps a bounded pre-roll ring (for example 1 s) so STT can start with the wake phrase included if it wants to; the ring is cleared on mute (REQ-VOI-008, REQ-VOI-001).
2. **Muted means not fed.** The caller stops calling `process` while muted and calls `reset` on unmute.
3. **Refractory period** inside the engine (default 2 s, configurable) so one utterance yields one detection. Sliding-window averaging (5 windows for mWW) is inside the engine too.
4. **Sensitivity** is one normalised knob per word. The mapping is `cutoff = lerp(strict, loose, s)` with the model's own published cutoff at `s = 0.5`; the portal shows the normalised value only.
5. **Several words, one engine.** All words share the frontend (mWW: one frontend, N small models; about 0.3% each on the PC). Each configured word routes to a **profile** (persona, STT language, tool set, voice) in the session layer; the engine knows nothing about profiles:

   ```toml
   [[wake]]
   word = "okay_nabu"            # model id; "ptt" is the built-in push-to-talk source
   sensitivity = 0.5
   profile = "default"

   [[wake]]
   word = "hey_mycroft"
   sensitivity = 0.6
   profile = "kitchen"
   ```

6. **Push-to-talk is another source of the same event** (`WordId::PTT`, score 1.0), so the session layer has one entry point. Remote satellites (ESPHome `API_AUDIO`, Wyoming) send their own `detection`, which also maps to a word id and profile.
7. **Provenance is part of the API**: `WordInfo` carries the model hash and licence string so the portal can show them and REQ-VOI-009 can be checked at start-up.

## 10. What remains to verify

On a PC (A3, no hardware):

- Write the Rust microWakeWord runtime and check it against `pymicro-wakeword` probabilities on the same clips.
- Build the evaluation set R-03 asks for: real voices, several microphones and rooms, an hour of household audio (television, kitchen, music), a hard-negative phrase list, with licences recorded. This decides the accuracy question that 5.1 cannot.
- Decide the "ClawDot" phrase and the dataset manifest (section 4).

On the Dot (B0, only with the maintainer's explicit go for each device action):

- Real RSS and CPU of the chosen runtime on the four A53 cores with the T5D or EchoMuse audio front end running (AEC and beamforming cost shares the same core budget).
- Whether the Dot's own front end already runs a wake word (T5D `echod` does) and ClawDot can simply subscribe to its detections over ESPHome; then the in-daemon engine is only needed on PC and Pi.
- Cold-start to wake-ready against the 5 s budget, and whether model loading stays under 1 MB of heap.
- Thermal and Wi-Fi load effects on CPU in a 24 h soak.

## Spec impact (recommended; nothing applied here)

- ADR-0002: keep Proposed; the proposal text becomes "microWakeWord via a small Rust runtime; TFLite FFI only as a fallback".
- REQ-VOI-009: add "provenance includes the model file hash and the declared training-data licence where one exists", and a note that Apache-2.0 on the weights does not by itself prove the training data is clean.
- Spec 0001: add REQ-VOI-0xx for push-to-talk as a required, always-available start source; add the `WakeEngine` seam to the voice-pipeline architecture note.
- Spec 0002 section 7 question 2: answer with the section 7 table once the Rust runtime is measured.
- Roadmap A3: split into A3a (PC finding: this note) and A3b (Rust runtime plus evaluation set).

## Confidence

Licences and maintenance dates: high (primary pages and API reads, 2026-11-09), except Picovoice's current terms (low: forum report of a vendor email, pricing page not rendered) and the dataset licences quoted from the training notebook and dataset card (high for what they say, not independently verified against the original datasets). PC CPU figures: high for what they are (repeatable, three runs for the Python OWW figure, 3.9-4.0%). Accuracy: low (tiny synthetic sample). armv7 estimates: low to medium, with the stated assumptions. Everything about the not-yet-written Rust runtime is a plan.

## Sources

- openWakeWord: https://github.com/dscripka/openWakeWord (README, "License"; releases)
- microWakeWord: https://github.com/OHF-Voice/micro-wake-word , https://github.com/kahrendt/microWakeWord (README, `notebooks/basic_training_notebook.ipynb`), dataset https://huggingface.co/datasets/kahrendt/microwakeword
- Models: https://github.com/esphome/micro-wake-word-models ; ESPHome page https://esphome.io/components/micro_wake_word.html
- Runtimes: https://github.com/OHF-Voice/pymicro-wakeword , https://docs.rs/micro-wakeword , https://github.com/sonos/tract
- Rustpotter: https://github.com/GiviMAD/rustpotter , https://crates.io/crates/rustpotter
- LiveKit: https://github.com/livekit/livekit-wakeword , https://docs.rs/crate/livekit-wakeword/latest
- Porcupine: https://github.com/Picovoice/porcupine ; forum report https://community.home-assistant.io/t/fyi-picovoice-confirmed-free-tier-accesskeys-will-stop-working-after-june-30-2026/1012744
- Piper: https://github.com/OHF-Voice/piper1-gpl , https://github.com/rhasspy/piper (archived)
- Harnesses: `spikes/wakeword/` in this repository
