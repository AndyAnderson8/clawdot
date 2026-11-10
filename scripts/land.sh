#!/usr/bin/env bash
# Land work with a MERGE COMMIT, only after every gate passes. The only way work reaches dev (or a hotfix reaches main).
#   scripts/land.sh <branch> [--wait SECONDS]      merge a working branch (or integration/<name>) into dev
#   scripts/land.sh --tiny                         publish ONE tiny commit made directly on dev
#   scripts/land.sh hotfix/<name> --into main      merge a hotfix into main (a human then tags it)
# Batching (docs/process/git-workflow.md section 5a): collect several branches locally, land them once.
#   scripts/land.sh --new-integration <name>       create integration/<name> from up-to-date dev (= scripts/new-integration.sh)
#   scripts/land.sh <branch> --into integration/<name>
#                                                  LOCAL merge commit into the integration branch after the FAST check
#                                                  (check.sh --fast --tests). Never pushes, never runs CI. Deletes <branch>
#                                                  only after the merge. <branch> must contain the integration branch.
#   scripts/land.sh --update integration/<name>    merge the current dev into the integration branch (locally, same rules)
#   scripts/land.sh integration/<name>             land the whole batch into dev: contains dev, FULL check, CI on the exact
#                                                  merge, push, verify, clean up. Exactly like a single branch.
# The integration branch is never checked out by these steps: the merge is made in a temporary detached worktree and the
# branch ref is moved with a compare-and-swap, so agents batching into the same branch never block each other.
# --wait SECONDS: if another worktree holds the target branch, retry for up to SECONDS (default 0 = fail at once).
# Exit codes: 0 landed (and verified on origin when the target is dev/main); 1 other error; 3 target branch held by
#   another worktree; 4 merge exists only locally (push rejected or unverified; nothing deleted, recovery printed);
#   5 CI or publishing failed (target restored, or exact recovery printed); 6 the integration branch moved underneath
#   the local merge (nothing changed; merge it into your branch and land again).
# Gate mode (scripts/gate-mode, read from committed dev; ADR-0018): `full` runs the full gate and CI as above.
#   `stabilization` runs only the fast local check (check.sh --fast --tests) and the commit-message scan, merges, pushes
#   dev directly (no candidate ref, no CI wait) and prints a "FULL GATE SKIPPED BY MODE" line. It applies to landing
#   into dev only; hotfixes into main and releases always run the full gate and CI. Exit codes are unchanged.
# Releases are NOT landed here: use scripts/release.sh (prepare, then publish).
#
# The merge commit's tree equals the tested branch tip because the branch must already contain its
# target. With a remote, the exact merge commit is pushed to a candidate ref, CI must pass on it, and only
# then is it pushed to the protected branch. Without a remote it is a local merge after local checks.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
# shellcheck source=scripts/lib-git.sh
. scripts/lib-git.sh

source_branch=""
target="dev"
tiny=0
update=0
wait_secs=0
while [ $# -gt 0 ]; do
  case "$1" in
    --into) target="${2:-}"; shift ;;
    --tiny) tiny=1 ;;
    --update) update=1 ;;
    --new-integration) exec scripts/new-integration.sh "${2:-}" ;;
    --wait) wait_secs="${2:-}"; shift; [[ "$wait_secs" =~ ^[0-9]+$ ]] || die "--wait needs a number of seconds" ;;
    -*) die "unknown option $1" ;;
    *) source_branch="$1" ;;
  esac
  shift
done

[ -z "$(git status --porcelain)" ] || die "working tree is not clean"
# Batching into an integration branch is local by design: no fetch needed, nothing leaves this machine.
if has_remote && ! is_integration "$target" && [ "$update" -eq 0 ]; then git fetch --quiet --prune origin; fi

if [ "$tiny" -eq 1 ]; then
  [ "$(current_branch)" = "dev" ] || die "--tiny must be run on dev"
  [ "$(unlanded_commits)" -eq 1 ] || die "--tiny needs exactly one unlanded commit on dev"
  read -r lines files < <(diff_stats HEAD~1 HEAD)
  [ "$lines" -le "$TINY_MAX_LINES" ] && [ "$files" -le "$TINY_MAX_FILES" ] ||
    die "that commit is not tiny ($lines lines, $files files; max $TINY_MAX_LINES/$TINY_MAX_FILES). Move it to a branch."
  mode=$(gate_mode) || exit 1
  if [ "$mode" = stabilization ]; then
    gate_skip_notice
    CHECK_BASE=HEAD~1 scripts/check.sh --fast --tests || exit 1
    scripts/check-commit-msg.sh --range HEAD~1..HEAD || exit 1
  else
    scripts/check.sh
  fi
  rc=0
  if [ "$mode" = stabilization ]; then publish dev direct || rc=$?; else publish dev || rc=$?; fi
  case "$rc" in
    0) ;;
    2)
      echo "error: NOT LANDED. The tiny commit exists only in your LOCAL dev; origin/dev does not contain it. See the message above." >&2
      exit "$EX_LOCAL_ONLY"
      ;;
    *)
      echo "error: CI or publishing failed for the tiny commit; it is still only on local dev" >&2
      exit "$EX_CI_FAILED"
      ;;
  esac
  echo "land: tiny commit landed"
  exit 0
fi

[ -n "$source_branch" ] || die "usage: scripts/land.sh <branch> [--into integration/<name> | --into main (hotfix only)] | --update integration/<name> | --new-integration <name> | --tiny"
git rev-parse --verify -q "refs/heads/$source_branch" >/dev/null || die "no such branch: $source_branch"

