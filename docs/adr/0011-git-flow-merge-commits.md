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

Option 3. Branches: `main` (releases), `dev` (integration), and `feature|bugfix|hotfix|release|docs|chore|spike/<name>`. All integration is by merge commit through `scripts/land.sh`; releases fast-forward `main` from a verified `release/X.Y.Z` branch and are tagged by `scripts/release.sh`. A single tiny direct commit to `dev` (at most 10 lines in 2 files) is the only exception to the branch rule. Conventional Commits drive the SemVer bump and a generated changelog (git-cliff) that ships in the release. Commits are authored by the human only; AI attribution and identities are rejected. Enforcement is layered: agent guard, git hooks, landing scripts, CI, and server rulesets. External contributors use PRs into `dev`.

## Consequences

**Positive:** matches the maintainer's workflow; real history; server-verified gates without PRs; agents are constrained by several independent layers; releases always carry a changelog.
**Negative:** landing waits for a CI round trip once a remote exists; `release.sh` and the remote-CI path of `land.sh` are not exercised until the first remote and release; merge-heavy history is noisier than squashed history; local hooks can be bypassed (server rules and CI are the backstop).
**Follow-ups:** apply rulesets when the GitHub repository is created ([repo-settings.md](../process/repo-settings.md)); exercise the remote path and `release.sh` on a throwaway repository first; enable signed commits once keys exist.
