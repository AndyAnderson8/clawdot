# Architecture and roadmap review, 2026-11-11

A review of the code and the roadmap as they stand on `dev` at `384b1d8`, with a list of what to fix and in what order. Every finding names the evidence it rests on. Numbers were measured on that commit.

## The short answer

The architecture is sound at its core and does not need a rewrite. The safety model is the strongest part: tools go through one host with risk tiers, confirmation held by the host and an audit log, and every new feature this week (realtime voice, the MCP server, memory recall) was made to pass through it. The code is disciplined: `unwrap` and `expect` are denied by lint outside tests, there are two TODO comments in the whole tree, about half of each large file is tests, and the default build has about a dozen direct dependencies.

Three things are wrong, and they matter more with each feature added:

1. **One loop does everything, one thing at a time.** While the assistant is answering, nothing else runs.
2. **The same fact is written in several places by hand** (settings in six, routes in four), so adding anything means editing all of them and merges collide.
3. **Breadth is outrunning proof.** A lot was built in the last days and almost none of it has run against a real service or on the device.

The roadmap is ambitious for one maintainer. The order should change: prove what exists and fix the loop before adding the hive, an operating system layer or more providers.

## What is good and should be kept

- **The tool host and its tiers.** One place decides whether an action runs. Realtime voice reuses it instead of bypassing it.
- **Untrusted text is treated as data** everywhere: webhook text, tool results, `ask_user` answers, skills.
- **Bounded stores** with atomic writes and private file modes (`store.rs`), and a resource budget written down as a spec.
- **Optional things are cargo features that add little:** realtime added no crate, the hive added about 56 KB.
- **The portal is small** (one runtime dependency, about 7,400 lines) and has a real browser test.
- **The updater** is signed, checks a hash, probes `--version` and keeps the previous binary.

## Findings

Priority 1 is worth doing before the next feature. Priority 2 is debt that slows every change. Priority 3 is cleanup.

### Core

| # | Priority | Finding | Evidence | Fix |
|---|---|---|---|---|
| C1 | 1 | **The main loop blocks on everything it awaits.** A model turn, a tool call, an announcement and a realtime session (up to 300 s) are awaited inside the one `select!`. While one runs, timers ring late, portal changes wait, and a second speaker waits. | `main.rs`: `main()` is 594 lines; the loop at line 1777 awaits `brain.respond`, `ext_apply`, `announce` and `run_realtime` inline. The realtime agent listed this as its known limit. | Keep one owner of the state (that part is right), but run each turn as its own task that sends its result back as a message. Start with realtime sessions and model turns. |
| C2 | 1 | **One conversation for every speaker.** Two rooms share one context and one held confirmation. | `Brain.conversation` is a single value (`main.rs` line 838). | Key the conversation and the held action by speaker id. The hive and multi-room need this first. |
| C3 | 1 | **A setting is defined in about six places.** The field table (with a hand-counted length), the validator `match`, the defaults list, the "which keys are live" lists, the status output and the portal form. | `settings.rs` `FIELDS: [Field; 35]`; `main.rs` `apply_live` lists of key strings; this week two branches collided on the array length. | One table per setting (key, type, default, validate, live or restart) and derive the rest from it. A typed struct instead of a JSON map with string keys. |
| C4 | 2 | **Routes are written four times.** The `known` list, the handler `match`, the "needs a sign-in" test list and `docs/api/openapi.yaml`. | `ui.rs`: 35 entries in `known`, 50 match arms; a drift test covers only the OpenAPI pair. | One route table (method, path, handler, who may call it). Generate the `known` check and the auth test from it. |
| C5 | 2 | **Too much lives in the binary.** Argument parsing, settings seeding, the `Brain`, live settings, status and the speech registry are in `main.rs`, where only whole-process tests can reach them. | `main.rs` has 1,892 lines outside tests; `tests/binary.rs` is 1,618 lines of spawned processes. | Move them into library modules (`cli`, `brain`, `runtime`). `main.rs` should be about a hundred lines. |
| C6 | 2 | **Seven process-wide globals.** Secrets, speech source, speech handles, hive, avahi file, updater, log ring. Tests share them, and two cores cannot run in one process, which the hive tests will want. | `static` items in `secrets.rs`, `speech.rs`, `main.rs`, `hive/mod.rs`, `update.rs`. | Hold them in `Core` and pass handles. The log ring and the hardware probe can stay global. |
| C7 | 2 | **Three functions over 500 lines.** | `run_turn_from` 632, `run_session` 520, `main` 594. | Split by phase around a small state struct. Do it when C1 touches them. |
| C8 | 2 | **Errors are strings, and code matches on their text.** | 130 `Result<_, String>` in core; `ui.rs` decides a status code with `e.contains("401")`. | A small error enum at the boundaries where a caller branches (provider calls first). Strings are fine where the text is only shown. |
| C9 | 2 | **Realtime reuses the turn loop through a trick.** It feeds the model's function calls to `run_turn` as a scripted backend, which doubles the request count and ties two designs together. | Reported by the realtime agent; `realtime.rs`. | Pull the gate (tier, confirm, audit, limits) out of `session.rs` as its own component that both paths call. |
| C10 | 3 | **The environment is read in 23 places.** | `std::env::var` in seven files. | Read it once at start and pass a lookup, as `settings.rs` already does. |
| C11 | 3 | **File writes happen inside request handlers.** The files are tiny, so it is not a problem today. | `JsonStore` uses `std::fs`; called from `ui.rs` handlers. | Leave it; note it if stores grow (the hive will grow them). |
| C12 | 3 | **The version is `0.0.0`.** The updater compares versions and every deploy is identified only by a hash. | `Cargo.toml`. | Start versioning, and put the git commit in `--version`. |

