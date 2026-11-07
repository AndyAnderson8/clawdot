#!/usr/bin/env bash
# Land work with a MERGE COMMIT, only after every gate passes. The only way work reaches dev (or a hotfix reaches main).
#   scripts/land.sh <branch>                       merge a working branch into dev
#   scripts/land.sh --tiny                         publish ONE tiny commit made directly on dev
#   scripts/land.sh hotfix/<name> --into main      merge a hotfix into main (a human then tags it)
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
while [ $# -gt 0 ]; do
  case "$1" in
    --into) target="${2:-}"; shift ;;
    --tiny) tiny=1 ;;
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
  publish dev || die "CI failed for the tiny commit; dev was not published"
  echo "land: tiny commit landed"
  exit 0
fi

[ -n "$source_branch" ] || die "usage: scripts/land.sh <branch> [--into main (hotfix only)] | --tiny"
git rev-parse --verify -q "$source_branch" >/dev/null || die "no such branch: $source_branch"

case "$target" in
  dev)
    valid_branch_name "$source_branch" || die "'$source_branch' is not a valid working branch name"
    sync_ff dev
    git merge-base --is-ancestor dev "$source_branch" ||
      die "$source_branch does not contain dev. Update it first:  git switch $source_branch && git merge dev"
    git checkout -q "$source_branch"
    scripts/check.sh
    merge_into dev "$source_branch" "Merge $source_branch into dev" ||
      die "CI or publishing failed; dev was reset to its previous commit and $source_branch is untouched"
    git branch -d "$source_branch"
    if has_remote; then git push --quiet origin --delete "$source_branch" 2>/dev/null || true; fi
    echo "land: $source_branch -> dev done"
    ;;
  main)
    case "$source_branch" in
      hotfix/*) ;;
      *) die "only hotfix/* branches are landed into main here. Releases: scripts/release.sh prepare, land the release branch into dev, then the human runs scripts/release.sh publish X.Y.Z" ;;
    esac
    sync_ff main
    git merge-base --is-ancestor main "$source_branch" ||
      die "$source_branch does not contain main. Update it first:  git switch $source_branch && git merge main"
    git checkout -q "$source_branch"
    scripts/check.sh
    merge_into main "$source_branch" "Merge $source_branch into main" ||
      die "CI or publishing failed; main was reset to its previous commit"
    git branch -d "$source_branch"
    echo "land: $source_branch -> main done."
    echo "next (human): scripts/release.sh tag X.Y.Z   then back-merge main into dev on a chore/ branch (see docs/process/runbook.md)"
    ;;
  *) die "--into must be dev or main" ;;
esac
