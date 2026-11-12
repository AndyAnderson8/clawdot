# Action safety: who may run a tool, and when

How the core decides whether a tool call made by the language model actually runs. The requirements are REQ-AST-008, REQ-AST-015, REQ-INB-010, REQ-CFG-036 and REQ-SEC-010 to REQ-SEC-016 in [spec 0001](../specs/0001-product-requirements.md); the reasoning is in [agentic-scope.md](../design/agentic-scope.md) section 3. The code is `crates/core/src/session.rs` (`run_turn_with`), with the tool hosts in `assistant/tools.rs`, `httptools.rs`, `mcp.rs` and `extensions.rs`.

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

The portal lists each tool's tier and lets the administrator choose it (REQ-SEC-030, `POST /api/tools/<name>/tier`): an HTTP or MCP tool can be set to read-only, reversible or costly (or back to its declared tier), and a built-in tool can only be raised. `destructive` is not offered. The choice is stored in `tools.json`, applies to the next call, and is applied in `WithPolicy::risk` on top of the declared tier with the baseline unchanged, so untrusted text in a turn still holds a lowered tool.

## Host-held confirmation (REQ-SEC-011)

A call whose tier is `costly` or `destructive` is not run. Instead, in the same round:

1. The host builds a **pending action**: the exact tool name, its arguments in canonical form (object keys sorted, so equal arguments compare equal), and a plain-language description produced by the tool host from those arguments (`ToolHost::describe_action`). Tools with no description of their own read back their name and their arguments, made safe to say: the audit log's redaction is applied (`audit::spoken_args`, the same code as the log), and a text value longer than 60 characters is spoken as "a long value". A tool host's own description (for example the `instruct` reply, which must be the exact text, REQ-AST-015) is spoken as written. Only the spoken words change; the stored action keeps its exact arguments, so a yes runs what was asked.
2. The host ends the turn with its own words: the description, then "Yes or no?". The model does not write this sentence, so its wording cannot soften or change it. The pending action is kept in the `Conversation`.
3. At the start of the **next** user turn the host reads the user's utterance (`is_yes`, `is_no`). Only a whole-utterance yes within 120 seconds runs the stored action, with the stored arguments. The result is put in the history as the result of that call and the model then tells the user what happened.
4. Anything else discards the action: a no or cancel (the host says "Okay, I won't do that."), an unrelated request, a sentence that merely contains "yes", or a yes after the 120 seconds ("That took too long..."). A second held action in a later round replaces the first. A bare yes with nothing pending goes to the model like any other words.

What cannot release an action: a `confirmed` argument (it is not offered in any schema, and one the model supplies is removed before the call unless the tool itself declares a parameter of that name); text in a tool result such as "the user said yes"; the model calling the same tool again, with the same or different arguments (a new call is a new question); the same turn's user text. A different tool, or any change to an argument, is a different action and starts a new question.

Who said yes is unknown: ClawDot does not identify speakers, so a guest or a television could answer. The 120-second window, the read-back and the half-duplex microphone limit that; this is recorded in the [threat model](../security/threat-model.md).

## Turns the user did not start (REQ-SEC-012, REQ-INB-010)

`run_turn_with` is for the user's voice. A turn started by anything else (an inbox event, a schedule, a rule) must use `run_event_turn_with`, which sets `Trigger::Event`:

- The trigger's text is shown to the model fenced in `<event_text>` and labelled untrusted, with angle brackets removed so it cannot close the fence. It is never read as an answer: an event cannot release a held action, however it is worded.
- Only `read_only` tools run. A call to anything else is not run and is not refused outright: it is saved as a proposal (the same pending action as above) and the host speaks it as "Suggested action: ... Say yes if you want that, or no to drop it." The user's own later turn decides, within 120 seconds, exactly as for a normal held action.
- The turn counts as holding untrusted text (see below).

Nothing in the product starts an LLM turn from an event yet (announcements are spoken without the model); this is the gate that any such feature, and the rules of agentic-scope UC9, must go through.

## Untrusted text in the turn (REQ-SEC-012)

A conversation is *tainted* once text written by someone else is in what the model reads: an event turn's text, or the result of a tool that `ToolHost::returns_untrusted_text` says returns outside text. The built-in timers, alarms, reminders and music tools do not; `inbox_status`, `inbox_details`, every HTTP tool and every MCP tool do, and so does any tool that does not say (the default is untrusted).

While tainted, a call that is not `read_only` is judged at its `baseline` tier instead of the one the administrator set (`Risk::under`). So an HTTP or MCP tool that the administrator lowered to `reversible` (for a light, say) is held for a yes again once a web page or an alert has been read; a `read_only` tool and the built-in reversible tools are not affected. The host's read-back then adds that the request followed outside text, so the user knows to listen carefully.

The taint lasts as long as the conversation's history does (it is cleared when the history expires after 600 seconds idle), because the text stays in the context the model reads. A later turn in the same conversation is therefore held to the same rule.

Not done here: fencing every tool result in angle brackets and naming which argument came from outside text (agentic-scope G6 and the read-back rule in section 3.4). The `Risk` baseline and `tainted` flag are the hooks for the second.

## Skill allow-lists (REQ-SEC-013, REQ-CFG-036)

