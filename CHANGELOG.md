# Changelog

All notable changes are documented here. This file is generated from Conventional Commits by
git-cliff at release time; do not edit it by hand. Versioning follows [SemVer](https://semver.org).
The satellite protocol is versioned separately (`protocol-vMAJOR.MINOR`).

## [0.1.0] - 2026-11-07

### Added
- **scripts:** Publish releases as a tagged dev-to-main merge commit (6bf3554)
- **agents:** Block agents from publishing releases (7940b4a)
- **protocol:** Add Wyoming-compatible framing with hard limits (929e9ae)
- **protocol:** Add typed messages, capabilities, and versioning (f89c0a9)
- **protocol:** Add the agent-bridge reply wire types (5713ca4)
- **core:** Add atomic versioned JSON storage and the scheduler (c7e6e32)
- **core:** Add the local fast-path command matcher (44ab9fc)
- **core:** Add the agent inbox (0409a9b)
- **core:** Add the shared application core (321ff48)
- **core:** Add the HTTP front end for webhooks and bridges (00b4e0d)
- **core:** Dispatch fast-path commands to the scheduler and inbox (0b740a6)
- **core:** Add the clawdot binary with a text console (342299f)
- **bridge:** Add an SSE parser and a std-only HTTP client (02940da)
- **bridge:** Resume Claude Code sessions from voice replies (d3bab08)
- **core:** Add the language-model client, usage caps, and tool loop (82c84c3)
- **core:** Expose assistant tools to the model and wire LLM flags (5efff31)


### Documentation
- Add README, Apache-2.0 license, and NOTICE (512e4b5)
- **spec:** Add requirements, resource budget, and protocol specs (24fbad6)
- **adr:** Record architecture decisions 0001-0010 (a6ef15a)
- **arch:** Add architecture overview and extensibility model (fc2e91a)
- **research:** Add research notes R-01 to R-06 (33dd6ba)
- **security:** Add threat model, privacy, and testing strategy (77fa525)
- **process:** Add SDLC, git workflow, contribution guide, dev setup (cc4ace9)
- Add roadmap, future scope, docs index, and changelog (26e5176)
- **adr:** Record git flow with merge commits as ADR-0011 (d325f94)
- **process:** Rewrite git workflow as git flow (1e9bac4)
- **contrib:** Update contributing guide and PR template for git flow (badee88)
- **spec:** Require the changelog in the build and About page (c56c242)
- **roadmap:** Record git flow enforcement and changelog in the build (6c0822a)
- **process:** Document hook stubs that fail closed on main (5febf4a)
- **process:** Add a step-by-step runbook for humans and agents (e0dce91)
- **process:** Switch releases to a dev-to-main merge commit (9206050)
- Describe the project with the BYOA tagline (a717882)
- **process:** Add a sudo-free WSL setup for Rust builds (ea5f137)
- **reports:** Record the overnight session and update the roadmap (4ba33f3)


### Fixed
- **hooks:** Block commits on main when versioned hooks are absent (e97b123)
- **agents:** Allow reading git identity and hook settings in the guard (fa8705b)
- **hooks:** Write stubs only to the real hooks directory (2b09c9e)
- **agents:** Scan only the command text in the guard (9189e01)
- **scripts:** Drop unused loop variable flagged by shellcheck (681434f)
- **scripts:** Pass only existing files to shellcheck (c2a8149)
- **core:** Do not repeat the headline when the summary is the headline (88509f7)


### Maintenance
- Initialize repository (a9d0a15)
- Add editor, line-ending, and ignore configuration (bad25cd)
- **github:** Add issue and pull request templates (db256e1)
- **scripts:** Add quality gates and policy regression tests (812083f)
- **scripts:** Add gated landing, release, and GitHub setup scripts (1f0676f)
- **hooks:** Add git hooks for branch and commit discipline (be63538)
- Add CI workflow with the required gate job (4fd121c)
- Add release workflow and git-cliff changelog config (5d46660)
- Update GitHub Actions weekly through Dependabot (61173cc)
- **agents:** Add agent rules and Claude Code git guard (a6648fa)
- Resolve shellcheck source hints from the script directory (107003c)
- Add Rust workspace with pinned toolchain and size-first profile (6159b50)
- Add cargo-deny licence and source policy (7bf7e5d)
- **scripts:** Run cargo through WSL when no native toolchain exists (ce9aaae)
- **wsl:** Strip the host target triple from the zig cc wrapper (426d02a)
- **budget:** Add the resource-budget measurement script (8d8d23d)


### Performance
- **agents:** Make the git guard spawn-free (e1870ee)
- **scripts:** Check documentation links with a single grep (de16b85)
- **scripts:** Run the slow policy tests only when the tooling changes (87b0c69)


### Tests
- **scripts:** Cover landing and release flow end to end (36993d6)
- **core:** Run the real binary and check it announces a posted event (daf0e63)
- **e2e:** Prove the agent loop over real sockets (a8bf3ec)
- **core:** Run the binary against a fake model provider (fe24618)


