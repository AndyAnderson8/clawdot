#!/usr/bin/env bash
# Shared helpers for ClawDot git-flow scripts and hooks. Source this file; do not execute it.
# Policy: docs/process/git-workflow.md

die() { echo "error: $*" >&2; exit 1; }

current_branch() { git symbolic-ref --quiet --short HEAD 2>/dev/null || echo "(detached)"; }

is_protected() { case "$1" in main | dev) return 0 ;; *) return 1 ;; esac; }

# Allowed branch names (git flow). integration/<name> batches several working branches before one landing.
valid_branch_name() {
  [[ "$1" =~ ^(feature|bugfix|hotfix|release|docs|chore|spike|integration)/[a-z0-9][a-z0-9._-]*$ ]]
}

# Integration branches change only through scripts/land.sh (--into / --update) and scripts/new-integration.sh.
is_integration() { [[ "$1" =~ ^integration/[a-z0-9][a-z0-9._-]*$ ]]; }

# Working branches that may be batched into an integration branch.
batchable_branch_name() { [[ "$1" =~ ^(feature|bugfix|docs|chore|spike)/[a-z0-9][a-z0-9._-]*$ ]]; }

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

# Exit codes of scripts/land.sh (documented in docs/process/runbook.md). 1 is any other error.
EX_HELD=3        # the target branch is checked out in another worktree
EX_LOCAL_ONLY=4  # the merge exists only in the local target branch; origin does not contain it
EX_CI_FAILED=5   # CI or the pre-publish steps failed; the target was restored (or recovery was printed)
EX_MOVED=6       # the integration branch moved underneath a local batching merge; nothing was changed

# branch_holder <branch>: path of ANOTHER worktree that has <branch> checked out (empty if none).
branch_holder() {
  local here path="" line
  here=$(git rev-parse --show-toplevel)
  while IFS= read -r line; do
    case "$line" in
      "worktree "*) path=${line#worktree } ;;
      "branch refs/heads/$1")
        if [ "$path" != "$here" ]; then echo "$path"; return 0; fi
        ;;
    esac
  done < <(git worktree list --porcelain)
}

# wait_for_branch_free <branch> <seconds>: succeed when no other worktree holds <branch>. Waits up to
# <seconds> (re-checking every LAND_WAIT_INTERVAL seconds, default 10); 0 means fail at once.
wait_for_branch_free() {
  local br="$1" limit="${2:-0}" deadline holder interval="${LAND_WAIT_INTERVAL:-10}"
  deadline=""
  while :; do
    holder=$(branch_holder "$br")
    [ -n "$holder" ] || return 0
    # The clock starts once the branch is first seen held, so a slow first check cannot eat the wait.
    [ -n "$deadline" ] || deadline=$((SECONDS + limit))
    if [ "$limit" -eq 0 ] || [ "$SECONDS" -ge "$deadline" ]; then
      echo "error: '$br' is checked out in another worktree: $holder" >&2
      echo "       Only one worktree can hold '$br'. Ask whoever uses it to release it (git -C \"$holder\" switch --detach)," >&2
      echo "       or retry with:  scripts/land.sh <branch> --wait SECONDS" >&2
      return 1
    fi
    echo "land: '$br' is held by $holder; retrying in ${interval}s ..."
    sleep "$interval"
  done
}

