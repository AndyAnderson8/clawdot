# R-12: Agent harnesses as an optional brain (Hermes Agent), and what to learn from opencode

Date: 2026-11-11 · Feeds: [roadmap](../roadmap.md) (agent integration), [future-scope.md](../future-scope.md) (ClawDot MCP server, multiple assistant profiles, agent connect kits), REQ-EXT-006, REQ-INB-020

The maintainer wants two things. (a) Use an agent harness, Hermes Agent, as ClawDot's optional **brain**, with ClawDot staying the voice front end, the device, the safety checks and the portal. (b) Learn what Hermes Agent and opencode do well that would improve ClawDot's own agent loop. opencode is **not** a brain candidate (the maintainer's correction, section 3). This note is research and a recommendation, not a decision; nothing here changes a contract until it becomes a spec amendment or an ADR.

Everything below was read from each project's own repository and documentation on 2026-11-11. **Nothing was run.** No harness was installed and no call was made to either one; the "works today" statements about ClawDot come from reading its code and docs.

## 1. Short answer

* **Hermes Agent**: link design 1 (point `llm.url` at its OpenAI-compatible server) works today and is the right first step, with Hermes limited to read-only toolsets. Add design 2 (a ClawDot MCP server plus the existing announce path) later, for scheduled and long jobs.
* **opencode** is not a brain candidate (the maintainer's decision). Study its design (client-server split, permission rules, skills loading, project memory files) and borrow ideas, not code: it is MIT-licensed TypeScript for a developer machine (section 3).
* **ACP** (design 3) is not recommended now. It is an editor-to-agent protocol over a local subprocess; ClawDot's core never starts processes (REQ-EXT-001). It becomes worthwhile only as one adapter in an off-device bridge.
* **Safety**: ClawDot's ask-out-loud tiers hold only for actions that pass through ClawDot's host. An action a harness takes with its own tools is governed by the harness's own approval system. The designs differ mainly in how they close that gap (section 5).
* **Cheapest wins for the loop itself**: SKILL.md compatibility is already half built; a draft-a-skill flow that the person must accept, searchable notes and a published OpenAPI description are the best small steps (section 7).

## 2. Hermes Agent

Source: [github.com/NousResearch/hermes-agent](https://github.com/NousResearch/hermes-agent) and [the docs](https://hermes-agent.nousresearch.com/docs/). Latest release seen: **v0.23.1**, 2026-11-10 (GitHub releases API). Licence: **MIT** (repository page). Not an open-weights model; it is a harness that uses any model provider.

### How it is driven from outside

| Surface | What it is | Source page |
|---|---|---|
| **API server** | An OpenAI-compatible HTTP server, started by `hermes gateway` with `API_SERVER_ENABLED=true`. Default `127.0.0.1:8642`, base path `/v1`. `POST /v1/chat/completions` (stateless, client sends the whole `messages` array), `POST /v1/responses` (server-side state), `GET /v1/models`, `/v1/skills`, `/v1/toolsets`, `/v1/capabilities`, health probes, and a **runs** API: `POST /v1/runs`, `GET /v1/runs/{id}/events` (SSE), `/stop`, `/steer`, `/approval`, `/clarify`. Also `/api/jobs` (scheduled jobs) and `/api/sessions`. | [API server](https://hermes-agent.nousresearch.com/docs/user-guide/features/api-server), [programmatic integration](https://hermes-agent.nousresearch.com/docs/developer-guide/programmatic-integration) |
| **Auth** | `Authorization: Bearer <API_SERVER_KEY>`; the page says the key is required for every deployment, even on loopback. CORS is off unless an allowlist is set. | API server page |
| **Streaming** | Standard `chat.completion.chunk` events; tool starts arrive as an extra `hermes.tool.progress` event (can be switched off with `gateway.platforms.api_server.tool_progress_events: false`); `delta.reasoning_content` carries reasoning; a `: keepalive` comment after 10 s of silence. | API server page |
| **Sessions** | `X-Hermes-Session-Id` continues a transcript; `X-Hermes-Session-Key` scopes long-term memory; `Idempotency-Key` de-duplicates. | API server page |
| **ACP** | `hermes acp`, a stdio JSON-RPC server for editors (VS Code, Zed, JetBrains). Permission requests are sent to the client; the default wait is 300 s, then deny. MCP servers can be passed in `session/new` if the gateway advertises `acp-session-mcp-v1`. | [ACP](https://hermes-agent.nousresearch.com/docs/user-guide/features/acp) |
| **Hermes as an MCP server** | `hermes mcp serve` was moved into the `mcp-serve` plugin (`hermes plugins install mcp-serve`, then run `hermes mcp-serve` over stdio). | [MCP](https://hermes-agent.nousresearch.com/docs/user-guide/features/mcp) |
| **TUI gateway** | JSON-RPC over stdio or WebSocket with `approval` and `clarify` requests sent to the client; meant for custom hosts. | programmatic integration page |
| **Python** | `run_agent.AIAgent`, in process. | programmatic integration page |

### Skills, memory, scheduling, permissions, MCP

* **Skills** ([page](https://hermes-agent.nousresearch.com/docs/user-guide/features/skills)). A directory per skill with a required `SKILL.md` (frontmatter `name`, `description`, `version`, `platforms`, `metadata.hermes`, `required_environment_variables`) and optional `references/`, `templates/`, `scripts/`, `assets/`. It says it is compatible with the [agentskills.io](https://agentskills.io) open standard (I did not read that standard, so the exact overlap is unverified). Progressive disclosure in three levels (list, view, view a file). **The agent writes its own skills** with a `skill_manage` tool (`create`, `patch`, `edit`, `delete`); `skills.write_approval: true` stages every write for the person to approve (`/skills pending`, `/skills approve`). A hub installs skills from several sources with a built-in scanner for exfiltration and injection patterns; a `dangerous` verdict cannot be overridden.
* **Memory** ([page](https://hermes-agent.nousresearch.com/docs/user-guide/features/memory)). Two small files in `~/.hermes/memories/`: `MEMORY.md` (the agent's notes, 2,200 characters) and `USER.md` (the person, 1,375 characters), frozen into the system prompt at session start. No auto-compaction: a write over the limit is an error the agent must resolve. **Session search**: every past session is kept in SQLite (`state.db`) with FTS5 full-text search, returned as the stored messages (no model summary). Optional external memory providers (Honcho, Mem0 and others) run beside it.
* **Scheduling** ([page](https://hermes-agent.nousresearch.com/docs/user-guide/features/cron)). Jobs in `~/.hermes/cron/jobs.json`, created by `hermes cron create`, `/cron add`, or by the agent through a `cronjob_manage` tool. Schedules: one-shot (`in 30m`), interval, "every monday 9am", cron expressions. The gateway checks every 60 s; each due job starts a **fresh** session, runs a self-contained prompt, and delivers the final answer to a target (a chat platform, `local`, `bot-chat`). Cron runs cannot create jobs unless `cron.allow_agent_scheduling` is on. A response containing `[SILENT]` suppresses delivery. Script jobs run only from `$HERMES_HOME/scripts/`. I found no documented "deliver to a webhook" target (see section 4).
* **Permissions** ([security page](https://hermes-agent.nousresearch.com/docs/user-guide/security)). `approvals.mode` is `smart` (default: an auxiliary model scores a dangerous command, approves low risk, auto-denies the worst, asks about the rest), `manual` (always ask) or `off` (same as `--yolo`). A hardline blocklist (`rm -rf /`, fork bombs) always applies. In the CLI the prompt is inline; on messaging platforms the person replies "yes" or "no" in the chat; unanswered prompts are denied after `timeout` (default 300 s). **On the Docker, Modal and similar terminal backends the dangerous-command checks are skipped because the container is the boundary.** "Always" answers are saved to `command_allowlist`. Approvals on the runs API arrive as `waiting_for_approval` with `pending_controls`, answered on `/approval`. The documentation does not say whether `/v1/chat/completions` can surface an approval at all; treat that as "no" until tested.
* **MCP client** ([page](https://hermes-agent.nousresearch.com/docs/user-guide/features/mcp)). `mcp_servers:` in `~/.hermes/config.yaml`, stdio (`command`, `args`, `env`) or HTTP (`url`, `headers`; OAuth and legacy SSE are mentioned). Tool names become `mcp_<server>_<tool>`. `tools.include` and `tools.exclude` filter. Sampling is on by default with caps. So a ClawDot MCP server over Streamable HTTP with a bearer header is a supported shape.

### Resource needs

Not stated in the pages I could read (the quick start gave no RAM or disk figures; the installation guide was not fetched). Known: it installs Python (3.14 per the README), Node.js, ripgrep and FFmpeg; it needs a **model with at least 64,000 tokens of context**; it runs a gateway daemon. It is a PC or NAS service, not a device-class one. I could not verify idle memory. A first real run needs only a PC or home server that can reach a model provider.

### Gaps for a voice use

* **Latency.** The API server runs the full agent with its toolset and the final answer arrives after every tool round. Chat Completions streams text, and tool progress arrives as extra events, but a plain OpenAI client ignores them. A voice reply that needs three tool rounds is slow with nothing said in between (ClawDot's own loop speaks a plan and progress phrases, [agent-loop.md](../architecture/agent-loop.md)).
* **Who asks for confirmation.** Hermes asks on its own channel (CLI, chat platform, runs API). Over chat completions there is no documented path, so a command that needs approval would wait and then be denied, or run under `smart` or `off`.
* **Client tools.** The page does not say what happens to a `tools` array sent by the client (ClawDot sends one with `tool_choice: auto`, `crates/core/src/llm.rs`). Tools run on the server and are replayed as completed, so ClawDot's own tools (timers, notes, its HTTP tools) probably cannot be called through this link. Unverified; this is the first thing to test.
* **Persona.** A `system` message is layered on top of Hermes's own prompt, so ClawDot's spoken-style persona works but competes with it.
* **Tool results.** They stay inside Hermes. ClawDot sees only the reply text (and optionally the progress events). That is fine for a brain, but it means ClawDot's audit log shows one opaque model call, not the actions behind it.
* **Voice mode** exists in Hermes (CLI, Telegram, Discord) with its own speech-to-text and speech-to-speech providers. Nothing in the docs I read is an external voice front-end API, and ClawDot does not need one.

## 3. opencode: what is worth studying for ClawDot's own loop

The maintainer corrected the brief: opencode is **not** a candidate brain, and this note gives it no link design and no recipe. The question is only whether it is wise to study its code and design to improve ClawDot's own harness. Short answer: **yes, study the design; do not copy the code.**

Sources: [github.com/sst/opencode](https://github.com/sst/opencode) and [opencode.ai/docs](https://opencode.ai/docs), read 2026-11-11; latest release seen **v1.19.5**, 2026-10-28. The docs site describes a newer configuration shape than I can tie to that release, so treat config details as "check against the installed version". Nothing was run.

| Fact | Value | Bearing on ClawDot |
|---|---|---|
| Licence | MIT (`LICENSE`, "Copyright (c) 2025 opencode") | Permissive, so reading and learning from it is free; copying code would need the notice kept. Not needed. |
| Language | About 84 percent TypeScript, 12 percent MDX, under 1 percent Rust (GitHub languages API) | A different language and runtime from ClawDot's Rust core, which must fit an Echo Dot Gen 2 (spec 0002). Nothing could be linked in; ideas only. |
| Purpose | A coding agent for a developer machine: files, shell, a working directory | ClawDot is a voice assistant on a small device. Most of its tools (edit, patch, shell) have no place on the speaker. |

**Worth studying (design, not code)**

* **Client-server split.** `opencode serve` owns sessions, tools and permissions; the terminal UI, a desktop app and editors are clients of one HTTP and SSE API, with the OpenAPI 3.1 description served at `/doc` ([server](https://opencode.ai/docs/server/)). ClawDot already has the same shape (core plus satellite, portal and bridges). The lesson is the published, versioned description (item 4 in section 7) and a single event stream that the portal and sidecars both consume.
* **Permissions as data.** Rules of `allow`, `ask` or `deny` per action with wildcard resources, last match wins, `deny` always enforced, and an answer of once, always or reject ([permissions](https://opencode.ai/docs/permissions/)). ClawDot's four risk tiers are per tool; per-argument rules are a possible refinement (item 5). Not to copy: its default allows nearly everything, and "always" saves a rule from a single answer, both wrong for a speaker (ClawDot's host asks, and a spoken "always" is not a safe grant).
* **Skills.** `SKILL.md` folders, found in `.opencode`, `.claude` and `.agents` directories, advertised by description and loaded on demand by a `skill` call, with `allow`, `ask` or `deny` per skill ([skills](https://opencode.ai/docs/skills/)). This is the format ClawDot's portal already imports (section 4); the per-skill permission and the on-demand load are the useful parts (item 2).
* **Project memory files.** `AGENTS.md` (global and per project, with `CLAUDE.md` as a fallback) plus an `instructions` list ([rules](https://opencode.ai/docs/rules/)). There is no learned memory; persistence is plain text the person owns. This matches ClawDot's notes philosophy (only the host writes, from the person's words) and supports keeping that rule.
* **Plugins and MCP.** Tools come from MCP servers (stdio or Streamable HTTP, OAuth by default for remote) named `<server>_<tool>` ([MCP](https://opencode.ai/docs/mcp-servers/)). ClawDot's MCP client already does the HTTP half, with per-tool tiers and a size cap; OAuth is a known gap on ClawDot's side. Its plugin mechanism was not read; third-party code running inside the core is what REQ-EXT-001 forbids for ClawDot.
* **ACP.** `opencode acp` makes it an agent for editors over JSON-RPC on stdio ([ACP](https://opencode.ai/docs/acp/)). Relevant only as a protocol to know, since ClawDot is not an editor and the core starts no processes (section 5, design 3).

**Not worth taking:** its coding tools, the terminal UI, the diff and patch machinery, and the third-party memory plugins (not evaluated). It has no scheduler. What it does in non-interactive mode when a permission is `ask` is undocumented in what I read, which is itself a warning: a voice client must never depend on an unattended default.

## 4. What ClawDot already has to build on

Read from the repository on `origin/dev` (7cd74ae):

* **The language-model address is configurable.** `llm.url` plus `llm.model` and `llm.key_env`; `https://` is required unless the host is localhost, `.local` or a private address ([configuration.md](../architecture/configuration.md)). The client posts `{url}/chat/completions`, streams, and sends `tools` with `tool_choice: auto` (`crates/core/src/llm.rs`, `transport.rs`). The portal has a key field and a model list (REQ-CFG-082).
* **Inbox, announce and the reply stream.** `POST /api/announce` and `clawdot say` ([announce guide](../guides/announce.md), REQ-INB-021..027); endpoint tokens, quiet hours, burst limits, the audit log; a per-endpoint SSE reply stream and a bridge with pluggable per-harness adapters (REQ-INB-008, REQ-INB-020); the `claude-code` hook adapter.
* **MCP client only** (Streamable HTTP, revision `2025-06-18`, [mcp.md](../architecture/mcp.md)). Its tools are `costly` by default so the host reads them back and waits for a yes (REQ-SEC-010, REQ-SEC-011). A ClawDot MCP server is explicitly out of v1.0 (REQ-EXT-006, [future-scope.md](../future-scope.md)) because an agent that can interrupt you and block on your answer is a spoken-injection surface.
* **`ask_user`** exists inside the loop: it ends the turn with the model's question and the next utterance is the answer ([agent-loop.md](../architecture/agent-loop.md), REQ-AST-021).
* **Skills** are text plus an optional tool allow-list ([extensibility.md](../architecture/extensibility.md), REQ-CFG-036, REQ-SEC-013). The portal's Skills tab already reads `SKILL.md` files (`readSkillFiles` in `portal/src/forms.ts`): a Markdown file with `---` frontmatter `name` and `description` (the folder name otherwise), a body up to 2,000 characters and a name up to 40 characters; anything else is listed as skipped with the reason. Imports arrive turned off (REQ-CFG-036).
* **Recipes** are declarative HTTP tools, MCP connections or inbox endpoints, each with a stated tier, credentials as environment variable names only, results clipped to 1,000 characters ([recipes/README.md](../recipes/README.md)).
* **Notes** are the only memory: at most 50 notes of 200 characters, written only by the host from the person's own "remember that ..." (`notes.rs`, REQ-AST-023).

## 5. Three link designs

```
Design 1   mic > ClawDot > [llm.url] > harness (runs its own tools) > text > ClawDot speaks
Design 2   harness (MCP client) > ClawDot MCP server: announce, ask_user  > speaker
           harness / script > POST /api/announce or the reply stream      > speaker
Design 3   ClawDot (bridge) > ACP over stdio > harness; permission requests relayed to the person
```

### Design 1: the harness is the language model

ClawDot's `llm.url` points at the harness's OpenAI-compatible server. Works today for Hermes (section 6 and the [recipe](../recipes/hermes-brain.md)).

* **Needs built:** nothing in the core to try it. Useful additions: show the harness name in the portal's model field, treat a slow reply as normal (raise the per-request timeout, see open questions), and speak "one moment" while waiting, which the agent loop already does for tools but not for a slow model.
* **Safety:** weakest of the three. Any action the harness takes is invisible to ClawDot: its tiers, daily caps, taint rule and audit log do not see it. The mitigation is configuration, not code: run the Hermes profile with **only read-only toolsets** (web, memory, skills), no terminal, no file write, no messaging and no cron creation, and keep acting tools in ClawDot where the tiers apply. With that profile design 1 is a better "knows things" brain, not a doer.
* **Latency:** the first spoken word waits for the harness's first token. If it uses tools first, that can be many seconds.

### Design 2: the harness is an MCP client of ClawDot, plus a webhook for results

The harness has a ClawDot MCP server in its `mcp_servers` list and its own jobs call two tools: `announce` (say this, one way) and `ask_user` (ask this, wait for the spoken answer). Results of long or scheduled work come back through the existing announce path or the inbox webhook; spoken instructions go back through the reply stream and a bridge adapter. Hermes is an MCP client over Streamable HTTP with a bearer header.

* **Needs built:** (1) the ClawDot MCP server, a Streamable HTTP endpoint on the existing listener authenticated with an **endpoint token** (so one agent can be muted or revoked on its own), exposing exactly two tools; `announce` is a thin wrapper over the announce path (same untrusted-text rules, REQ-INB-022); (2) `ask_user` as a blocking call with a hard time limit that returns the person's words as **data**; (3) a spec amendment replacing REQ-EXT-006's "not part of v1.0" and a threat-model entry for spoken prompt injection (an agent speaking a convincing "your bank needs you to say yes"); (4) a recipe. Without the MCP server, a harness can already reach the speaker by an HTTP call to `/api/announce` (a Hermes script job, or any harness that can run `curl`), at the cost of no `ask_user`.
* **Safety:** the best of the three for what the person says, because the person's "yes" is a ClawDot-held event. But `ask_user` returns an answer to a harness whose own tools may act on it, so the rule must be: the answer is data, ClawDot never lets it satisfy a ClawDot tier (the reply to an `ask_user` can never stand in for a confirmation of a ClawDot action, matching `inj-announce-text-cannot-say-yes`), and the harness's own approval system stays on for anything that acts. This design covers **what the harness says to the person**, not **what the harness does**. To make ClawDot's tiers apply to acting, expose the acting tools through ClawDot (HTTP tools and MCP connections, which already default to `costly`) and give the harness only those, or relay its approval requests (design 3 and the HTTP approval endpoints).
* **Latency:** best for long work, because nothing blocks the voice loop; the speaker is told when it is done.

### Design 3: ACP

Hermes can run as an ACP agent (`hermes acp`). The protocol ([agentclientprotocol.com](https://agentclientprotocol.com/protocol/overview)) is JSON-RPC 2.0 with `session/prompt`, `session/update` notifications and a client-side `session/request_permission` that asks the user to authorise a tool call. The overview page I read names no transport; Hermes documents stdio and a subprocess model.

* **Needs built:** a process that starts the harness as a child and speaks ACP to it, which the core must not do (REQ-EXT-001). It would be an off-device bridge (a sidecar, like the Discord and ntfy bridges) that turns `session/request_permission` into a spoken read-back and the person's yes or no into the answer, and `session/update` into progress phrases. One such bridge would serve both harnesses and any later ACP agent.
* **Safety:** the best match to ClawDot's model, because the *client* owns permission: the harness cannot act without the bridge answering, and the bridge can ask out loud with the exact action read back. Weak point: the bridge must not auto-allow (the Hermes docs note a headless client that answers permission requests itself), and "allow always" must not be offered to the voice user.
* **Cost:** the most new code; the editor-centred protocol assumes a working directory, file tools and diffs, not a conversation. The protocol's version and its handling of MCP servers per session were only partly confirmed.

### Recommendation for Hermes Agent

**Design 1 now**, with a read-only Hermes profile, behind `API_SERVER_KEY` on the LAN. **Design 2 next**, for scheduled and long jobs (needs the ClawDot MCP server, or the existing announce call from a Hermes script job). ACP only if a generic off-device bridge is ever wanted.

* **Why:** Hermes already serves what ClawDot already speaks, so the trial costs one configuration change, and the best things about Hermes (skills it writes, searchable history, cron) are reachable without new code.
* **Safety rule:** Hermes keeps its own approvals on; the voice profile gets no acting toolsets. Anything that acts is a ClawDot tool with a ClawDot tier, so the person is asked out loud.
* **What ClawDot would need to build:** nothing for design 1. For design 2: the two-tool MCP server, the `ask_user` time limit and answer-is-data rule, a threat-model entry, a spec amendment (section "Spec impact"), and a recipe.

## 6. A first run, in order

1. **Hermes, design 1** ([recipe](../recipes/hermes-brain.md)): start the API server with a read-only profile, point `llm.url` at it, ask a factual question and a two-tool question, and record the time to the first word. Then ask something that needs a ClawDot tool ("set a timer") and see whether the harness answers or drops the `tools` array.
2. **Hermes cron to speaker**: a script job that calls `/api/announce` (works with no new ClawDot code).

## 7. What the harnesses do well that ClawDot's own loop could use

Ranked by value for the effort. All stay inside the existing safety model; none adds a dependency.

| # | Idea | Learned from | What it is | Files it would touch |
|---|---|---|---|---|
| 1 | **A draft-a-skill flow the person must accept** | Hermes `skill_manage` with `skills.write_approval` | After a multi-step turn, the person says "make that a skill". The host (not the model) drafts a skill from the turn's own record (the plan, the tools used) with the summary model, shows it in the portal as a **pending draft**, and nothing is saved or enabled until accepted in the portal. Hermes lets the agent write skills freely and offers approval as an option; ClawDot should make approval the only path, since a skill is standing instruction text. | `crates/core/src/extensions.rs` (a drafts list, never loaded), `crates/core/src/session.rs` (the voice trigger, like `notes::parse`), `crates/core/src/app/portal.rs` (`/api/skills/drafts`), `portal/src/tabs/Skills.tsx`, spec (new REQ), `evals/scenarios/` (a draft cannot enable itself) |
| 2 | **SKILL.md compatibility** | both: `SKILL.md` + frontmatter, progressive disclosure; opencode reads `.claude/skills` and `.agents/skills` | Already half there: the portal imports a `SKILL.md` with `name` and `description`. Gaps: the 2,000-character body limit and 40-character name drop real skills with bundled `references/`; `allowed-tools`-style frontmatter is not mapped to the tool allow-list; no list of what an imported skill needs. Do: accept a skill folder or zip, import the body up to the limit and store `description` separately so only descriptions go into the prompt and the body loads on demand (a `read_skill` tool, `read_only`), map a declared tool list onto `tools` (narrowing only), and keep the "arrive turned off" rule. | `portal/src/forms.ts` (`readSkillFiles`), `portal/src/tabs/Skills.tsx`, `crates/core/src/extensions.rs` (`skills_prompt`, a description field), `docs/architecture/extensibility.md`, `portal/test/unit/forms.test.ts` |
| 3 | **Searchable memory** | Hermes FTS5 session search | Notes are 50 short lines; asking "what did we decide about the NAS" cannot search past conversations. Add a bounded, local, opt-in full-text index of the person's own utterances and spoken answers (not tool output), searched by the host on "what did I say about ..." and exposed to the model as a `read_only` `search_history` tool whose results are fenced as data. Plain substring or a small inverted index is enough at this size; no SQLite dependency is needed (the budget in `docs/specs/0002-resource-budget.md` applies). Retention and a "forget" control come first. | `crates/core/src/notes.rs` (or a new `history.rs`), `crates/core/src/session.rs`, `crates/core/src/app/portal.rs` (`GET/DELETE /api/history`), `portal/src/tabs/Notes.tsx`, `docs/specs/` (a requirement), `evals/scenarios/` (history text cannot act) |
| 4 | **A published OpenAPI description of the portal API** | opencode serves its OpenAPI 3.1 at `/doc` | An agent or a script can then drive the portal without reading the Rust. The routes are many and bespoke; a hand-maintained `docs/api/openapi.yaml` checked by a test that every route in the router appears in it is more honest than generating it. REQ-EXT-005 already requires the sidecar surface to be documented and versioned. | `docs/api/openapi.yaml` (new), `crates/core/src/app/portal.rs` and `announce_api.rs` (a route-list test), `scripts/check.sh` (link check), `docs/architecture/extensibility.md` |
| 5 | **Permission granularity by pattern** | opencode `allow`/`ask`/`deny` with wildcards, last match wins | ClawDot's tiers are per tool. A per-argument rule ("allow `set_light` for the kitchen, ask for the bedroom") would reduce spoken confirmations without lowering safety. Wait until the confirmation fatigue is measured. | `crates/core/src/httptools.rs`, `crates/core/src/mcp.rs`, `docs/architecture/action-safety.md` |
| 6 | **Progress events as first-class** | Hermes `tool.started` and `message.interim` | The loop already speaks plans and progress; exposing the same events on a stream would let the portal show a turn live. Low priority for voice. | `crates/core/src/session.rs`, `crates/core/src/app/portal.rs` |
| 7 | **Scheduled model jobs, safely** | Hermes cron (fresh session, self-contained prompt, `[SILENT]`, no job creation by a cron run, an injection scan at creation) | ClawDot's scheduler fires reminders; "every morning tell me the weather" is a scheduled **turn**. The existing rule (a turn the person did not start can call only read-only tools, REQ-SEC-012) is a stronger default than Hermes's and should be kept. Larger than items 1 to 4; needs a spec. | `crates/core/src/sched.rs`, `crates/core/src/session.rs` |

Do not copy: free self-modification (Hermes lets the agent patch its own skills), terminal-backed tools on the speaker, a skill hub with remote install (REQ-EXT-001 and the pack trust model say data only and hash-pinned).

## 8. What I could not verify

* Whether Hermes's `/v1/chat/completions` honours a client `tools` array, can surface an approval, or hangs when an approval is needed.
* Any Hermes idle memory or CPU figure; the installation guide page was not read.
* Whether a Hermes cron job can deliver to a webhook (only chat targets, `local` and `bot-chat` were documented).
* The exact overlap of Hermes and opencode skills with the agentskills.io standard.
* opencode's behaviour under `opencode run` when a permission is `ask`, and its plugin mechanism.
* Which opencode configuration shape matches release 1.19.5 (the docs describe a newer one).
* The ACP version, transport options beyond stdio, and how a client should answer `session/request_permission` for a voice user.

## Spec impact (recommended)

1. Amend REQ-EXT-006 to describe the two-tool ClawDot MCP server (`announce`, `ask_user`), its endpoint-token auth, the `ask_user` time limit, and the rule that an answer is data and never a confirmation; add a threat-model entry first.
2. Add requirements for the draft-a-skill flow (item 1) and the skill import changes (item 2); add `read_skill` to the tool list.
3. Add a recipe for Hermes Agent as a brain: design 1 (written, untested; [recipe](../recipes/hermes-brain.md)), and design 2 after the MCP server exists.
4. ADR candidate: "an agent harness is a brain only with read-only tools, or behind ClawDot's confirmation".

## Confidence

Hermes API server, skills, memory, cron, security and ACP: **high** for what the pages state (primary documentation read in full for the claim), **low** for behaviour not stated (the gaps in section 8). opencode server, skills, permissions, MCP and rules: **high** for the pages read, **medium** overall (only the design is used here) because the docs describe a newer configuration shape than the latest release I confirmed. ACP: **medium** (one overview page). Nothing was run.
