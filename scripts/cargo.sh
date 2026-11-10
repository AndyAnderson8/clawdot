#!/usr/bin/env bash
# Run cargo natively when it is installed; otherwise run it inside WSL (Windows machines without a C
# toolchain). One-time WSL setup is described in docs/process/dev-environment.md. The build output lives
# in the WSL home directory (fast), not in the repository.
# Optional compiler cache: when sccache is installed it is used as RUSTC_WRAPPER, with the cache kept OUTSIDE the
# repository (default ~/.cache/clawdot-sccache, capped at 10G). Worktrees keep separate target directories but
# share this cache, so a second worktree rebuilds dependencies from cache. CLAWDOT_NO_SCCACHE=1 turns it off, and an
# RUSTC_WRAPPER you set yourself is respected. Without sccache nothing changes.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

if command -v cargo >/dev/null 2>&1; then
  if [ -z "${RUSTC_WRAPPER:-}" ] && [ -z "${CLAWDOT_NO_SCCACHE:-}" ] && command -v sccache >/dev/null 2>&1; then
    export RUSTC_WRAPPER=sccache
    export SCCACHE_DIR="${SCCACHE_DIR:-$HOME/.cache/clawdot-sccache}"
    export SCCACHE_CACHE_SIZE="${SCCACHE_CACHE_SIZE:-10G}"
  fi
  exec cargo "$@"
fi
command -v wsl.exe >/dev/null 2>&1 || { echo "cargo not found and no WSL available" >&2; exit 127; }

# wsl.exe expands $VARIABLES before the inner shell runs, so the base directory is spelled out here and
# must match ~/.clawdot-env.sh.
# A linked worktree gets its own build directory. Cargo makes artifact hashes independent of the absolute
# workspace path, so worktrees sharing one target directory overwrite each other's builds.
target_suffix=""
if [ "$(git rev-parse --git-dir)" != "$(git rev-parse --git-common-dir)" ]; then
  target_suffix="-$(basename "$PWD")"
fi

win_root=$(cygpath -m "$PWD")
args=""
for a in "$@"; do args+=" $(printf '%q' "$a")"; done
target_export=""
if [ -n "$target_suffix" ]; then target_export="export CARGO_TARGET_DIR=\$HOME/.cache/clawdot-target$target_suffix && "; fi
# sccache inside WSL, if installed there (same rules as the native branch above).
cache_export=""
if [ -z "${CLAWDOT_NO_SCCACHE:-}" ]; then
  # shellcheck disable=SC2016 # the variables must expand inside WSL, not here
  cache_export='if [ -z "${RUSTC_WRAPPER:-}" ] && command -v sccache >/dev/null 2>&1; then export RUSTC_WRAPPER=sccache SCCACHE_DIR="${SCCACHE_DIR:-$HOME/.cache/clawdot-sccache}" SCCACHE_CACHE_SIZE="${SCCACHE_CACHE_SIZE:-10G}"; fi && '
fi
cmd=". \"\$HOME/.clawdot-env.sh\" && ${cache_export}${target_export}cd \"\$(wslpath -a '$win_root')\" && cargo$args"
MSYS_NO_PATHCONV=1 exec wsl.exe -d "${CLAWDOT_WSL_DISTRO:-Ubuntu}" -- bash -lc "$cmd"