# publish <branch>: make a locally integrated branch public ONLY after CI passed on the exact commit.
# The commit is first pushed to a candidate ref (ci/<branch>/<sha>) that triggers CI. When the
# required `gate` check is green, the same commit is pushed to the protected branch, which the
# server accepts because that SHA already has passing checks. Success is claimed only after a fetch
# shows that origin/<branch> contains the commit.
# Returns 0 on success, 1 when nothing reached the protected branch (CI failed or never started), and
# 2 when CI passed but the push or its verification failed (the candidate ref is then kept).
# With no remote configured (early development) this is a local-only no-op.
publish() {
  local br="$1" sha cand id
  has_remote || { echo "land: no remote configured; integrated locally only (CI gate applies once a remote exists)"; return 0; }
  command -v gh >/dev/null 2>&1 || { echo "land: the gh CLI is required to verify CI before publishing $br" >&2; return 1; }
  sha=$(git rev-parse "$br")
  cand="ci/$br/${sha:0:10}"
  CLAWDOT_LANDING=1 git push origin "$sha:refs/heads/$cand" || return 1
  echo "land: waiting for CI on $cand ..."
  id=""
  for _ in $(seq 1 36); do
    id=$(gh run list --branch "$cand" --workflow ci --json databaseId,headSha \
      -q ".[] | select(.headSha==\"$sha\") | .databaseId" | head -1)
    [ -n "$id" ] && break
    sleep "${LAND_CI_POLL:-5}"
  done
  if [ -z "$id" ] || ! gh run watch "$id" --exit-status; then
    git push origin --delete "$cand" || true
    return 1
  fi
  if ! CLAWDOT_LANDING=1 git push origin "$br"; then
    echo "land: CI passed on $cand, but the push of $br to origin was rejected. Candidate ref $cand kept." >&2
    return 2
  fi
  if ! git fetch --quiet origin || ! git merge-base --is-ancestor "$sha" "origin/$br"; then
    echo "land: the push of $br reported success, but origin/$br does not contain $sha. Candidate ref $cand kept." >&2
    return 2
  fi
  echo "land: verified: origin/$br contains ${sha:0:10}"
  git push origin --delete "$cand" || true
}

# sync_ff <branch>: fast-forward a local branch from origin (no-op without a remote).
sync_ff() {
  if has_remote && git rev-parse --verify -q "origin/$1" >/dev/null; then
    git checkout -q "$1" || die "cannot switch to $1 (is another worktree holding it?)"
    git merge --ff-only -q "origin/$1" || die "local $1 has diverged from origin/$1"
  fi
}

# merge_into <target> <source> <message>: create a MERGE COMMIT on <target> (never a fast-forward),
# then publish it. Returns 0 on success, otherwise one of the EX_* codes after printing what happened:
#  - CI failed / not published: <target> is reset to where it was (verified; else exact manual steps).
#  - published but the push was rejected or unverified: the merge stays in local <target> and is NOT
#    reset automatically; the message says how to recover.
merge_into() {
  local target="$1" source="$2" msg="$3" prev merge rc=0
  if ! git checkout -q "$target"; then
    echo "error: cannot switch to $target (is another worktree holding it?)" >&2
    return "$EX_HELD"
  fi
  [ "$(current_branch)" = "$target" ] || { echo "error: not on $target after switching" >&2; return "$EX_HELD"; }
  prev=$(git rev-parse "$target")
  CLAWDOT_LANDING=1 git merge --no-ff -q -m "$msg" "$source"
  if [ "$(git rev-parse "$target")" = "$prev" ]; then
    die "nothing to merge: $target already contains $source"
  fi
  merge=$(git rev-parse "$target")
  publish "$target" || rc=$?
  case "$rc" in
    0) return 0 ;;
    2)
      {
        echo "error: NOT LANDED. The merge commit ${merge:0:10} exists only in your LOCAL $target; origin/$target does not contain it."
        echo "       Nothing was deleted: $source and the CI candidate ref are kept, and local $target was not reset."
        echo "       Find out why the push was refused (see the message above), then recover local $target with:"
        echo "         git switch $target && git reset --hard $prev"
        echo "       and run scripts/land.sh $source again."
      } >&2
      return "$EX_LOCAL_ONLY"
      ;;
    *)
      git reset -q --hard "$prev" 2>/dev/null || true
      if [ "$(git rev-parse "$target")" = "$prev" ]; then
        echo "error: CI or publishing failed; $target was reset to $prev and $source is untouched" >&2
      else
        {
          echo "error: CI or publishing failed AND $target could not be reset automatically."
          echo "       Restore it with:  git switch $target && git reset --hard $prev"
        } >&2
      fi
      return "$EX_CI_FAILED"
      ;;
  esac
}

