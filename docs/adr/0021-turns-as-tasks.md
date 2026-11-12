# ADR-0021: Turns run as tasks beside the main loop

- **Status:** Proposed
- **Date:** 2026-11-11
- **Deciders:** Maintainer

## Context

The main loop of `crates/core/src/main.rs` was one `select!` that awaited each piece of slow work inline: a model turn (`brain.respond`), a change to tools or skills from the portal, a spoken announcement and a realtime voice session. While one ran, nothing else did. A timer that fell due during a 20 second turn rang late, a portal change waited, a second speaker waited, and a realtime session (up to 300 seconds) stopped the whole assistant. The architecture review (finding C1) asked for turns to run as their own tasks while the state keeps one owner. The same review found that one `Conversation` served every speaker (C2), so a "yes" in one room could answer the question asked in another, and that the realtime path ran its model's function calls through `run_turn_with` with a scripted fake backend, which counted extra model requests (C9).

Forces:

- The state that matters is mutable and not thread-safe by design: the timers and the built-in tools (`Assistant`), the portal's tools, connections and skills (`Extensions`), the settings, the usage files. Putting `Arc<Mutex<..>>` around the whole brain would make every await a hazard and every safety property depend on lock discipline.
- The safety properties of a turn (tiers, host-held confirmation, audit, limits, cancel, spend cap, quiet hours) must not change, and must not be re-implemented for a second path.
- Echo Dot Gen 2 limits (spec 0002): more tasks must not mean more memory in the default configuration.

## Decision

