# Requirement Traceability

Which requirements from [spec 0001](../specs/0001-product-requirements.md) and [spec 0003](../specs/0003-satellite-protocol.md) are implemented, where, and how they are verified. Maintained by hand until the traceability tool (TASK-006) exists; status as of 2026-11-07.

Legend: **Done** = implemented and tested. **Partial** = some of the requirement. **Not started**. "Tests" names a module's test group; run them with `scripts/cargo.sh test --workspace`.

## Agent inbox (INB)

| ID | Status | Where | Verified by |
|---|---|---|---|
| INB-001 endpoints | Done | `core::inbox::endpoint`, `core::app`, `core::http` | endpoint, app, and http tests |
| INB-002 per-endpoint revocable token | Done | `endpoint` (hash, rotate, revoke), streams end on rotation | `rotating_the_token_ends_the_stream`, endpoint tests |
| INB-003 event model and adapters | Done | `inbox::adapter`, `adapters/*.json` | adapter tests (Claude Code `Stop`, `Notification`, generic) |
| INB-004 persistence, bounded | Done | `inbox::queue`, `store` | restart and eviction tests |
| INB-005 tone and announce | Partial | announcement text and ordering done; the tone is a satellite concern | queue ordering tests |
| INB-006 defer while busy | Done | `Inbox::plan(device_idle)` | `nothing_is_announced_while_the_device_is_busy` |
| INB-007 hear, details, dismiss, snooze, reply | Done (by text) | `assistant` inbox intents | `the_agent_loop_by_voice` and related |
| INB-008 reply stream, device never calls out | Done | `http` SSE, `bridge` | `agent_loop` end-to-end tests |
| INB-009 outbound subscribe or poll sources | Not started | | |
| INB-010 inbox text is untrusted data | Done | summary prompt fencing; no tool is ever invoked from event text | summary and injection tests |
| INB-011 notification latency <= 3 s | Not measured | push is immediate; the announcer loop ticks every second | |
| INB-012 quiet hours, per-endpoint mute | Done (library) | `EndpointFilter`; no UI yet | queue tests |
| INB-013 integration snippets | Partial | the binary prints Claude Code hook settings; HA and Hermes recipes not written | |
| INB-014 filtering, priority, coalescing | Done | `inbox::queue` | coalescing and filter tests |
| INB-015 reference agent bridge | Done | `crates/bridge` | bridge and `agent_loop` tests |
| INB-016 "what are my agents doing" | Done | `Inbox::status_by_source`, `summary::status_sentence` | status tests |
| INB-017 reply types | Done | `ReplyKind`, assistant intents (`ack`, `instruct`; snooze, dismiss, details are local) | agent-loop tests |
| INB-018 bounded spoken summaries | Partial | prompt, clamp, fallback done; the `summary_model` call needs the LLM client | summary tests |
| INB-019 "Connect an agent" UI flow | Not started | needs the portal | |
| INB-020 pluggable bridge harnesses | Done (Claude Code only) | `bridge::harness` | harness tests |

## Timers, alarms, reminders (TMR)

| ID | Status | Where | Verified by |
|---|---|---|---|
| TMR-001 timers, alarms, reminders | Done | `core::sched` | sched tests |
| TMR-002 fire within +/- 1 s | Partial | deadlines are exact; the console loop ticks at 1 s | |
| TMR-003 persist, work offline | Done | `sched`, `store` | `state_survives_a_restart` |
| TMR-004 recurrence, dismiss, snooze | Partial | recurrence and snooze API done; no voice "snooze" for a ringing alarm | |
| TMR-005 time zone and DST | Done | `IanaZone` with bundled database | DST test across the 2026 spring-forward |
| TMR-006 no firing until the clock is valid | Done | `Clock::wall_clock_valid` | `nothing_fires_until_the_wall_clock_is_valid` |
| TMR-007 alarm sounds without providers | Not started | needs a satellite | |
| TMR-008 list, edit, cancel by voice and UI | Partial | list and cancel by voice; no edit, no UI | |

## Assistant (AST)

| ID | Status | Notes |
|---|---|---|
| AST-010 local fast path | Done | `core::fastpath` and `core::assistant`; speed and pathological-input tests |
| AST-001..003, 005..009, 011..014 | Not started | need the LLM client (OpenAI-compatible, streaming, tools, MCP, limits) |
| AST-004 built-in tools | Partial | timers, alarms, reminders, and inbox control work through the fast path; LLM tool versions not built |

## Satellite protocol (SAT)

| ID | Status | Where |
|---|---|---|
| SAT-001 framing | Done | `protocol::frame` (limits, sticky errors, fuzz-style test) |
| SAT-002, 003 hello, welcome, capabilities | Types done; handshake logic not wired into the core | `protocol::message` |
| SAT-004..007 formats, events, commands | Types done | `protocol::message` |
| SAT-008 keepalive and reconnect | Types only | |
| SAT-009 versioning | Done | `Version::compatible_with` |
| SAT-010 token auth | Field only | |
| SAT-011 namespaced extensions | Done | `kind::*` |
| SAT-012 conformance harness and schemas | Not started | |
| SAT-013 minimal capability set | Done (types) | |
| SAT-014..016 Wyoming bridge, reference satellites, display events | Not started | |

## Voice (VOI), config (CFG), music (MUS), extensions (EXT)

Not started, except: **EXT-001** (no loaded third-party code) holds trivially, **EXT-004** (adapters as data) is Done, and **SEC-007** (no telemetry) holds because nothing sends anything. The voice pipeline needs audio hardware and a wake-word runtime (SPIKE-002, SPIKE-006), the portal is M4, and music is M5.

## Security and non-functional

| ID | Status | Notes |
|---|---|---|
| SEC-004 inbound limits, no panics on malformed input | Done for HTTP and framing | body, connection, stream, and rate limits; random-garbage test; `unwrap`/`expect`/`panic` are lint errors |
| SEC-001 TLS to providers | Not started | no outbound provider calls yet |
| NFR-008 atomic writes | Done | `store` |
| NFR-010 schema versions | Partial | versioned and refuses newer data; no migrations yet |
| NFR-110 bounded everything | Done | queues, bodies, connections, streams, replies, history of handled ids |
| NFR-001, 101..103 budget | Measured 2026-11-07 after MCP and the UI status page (x86_64 musl, load of 400 hooks): binary 2.62 MiB (armv7: 2.10 MiB), idle RSS 3.0 MiB, peak RSS 4.3 MiB, state 114 KiB. The load test does not include live audio sessions. ARM memory and all CPU limits unmeasured | `scripts/budget.sh` |
