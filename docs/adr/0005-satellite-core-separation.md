# ADR-0005: Separate satellite and core via a protocol

- **Status:** Accepted
- **Date:** 2026-11-07
- **Deciders:** Maintainer

## Context

The harness must be hardware-agnostic: a Pi, a PC, an ESP32, or a (hypothetically unlocked) Echo Dot should all be possible, and a port should not need to understand LLMs or tools.

## Options considered

1. **Monolith per device:** each port reimplements logic. Duplicated effort and divergence.
2. **Adopt the Wyoming protocol:** an existing satellite protocol from the Home Assistant ecosystem, with implementations on Pi/ESP32. It has no notion of an agent inbox or our indicator semantics, and its evolution is outside our control.
3. **Custom minimal protocol (WebSocket + JSON + binary audio):** full control, tailored to needs, but one more thing to design and maintain.

## Decision

Define a **custom, minimal, versioned satellite protocol** ([spec 0003](../specs/0003-satellite-protocol.md)) with an in-process transport for the all-in-one build. Revisit Wyoming interoperability (e.g. a bridge) as a later option.

## Consequences

**Positive:** ports are small; the core is device-agnostic; the protocol is testable via a conformance suite; supports remote and in-process modes uniformly.
**Negative:** we own protocol design, versioning, and documentation; no ecosystem of existing satellites.
**Follow-ups:** conformance suite; JSON Schemas; revisit a Wyoming bridge.
