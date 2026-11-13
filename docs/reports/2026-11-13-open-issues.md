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
| U2 | **Streaming the model's text into speech** (first sound before the model has finished). | Designed in [speech-latency.md](../architecture/speech-latency.md), stage 2. Not built. The reply is spoken sentence by sentence only after the whole reply text exists. | Build it as the design says. This is a feature, not a fix; it goes through the roadmap. |
| U3 | **The branch `docs/agent-harness-brains`** (a research note on agent harnesses as an optional brain, and an untested recipe for using Hermes Agent as the model). | Unlanded, several days behind `dev`. | Merge `dev` into it, check the recipe still matches the settings, land it or delete it. |
| U4 | **The guide for the MCP server** ([hermes-mcp.md](../guides/hermes-mcp.md)) says it was written from Hermes Agent's documentation and never run against a live one. | Untested. | Run it against a real MCP client and correct the configuration example. |
| U5 | **Spoken sentences are still English text in code.** The word lists the logic acts on moved to `crates/core/src/lang/`; the sentences the speaker says did not. | The header of `crates/core/src/lang.rs` lists the files that still hold sentences. The fast path's grammar also names one product ("tell claude to ...", "what did claude say"), now as data in `lang/en.rs`. | Only if a second language is wanted (decision D6). The grammar wording can be made neutral at any time. |

## 3. Structure and code smells

None of these changes behaviour. Each should land with the existing tests passing unchanged.

| # | Smell | Where | Suggested change | Size |
|---|---|---|---|---|
| S14 | Functions over 60 lines remain: `run_session` (81) and `run_one` (77) have no clean seam, `Msg::put_payload` is 64, and about 30 more sit between 61 and 70 (`httptools::validate`, `hive::discovery::run`, `esphome::media::handle`, `ui/status.rs::diagnostics`, `settings.rs::open`, among them). | as named | Find them with a scan of non-test code for bodies over 60 lines; split the ones with a clean seam. | M |
| S18 | Device daemon: the board schema cannot say names must be unique (only the loader checks); `wake_name` returns the model's phrase, not a stable word id; board fields declared but not used by the daemon (`duck`, `volume_control`, `mixer_routes`, `leds.count`, `mute.state_file`, `detect`) are listed in `device-daemon.md`; `connect_core` has no timeout on name resolution; `MwwSink` does not surface `Engine::faults()`; reconnect and conformance cases need the minor cleanups D14, D15, D17 to D19 (named constants, one place that decides button and LED availability, conformance that checks `Cargo.lock` and records why a Must case is skipped). | `crates/device/`, `crates/device-conformance/` | Wire the board fields where the daemon has a seam, or drop them from the schema; log faults; do the cleanups. | M |
| S19 | Wake runtime function lengths: `Plan::step` (about 230 lines), `from_model` (about 268), `run_step` (about 160), `read_options`, `Frontend::frame`; the reference satellite's `run` (about 150). | `crates/wake/src/interp.rs`, `crates/satellite/src/lib.rs` | Split by phase and operator. | M |
| S20 | Audit records written before the clock is known carry day 0 and are not recounted for the real day after a restart; the usage chart's `request_days` reports the saved day while the clock is unset. | `crates/core/src/audit.rs`, `usage.rs`, `ui/status.rs` | Re-stamp day-0 records onto the first real day, and check what the portal shows. | S |
| S16 | The portal handlers in `crates/core/src/ui/` repeat body-reading and error-mapping boilerplate, and the dispatcher's tool and connection arms are still inline. | `crates/core/src/ui/`, `ui/common.rs` | Move the repeated parts into `common.rs`; move the inline arms into `ui/tools.rs`. | S |
| S17 | An eval sentence count and a speech request cap changed slightly in the structure batch (see the completed list, S10 and S11). A very slow voice or a 44.1 kHz server over a 1,000-character direct request is refused at 4 MiB. | `crates/core/src/apiclient.rs`, `crates/eval/src/run.rs` | Confirm both against real providers and the live eval scenarios; raise the cap per sample rate if needed. | S |
| S7 | `TurnConfig.fallback_model` and `summary_model` are still read by the model loop and the summary code, but nothing sets them from configuration any more (start-up passes `None`). It is dead plumbing, or a feature that was switched off by accident. | `crates/core/src/runtime.rs` (`agent_turn_config`, `build_llm`), `crates/core/src/session/gate/rounds.rs`, `crates/core/src/speaker.rs`, `crates/core/src/session.rs` | Decide first (D7): wire the two configuration keys back in, or remove the fields and the code that reads them. | S to M |
| S8 | The built portal page (`crates/core/src/ui.html`, about 200 KB) is committed, so two branches that touch the portal always conflict on it. | `portal/build.mjs`, `crates/core/build.rs` | Build it in `build.rs` or in CI and stop committing it; or keep committing it and add a merge driver that rebuilds. Either needs Node at build time to be acceptable. | M |
| S12 | The version is `0.0.0` everywhere. | `Cargo.toml`, `portal/package.json` | Set by the first release (`scripts/release.sh prepare`); the maintainer publishes. | S |

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
| D9 | Announcements in quiet hours (REQ-DEV-006 Must) cannot be deferred by the device, because it cannot tell an announcement from a reply. Mark unsolicited audio on the wire (a protocol change, version 0.2 of the satellite protocol), or have the core hold announcements in quiet hours, or accept REQ-DEV-006 as partly met. | Partly met; documented. |
| D10 | A clock wrongly set far into the future and later corrected leaves the daily counters on the future day (the spend cap stays reached until real time catches up). Allow a large backwards step to roll, or leave it as it is (fails safe). | The counters never roll backwards. |

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
