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

What it does: runs all gates on your branch, creates a **merge commit** on `dev`, and (once a remote exists) pushes that exact commit to a CI candidate branch, waits for CI to pass, then publishes it. Your branch is deleted afterward.

If it fails:

| Message | Fix |
|---|---|
| `does not contain dev` | `git switch <branch> && git merge dev`, then land again |
| `working tree is not clean` | commit or stash your changes |
| a gate fails | fix it on your branch, commit, land again |
| `CI failed ... dev was reset` | open the failing CI run, fix on your branch, land again. `dev` is unchanged. |
| `gh CLI is required` | install and sign in to GitHub's CLI (`gh auth login`) |

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