# --update: bring dev into an integration branch (locally), so it can later be landed.
if [ "$update" -eq 1 ]; then
  is_integration "$source_branch" || die "--update takes an integration/<name> branch"
  if has_remote; then git fetch --quiet --prune origin; fi
  devref=dev
  if has_remote && git rev-parse -q --verify refs/remotes/origin/dev >/dev/null; then
    git merge-base --is-ancestor dev origin/dev || die "local dev has commits that origin/dev lacks; land them first"
    devref=origin/dev
  fi
  if git merge-base --is-ancestor "$devref" "$source_branch"; then
    echo "land: $source_branch already contains $devref"
    exit 0
  fi
  wait_for_branch_free "$source_branch" "$wait_secs" || exit "$EX_HELD"
  rc=0
  merge_into_integration "$source_branch" "$devref" "Merge dev into $source_branch" update || rc=$?
  [ "$rc" -eq 0 ] || exit "$rc"
  echo "land: dev merged into $source_branch (local only; now at $(git rev-parse --short "$source_branch"))"
  exit 0
fi

case "$target" in
  integration/*)
    is_integration "$target" || die "'$target' is not a valid integration branch name (integration/<kebab-name>)"
    batchable_branch_name "$source_branch" ||
      die "'$source_branch' cannot be batched: only feature/, bugfix/, docs/, chore/ and spike/ branches go into an integration branch"
    git rev-parse -q --verify "refs/heads/$target" >/dev/null ||
      die "no such branch: $target. Create it with:  scripts/new-integration.sh ${target#integration/}"
    stip=$(git rev-parse "refs/heads/$source_branch")
    wait_for_branch_free "$target" "$wait_secs" || exit "$EX_HELD"
    rc=0
    merge_into_integration "$target" "$source_branch" "Merge $source_branch into $target" || rc=$?
    [ "$rc" -eq 0 ] || exit "$rc"
    # Only after the merge: free the source branch. A branch checked out here is detached first (same commit).
    git merge-base --is-ancestor "$stip" "$target" || die "internal: $source_branch is not contained in $target; branch kept"
    if [ "$(current_branch)" = "$source_branch" ]; then git checkout -q --detach; fi
    holder=$(branch_holder "$source_branch")
    if [ -n "$holder" ]; then
      echo "land: $source_branch is checked out in $holder; merged, but the branch was kept there (delete it with: git branch -D $source_branch)"
    else
      git branch -D "$source_branch" >/dev/null
    fi
    echo "land: $source_branch -> $target done (local only; no push, no CI). $target is now at $(git rev-parse --short "$target")"
    echo "      Land the batch into dev with:  scripts/land.sh $target"
    ;;
  dev)
    valid_branch_name "$source_branch" || die "'$source_branch' is not a valid working branch name"
    wait_for_branch_free dev "$wait_secs" || exit "$EX_HELD"
    sync_ff dev
    mode=$(gate_mode) || exit 1
    if ! git merge-base --is-ancestor dev "$source_branch"; then
      if is_integration "$source_branch"; then
        die "$source_branch does not contain dev. Update it first (no checkout needed):  scripts/land.sh --update $source_branch"
      fi
      die "$source_branch does not contain dev. Update it first:  git switch $source_branch && git merge dev"
    fi
    if is_integration "$source_branch"; then
      # Integration branches are never held: check the exact tip out detached.
      git checkout -q --detach "$source_branch"
    else
      git checkout -q "$source_branch"
    fi
    if [ "$mode" = stabilization ]; then
      gate_skip_notice
      CHECK_BASE=dev scripts/check.sh --fast --tests || die "the fast local check failed"
      scripts/check-commit-msg.sh --range "dev..$source_branch" || die "the commit message check failed"
    else
      scripts/check.sh
    fi
    # The full gate can take a long time. Re-check, right before merging, that dev is still free and that this
    # branch still contains it (another landing may have happened meanwhile).
    git checkout -q --detach
    if has_remote; then git fetch --quiet --prune origin; fi
    wait_for_branch_free dev "$wait_secs" || exit "$EX_HELD"
    sync_ff dev
    if ! git merge-base --is-ancestor dev "$source_branch"; then
      die "dev moved while the checks ran, so $source_branch no longer contains it. Merge dev into the branch (integration branches: scripts/land.sh --update $source_branch) and land again"
    fi
    [ "$(gate_mode)" = "$mode" ] || die "scripts/gate-mode changed on dev while the checks ran; land again so the right gate applies"
    pmode=ci
    [ "$mode" = stabilization ] && pmode=direct
    merge_into dev "$source_branch" "Merge $source_branch into dev" "$pmode" || exit $?
    git branch -d "$source_branch" || echo "land: merged, but could not delete $source_branch here (checked out elsewhere?); delete it with: git branch -D $source_branch"
    if has_remote; then git push --quiet origin --delete "$source_branch" 2>/dev/null || true; fi
    echo "land: $source_branch -> dev done"
    ;;
  main)
    case "$source_branch" in
      hotfix/*) ;;
      *) die "only hotfix/* branches are landed into main here. Releases: scripts/release.sh prepare, land the release branch into dev, then the human runs scripts/release.sh publish X.Y.Z" ;;
    esac
    wait_for_branch_free main "$wait_secs" || exit "$EX_HELD"
    sync_ff main
    git merge-base --is-ancestor main "$source_branch" ||
      die "$source_branch does not contain main. Update it first:  git switch $source_branch && git merge main"
    git checkout -q "$source_branch"
    scripts/check.sh
    merge_into main "$source_branch" "Merge $source_branch into main" || exit $?
    git branch -d "$source_branch"
    echo "land: $source_branch -> main done."
    echo "next (human): scripts/release.sh tag X.Y.Z   then back-merge main into dev on a chore/ branch (see docs/process/runbook.md)"
    ;;
  *) die "--into must be dev, main or integration/<name>" ;;
esac