A skill's `tools` list is a permission, not just a hint. In `extensions.rs`, `Extensions::active_scope(user_text)` finds the enabled skills the user named in this turn (the skill's name as whole words, any case) and, if any of them lists tools, returns the union of those lists. `WithPolicy` stores it at the start of the turn (`begin_turn`), then drops every other tool from what the model is offered and refuses a call to one by name. Because it only filters, a scope can narrow what is available but never turn a tool on, lower a tier or skip a confirmation: a tool that is off stays off, and the held-action rules above still apply to what remains. A skill with an empty list, a skill that is off, and a turn that names no skill have no scope. The scope lasts for one turn: the user's "yes" to a held action is a new turn and releases the stored action as proposed in the scoped turn.

Not done: scopes for inbox endpoints and rules, and a tier ceiling per scope (agentic-scope P1-5), and showing an imported skill's allow-list in the portal before it is enabled (imported skills already arrive off and marked for review, and cannot be turned on until accepted, REQ-SKL-002).

## Audit log (REQ-SEC-014)

Every call, hold, answer, refusal and cap hit writes one line to `audit.jsonl` in the data directory (code: `crates/core/src/audit.rs`, written from `run_turn_with`). The file is append-only, created 0600, and rotated at 256 KB to `audit.1.jsonl` (the older file is dropped), so the disk holds at most 512 KB. At start-up a crash is tolerated: damaged lines, a half-written last line and lines from a newer format are skipped and counted, and the next record starts on a fresh line. If a record cannot be written (disk full) the action still proceeds, the failure is counted in the log's health figures and printed to the console; the alternative, refusing to act without a record, would let a full disk disable the speaker's timers.

A record holds: `id` (counts up across restarts), `ts`, `day`, `turn`, `trigger` (`voice`, or `event:<source>` for a turn from `run_event_turn_with`), `tool`, `tier`, `args`, `decision`, `held_id`, `dry_run`, `status` and `result` (when it ran), `turn_cost_usd` (the model spend so far in that turn, when the provider reports cost) and a fixed `note`.

| `decision` | Meaning |
|---|---|
| `ran` | A reversible or read-only call ran |
| `held` | A costly or destructive call is waiting for the user's yes |
| `proposed` | A turn the user did not start asked for an action; kept as a proposal |
| `approved` | The user's yes released a held or proposed call (`held_id` points at it) |
| `declined`, `expired` | The user said no or asked for something else; the answer came too late, or the conversation went idle |
| `blocked:cap`, `blocked:dry-run` | A daily cap or dry-run mode stopped it |
| `blocked:invalid`, `blocked:pending` | The model named no such tool, sent unreadable arguments, or asked for something while another action was being asked about |

What is kept is bounded and cleaned before it is written. Argument keys that sound secret (token, key, password, auth, cookie and similar) have their value replaced; in text, bearer values, `key=value` pairs with a secret-sounding key, passwords in URLs, well-known token formats (`sk-`, `ghp_`, `cdt_`, JWTs) and long token-like strings are replaced; every value is clipped to 200 characters, a record's arguments to about 1 KB, and a tool name or trigger to plain characters. A result is kept only as its first 200 characters. The user's words and audio are never written. Redaction is heuristic: a secret in an unusual form can survive, which is why the log is private to the service user and only the administrator can read it.

`GET /api/audit` (administrator only, read-only) returns the records newest first with `limit` (default 50, at most 200), `before` (a record id: pass the previous page's `next_before`), and the filters `tool`, `tier`, `decision` (a value, or a prefix ending in a colon such as `blocked:`), `trigger` (`voice`, `event`, `event:<source>`) and `since` (Unix seconds). The answer also carries the log's health figures and the policy in force. Argument and result text can come from outside; a page showing it must render it as plain text. The portal's **Activity** tab shows it read-only (REQ-CFG-060): each record gets a plain-words outcome, the redacted arguments and result as text, the long `detail` on request, the filters above, and a "Show older" button that passes `next_before`. It has no control that changes or deletes a record.

## Daily caps and dry-run (REQ-SEC-015, REQ-SEC-016)

`limits.max_actions_per_day` (default 100) counts calls that ran and are not read-only; `limits.max_costly_per_day` (default 20) counts the costly and destructive ones among them. Counting is from the audit log itself, per local day, so a restart does not reset it (rotation can forget a very busy day, which only errs towards allowing). A call over a cap is not run and not asked about: the model gets "Not run: the limit of N actions a day is used up", the record says `blocked:cap`, and a held call is checked again when the yes arrives. Read-only calls are never limited.

`limits.dry_run` or `--dry-run` makes every call that is not read-only return "Dry run: nothing was done. It would have: ..." (the same plain description that a confirmation reads back) and record `blocked:dry-run`; nothing is held or asked, and dry-run calls use none of the day's allowance. Read-only tools still run, since reading is what makes the plan believable. The policy is a setting like the others (REQ-CFG-062): the configuration file, the portal and the flag are layered, and a change in the portal applies to the next call (`AuditLog::set_policy`). A flag such as `--dry-run` still wins over a saved value, and the page says so.

## Not built here (follow-ups)

The second factor for destructive tools (a repeated target name, a button or portal tap, agentic-scope P2-6), fencing of every tool result, a repeat-within-10-seconds limit, per-tool caps and dry-run are separate work in the P0 and P1 list of agentic-scope.md.
