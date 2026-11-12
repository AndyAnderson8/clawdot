# Code read and quality pass, 2026-11-12

A record of two days of work on the quality of `crates/core`: what was read line by line, what was found, what was fixed, what is still open and what needs the maintainer's decision. It follows the [architecture review of 2026-11-11](2026-11-11-architecture-review.md) and its [proving pass](2026-11-11-proving-pass.md).

All of the fixes below are on `dev`, each landed only after the whole workspace test suite passed, with CI green afterwards. **None of it has run on a device.** The spec stands at version 0.53.

## How the work was done

1. Fourteen batches of fixes written by subagents from review notes, then two review passes by subagents over the modules nobody had read (server side and client side).
2. A line-by-line read by the reviewer of about half of the core's non-test code, security-critical paths first. This found things the batches had missed, including the two most serious problems in the report. The lesson is recorded: tests and agent reports are not a substitute for reading the code.
3. The eight design problems that read turned up were put to the maintainer, decided, and fixed.
4. A second pass for architecture, code smells and device assumptions, with the rule that the core must not assume any one device.

### What was read by the reviewer

| Read in full or mostly | Read in part | Not read yet |
|---|---|---|
| The gate, the session, the model loop (`session.rs`, `session/gate.rs`, `gate/rounds.rs`) | Tool policy and skills (`extensions.rs`, about 60%) | Speech clients (`speech.rs`, `speechio.rs`, `speaker.rs`) |
| Turns and speakers (`turns.rs`, `speakers.rs`, `connects.rs`) | The MCP client (`mcp.rs`, about 55%) | Settings validation, `config.rs`, `seed.rs`, `cli.rs` |
| The HTTP server, routing, the Host guard (`http.rs`, `routes.rs`, `hostguard.rs`, `limit.rs`) | The updater (`update.rs`, about 60%) | Memory (`notes.rs`), `inbox/queue.rs`, `event.rs`, `reply.rs` |
| Core state, admin and sign-in (`app.rs`, `app/*.rs`, `admin.rs`) | The audit log (`audit.rs`, a quarter), `usage.rs` | ESPHome (`esphome/*`), hive (`hive/*`) |
| Storage and secrets (`store.rs`, `secrets.rs`, `app/persist.rs`) | The model reply parser (`llm.rs`, half) | The fast path, `skillfolder.rs`, `media.rs`, `vad.rs` |
| Announcements and the MCP server (`announce_api.rs`, `mcp_server.rs`) | The satellite session (`satellite.rs`: handshake, event loop, state) | The new `app_loop.rs` and `startup.rs` as they are now |
| HTTP tools and the outbound transport (`httptools.rs`, `transport.rs`) | The scheduler (`sched.rs`: types and outline) | Every other crate, and most of the portal's TypeScript |
| The voice dialog (`voice.rs`), webhook adapters and summaries (`inbox/adapter.rs`, `inbox/summary.rs`), endpoints (`inbox/endpoint.rs`) | The portal handlers (`ui.rs`, about 90%) | |

Several of the files in the first column have changed since they were read, by the fixes below. Only the parts of those changes judged risky were re-read (the gate split, the model loop move, the held-action release, the cross-site and Host checks).

## Problems found and fixed

### Security

| Problem | Fix |
|---|---|
| A web page in a browser on the home network could send writes to the speaker in open mode (cross-site request), and DNS rebinding could make a foreign page share the portal's origin. | Writes from another origin are refused; any `/api` request a browser labels cross-site is refused; the page and the routes that take no token answer only to an IP address, `localhost`, the machine's name or the configured public address. |
| `GET /api/models` took an address and an environment variable name from its query. A cross-site GET (an image tag) could make the device send that variable's value to the page's server. Found by reading; missed by every earlier pass. | The route takes nothing from the address. |
| The confirmation question hid content: any value over 60 characters was spoken as "a long value" while the exact text still ran. | Every argument is read in full; a read-back over 700 characters is refused, not shortened; a secret-looking value is spoken as "a hidden key" (REQ-SEC-011). |
| A tool lowered to read-only ran with the model's arguments, unconfirmed, after outside text entered the turn: a way to leak memories through a "fetch" tool. | A tool lowered below its baseline is held again once the turn is tainted (REQ-SEC-012). |
| MCP tool descriptions and schemas come from the server, reach the model on every request, and could change after approval. | Each tool is pinned by a fingerprint of what was approved; a new or changed tool is off until accepted in the portal; schema text is reduced. |
| A webhook could choose how it was announced ("From ...") and so impersonate the system or a person. | The speaker names the endpoint, by the spoken name the user sets; the sender's label is never spoken. |
| A re-created webhook inherited a deleted one's waiting replies, open events and rate limit. | Deleting a webhook drops them. |
| One shared failed-attempt budget let any host lock the administrator out. | Failures are counted per address, with a higher overall ceiling as a backstop (REQ-SEC-004). |
| The password could be changed without the current one. | The current password is required (REQ-CFG-024). |
| A provider that echoed the Authorization header put the key in an error message; the console log line was not redacted; a debug print of the HTTP tools included credentials. | Scrubbed and redacted in each place. |
| Switching off an MCP source whose name contains `__` left its tools on. A path value of `..` could turn a delete of one item into a delete of everything. | Server ownership comes from the client, not the name; `.` and `..` are refused. |
| The page's content-security policy allowed inline scripts. | The policy lists hashes of the page's own scripts and style, computed at build time. |
| The updater accepted version `10.2.0` for `0.2.0`; two requests could both start an install; a download could hold 190 MiB in memory. | Exact version match; check-and-claim in one step; a 16 MiB cap. |

