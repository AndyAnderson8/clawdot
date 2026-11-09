#!/usr/bin/env bash
# Create an integration branch for batching several working branches before ONE landing into dev.
#   scripts/new-integration.sh <name>      creates integration/<name> from up-to-date dev (also: land.sh --new-integration <name>)
# The branch is created without checking anything out, so no worktree holds it. Batch into it with
#   scripts/land.sh <branch> --into integration/<name>      (local merge commit; no push, no CI)
# and land the whole batch with  scripts/land.sh integration/<name>.  Policy: docs/process/git-workflow.md section 5a.
# Exit codes: 0 created; 1 error (bad name, branch exists, dev not up to date).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
# shellcheck source=scripts/lib-git.sh
. scripts/lib-git.sh

name="${1:-}"
[[ "$name" =~ ^[a-z0-9][a-z0-9._-]*$ ]] || die "usage: scripts/new-integration.sh <kebab-name>   (creates integration/<name>)"
branch="integration/$name"
git rev-parse -q --verify "refs/heads/$branch" >/dev/null && die "$branch already exists"
git rev-parse -q --verify refs/heads/dev >/dev/null || die "no local dev branch"

base=dev
if has_remote; then
  git fetch --quiet --prune origin
  if git rev-parse -q --verify refs/remotes/origin/dev >/dev/null; then
    git merge-base --is-ancestor dev origin/dev ||
      die "local dev has commits that origin/dev lacks (or has diverged). Land or reset them first; integration branches start from a dev that is on origin."
    base=origin/dev
  fi
fi
git branch --no-track "$branch" "$base"
echo "created $branch at $(git rev-parse --short "$branch") (from $base)"
echo "next: work on feature/... branches, then  scripts/land.sh <branch> --into $branch"
