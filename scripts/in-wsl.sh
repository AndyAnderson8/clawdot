#!/usr/bin/env bash
# Run a command inside WSL at the same repository path, with the ClawDot build environment loaded
# (see scripts/setup-wsl.sh). Used by scripts that need Linux, such as scripts/budget.sh.
#   scripts/in-wsl.sh <command> [args...]
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

command -v wsl.exe >/dev/null 2>&1 || { echo "WSL is not available" >&2; exit 127; }
win_root=$(cygpath -m "$PWD")
args=""
for a in "$@"; do args+=" $(printf '%q' "$a")"; done
cmd=". \"\$HOME/.clawdot-env.sh\" && cd \"\$(wslpath -a '$win_root')\" &&$args"
MSYS_NO_PATHCONV=1 exec wsl.exe -d "${CLAWDOT_WSL_DISTRO:-Ubuntu}" -- bash -lc "$cmd"
