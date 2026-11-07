# ADR-0003: Echo Dot Gen 2 as the reference resource budget

- **Status:** Accepted
- **Date:** 2026-11-07
- **Deciders:** Maintainer

## Context

We want the harness to run on very modest hardware and to avoid the usual drift where a "lightweight" project slowly bloats. The Echo Dot Gen 2 (≈ quad Cortex-A53, 512 MB RAM, small flash; specs to be verified by SPIKE-001) is a real, widely available, low-end smart speaker whose vendor software is itself a thin client that performs wake word on-device and delegates heavy processing to the cloud.

## Decision

- There is **one** profile. No "lite" and "full" variants of the core.
- The reference budget (see [spec 0002](../specs/0002-resource-budget.md)) is derived from Gen 2-class hardware and is enforced in CI.
- Heavy workloads (STT, TTS, LLM) are remote providers; users who want local models run them on another machine and point a provider endpoint at it.
- Whether the Dot can actually be unlocked is **not** a project dependency. The budget is the benchmark either way.

## Consequences

**Positive:** forced discipline; runs almost anywhere (Pi Zero, old routers, SBCs); a clear, testable promise.
**Negative:** rules out on-device inference in the core; dependency choices are constrained; some conveniences (heavy frameworks, SQLite, rich UI libraries) are off the table.
**Follow-ups:** SPIKE-001 to verify the specs; revisit numbers after a measured baseline.
