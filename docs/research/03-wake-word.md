# R-03: Wake-word engines

Date: 2026-11-07 · Feeds: ADR-0002, spec 0002 (budget), SPIKE-002, REQ-VOI-001

## Findings

| | openWakeWord | microWakeWord |
|---|---|---|
| Code license | Apache-2.0 | Apache-2.0 (OHF-Voice/micro-wake-word) |
| **Model license** | **Pretrained models: CC BY-NC-SA 4.0** (non-commercial, share-alike; training data of unclear licence) | **Apache-2.0 including weights** per the `esphome/micro-wake-word-models` repo |
| Runtime | onnxruntime + tflite-runtime; Python API | TensorFlow Lite **for Microcontrollers** |
| Footprint | Not stated. One Pi 3 core runs 15-20 models in real time. Authors say it is too heavy for microcontrollers. | Designed for low-power MCUs (ESP32-class). Model size not stated. |
| Bindings | Python; community C++ port (rhasspy/openWakeWord-cpp), basic | Not stated. No Rust or Linux/ARM guidance found. |
| Pretrained words | alexa, hey mycroft, hey jarvis, hey rhasspy, others (English only) | `okay_nabu` named; list not visible in the pages read |
| Custom training | Colab notebooks, synthetic speech, sub-hour for basic results | Piper-generated samples, SpecAugment, streaming conversion. **README warns the basic notebook's output will likely be unusable without experimentation.** |
| Quality claims | Target <5% false-reject, <0.5 false-accept/hr (author-measured, small samples) | None published in pages read |

Sources: [openWakeWord](https://github.com/dscripka/openWakeWord), [micro-wake-word](https://github.com/OHF-Voice/micro-wake-word), [micro-wake-word-models](https://github.com/esphome/micro-wake-word-models), [microWakeWord](https://github.com/kahrendt/microWakeWord).

## Implications

1. **Licence trap:** openWakeWord's pretrained models are non-commercial and share-alike. They cannot be bundled in a permissively licensed project (ADR-0007) without conflict. We could train our own, but that is real work.
2. **microWakeWord is the better fit** for the budget (MCU-class runtime) and licensing (Apache-2.0 weights). Its risks: no known Rust or Linux/ARM integration (TFLM is portable C++, so FFI should work, but unverified), unknown model memory, and custom-word quality needs effort.
3. **Pure-Python openWakeWord can't run in-process** in a Rust, 48 MB design. It could only be an optional external sidecar for users who accept its licensing.
4. Neither engine's accuracy has independent evidence. Our own evaluation set (testing strategy §3) is required.

## Spec impact (recommended)

- ADR-0002 remains *Proposed* until SPIKE-002 shows TFLM runs on armv7 in budget via FFI.
- Add REQ-VOI-009: the core MUST NOT bundle wake-word models with licence terms incompatible with the project licence; shipped models need recorded provenance.
- `WakeEngine` is a trait with microWakeWord (TFLM) as the default candidate and a sidecar adapter option.
- SPIKE-002 questions: TFLM arena/RAM and CPU on a Cortex-A53/armv7; build in static musl; feed from 16 kHz frames; evaluate false accepts on real household audio.
- Plan a **wake-word model workstream** (training, eval set, provenance) as a separate epic. It is likely the largest quality risk.

## Confidence
Licences: high (primary). Footprint and Linux/Rust viability: low, which is why it's a spike.