### Device and build

| # | Priority | Finding | Evidence | Fix |
|---|---|---|---|---|
| D1 | 1 | **The build that runs on the Dot is not built or measured in CI.** The budget is called a feature, but no gate enforces it. | `ci.yml` line 99 is still `TODO(TASK-003)` for the armv7 build and the budget; the Dot uses features `esphome-satellite,esphome-noise` on armv7 musl, built by hand. | A CI job that builds exactly that artifact and fails on size, plus the feature combinations `default`, `all` and `dot`. |
| D2 | 1 | **Two device strategies are carried at once.** Our own daemon, satellite, wake word and conformance suite are about 16,000 lines, and none of it is what runs on the Dot. What runs is the ESPHome client talking to another project's voice service. The roadmap now adds a third piece, our own operating system layer. | `crates/device`, `satellite`, `wake`, `device-conformance`; `docs/guides/dot-ssh.md`. | Decide which path is the product for the next six months and freeze the other. The own-Linux work should not start before that decision and the B3 gate. |
| D3 | 2 | **The portal and the API key travel unencrypted on the home network**, and the Dot runs everything as root with a root SSH key. | Portal is plain HTTP; `secrets.rs` says the key file is not encrypted at rest; `dot-ssh.md`. | An HTTPS option for the portal before any hive feature shares secrets. A non-root user for ClawDot on our own image. |
| D4 | 3 | **The Dot supervisor script has a fixed network range.** | `scripts/dev/dot-run.sh`: `192.168.0.0/24`. | Derive it from the interface, or make it a variable. |

### Portal

| # | Priority | Finding | Evidence | Fix |
|---|---|---|---|---|
| P1 | 1 | **Backend features without a page.** Realtime, the speech provider choice and key, accepting an imported skill, memory search and hive peers exist only as API. | This week's agent reports. | Build them before more backend work, or they will rot. Skills needs it most: enabling an imported skill now fails until it is accepted. |
| P2 | 2 | **The built page is committed.** Every UI branch changes `ui.html` and conflicts with every other. | 187 KB, changed in 46 commits. | Keep it committed (the Rust-only build needs it), but rebuild it in the landing step instead of merging it by hand, and mark it so git never tries to merge it. |
| P3 | 2 | **The settings form component carries state it was not made for.** Private keys such as `_keyok` and `_keyrev` are passed through the values map to signal between parts. | 7 uses in `Settings.tsx` and `SettingsPage.tsx`. | Split the form into field components with a small shared state hook. Move each settings section out of the 646-line `SettingsPage.tsx`. |
| P4 | 2 | **A third and fourth copy of the API.** `mock-api.ts` reimplements the server and `types.ts` restates its shapes. | 691 and 244 lines. | Check the mock's answers against `openapi.yaml` in the unit tests, and later generate the types from it. |
| P5 | 3 | **13 browser `confirm()` boxes** beside the new dialog pattern. | `grep confirm(`. | One confirm dialog component. |
| P6 | 3 | **One 1,750-line browser test** that knows 176 element ids, and a 2,157-line stylesheet that grew by appending. | `smoke.ts`, `styles.css`. | Split the test by page. One pruning pass on the stylesheet. |