### Correctness

| Problem | Fix |
|---|---|
| The daily action caps stopped counting when the audit log could not be written (a full disk). | The count comes before the write (REQ-SEC-016). |
| The spend cap only worked for providers that report a cost. | Cost is the reported figure, else tokens times the user's prices, else an estimate at deliberately high default prices (REQ-AST-011). |
| A failed save was reported as saved: a webhook got "accepted" and the user heard "timer set". | Waiters learn whether the write succeeded; HTTP answers 500 or 507; the speaker says a timer could not be saved. |
| Long replies in scripts without spaces, or with accents or emoji, could abort the turn (a cut inside a character). | The cut backs up to a character boundary. |
| Saying stop during a reply let paused music resume; music queued by a reply that could not be spoken played after a later one. | Both fixed in the satellite session. |
| Speech synthesis ran inside the satellite session's event loop and could outlast the dead-peer time; a satellite that stopped reading wedged its session. | Synthesis runs beside the loop; writes time out. |
| One failed accept stopped the web server or the satellite listener for good. | Logged and retried. |
| The diagnostics file said it held none of the user's words but carried timer and reminder labels. | Labels are left out. |
| A wall-clock step (network time arriving on a device with no battery clock) made a timer ring at the wrong time or be reported as missed, and could stretch the confirmation window, a session or the reset window. | Durations use a monotonic clock; calendar points use the wall clock. A countdown timer works before the clock is set (REQ-TMR-009). |
| Saved keys could be lost when the keys file was briefly unreadable; a power cut could bring back an old file; an update left a moment with no program on disk. | Read-modify-write is guarded; the directory is flushed after a rename; the update swap is one atomic step. |

### Structure

- Turns, realtime sessions, speech and MCP connects are tasks beside the loop; each speaker has its own conversation and held confirmation ([ADR-0021](../adr/0021-turns-as-tasks.md)).
- One table each for settings, HTTP routes and built-in tools, with the views derived from it.
- `main` went from about 2,000 lines to 41. The running program is an application type with one method per event (`app_loop.rs`), and start-up is one explicit sequence.
- The gate's and the realtime code's long positional parameter lists became context structs; no function in the core silences the too-many-arguments lint.
- Process-wide statics for secrets, speech, hive and the updater moved onto the core, and a test runs two cores in one process.
- Decisions made by matching spoken or error text were replaced by typed values.
- Every change writes only the file it touched, from a background writer: an acknowledgement went from about 27 KB across three files to 166 bytes.
- **The core names no device.** Hardware detection reads only what the operating system shows; a device's name, label and unseen parts come from a `device` section of the configuration file. The unused `board` setting is retired.
- The reply is spoken sentence by sentence as it is synthesised: for a four-sentence reply the first sound follows one synthesis, where it used to follow four.

### Documents

The README, the architecture overview, `PRIVACY.md`, `SECURITY.md`, `CONTRIBUTING.md` and the development-environment guide described the project as planned, not as built (two said no code existed). They were rewritten from the code. Seven architecture documents were corrected.

## Still open

### Needs the maintainer

1. Eight decision records are marked Proposed although they are built and relied on: 0006, 0013, 0016, 0017, 0018, 0019, 0020 and 0021.
2. Whether the portal must be reachable through a reverse proxy or by a name the device cannot recognise. Today such a name gets a 403 until the public address is set.
3. Whether a tool or MCP source with a credential may use plain `http://` to a host outside the home network.
4. Whether an https update feed may redirect to plain http on the device itself, and whether release files should carry a signed version line.
5. Whether a pending confirmation should survive when the realtime model calls a different tool. Today the new call cancels it.
6. The device strategy: about 16,000 lines of device daemon, wake word and satellite code have not been reviewed, pending the decision on which path is the product.
7. Whether to turn the full landing gate back on ([ADR-0018](../adr/0018-landing-gate-mode.md)).

### Known gaps in the code

