# ADR-0002: Implementation language: Rust

- **Status:** Proposed (accept after SPIKE-002 confirms the wake-word runtime fits)
- **Date:** 2026-11-07
- **Deciders:** Maintainer

## Context

The system must run in ≤ 64 MB RSS (48 MB target) and ≤ 8 MB binary on a 32-bit ARM Cortex-A53 device, with real-time audio paths, TLS, and an HTTP server. It will be open source and must be safe when handling untrusted network input.

## Decision drivers

- Low and predictable memory (no GC pauses or large runtime floor)
- Static, small, cross-compiled binaries (musl)
- Memory safety for network-facing parsers
- C interop for codecs and wake-word runtimes
- Contributor accessibility

## Options considered

1. **Rust:** smallest predictable footprint, memory safe, strong cross-compilation, good async and HTTP ecosystem. Steeper learning curve; the wake-word ecosystem is mostly C/Python.
2. **Go:** simple, fast to develop, easy cross-compile and embedding. GC and runtime raise the floor (tens of MB is plausible); less predictable under a 64 MB cap.
3. **C/C++:** minimal footprint and direct access to audio and ML runtimes. Memory-unsafe on a network-facing device; slower to develop to a high standard.
4. **Python:** best ML/audio ecosystem, but cannot meet the budget.

## Decision

Use **Rust** for core, protocol, and reference satellites, with C dependencies (e.g. libopus, TFLite Micro) linked statically where justified.

## Consequences

**Positive:** meets budget with margin, safe parsers, one toolchain for all targets.
**Negative:** contributor learning curve; wake-word integration requires FFI or a pure-Rust engine.
**Follow-ups:** SPIKE-002 (wake-word runtime footprint); pin the toolchain and MSRV.
