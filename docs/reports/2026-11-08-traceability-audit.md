# Traceability audit, 2026-11-08

Report-only audit of every requirement ID in [spec 0001](../specs/0001-product-requirements.md), [spec 0002](../specs/0002-resource-budget.md) and [spec 0003](../specs/0003-satellite-protocol.md) against the code, the tests, and the commit history at `dev` commit `2860d5a`. Nothing was changed except this file. The hand-maintained map in [quality/traceability.md](../quality/traceability.md) was read for cross-checking but not edited.

## Method

No traceability tool exists yet: TASK-006 ("Traceability report tool") is still an open backlog item in [roadmap.md](../roadmap.md), and no script in `scripts/` or `.github/workflows/` extracts requirement IDs. This audit therefore used a one-off throwaway script (kept outside the repository; not proposed as the TASK-006 tool). It did three things:

1. Read every `| REQ-... |` table row in the three specs (ID, priority, milestone). Spec 0001 has 101 rows, spec 0002 has 14, spec 0003 has 16: **131 requirements**. Spec 0001 was audited as it stands on `dev`; other branches that amend it were not considered, so IDs added there will show up as "cited but not in the specs" until those land.
2. Scanned every `crates/**/*.rs` file for `REQ-AREA-nnn` citations, expanding shorthand such as `REQ-INB-004..007, 012`, `REQ-INB-001/002` and `REQ-VOI-002, 003, 004`. A citation counts as a **test** citation when it is in a file under `tests/` or after `#[cfg(test)]` in a source file; the nearest enclosing or following `fn` is recorded as the test name. Otherwise it is a **code** citation. IDs reached only through the middle of a `..` range (for example the module overview `REQ-INB-001..020` in `inbox/mod.rs`) are treated as weak and do not count as implemented.
3. Scanned the subject and body of every commit reachable from `HEAD` (including merge history from `dev`) for the same pattern.

Status definitions, applied in this order:

| Status | Meaning |
|---|---|
| tested | A test (or a comment inside a test module) cites the ID. Test names are a heuristic: a `// ----- group (REQ-x) -----` banner is attributed to the first `fn` after it and marked `(banner)`, so the named function is a helper or the first test of a group, not a complete list. |
| implemented | Non-test code cites the ID directly (path given). No test cites it by ID; a test may still cover the behaviour without naming it. |
| untraced | Only commits reference it, or only the middle of a `..` range in a module header. Work may well exist, but nothing in the code points at the requirement. |
| spec-only | Cited nowhere in code, tests or commits. |

Caveat that matters for reading the numbers: **citation is evidence of intent, not proof of coverage.** A module header citing an ID shows where the author believes it lives; it does not show the requirement is fully met. Where the hand-maintained map says "Partial" or "Not started" it is noted below.

## Headline numbers

| Status | All | Must | Should |
|---|---|---|---|
| tested | 16 (12%) | 14 | 2 |
| implemented | 57 (44%) | 48 | 9 |
| untraced | 22 (17%) | 16 | 6 |
| spec-only | 36 (27%) | 21 | 15 |
| **Total** | **131** | **99** | **32** |

By spec:

| Spec | Total | tested | implemented | untraced | spec-only |
|---|---|---|---|---|---|
| 0001 product requirements | 101 | 14 | 48 | 13 | 26 |
| 0002 resource budget | 14 | 1 | 1 | 4 | 8 |
| 0003 satellite protocol | 16 | 1 | 8 | 5 | 2 |

- **Citations exist in code but almost never in tests.** Only 16 of 131 requirements are cited by a test. The project's convention ("cite requirement IDs in tests and commits", AGENTS.md) is followed for commits (86 distinct IDs appear in commit messages) and module headers, but not for test names. The 324-test suite largely covers these behaviours; it just does not say which requirement each test serves.
- **Every ID cited anywhere exists.** No code, test or commit cites a requirement that is missing from the specs (see the dedicated section below).
- Of 99 Must requirements, 37 (21 spec-only plus 16 untraced) have no direct code or test citation. Many of those are done in practice; see the next sections.

## Top gaps

