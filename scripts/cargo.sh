#!/usr/bin/env bash
# Run cargo natively when it is installed; otherwise run it inside WSL (Windows machines without a C
# toolchain). One-time WSL setup is described in docs/process/dev-environment.md. The build output lives
# in the WSL home directory (fast), not in the repository.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

if command -v cargo >/dev/null 2>&1; then exec cargo "$@"; fi
command -v wsl.exe >/dev/null 2>&1 || { echo "cargo not found and no WSL available" >&2; exit 127; }

win_root=$(cygpath -m "$PWD")
args=""
for a in "$@"; do args+=" $(printf '%q' "$a")"; done
cmd=". \"\$HOME/.clawdot-env.sh\" && cd \"\$(wslpath -a '$win_root')\" && cargo$args"
MSYS_NO_PATHCONV=1 exec wsl.exe -d "${CLAWDOT_WSL_DISTRO:-Ubuntu}" -- bash -lc "$cmd"
