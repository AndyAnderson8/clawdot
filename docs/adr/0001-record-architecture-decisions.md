# ADR-0001: Record architecture decisions

- **Status:** Accepted
- **Date:** 2026-11-07
- **Deciders:** Maintainer

## Context

The project aims to be open source and long-lived, with outside contributors and hardware porters. Rationale that lives only in chat or memory is lost, and newcomers re-litigate settled questions.

## Decision

We record significant architectural decisions as ADRs in `docs/adr/`, using the MADR-style template, immutable once Accepted and superseded by new ADRs when changed.

## Consequences

**Positive:** durable rationale, faster onboarding, fewer repeated debates.
**Negative:** small authoring overhead per decision.
