# Runbook: working in this repository

Step-by-step commands for humans and AI agents. The policy and its reasons are in [git-workflow.md](git-workflow.md); this page is what to type. Every step has the same shape: command, what it does, and what to do if it fails.

## 0. Who may do what

| Action | Human | AI agent |
|---|---|---|
| Create branches, commit, update from `dev`, run checks | yes | yes |
| Land a branch into `dev` (`scripts/land.sh`) | yes | yes |
| Prepare a release (`scripts/release.sh prepare`) | yes | yes |
| **Publish a release** (`scripts/release.sh publish` / `tag`) | yes | **no. Stop and ask the human to run it** |
| Push to `main` or `dev` by hand, force-push, delete branches, create tags | no | no |
| Change git identity, hooks, or CI/ruleset settings | human only | no |

The only intended direct write to a protected branch is the landing and release scripts. They enforce everything else for you.

## 1. One-time setup (after cloning)

```
scripts/setup.sh
```

Installs the git hooks and checks your identity. Commits must be authored by a human identity (the maintainer's). Never configure an AI name or email.

## 2. Start work

```
git switch dev && git pull
git switch -c feature/timer-tool        # or bugfix/..., docs/..., chore/..., spike/...
```

Branch names are `type/kebab-name` (`feature/`, `bugfix/`, `hotfix/`, `release/`, `docs/`, `chore/`, `spike/`). Never work directly on `dev` or `main`.

## 3. Commit (early, often, small)

```
git add <specific files>
git commit -m "feat(sched): add persistent timer store" -m "Why this change is needed. Refs: REQ-TMR-001"
```

- Format: `type(scope): summary`, imperative, at most 72 characters, no trailing period. Types: `feat fix docs spec refactor perf test build ci chore revert`.
- One logical change per commit, with its tests and docs. Reference requirement IDs in the body.
- Do not add `Co-Authored-By`, "Generated with ...", or any AI attribution. Hooks and CI reject it.
- Hooks run on each commit and check only fast things: branch name, commit message, and `scripts/check.sh --fast` (changed shell files, docs if Markdown changed, `cargo fmt` and scoped `clippy` if Rust changed; docs-only commits skip Rust). Tests and `cargo-deny` run at `scripts/land.sh` and in CI. If a hook fails, fix the cause and commit again; never use `--no-verify`.

## 4. Keep your branch current

```
git fetch && git merge dev        # run on your branch; resolve any conflicts here
```

A branch can only be landed if it contains the latest `dev`.

## 5. Land into `dev`

```
scripts/check.sh                  # optional preview: the FULL gate that land.sh and CI run (format, lint, types, tests, deny, docs, commits)
scripts/check.sh --fast           # the quicker subset that the commit hook runs on staged files
scripts/land.sh feature/timer-tool
```

What it does: runs all gates on your branch, creates a **merge commit** on `dev`, and (once a remote exists) pushes that exact commit to a CI candidate branch, waits for CI to pass, then publishes it. After the push, it fetches and checks that `origin/dev` contains the merge commit; only then does it print `done` and delete your branch.

Exit codes: `0` landed and verified, `1` other error, `3` `dev` held by another worktree, `4` merge exists only in local `dev`, `5` CI or publishing failed, `6` (batching only, section 5a) integration branch moved underneath. `--wait SECONDS` (default 0) retries while another worktree holds `dev`.

**Gate mode.** `cat scripts/gate-mode` shows the current mode. In `stabilization` (the current default, [ADR-0018](../adr/0018-landing-gate-mode.md)) landing into `dev` runs only the fast local check, pushes `dev` directly without waiting for CI, and prints `FULL GATE SKIPPED BY MODE`. Run `scripts/check.sh` yourself first if you want the full gate. To restore the full gate and CI, edit `scripts/gate-mode` to `full` on a `chore/` branch and land it; do this before the first release. Hotfixes into `main` and releases are always fully gated. If `dev` has a server rule requiring `gate`, the direct push is refused (exit `4`) until that rule is relaxed ([repo-settings.md](repo-settings.md)).

If it fails:

| Message | Fix |
|---|---|
| `scripts/gate-mode must be 'full' or 'stabilization'` | fix the file on a `chore/` branch (one word, comments with `#` allowed) |
| `does not contain dev` | `git switch <branch> && git merge dev`, then land again |
| `working tree is not clean` | commit or stash your changes |
| a gate fails | fix it on your branch, commit, land again |
| `CI or publishing failed; dev was reset` (exit 5) | open the failing CI run, fix on your branch, land again. `dev` is unchanged. If it says `dev` could not be reset, run the `git reset --hard` it prints. |
| `NOT LANDED ... exists only in your LOCAL dev` (exit 4) | the push to `origin/dev` was rejected or not confirmed. Your branch and the CI candidate ref are kept. Read the server message, restore local `dev` with the `git switch dev && git reset --hard <sha>` it prints, then land again. |
| `dev is checked out in another worktree` (exit 3) | only one worktree can hold `dev`. Ask the holder to release it (`git -C <path> switch --detach`), or run `scripts/land.sh <branch> --wait 300` to retry for up to 300 seconds. |
| `gh CLI is required` | install and sign in to GitHub's CLI (`gh auth login`) |

## 5a. Batch several branches, land once (integration branches)

Use this for a milestone or parallel agent work you want to test and publish together. For a single branch use section 5. Policy: [git-workflow.md section 5a](git-workflow.md#5a-batching-a-milestone-integration-branches).

```
scripts/new-integration.sh my-milestone                     # or: scripts/land.sh --new-integration my-milestone
# on each working branch, once it is ready:
git merge integration/my-milestone                          # the branch must contain the batch so far
scripts/land.sh feature/x --into integration/my-milestone  # fast check, LOCAL merge commit, branch deleted. No push, no CI.
# when dev has moved on (or land says "does not contain dev"):
scripts/land.sh --update integration/my-milestone           # merges dev into the batch, locally
# when the milestone is done:
scripts/land.sh integration/my-milestone --wait 900         # full gate, CI on the exact merge, push dev, verify
```

Extra exit code for `--into` and `--update`: `6` the integration branch moved while the merge was being made (nothing changed). Exit `3` means a worktree has the integration branch checked out (or, when landing, `dev`).

| Message | Fix |
|---|---|
| `<branch> does not contain integration/<name>` | `git switch <branch> && git merge integration/<name>`, then land again |
| `integration/<name> moved underneath this landing` (exit 6) | someone else batched first: `git merge integration/<name>` on your branch, land again |
| `'integration/<name>' is checked out in a worktree` (exit 3) | `git -C <path> switch --detach` there, or add `--wait SECONDS`. Integration branches never need a checkout. |
| `does not contain dev` when landing the batch | `scripts/land.sh --update integration/<name>`; on conflicts follow the branch recipe it prints |
| `commits are never made by hand on an integration branch` | switch away (`git switch --detach`); put the change on a `feature/` or `chore/` branch and land it `--into` the batch |
| the fast check fails | fix it on the source branch; the integration branch is untouched |
| landing the batch fails (exit 4 or 5) | exactly as in section 5; the integration branch and its candidate ref are kept |

## 6. The tiny-change exception

Only for **one** change of at most 10 lines in 2 files, with nothing else unlanded on `dev`. If in doubt, use a branch.

```
git switch dev && git pull
git add <file> && git commit -m "docs: fix typo in README"
scripts/land.sh --tiny
```

## 7. Cut a release

Three steps. Steps 1 and 2 are for anyone. **Step 3 is human-only.**

```
# 1. Prepare: branches release/X.Y.Z from dev, regenerates CHANGELOG.md, commits it
git switch dev && git pull
scripts/release.sh prepare            # computes the next SemVer from commits; or: prepare 0.2.0

# 2. Land the release branch into dev like any other branch
scripts/land.sh release/0.2.0

# 3. HUMAN ONLY, in an interactive terminal: publish
scripts/release.sh publish 0.2.0
```

What `publish` does: checks `dev` has the changelog entry for that version and contains `main`, runs the release gate, asks you to type `v0.2.0` to confirm, then **merges `dev` into `main` with a merge commit** (pushed only after CI passes on that exact commit), **tags that merge commit** `v0.2.0` (annotated), and pushes the tag. The tag triggers the `release` workflow, which builds the artifacts and creates the GitHub Release with `CHANGELOG.md` attached.

**Agents:** when you reach step 3, stop and tell the human exactly: "Run `scripts/release.sh publish X.Y.Z` in your terminal." Do not try to bypass the interactive check.

Version choice (SemVer): breaking change = major (minor while 0.x); any `feat` = minor; only fixes = patch. See [git-workflow.md section 7](git-workflow.md#7-versioning-and-tags).

If it fails:

| Message | Fix |
|---|---|
| `no entry for X.Y.Z` in `dev`'s changelog | do steps 1 and 2 first |
| `main has commits that dev lacks` | a hotfix landed on `main`; back-merge it into `dev` first (section 8) |
| `tag already exists` | pick the next version; never move or delete tags |
| `must be run by the human` | expected for agents; hand off to the human |

## 8. Hotfix (urgent fix to a released version)

```
git switch main && git pull
git switch -c hotfix/fix-opus-crash
# fix, test, commit (Conventional Commits); regenerate the changelog for the patch version:
git cliff --tag v0.2.1 --output CHANGELOG.md && git add CHANGELOG.md && git commit -m "chore(release): v0.2.1"
scripts/land.sh hotfix/fix-opus-crash --into main     # merge commit into main after all gates and CI
# HUMAN ONLY, interactive terminal:
scripts/release.sh tag 0.2.1                          # tags main's tip; triggers the release workflow
# Then bring the fix back into dev:
git switch dev && git pull && git switch -c chore/back-merge-v0.2.1
git merge main
scripts/land.sh chore/back-merge-v0.2.1
```

## 9. External pull requests (maintainer)

Contributors open PRs against `dev` from forks. In the GitHub UI check that `gate` is green, review the diff, and use **Create a merge commit**. Do not use the landing script for others' work. Details and scaling notes: [git-workflow.md section 8](git-workflow.md#8-external-contributors).

## 10. Never

- Commit to `main`; commit to `dev` beyond the single tiny change; force-push; rewrite published history.
- Use `--no-verify`, edit or delete `.githooks/`, or change hook or identity configuration.
- Create tags by hand, or run `release.sh publish`/`tag` as an agent.
- Add AI attribution to commits, PRs, or docs.
- Merge or push to `dev`/`main` by any route other than `scripts/land.sh` and `scripts/release.sh`.
- Commit, merge, rebase or move refs on an `integration/` branch by hand (use `scripts/land.sh --into` and `--update`).

## 11. Changing the web portal

The portal is TypeScript and Preact in `portal/`; the page that ships is the generated, committed `crates/core/src/ui.html` ([ADR-0016](../adr/0016-portal-typescript-stack.md)). Rust-only work never needs Node.

```
cd portal && npm ci --ignore-scripts      # once (Node 24, see .nvmrc)
npm run build                             # regenerates crates/core/src/ui.html; commit it with the source change
npm run check                             # type-check, unit tests, and "the committed page is current"
npm run test:browser                      # headless Chrome against a mock device (CHROME=/path if not found)
```

`scripts/check.sh` runs the portal steps only when `portal/` or `ui.html` changed, and CI always runs them. If CI says the page is out of date, run `npm run build` and commit the result. Never edit `ui.html` by hand.
