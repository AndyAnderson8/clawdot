# Action safety: who may run a tool, and when

How the core decides whether a tool call made by the language model actually runs. The requirements are REQ-AST-008, REQ-AST-015, REQ-INB-010, REQ-CFG-036 and REQ-SEC-010 to REQ-SEC-013 in [spec 0001](../specs/0001-product-requirements.md); the reasoning is in [agentic-scope.md](../design/agentic-scope.md) section 3. The code is `crates/core/src/session.rs` (`run_turn`), with the tool hosts in `assistant/tools.rs`, `httptools.rs`, `mcp.rs` and `extensions.rs`.

The model is never trusted. It can be steered by text it reads (an event, a web page, an MCP result) and it can simply be wrong. So every rule below is enforced by the host in code, from the user's own words, and none of it depends on what the model says.

## Risk tiers (REQ-SEC-010)

Every tool call has a tier, declared by whoever wrote or configured the tool and read through `ToolHost::risk(name, args)`:

| Tier | Meaning | Examples | What the host does |
|---|---|---|---|
| `read_only` | Reads; nothing leaves ClawDot | `current_time`, `list_timers`, `inbox_status`, `inbox_details` | Runs |
| `reversible` | Changes ClawDot's own state or something trivial | timers, alarms, reminders, music control, snooze, dismiss, `inbox_reply` `ack` | Runs |
| `costly` | Visible to others, costs money, or changes a remote system | `inbox_reply` `instruct`; every HTTP and MCP tool by default | Holds for the user's yes |
| `destructive` | Loses data or access | none built in | Not offered to the model unless `TurnConfig::allow_destructive` is on; held when it is |

Built-in tools declare theirs in `assistant/tools.rs`. A tool with no declaration is `costly`. HTTP tools take `tier` (or the older `confirm`) and MCP servers take `tiers` per tool, `confirm` and `no_confirm`; the default for both is `costly`. A tier set by the administrator is a `Risk` with the tier in force and a `baseline` (the conservative tier before the override); the baseline is what the untrusted-text rule falls back to. Hints that come from a server, such as an MCP `readOnlyHint` annotation, are ignored: only configuration and the portal can lower a tier.

The portal listing includes each HTTP and MCP tool's `tier`; a tier selector is a follow-up.

## Host-held confirmation (REQ-SEC-011)

A call whose tier is `costly` or `destructive` is not run. Instead, in the same round:

1. The host builds a **pending action**: the exact tool name, its arguments in canonical form (object keys sorted, so equal arguments compare equal), and a plain-language description produced by the tool host from those arguments (`ToolHost::describe_action`). Tools with no description of their own read back their name and exact arguments.
2. The host ends the turn with its own words: the description, then "Yes or no?". The model does not write this sentence, so its wording cannot soften or change it. The pending action is kept in the `Conversation`.
3. At the start of the **next** user turn the host reads the user's utterance (`is_yes`, `is_no`). Only a whole-utterance yes within 120 seconds runs the stored action, with the stored arguments. The result is put in the history as the result of that call and the model then tells the user what happened.
4. Anything else discards the action: a no or cancel (the host says "Okay, I won't do that."), an unrelated request, a sentence that merely contains "yes", or a yes after the 120 seconds ("That took too long..."). A second held action in a later round replaces the first. A bare yes with nothing pending goes to the model like any other words.

What cannot release an action: a `confirmed` argument (it is not offered in any schema, and one the model supplies is removed before the call unless the tool itself declares a parameter of that name); text in a tool result such as "the user said yes"; the model calling the same tool again, with the same or different arguments (a new call is a new question); the same turn's user text. A different tool, or any change to an argument, is a different action and starts a new question.

Who said yes is unknown: ClawDot does not identify speakers, so a guest or a television could answer. The 120-second window, the read-back and the half-duplex microphone limit that; this is recorded in the [threat model](../security/threat-model.md).

## Not built here (follow-ups)

The audit log of every call and decision (REQ-SEC-014), the evaluation harness (REQ-AST-012), per-day caps and dry-run (REQ-SEC-015, REQ-SEC-016) are separate branches in the P0 and P1 list of agentic-scope.md. Until the audit log exists, the console is the only record of a held action.
