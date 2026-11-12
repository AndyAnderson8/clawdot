# Contributing to ClawDot

Thanks for your interest. This project is spec-driven and budget-constrained, which makes contributions a bit more structured than usual. The structure exists to keep quality high, and it should be easy to follow once you've seen it once.

## TL;DR

1. Find or open an issue. For behaviour changes, there must be an **Accepted spec** (or a spec PR) first.
2. Fork the repository and branch from `dev`: `feature/<name>`, `bugfix/<name>`, `docs/<name>`, and so on. Never work on `main` or `dev`.
3. Write tests (tagged with requirement IDs) and code. Commit small and often.
4. Run `scripts/setup.sh` once (enables the git hooks), then `scripts/check.sh` before pushing.
5. Every commit must be a [Conventional Commit](docs/process/git-workflow.md#3-commit-messages). Open a pull request **against `dev`** and fill in the template.
6. Sign off your commits (`git commit -s`).

Maintainers and their AI pair programmers follow the same rules but integrate with `scripts/land.sh` instead of pull requests. The full policy is the [git flow](docs/process/git-workflow.md). AI agents must also follow [AGENTS.md](AGENTS.md).

## Ground rules

- **Spec first.** If you're changing observable behaviour, the protocol, the resource budget, or security posture, propose a spec change before the code. See [spec process](docs/specs/README.md).
- **Mind the budget.** Every dependency and allocation is judged against [the resource budget](docs/specs/0002-resource-budget.md). PRs that exceed it fail CI. A new dependency needs a one-line justification and size impact in the PR.
- **Small PRs.** Focused, reviewable, and with tests and docs included.
- **Be kind.** Reviews are about the work. We follow the [Contributor Covenant](https://www.contributor-covenant.org/version/2/1/code_of_conduct/).

## Development setup

You need git and bash (Git Bash on Windows). Everything else is optional until a gate asks for it:

- **Rust**, at the version pinned in `rust-toolchain.toml` (rustup installs it). On Windows without a C toolchain, `scripts/cargo.sh` runs cargo inside WSL instead; see [the development environment](docs/process/dev-environment.md).
- **Node 24 or newer**, only when you change `portal/` or `crates/core/src/ui.html`. Install the two sets of tools once: `npm --prefix portal ci --ignore-scripts` and `npm --prefix portal/lint ci --ignore-scripts` (`scripts/check.sh` does this for you when they are missing). The portal's own commands are in [portal/README.md](portal/README.md).
- **shellcheck**, to match the shell checks in CI.
- Optional: `cargo-deny` (the full gate runs it when installed; CI always does), `just` (a thin wrapper over the scripts).

```
scripts/setup.sh                  # once after cloning: installs the git hooks, checks your git identity
scripts/cargo.sh test -p clawdot-core   # any cargo command; native cargo if present, otherwise WSL
scripts/check.sh --fast           # quick check scoped to what you changed (the pre-commit hook runs this)
scripts/check.sh                  # every gate: format, clippy, tests, docs, commit messages, portal, shell, deny
```

`scripts/check.sh` must pass before you ask for a merge. The full gate also builds and tests the optional `realtime` feature and replays the agent-loop evaluation scenarios.

### Landing (maintainers)

Maintainers and their AI pair programmers do not open pull requests. They merge a branch that already contains `dev` with `scripts/land.sh <branch>`, which runs the gates and makes a merge commit. In the current gate mode (`scripts/gate-mode`, [ADR-0018](docs/adr/0018-landing-gate-mode.md)) landing runs only the fast check and does not wait for CI, so run `scripts/check.sh` yourself first. The step-by-step commands are in the [runbook](docs/process/runbook.md). Releases use `scripts/release.sh`, and publishing is done by the maintainer.

## Process links

| Topic | Document |
|---|---|
| Board, WIP limits, DoR/DoD | [docs/process/sdlc.md](docs/process/sdlc.md) |
| Git flow: branches, commits, landing, releases | [docs/process/git-workflow.md](docs/process/git-workflow.md) |
| Step-by-step commands (humans and agents) | [docs/process/runbook.md](docs/process/runbook.md) |
| Rules for AI coding agents | [AGENTS.md](AGENTS.md) |
| Writing a spec | [docs/specs/README.md](docs/specs/README.md) |
| Testing expectations | [docs/quality/testing-strategy.md](docs/quality/testing-strategy.md) |
| Security | [SECURITY.md](SECURITY.md) |

## Licensing of contributions

ClawDot is licensed under [Apache-2.0](LICENSE). Unless you state otherwise, any contribution you submit is licensed under the same terms (Apache-2.0 §5). Don't submit code you don't have the right to license, and don't add dependencies with licences that `cargo-deny` rejects (see [ADR-0007](docs/adr/0007-license.md)).

## Developer Certificate of Origin

By contributing you certify the [DCO 1.1](https://developercertificate.org/). Add a sign-off line to each commit:

```
Signed-off-by: Your Name <you@example.com>
```

`git commit -s` does this automatically.

## Code standards (summary)

- `cargo fmt` clean; `cargo clippy -- -D warnings` clean.
- No `unwrap()`/`expect()` in non-test code paths that handle external input.
- `unsafe` requires a `// SAFETY:` comment and reviewer sign-off.
- Errors are typed and carry context. No silent failures.
- No panics from untrusted input (protocol frames, webhook bodies, LLM output).
- Logging is structured, leveled, and never contains secrets or raw audio.
- Public items are documented.

## Reporting bugs and requesting features

Use the issue templates. For security issues, **do not open a public issue**. See [SECURITY.md](SECURITY.md).