1. **Test citations.** 85 of the 99 Must requirements are not cited by any test. Fix once TASK-006 exists by making the tool fail on Must requirements with no test citation, and by adding `Refs`-style `// REQ-...` comments to existing tests (a documentation-only change).
2. **Resource budget spec 0002 is barely traced.** Measured by `scripts/budget.sh` (binary 2.62 MiB, idle RSS 3.0 MiB, peak 4.3 MiB per the overnight report), but only REQ-NFR-110 and REQ-NFR-111 are cited in code. REQ-NFR-104 (idle CPU), 106, 107, 108, 113 are spec-only: CPU and ARM memory are unmeasured per the hand-maintained map, and the rest have no trace.
3. **Voice front end.** REQ-VOI-001 (on-device wake word) has no code or citation; the dialog state machine and VAD exist (`core/voice`, `core/vad`) but wake-word runtime waits on SPIKE-002/006. REQ-VOI-009 (model provenance) is commit-only.
4. **Security and privacy statements with no anchor.** REQ-SEC-003 (unambiguous microphone state), REQ-SEC-006 (authenticated satellite connections; the code cites REQ-SAT-010 but not SEC-006), REQ-SEC-007 (no telemetry; true by construction, documented in PRIVACY.md, never cited).
5. **Operational Musts with no implementation evidence:** REQ-NFR-001, 003, 005, 007 (budgets enforced in CI, outage recovery, structured logging, reproducible builds), REQ-NFR-004 (30-day soak, M6).
6. **Web UI breadth.** REQ-CFG-002 (configure providers, wake word, voice, volume, time zone, tool policies) and REQ-CFG-012 (headless Pi network documentation) are spec-only in M4; REQ-CFG-010 (manage MCP, HTTP tools, media backends from the UI) is M5.
7. **Persona and prompt configuration** REQ-AST-013 (M3) has no trace, including the rule that the configured prompt cannot disable the confirm policy.
8. **Documented sidecar API** REQ-EXT-005 (M5) and the non-preclusion rule REQ-EXT-006 (M2) are spec-only.
9. **Satellite protocol conformance:** REQ-SAT-012 (conformance harness and JSON Schemas) is commit-only; REQ-SAT-014 (Wyoming bridge) and REQ-SAT-016 (display events) are spec-only.
10. **The hand-maintained map is stale.** [quality/traceability.md](../quality/traceability.md) is dated 2026-11-07 and says all of AST-001..003, 005..009, 011..014 and the whole voice, config and music groups are "Not started", while the overnight session report and the code now contain the LLM client, MCP client, HTTP tools, voice dialog layer, STT/TTS clients, web UI and radio. Several of its other lines (INB-009, INB-019, TMR-007) agree with this audit; the others should be refreshed (or replaced by the TASK-006 tool output).

## Must requirements not yet implemented, by milestone

"Not yet implemented" here combines this audit's evidence with the hand-maintained map and the overnight report. Items marked *likely done, uncited* appear to exist but cannot be confirmed from citations; items marked *gap* have no sign of implementation.

| Milestone | Must requirements without implementation evidence |
|---|---|
| M0 | REQ-SEC-005 (cargo-deny / cargo-audit in CI, SBOM per release): *likely done, uncited*; `deny.toml` and `release.yml` mention audit and SBOM. |
| M1 | REQ-VOI-001 on-device wake word (*gap*, blocked on SPIKE-002/006). REQ-VOI-009 wake-word model provenance and licence (commit-only; *gap* until a model is shipped). REQ-NFR-001 budgets enforced in CI (*partly done*: `scripts/budget.sh`, ratchet not tightened). REQ-NFR-003 provider-outage recovery (*gap*). REQ-NFR-005 structured, bounded logs (*gap*). REQ-NFR-007 reproducible x86_64 and armv7 builds (*partly done*: armv7 cross-build measured). REQ-SEC-003 unambiguous mic state (*gap*; the dialog has mute state, no user-visible guarantee traced). REQ-SEC-006 satellite authentication (*likely done* via REQ-SAT-010 token check, uncited under this ID). REQ-SEC-007 no telemetry (*done by construction, uncited*). REQ-EXT-001 no loaded third-party code (*holds trivially, commit-only*). |
| M2 | REQ-INB-005 tone and announce (*partial*; tone is a satellite concern). REQ-INB-015 reference agent bridge (*done*: `crates/bridge`; cited only via range). REQ-EXT-006 inbox model must not preclude an MCP server (*no work needed, uncited*). |
| M3 | REQ-AST-013 persona and prompt configuration (*gap*). REQ-TMR-002 fire within 1 s (*partial*: loop ticks at 1 s, not measured). REQ-TMR-004 recurrence, dismiss, snooze (*partial*: no voice snooze for a ringing alarm). REQ-TMR-007 alarm sounds without providers (*gap*: needs a satellite local sound). |
| M4 | REQ-CFG-002 UI provider and wake-word configuration (*gap*). REQ-CFG-012 headless Pi network docs (*gap*). |
| M5 | REQ-CFG-010 UI for MCP, HTTP tools, media backends (*gap*). REQ-EXT-005 documented and versioned sidecar API (*gap*; the surface exists, the versioned document does not). |
| M6 | REQ-NFR-004 30-day unattended run (*gap*). |
| spec 0002 (no milestone column) | REQ-NFR-101, 102, 103 (idle RSS, peak RSS, binary size: *measured*, commit-only reference); REQ-NFR-104 idle CPU, 106, 107, 108, 113 (*gap*, unmeasured); REQ-NFR-114 budget ratchet (*partly done*: script exists, limits not tightened). |
| spec 0003 (no milestone column) | REQ-SAT-006/007/008 events, commands, keepalive (*done in `core/satellite` and `protocol`; cited only through a range*). REQ-SAT-012 conformance harness and JSON Schemas (*gap*). REQ-SAT-015 reference satellites for desktop and Raspberry Pi (*partial*: a reference satellite client exists, no Raspberry Pi build or conformance suite). |

