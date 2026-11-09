# ADR-0014: Language: Rust core; Go, Python or others allowed for sidecars, bridges and upstream patches

- **Status:** Proposed
- **Date:** 2026-11-09
- **Deciders:** Maintainer

## Context

[ADR-0002](0002-implementation-language-rust.md) chose Rust for the core, protocol, and reference satellites. It says nothing about the parts of the project that are not the core: bridges to other systems, optional sidecars ([ADR-0010](0010-extension-model.md)), and patches to upstream projects such as the board images we build on. Contributors will reasonably ask whether a Go or Python component is welcome.

Evidence from existing Echo Dot projects ([R-08](../research/08-existing-echo-dot-projects.md)): TECHO5 and EchoMuse are written in Go, Python and C, and they expose the speaker to a brain over the ESPHome native API. We integrate with them over that API, not by sharing code, so their language does not constrain ours.

The hard constraint is the resource budget ([ADR-0003](0003-reference-hardware-budget.md), spec 0002): the core must run in at most 64 MB RSS (48 MB target) and an 8 MB binary on a 32-bit ARM Cortex-A53.

## Decision drivers

- A single static binary for the core, with a predictable memory floor
- The resource budget applies to what runs on the Dot, not to everything the project ships
- Every boundary in the system is already a wire protocol (satellite protocol, inbox, MCP, HTTP tools, ESPHome API), so components do not need to share a language
- Use the right tool for upstream work: a patch to a Go or Python project should be written in that project's language so it can be accepted

## Options considered

1. **Rust everywhere.** One toolchain, but forces awkward rewrites of bridges and makes upstream patches impossible.
2. **Rust core; any language at the edges (chosen).** Keeps the budget-critical code in one language and does not tax the edges.
3. **Polyglot core.** A Go or Python core cannot meet the budget (see ADR-0002).

## Decision

1. The **core** (the on-device binary, the protocol crates, and the reference satellites) is **Rust**, as in ADR-0002. It ships as one static binary.
2. **Sidecars, bridges, and upstream patches may use Go, Python, or another language** when that is the better fit. They talk to the core only across an existing protocol boundary and never link into the core binary. Anything that runs on the Dot itself must still fit the budget as measured, so a Python sidecar on the Dot is a justified exception, not a default; sidecars on other hosts have no such limit.
3. Patches to third-party projects follow that project's language and conventions. Our licensing and attribution rules ([ADR-0007](0007-license.md)) still apply to anything we vendor.
4. **The wake-word runtime is the weak spot for Rust.** The mature engines are C, C++ or Python. Prefer, in order: a pure-Rust runtime, then the engine already present in the base image, and only then a statically linked C runtime behind a small FFI shim. SPIKE-002 settles the choice and also gates accepting ADR-0002.

## Consequences

**Positive:** the core stays small and safe; bridges can reuse mature libraries; upstream contributions are possible; no change to the build of the default binary.
**Negative:** more than one toolchain in the wider project, with separate CI and security review for each non-Rust component; contributors must read the protocol docs rather than shared types.
**Follow-ups:** document the sidecar contract under the extension model; record the SPIKE-002 outcome and move both language ADRs to Accepted.

## Revisit when

SPIKE-002 shows no acceptable wake-word runtime, or a non-Rust component needs to run on the Dot by default.
