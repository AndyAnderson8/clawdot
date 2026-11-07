#!/usr/bin/env bash
# Shared helpers for ClawDot git-flow scripts and hooks. Source this file; do not execute it.
# Policy: docs/process/git-workflow.md

die() { echo "error: $*" >&2; exit 1; }

current_branch() { git symbolic-ref --quiet --short HEAD 2>/dev/null || echo "(detached)"; }

is_protected() { case "$1" in main | dev) return 0 ;; *) return 1 ;; esac; }

# Allowed working-branch names (git flow).
valid_branch_name() {
  [[ "$1" =~ ^(feature|bugfix|hotfix|release|docs|chore|spike)/[a-z0-9][a-z0-9._-]*$ ]]
}

# diff_stats <git diff args...>: prints "<changed lines> <files>"; binary files count as 999 lines.
diff_stats() {
  git diff --numstat "$@" | awk '
    { f++; if ($1 == "-" || $2 == "-") l += 999; else l += $1 + $2 }
    END { printf "%d %d\n", l, f }'
}

# Number of non-merge commits on the current branch that have not been landed yet.
unlanded_commits() {
  local base
  if base=$(git rev-parse --verify -q '@{u}' 2>/dev/null); then
    git rev-list --count --no-merges "$base"..HEAD
    return
  fi
  base=$(git log --merges -1 --format=%H 2>/dev/null)
  [ -n "$base" ] || base=$(git merge-base main HEAD 2>/dev/null || true)
  [ -n "$base" ] || { echo 0; return; }
  git rev-list --count --no-merges "$base"..HEAD
}

# The tiny-commit exception for dev: ONE commit, at most 10 changed lines in at most 2 files.
TINY_MAX_LINES=10
TINY_MAX_FILES=2
tiny_staged_ok() {
  local lines files
  read -r lines files < <(diff_stats --cached)
  [ "$files" -ge 1 ] && [ "$files" -le "$TINY_MAX_FILES" ] && [ "$lines" -le "$TINY_MAX_LINES" ] \
    && [ "$(unlanded_commits)" -eq 0 ]
}

has_remote() { git remote get-url origin >/dev/null 2>&1; }

# publish <branch>: make a locally integrated branch public ONLY after CI passed on the exact commit.
# The commit is first pushed to a candidate ref (ci/<branch>/<sha>) that triggers CI. When the
# required `gate` check is green, the same commit is pushed to the protected branch, which the
# server accepts because that SHA already has passing checks.
# With no remote configured (early development) this is a local-only no-op.
publish() {
  local br="$1" sha cand id i
  has_remote || { echo "land: no remote configured; integrated locally only (CI gate applies once a remote exists)"; return 0; }
  command -v gh >/dev/null 2>&1 || { echo "land: the gh CLI is required to verify CI before publishing $br" >&2; return 1; }
  sha=$(git rev-parse "$br")
  cand="ci/$br/${sha:0:10}"
  CLAWDOT_LANDING=1 git push origin "$sha:refs/heads/$cand"
  echo "land: waiting for CI on $cand ..."
  id=""
  for i in $(seq 1 36); do
    id=$(gh run list --branch "$cand" --workflow ci --json databaseId,headSha \
      -q ".[] | select(.headSha==\"$sha\") | .databaseId" | head -1)
    [ -n "$id" ] && break
    sleep 5
  done
  if [ -z "$id" ]; then
    git push origin --delete "$cand" || true
    return 1
  fi
  if gh run watch "$id" --exit-status; then
    CLAWDOT_LANDING=1 git push origin "$br"
    git push origin --delete "$cand" || true
  else
    git push origin --delete "$cand" || true
    return 1
  fi
}

# sync_ff <branch>: fast-forward a local branch from origin (no-op without a remote).
sync_ff() {
  if has_remote && git rev-parse --verify -q "origin/$1" >/dev/null; then
    git checkout -q "$1"
    git merge --ff-only -q "origin/$1" || die "local $1 has diverged from origin/$1"
  fi
}

# merge_into <target> <source> <message>: create a MERGE COMMIT on <target> (never a fast-forward),
# then publish it. If publishing fails, <target> is reset to where it was, so nothing unverified lingers.
merge_into() {
  local target="$1" source="$2" msg="$3" prev
  git checkout -q "$target"
  prev=$(git rev-parse "$target")
  CLAWDOT_LANDING=1 git merge --no-ff -q -m "$msg" "$source"
  if [ "$(git rev-parse "$target")" = "$prev" ]; then
    die "nothing to merge: $target already contains $source"
  fi
  if ! publish "$target"; then
    git reset -q --hard "$prev"
    return 1
  fi
}