Count of genuine *gaps* among the Musts (no sign of work): about 18 of 99.

## Requirement IDs cited but not in the specs

None. All 85 distinct IDs found in `crates/**/*.rs` and all 86 distinct IDs found in commit messages (reachable from `HEAD`) exist in specs 0001 to 0003. Two notes:

- Shorthand without the `REQ-` prefix, for example `EXT-004` in `inbox/mod.rs` ("REQ-INB-003, EXT-004") and the `AST-010`/`SAT-004` forms in `docs/quality/traceability.md`, cannot be matched mechanically. TASK-006 should either require the full form or accept the short form deliberately.
- IDs added by in-flight spec amendments on other branches are not visible to this audit and may appear as unknown once they merge.

## Per-requirement map

Evidence lists the module (crate-relative, without `src/`), test names, and up to three commits (`...` means more). `-` means no evidence.

### Spec 0001

| ID | Pri | MS | Status | Evidence |
|---|---|---|---|---|
| REQ-VOI-001 | Must | M1 | spec-only | - |
| REQ-VOI-002 | Must | M1 | implemented | code: `core/vad`, `core/voice`; commits: 366f86c |
| REQ-VOI-003 | Must | M1 | implemented | code: `core/speech`, `core/voice`; commits: d6f2fa7, d0f2abb, 47e7c30 |
| REQ-VOI-004 | Must | M1 | implemented | code: `core/llm`, `core/speech`, `core/voice`; commits: d6f2fa7, d0f2abb, 47e7c30 |
| REQ-VOI-005 | Should | M6 | spec-only | - |
| REQ-VOI-006 | Must | M1 | implemented | code: `core/voice`, `protocol/message`; commits: 47e7c30 |
| REQ-VOI-007 | Should | M1 | implemented | code: `core/voice`; commits: 47e7c30 |
| REQ-VOI-008 | Must | M1 | implemented | code: `core/voice`; commits: 0a80d45, 47e7c30 |
| REQ-VOI-009 | Must | M1 | untraced | commits: 7bf7e5d |
| REQ-VOI-010 | Should | M1 | implemented | code: `core/voice`; commits: 47e7c30 |
| REQ-VOI-011 | Should | M6 | spec-only | - |
| REQ-VOI-012 | Must | M1 | implemented | code: `core/voice`; commits: 47e7c30 |
| REQ-AST-001 | Must | M1 | implemented | code: `core/llm`, `core/transport`; commits: fe24618, 82c84c3 |
| REQ-AST-002 | Must | M1 | implemented | code: `core/llm`, `core/session` |
| REQ-AST-003 | Must | M1 | implemented | code: `core/llm`, `core/session` |
| REQ-AST-004 | Must | M3 | implemented | code: `core/assistant/tools`; commits: fe24618, 5efff31, 82c84c3... |
| REQ-AST-005 | Should | M5 | implemented | code: `core/mcp`; commits: a3e1c74, fcda499 |
| REQ-AST-006 | Must | M1 | implemented | code: `core/session` |
| REQ-AST-007 | Must | M1 | implemented | code: `core/session` |
| REQ-AST-008 | Must | M5 | implemented | code: `core/httptools`, `core/mcp`, `core/session`; commits: a3e1c74, fcda499, f4f8d43 |
| REQ-AST-009 | Should | M1 | implemented | code: `core/llm`, `core/session`; commits: 82c84c3 |
| REQ-AST-010 | Must | M3 | tested | code: `core/assistant`, `core/fastpath`; tests: `core/fastpath::timers_can_be_listed_in_many_phrasings`; commits: db8655f, 0b740a6, 44ab9fc |
| REQ-AST-011 | Must | M3 | implemented | code: `core/llm`, `core/session`, `core/usage` |
| REQ-AST-012 | Should | M3 | spec-only | - |
| REQ-AST-013 | Must | M3 | spec-only | - |
| REQ-AST-014 | Must | M5 | implemented | code: `core/assistant/tools`, `core/httptools`; commits: f4f8d43, cbd9c84 |
| REQ-INB-001 | Must | M2 | implemented | code: `core/http`, `core/inbox/endpoint`, `core/inbox/mod`; commits: daf0e63, 00b4e0d, 0409a9b |
| REQ-INB-002 | Must | M2 | implemented | code: `core/http`, `core/inbox/endpoint`, `core/inbox/mod`; commits: 00b4e0d, 0409a9b |
| REQ-INB-003 | Must | M2 | implemented | code: `core/inbox/mod`; commits: 0409a9b |
| REQ-INB-004 | Must | M2 | implemented | code: `core/inbox/mod`, `core/inbox/queue`; commits: 0409a9b |
| REQ-INB-005 | Must | M2 | untraced | range mention only: `core/inbox/mod`, `core/inbox/queue`; commits: 0409a9b |
| REQ-INB-006 | Must | M2 | implemented | code: `core/inbox/mod`, `core/inbox/queue`; commits: 0409a9b |
| REQ-INB-007 | Must | M2 | implemented | code: `core/inbox/queue`; commits: 321ff48 |
| REQ-INB-008 | Must | M2 | implemented | code: `core/app`, `core/http`, `core/inbox/mod`, `core/inbox/reply`, `protocol/bridge`; commits: a8bf3ec, 02940da, 00b4e0d... |
| REQ-INB-009 | Should | M2 | untraced | range mention only: `core/inbox/mod`; commits: 9a6cc2e |
| REQ-INB-010 | Must | M2 | implemented | code: `core/inbox/summary`; commits: 5efff31, 0409a9b |
| REQ-INB-011 | Should | M2 | untraced | range mention only: `core/inbox/mod` |
| REQ-INB-012 | Should | M2 | implemented | code: `core/inbox/endpoint`, `core/inbox/mod`, `core/inbox/queue`; commits: 0409a9b |
| REQ-INB-013 | Should | M2 | untraced | range mention only: `core/inbox/mod`; commits: 342299f |
| REQ-INB-014 | Must | M2 | implemented | code: `core/inbox/endpoint`, `core/inbox/mod`, `core/inbox/queue`; commits: 0409a9b |
| REQ-INB-015 | Must | M2 | untraced | range mention only: `core/inbox/mod`; commits: 9a6cc2e, d3bab08, 02940da |
| REQ-INB-016 | Should | M2 | implemented | code: `core/inbox/queue`; commits: 0b740a6, 0409a9b |
| REQ-INB-017 | Must | M2 | implemented | code: `core/app`; commits: 9a6cc2e, a8bf3ec, 0b740a6... |
| REQ-INB-018 | Must | M2 | implemented | code: `core/inbox/mod`, `core/inbox/summary`; commits: daf0e63, 88509f7, 0409a9b |
| REQ-INB-019 | Should | M4 | untraced | range mention only: `core/inbox/mod` |
| REQ-INB-020 | Must | M2 | implemented | code: `bridge/harness`, `core/inbox/mod`; commits: d3bab08 |
| REQ-TMR-001 | Must | M3 | implemented | code: `core/sched`; commits: c7e6e32 |
| REQ-TMR-002 | Must | M3 | untraced | range mention only: `core/sched`; commits: c7e6e32 |
| REQ-TMR-003 | Must | M3 | implemented | code: `core/sched`; commits: c7e6e32 |
| REQ-TMR-004 | Must | M3 | untraced | range mention only: `core/sched`; commits: c7e6e32 |
| REQ-TMR-005 | Must | M3 | implemented | code: `core/sched`; commits: c7e6e32 |
| REQ-TMR-006 | Must | M3 | implemented | code: `core/main`, `core/sched`; commits: 9ce871d, d3cd6a3, d3cd6a3... |
| REQ-TMR-007 | Must | M3 | untraced | range mention only: `core/sched`; commits: c7e6e32 |
| REQ-TMR-008 | Should | M3 | tested | code: `core/sched`; tests: `core/fastpath::timers_can_be_listed_in_many_phrasings`; commits: db8655f |
| REQ-TMR-009 | Must | M3 | tested | code: `core/assistant`; tests: `core/assistant::an_unset_clock_is_admitted_not_read_out_as_the_time`, `core/assistant/tools::time_dependent_tools_refuse_an_unset_clock`; commits: 9ce871d, d3cd6a3 |
| REQ-MUS-001 | Must | M5 | tested | code: `core/media`; tests: `core/assistant::radio (banner)`, `core/satellite::start_music (banner)`; commits: d1bb86a, ca4b0da |
| REQ-MUS-002 | Must | M5 | tested | code: `core/media`; tests: `core/assistant::radio (banner)`; commits: 8133fbf, d1bb86a, dc1b0e8... |
| REQ-MUS-003 | Must | M5 | implemented | code: `core/media`; commits: ca4b0da |
| REQ-MUS-005 | Should | M5 | spec-only | - |
| REQ-MUS-006 | Should | M5 | untraced | commits: ca4b0da |
| REQ-MUS-004 | Must | M5 | tested | code: `core/media`, `core/satellite`; tests: `core/satellite::start_music (banner)`; commits: d1bb86a, dc1b0e8, ca4b0da |
| REQ-CFG-001 | Must | M4 | tested | code: `core/ui`; tests: `core/http::admin (banner)/the_page_is_static_self_contained_and_locked_down`; commits: 3ee6284 |
| REQ-CFG-002 | Must | M4 | spec-only | - |
| REQ-CFG-003 | Must | M4 | tested | code: `core/ui`; tests: `core/http::admin (banner)`; commits: 3ee6284 |
| REQ-CFG-004 | Should | M4 | implemented | code: `core/ui`; commits: 40a2923 |
| REQ-CFG-005 | Must | M4 | tested | code: `core/ui`; tests: `core/http::admin (banner)`; commits: 3ee6284 |
| REQ-CFG-006 | Must | M4 | tested | code: `core/inbox/endpoint`, `core/ui`; tests: `core/http::admin (banner)`; commits: 3ee6284 |
| REQ-CFG-007 | Must | M1 | implemented | code: `core/config`; commits: a3e1c74, d6f2fa7, 980122d |
| REQ-CFG-008 | Should | M4 | implemented | code: `core/ui`; commits: 3ee6284 |
| REQ-CFG-009 | Should | M4 | spec-only | - |
| REQ-CFG-010 | Must | M5 | spec-only | - |
| REQ-CFG-011 | Should | v1.x | spec-only | - |
| REQ-CFG-012 | Must | M4 | spec-only | - |
| REQ-CFG-013 | Should | M4 | untraced | commits: c56c242 |
| REQ-CFG-014 | Must | M4 | tested | code: `core/main`; tests: `core/tests/binary::a_taken_port_fails_without_revealing_a_token`; commits: d6bd459, a45e535 |
| REQ-CFG-015 | Must | M4 | tested | code: `core/main`; tests: `core/tests/binary::a_taken_port_fails_without_revealing_a_token/create_endpoint/snippets_follow_the_listen_address`; commits: d6bd459, a45e535 |
| REQ-CFG-016 | Must | M4 | tested | code: `core/main`; tests: `core/tests/binary::snippets_follow_the_listen_address`; commits: d6bd459, a45e535 |
| REQ-CFG-017 | Should | M1 | tested | code: `core/main`; tests: `core/tests/binary::quit_leaves_the_console`; commits: d6bd459, a45e535 |
| REQ-NFR-001 | Must | M1 | spec-only | - |
| REQ-NFR-002 | Should | M6 | spec-only | - |
| REQ-NFR-003 | Must | M1 | spec-only | - |
| REQ-NFR-004 | Must | M6 | spec-only | - |
| REQ-NFR-005 | Must | M1 | spec-only | - |
| REQ-NFR-006 | Should | M4 | spec-only | - |
| REQ-NFR-007 | Must | M1 | spec-only | - |
| REQ-NFR-008 | Must | M3 | implemented | code: `core/store`; commits: c7e6e32 |
| REQ-NFR-009 | Must | M3 | implemented | code: `core/assistant`; commits: db8655f, 44ab9fc |
| REQ-NFR-010 | Must | M6 | implemented | code: `core/store`; commits: c7e6e32 |
| REQ-SEC-001 | Must | M1 | implemented | code: `core/httptools`, `core/speech`, `core/transport`; commits: d0f2abb |
| REQ-SEC-002 | Must | M1 | implemented | code: `core/speech`; commits: d0f2abb |
| REQ-SEC-003 | Must | M1 | spec-only | - |
| REQ-SEC-004 | Must | M2 | implemented | code: `core/app`, `core/http`, `protocol/frame`; commits: 00b4e0d, 321ff48, 929e9ae |
| REQ-SEC-005 | Must | M0 | spec-only | - |
| REQ-SEC-006 | Must | M1 | spec-only | - |
| REQ-SEC-007 | Must | M1 | spec-only | - |
| REQ-EXT-001 | Must | M1 | untraced | commits: fcda499, f4f8d43 |
| REQ-EXT-002 | Should | v1.x | spec-only | - |
| REQ-EXT-003 | Should | v1.x | spec-only | - |
| REQ-EXT-004 | Must | M2 | implemented | code: `core/inbox/adapter`; commits: 0409a9b |
| REQ-EXT-005 | Must | M5 | spec-only | - |
| REQ-EXT-006 | Must | M2 | spec-only | - |

