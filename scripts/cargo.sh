#!/usr/bin/env bash
# Run cargo natively when it is installed; otherwise run it inside WSL (Windows machines without a C
# toolchain). One-time WSL setup is described in docs/process/dev-environment.md. The build output lives
# in the WSL home directory (fast), not in the repository.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

if command -v cargo >/dev/null 2>&1; then exec cargo "$@"; fi
command -v wsl.exe >/dev/null 2>&1 || { echo "cargo not found and no WSL available" >&2; exit 127; }

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
if [ -n "$target_suffix" ]; then target_export="export CARGO_TARGET_DIR=\$CARGO_TARGET_DIR$target_suffix && "; fi
cmd=". \"\$HOME/.clawdot-env.sh\" && ${target_export}cd \"\$(wslpath -a '$win_root')\" && cargo$args"
MSYS_NO_PATHCONV=1 exec wsl.exe -d "${CLAWDOT_WSL_DISTRO:-Ubuntu}" -- bash -lc "$cmd"
