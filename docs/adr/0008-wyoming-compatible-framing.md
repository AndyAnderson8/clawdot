# ADR-0008: Wyoming-compatible framing for the satellite protocol

- **Status:** Proposed (decide in M0, after SPIKE-005)
- **Date:** 2026-11-07
- **Deciders:** Maintainer
- **Refines:** [ADR-0005](0005-satellite-core-separation.md) (the satellite/core split stands; this changes the wire framing)

## Context

ADR-0005 chose a custom protocol and spec 0003 drafted it over WebSocket. Research [R-05](../research/05-wyoming-and-local-intents.md) shows the Wyoming protocol (MIT) uses a simpler framing: a single-line JSON header ending in `\n`, an optional JSON data block, and a length-prefixed binary payload over plain TCP. It already has audio, wake, ASR, TTS, satellite, and `describe`/`info` events. It has **no authentication or encryption** and no notion of inbox announcements or indicator states.

## Decision drivers

- Minimal implementation burden on tiny clients (ESP32, a hypothetical Dot port)
- Ecosystem reach (existing satellites; a bridge)
- Our own needs: auth (REQ-SEC-006), inbox/indicator semantics, versioning
- Avoiding dependence on an external project's roadmap

## Options considered

1. **WebSocket + JSON + binary frames (current draft).** Familiar, browser-friendly. Needs an HTTP upgrade, frame masking, and ping/pong handling, which is more code and RAM on tiny clients.
2. **Wyoming-compatible framing with a namespaced extension layer.** Same framing as Wyoming; core Wyoming events used as-is where they fit; our additions under a `clawdot.` event namespace; a `hello` carrying a per-satellite token; optional TLS. Gives a cheap path to a Wyoming bridge or direct compatibility.
3. **Pure Wyoming, no extensions.** Maximum compatibility, but cannot express inbox, auth, or indicator semantics.
4. **Custom binary protocol.** Smallest, but least inspectable and no ecosystem.

## Proposed decision

Option 2. Specify our protocol as "Wyoming-compatible framing plus the `clawdot.*` extension events". Authentication and TLS are mandatory for non-loopback transports and are our responsibility, since Wyoming's own stance is trusted-network only.

## Consequences

**Positive:** simpler clients, an easy bridge, existing tooling can interoperate on the core subset.
**Negative:** we must handle TLS over raw TCP ourselves; we track an external project's conventions; WebSocket's browser reach is lost (not needed).
**Follow-ups:** SPIKE-005 (prototype the framing and a TLS-wrapped `hello`; measure client code size); update spec 0003 and REQ-SAT-001/011 on acceptance.
