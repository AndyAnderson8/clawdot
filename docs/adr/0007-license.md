# ADR-0007: Project license: Apache-2.0

- **Status:** Accepted
- **Date:** 2026-11-07
- **Deciders:** Maintainer

## Context

The maintainer's intent: anyone may use the project however they want, with public attribution. The project targets embedded and hardware ports, including potentially commercial or closed products, so adoption friction matters.

## Decision drivers

- Unrestricted use, modification, and redistribution (commercial and closed derivatives allowed)
- A real attribution requirement, not just a request
- Explicit patent grant (protects porters and users in a codec/ML-adjacent field)
- Compatibility with the Rust ecosystem and our dependencies (TFLite Micro, microWakeWord weights, rustls, libopus are Apache-2.0 / MIT / BSD)

## Options considered

1. **Apache-2.0:** permissive; requires retaining the licence, the `NOTICE` file, and copyright/attribution notices, and stating changes; explicit patent grant and patent-retaliation clause.
2. **MIT, or MIT OR Apache-2.0 (Rust convention):** permissive. MIT's attribution is just keeping the copyright and licence notice, which is weaker and has no `NOTICE` mechanism; dual licensing lets downstream choose the weaker terms.
3. **BSD-4-clause (advertising clause):** would legally require *visible* credit in advertising, but it is widely avoided, incompatible with the GPL, and hostile to the ecosystem.
4. **Copyleft (GPL/AGPL/MPL):** keeps derivatives open but conflicts with "use however you want" and deters embedded ports.

## Decision

Licence the project under **Apache-2.0**.

- **Attribution:** Apache-2.0 §4 obliges redistributors to keep the licence, the `NOTICE` attributions, and notices of modification. We ship a `NOTICE` file for exactly this purpose.
- **Public credit:** no standard open-source licence can force a visible credit in a product's user interface without a nonstandard advertising clause (rejected above). The README therefore **asks** for visible credit as a courtesy; that request is not a licence term.
- **Contributions:** inbound = outbound under Apache-2.0 §5 (contributions are licensed under the project licence unless stated otherwise). No CLA. DCO sign-off remains dormant until the first external contributor ([SDLC §13](../process/sdlc.md#13-proportionality-solo-maintainer-mode)).
- **Dependencies:** enforced by `cargo-deny`. Allow Apache-2.0, MIT, BSD-2/3-Clause, ISC, Zlib, Unicode. Review MPL-2.0 case by case. Deny GPL, AGPL, LGPL (our static single binary makes them problematic), and any non-commercial or share-alike terms (e.g. CC BY-NC-SA; see REQ-VOI-009 and [R-03](../research/03-wake-word.md)).

## Consequences

**Positive:** maximal adoption by porters; clear patent grant; real attribution via `NOTICE`; no CLA friction.
**Negative:** third parties may build closed derivatives (accepted); Apache-2.0 is incompatible with GPLv2-only projects (compatible with GPLv3).
**Follow-ups:** `license = "Apache-2.0"` in every `Cargo.toml`; `deny.toml` allowlist in TASK-002; optional SPDX headers (`SPDX-License-Identifier: Apache-2.0`). The copyright holder in `NOTICE` is Andy Anderson; contributors keep copyright in their own contributions under Apache-2.0 §5.