# merge_into_integration <integration> <source> <message> [update]: LOCAL merge commit on an integration branch; no push,
# no CI. The merge is made in a temporary DETACHED worktree, so nobody has to hold the integration branch checked
# out and several agents never block each other. <source> must already contain the integration tip, so the merge
# commit's tree is exactly the source tree. The fast gate (check.sh --fast --tests, scoped to what the merge adds)
# and the commit-message scan run on that merged tree. Only then is the branch ref moved with a compare-and-swap
# (`git update-ref <ref> <new> <old>`): if the integration branch moved in the meantime nothing changes and
# EX_MOVED is returned. EX_HELD is returned while a worktree has the branch checked out (moving a checked-out ref
# would corrupt that checkout); callers may wait first with wait_for_branch_free. Other failures return 1.
merge_into_integration() {
  local integ="$1" source="$2" msg="$3" mode="${4:-contains}" here itip stip td new="" rc=0 holder
  here=$(git rev-parse --show-toplevel)
  itip=$(git rev-parse -q --verify "refs/heads/$integ") || { echo "error: no such branch: $integ" >&2; return 1; }
  stip=$(git rev-parse -q --verify "$source^{commit}") || { echo "error: no such branch: $source" >&2; return 1; }
  if [ "$mode" != update ]; then
    git merge-base --is-ancestor "$itip" "$stip" || {
      echo "error: $source does not contain $integ. Update it first:  git switch $source && git merge $integ" >&2
      return 1
    }
  fi
  git merge-base --is-ancestor "$stip" "$itip" && {
    echo "error: nothing to merge: $integ already contains $source" >&2
    return 1
  }
  holder=$(branch_holder "$integ")
  if [ -n "$holder" ] || [ "$(current_branch)" = "$integ" ]; then
    echo "error: '$integ' is checked out in a worktree (${holder:-this one}); landing never moves a checked-out branch." >&2
    echo "       Release it with 'git switch --detach' there (integration branches need no checkout), or use --wait SECONDS." >&2
    return "$EX_HELD"
  fi
  td=$(mktemp -d)
  git worktree add -q --detach "$td/wt" "$itip" || { rm -rf "$td"; echo "error: could not create a temporary worktree" >&2; return 1; }
  (
    cd "$td/wt" || exit 1
    if ! CLAWDOT_LANDING=1 git merge --no-ff -q -m "$msg" "$stip"; then
      git merge --abort 2>/dev/null || true
      echo "error: the merge has conflicts. Resolve them on a branch:  git switch -c chore/update-${integ#integration/} $integ && git merge $source" >&2
      echo "       then land that branch with:  scripts/land.sh chore/update-${integ#integration/} --into $integ" >&2
      exit 1
    fi
    new=$(git rev-parse HEAD)
    CHECK_BASE="$itip" "$here/scripts/check.sh" --fast --tests || exit 1
    "$here/scripts/check-commit-msg.sh" --range "$itip..$new" || exit 1
    echo "$new" >"$td/sha"
  ) || rc=$?
  git worktree remove --force "$td/wt" >/dev/null 2>&1 || true
  [ -f "$td/sha" ] && new=$(cat "$td/sha")
  rm -rf "$td"
  if [ "$rc" -ne 0 ] || [ -z "$new" ]; then
    echo "error: the local merge or its fast checks failed for $source; $integ is unchanged" >&2
    return 1
  fi
  git merge-base --is-ancestor "$itip" "$new" || { echo "error: internal: the merge does not extend $integ" >&2; return 1; }
  if ! git update-ref -m "land: $msg" "refs/heads/$integ" "$new" "$itip" 2>/dev/null; then
    echo "error: $integ moved underneath this landing (it was ${itip:0:10}, now $(git rev-parse --short "refs/heads/$integ")). Nothing was changed." >&2
    echo "       Merge the new tip into your branch and land again:  git switch $source && git merge $integ" >&2
    return "$EX_MOVED"
  fi
}
