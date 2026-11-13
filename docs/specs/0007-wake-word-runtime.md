# 0007: Wake-word runtime

| | |
|---|---|
| **Status** | Draft |
| **Version** | 0.3 |
| **Author(s)** | Maintainer direction, drafted by an agent |
| **Created** | 2026-11-09 |
| **Related** | [R-11](../research/11-wake-word-spike.md) sections 3.2, 8, 9, [0006](0006-device-layer-contract.md) (REQ-DEV-017, REQ-DEV-021, section 7), [0001](0001-product-requirements.md) (REQ-VOI-001, REQ-VOI-009), [0002](0002-resource-budget.md) |

## 1. Summary

The wake-word engine behind the `WakeEngine` seam of spec 0006 is a small purpose-built Rust runtime for the streaming int8 microWakeWord models: a reader for the `.tflite` file, a port of the TensorFlow Lite Micro audio frontend, and an interpreter for the few operators these models use. It has no dependencies and no `unsafe`. This spec fixes what it accepts, what it must never do with a hostile or damaged model, how closely it must agree with the reference implementation, and how it plugs into the device daemon.

## 2. Scope

**In scope:** the `clawdot-wake` crate (model reader, frontend, interpreter, detection rule), its adapter to the `WakeEngine` trait in `clawdot-device` behind a cargo feature, and the validation against the reference.

**Out of scope:** training a model or a custom word; always-on microphone capture in the daemon (REQ-DEV-025..030 of spec 0006); echo cancellation; any model format other than the TFLite flat buffer; measurements on a real device (B3).

## 3. Definitions

| Term | Meaning |
|---|---|
| Reference | The TensorFlow Lite C library driven by `pymicro-wakeword` with `pymicro-features` (the TFLite Micro frontend), Apache-2.0, as used in R-11 |
| Frontend | Audio to 40 log-mel features every 10 ms: 30 ms window, 512-point fixed-point FFT, noise reduction, per-channel gain control, log scale |
| Window of probabilities | The last N model outputs (N = `sliding_window_size`, 5 for the official models) averaged for the decision |
| Manifest | The small JSON next to a model (`probability_cutoff`, `sliding_window_size`, `feature_step_size`) |

## 4. Requirements

