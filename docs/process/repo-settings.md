# Repository Settings (GitHub)

To apply when the GitHub repository is created. They make the server enforce the policy in [git-workflow.md](git-workflow.md). `scripts/setup-github.sh` applies them with the `gh` CLI (a human runs it once, authenticated as the repository admin). Status: **not yet applied** (no remote exists). The ruleset payloads below are written from the GitHub API documentation and have not been exercised; verify each in the UI the first time.

## General
- Default branch: **`dev`** (so Dependabot and contributor PRs target it). `main` holds releases.
- Merge options: allow **merge commits**; disable squash and rebase merging (PRs from external contributors still produce merge commits). Automatically delete head branches.
- Actions: allow GitHub Actions. Workflow permissions default to read-only.
- Secret scanning and push protection: on. Dependabot alerts: on. Private vulnerability reporting: on.

## Rulesets

### `main`
- Target: `refs/heads/main`.
- Block deletion. Block force pushes (non-fast-forward).
- Require status check **`gate`** to pass for the pushed commit.
- No pull-request rule: the maintainer fast-forwards `main` from a CI-verified release branch, and any commit without a passing `gate` is rejected by the server, which is what makes direct commits to `main` impossible.
- Later: require signed commits once signing keys are configured.

### `dev`
- Target: `refs/heads/dev`.
- Block deletion. Block force pushes.
- Require status check **`gate`** for the pushed commit. `scripts/land.sh` pushes a candidate ref first so the exact SHA already has a passing `gate` when it is pushed to `dev`.
- Do not require "up to date before merging" (the landing script guarantees the branch contains `dev`).

### Release tags
- Target: `refs/tags/v*`. Block deletion and updates (tags are immutable).

## Checks and names
The required context is the job named `gate` in `.github/workflows/ci.yml`. If the check name shown in the UI differs, update the ruleset, not the workflow.

## Secrets
None required for CI today. A release workflow uses the default `GITHUB_TOKEN` with `contents: write`.

## Verification checklist after applying
1. Direct push of a commit to `main` is rejected.
2. Direct push of an untested commit to `dev` is rejected.
3. `scripts/land.sh <branch>` publishes through a candidate ref and succeeds.
4. Force-push to `dev`/`main` is rejected.
5. Deleting a `v*` tag is rejected.
