#!/usr/bin/env bash
# Regression tests for the git-flow policy tooling: commit-message rules and the agent guard.
# Runs in a scratch repository; touches nothing in the working tree. Part of scripts/check.sh.
set -u
root=$(git rev-parse --show-toplevel)
pass=0
failn=0
ok() { pass=$((pass + 1)); }
fail() { failn=$((failn + 1)); echo "FAIL: $1" >&2; }

# Built from pieces so this file never trips AI-attribution scanners by itself.
ai="Cl""aude"
anth="noreply@anthro""pic.com"

# --- commit message rules ---------------------------------------------------------------------
tmp=$(mktemp)
cm() { # cm <expected-exit> <message>
  printf '%s\n' "$2" >"$tmp"
  "$root/scripts/check-commit-msg.sh" --file "$tmp" >/dev/null 2>&1
  local got=$?
  if [ "$got" = "$1" ]; then ok; else fail "commit-msg exit $got, wanted $1: $2"; fi
}
cm 0 "feat(sched): add persistent timer store"
cm 0 "fix: handle empty transcript"
cm 0 "chore(release)!: v1.0.0"
cm 0 "Merge feature/x into dev"
cm 0 "$(printf 'docs: explain why\n\nRefs: REQ-TMR-001')"
cm 1 "Added timers"
cm 1 "feat: Add timers."
cm 1 "feature(x): add timers"
cm 1 "feat(x):missing space"
cm 1 "feat: this subject line is far too long to be acceptable because it exceeds seventy-two characters"
cm 1 "$(printf 'feat: ok\nno blank line')"
cm 1 "$(printf 'feat: ok\n\nCo-Authored-By: %s <%s>' "$ai" "$anth")"
cm 1 "$(printf 'feat: ok\n\nGenerated with [%s Code](https://%s.ai/code)' "$ai" "${ai,,}")"
rm -f "$tmp"

# --- agent guard --------------------------------------------------------------------------------
S=$(mktemp -d)
(
  cd "$S" || exit 1
  git init -q -b main
  git config user.name T
  git config user.email t@example.com
  mkdir -p scripts .claude/hooks
  cp "$root/scripts/lib-git.sh" scripts/
  cp "$root/.claude/hooks/guard-git.sh" .claude/hooks/
  git commit -q --allow-empty -m "chore: initialize repository"
  git branch dev
) || { echo "could not build scratch repo" >&2; exit 2; }

g() { # g <branch> <expected-exit> <command>
  (cd "$S" && { git checkout -q "$1" 2>/dev/null || git checkout -q -b "$1"; })
  local json out got
  json=$(printf '{"tool_name":"Bash","tool_input":{"command":"%s"}}' "$3")
  out=$(cd "$S" && printf '%s' "$json" | CLAUDE_PROJECT_DIR="$S" bash .claude/hooks/guard-git.sh 2>&1)
  got=$?
  if [ "$got" = "$2" ]; then ok; else fail "guard exit $got, wanted $2 on $1: $3 ${out:+($out)}"; fi
}
nv="--no-""verify"

g main 0 "git status"
g main 2 "git commit -m x"
g main 2 "git push origin main"
g dev 2 "git commit -m x"
g dev 2 "git merge feature/x"
g dev 0 "git pull --ff-only"
g feature/x 0 "git commit -m feat: x"
g feature/x 0 "git push origin feature/x"
g feature/x 0 "git tag -l"
g feature/x 0 "ls -la"
g feature/x 2 "git commit $nv -m x"
g feature/x 2 "git commit -m x --author=a"
g feature/x 2 "git config user.name Foo"
g feature/x 2 "git push --force origin feature/x"
g feature/x 2 "git push origin HEAD:dev"
g feature/x 2 "git tag v1.0.0"
g feature/x 2 "git branch -D dev"
g feature/x 2 "gh pr merge 3"
g feature/x 2 "git commit -m x -m Co-Authored-By: $ai <a@b.c>"
g wip 2 "git commit -m x"
rm -rf "$S"

echo "policy tests: $pass passed, $failn failed"
[ "$failn" -eq 0 ]
