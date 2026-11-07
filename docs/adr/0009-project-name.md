# ADR-0009: Project name: ClawDot

- **Status:** Accepted
- **Date:** 2026-11-07
- **Deciders:** Maintainer

## Context

Research note [R-06](../research/06-naming.md) found no exact "clawdot" repository, but noted that the prefix "Clawd" was the point of an earlier trademark objection involving a different project (Clawdbot, since renamed). It flagged potential trademark risk, user confusion, and search-noise from that ecosystem, and recommended choosing a different name before the first public release.

## Decision

The maintainer has decided to keep the name **ClawDot** (capital D, as a deliberate separation from the "Clawd" prefix). The risks above are acknowledged and accepted by the maintainer.

## Mitigations (adopted with the decision)

- The README states that ClawDot is an independent project, **not affiliated with or endorsed by Anthropic**.
- No use of Anthropic or Claude logos, mascots, or branding; no imagery or copy implying affiliation.
- Agent-integration docs refer to Claude Code only as a compatibility target, using its plain name.
- Before the first registry publication (crates.io, container registries, domains), repeat a collision check and keep those registrations under the maintainer's control.
- The name is revisited if a rights holder objects. A rename is a find-and-replace while there are few dependants; the cost grows after public release.

## Consequences

**Positive:** no renaming churn; the maintainer's preferred identity.
**Negative:** residual trademark and confusion risk persists; a forced rename after wide adoption would be costly.
