# Open issues, 2026-11-13

Everything known to be wrong, unfinished, unproven or undecided on `dev` after the code read and the fix batches of 11 to 13 November. It is written so that someone who was not there (a person or a coding agent) can pick an item and do it. The history of what was found and fixed is in [the code read and quality pass](2026-11-12-code-read-and-quality-pass.md); this file is only what is left.

State of `dev` when this was written: product spec 0.60, ESPHome spec 0.3, the whole workspace test suite and CI passing. **Nothing from these days has run on a device.**

## How to work on an item

- Read `AGENTS.md` first. Branch from `dev`, Conventional Commits with a body that says why, no AI attribution, land only with `scripts/land.sh`.
- Run every cargo command through `scripts/cargo.sh` (it builds in WSL). Before landing, `scripts/cargo.sh test --workspace --locked --all-features` must pass.
- Behaviour comes from `docs/specs/0001-product-requirements.md`. When a row changes, bump its version by 0.01 and add a changelog row. Two branches that both bump it will conflict on the version line and the changelog; renumber the later one.
- The core must not assume one device or, where it can be avoided, one language. What differs is data in the configuration file or in `crates/core/src/lang/`.
- Each item below says where the code is. Read that code before changing it; several items say "decide first", and those need the maintainer.

Sizes: **S** is under an hour, **M** is a few hours, **L** is a day or more.

## 1. Bugs and gaps in behaviour

