# 0002: Resource Budget

| | |
|---|---|
| **Status** | Draft |
| **Version** | 0.4 |
| **Created** | 2026-11-07 |
| **Related** | ADR-0003, [0001](0001-product-requirements.md) REQ-NFR-001 |

## 1. Summary

Defines the hard resource limits for the complete system and how CI enforces them. The limits derive from the reference device (Echo Dot Gen 2-class). **These figures are targets to be validated by SPIKE-001**, which confirms the real hardware specs and measures a baseline.

**Normative baseline.** The Echo Dot Gen 2 hardware package (§2) is the baseline ClawDot is designed and verified against. The **default configuration** MUST run within this budget on that hardware class. This is a resource and platform-compatibility guarantee. It is separate from whether anyone has unlocked a real Dot to boot ClawDot (a firmware port, which is opportunistic; see ADR-0003 and R-04).

## 2. Reference device (from teardowns; see [R-04](../research/04-echo-dot-gen2.md))

| Item | Value | Basis |
|---|---|---|
| SoC | MediaTek MT8163V, quad-core ARM (Cortex-A53 class), ARM64 kernel with **32-bit (armv7) userland** | Teardowns, firmware analysis |
| RAM | 4 Gbit LPDDR3 = **512 MB**, shared with OS and vendor stack | Teardowns |
| Flash | 4 GB eMMC, shared | Teardowns |
| Network | 802.11 Wi-Fi | Teardowns |
| Audio | 7 microphones into four ADCs; AEC/beamforming not provided by dedicated hardware | EEVblog teardown |
| Unlock status | **No known unlock or custom firmware.** Port is opportunistic only. | R-04 |

Real-device RSS and CPU baselines are still to be measured (SPIKE-001, hands-on phase, if hardware becomes available). Until then the CI qemu-user and cgroup-limited runs on an armv7 target are the benchmark.

Because the OS and drivers consume part of the 512 MB, the harness's share is budgeted conservatively.

## 3. Definitions

| Term | Meaning |
|---|---|
| **RSS** | Resident set size of the ClawDot process (including satellite module in the all-in-one build). |
| **Idle** | Wake-word listening, no session, no playback. |
| **Peak** | Maximum during the defined benchmark scenarios. |

## 4. Requirements

| ID | Requirement | Pri | Ver |
|---|---|---|---|
| REQ-NFR-101 | **Idle RSS** MUST be ≤ 24 MB. | Must | T |
| REQ-NFR-102 | **Peak RSS** across all benchmark scenarios MUST be ≤ **64 MB (hard gate)**. The **design target is ≤ 48 MB**: exceeding 48 MB raises a CI warning and the PR MUST justify it. | Must | T |
| REQ-NFR-103 | The release binary (stripped, static, with embedded UI) MUST be ≤ 8 MB. | Must | T |
| REQ-NFR-104 | **Idle CPU** MUST be ≤ 15% of one core including wake-word inference. | Must | T |
| REQ-NFR-105 | **Active CPU** (streaming + playback) SHOULD be ≤ 50% of one core. | Should | T |
| REQ-NFR-106 | Persistent storage footprint (excluding user music cache) MUST be ≤ 16 MB, with log and inbox bounded. | Must | T |
| REQ-NFR-107 | Cold start to wake-word-ready MUST be ≤ 5 s. | Must | T |
| REQ-NFR-108 | Steady-state heap growth over 24 h of soak MUST be < 1 MB. | Must | T |
| REQ-NFR-109 | Idle network use MUST be ≤ 1 KB/s average, excluding configured inbox polling. | Should | T |
| REQ-NFR-110 | The process MUST use bounded queues and buffers everywhere. Unbounded growth from input is a defect. | Must | I/T |
| REQ-NFR-114 | **Ratchet.** After the M1 walking skeleton is measured, the hard gates (RSS, binary size, storage) MUST be re-set to ≈ 1.5× the measured values, never above the limits in this spec. Gates may only be tightened afterwards; loosening one requires an ADR. | Must | I |
| REQ-NFR-113 | The **default configuration** (all-in-one, cloud providers, half-duplex audio, batch STT, shipped features enabled by default) MUST meet every limit in this spec. Any optional feature that would exceed a limit (e.g. software AEC, streaming STT, extra decoders) MUST be off by default and its measured cost documented. | Must | T |
| REQ-NFR-111 | The local fast-path intent matcher (REQ-AST-010) MUST resolve or reject an utterance in ≤ 20 ms on the reference CPU and use ≤ 2 MB of heap including its grammar. | Must | T |
| REQ-NFR-112 | Audio decoders (Opus/MP3, if used) MUST be budgeted explicitly; prefer requesting PCM from TTS providers to avoid decode cost. | Should | I |

## 5. Enforcement

**Budget gate (CI job `budget`):**
1. Build release for `armv7-unknown-linux-musleabihf`.
2. Assert binary size ≤ limit.
3. Run under `qemu-arm` (user mode) with `/usr/bin/time -v` or cgroup limits through the scripted benchmark scenarios (idle 5 min; 20-turn conversation with fake providers; 100 inbox events; timer storm of 50 timers).
4. Record peak RSS; warn above the 48 MB target and fail the build above the 64 MB hard gate. CPU is checked with relative thresholds on the CI runner (qemu timings are not absolute) and absolute thresholds on real hardware when available.
5. Publish results as a PR comment and track trend history. **Regressions over 5% require explicit justification.**

**Real-hardware verification** is run manually per release on the reference device (or the closest available device) and recorded in the release notes.

**Dependency gate:** every new crate's size delta is measured (`cargo bloat`); additions over 100 KB require a PR note.

## 6. Design implications (informative)

- Single-threaded async runtime by default; threads only where needed (audio).
- No dynamic linking, and no reflection-heavy or large-runtime dependencies.
- Audio buffers are pre-allocated ring buffers; response text is streamed, not accumulated.
- Conversation history is capped by tokens and bytes.
- JSON file store with atomic writes, not an embedded SQL engine, unless measured otherwise.

## 7. Open questions

| # | Question | Owner |
|---|---|---|
| 1 | Verify Gen 2 specs and OS overhead (SPIKE-001); the numbers above are provisional. | Maintainer |
| 2 | Wake-word runtime memory footprint (SPIKE-002). | Maintainer |
| 3 | TLS (rustls) memory under concurrent streams (SPIKE-003). | Maintainer |

## 8. Changelog

| Version | Date | Change |
|---|---|---|
| 0.1 | 2026-11-07 | Initial draft |
| 0.2 | 2026-11-07 | Reference device table sourced from teardowns (R-04); added fast-path matcher and decoder budgets (NFR-111/112) |
| 0.3 | 2026-11-07 | Stated the Echo Dot Gen 2 hardware package as the normative baseline for the default configuration; added NFR-113 (default config meets the budget; heavier options are opt-in) |
| 0.4 | 2026-11-07 | Peak RSS: 48 MB design target (warn) and 64 MB hard gate (fail); added the ratchet rule (NFR-114). The 48 MB figure was a judgment call, not derived; M0 spikes and the M1 skeleton replace it with measured values |