- **Not proven on a device.** The new hardware probes, the clock behaviour after a real boot, sentence-by-sentence playback and the satellite changes have only run in tests and on a PC.
- **The ESPHome satellite client buffers a whole reply** before playing, so a speaker connected that way gains nothing from the sentence-by-sentence change until that client streams too.
- **The model's text is not streamed into speech.** The design is in [speech-latency.md](../architecture/speech-latency.md); it is not built.
- **`ask_user` in a turn that an event started.** The model can make the speaker ask a question and open the listening window on the strength of outside text. No source runs such turns through the loop today.
- **Skill tool scope in a realtime session.** A named skill's tool list does not narrow what a realtime session may call.
- **Inbox durations** (an announcement's time to live, snooze, reply expiry) still use the wall clock.
- **Pending writes are lost on SIGTERM** (a few milliseconds' worth). There is no signal handler; adding one needs a dependency.
- **Hive discovery** accepts replies from any source address; checking needs socket options the forbid-unsafe rule excludes.
- **Replies are keyed by endpoint name.** An endpoint id would remove that class of bug, at the cost of a protocol and file change.
- **Not checked against reality:** the default token prices, the token estimate, any provider's bill, the realtime, ElevenLabs and Deepgram services.
- **One end-to-end test** (`esphome_satellite`) timed out once under load and passed on every re-run.

### Smells left

- `runtime.rs` `apply_live` is a chain of `if changed(...)` blocks (about 90 lines); a handler per settings group would be cleaner. Its tests (about 700 lines) could move to their own file.
- `esphome/proto.rs` `decode` and `encode` are 219 and 135 lines; `skillfolder.rs` `read` is 112.
- The built-in webhook adapters are named in code (`Adapter::claude_code()`); they could be a list of embedded files.
- The portal keeps its own copy of every setting's label, kind and limits; the built page is committed, so portal branches conflict on it; the mock server restates the API by hand (a drift check now covers its routes).
- The version is still `0.0.0`.

## Numbers

| Measure | Value |
|---|---|
| Program size, armv7, all optional features | 3,501,076 bytes (limit 8 MiB) |
| Core unit tests, all features | about 890 |
| `main.rs` | 41 lines in `main` |
| Functions over 70 lines in the core | 26 (the longest is a protocol decoder) |
| Spec version | 0.53 |

## Second pass, later the same day

The reading continued through the rest of the core's own logic, the agent bridge and the satellite protocol. Everything in this section is on `dev`, each landed after the whole workspace test suite passed. As before, none of it has run on a device. The product spec stands at version 0.54 and the ESPHome spec at 0.3.

### What was read in this pass

- **Core:** the fast path (`fastpath.rs`), the dispatcher and built-in tools (`assistant.rs`, `assistant/tools.rs`), the inbox queue, events and reply queue (`inbox/queue.rs`, `event.rs`, `reply.rs`), configuration, flags, seeding and start-up (`config.rs`, `cli.rs`, `seed.rs`, `startup.rs`, `commands.rs`), the settings validators, the speaker task and speech set-up (`speaker.rs`, `speechio.rs`), `media.rs`, `say.rs`, `skillfolder.rs`, `stopwatch.rs`, `vad.rs`, the ESPHome configuration, client and file server (`esphome_cfg.rs`, `esphome/client.rs`, `esphome/media.rs`), and the hive's entry point.
- **New code from the three structural changes below**, before each landed: the speech providers and shared client, the application loop's files and the live-settings table, the streamed ESPHome reply.
- **Other crates:** the agent bridge (`crates/bridge`, all of it) and the satellite protocol's messages (`crates/protocol`).
- **Portal:** only a search for unsafe rendering of outside data. None was found: no raw HTML, links from the update feed are checked by the core, and the session token lives in the tab's session storage.

**Still not read:** the ESPHome wire format and Noise code (`esphome/proto.rs`, `frame.rs`, `noise.rs`), hive discovery and DNS, the unread parts of `mcp.rs`, `update.rs`, `audit.rs`, `llm.rs`, `satellite.rs` and `sched.rs`, the reference satellite, the device daemon, the wake-word runtime, the evaluation and end-to-end crates, and most of the portal's TypeScript.

### Problems found and fixed

| Problem | Fix |
|---|---|
| A spoken reply went to the event that **arrived** last, not the one **spoken** last. An urgent event is spoken before an older one and a snoozed one comes back later, so "tell it to carry on" could reach a different agent than the one just heard. | Each event records the order it was last spoken in, and a reply goes to the one spoken most recently (REQ-INB-008). |
| With no speaker connected (the speaker off, or its link down), every waiting agent event was still summarised by the model, at a cost, and marked as announced. Nobody heard it. | When a satellite is configured and nobody is at the terminal, an event waits until one is connected (REQ-INB-006). |
| The agent bridge remembered handled replies by number alone. A speaker whose data was reset numbers its replies from one again, so the bridge acknowledged new spoken instructions as already handled and never ran them. | A reply is known by its number and the time it was queued. |
| A `SKILL.md` at the top of an upload made every Markdown file beside it both a helper file and a second skill. | A file below a skill folder is that skill's helper only. |
| In the new streamed ESPHome reply, a sentence that arrived after a pause was sent to the device all at once, which a small device buffer may drop. | Pacing restarts from when the sentence arrives. |
| When a speaker spoke twice quickly and the second request could be answered at once, its answer came out before the answer to the first. | The finished turn's reply is delivered first. |
| The start-up warning still said timers do not ring before the clock is set; the `--name` help named the wrong default; two checks asked whether the speech provider was "the OpenAI style" by name where the question is whether it has its own address. | Corrected. |

### Structure

- **Speech is one file per provider** behind a `Provider` trait (`speech/openai.rs`, `elevenlabs.rs`, `deepgram.rs`); the provider is matched in one place, where it was matched in about seventeen. The shared HTTP client (`apiclient.rs`), the audio helpers (`audio.rs`) and the plain-http rule (`netpolicy.rs`) are their own modules, and the language model's list of models moved to the language-model client.
- **The application loop's behaviour is all on `App`**, in `app_loop/turns.rs`, `portal.rs` and `connections.rs`. `runtime.rs` went from 1,631 lines to 489. The live settings are a table of appliers, with a test that every live setting has exactly one. Ten large inline test modules moved to their own files. The built-in webhook adapters are a list of embedded files, so no product is named in that code.
- **ESPHome devices can play a reply from its first sentence.** A per-device setting, `reply`, is `whole` (the default, as before) or `streamed`. `streamed` rests on device behaviour the repository has no record of (a pause between sentences in API audio; a WAV with no declared length over a chunked response), so it is off until tried on a device.
- **The bridge uses its `Harness` trait**, which existed but was bypassed for the one concrete harness.
- One end-to-end test that timed out under load is now held open by a gate instead of a fixed delay.

### In progress

- **Binding a spoken instruction to its agent.** Today the read-back says "I will send this to the agent" without saying which, and the destination is looked up when the held call finally runs. An event announced between the read-back and the "yes" would receive the confirmed text. The change binds the call to the event at read-back time and names the origin.
- **Language as data.** Confirmation words, memory commands, the fast path's number words and grammar, sentence abbreviations and date words are English lists inside the functions that use them, and the fast path's intents are strings. They are moving into one language module and a typed intent, with no change in behaviour. A second language stays out of scope.

### Found and not yet changed

- **Endpointer tuning is fixed in code** (`vad.rs`: level, noise ratio, silence length). A microphone with a different gain cannot be tuned without rebuilding. This belongs in the device's preset or the satellite's configuration.
- **A timer that rings while no speaker is connected is not heard** and is not repeated; the line is dropped. Agent events now wait; rung timers do not.
- **Devices connected through ESPHome have no local password reset.** The reset by a long button press exists only in the native satellite protocol.
- **Start-up arguments are one mutable bag** (`cli::Args`) that holds the flags, the configuration file's sections and the settings resolved from them, and `seed.rs` maps configuration keys to setting keys by hand, so a new setting is added in three places.
- **`commands.rs` still names one adapter in code** when it prints the set-up snippet for a new endpoint; the snippet should come from the adapter's own file.
- **The realtime address is not checked against the plain-http rule when it is saved**, only when it connects, so a refused address is reported late.
- **The bridge reads lines and headers of any length** from the speaker, and does not parse an IPv6 address in `--url`.
- **The fast path's grammar names one product** ("tell claude to ..."). It becomes data with the language work; whether it stays is a wording decision.

### Decisions needed, added to the list above

8. **Try `reply: streamed` on the Echo Dot.** It needs the current build on the device and one line in its configuration. What to listen for is in [esphome-satellite.md](../architecture/esphome-satellite.md).
9. **The assumed name `clawdot.local`.** With no public address set and a listen address of `0.0.0.0`, printed snippets use `clawdot.local` (REQ-CFG-009, REQ-CFG-016) whatever the machine is called. The core already reads the machine's name for the Host check and could use it here.
10. **Languages.** After the language work the core can take a second language as a data file, but the spoken sentences are still English text in code. Whether English only is acceptable for the first release decides how far that goes.
11. **A rung timer with no speaker connected:** drop it (today), repeat it when a speaker returns, or sound it only if the speaker returns within a short time.

### Numbers after this pass

| Measure | Value |
|---|---|
| Core unit tests, all features | 926 |
| `runtime.rs` | 489 lines (was 1,631) |
| Functions over 70 lines in the core | 25 |
| Product spec | 0.54 |
| ESPHome spec | 0.3 |
