# ADR-0018: Landing gate mode: a temporary stabilization mode for dev

- **Status:** Proposed
- **Date:** 2026-11-09
- **Deciders:** Maintainer

## Context

[ADR-0011](0011-git-flow-merge-commits.md) makes every landing into `dev` run the full gate (`scripts/check.sh`) and then wait for CI on a candidate ref. While the codebase is still changing quickly and nothing has been released, the CI wait is the slowest step of every landing and several agents land in sequence. The maintainer decided that during this stabilization period landing on `dev` should run only the fast local check and should not wait for CI. The full gate comes back once things are stable.

This must be an explicit, documented, reversible mode, not a bypass: the policy stays visible in the repository, the script says out loud when it skipped something, and the strict behaviour is one committed line away.

## Decision drivers

- Faster landings now, without hiding that a gate was skipped.
- Reversible by a one-line, reviewable change; no scripts to restore.
- `main`, hotfixes and releases keep the full gate and required CI.
- No new way for a branch to weaken the gate for itself.

## Options considered

1. **Keep the full gate everywhere.** Safe, but slow for work that has no users yet.
2. **Ad hoc `--skip-ci` flag or environment variable.** Easy to leave on and invisible in history. Rejected.
3. **A committed mode file read by `scripts/land.sh`.** The mode is a reviewed change in history and is printed on every landing. Chosen.

## Decision

`scripts/gate-mode` holds one word, read by `scripts/land.sh` from the committed copy on local `dev` (so a branch cannot change the mode it is landed under; the change that flips the mode lands under the mode in force before it). A missing file means `full`. Any other value is an error.

- `full`: unchanged behaviour (full `scripts/check.sh`, candidate ref, wait for CI, push, verify).
- `stabilization`: run `scripts/check.sh --fast --tests` against `dev` and the commit-message scan on `dev..<branch>`, create the merge commit, push `dev` directly, verify `origin/dev`, and print `FULL GATE SKIPPED BY MODE 'stabilization'`. No candidate ref, no CI wait. The exit-code contract (3, 4, 5, 6) is unchanged; a server that rejects the push still gives exit 4.

The mode applies only to landings into `dev` (a branch, an `integration/` batch, `--tiny`). Hotfixes into `main`, `scripts/release.sh` and the release gate always run the full gate and CI. CI itself still runs on `dev` and on pushes and pull requests as before; it just no longer blocks the landing.

The default for this repository is `stabilization`.

## Exit criteria

Switch `scripts/gate-mode` to `full` (a normal `chore/` branch) at the earliest of:

1. before the first release (`scripts/release.sh prepare` must not be run while the mode is `stabilization`);
2. when the maintainer judges the codebase stable, for example the roadmap milestone for the current work is complete and CI on `dev` has been green for a sustained stretch;
3. immediately, if a landed change breaks `dev` in a way the fast check missed twice.

When the mode flips back, the ruleset note in [repo-settings.md](../process/repo-settings.md) applies in reverse.

## Consequences

**Positive:**
- Landings take the time of the fast check only.
- The skipped gate is announced on every landing and is visible in history as a one-line file change.

**Negative / costs:**
- A change can reach `dev` that fails the full gate (tests of untouched packages, `cargo-deny`, the policy tests, the history scan, budget). CI still runs on `dev` after the fact, so breakage is found but not prevented.
- The server-side `dev` ruleset that requires `gate` on the pushed SHA rejects the direct push. While the mode is `stabilization` the maintainer must relax that rule for `dev` (it is not applied yet; see repo-settings.md). `main` protection is untouched.

**Follow-ups:**
- Flip to `full` per the exit criteria and accept this ADR then, or supersede it.
