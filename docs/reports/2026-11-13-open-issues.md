# Open issues, 2026-11-13

Everything known to be wrong, unfinished, unproven or undecided on `dev` after the code read and the fix batches of 11 to 13 November. It is written so that someone who was not there (a person or a coding agent) can pick an item and do it. The history of what was found and fixed is in [the code read and quality pass](2026-11-12-code-read-and-quality-pass.md); this file is only what is left.

State of `dev` when this was written: product spec 0.60 (now 0.65; the items fixed since are in [the completed list](2026-11-13-complete.md) and removed from the tables below), ESPHome spec 0.3, the whole workspace test suite and CI passing. **Nothing from these days has run on a device.**

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

## 2. Work that was started and not finished

| # | What | State | What to do |
|---|---|---|---|
| U1b | **The coding-agent set-up text exists twice** (the adapter file and `portal/src/snippets.ts`). A drift test now ties them, but there are still two copies. | Drift test landed. | Serve each adapter's `setup` lines from the API and have the page print them. The wording around the JSON and the token placeholder differ, so the response, the fake device and the page change together. |
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
