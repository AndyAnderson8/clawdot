# The agent loop

How one turn runs when the user asks for something that takes more than one step. The design is [agentic-scope.md](../design/agentic-scope.md) section 2; the requirements are REQ-AST-019 to REQ-AST-025, REQ-VOI-020, REQ-SEC-017 and REQ-CFG-044. The code is `session::run_turn_with` (the loop), `notes` (memory), `cancel` (the stop signal), `voice::Dialog` and `satellite::run_session` (audio). The measure of it is the `AGT` and `INJ` scenarios in [evals.md](evals.md).

## Where a turn runs (ADR-0021, REQ-AST-026, REQ-AST-027)

A turn does not run inside the main loop. The loop owns the state (the timers and built-in tools, the portal's tools, settings and skills, the usage files); a model turn is a task that gets what it needs and sends its results back:

```
 speaker says X ──► loop: fast path? ── yes ─► reply now
                          │ no
                          ▼
              Hub::submit ── the speaker's Conversation moved into a new task, with a TurnKit:
                          │     prompt + skills, limits, tool list, a read-only copy of the policy,
                          │     clones of the HTTP/MCP clients, a handle on the usage counters
                          ▼
   turn task:  run_turn_with (the unchanged turn) ─ HTTP/MCP tool ─► called by the task itself
                          │                      └ built-in tool ─► ToolReq ─► loop runs it on the assistant,
                          │                                          answers with the result and its effects
                          ▼
              Finished { conversation, reply, effects } ─► loop: put the conversation back, save the counters,
                                                            deliver the reply, start the speaker's next request
```

* **Conversations.** One per speaker (`speakers.rs`): a satellite connection (`sat:<random id>`, made per connection), the console, or any other conversing source. At most 8, each already bounded by `CONVERSATION_MAX_MESSAGES` and `CONVERSATION_MAX_BYTES` (currently 20 messages and 24,000 bytes); one idle for 600 s is dropped; a full set gives the place of the speaker idle longest to a newcomer. The held action, the recent actions, the stored results and the "forget my notes?" question live in the conversation, so they are per speaker: a yes from another speaker or another connection releases nothing. A speaker's second request while its first runs waits (at most 2); another speaker is not held up.
* **Snapshot.** A turn begins with the persona, skills, model, limits, tool list, which tools are off and the tiers the administrator chose, as they were at that moment. A portal change is applied by the loop at once and reaches the next turn; the turn in flight keeps its rules.
* **The gate.** `session/gate.rs` (`Turn`) holds the tier check, hold and release, audit, caps, limits and cancel. The model-turn loop and the realtime path both call it; realtime has no scripted backend and no extra model request.
* **Realtime and speech.** A realtime session is a task with its own conversation and the same link for built-in tools. Spoken lines (timers, portal phrases) and the agent-event announcements go through one speaker task, in order. None of them holds the loop.
* **Counters.** The daily request and spend counters are a handle on one small cell that every turn counts into, so the spend cap sees all of them at once; the loop alone saves them and changes the limits.
* **Where the code is.** `main.rs` reads the environment and the arguments, runs a one-shot command if one was asked for, and otherwise calls `startup::start` and runs the `App` it returns. `startup.rs` is the start-up sequence in its fixed order (settings, clock, core, management commands, `build_brain`, the bound socket, then the satellite, hive and update starts). `app_loop.rs` is the running application: `App` owns the `Brain`, the `Hub`, the settings in effect, the `Sources` (the channels it listens on) and the handle on the speaker task (`SpeakerHandle`), and has one method per event (`on_portal_job`, `on_console_line`, `on_ask`, `on_hub_event`, `on_realtime_job`, `on_mcp_connected`, `on_tick`) that the `select!` in `App::run` calls, so each can be tested in process. All of the loop's behaviour is on `App`, split by concern: `app_loop/turns.rs` (`dispatch`, `finish_turn`, `drain`, `deliver_confirmed`, `respond_inline`), `app_loop/portal.rs` (`on_portal_job` and the changes that need no network) and `app_loop/connections.rs` (`on_mcp_connected`). The one-shot `--voice-in` command builds an `App` with `App::one_shot` (no speaker task, so no tick) and calls the same methods. The state and its helpers are library modules: `runtime.rs` (`Brain`, the language model client, the status snapshot) with `runtime/live.rs` (`apply_live`: a table of appliers, one per live setting or settings group, which a test checks against `settings::FIELDS`), `speaker.rs` (the speaker task), `speechio.rs` (speech clients and the live voice and language handles), `cli.rs` (arguments and help), `seed.rs` (the configuration file and flags into the settings layers) `commands.rs` (`say`, `--reset-admin`, endpoint snippets, the console) and `env.rs` (the environment, read once in `main` and passed down).
* **Portal changes.** Changes to tools, skills, settings and timers are quick and local, so the loop applies them on the spot. Adding an MCP server is the one that uses the network, so it is split (`connects.rs`): the loop checks the request, a task connects (handshake and tool listing, at most 65 s) and sends the result back, and the loop adds the server and answers the portal. A source removed while it connects is cancelled, a result that arrives for a cancelled or replaced connection is dropped (each connection has a generation), a second request for the same name is refused, and shutdown aborts the tasks. Servers saved from earlier (and those in the configuration file) are declared at startup and connected the same way, in tasks started with the loop and run up to eight at a time: the speaker is usable at once, a source shows as connecting in the portal until it answers, and a dead one ends as failed (still saved, with its reason) after its own timeout. A turn that starts before a source is connected does not have its tools.

## One turn

```
user words ──► notes? ──► model round ──► calls ──► host checks ──► tool ──► result ──► next round
                │             ▲   │                    │                       │
                │             │   └ words: plan/progress, spoken now          └ long? summarised for speech
                └ saved/read by the host, no model      └ limit, deadline, cancel, tier, hold
```

Everything that decides the shape of the turn is the host's. The model proposes calls and words; it cannot raise a limit, release a held action, write a note, or end a turn without the host's account of it.

## Limits (REQ-AST-019)

| Limit | Default | Ceiling | What the user hears when it ends the turn |
|---|---|---|---|
| Model rounds | 8 | 20 | "I stopped after 8 steps because I reached my limit of 8 rounds of thinking. Here is where I got to: ..." |
| Tool calls | 12 | 20 | "... my limit of 12 tool calls for one request. ... I did not get to: ..." |
| Wall clock | 60 s | 300 s | "... because I ran out of time. My limit is 60 seconds ..." |
| One tool call | 45 s | 120 s | an error result the model sees ("did not answer within 45 seconds, may or may not have done anything") |

They are set in the `agent` section of the configuration file ([configuration.md](configuration.md)); a larger value is clamped to the ceiling and zero is refused. The deadline is also the upper bound of a tool call, so the last call cannot run past it. The account is built from the host's own record of steps, never from tool output: the tool names (made into plain words), whether each worked, and the calls that were not started. Ending at a limit says "Tell me to carry on if you want me to continue": the next user turn continues from the same history. There are no background tasks yet (agentic-scope P2-3), so the work is not kept running.

## Speech while working (REQ-AST-020)

`TurnCtl.speak` is a channel; each phrase is sent the moment the host has it and also listed in `TurnResult.early`. A caller with no channel (the console, a recorded utterance) gets the phrases in front of the answer.

1. **Plan.** In the first round, the first sentence of the model's words alongside its calls (at most 200 characters). If the model said nothing and made three or more calls, the host writes "I'll do github list runs, then github list prs, then ...". The default persona asks the model for the plan.
2. **Remarks.** Later rounds: the first sentence of the model's words, at most once per 10 seconds.
3. **Progress.** Before a call to a tool the host expects to be slow (`ToolHost::expected_slow`: every HTTP tool and MCP tool), "Checking ..." for a read and "Working on ..." for anything else, built from the tool name, at most once per 10 seconds and not in a round that already spoke. Tool output is never spoken this way.

`agent.plan_speech` and `agent.progress_speech` turn them off. Token-by-token streaming from the provider is not built: the speech is sentence-level, as each model round returns.

On the satellite, phrases are played one at a time as they arrive. While one plays the dialog stays "thinking", so the follow-up window does not open and a wake word is ignored (half-duplex, REQ-VOI-012); the finished answer waits until the last phrase ends.

## Questions (REQ-AST-021)

The host adds an `ask_user` tool to every request. A call to it ends the turn with the model's question (cleaned, at most 300 characters, ending with a question mark); any other calls in the same reply are skipped; it uses no tool budget. The conversation keeps everything, so the next turn is the answer. After a question, and after the host's own yes-or-no for a held action, the follow-up window is 15 seconds instead of 8 (`follow_up_question_secs`, never shorter than `follow_up_secs`, and still off if follow-ups are off). "No" or "never mind" drops the question without a model call. When a call fails because a required argument is missing, the error the model reads says to ask instead of guessing.

## Cancel (REQ-AST-022, REQ-VOI-020)

The satellite session holds one `CancelToken` per running turn. The turn looks at it at the top of each round, before each call, while it waits for the model (the request is abandoned) and while a read-only or reversible call runs (the call is dropped). A costly or destructive call is allowed to finish, because it cannot be recalled, and the account says "The request to ... was already sent". A held action is discarded and audited as `cancelled`; so is a call dropped in flight; every cancelled turn also writes one `turn` record. The history stays well formed: every call has a result ("Not run: the user cancelled.").

How the token gets set while the assistant is working:

- **The action button** cancels at once (no speech recognition).
- **The wake word, then a stop phrase.** A wake word heard while a turn is running and nothing is playing opens a short capture (the indicator shows listening, the microphone is streamed, at most 4 s of silence). The audio is transcribed, and only the stop phrases cancel (`session::is_cancel`: stop, cancel, never mind, forget it, and a few like them). Anything else is dropped. This costs one speech-to-text call per attempt and is the price of having no local keyword spotter yet.
- **Not while speaking.** While the assistant plays speech, tones or music (including progress phrases), the wake word is ignored and the microphone is not processed (REQ-VOI-012). The button still works.
- **Not on ESPHome satellites**, whose protocol refuses a new voice run while the assistant is working (REQ-ESP-007).

What remains for barge-in (REQ-VOI-005): hearing "stop" over the assistant's own voice needs echo cancellation (B3), a local keyword check would remove the speech-to-text call, and a `clawdot.cancel-turn` wire message ([spec 0006](../specs/0006-device-layer-contract.md) section 8) would let a device that stops its own playback tell the core.

## Memory (REQ-AST-023, REQ-SEC-017)

- **History:** as before, bounded by `CONVERSATION_MAX_MESSAGES`, `CONVERSATION_MAX_BYTES` and `CONVERSATION_IDLE_SECS` (currently 20 messages, 24,000 bytes, 600 s idle).
- **Recent actions:** the last 10 calls of the conversation (id, tool, tier, a short redacted target, how it ended) are put at the end of the system prompt as host-written data. "Undo that" and "do it again" resolve from them. They expire with the history.
- **Notes:** off unless `agent.notes` is true. At most 500 notes of 300 characters and 64 KB in all (REQ-MEM-001), in `memory.json` in the data directory (0600, atomic writes, bounds re-applied on load). Only the host writes them, from the user's own words in a turn the user started when the sentence begins with "remember that ...", "make a note that ...", "take a note ..." and a few more (`notes::parse`); a model reply, a tool result, an event or a summary cannot. The model is not offered a tool that writes them. Text that the audit redaction would change (keys, passwords, tokens) is refused. They are listed by "what do you remember", forgotten by "forget note 2" or "forget the note about the NAS" (one match only), and "forget everything" asks first. The prompt never carries more than 1,600 characters of them (REQ-MEM-002): a memory of 20 notes or fewer that fits is shown whole, a larger one is ranked for the turn (BM25-style word overlap with the utterance and the turn before it, plus a small recency bonus; no embeddings, REQ-MEM-003) and the best 12 or fewer are shown with their true numbers, and the read-only `recall` tool (REQ-MEM-005) looks up the rest. The model sees them as the user's own labelled data after the persona; they cannot change tiers, confirmation or allow-lists. `GET /api/notes` (with `?q=` to search, REQ-MEM-004), `DELETE /api/notes/{id}` and `DELETE /api/notes` do the same for the administrator.

## Long results (REQ-AST-024)

A successful result over 400 characters (`agent.summarize_over_chars`, 0 turns it off) is stored (the last 5, 2,000 characters each), summarised by `llm.summary_model` (the primary model if unset; no tools; the text fenced and labelled as data; clamped to two sentences) and replaced in the model's context by the summary, a note that it came from untrusted text, and an id. The model can call `result_details` with the id to read the fenced full text. If the summary model fails, the summary is "<tool> returned a long result (N characters)." The audit record of the call keeps up to 2,000 characters of the result, redacted line by line, in `detail` (shown by `GET /api/audit`), so the portal has the full text.

## Errors (REQ-AST-025)

A failed or timed-out call is an error result for the model, who may try one alternative. Two failed calls in a row end the turn with: which tool, how many times, "didn't work" or "did not answer in time", for a costly tool "I can't tell whether it went through, so check before you try again", and "You can ask me to try again, or ask me to do it another way." An empty model reply after a failure gets the same account instead of "Done.". A success in between resets the count.

## Time: durations and calendar points (REQ-TMR-002, REQ-TMR-003, REQ-SEC-011)

A device with no battery clock boots with a stale time and has it corrected by the network later, by minutes or hours, in one step. So the core reads two clocks through one trait (`sched::Clock`) and never mixes them.

- `Clock::monotonic_secs()` counts seconds since an arbitrary origin, never goes backwards, and does not notice a step of the wall clock. It is `std::time::Instant` behind the system clock. Everything that is a **duration** uses it. A reading means nothing after a restart, and is compared only with another reading from the same process.
- `Clock::now()` is the wall clock (UTC unix seconds). Everything that is a **point on the calendar** uses it.

| Thing | Kind | Clock |
|---|---|---|
| Countdown timer, snooze, reminder "in ten minutes" | duration | monotonic |
| Confirmation window of a held action (120 s), the clarifying-question window, the forget-all confirmation | duration | monotonic |
| Conversation idle expiry (600 s), per-speaker idle eviction | duration | monotonic |
| Administrator session (12 h), password-reset window (300 s) and its cooldown (60 s) | duration | monotonic |
| Rate-limit buckets (hook, announce, failed sign-in attempts) | duration | monotonic |
| Turn deadline and the spacing of progress phrases | duration | monotonic |
| Alarm, reminder at a date and time, repeating alarm | calendar | wall |
| Quiet hours, the day number for usage counters and caps | calendar | wall |
| Stopwatch (REQ-TMR-011: time while the speaker was off counts) | calendar | wall |
| Audit record time, event time shown to people, notes' dates | calendar | wall |
| Announcement `ttl_secs`, inbox snooze, reply expiry, handled-event pruning | calendar (kept) | wall |

The last row is a decision, not an oversight: those records are stored in files and compared across restarts, and their worst failure is a notification dropped or spoken late, with no security property. Moving them to the monotonic clock needs the same save-and-reconcile scheme the timers have (below) for the inbox and reply files; it is open work, not done.

**A countdown across a restart.** The scheduler keeps a countdown as a monotonic due time in memory. In the file it is `remaining_secs` (what was left at the save) and `saved_at` (the wall time of the save, only if the wall clock was valid then). Both are recomputed at **every** save, because the file is rewritten only when the set of items changes. At load the time the program was off is taken off with the wall clock: `off = max(0, wall_now - saved_at)`, and nothing when the wall clock is not valid. A countdown that ran out meanwhile comes back late by the difference, and is announced as missed when over five minutes late (`MISSED_GRACE_SECS`). The older form, an absolute `fire_at` with no `remaining_secs`, loads as `fire_at - wall_now` seconds left. `fire_at` is still written (refreshed at each save) so an older build can read the file.

**What a person is shown** (the portal's "rings at 14:05", `seconds_left`, "how much time is left") is derived at read time from the seconds left, so it stays right after a step of the clock.

**The wall clock gate.** An alarm or a reminder at a time of day does not ring until the wall clock is valid (REQ-TMR-006) and cannot be set before that (REQ-TMR-009). A countdown needs no calendar, so it can be set, listed and rung on a device that has never had the time.

**Suspend.** `Instant` is `CLOCK_MONOTONIC` on Linux and does not advance while the machine is suspended, so a timer on a suspended laptop would lose the time asleep. The devices this software targets do not suspend. `CLOCK_BOOTTIME` would fix it but needs a `libc` call or a new dependency, so it is not done.

**One residual error.** The time the program was off is counted with the wall clock at load. If the clock is stale at that moment (not yet corrected by the network), the downtime is under-counted. That is the stale-boot problem of REQ-BRD-010 and is not closed here.

## What is not here

Background tasks with ids (P2-3), rules (P2-1), token streaming, a local stop-word spotter and barge-in, and per-tool caps. The limits, the speech switches, the notes switch and the daily caps can be changed in the portal (Settings, "Agent settings", REQ-CFG-062) and apply to the next turn; notes are managed in the Notes tab.