### Spec 0002

| ID | Pri | MS | Status | Evidence |
|---|---|---|---|---|
| REQ-NFR-101 | Must | - | untraced | commits: 8d8d23d |
| REQ-NFR-102 | Must | - | untraced | commits: 8d8d23d |
| REQ-NFR-103 | Must | - | untraced | commits: 8d8d23d |
| REQ-NFR-104 | Must | - | spec-only | - |
| REQ-NFR-105 | Should | - | spec-only | - |
| REQ-NFR-106 | Must | - | spec-only | - |
| REQ-NFR-107 | Must | - | spec-only | - |
| REQ-NFR-108 | Must | - | spec-only | - |
| REQ-NFR-109 | Should | - | spec-only | - |
| REQ-NFR-110 | Must | - | implemented | code: `core/app`, `core/http`, `core/inbox/queue`, `protocol/frame`; commits: 00b4e0d |
| REQ-NFR-114 | Must | - | untraced | commits: 8d8d23d |
| REQ-NFR-113 | Must | - | spec-only | - |
| REQ-NFR-111 | Must | - | tested | code: `core/fastpath`; tests: `core/fastpath::matching_is_fast_enough_for_the_budget`; commits: 44ab9fc |
| REQ-NFR-112 | Should | - | spec-only | - |

