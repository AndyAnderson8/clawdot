#!/usr/bin/env bash
# Regression tests for the git-flow policy tooling: commit-message rules and the agent guard.
# Runs in a scratch repository; touches nothing in the working tree. Part of scripts/check.sh.
# Each scratch-repo block runs in its own subshell on purpose, so its PATH and variables stay local.
# SC2016: the stub scripts written with printf '...' must keep their $ unexpanded.
# shellcheck disable=SC2030,SC2031,SC2016
set -u
# When run from a git hook, git exports variables that would point scratch-repo commands at the real repository.
unset GIT_DIR GIT_INDEX_FILE GIT_WORK_TREE GIT_PREFIX GIT_COMMON_DIR GIT_OBJECT_DIRECTORY GIT_ALTERNATE_OBJECT_DIRECTORIES
root=$(git rev-parse --show-toplevel)
# A landing into an integration branch sets CHECK_BASE for its own checks; the scratch repositories below have their own
# `dev` and must measure against it.
unset CHECK_BASE
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
# integration branches: valid for scripts, but changed only by scripts/land.sh (no manual commits, merges, or ref moves)
g integration/x 2 "git commit -m x"
g integration/x 2 "git merge feature/x"
g integration/x 2 "git rebase dev"
g integration/x 0 "git switch --detach integration/x"
g integration/x 0 "git log --oneline integration/x"
g feature/x 2 "git update-ref refs/heads/integration/x HEAD"
g feature/x 2 "git update-ref refs/heads/dev HEAD"
g feature/x 2 "git branch -f integration/x HEAD"
g feature/x 0 "git branch -f feature/devops HEAD"
g feature/x 2 "git push --force origin integration/x"
g feature/x 2 "git push origin HEAD:dev"
g feature/x 0 "scripts/land.sh feature/x --into integration/x"
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
if (
  cd "$H" || exit 1
  mkdir -p .githooks
  printf '#!/usr/bin/env bash\necho sentinel\n' >.githooks/pre-commit
  git config core.hooksPath .githooks
  bash "$root/scripts/setup.sh" >/dev/null 2>&1
  grep -q sentinel .githooks/pre-commit
); then
  ok
else
  fail "setup.sh overwrote the versioned .githooks directory"
