# Specifications

Specs define **what the system does** and the contracts it must honour. Code implements specs; tests verify them.

## Index

| ID | Title | Status |
|---|---|---|
| [0001](0001-product-requirements.md) | Product requirements | Draft |
| [0002](0002-resource-budget.md) | Resource budget | Draft |
| [0003](0003-satellite-protocol.md) | Satellite protocol | Draft (skeleton) |
| [0004](0004-esphome-satellite-client.md) | ESPHome voice-satellite client | Draft |
| [0006](0006-device-layer-contract.md) | Device layer contract (portable daemon, board files, capabilities) | Draft |
| [0007](0007-wake-word-runtime.md) | Wake-word runtime (model reader, audio frontend, int8 interpreter, detection rule) | Draft |

## Lifecycle

```
Draft → In Review → Accepted → Implemented
                       ↓
                  Superseded / Withdrawn
```

| Status | Meaning |
|---|---|
| **Draft** | Being written. May change freely. |
| **In Review** | PR open for review. |
| **Accepted** | Merged. Implementation may begin. Changes need a PR and, for contract changes, a version bump. |
| **Implemented** | Every requirement is covered by passing tests. |
| **Superseded** | Replaced by a newer spec (linked). |
| **Withdrawn** | Abandoned, with reason recorded. |

## Rules

1. **Numbering:** `NNNN-kebab-title.md`, sequential and never reused.
2. **Requirement IDs:** `REQ-<AREA>-<NNN>`, stable and never reused. If a requirement is removed, mark it `[REMOVED]` rather than deleting it.
3. **Keywords:** MUST, MUST NOT, SHOULD, SHOULD NOT, MAY carry [RFC 2119](https://www.rfc-editor.org/rfc/rfc2119) meaning.
4. **Testability:** every MUST and SHOULD requirement has a verification method (Test, Inspection, Analysis, Demonstration).
5. **Priority:** MoSCoW in the requirement table, and a target milestone.
6. **No implementation detail** unless it is itself the contract (e.g. wire format). Design belongs in architecture docs and ADRs.
7. **Traceability:** tests reference IDs, e.g. `#[test] fn req_tmr_001_timer_persists()` or a `// REQ-TMR-001` comment. CI reports requirements with no covering test.

## Area codes

| Code | Area |
|---|---|
| `VOI` | Voice pipeline (wake, STT, TTS, barge-in) |
| `AST` | Assistant behaviour (LLM, tools, MCP) |
| `INB` | Agent inbox and webhooks |
| `TMR` | Timers, alarms, reminders |
| `MUS` | Music |
| `CFG` | Configuration and web UI |
| `SAT` | Satellite protocol |
| `NFR` | Non-functional (performance, budget, reliability) |
| `SEC` | Security and privacy |
| `EXT` | Extensibility (packs, HTTP tools, sidecars) |
| `BRD` | Board support (buttons, LED ring, mute, Wi-Fi, clock, audio device names) |
| `ESP` | ESPHome native-API voice-satellite client |
| `DEV` | Device layer contract: the portable device daemon, board files, capability model, conformance |
| `WAK` | Wake-word runtime: model reader, audio frontend, interpreter, detection rule, validation against the reference |

## Change process

- **Adding a requirement:** PR to the spec, then issues reference the new ID.
- **Changing an Accepted requirement:** PR with rationale. If it alters behaviour that ports depend on, bump the spec version and note it in the changelog section.
- **Disagreement:** record in the PR; if it is a significant decision, capture it as an ADR.

New specs start from [`_template.md`](_template.md).
