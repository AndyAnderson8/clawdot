#!/usr/bin/env bash
# Run every quality gate. Nothing may be landed into dev unless this passes.
#   scripts/check.sh                 full gate (formatting, lint, type-check, unit tests, docs, commits)
#   scripts/check.sh --fast          pre-commit subset (skips the test run)
#   scripts/check.sh --release X.Y.Z additionally verify release readiness
set -u -o pipefail
cd "$(git rev-parse --show-toplevel)" || exit 2

fast=0
release=""
while [ $# -gt 0 ]; do
  case "$1" in
    --fast) fast=1 ;;
    --release) release="${2:-}"; shift ;;
    *) echo "usage: $0 [--fast] [--release X.Y.Z]" >&2; exit 2 ;;
  esac
  shift
done

fail=0
step() { printf '\n==> %s\n' "$1"; }
run() { "$@" || { echo "FAILED: $*" >&2; fail=1; }; }

step "shell scripts: syntax"
for f in scripts/*.sh .githooks/* .claude/hooks/*.sh; do
  [ -f "$f" ] || continue
  run bash -n "$f"
done
if command -v shellcheck >/dev/null 2>&1; then
  step "shell scripts: shellcheck"
  sc_files=()
  for f in scripts/*.sh .githooks/* .claude/hooks/*.sh; do [ -f "$f" ] && sc_files+=("$f"); done
  if [ "${#sc_files[@]}" -gt 0 ]; then run shellcheck -x "${sc_files[@]}"; fi
else
  echo "note: shellcheck not installed locally (CI runs it)"
fi

# The policy tests spin up scratch repositories and are slow (hundreds of process spawns). Locally they
# run only when the policy tooling itself changed (staged, or on this branch relative to dev). CI always
# runs them, which is the authoritative check.
policy_relevant() {
  [ -n "${CI:-}" ] && return 0
  { git diff --cached --name-only; git diff --name-only dev...HEAD 2>/dev/null; } |
    grep -qE '^(scripts/|\.githooks/|\.claude/|\.shellcheckrc)'
}
if [ -f scripts/test-policy.sh ] && policy_relevant; then
  step "policy tooling: commit-message rules, agent guard, landing and release flow"
  run scripts/test-policy.sh
fi

step "documentation: links and requirement IDs"
run scripts/check-docs.sh

step "commit messages and identities"
range=""
if git rev-parse --verify -q origin/dev >/dev/null; then
  range="origin/dev..HEAD"
elif git rev-parse --verify -q dev >/dev/null && [ "$(git rev-parse HEAD)" != "$(git rev-parse dev)" ]; then
  range="dev..HEAD"
fi
if [ -n "$range" ]; then run scripts/check-commit-msg.sh --range "$range"; else echo "(no range to check)"; fi

if [ -f Cargo.toml ]; then
  # scripts/cargo.sh uses native cargo, or WSL when there is no native toolchain. Missing both is a failure.
  step "rust: formatting"
  run scripts/cargo.sh fmt --all --check
  step "rust: lint and type-check (clippy, warnings are errors)"
  run scripts/cargo.sh clippy --workspace --all-targets --locked -- -D warnings
  if [ "$fast" -eq 0 ]; then
    step "rust: unit and integration tests"
    run scripts/cargo.sh test --workspace --locked
    if [ -f deny.toml ] && scripts/cargo.sh deny --version >/dev/null 2>&1; then
      step "rust: dependency policy (cargo-deny)"
      run scripts/cargo.sh deny check
    else
      echo "note: cargo-deny is not installed locally (CI enforces it)"
    fi
  fi
else
  echo "note: no Cargo.toml yet; Rust gates activate automatically once the workspace exists"
fi

if [ -n "$release" ]; then
  step "release readiness: $release"
  grep -q "^## \[$release\]" CHANGELOG.md || { echo "FAILED: CHANGELOG.md has no entry for $release" >&2; fail=1; }
  [ -z "$(git status --porcelain)" ] || { echo "FAILED: working tree is not clean" >&2; fail=1; }
  if [ -f Cargo.toml ] && grep -q '^\[workspace.package\]' Cargo.toml; then
    cv=$(awk '/^\[workspace.package\]/{s=1;next} /^\[/{s=0} s && /^version *=/{gsub(/[" ]/,"",$0); sub(/version=/,"",$0); print; exit}' Cargo.toml)
    [ "$cv" = "$release" ] || { echo "FAILED: Cargo workspace version '$cv' != $release" >&2; fail=1; }
  fi
fi

echo
if [ "$fail" -eq 0 ]; then echo "check: all gates passed"; else echo "check: FAILED" >&2; fi
exit "$fail"