fi
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
  copy_scripts() {
    mkdir -p scripts
    for f in "$root"/scripts/*.sh; do [ "$(basename "$f")" = test-policy.sh ] || cp "$f" scripts/; done
    cp "$root/.shellcheckrc" .shellcheckrc
  }
  git switch -q dev
  copy_scripts
  git add scripts .shellcheckrc
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
  copy_scripts && git add scripts .shellcheckrc && git commit -q -m "build: add scripts"
  echo fix >h.txt && git add h.txt && git commit -q -m "fix: hot"
  scripts/land.sh hotfix/h --into main >"$out" 2>&1 || exit 21
  [ "$(git rev-list --parents -n1 main | wc -w)" -eq 3 ] || exit 22
  exit 0
)
rc=$?
if [ "$rc" -eq 0 ]; then ok; else fail "landing/release flow test failed at step code $rc (output: $(tail -3 "$out" | tr '\n' ' '))"; fi
rm -rf "$L" "$out"

# --- commit-time checks are fast; the full gate is not (scratch repository with a recording cargo stub) ----
F=$(mktemp -d)
calls="$F/cargo-calls.log"
fout=$(mktemp)
(
  cd "$F" || exit 1
  git init -q -b main
  git config user.name T
  git config user.email t@example.com
  git config core.autocrlf false
  mkdir -p scripts .githooks crates/a/src
  for f in check.sh check-docs.sh check-commit-msg.sh lib-git.sh; do cp "$root/scripts/$f" scripts/; done
  cp "$root/.githooks/pre-commit" "$root/.githooks/commit-msg" .githooks/
  cp "$root/.shellcheckrc" .shellcheckrc
  # The stub records its arguments instead of building anything.
  cat >scripts/cargo.sh <<'STUB'
#!/usr/bin/env bash
echo "$*" >>"${CARGO_CALLS:?}"
exit 0
STUB
  chmod +x scripts/*.sh .githooks/*
  printf '[workspace]\nmembers = ["crates/*"]\n' >Cargo.toml
  printf '[package]\nname = "pkg-a"\nversion = "0.0.0"\n' >crates/a/Cargo.toml
  : >deny.toml
  echo "// a" >crates/a/src/lib.rs
  git add -A
  git commit -q -m "chore: initialize repository"
  git branch dev
  git switch -q -c chore/x
  git config core.hooksPath .githooks
  export CARGO_CALLS="$calls"

  # 1. a docs-only commit runs no Rust step and says what was skipped
  : >"$calls"
  mkdir -p docs && echo "# d" >docs/d.md && git add docs/d.md
  git commit -m "docs: add d" >"$fout" 2>&1 || exit 31
  [ ! -s "$calls" ] || exit 32
  grep -q "scripts/land.sh and in CI" "$fout" || exit 33

  # 2. a Rust commit runs fmt and clippy scoped to the affected package, but not tests or deny
  : >"$calls"
  echo "// b" >>crates/a/src/lib.rs && git add -A
  git commit -m "feat(a): change a" >"$fout" 2>&1 || exit 34
  grep -q "^fmt " "$calls" || exit 35
  grep -q "^clippy -p pkg-a " "$calls" || exit 36
  ! grep -qE "^(test|deny) " "$calls" || exit 37

  # 3. a root manifest change lints the whole workspace
  : >"$calls"
  echo "# c" >>Cargo.toml && git add -A
  git commit -m "build: touch manifest" >"$fout" 2>&1 || exit 38
  grep -q "^clippy --workspace " "$calls" || exit 39

  # 4. a bad commit message still blocks, whatever the files
  : >"$calls"
  echo "# e" >docs/e.md && git add docs/e.md
  if git commit -m "Added e." >"$fout" 2>&1; then exit 40; fi
  git reset -q

  # 5. the full gate still runs tests and cargo-deny
  : >"$calls"
  scripts/check.sh >"$fout" 2>&1 || exit 41
  grep -q "^test " "$calls" || exit 42
  grep -q "^deny check" "$calls" || exit 43
  exit 0
)
rc=$?
if [ "$rc" -eq 0 ]; then ok; else fail "commit-time check test failed at step code $rc (output: $(tail -3 "$fout" | tr '\n' ' '))"; fi
rm -rf "$F" "$fout"

# --- path-based step selection: check.sh --fast --tests runs only the steps for the paths that changed vs dev ----
# (scratch repository; cargo, npm and the policy tests are recording stubs)
S=$(mktemp -d)
sout=$(mktemp)
(
  cd "$S" || exit 1
  git init -q -b main
  git config user.name T
  git config user.email t@example.com
  git config core.autocrlf false
  mkdir -p scripts bin crates/a/src crates/b/src portal/src portal/node_modules portal/lint/node_modules
  for f in check.sh check-docs.sh check-commit-msg.sh lib-git.sh; do cp "$root/scripts/$f" scripts/; done
  cp "$root/.shellcheckrc" .shellcheckrc
  printf '#!/usr/bin/env bash\necho "cargo $*" >>"${CALLS:?}"\n' >scripts/cargo.sh
  printf '#!/usr/bin/env bash\necho "policy" >>"${CALLS:?}"\n' >scripts/test-policy.sh
  printf '#!/usr/bin/env bash\necho "npm $* QUICK=${CLAWDOT_BROWSER_QUICK:-}" >>"${CALLS:?}"\n' >bin/npm
  chmod +x scripts/*.sh bin/npm
  printf '[workspace]\nmembers = ["crates/*"]\n' >Cargo.toml
  printf '[package]\nname = "pkg-a"\nversion = "0.0.0"\n' >crates/a/Cargo.toml
  printf '[package]\nname = "pkg-b"\nversion = "0.0.0"\n' >crates/b/Cargo.toml
  echo '{}' >portal/package.json
  echo "// a" >crates/a/src/lib.rs
  echo "// b" >crates/b/src/lib.rs
  echo "// p" >portal/src/p.ts
  git add -A
  git commit -q -m "chore: initialize repository"
  git branch dev
  export PATH="$S/bin:$PATH"
  export CALLS="$S.calls"
  unset CI
  case_run() { # case_run <branch> <file> <content>: one commit on a fresh branch off dev, then the scoped check
    git switch -q -c "$1" dev
    mkdir -p "$(dirname "$2")"
    printf '%s\n' "$3" >>"$2"
    git add -A
    git commit -q -m "chore: change $2"
    : >"$CALLS"
    scripts/check.sh --fast --tests >"$sout" 2>&1
  }

  # 1. docs only: no cargo, no npm, no policy tests; the docs step runs
  case_run c1 docs/d.md "# d" || exit 61
  [ ! -s "$CALLS" ] || exit 62
  grep -q "documentation: links" "$sout" || exit 63

  # 2. one crate: fmt, clippy and tests for that package only; no deny, no npm
  case_run c2 crates/a/src/lib.rs "// more" || exit 64
  grep -q "^cargo clippy -p pkg-a " "$CALLS" || exit 65
  grep -q "^cargo test -p pkg-a " "$CALLS" || exit 66
  ! grep -q "pkg-b\|--workspace\|deny\|^npm \|^policy" "$CALLS" || exit 67

  # 3. portal: lint, types, unit tests, built-page check and the QUICK browser smoke; no licences/audit, no cargo
  case_run c3 portal/src/p.ts "// more" || exit 68
  grep -q "^npm .*lint " "$CALLS" || exit 69
  grep -q "^npm .*typecheck " "$CALLS" || exit 70
  grep -q "^npm .*check-build " "$CALLS" || exit 71
  grep -q "^npm .*test:browser QUICK=1$" "$CALLS" || exit 72
  cat "$CALLS" >&2
  ! grep -qE "licenses|audit|^cargo " "$CALLS" || exit 73

  # 4. scripts: the policy tests run; no cargo, no npm
  case_run c4 scripts/extra.sh '#!/usr/bin/env bash' || exit 74
  grep -q "^policy" "$CALLS" || exit 75
  ! grep -qE "^(cargo|npm) " "$CALLS" || exit 76

  # 5. the full gate (CI=1) still runs everything, and the browser test is NOT in quick mode
  git switch -q c3
  : >"$CALLS"
  CI=1 scripts/check.sh --full >"$sout" 2>&1 || exit 77
  grep -q "^npm .*test:browser QUICK=$" "$CALLS" || exit 78
  grep -q "^npm .*licenses" "$CALLS" || exit 79
  grep -q "^cargo test --workspace " "$CALLS" || exit 80
  grep -q "^policy" "$CALLS" || exit 81

  # 6. bad flag combinations are refused
  if scripts/check.sh --full --fast >/dev/null 2>&1; then exit 82; fi
  if scripts/check.sh --tests >/dev/null 2>&1; then exit 83; fi
  exit 0
)
rc=$?
if [ "$rc" -eq 0 ]; then ok; else fail "path-selection test failed at step code $rc (output: $(tail -3 "$sout" | tr '\n' ' '))"; fi
rm -rf "$S" "$S.calls" "$sout"

# --- scripts/cargo.sh uses sccache only when installed, and never when disabled or already set ----
W=$(mktemp -d)
(
  cd "$W" || exit 1
  git init -q -b main
  mkdir -p scripts bin nosc
  cp "$root/scripts/cargo.sh" scripts/
  printf '#!/usr/bin/env bash\necho "wrapper=${RUSTC_WRAPPER:-none} dir=${SCCACHE_DIR:-none} size=${SCCACHE_CACHE_SIZE:-none}"\n' >bin/cargo
  printf '#!/usr/bin/env bash\nexit 0\n' >bin/sccache
  chmod +x scripts/cargo.sh bin/cargo bin/sccache
  export HOME="$W/home"
  unset RUSTC_WRAPPER SCCACHE_DIR SCCACHE_CACHE_SIZE CLAWDOT_NO_SCCACHE
  # PATH gets the stub dir for cargo; for the "no sccache" case sccache is shadowed by removing its stub.
  out=$(PATH="$W/bin:$PATH" scripts/cargo.sh check) || exit 91
  case "$out" in "wrapper=sccache dir=$W/home/.cache/clawdot-sccache size=10G") ;; *) exit 92 ;; esac
  out=$(CLAWDOT_NO_SCCACHE=1 PATH="$W/bin:$PATH" scripts/cargo.sh check) || exit 93
  case "$out" in "wrapper=none "*) ;; *) exit 94 ;; esac
  out=$(RUSTC_WRAPPER=mine PATH="$W/bin:$PATH" scripts/cargo.sh check) || exit 95
  case "$out" in "wrapper=mine "*) ;; *) exit 96 ;; esac
  rm bin/sccache
  out=$(PATH="$W/bin:$PATH" scripts/cargo.sh check) || exit 97
  case "$out" in "wrapper=none "*) ;; *) exit 98 ;; esac
  exit 0
)
rc=$?
if [ "$rc" -eq 0 ]; then ok; else fail "sccache wiring test failed at step code $rc"; fi
rm -rf "$W"

# --- optional steps: the full gate passes in a repository with no portal/, no bridges/ and no Rust ----
# (The Python sidecar and portal steps must run only where their directories exist and say nothing elsewhere.)
O=$(mktemp -d)
oout=$(mktemp)
(
  cd "$O" || exit 1
  git init -q -b main
  git config user.name T
  git config user.email t@example.com
  git config core.autocrlf false
  mkdir -p scripts .githooks
  for f in check.sh check-docs.sh check-commit-msg.sh lib-git.sh; do cp "$root/scripts/$f" scripts/; done
  cp "$root/.shellcheckrc" .shellcheckrc
  chmod +x scripts/*.sh
  echo "# readme" >README.md
  git add -A
  git commit -q -m "chore: initialize repository"
  git branch dev
  scripts/check.sh >"$oout" 2>&1 || exit 51
  grep -q "check: all gates passed" "$oout" || exit 52
  ! grep -qiE "python|unittest|ruff|portal|personal" "$oout" || exit 53
  # The fast gate is just as quiet: no portal/, bridges/, personal-data script or node-dependent file needed.
  echo "more" >>README.md && git add -A
  scripts/check.sh --fast >"$oout" 2>&1 || exit 56
  grep -q "check --fast: passed" "$oout" || exit 57
  # (The closing "skipped: ..." summary names the steps that did not run; that line is not a step running.)
  ! grep -v "^check --fast: skipped" "$oout" | grep -qiE "python|unittest|ruff|portal|personal|node" || exit 58
  git commit -q -m "docs: touch readme"
  # With a bridges/ directory that has no tests the step still stays quiet and passes.
  mkdir -p bridges && echo "# b" >bridges/README.md && git add -A && git commit -q -m "docs: add bridges readme"
  scripts/check.sh >"$oout" 2>&1 || exit 54
  ! grep -qiE "unittest|ruff" "$oout" || exit 55
  exit 0
)
rc=$?
if [ "$rc" -eq 0 ]; then ok; else fail "optional-steps test failed at step code $rc (output: $(tail -3 "$oout" | tr '\n' ' '))"; fi
rm -rf "$O" "$oout"

# --- land.sh against a remote: rejected push, CI failure, held dev, verification (scratch repos, gh stub) ----
R=$(mktemp -d)
rout=$(mktemp)
(
  cd "$R" || exit 1
  git init -q --bare origin.git
  # The server can be told to refuse pushes to dev (as a repository rule would) or to accept and then
  # silently drop them (so only the post-push verification can notice).
  cat >origin.git/hooks/pre-receive <<'HOOK'
#!/bin/sh
while read -r old new ref; do
  if [ "$ref" = refs/heads/dev ] && [ -f "$GIT_DIR/reject" ]; then echo "push declined due to repository rule violations" >&2; exit 1; fi
done
HOOK
  cat >origin.git/hooks/post-receive <<'HOOK'
#!/bin/sh
while read -r old new ref; do
  if [ "$ref" = refs/heads/dev ] && [ -f "$GIT_DIR/revert" ]; then git update-ref refs/heads/dev "$old"; fi
done
HOOK
  chmod +x origin.git/hooks/*
  # A gh stub: CI "runs" immediately and passes unless the file fail-ci exists.
  mkdir -p bin
  cat >bin/gh <<'STUB'
#!/bin/sh
case "$1 $2" in
  "run list") echo 1 ;;
  "run watch") [ ! -f "$R_FAIL_CI" ] ;;
esac
STUB
  chmod +x bin/gh
  export R_FAIL_CI="$R/fail-ci" PATH="$R/bin:$PATH" LAND_CI_POLL=0 LAND_WAIT_INTERVAL=1

  git init -q -b main work
  cd work || exit 1
  git config user.name T
  git config user.email t@example.com
  git config core.autocrlf false
  git remote add origin "$R/origin.git"
  git commit -q --allow-empty -m "chore: initialize repository"
  git branch dev
  git switch -q dev
  mkdir -p scripts
  for f in "$root"/scripts/*.sh; do [ "$(basename "$f")" = test-policy.sh ] || cp "$f" scripts/; done
  cp "$root/.shellcheckrc" .shellcheckrc
  git add scripts .shellcheckrc
  git commit -q -m "build: add scripts"
  git push -q origin main dev 2>/dev/null
  git switch -q -c feature/x dev
  echo hi >f.txt && git add f.txt && git commit -q -m "feat: add f"
  prev=$(git rev-parse dev)
  tip=$(git rev-parse feature/x)

  # 1. the final push is rejected: non-zero exit, branch and candidate kept, dev recoverable, no "done"
  : >"$R/origin.git/reject"
  rc=0; scripts/land.sh feature/x >"$rout" 2>&1 || rc=$?
  [ "$rc" -eq 4 ] || exit 51
  grep -q "LOCAL dev" "$rout" || exit 52
  grep -q "git reset --hard $prev" "$rout" || exit 53
  ! grep -q "done" "$rout" || exit 54
  git rev-parse -q --verify feature/x >/dev/null || exit 55
  [ -n "$(git ls-remote origin 'refs/heads/ci/*')" ] || exit 56
  [ "$(git rev-parse origin/dev)" = "$prev" ] || exit 57
  [ "$(git rev-parse dev)" != "$prev" ] || exit 58
  # recover as the message says, then land again
  git switch -q dev && git reset -q --hard "$prev" && git switch -q feature/x
  rm "$R/origin.git/reject"

  # 2. the push "succeeds" but origin does not contain the commit: the verification catches it
  : >"$R/origin.git/revert"
  rc=0; scripts/land.sh feature/x >"$rout" 2>&1 || rc=$?
  [ "$rc" -eq 4 ] || exit 59
  grep -q "does not contain" "$rout" || exit 60
  git rev-parse -q --verify feature/x >/dev/null || exit 61
  git switch -q dev && git reset -q --hard "$prev" && git switch -q feature/x
  rm "$R/origin.git/revert"

  # 3. CI fails: dev is restored, exit 5, branch kept
  : >"$R/fail-ci"
  rc=0; scripts/land.sh feature/x >"$rout" 2>&1 || rc=$?
  [ "$rc" -eq 5 ] || exit 62
  [ "$(git rev-parse dev)" = "$prev" ] || exit 63
  [ "$(git rev-parse origin/dev)" = "$prev" ] || exit 64
  git rev-parse -q --verify feature/x >/dev/null || exit 65
  rm "$R/fail-ci"
  git switch -q feature/x

  # 4. dev held by another worktree: clear message naming it, exit 3, nothing changed
  git worktree add -q "$R/holder" dev
  rc=0; scripts/land.sh feature/x >"$rout" 2>&1 || rc=$?
  [ "$rc" -eq 3 ] || exit 66
  grep -q "holder" "$rout" || exit 67
  ! grep -q "nothing to merge" "$rout" || exit 68
  git rev-parse -q --verify feature/x >/dev/null || exit 69
  [ "$(git -C "$R/holder" rev-parse dev)" = "$prev" ] || exit 70
  # --wait gives up after the bound (still held)...
  rc=0; scripts/land.sh feature/x --wait 2 >"$rout" 2>&1 || rc=$?
  [ "$rc" -eq 3 ] || exit 71
  grep -q "retrying" "$rout" || exit 72
  # ...and succeeds when the holder lets go in time
  (sleep 2; git worktree remove --force "$R/holder") &
  rc=0; scripts/land.sh feature/x --wait 30 >"$rout" 2>&1 || rc=$?
  wait
  [ "$rc" -eq 0 ] || exit 73

  # 5. a clean landing is verified against origin and cleans up
  git fetch -q origin
  git merge-base --is-ancestor "$tip" origin/dev || exit 74
  [ "$(git rev-parse dev)" = "$(git rev-parse origin/dev)" ] || exit 75
  ! git rev-parse -q --verify feature/x >/dev/null || exit 76
  grep -q "verified: origin/dev" "$rout" || exit 77
  # candidate refs of the failed attempts are kept on purpose; the successful one is removed
  [ -z "$(git ls-remote origin "refs/heads/ci/dev/$(git rev-parse origin/dev | cut -c1-10)")" ] || exit 78
  exit 0
)
rc=$?
if [ "$rc" -eq 0 ]; then ok; else fail "land.sh remote test failed at step code $rc (output: $(tail -4 "$rout" | tr '\n' ' '))"; fi
rm -rf "$R" "$rout"

# --- gate mode (scripts/gate-mode): stabilization lands on the fast check without CI; full and main stay gated ----------
G=$(mktemp -d)
gout=$(mktemp)
(
  cd "$G" || exit 1
  git init -q --bare origin.git
  cat >origin.git/hooks/pre-receive <<'HOOK'
#!/bin/sh
while read -r old new ref; do
  if [ "$ref" = refs/heads/dev ] && [ -f "$GIT_DIR/reject" ]; then echo "push declined due to repository rule violations" >&2; exit 1; fi
done
HOOK
  chmod +x origin.git/hooks/pre-receive
  mkdir -p bin
  cat >bin/gh <<'STUB'
#!/bin/sh
echo "gh $*" >>"$G_GH_LOG"
case "$1 $2" in
  "run list") echo 1 ;;
  "run watch") [ ! -f "$G_FAIL_CI" ] ;;
esac
STUB
  chmod +x bin/gh
  export G_GH_LOG="$G/gh.log" G_FAIL_CI="$G/fail-ci" PATH="$G/bin:$PATH" LAND_CI_POLL=0 LAND_WAIT_INTERVAL=1
  : >"$G_GH_LOG"
  git init -q -b main work
  cd work || exit 1
  git config user.name T
  git config user.email t@example.com
  git config core.autocrlf false
  git remote add origin "$G/origin.git"
  git commit -q --allow-empty -m "chore: initialize repository"
  git branch dev
  git switch -q dev
  mkdir -p scripts
  for f in "$root"/scripts/*.sh; do [ "$(basename "$f")" = test-policy.sh ] || cp "$f" scripts/; done
  cp "$root/.shellcheckrc" .shellcheckrc
  git add scripts .shellcheckrc
  git commit -q -m "build: add scripts"
  git branch -f main dev
  setmode() { # setmode <value>: commit scripts/gate-mode on dev and publish it (the scratch repo has no hooks)
    printf '# comment\n%s\n' "$1" >scripts/gate-mode
    git add scripts/gate-mode && git commit -q -m "chore: set gate mode"
    git push -q origin main dev 2>/dev/null
  }
  mk() { # mk <branch> <file> [base]
    git switch -q -c "$1" "${3:-dev}" && echo hi >"$2" && git add "$2" && git commit -q -m "feat: add $2"
  }

  # 1. no gate-mode file: the full gate and CI apply (default)
  git push -q origin main dev 2>/dev/null
  mk feature/a a.txt || exit 201
  scripts/land.sh feature/a >"$gout" 2>&1 || exit 202
  grep -q "run watch" "$G_GH_LOG" || exit 203
  ! grep -q "FULL GATE SKIPPED" "$gout" || exit 204

  # 2. stabilization: fast check, direct push, no candidate ref, no gh call, a clear notice
  git switch -q dev && setmode stabilization
  : >"$G_GH_LOG"
  mk feature/b b.txt || exit 205
  tipb=$(git rev-parse feature/b)
  scripts/land.sh feature/b >"$gout" 2>&1 || exit 206
  grep -q "FULL GATE SKIPPED BY MODE 'stabilization'" "$gout" || exit 207
  grep -q "verified: origin/dev" "$gout" || exit 208
  [ ! -s "$G_GH_LOG" ] || exit 209
  [ -z "$(git ls-remote origin 'refs/heads/ci/*')" ] || exit 210
  git fetch -q origin
  git merge-base --is-ancestor "$tipb" origin/dev || exit 211
  [ "$(git log -1 --format=%s origin/dev)" = "Merge feature/b into dev" ] || exit 212
  ! git rev-parse -q --verify feature/b >/dev/null || exit 213
  ! grep -q "check: all gates passed" "$gout" || exit 214

  # 3. exit codes still work: rejected push (4), held dev (3), a bad commit message is refused (1)
  prev=$(git rev-parse origin/dev)
  mk feature/c c.txt || exit 215
  : >"$G/origin.git/reject"
  rc=0; scripts/land.sh feature/c >"$gout" 2>&1 || rc=$?
  [ "$rc" -eq 4 ] || exit 216
  grep -q "LOCAL dev" "$gout" || exit 217
  git rev-parse -q --verify feature/c >/dev/null || exit 218
  git switch -q dev && git reset -q --hard "$prev" && git switch -q feature/c
  rm "$G/origin.git/reject"
  git worktree add -q "$G/holder" dev
  rc=0; scripts/land.sh feature/c >"$gout" 2>&1 || rc=$?
  [ "$rc" -eq 3 ] || exit 219
  git worktree remove --force "$G/holder"
  git switch -q -c feature/badmsg dev && echo x >bad.txt && git add bad.txt && git commit -q -m "Added a thing" || exit 220
  rc=0; scripts/land.sh feature/badmsg >"$gout" 2>&1 || rc=$?
  [ "$rc" -eq 1 ] || exit 221
  [ "$(git rev-parse origin/dev)" = "$prev" ] || exit 222
  git switch -q feature/c

  # 4. an invalid value is an error, not a silent default
  git switch -q dev && setmode bogus && git switch -q feature/c
  git merge -q dev 2>/dev/null
  rc=0; scripts/land.sh feature/c >"$gout" 2>&1 || rc=$?
  [ "$rc" -eq 1 ] || exit 223
  grep -q "gate-mode must be" "$gout" || exit 224
  git switch -q dev && setmode full && git switch -q feature/c && git merge -q dev

  # 5. full again: CI is waited for, a CI failure exits 5 and leaves dev alone
  prev=$(git rev-parse origin/dev)
  : >"$G_GH_LOG"; : >"$G_FAIL_CI"
  rc=0; scripts/land.sh feature/c >"$gout" 2>&1 || rc=$?
  [ "$rc" -eq 5 ] || exit 225
  grep -q "run watch" "$G_GH_LOG" || exit 226
  [ "$(git rev-parse origin/dev)" = "$prev" ] || exit 227
  rm "$G_FAIL_CI"

  # 6. a hotfix into main keeps CI even in stabilization mode
  git switch -q dev && setmode stabilization
  git switch -q -c hotfix/h main && echo h >h.txt && git add h.txt && git commit -q -m "fix: hotfix" || exit 228
  : >"$G_GH_LOG"; : >"$G_FAIL_CI"
  rc=0; scripts/land.sh hotfix/h --into main >"$gout" 2>&1 || rc=$?
  [ "$rc" -eq 5 ] || exit 229
  grep -q "run watch" "$G_GH_LOG" || exit 230
  exit 0
)
rc=$?
if [ "$rc" -eq 0 ]; then ok; else fail "gate mode test failed at step code $rc (output: $(tail -4 "$gout" | tr '\n' ' '))"; fi
rm -rf "$G" "$gout"

# --- integration branches: batch locally (no push, no CI), then land the batch (scratch repos, gh stub) ----------
I=$(mktemp -d)
iout=$(mktemp)
(
  cd "$I" || exit 1
  git init -q --bare origin.git
  cat >origin.git/hooks/pre-receive <<'HOOK'
#!/bin/sh
while read -r old new ref; do
  if [ "$ref" = refs/heads/dev ] && [ -f "$GIT_DIR/reject" ]; then echo "push declined due to repository rule violations" >&2; exit 1; fi
done
HOOK
  chmod +x origin.git/hooks/pre-receive
  mkdir -p bin
  # The gh stub logs every call, so "no CI call" can be asserted.
  cat >bin/gh <<'STUB'
#!/bin/sh
echo "$*" >>"$I_GH_LOG"
case "$1 $2" in
  "run list") echo 1 ;;
  "run watch") exit 0 ;;
esac
STUB
  chmod +x bin/gh
  export I_GH_LOG="$I/gh.log" PATH="$I/bin:$PATH" LAND_CI_POLL=0 LAND_WAIT_INTERVAL=1 I_MOVE="$I/move"
  : >"$I_GH_LOG"

  git init -q -b main work
  cd work || exit 1
  git config user.name T
  git config user.email t@example.com
  git config core.autocrlf false
  git remote add origin "$I/origin.git"
  git commit -q --allow-empty -m "chore: initialize repository"
  git branch dev
  git switch -q dev
  mkdir -p scripts .githooks
  for f in "$root"/scripts/*.sh; do [ "$(basename "$f")" = test-policy.sh ] || cp "$f" scripts/; done
  cp "$root"/.githooks/* .githooks/
  cp "$root/.shellcheckrc" .shellcheckrc
  # Test seam: a pre-merge-commit hook that moves the integration branch once, as a racing agent would.
  # shellcheck disable=SC2016
  sed -i 's|^exit 0$|if [ -n "${I_MOVE:-}" ] \&\& [ -f "$I_MOVE" ]; then rm "$I_MOVE"; git update-ref refs/heads/integration/batch "$(git commit-tree -p refs/heads/integration/batch -m "chore: racer" "refs/heads/integration/batch^{tree}")"; fi\nexit 0|' .githooks/pre-merge-commit
  git add scripts .githooks .shellcheckrc
  git commit -q -m "build: add scripts"
  git push -q origin main dev 2>/dev/null
  git config core.hooksPath .githooks
  snap() { git ls-remote origin | sort; }
  mkbranch() { # mkbranch <name> <file>: a branch from the given base (default dev) with one commit
    git switch -q -c "$1" "${3:-dev}" && echo "$1" >"$2" && git add "$2" && git commit -q -m "feat: add $2"
  }

  # 1. create the integration branch from up-to-date dev; nothing holds it
  scripts/new-integration.sh batch >"$iout" 2>&1 || exit 80
  [ "$(git rev-parse integration/batch)" = "$(git rev-parse origin/dev)" ] || exit 81
  ! git worktree list --porcelain | grep -q 'branch refs/heads/integration' || exit 82
  if scripts/new-integration.sh batch >"$iout" 2>&1; then exit 83; fi
  if scripts/new-integration.sh "Bad Name" >"$iout" 2>&1; then exit 84; fi

  # 2. batch two branches: local merge commits only, no push, no CI call, sources deleted after the merge
  before=$(snap)
  mkbranch feature/a a.txt || exit 85
  scripts/land.sh feature/a --into integration/batch >"$iout" 2>&1 || exit 86
  grep -q "local only; no push, no CI" "$iout" || exit 87
  [ "$(git log -1 --format=%s integration/batch)" = "Merge feature/a into integration/batch" ] || exit 88
  [ "$(git rev-list --parents -n1 integration/batch | wc -w)" -eq 3 ] || exit 89
  ! git rev-parse -q --verify feature/a >/dev/null || exit 90
  [ -z "$(git symbolic-ref -q --short HEAD)" ] || exit 91
  mkbranch feature/b b.txt || exit 92
  # a branch that lacks the integration tip is refused, and kept
  if scripts/land.sh feature/b --into integration/batch >"$iout" 2>&1; then exit 93; fi
  grep -q "does not contain integration/batch" "$iout" || exit 94
  git rev-parse -q --verify feature/b >/dev/null || exit 95
  git merge -q -m "Merge integration/batch into feature/b" integration/batch || exit 96
  scripts/land.sh feature/b --into integration/batch >"$iout" 2>&1 || exit 97
  [ "$(git rev-list --count --merges dev..integration/batch)" -ge 2 ] || exit 98
  [ "$(snap)" = "$before" ] || exit 99
  [ ! -s "$I_GH_LOG" ] || exit 100
  [ -z "$(git ls-remote origin 'refs/heads/ci/*')" ] || exit 101
  [ "$(git rev-parse dev)" = "$(git rev-parse origin/dev)" ] || exit 102
  # only feature/chore/bugfix/docs/spike branches are batched; dev and main cannot be targets this way
  git switch -q -c release/9.9.9 dev
  if scripts/land.sh release/9.9.9 --into integration/batch >"$iout" 2>&1; then exit 103; fi
  grep -q "cannot be batched" "$iout" || exit 104
  git switch -q --detach && git branch -q -D release/9.9.9

  # 3. the integration branch moves while a merge is in flight: exit 6, nothing changed, no leftovers
  mkbranch feature/c c.txt integration/batch || exit 105
  : >"$I_MOVE"
  tipc=$(git rev-parse feature/c)
  rc=0; scripts/land.sh feature/c --into integration/batch >"$iout" 2>&1 || rc=$?
  [ "$rc" -eq 6 ] || exit 106
  grep -q "moved underneath" "$iout" || exit 107
  git rev-parse -q --verify feature/c >/dev/null || exit 108
  [ "$(git log -1 --format=%s integration/batch)" = "chore: racer" ] || exit 109
  [ "$(git worktree list | wc -l)" -eq 1 ] || exit 110
  git merge -q -m "Merge integration/batch into feature/c" integration/batch || exit 111
  scripts/land.sh feature/c --into integration/batch >"$iout" 2>&1 || exit 112
  git merge-base --is-ancestor "$tipc" integration/batch || exit 113

  # 4. a worktree holding the integration branch: exit 3 naming it; --wait succeeds once it lets go
  git worktree add -q "$I/holder" integration/batch
  mkbranch feature/d d.txt integration/batch || exit 114
  rc=0; scripts/land.sh feature/d --into integration/batch >"$iout" 2>&1 || rc=$?
  [ "$rc" -eq 3 ] || exit 115
  grep -q "holder" "$iout" || exit 116
  git rev-parse -q --verify feature/d >/dev/null || exit 117
  (sleep 2; git worktree remove --force "$I/holder") &
  rc=0; scripts/land.sh feature/d --into integration/batch --wait 30 >"$iout" 2>&1 || rc=$?
  wait
  [ "$rc" -eq 0 ] || exit 118
  [ "$(snap)" = "$before" ] || exit 119
  [ ! -s "$I_GH_LOG" ] || exit 120

  # 5. dev moves on (a normal landing, with CI); the stale integration branch is refused until updated
  git switch -q dev
  mkbranch feature/e e.txt || exit 121
  scripts/land.sh feature/e >"$iout" 2>&1 || exit 122
  [ -s "$I_GH_LOG" ] || exit 123
  : >"$I_GH_LOG"
  git switch -q --detach
  rc=0; scripts/land.sh integration/batch >"$iout" 2>&1 || rc=$?
  [ "$rc" -eq 1 ] || exit 124
  grep -q "does not contain dev" "$iout" || exit 125
  grep -q -- "--update integration/batch" "$iout" || exit 126
  git rev-parse -q --verify integration/batch >/dev/null || exit 127
  [ ! -s "$I_GH_LOG" ] || exit 128
  scripts/land.sh --update integration/batch >"$iout" 2>&1 || exit 129
  git merge-base --is-ancestor origin/dev integration/batch || exit 130
  [ "$(git log -1 --format=%s integration/batch)" = "Merge dev into integration/batch" ] || exit 131
  [ ! -s "$I_GH_LOG" ] || exit 132

  # 6. landing the batch: contains dev, FULL check, CI on the exact merge, push, verify, clean up
  tipb=$(git rev-parse integration/batch)
  scripts/land.sh integration/batch >"$iout" 2>&1 || exit 133
  grep -q "check: all gates passed" "$iout" || exit 134
  grep -q "run watch" "$I_GH_LOG" || exit 135
  git fetch -q origin
  [ "$(git rev-parse dev)" = "$(git rev-parse origin/dev)" ] || exit 136
  [ "$(git log -1 --format=%s origin/dev)" = "Merge integration/batch into dev" ] || exit 137
  git merge-base --is-ancestor "$tipb" origin/dev || exit 138
  ! git rev-parse -q --verify integration/batch >/dev/null || exit 139
  grep -q "verified: origin/dev" "$iout" || exit 140
  [ -z "$(git ls-remote origin 'refs/heads/ci/*')" ] || exit 141
  [ "$(git rev-list --count --merges origin/dev)" -ge 5 ] || exit 142

  # 7. a rejected push keeps everything and exits non-zero
  prev=$(git rev-parse origin/dev)
  scripts/new-integration.sh rej >"$iout" 2>&1 || exit 143
  mkbranch feature/f f.txt integration/rej || exit 144
  scripts/land.sh feature/f --into integration/rej >"$iout" 2>&1 || exit 145
  git switch -q dev
  : >"$I/origin.git/reject"
  rc=0; scripts/land.sh integration/rej >"$iout" 2>&1 || rc=$?
  [ "$rc" -eq 4 ] || exit 146
  grep -q "LOCAL dev" "$iout" || exit 147
  ! grep -q " done" "$iout" || exit 148
  git rev-parse -q --verify integration/rej >/dev/null || exit 149
  [ "$(git rev-parse origin/dev)" = "$prev" ] || exit 150
  [ -n "$(git ls-remote origin 'refs/heads/ci/*')" ] || exit 151
  git reset -q --hard "$prev"
  rm "$I/origin.git/reject"

  # 8. direct commits stay blocked: dev (beyond the tiny change) and any integration branch
  git switch -q dev
  echo 1 >x1.txt && echo 2 >x2.txt && echo 3 >x3.txt && git add x1.txt x2.txt x3.txt
  if git commit -q -m "feat: too big for dev" >"$iout" 2>&1; then exit 152; fi
  git reset -q && rm -f x1.txt x2.txt x3.txt
  git switch -q -c chore/z dev && echo z >z.txt && git add z.txt && git commit -q -m "chore: add z" || exit 156
  git switch -q integration/rej
  echo 1 >y.txt && git add y.txt
  if git commit -q -m "feat: by hand" >"$iout" 2>&1; then exit 153; fi
  grep -q "never made by hand" "$iout" || exit 154
  git reset -q && rm -f y.txt
  if git merge --no-ff -q chore/z >"$iout" 2>&1; then exit 155; fi
  git merge --abort 2>/dev/null || true
  git switch -q --detach
  exit 0
)
rc=$?
if [ "$rc" -eq 0 ]; then ok; else fail "integration branch test failed at step code $rc (output: $(tail -4 "$iout" | tr '\n' ' '))"; fi
rm -rf "$I" "$iout"

# --- scripts/cargo.sh must not build into the repository (stray '-agent-<id>' directories) -------------
if grep -qE '\$\{?CARGO_TARGET_DIR' "$root/scripts/cargo.sh"; then
  fail "scripts/cargo.sh expands CARGO_TARGET_DIR (can yield a relative '-agent-<id>' directory in the repo)"
else ok; fi
if grep -qxF '/-agent-*' "$root/.gitignore"; then ok; else fail ".gitignore lacks the /-agent-* entry"; fi

echo "policy tests: $pass passed, $failn failed"
[ "$failn" -eq 0 ]