### Spec 0003

| ID | Pri | MS | Status | Evidence |
|---|---|---|---|---|
| REQ-SAT-001 | Must | - | implemented | code: `core/satellite`, `protocol/frame`; commits: 929e9ae |
| REQ-SAT-002 | Must | - | implemented | code: `core/satellite`; commits: 25bd2c6, 0a80d45, f89c0a9 |
| REQ-SAT-003 | Must | - | implemented | code: `protocol/message`; commits: f89c0a9 |
| REQ-SAT-004 | Must | - | implemented | code: `protocol/message` |
| REQ-SAT-005 | Must | - | implemented | code: `protocol/message` |
| REQ-SAT-006 | Must | - | untraced | range mention only: `core/satellite`; commits: 0a80d45, f89c0a9 |
| REQ-SAT-007 | Must | - | untraced | range mention only: `core/satellite`; commits: 0a80d45, f89c0a9 |
| REQ-SAT-008 | Must | - | untraced | range mention only: `core/satellite`; commits: 0a80d45 |
| REQ-SAT-009 | Must | - | implemented | code: `core/satellite`, `protocol/message`, `satellite/lib`; commits: dc1b0e8, 60afe1f, f89c0a9 |
| REQ-SAT-010 | Must | - | implemented | code: `core/satellite`, `protocol/message`; commits: 25bd2c6, d438e5d, 0a80d45 |
| REQ-SAT-011 | Should | - | implemented | code: `protocol/message`; commits: 929e9ae |
| REQ-SAT-012 | Must | - | untraced | commits: 25bd2c6 |
| REQ-SAT-013 | Must | - | tested | code: `protocol/message`; tests: `protocol/message::capabilities_default_to_none_so_a_minimal_satellite_works`; commits: d438e5d, f89c0a9 |
| REQ-SAT-014 | Should | - | spec-only | - |
| REQ-SAT-015 | Must | - | untraced | commits: 8133fbf, 60afe1f |
| REQ-SAT-016 | Should | - | spec-only | - |
