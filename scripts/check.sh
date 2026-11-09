#!/usr/bin/env bash
# Run every quality gate. Nothing may be landed into dev unless this passes.
#   scripts/check.sh                 full gate (formatting, lint, type-check, unit tests, docs, commits)
#   scripts/check.sh --fast          commit-time subset, scoped to the staged files (the pre-commit hook calls this):
#                                    lint of changed shell files, docs checks when docs changed, fmt and clippy
#                                    (affected packages only) when Rust changed. No tests, cargo-deny, policy tests
#                                    or history scan; scripts/land.sh and CI always run the full gate.
#   scripts/check.sh --fast --tests  the fast subset plus `cargo test` for the affected packages (used when batching into an
#                                    integration branch; CHECK_BASE=<rev> sets what "changed" is measured against, default dev)
#   scripts/check.sh --release X.Y.Z additionally verify release readiness
set -u -o pipefail
cd "$(git rev-parse --show-toplevel)" || exit 2

fast=0
tests=0
release=""
while [ $# -gt 0 ]; do
  case "$1" in
    --fast) fast=1 ;;
    --tests) tests=1 ;;
    --release) release="${2:-}"; shift ;;
    *) echo "usage: $0 [--fast [--tests]] [--release X.Y.Z]" >&2; exit 2 ;;
  esac
  shift
done

fail=0
step() { printf '\n==> %s\n' "$1"; }
run() { "$@" || { echo "FAILED: $*" >&2; fail=1; }; }

# Fast mode looks only at what this commit touches: the staged files or, when run by hand with nothing
# staged, whatever differs from dev.
changed=""
if [ "$fast" -eq 1 ]; then
  changed=$(git diff --cached --name-only --diff-filter=ACMR)
  [ -n "$changed" ] || changed=$({ git diff --name-only "${CHECK_BASE:-dev}"...HEAD; git diff --name-only; } 2>/dev/null | sort -u)
fi
changed_matches() { printf '%s\n' "$changed" | grep -qE "$1"; }
shell_re='^(scripts/[^/]+\.sh|\.githooks/[^/]+|\.claude/hooks/[^/]+\.sh)$'
skipped=()

sh_files=()
if [ "$fast" -eq 1 ]; then
  while IFS= read -r f; do
    [ -f "$f" ] && sh_files+=("$f")
  done < <(printf '%s\n' "$changed" | grep -E "$shell_re" || true)
else
  for f in scripts/*.sh .githooks/* .claude/hooks/*.sh; do
    [ -f "$f" ] && sh_files+=("$f")
  done
fi
if [ "${#sh_files[@]}" -gt 0 ]; then
  step "shell scripts: syntax"
  for f in "${sh_files[@]}"; do run bash -n "$f"; done
  if command -v shellcheck >/dev/null 2>&1; then
    step "shell scripts: shellcheck"
    run shellcheck -x "${sh_files[@]}"
  else
    echo "note: shellcheck not installed locally (CI runs it)"
  fi
else
  skipped+=("shell checks")
fi

# The policy tests spin up scratch repositories and are slow (hundreds of process spawns). Locally they
# run only when the policy tooling itself changed (staged, or on this branch relative to dev). CI always
# runs them, which is the authoritative check. Never at commit time.
policy_relevant() {
  [ -n "${CI:-}" ] && return 0
  { git diff --cached --name-only; git diff --name-only dev...HEAD 2>/dev/null; } |
    grep -qE '^(scripts/|\.githooks/|\.claude/|\.shellcheckrc)'
}
if [ "$fast" -eq 0 ] && [ -f scripts/test-policy.sh ] && policy_relevant; then
  step "policy tooling: commit-message rules, agent guard, landing and release flow"
  run scripts/test-policy.sh
fi

# The portal (TypeScript, ADR-0016) is checked only when its sources or the generated page changed (always in
# CI), so Rust-only work never needs Node. The generated crates/core/src/ui.html must match the sources.
portal_re='^(portal/|crates/core/src/ui\.html$)'
portal_relevant() {
  [ -n "${CI:-}" ] && [ "$fast" -eq 0 ] && return 0
  if [ "$fast" -eq 1 ]; then changed_matches "$portal_re"; return; fi
  { git diff --name-only dev...HEAD; git diff --name-only HEAD; git diff --cached --name-only; } 2>/dev/null | grep -qE "$portal_re"
}
if [ -d portal ] && portal_relevant; then
  if ! command -v npm >/dev/null 2>&1; then
    echo "FAILED: the portal changed but Node and npm are not installed (see portal/package.json engines)" >&2
    fail=1
  else
    step "portal: install, type-check, unit tests, built page is current"
    [ -d portal/node_modules ] || run npm --prefix portal ci --ignore-scripts --no-audit --no-fund
    run npm --prefix portal run --silent typecheck
    run npm --prefix portal test --silent
    run npm --prefix portal run --silent check-build
    if [ "$fast" -eq 0 ]; then
      step "portal: licences, audit, browser smoke test"
      run npm --prefix portal run --silent licenses
      run npm --prefix portal run --silent audit
      run npm --prefix portal run --silent test:browser
    fi
  fi
else
  skipped+=("portal checks")
fi

if [ "$fast" -eq 0 ] || changed_matches '\.md$'; then
  step "documentation: links and requirement IDs"
  run scripts/check-docs.sh
else
  skipped+=("docs checks")
fi

# At commit time the message is checked by the commit-msg hook; the history scan belongs to the full gate.
if [ "$fast" -eq 0 ]; then
  step "commit messages and identities"
  range=""
  if git rev-parse --verify -q origin/dev >/dev/null; then
    range="origin/dev..HEAD"
  elif git rev-parse --verify -q dev >/dev/null && [ "$(git rev-parse HEAD)" != "$(git rev-parse dev)" ]; then
    range="dev..HEAD"
  fi
  if [ -n "$range" ]; then run scripts/check-commit-msg.sh --range "$range"; else echo "(no range to check)"; fi
fi

# affected_packages: Cargo package names for changed Rust-relevant files under crates/<dir>/, or the single
# word WORKSPACE when a changed Rust-relevant file lives elsewhere (root manifest, lockfile, cargo config).
rust_re='\.rs$|(^|/)Cargo\.(toml|lock)$|^deny\.toml$|^rust-toolchain|^\.cargo/'
affected_packages() {
  local f dir name
  declare -A seen=()
  while IFS= read -r f; do
    printf '%s\n' "$f" | grep -qE "$rust_re" || continue
    case "$f" in
      crates/*/*)
        dir=${f#crates/}
        dir=${dir%%/*}
        name=$(grep -m1 '^name *=' "crates/$dir/Cargo.toml" 2>/dev/null | sed 's/^name *= *"\(.*\)".*/\1/')
        if [ -z "$name" ]; then
          echo WORKSPACE
          return
        fi
        if [ -z "${seen[$name]:-}" ]; then
          seen[$name]=1
          echo "$name"
        fi
        ;;
      *)
        echo WORKSPACE
        return
        ;;
    esac
  done <<<"$changed"
}

