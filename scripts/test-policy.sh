#!/usr/bin/env bash
# Regression tests for the git-flow policy tooling: commit-message rules and the agent guard.
# Runs in a scratch repository; touches nothing in the working tree. Part of scripts/check.sh.
set -u
# When run from a git hook, git exports variables that would point scratch-repo commands at the real repository.
unset GIT_DIR GIT_INDEX_FILE GIT_WORK_TREE GIT_PREFIX GIT_COMMON_DIR GIT_OBJECT_DIRECTORY GIT_ALTERNATE_OBJECT_DIRECTORIES
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
  git config core.autocrlf false
  mkdir -p scripts .claude/hooks
  cp "$root/scripts/lib-git.sh" scripts/
  cp "$root/.claude/hooks/guard-git.sh" .claude/hooks/
  git commit -q --allow-empty -m "chore: initialize repository"
  git branch dev
) || { echo "could not build scratch repo" >&2; exit 2; }

g() { # g <branch> <expected-exit> <command>
  (cd "$S" && { git checkout -q "$1" 2>/dev/null || git checkout -q -b "$1"; })
  local json out got
  # Mirrors the real payload, including unrelated fields such as permission_mode.
  json=$(printf '{"session_id":"s","cwd":"/x","permission_mode":"bypassPermissions","hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"%s","description":"d"},"tool_use_id":"t"}' "$3")
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
g feature/x 0 "scripts/release.sh prepare 1.0.0"
g feature/x 2 "scripts/release.sh publish 1.0.0"
g feature/x 2 "scripts/release.sh tag 1.0.0"
g feature/x 0 "git checkout -- .githooks"
g feature/x 0 "git status --short | wc -l && head -3 .githooks/pre-commit"
g feature/x 2 "git commit $nv -m x"
g feature/x 2 "git commit -m x --author=a"
g feature/x 0 "git config --get user.name"
g feature/x 0 "git config --get core.hooksPath"
g feature/x 2 "git config user.name Foo"
g feature/x 2 "git config core.hooksPath /tmp/elsewhere"
g feature/x 2 "git -c user.name=Foo commit -m x"
g feature/x 2 "git push --force origin feature/x"
g feature/x 2 "git push origin HEAD:dev"
g feature/x 2 "git tag v1.0.0"
g feature/x 2 "git branch -D dev"
g feature/x 2 "gh pr merge 3"
g feature/x 2 "git commit -m x -m Co-Authored-By: $ai <a@b.c>"
g wip 2 "git commit -m x"
rm -rf "$S"

# --- hook stubs fail closed on main when the versioned hooks are absent ---------------------------
H=$(mktemp -d)
(
  cd "$H" || exit 1
  git init -q -b main
  git config user.name T
  git config user.email t@example.com
  git config core.autocrlf false
  git commit -q --allow-empty -m "chore: initialize repository"
) || { echo "could not build scratch repo for stubs" >&2; exit 2; }
if (cd "$H" && bash "$root/scripts/setup.sh" >/dev/null 2>&1); then ok; else fail "setup.sh failed in a scratch repo"; fi
if (cd "$H" && git commit -q --allow-empty -m "chore: direct commit" >/dev/null 2>&1); then
  fail "a commit on main succeeded even though versioned hooks are absent"
else ok; fi
if (cd "$H" && git switch -q -c feature/x && git commit -q --allow-empty -m "chore: on a branch" >/dev/null 2>&1); then
  ok
else fail "a commit on a feature branch should be allowed by the stubs"; fi

# setup.sh must never overwrite the versioned hooks, even when a hooks-path setting points at them.
(
  cd "$H" || exit 1
  mkdir -p .githooks
  printf '#!/usr/bin/env bash\necho sentinel\n' >.githooks/pre-commit
  git config core.hooksPath .githooks
  bash "$root/scripts/setup.sh" >/dev/null 2>&1
  grep -q sentinel .githooks/pre-commit
) && ok || fail "setup.sh overwrote the versioned .githooks directory"
rm -rf "$H"

# --- landing and release flow, end to end in local mode (scratch repository) --------------------
L=$(mktemp -d)
out=$(mktemp)
(
  cd "$L" || exit 1
  git init -q -b main
  git config user.name T
  git config user.email t@example.com
  git config core.autocrlf false
  git commit -q --allow-empty -m "chore: initialize repository"
  git branch dev
  copy_scripts() { mkdir -p scripts; for f in "$root"/scripts/*.sh; do [ "$(basename "$f")" = test-policy.sh ] || cp "$f" scripts/; done; }
  git switch -q dev
  copy_scripts
  git add scripts
  git commit -q -m "build: add scripts"

  # 1. a feature branch lands into dev as a real merge commit, and the branch is deleted
  git switch -q -c feature/x dev
  echo hi >f.txt && git add f.txt && git commit -q -m "feat: add f"
  scripts/land.sh feature/x >"$out" 2>&1 || exit 11
  [ "$(git rev-list --parents -n1 dev | wc -w)" -eq 3 ] || exit 12
  [ "$(git log -1 --format=%s dev)" = "Merge feature/x into dev" ] || exit 13
  ! git rev-parse -q --verify feature/x >/dev/null || exit 14

  # 2. a release branch cannot be landed into main; it lands into dev like any branch
  git switch -q -c release/1.0.0 dev
  printf '## [1.0.0]\n' >CHANGELOG.md && git add CHANGELOG.md && git commit -q -m "chore(release): v1.0.0"
  if scripts/land.sh release/1.0.0 --into main >"$out" 2>&1; then exit 15; fi
  scripts/land.sh release/1.0.0 >"$out" 2>&1 || exit 16

  # 3. publishing refuses without an interactive terminal, and changes nothing
  if scripts/release.sh publish 1.0.0 </dev/null >"$out" 2>&1; then exit 17; fi
  grep -q "interactive terminal" "$out" || exit 18
  [ -z "$(git tag)" ] || exit 19
  [ "$(git rev-parse main)" = "$(git rev-list --max-parents=0 dev)" ] || exit 20

  # 4. a hotfix merges into main with a merge commit
  git switch -q -c hotfix/h main
  copy_scripts && git add scripts && git commit -q -m "build: add scripts"
  echo fix >h.txt && git add h.txt && git commit -q -m "fix: hot"
  scripts/land.sh hotfix/h --into main >"$out" 2>&1 || exit 21
  [ "$(git rev-list --parents -n1 main | wc -w)" -eq 3 ] || exit 22
  exit 0
)
rc=$?
if [ "$rc" -eq 0 ]; then ok; else fail "landing/release flow test failed at step code $rc (output: $(tail -3 "$out" | tr '\n' ' '))"; fi
rm -rf "$L" "$out"

echo "policy tests: $pass passed, $failn failed"
[ "$failn" -eq 0 ]
