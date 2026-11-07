#!/usr/bin/env bash
# Land work with a merge commit, only after every gate passes. This is the ONLY way work reaches dev or main.
#   scripts/land.sh <branch>                  merge a working branch into dev (merge commit)
#   scripts/land.sh --tiny                    publish ONE tiny commit made directly on dev
#   scripts/land.sh <release|hotfix>/<ver> --into main    fast-forward main to a release branch
#
# The merge commit's tree is identical to the tested branch tip because the branch must already
# contain dev. With a remote, the exact commit is pushed to a candidate ref, CI must pass on it, and
# only then is it pushed to the protected branch. Without a remote it is a local merge after local checks.
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

sync_ff() { # fast-forward a local branch from its remote counterpart, if any
  if has_remote && git rev-parse --verify -q "origin/$1" >/dev/null; then
    git checkout -q "$1"
    git merge --ff-only -q "origin/$1" || die "local $1 has diverged from origin/$1"
  fi
}

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

[ -n "$source_branch" ] || die "usage: scripts/land.sh <branch> [--into dev|main] | --tiny"
git rev-parse --verify -q "$source_branch" >/dev/null || die "no such branch: $source_branch"

case "$target" in
  dev)
    valid_branch_name "$source_branch" || die "'$source_branch' is not a valid working branch name"
    sync_ff dev
    git merge-base --is-ancestor dev "$source_branch" ||
      die "$source_branch does not contain dev. Update it first:  git switch $source_branch && git merge dev"
    git checkout -q "$source_branch"
    scripts/check.sh
    git checkout -q dev
    prev=$(git rev-parse dev)
    CLAWDOT_LANDING=1 git merge --no-ff -m "Merge $source_branch into dev" "$source_branch"
    if ! publish dev; then
      git reset -q --hard "$prev"
      die "CI failed; dev was not published and has been reset to its previous commit"
    fi
    git branch -d "$source_branch"
    if has_remote; then git push --quiet origin --delete "$source_branch" 2>/dev/null || true; fi
    echo "land: $source_branch -> dev done"
    ;;
  main)
    case "$source_branch" in release/* | hotfix/*) ;; *) die "only release/* or hotfix/* branches may be landed into main" ;; esac
    ver="${source_branch#*/}"
    sync_ff main
    git merge-base --is-ancestor main "$source_branch" || die "main is not an ancestor of $source_branch (it must fast-forward)"
    git checkout -q "$source_branch"
    scripts/check.sh --release "$ver"
    git checkout -q main
    prev=$(git rev-parse main)
    CLAWDOT_LANDING=1 git merge --ff-only -q "$source_branch"
    if ! publish main; then
      git reset -q --hard "$prev"
      die "CI failed; main was not published and has been reset"
    fi
    echo "land: $source_branch -> main done (run scripts/release.sh finish $ver to tag and back-merge)"
    ;;
  *) die "--into must be dev or main" ;;
esac