### Process and documents

| # | Priority | Finding | Evidence | Fix |
|---|---|---|---|---|
| R1 | 1 | **Nothing built this week is proven outside tests.** Realtime, ElevenLabs, Deepgram, the MCP server with a real client and hive discovery on a real network have never run for real. A spoken turn on the Dot has not been re-tested since many deploys ago. | Every agent report says so. | A proving pass with a checklist before the next feature. Record results in the repo. |
| R2 | 1 | **The full gate is off.** Landing runs a fast check and does not wait for CI. With several branches landed in a row, CI cancelled three runs, so those commits were never tested alone. | `scripts/gate-mode` is `stabilization`; three cancelled runs on 2026-11-11. | Return to `full`, or land parallel work through one integration branch. |
| R3 | 2 | **The policy tests take up to 1 h 45 min on Windows.** | Measured on 2026-11-11. | Run them inside WSL or cut the process spawns. The trigger is already narrowed to the real policy scripts. |
| R4 | 2 | **The spec's version line makes parallel work collide.** One 459-line file with 258 requirements and a version bumped on every change. | Agents had to be told not to touch it. | Drop the per-change version and changelog (git has the history), or split the spec by section. |
| R5 | 2 | **The roadmap is hard to read as a plan.** 515 lines that mix phases, status notes, a backlog and three numbered lists. Research notes have two R-11s and no R-13. | `docs/roadmap.md`, `docs/research/`. | One page: now, next, later, with a link per item. Move status history out. Renumber the research notes. |
| R6 | 3 | **61 worktrees and 62 local branches.** One research branch is held, unlanded. | `git worktree list`. | Remove the finished agent worktrees. Land or drop `docs/agent-harness-brains`. |

## The roadmap

**Is the direction right?** Yes. A private, hardware-agnostic brain with a strict safety model is a real difference from the Home Assistant front ends, and the hive is the right long-term idea.

**What I would change:**

1. **Stop adding surface until what exists is proven (R1) and has a page (P1).** Each unproven feature is a guess that later work builds on.
2. **Fix the loop (C1, C2) before realtime and the hive go further.** Realtime voice in a loop that blocks for the whole session is not usable in a real house, and the hive assumes speakers that keep working while one is busy.
3. **Do the settings and route tables (C3, C4) before more settings arrive.** Realtime added nine settings and the speech providers added more; each one cost six edits.
4. **Make the device decision (D2) before the own-Linux layer.** That phase is the largest item on the roadmap and the least suited to one person. Do its first step only (the inventory and the kernel decision), and keep the current image as the base until the voice loop itself is good.
5. **Put the basics of a speaker ahead of the extras.** About seven seconds from the end of speech to the reply, no interrupting it while it talks and no echo cancellation are what a person in the room notices. They sit in phase B3, which is still an open gate, behind work on themes, providers and the hive.
6. **Hold the hive at its first step.** Identity and discovery are in. Pairing and shared data should wait for C2, C6 and D3, and for the maintainer's answers to the design questions.

## Suggested order

1. Proving pass on what exists (R1), and return the gate to full (R2).
2. Portal pages for what the backend already does (P1).
3. The loop: turns as tasks, a conversation per speaker (C1, C2), and the gate pulled out of `session.rs` (C9) while there.
4. Settings table and route table (C3, C4); move the runtime out of `main.rs` (C5).
5. The Dot build and budget in CI (D1); versions (C12).
6. The device decision (D2), then latency and barge-in on the chosen path.
7. HTTPS for the portal (D3), then the next hive step.

Everything else in the tables can be done alongside, when a change touches the same file.

## What this review did not do

It read the code and measured it; it did not profile memory or latency on the device, audit the cryptography in the Noise or hive code line by line, or review the wake-word and device crates in depth.
