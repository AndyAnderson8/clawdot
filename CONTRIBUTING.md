# Contributing to ClawDot

Thanks for your interest. This project is spec-driven and budget-constrained, which makes contributions a bit more structured than usual. The structure exists to keep quality high, and it should be easy to follow once you've seen it once.

## TL;DR

1. Find or open an issue. For behaviour changes, there must be an **Accepted spec** (or a spec PR) first.
2. Branch: `<type>/<issue#>-<slug>`.
3. Write tests (tagged with requirement IDs) and code.
4. Run `just check` locally.
5. Open a PR with a Conventional Commit title, and fill in the template.
6. Sign off your commits (`git commit -s`).

## Ground rules

- **Spec first.** If you're changing observable behaviour, the protocol, the resource budget, or security posture, propose a spec change before the code. See [spec process](docs/specs/README.md).
- **Mind the budget.** Every dependency and allocation is judged against [the resource budget](docs/specs/0002-resource-budget.md). PRs that exceed it fail CI. A new dependency needs a one-line justification and size impact in the PR.
- **Small PRs.** Focused, reviewable, and with tests and docs included.
- **Be kind.** Reviews are about the work. We follow the [Contributor Covenant](https://www.contributor-covenant.org/version/2/1/code_of_conduct/).

## Development setup

Prerequisites (to be finalised in M0): Rust (pinned via `rust-toolchain.toml`), [`just`](https://github.com/casey/just), `cargo-deny`, `cargo-audit`, and `cross` or Zig for cross-compilation.

```
just check      # fmt, clippy, unit tests, cargo-deny
just test       # full test suite
just cross      # build for armv7-unknown-linux-musleabihf
just budget     # run the resource budget gate (qemu-user)
```

## Process links

| Topic | Document |
|---|---|
| Board, WIP limits, DoR/DoD | [docs/process/sdlc.md](docs/process/sdlc.md) |
| Branches, commits, PRs, releases | [docs/process/git-workflow.md](docs/process/git-workflow.md) |
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