| # | Problem | Where | What to do | Size |
|---|---|---|---|---|
| B1 | The portal's reminder form can only make a reminder at a date and time. A reminder after a delay ("in ten minutes") can be set by voice and by the model's tool, not in the portal. The portal API also refuses every reminder while the clock is not set, including one after a delay, which the scheduler would run as a countdown. | `crates/core/src/assistant/schedule_api.rs` (`add_item`), `portal/src/tabs/Reminders.tsx`, `portal/src/forms.ts` | Accept `seconds` for kind `reminder` in the API (use `Scheduler::set_reminder_in`, allowed without the clock), add an "after a delay / at a time" choice to the form, rebuild `ui.html`, add portal and API tests. REQ-TMR-009. | M |
| B2 | The `no_speech_ms` listening value only applies to the first listen after the wake word. After a reply the follow-up window length replaces it, and while the assistant is working a fixed 4 seconds is used, so those two cannot be tuned for a device. | `crates/core/src/voice.rs` (where the endpointer is rebuilt), `crates/core/src/config.rs` (`ListeningConfig`) | Decide whether the two other cases need their own keys or should scale from the tuned values; document whichever it is in `docs/architecture/configuration.md`. REQ-VOI-025. | S |
| B3 | An IPv6 link-local address (`fe80::`) is not treated as private, though an IPv4 link-local one is. Plain `http://` to a device reached by its link-local IPv6 address is refused, and hive discovery drops packets from such a source. | `crates/core/src/netpolicy.rs` (`is_private_ip`) | Treat `fe80::/10` as private; add it to the tests. Check the callers (language model, speech, realtime, hive) still read correctly. REQ-SEC-001. | S |
| B4 | Hive discovery drops a peer whose advertised address differs from the packet's source address. A machine with several addresses can advertise one and send from another, and would be dropped. The interface a packet arrived on, and its TTL, are still not checked. | `crates/core/src/hive/discovery.rs`, `docs/guides/hive.md` | Decide first: keep the strict rule (safer, may miss a peer) or accept any private source and keep the source address as the peer's address. The interface and TTL checks need socket options that the forbid-unsafe rule excludes. Hive is off by default and is discovery only. | S |
| B5 | In a normal turn, when the model makes several calls at once and one is held for confirmation, every later call in that batch is skipped, read-only ones included. In a realtime session a read-only call still runs while an action is held. The two paths differ. | `crates/core/src/session/gate.rs` (the batch loop and `sort_by_tier`), `crates/core/src/realtime.rs` | Decide first which rule is wanted, then make both paths follow it and say so in `docs/architecture/action-safety.md`. REQ-SEC-011. | M |
| B6 | `ask_user` is still offered to the model in a turn that outside text started, and is then refused when called. The model wastes a round on it. | `crates/core/src/session/gate.rs`, where the tool list for a turn is built | Leave `ask_user` out of the tool list of a turn the user did not start. Keep the refusal at the gate as the backstop. REQ-SEC-012. | S |
| B7 | A device connected through ESPHome has no way to open the administrator password reset from the device. The long button press exists only in the native satellite protocol (`clawdot.admin-reset`). | `crates/core/src/esphome/client.rs`, `crates/protocol/src/message.rs`, REQ-SAT-020 | Decide first. ESPHome has no such event. Options: document `clawdot --reset-admin` as the route for such devices (smallest), or map some device action the ESPHome API does report. | S to M |
| B8 | With a terminal echoing speech and a satellite to wait for, a rung timer's line is printed once when it rings and not again when it is finally spoken, so the terminal does not show when the speaker said it or that it was said as missed. | `crates/core/src/speaker.rs` (`Say::Line`, `speak_waiting`) | Print the line that is actually spoken, at the time it is spoken. | S |
| B9 | The configuration file still accepts `llm.fallback_model`, `llm.summary_model` and `limits.max_requests_per_day` and ignores them without a word. A person who sets a fallback model believes they have one. | `crates/core/src/config.rs`, `crates/core/src/seed.rs` (`NOT_SETTINGS`), `crates/core/src/startup.rs` (warnings) | Print a start-up warning naming each retired key that is present. See also S7, which decides whether the fallback and summary models come back. | S |
| B10 | Replies to agents are keyed by endpoint name. A deleted and re-created endpoint with the same name is a different endpoint with the same key; the delete path now drops what the old one had queued, but any new code that forgets to do so reopens the bug. | `crates/core/src/inbox/reply.rs`, `crates/protocol/src/bridge.rs`, `crates/core/src/inbox/endpoint.rs` | Give an endpoint a stable id and key replies, events and rate limits by it. This changes saved files and the bridge's wire format (`Reply.endpoint`), so it needs a migration and a protocol note. | L |
| B11 | The stopwatch reads the wall clock. A clock that steps forward after the stopwatch started (network time arriving) cannot be told from time passing. This is documented as a limit in REQ-TMR-012. | `crates/core/src/stopwatch.rs`, `crates/core/src/assistant/stopwatch.rs` | Run it on the monotonic clock while the program is up and use the wall clock only across a restart, as countdowns do (`crates/core/src/sched.rs` shows the pattern). | M |
| B12 | A 32-bit ARM machine that is not ARMv7 (an ARMv6 board) is offered the `armv7` update file. The install is refused safely, because the new program is run with `--version` first, but the portal offers an update that can never install. | `crates/core/src/update.rs` (`asset_name`) | Report "no file for this machine" when the running program was not built for the offered target, or name the asset from the build target instead of `std::env::consts::ARCH`. | S |

## 2. Work that was started and not finished

