# ADR-0011: Git flow with merge commits and gated landing

- **Status:** Accepted
- **Date:** 2026-11-07
- **Deciders:** Maintainer
- **Supersedes:** the trunk-based, squash-merge, PR-required conventions in the first draft of `docs/process/git-workflow.md`

## Context

The maintainer works with AI coding agents as a pair programmer. Pull requests between two collaborators who are working together add ceremony without review value. The maintainer prefers git flow (feature and bugfix branches into an integration branch, releases onto `main`), merge commits that preserve real history, frequent small commits, and strict gates: nothing lands unless formatting, lint, type-check, and tests pass. Agents must be unable to author commits as themselves or to bypass the process.

## Decision drivers

- Preserve real, regular commit history (no squashing)
- Strong quality gates before integration and before release
- No PR ceremony between the maintainer and their agents
- Enforcement that holds even for tools that ignore instructions
- A changelog that ships inside every release

## Options considered

1. **Trunk-based, squash-merge PRs (first draft).** Simple and server-enforced, but squashes history and requires PRs.
2. **Git flow with direct merges, relying on local discipline only.** Matches the preference but is bypassable by anyone or anything.
3. **Git flow with merge commits, landed through a script that pushes the exact commit to a CI candidate ref first.** Keeps merge commits and no PRs, and makes the server enforce the gate because the pushed SHA already has a passing check.
4. **Git flow with auto-merging PRs.** Server-enforced and merge commits, but reintroduces PRs the maintainer does not want.

## Decision

Option 3. Branches: `main` (releases), `dev` (integration), and `feature|bugfix|hotfix|release|docs|chore|spike/<name>`. All integration is by merge commit through `scripts/land.sh`. A release is prepared on a `release/X.Y.Z` branch (changelog and version), landed into `dev`, and then published by the human: `scripts/release.sh publish` merges `dev` into `main` with a **merge commit** (pushed only after CI passes on that exact commit), tags that merge commit, and pushes the tag, which triggers the release workflow. Hotfixes merge into `main` from `hotfix/*` and are back-merged into `dev`. (Amended before first publication: the first draft fast-forwarded `main` from the release branch because merge commits on `main` seemed untestable. The candidate-ref technique used for `dev` works for `main` too.) A single tiny direct commit to `dev` (at most 10 lines in 2 files) is the only exception to the branch rule. Conventional Commits drive the SemVer bump and a generated changelog (git-cliff) that ships in the release. Commits are authored by the human only; AI attribution and identities are rejected. Enforcement is layered: agent guard, git hooks, landing scripts, CI, and server rulesets. External contributors use PRs into `dev`.

**Amendment (2026-11-09): fast checks at commit time, full gate at landing and in CI.** Running the whole gate (including the full test run) in the `pre-commit` hook made small, frequent commits slow, which works against the "commit small and often" rule. The hook now runs `scripts/check.sh --fast` on the staged files only: shellcheck for changed shell scripts, the docs check when Markdown changed, and `cargo fmt --check` plus `cargo clippy` (scoped to the affected packages) when Rust changed. Docs-only commits skip Rust entirely. Tests, `cargo-deny`, the policy tests and the history scan moved out of the commit path. Nothing else changes: `scripts/land.sh` and CI still run the complete gate before work reaches `dev` or `main`, and the branch-name, commit-message, identity and protected-branch hooks are unchanged. The original decision above is kept as written.

**Amendment (2026-11-09): landing reports only what origin confirms.** `scripts/land.sh` once printed "done" and deleted the source branch although the final push of `dev` was rejected, and mishandled `dev` being held by another worktree. Now success is claimed only after a fetch shows `origin/dev` contains the merge commit. A rejected or unverified push exits 4 and keeps the branch, the candidate ref and the local merge (recovery command printed, no automatic reset); a held target exits 3 up front, naming the worktree (`--wait SECONDS` retries, default off); a CI failure restores the target and exits 5, or prints the exact manual recovery. No gate is weakened: the full check, CI on the exact merge SHA and push-after-CI are unchanged.