if [ -f Cargo.toml ]; then
  pkg_args=(--workspace)
  rust_wanted=1
  if [ "$fast" -eq 1 ]; then
    mapfile -t pkgs < <(affected_packages)
    if [ "${#pkgs[@]}" -eq 0 ]; then
      rust_wanted=0
    elif ! printf '%s\n' "${pkgs[@]}" | grep -qx WORKSPACE; then
      pkg_args=()
      for p in "${pkgs[@]}"; do pkg_args+=(-p "$p"); done
    fi
  fi
  if [ "$rust_wanted" -eq 1 ]; then
    # scripts/cargo.sh uses native cargo, or WSL when there is no native toolchain. Missing both is a failure.
    step "rust: formatting"
    run scripts/cargo.sh fmt --all --check
    step "rust: lint and type-check (clippy, warnings are errors) [${pkg_args[*]}]"
    run scripts/cargo.sh clippy "${pkg_args[@]}" --all-targets --locked -- -D warnings
    if [ "$fast" -eq 1 ] && [ "$tests" -eq 1 ]; then
      if [ "${#pkg_args[@]}" -gt 0 ] && [ "${pkg_args[0]}" = "-p" ]; then
        step "rust: tests of the touched packages [${pkg_args[*]}]"
        run scripts/cargo.sh test "${pkg_args[@]}" --locked
      else
        echo "note: a workspace-wide Rust change; the full test run is deferred to the landing into dev"
      fi
    fi
    if [ "$fast" -eq 0 ]; then
      step "rust: unit and integration tests"
      run scripts/cargo.sh test --workspace --locked
      # The agent-loop scenarios (REQ-AST-016): scripted model, no network, no spend. `cargo test` above runs
      # them too; this names the gate and prints which scenario broke. Live mode is never run here.
      if [ -d evals/scenarios ]; then
        step "agent-loop scenarios (replay mode; the injection set must pass 100%)"
        run scripts/cargo.sh run -q --locked -p clawdot-eval -- replay
      fi
      if [ -f deny.toml ] && scripts/cargo.sh deny --version >/dev/null 2>&1; then
        step "rust: dependency policy (cargo-deny)"
        run scripts/cargo.sh deny check
      else
        echo "note: cargo-deny is not installed locally (CI enforces it)"
      fi
    fi
  else
    skipped+=("Rust fmt/clippy")
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
if [ "$fast" -eq 1 ]; then
  [ "$tests" -eq 1 ] || skipped+=("tests")
  skipped+=("cargo-deny" "policy tests" "history scan")
  echo "check --fast: skipped (nothing relevant changed, or deferred): ${skipped[*]}. The full gate runs at scripts/land.sh and in CI."
fi
if [ "$fail" -eq 0 ]; then
  if [ "$fast" -eq 1 ]; then echo "check --fast: passed"; else echo "check: all gates passed"; fi
else
  echo "check: FAILED" >&2
fi
exit "$fail"