| # | What | State | What to do |
|---|---|---|---|
| U1 | **A drift check between the portal's coding-agent set-up text and the adapter file.** `clawdot --create-endpoint` prints the hook JSON and bridge line from `crates/core/adapters/claude-code.json` (`setup`). The portal builds the same text by hand in `portal/src/snippets.ts`. Nothing ties the two together. | A test was written for `portal/test/unit/snippets.test.ts` that reads the adapter file, fills the placeholders and compares it with what `snippets()` returns. It passed, and so did the type check, but the commit hook reported `check: FAILED` after the portal tests and the `ui.html` check, and the cause was not found (the hook's lint or format step is the likely one; the test used `await import("node:fs")` inside the test body). It was not committed. | Re-create the test with a normal top-level `import { readFileSync } from "node:fs"`, run `npm run lint` and `npm run format:check` in `portal/` to see what the hook objects to, and land it. Better still, remove the duplication: serve each adapter's `setup` lines from the API and have the page print them. |
| U2 | **Streaming the model's text into speech** (first sound before the model has finished). | Designed in [speech-latency.md](../architecture/speech-latency.md), stage 2. Not built. The reply is spoken sentence by sentence only after the whole reply text exists. | Build it as the design says. This is a feature, not a fix; it goes through the roadmap. |
| U3 | **The branch `docs/agent-harness-brains`** (a research note on agent harnesses as an optional brain, and an untested recipe for using Hermes Agent as the model). | Unlanded, several days behind `dev`. | Merge `dev` into it, check the recipe still matches the settings, land it or delete it. |
| U4 | **The guide for the MCP server** ([hermes-mcp.md](../guides/hermes-mcp.md)) says it was written from Hermes Agent's documentation and never run against a live one. | Untested. | Run it against a real MCP client and correct the configuration example. |
| U5 | **Spoken sentences are still English text in code.** The word lists the logic acts on moved to `crates/core/src/lang/`; the sentences the speaker says did not. | The header of `crates/core/src/lang.rs` lists the files that still hold sentences. The fast path's grammar also names one product ("tell claude to ...", "what did claude say"), now as data in `lang/en.rs`. | Only if a second language is wanted (decision D6). The grammar wording can be made neutral at any time. |

## 3. Structure and code smells

None of these changes behaviour. Each should land with the existing tests passing unchanged.