**Amendment (2026-11-09): integration branches batch a milestone, with one CI round trip.** Landing every small branch into `dev` costs a full gate and a CI wait each time, and several agents working on one milestone queue behind each other for `dev`. The maintainer's rule is: do not push while building, add all the features, run the tests when ready to merge, then push. A new branch type `integration/<name>` (created by `scripts/new-integration.sh <name>` or `scripts/land.sh --new-integration <name>` from an up-to-date `dev`) does that.

- **Batching is local.** `scripts/land.sh <branch> --into integration/<name>` (for `feature|bugfix|docs|chore|spike` branches only) requires the branch to contain the integration branch, runs the fast check on the merged tree (`check.sh --fast --tests`: shellcheck, docs, fmt, clippy and the tests of the touched packages, measured against the integration tip) plus the commit-message scan, makes a **merge commit** on the integration branch, and deletes the source branch only after that. It never pushes and never calls CI. `scripts/land.sh --update integration/<name>` merges the current `dev` into the batch the same way.
- **No long-held checkouts.** Only one worktree can check out a branch, so the merge is made in a temporary **detached** worktree and the branch ref is then moved with a compare-and-swap (`git update-ref <ref> <new> <old>`), which is a verified fast-forward by construction (the merge's first parent is the old tip). If the integration branch moved in the meantime nothing changes and the script exits 6 (the author merges the new tip into the branch and lands again). If a worktree has the branch checked out the script exits 3 (`--wait` retries), because moving a checked-out ref would corrupt that checkout. Agents batching into the same branch therefore never block each other beyond that rule.
- **Landing the batch is unchanged for `dev`.** `scripts/land.sh integration/<name>` requires the branch to contain `dev` (else it says to run `--update`), runs the **full** gate on the exact tip, makes the merge commit on `dev`, pushes that exact commit to the candidate ref, waits for CI, pushes `dev`, verifies `origin/dev` contains it, and only then deletes the integration branch. Exit codes 3, 4 and 5 and the held-`dev` detection and `--wait` behave as before. Nothing reaches `dev` or `main` without the full local gate and CI on the exact merge SHA; the server rulesets are untouched.
- **Integration branches are script-owned.** The branch-name check accepts `integration/<name>`, but the `pre-commit` hook and the agent guard refuse manual commits, merges, rebases and ref moves on them, the `pre-merge-commit` hook refuses merges without `scripts/land.sh`, and `pre-push` refuses force-pushes of them. Direct commits to `dev` and `main`, force-pushes, manual merges into `dev` and `main`, and pushes to `dev`/`main` stay blocked exactly as before. CI also runs on `integration/**` pushes, should anyone push one.
- **History.** Merge commits keep every branch visible: `git log --first-parent dev` shows one entry per batch, `git log --first-parent integration/<name>` (before landing) one per branch, and `git bisect` descends into the batch without losing the branch structure. A bad branch can be backed out of an unlanded batch only by recreating the batch; after landing, `git revert -m 1 <merge>` reverts a whole batch, and reverting one branch means reverting that branch's inner merge commit (`git revert -m 1 <inner merge>`). The cost is that a failure found at landing may come from any branch in the batch; batch a milestone, not a quarter. Single branches still land straight into `dev` as before and remain the default.
- **Left alone on purpose.** Remote copies of batched source branches are not deleted (batching never pushes or touches the remote); a human deletes any pushed copies by hand.

## Consequences

**Positive:** matches the maintainer's workflow; real history; server-verified gates without PRs; agents are constrained by several independent layers; releases always carry a changelog.
**Negative:** landing waits for a CI round trip once a remote exists; `release.sh` and the remote-CI path of `land.sh` are not exercised until the first remote and release; merge-heavy history is noisier than squashed history; local hooks can be bypassed (server rules and CI are the backstop).
**Follow-ups:** apply rulesets when the GitHub repository is created ([repo-settings.md](../process/repo-settings.md)); exercise the remote path and `release.sh` on a throwaway repository first; enable signed commits once keys exist.