1. **The loop is the only owner of state.** It keeps the assistant, the extensions, the settings and the usage files, and it applies every change to them. Nothing else holds a lock on them.
2. **A model turn is a task that owns what it needs and sends messages back.** When a speaker says something the fast path cannot answer, the loop builds a `TurnKit`: a snapshot of the system prompt (persona and enabled skills), the turn limits and model names, the built-in tool list, a read-only copy of the policy (`Extensions::snapshot`: what is off, the chosen tiers, the skills' tool lists), and shared handles on the HTTP and MCP clients. It spawns `turns::run_one` with the speaker's own `Conversation` moved in. The task runs `run_turn_with` unchanged, with the host stack `WithPolicy<WithMcp<WithHttp<BuiltinLink>>>`.
3. **Who runs which tool call.** HTTP and MCP tools are called by the task itself, through its clones of the clients (`Arc<HttpTools>`, `Arc<McpClient>`; a portal change makes the loop a private copy with `Arc::make_mut`, and a running turn keeps what it began with). A built-in tool (timers, alarms, the stopwatch, music, the inbox) changes state the loop owns, so the task sends the call to the loop as a `ToolReq` and awaits the answer; the loop runs it on the assistant, takes the effects it produced and answers with both. The pure rules (tier, wording, whether results are untrusted) are free functions, so the task asks them without the assistant.
4. **A change made while a turn runs reaches the next turn.** A turn's snapshot does not move under it. The portal's settings, persona, model, tools and tiers are applied by the loop at once, as before; the turn in flight finishes with the rules it began with. This is the only visible difference to the old loop, and it is the safer one: a turn never sees a tier change halfway through.
5. **One conversation per speaker.** `speakers::Speakers` holds at most 8 conversations, each already bounded by `CONVERSATION_MAX_MESSAGES` and `CONVERSATION_MAX_BYTES` (currently 20 messages and 24,000 bytes), so the set stays under a megabyte. A speaker is a satellite connection (`sat:<random id>`, new for each connection so one speaker cannot claim another's), the console (`console`), or any other source that converses. The conversation, with its held action, recent actions and stored results, is checked out to the turn task and checked in at the end. A second request from a speaker whose turn is running waits (at most 2), in order; another speaker is not held up. A conversation idle for 600 seconds is dropped; when the set is full the one idle longest gives way, and if every place has a running turn the newcomer is told to ask again.
6. **The gate is one component.** The tier check, hold and release, audit, caps, limits and cancel moved out of `run_turn_from` into `session/gate.rs` (`Turn`). The model-turn loop and the realtime path both call it: realtime passes its model's function calls to `run_proposed_calls` and a spoken yes or no to `answer_held_action`, with no scripted backend and no model request. A realtime session now counts as one request; before, every batch of calls and every spoken yes added more.
7. **A realtime session is a task too.** It owns a fresh conversation and the same kind of snapshot and tool link; its built-in tool calls are served by the loop. It ends with a message to the loop, which saves the counters and updates the status.
8. **Speaking is a task too.** Spoken lines (timers, portal phrases) and the poll for agent events are handed to one speaker task that says them in order, so a slow voice or a summary from the model does not hold the loop. The status page reads whether a speaker is free from the last poll.
9. **The one shared cell.** The daily request and spend counters (`UsageTracker`) are a handle on one `Mutex`-guarded cell, because the spend cap must see every concurrent turn's cost the moment it is added; a snapshot would let two turns each pass the check and together exceed the cap. The lock covers a few arithmetic operations and is never held across an await. The loop still alone saves the counters and changes the limits.

## Consequences

**Positive:**
- Timers ring on time during a turn or a realtime session; portal changes apply at once; one room's slow request does not stop another room.
- A held confirmation belongs to its speaker. A yes from another speaker, or from a new connection, releases nothing.
- Realtime and the model turn share the gate; no second implementation to keep in step. Request counts are right.
- The loop code is shorter: `brain.respond` and `run_realtime` (about 150 lines of inline awaiting) became `dispatch`, `finish_turn` and `Hub`.

**Negative and limits:**
- Changes to tools, connections and skills in the portal are applied by the loop, because they are quick and local, with one exception that is now handled like a turn: connecting a new MCP server (a handshake and a tool listing over the network). The loop checks the request and a task makes the connection (`connects.rs`, `McpClient::prepare_server`, `PendingServer::load`); the task sends the tools back and the loop alone adds the server (`Extensions::mcp_commit`) and answers the portal. The extensions are never handed out. Each connection has a generation; a result whose connection was cancelled (the source was removed while connecting) or replaced is dropped; a second request for a name that is connecting is refused; a connection takes at most 65 seconds; on shutdown the tasks are aborted and the waiting requests answered. The portal waits up to 70 seconds for that one answer instead of 5. Saved and configured servers use the same mechanism at startup: `McpClient::declare` checks the definitions (a bad one still stops startup) and lists every server as connecting, then the listener and the loop start and each server connects in a task (`Connects::start_saved`, at most eight at a time, each bounded by its own timeout). The loop applies each result with `McpClient::settle`: the tools appear when the server answers, and a server that fails stays saved and is listed with its error. So one dead server delays nothing, and a turn that begins before a source has connected simply does not have its tools yet. Removing a saved source that is still connecting stops its task. The portal listing says `connecting: true` until the source has answered or failed. With `--voice-in` (one utterance, no loop) the sources are awaited first.
- A turn that is running sees the portal as it was when it began. A tool switched off a moment ago can still be called by a turn already running; the gate's checks at call time still apply to the snapshot, not to the new setting.
- If a turn task dies without answering (a panic), the speaker's conversation starts afresh and the satellite is told it cannot answer; the held action, if any, is lost, which fails safe.
- The counters have a lock, which is a departure from "no shared state"; it is small, local and the reason is above.
- One more task per turn costs the task's stack and a snapshot of the tool list; tool specs for the built-in tools are a few kilobytes. The default configuration has one speaker and no realtime, so it runs one turn at a time as before.

## Alternatives considered

- **`Arc<Mutex<Brain>>`.** Rejected: the lock would be held across every model call or the turn would need to re-lock between steps, and every tool, setting and timer path becomes lock-order reasoning.
- **Keep one task but poll with timeouts** (run the turn in the loop with `select!` against a timer tick inside it). Rejected: the same nesting, and every await inside the turn needs the loop's other arms.
- **A copy of the usage counters for each turn, merged afterwards.** Rejected for the spend cap, above. Sufficient for nothing else here.
- **Stable satellite device id as the speaker.** Rejected: the id comes from the satellite's hello and, without a token, anyone could claim it and answer another speaker's held action. The connection's own random id cannot be claimed.