| # | Smell | Where | Suggested change | Size |
|---|---|---|---|---|
| S1 | Two protocol functions are far too long: `decode` is 219 lines and `encode` 135. | `crates/core/src/esphome/proto.rs` | One function per message type, with `decode` and `encode` as a table or a match that calls them. | M |
| S2 | Other functions over 85 lines: `skillfolder::read` (114), `transport::exchange` (107), `fastpath::parse_alternatives` (104), `mcp::rpc` (93), `startup::start` (93), `gate::sort_by_tier` (89), `esphome_cfg::validate` (87), `seed::resolve` (87), `gate::execute` (86). There are 25 functions over 70 lines in the core. | as named | Split each by step. The count is of functions whose body runs over 70 lines in non-test code. | M |
| S3 | Four source files are over 1,300 lines: `ui.rs` (1,730), `satellite.rs` (1,728), `session.rs` (1,486), `fastpath.rs` (1,336). Two test files are larger still (`session/tests.rs`, `http/tests.rs`). | `crates/core/src/` | Split by concern: `ui.rs` by portal area, `satellite.rs` into handshake, event loop and playback, `fastpath.rs` into the grammar engine and the value parsers. | L |
| S4 | `runtime.rs` has two `start_realtime` bodies, one per cargo feature, that differ only in the `cfg` line. | `crates/core/src/runtime.rs` | One function, with the difference inside it. | S |
| S5 | The doc comment of `settings::Group::Own` describes a role it no longer has: the live-settings table decides which applier runs. | `crates/core/src/settings.rs`, `crates/core/src/runtime/live.rs` | Correct the comment, or remove the variant if nothing needs it. | S |
| S6 | The portal keeps its own copy of every setting's label, kind and limits, apart from `settings::FIELDS`. | `portal/src/tabs/Settings.tsx`, `portal/src/tabs/SettingsPage.tsx`, `crates/core/src/settings.rs` | Serve the table's metadata (kind, range, whether it is live) from the API and have the page use it, keeping only the wording in the page; or add a drift test like the one for routes. | M |
| S7 | `TurnConfig.fallback_model` and `summary_model` are still read by the model loop and the summary code, but nothing sets them from configuration any more (start-up passes `None`). It is dead plumbing, or a feature that was switched off by accident. | `crates/core/src/runtime.rs` (`agent_turn_config`, `build_llm`), `crates/core/src/session/gate/rounds.rs`, `crates/core/src/speaker.rs`, `crates/core/src/session.rs` | Decide first (D7): wire the two configuration keys back in, or remove the fields and the code that reads them. | S to M |
| S8 | The built portal page (`crates/core/src/ui.html`, about 200 KB) is committed, so two branches that touch the portal always conflict on it. | `portal/build.mjs`, `crates/core/build.rs` | Build it in `build.rs` or in CI and stop committing it; or keep committing it and add a merge driver that rebuilds. Either needs Node at build time to be acceptable. | M |
| S9 | The browser tests' fake device restates the HTTP API by hand. A drift check covers its routes, not its response shapes. | `portal/test/browser/mock-api.ts`, `docs/api/openapi.yaml` | Generate the fake's responses from the API description, or check shapes as well as routes. | M |
| S10 | Speech client leftovers: the shared client hard-codes the speech key's name (`Client::current_key` reads `SPEECH_KEY`), so it is not as provider-neutral as its module says; the model list reports errors as "the speech server"; `ui.rs` still decides "is this OpenAI" from the address and holds OpenAI's voice list outside the provider file; a reply is buffered up to 9 MiB (`MAX_RESPONSE`) in memory on a device with a few megabytes to spare; two resamplers exist that differ by one bit at 11,025 Hz. | `crates/core/src/apiclient.rs`, `crates/core/src/ui.rs`, `crates/core/src/speech/openai.rs`, `crates/core/src/audio.rs` | Pass the key name in; give the model list its own `Wire` title; move the OpenAI voice list into the provider; lower the cap to what the longest sentence can need, or stream; merge the resamplers once the output difference is accepted. | M |
| S11 | Duplicates outside the core: `crates/eval/src/run.rs` has its own sentence splitter; `crates/device/src/button.rs` has its own console quit words. | as named | Use the core's `SentenceSplitter` and the language's quit words. | S |
| S12 | The version is `0.0.0` everywhere. | `Cargo.toml`, `portal/package.json` | Set by the first release (`scripts/release.sh prepare`); the maintainer publishes. | S |
| S13 | The Noise transport keys and the pre-shared key are plain arrays that are not wiped when dropped. | `crates/core/src/esphome/noise.rs`, `crates/core/src/esphome/client.rs` | Wrap them in `zeroize::Zeroizing` (the crate is already a dependency of the TLS stack; check before adding it directly). | S |

## 4. Not proven

These are built and pass their tests. Nobody has seen them work for real.

1. **On the Echo Dot.** The device still runs a build from before all of this. Not seen on it: the hardware detection that no longer names the Dot (the Dot shows "Unknown device" until its configuration has a `device` section; an example is `examples/devices/echo-dot2.json`), a boot with no clock followed by network time, sentence-by-sentence replies, the satellite session changes, the listening values (`satellite.listening`, `esphome.devices[].listening`), a rung timer waiting for the speaker to reconnect, stop on SIGTERM under the device's supervisor.
2. **`reply: "streamed"` for an ESPHome device.** It depends on three things the repository has no record of: the device tolerating a pause between sentences in API audio, flushing its buffer when the stream is ended early, and playing a WAV with no declared length over a chunked response. What to listen for is in [esphome-satellite.md](../architecture/esphome-satellite.md). The default is `whole`.
3. **Real services.** The realtime voice provider (including the `session.update` that narrows the tool list when a skill is named), ElevenLabs and Deepgram have only been run against stand-ins. The default token prices, the token estimate and any provider's bill have not been compared.
4. **A late transcript in a realtime session.** If the provider sends the user's transcript after the model has already made its calls, a skill named in that utterance narrows the tools only from the next calls. The timing is unmeasured.
5. **Windows.** The Ctrl-C path of the orderly stop compiles and is untested.
6. **Slow flash.** The audit log appends synchronously on each action. How long that takes on the Dot's storage is unmeasured.
7. **Possibly flaky tests.** The speaker tests in `crates/core/src/speaker.rs` wait real time (50 ms and 150 ms). One unidentified failure was seen once in a full run and not again; `matching_is_fast_enough_for_the_budget` (a timing assertion in `fastpath`) is the suspect.

