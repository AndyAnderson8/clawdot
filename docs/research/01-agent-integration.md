# R-01: Agent integration (hooks, resume, MCP)

Date: 2026-11-07 · Feeds: spec 0001 §4.3 (INB), §4.2 (AST-005), ADR-0006, threat model T1/T4

## Findings

**Claude Code hooks** ([docs](https://code.claude.com/docs/en/hooks)), read from the primary source:
- Has a **native `"type": "http"` hook**. Claude Code POSTs the event JSON to a URL, with configurable `headers` (e.g. `Authorization: Bearer $TOKEN`, with env vars interpolated only if listed in `allowedEnvVars`). No curl script is needed.
- Relevant events: `Stop` (agent finished responding), `SubagentStop`, `Notification`, `SessionStart`, `StopFailure`, `TaskCompleted`.
- Common payload: `session_id`, `transcript_path`, `cwd`, `hook_event_name`, `permission_mode`, optional `agent_id`/`agent_type`.
- `Stop`/`SubagentStop` provide **`last_assistant_message`** (the docs recommend it over reading the transcript).
- `Notification` payload: `message`, `title` (optional), `notification_type`. Types include `permission_prompt`, `idle_prompt`, `agent_completed`, `agent_needs_input`, `elicitation_dialog`.
- Hooks can run with `async: true` so they never block the agent. Default timeouts are long (600 s) so a slow speaker could stall an agent unless async.
- `Stop` fires **every time Claude finishes responding**, not only at the end of a long task. Without filtering it would be noisy.

**Resuming a session** ([headless docs](https://code.claude.com/docs/en/headless)), primary source:
- `claude -p "<prompt>" --resume <session_id>` continues a specific session; `--output-format json` returns `session_id`, `result`, and cost. Sessions can be found by ID across directories on the same machine.
- Headless runs have their own permission behaviour (manual mode denies unapproved tools unless `--allowedTools` or a permission mode is set).
- **Implication:** a reply cannot be injected into a *running interactive* session via a hook. Resuming starts a new process on the same transcript. Something must run `claude -p --resume` on the developer's machine.

**Other agents** (secondary sources only, unverified against official docs):
- Codex CLI: root-level `notify` config runs an external command with a JSON argument on `agent-turn-complete` (type, turn id, last assistant message reported).
- Gemini CLI: hooks in `~/.gemini/settings.json`; `AfterAgent` is the turn-complete analogue (v0.26.0+ reported).

**MCP** (secondary sources): latest spec revision reported as 2026-07-28 (previous 2025-11-25); two standard transports, stdio and Streamable HTTP; the newest revision reportedly drops the initialize handshake and protocol-level sessions on Streamable HTTP. The Rust SDK (`rmcp`) status is unverified.

## Spec impact (recommended)

1. **Inbox accepts Claude Code's native hook payload** via a per-endpoint `format` adapter (`claude-code`, `codex`, `gemini`, `generic`). Setup is then "paste this JSON into settings.json" with a URL and bearer token. Adapters map to the generic event model. Update REQ-INB-003, REQ-INB-013.
2. **Introduce an agent bridge.** The reply callback target is a small program on the developer machine (not the agent): it receives `{session_ref, text}` and runs `claude -p --resume`. We ship a reference bridge. This resolves how REQ-INB-008 works and reduces the speaker's SSRF exposure (T4): the device only talks to a pre-configured bridge URL.
3. **Event filtering and priority** are first-class: ignore `Stop` events from turns shorter than a threshold; map `agent_needs_input`/`permission_prompt` to higher priority than `agent_completed`. Add REQ-INB-014.
4. **Summaries:** `last_assistant_message` can be long and may contain attacker-influenced text (fetched web pages, repo content). The core summarises it with the LLM as *data* (REQ-INB-010) before speaking it.
5. **Hooks must be async** in all shipped snippets, so the speaker or network can never stall an agent.
6. **MCP client scope:** support **Streamable HTTP only**. stdio means spawning subprocesses, which is out of scope on the target device. Pin the spec revision and re-verify against the official spec at M5. Refine REQ-AST-005.
7. **Possible future feature (not v1):** voice approval of `permission_prompt` events. High value, high risk. It needs its own spec and threat-model entry.

## Confidence
Claude Code: high (primary). Codex/Gemini/MCP: medium-low (secondary). Verify before writing the adapters.