| ID | Requirement | Priority | Milestone | Verify |
|---|---|---|---|---|
| REQ-WAK-001 | The runtime MUST load a TFLite flat-buffer model and execute it with int8 kernels that follow the TFLite reference arithmetic (per-channel multipliers, rounding doubling high multiply, rounding shift). It MUST support exactly these operators: `CONV_2D`, `DEPTHWISE_CONV_2D`, `FULLY_CONNECTED`, `CONCATENATION`, `STRIDED_SLICE`, `SPLIT_V`, `RESHAPE`, `LOGISTIC`, `QUANTIZE`, and the stateful `CALL_ONCE`, `VAR_HANDLE`, `READ_VARIABLE`, `ASSIGN_VARIABLE`. A model that uses any other operator, a type or fusion it does not implement, or a quantisation it cannot honour exactly MUST be refused at load with an error that names the operator or tensor. | Must | A3 | T |
| REQ-WAK-002 | A model file is untrusted input. Reading, planning and running it MUST NOT panic, loop without bound, or allocate more than the limits of section 5 for any byte sequence; every offset, length, index and shape MUST be bounds checked, and arithmetic that can overflow MUST be checked. | Must | A3 | T |
| REQ-WAK-003 | All memory MUST be sized from the model at load: one activation arena planned from tensor lifetimes, plus the variable (streaming state) buffers. Processing audio MUST NOT allocate. The runtime MUST report its working memory in bytes. | Must | A3 | T |
| REQ-WAK-004 | The frontend MUST reproduce the TFLite Micro audio frontend with the microWakeWord parameters (16 kHz, 30 ms window, 10 ms step, 40 channels from 125 Hz to 7500 Hz, noise reduction with 10 smoothing bits, even and odd smoothing 0.025 and 0.06, minimum signal remaining 0.05, per-channel gain control with strength 0.95, offset 80 and 21 gain bits, log scale with shift 6) in integer arithmetic for the work done on every frame (the lookup tables are built once at start-up with floating point, as the reference builds them), so its output agrees with the reference within the tolerance of REQ-WAK-011. | Must | A3 | T |
| REQ-WAK-005 | Streaming state MUST live in the model's own variable tensors and start as the model's initial values. `reset` MUST restore the initial values, clear the frontend and the window of probabilities, and be equivalent to a fresh engine. The output for a given sample stream MUST be identical however the stream is cut into calls to `process`. | Must | A3 | T |
| REQ-WAK-006 | A detection MUST follow the reference rule: the model is run once per `stride` features (the model's input length, 3 for the official models); a decision exists only after N outputs; the decision is the mean of the last N dequantised outputs strictly greater than the word's cutoff. The detection score is that mean and `end_sample` is the stream offset (samples since the start or the last `reset`) of the end of the audio that produced the last feature. | Must | A3 | T |
| REQ-WAK-007 | Sensitivity MUST map one normalised value per word onto the cutoff: 0.5 is the manifest cutoff, lower is stricter, higher is looser, monotonic, and the result stays within 0.5 to 0.999, and a word loaded with a cutoff outside that range is refused. One frontend MUST serve all words of an engine. | Must | A3 | T |
| REQ-WAK-008 | The `WakeEngine` implementation MUST live behind a cargo feature of `clawdot-device` that is off by default, so the default build links none of it. It MUST use the refractory filter of spec 0006 section 7 so one utterance yields one detection, and MUST carry the SHA-256 of the model file and a licence string, supplied by the caller and required to be non-empty, in `WordInfo`. | Must | A3 | T |
| REQ-WAK-009 | The runtime MUST have no dependencies other than the Rust standard library, `unsafe_code` forbidden, and no C code (REQ-DEV-021). | Must | A3 | I |
| REQ-WAK-010 | Models shipped in the repository MUST be the official Apache-2.0 microWakeWord models with a recorded provenance (URL, commit, SHA-256, licence text). No other model, and no audio recording of a person, MAY be committed; test audio MUST be generated by the project's own code. | Must | A3 | I |
| REQ-WAK-011 | Agreement with the reference on the committed golden data MUST be: frontend features identical in at least 99 % of values and within one step of the int8 model input in at least 99.5 % (99.9 % on independent frames); the interpreter, run with the rounding of the library under test (`Rounding::GemmSingle`) on the reference's own features, bit-identical in every output; with the default TFLite reference rounding, outputs within 16 of 256; the end-to-end sliding-window mean within 0.04 on every frame. The achieved figures are recorded in [wake-word-runtime](../architecture/wake-word-runtime.md). | Must | A3 | T |
| REQ-WAK-012 | The runtime SHOULD process audio at no more than 2 % of one core of a 1.2 GHz Cortex-A53 per word. Measured on the PC and extrapolated until B3; the assumptions MUST be written next to the figure. | Should | A3 | A |

## 5. Limits

| Item | Limit |
|---|---|
| Model file | 1 MiB |
| Tensors, operators, buffers | 1,024 each |
| Tensor rank | 4 |
| Elements in one tensor | 1,048,576 |
| Activation arena plus variables | 1 MiB |
| Subgraphs | 4 |
| Multiply-accumulates per inference | 16,777,216 |
| Model input | one int8 tensor `[1, stride, 40]`, stride 1 to 16 |
| Model output | one uint8 or int8 tensor of one element |

## 6. Changelog

| Version | Date | Change |
|---|---|---|
| 0.1 | 2026-11-09 | First draft: REQ-WAK-001..012 from R-11 sections 3.2, 8 and 9. |
| 0.2 | 2026-11-09 | REQ-WAK-011 restated with the measured agreement (bit-exact interpreter under the library rounding, window mean within 0.04 end to end, not the 0.02 guessed in R-11); compute limit added to section 5. |
| 0.3 | 2026-11-13 | REQ-WAK-004: the frontend tables are built with floating point at start-up and only the per-frame work is integer. REQ-WAK-007: a word loaded with a cutoff outside 0.5 to 0.999 is refused. |