## 5. Decisions for the maintainer

| # | Question | What happens today |
|---|---|---|
| D1 | May a tool, an MCP source or `clawdot say` send a credential over plain `http://` to a host outside the home network? | Allowed. The language model, speech and realtime clients refuse it (`crate::netpolicy`). |
| D2 | May an https update feed redirect to plain http on the device itself, and should release files carry a signed version line? | The redirect is followed for `localhost`, `127.0.0.1` and `[::1]` only. The version is tied to the file by running the new program with `--version`. |
| D3 | Must the portal work behind a reverse proxy, or under a name the device does not know? | Such a name gets a 403 until the public address setting names it. |
| D4 | Accept the eight decision records marked Proposed that are built and relied on: 0006, 0013, 0016, 0017, 0018, 0019, 0020, 0021. | Proposed. |
| D5 | Turn the full landing gate back on ([ADR-0018](../adr/0018-landing-gate-mode.md)). | `stabilization` mode: a fast local check at landing, the full gate in CI afterwards. |
| D6 | Is English only acceptable for the first release? | Word lists are data; spoken sentences are English in code (U5). |
| D7 | Should a fallback model and a cheaper summary model exist? | The configuration keys are accepted and ignored (B9, S7). |
| D8 | The device strategy: which of the device daemon, the wake-word runtime and the reference satellite are the product. | About 16,000 lines in `crates/device`, `crates/wake` and `crates/satellite` have not been reviewed pending this. |

Choices made during the fix batch without sign-off, to confirm or reverse:

- A timer, alarm or reminder that rings with no speaker connected waits for one, and after 5 minutes is announced as missed instead of ringing (REQ-TMR-015).
- A reminder after a delay is a countdown and works before the clock is set (REQ-TMR-009).
- Printed set-up addresses use the machine's own name with `.local`, and `clawdot.local` only when the name cannot be read (REQ-CFG-016).
- A held confirmation stays until the person answers or its window ends; a second action that would need holding is refused meanwhile, from the model or from a turn that outside text started (REQ-SEC-011).

## 6. Code not yet read by the reviewer

Read in full or mostly: the gate and session, turns, the HTTP server and routing, core state and sign-in, storage, the inbox, the fast path and built-in tools, configuration and start-up, the speaker task, the scheduler, the updater, the MCP client, the audit log's gate and redaction, the ESPHome client, file server and Noise handshake, the agent bridge, and the satellite protocol's messages.

Not read:

- `crates/core/src/esphome/proto.rs` and `frame.rs` (the wire format).
- `crates/core/src/hive/dns.rs`, `identity.rs`, `avahi.rs`, and most of `discovery.rs`.
- About half of `crates/core/src/llm.rs` (the reply parser) and of `crates/core/src/satellite.rs` (playback and the reply path).
- The audit log's query and paging code, `crates/core/src/usage.rs` in full, `crates/core/src/notes.rs` beyond the command parser.
- `crates/satellite`, `crates/device`, `crates/device-conformance`, `crates/wake`, `crates/eval`, `crates/e2e`.
- Most of the portal's TypeScript. It was searched for unsafe rendering of outside data (none found) and not otherwise reviewed.

## 7. Housekeeping on the maintainer's machine

- Several finished agent worktree folders under `.claude/worktrees/` could not be deleted because Windows had them locked. They are no longer registered with git and can be removed by hand.
- WSL build caches under `~/.cache/clawdot-target-*` grow to several gigabytes each. The ones for finished agents were deleted; `clawdot-target`, `clawdot-target-arm` and `clawdot-target-work-tweaks` remain.
