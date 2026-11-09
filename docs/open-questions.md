# Open questions

Decisions an agent was unsure about and did not want to block on. Each entry says what was assumed, so the maintainer can confirm or overrule. Newest entries at the bottom of each section; remove an entry when it is decided and recorded in a spec, ADR or the roadmap.

## Dot front end and Phase B (from R-08, 2026-11-09)

1. **Which Dot base do we support?** [R-08](research/08-existing-echo-dot-projects.md) recommends TECHO5 Dot as the preferred front end, EchoMuse as a supported alternative, and rooted Fire OS as a developer-only path. Assumed: this is a proposal only; the roadmap and ADR-0012 are unchanged until the maintainer agrees.
2. **May a trial install go on the existing Dot?** B0 needs a TECHO5 install (a write to the recovery partition, after full partition backups). Assumed: no device write happens without the maintainer's explicit go, as the roadmap already requires for Phase B.
3. **Does `echod` serve a client that is not Home Assistant?** Unverified from source alone (single voice subscriber, behaviour with no HA, setup page opens only from HA). Assumed: unknown until B0; the fallback plans are in R-08 section 9.2.
4. **Native ESPHome client or a library?** R-08 assumes a hand-written client on `ring` with a minimal protobuf codec (no `prost`, no `snow`) to protect the dependency budget. Assumed: revisit with measured binary size when the code exists.
5. **Does the maintainer know how the existing Dot was unlocked?** R-07 unknown 10 was never answered. R-08 finds the likely candidates (`amonet` v2 boot-root or EchoLocal's `boot.img`) but cannot tell. Assumed: record the answer in the install guide when known.
