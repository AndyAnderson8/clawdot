#!/usr/bin/env bash
# Land work with a MERGE COMMIT, only after every gate passes. The only way work reaches dev (or a hotfix reaches main).
#   scripts/land.sh <branch> [--wait SECONDS]      merge a working branch into dev
#   scripts/land.sh --tiny                         publish ONE tiny commit made directly on dev
#   scripts/land.sh hotfix/<name> --into main      merge a hotfix into main (a human then tags it)
# --wait SECONDS: if another worktree holds the target branch, retry for up to SECONDS (default 0 = fail at once).
# Exit codes: 0 landed and verified on origin; 1 other error; 3 target branch held by another worktree;
#   4 merge exists only locally (push rejected or unverified; nothing deleted, recovery printed);
#   5 CI or publishing failed (target restored, or exact recovery printed).
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
wait_secs=0
while [ $# -gt 0 ]; do
  case "$1" in
    --into) target="${2:-}"; shift ;;
    --tiny) tiny=1 ;;
    --wait) wait_secs="${2:-}"; shift; [[ "$wait_secs" =~ ^[0-9]+$ ]] || die "--wait needs a number of seconds" ;;
    -*) die "unknown option $1" ;;
    *) source_branch="$1" ;;
  esac
  shift
done

[ -z "$(git status --porcelain)" ] || die "working tree is not clean"
if has_remote; then git fetch --quiet --prune origin; fi

if [ "$tiny" -eq 1 ]; then
  [ "$(current_branch)" = "dev" ] || die "--tiny must be run on dev"
  [ "$(unlanded_commits)" -eq 1 ] || die "--tiny needs exactly one unlanded commit on dev"
  read -r lines files < <(diff_stats HEAD~1 HEAD)
  [ "$lines" -le "$TINY_MAX_LINES" ] && [ "$files" -le "$TINY_MAX_FILES" ] ||
    die "that commit is not tiny ($lines lines, $files files; max $TINY_MAX_LINES/$TINY_MAX_FILES). Move it to a branch."
  scripts/check.sh
  rc=0
  publish dev || rc=$?
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

[ -n "$source_branch" ] || die "usage: scripts/land.sh <branch> [--into main (hotfix only)] | --tiny"
git rev-parse --verify -q "$source_branch" >/dev/null || die "no such branch: $source_branch"

case "$target" in
  dev)
    valid_branch_name "$source_branch" || die "'$source_branch' is not a valid working branch name"
    wait_for_branch_free dev "$wait_secs" || exit "$EX_HELD"
    sync_ff dev
    git merge-base --is-ancestor dev "$source_branch" ||
      die "$source_branch does not contain dev. Update it first:  git switch $source_branch && git merge dev"
    git checkout -q "$source_branch"
    scripts/check.sh
    merge_into dev "$source_branch" "Merge $source_branch into dev" || exit $?
    git branch -d "$source_branch"
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
  *) die "--into must be dev or main" ;;
esac
