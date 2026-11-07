# Git Workflow

## Model: trunk-based development

- `main` is the only long-lived branch. It must always build, pass tests, and be releasable.
- All work happens on **short-lived branches** (target < 3 days, hard limit 1 week) merged via PR.
- No long-running feature branches. Incomplete features hide behind compile-time features or runtime config, not branches.
- **Squash merge** only. One PR = one commit on `main`, so history reads as a changelog.

## Branch protection (`main`)

- Direct pushes disabled.
- PR required; required status checks: `fmt`, `clippy`, `test`, `deny`, `cross-armv7`, `budget`.
- Branch must be up to date before merge.
- Linear history required.
- Signed commits required.
- Force-push and deletion disabled.

## Branch naming

```
<type>/<issue-number>-<short-kebab-slug>
```

Types: `feat`, `fix`, `docs`, `spec`, `refactor`, `perf`, `test`, `build`, `ci`, `chore`, `spike`.

Examples: `feat/42-timer-tool`, `spec/17-protocol-handshake`, `fix/88-opus-underrun`.

## Commit messages

[Conventional Commits 1.0](https://www.conventionalcommits.org/), enforced in CI by commitlint (applied to the squashed PR title).

```
<type>(<scope>)<!>: <imperative summary, ≤ 72 chars>

<body: what and why, wrapped at 80; not how>

Refs: REQ-TMR-001, REQ-TMR-002
Closes: #42
BREAKING CHANGE: <description, if any>
```

**Types:** `feat`, `fix`, `docs`, `spec`, `refactor`, `perf`, `test`, `build`, `ci`, `chore`, `revert`.

**Scopes:** `core`, `protocol`, `satellite`, `audio`, `wake`, `session`, `stt`, `tts`, `llm`, `tools`, `mcp`, `sched`, `inbox`, `http`, `ui`, `store`, `config`, `budget`, `deps`.

**Rules:**
- Imperative mood: "add timer tool", not "added" or "adds".
- Summary has no trailing period.
- A `!` after the scope or a `BREAKING CHANGE:` footer marks a breaking change and triggers a major/minor bump per SemVer.
- Every `feat` and `fix` should include at least one `Refs:` requirement ID unless it's purely internal.
- No "WIP", "fix stuff", or "address review comments" on `main`. Squash handles this, so the PR title is what counts.

Examples:
```
feat(sched): add persistent timer store

Timers survive restarts and fire within ±1s of deadline without network.

Refs: REQ-TMR-001, REQ-TMR-004
Closes: #42
```
```
spec(protocol)!: require capability handshake before audio

BREAKING CHANGE: satellites must send `hello` before any audio frame.
```

## Pull requests

- Title follows the commit convention (it becomes the squash commit).
- Use the PR template. Link the issue, requirement IDs, and note budget impact.
- Keep PRs focused: aim for < 400 changed lines excluding generated files and lockfiles. Larger PRs need a stated reason.
- Draft PRs are encouraged for early feedback.
- Docs, specs, and tests ship in the **same PR** as the code that needs them.

## Sign-off and signing

- **DCO:** every commit carries `Signed-off-by:` (`git commit -s`). Chosen over a CLA to keep contribution friction low. See CONTRIBUTING. Enforcement is dormant until the first external contributor ([SDLC §13](sdlc.md#13-proportionality-solo-maintainer-mode)).
- **Signed commits:** use SSH or GPG signing (`git config commit.gpgsign true`).

## Tags and releases

- Tags are annotated, signed, and follow `vMAJOR.MINOR.PATCH`.
- Protocol versions are tagged `protocol-vMAJOR.MINOR` when the protocol spec changes.
- See [SDLC §9](sdlc.md#9-releases).

## Repository hygiene

- Never commit secrets. API keys live in config files that are git-ignored; the repo ships `config.example.toml` only. Secret scanning is enabled.
- Do not commit build outputs, model blobs over 1 MB, or audio fixtures over 500 KB without discussion. Use Git LFS for approved large fixtures.
- `Cargo.lock` is committed (this is an application).
- Line endings are normalised via `.gitattributes` and `.editorconfig`.
