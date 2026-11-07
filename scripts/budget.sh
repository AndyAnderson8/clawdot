#!/usr/bin/env bash
# Resource-budget gate (spec 0002: REQ-NFR-101, 102, 103, 114).
#
# Builds the release binary, checks its size, runs it through a load scenario, and checks idle and peak
# resident memory. Needs Linux; on Windows it re-runs itself inside WSL (scripts/in-wsl.sh).
#
#   scripts/budget.sh            measure and enforce the gates
#   scripts/budget.sh --report   measure and print, never fail
#
# What is and is not measured here:
#  * Binary size is measured for x86_64 and, when the cross toolchain is installed, 32-bit ARM
#    (armv7-unknown-linux-musleabihf), which is the Echo Dot Gen 2 userland.
#  * Memory is measured by RUNNING the x86_64 musl build. There is no armv7 emulation in this script, so
#    ARM memory is not measured; 32-bit pointers make real ARM use somewhat smaller, not larger.
#  * CPU limits are not measured here; they need real hardware (spec 0002 section 5).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
if [ "$(uname -s)" != Linux ]; then exec scripts/in-wsl.sh scripts/budget.sh "$@"; fi

report_only=0
[ "${1:-}" = "--report" ] && report_only=1

MAX_BIN_BYTES=$((8 * 1024 * 1024))
IDLE_KB=$((24 * 1024))
TARGET_KB=$((48 * 1024))
HARD_KB=$((64 * 1024))
HOST=x86_64-unknown-linux-musl
ARM=armv7-unknown-linux-musleabihf
tdir=${CARGO_TARGET_DIR:-target}
fail=0
warn=0

kb() { awk -v k="$1" 'BEGIN { printf "%.1f MiB", k / 1024 }'; }
mb() { awk -v b="$1" 'BEGIN { printf "%.2f MiB", b / 1048576 }'; }

build() { # build <triple>
  # Static musl builds link with zig (see scripts/setup-wsl.sh for the zcc-* wrappers).
  local wrapper="$HOME/bin/zcc-musl64"
  [ "$1" = "$ARM" ] && wrapper="$HOME/bin/zcc-armv7"
  if [ -x "$wrapper" ]; then
    local up
    up=$(echo "$1" | tr 'a-z-' 'A-Z_')
    export "CC_${1//-/_}=$wrapper" "CARGO_TARGET_${up}_LINKER=$wrapper"
    export "CARGO_TARGET_${up}_RUSTFLAGS=-C link-self-contained=no"
  fi
  cargo build --release --locked -q -p clawdot-core --bin clawdot --target "$1"
}

echo "Building $HOST ..."
build "$HOST"
host_bin="$tdir/$HOST/release/clawdot"
host_size=$(stat -c %s "$host_bin")

arm_size=""
if rustup target list --installed 2>/dev/null | grep -q "^$ARM\$" && [ -x "$HOME/bin/zcc-armv7" ]; then
  echo "Building $ARM ..."
  if build "$ARM"; then arm_size=$(stat -c %s "$tdir/$ARM/release/clawdot"); else echo "note: the armv7 build failed" >&2; fi
else
  echo "note: armv7 cross toolchain not installed; skipping the ARM size (see scripts/setup-wsl.sh)"
fi

# --- run the scenario -----------------------------------------------------------------------------
data=$(mktemp -d)
out="$data/out.txt"
port=$((20000 + RANDOM % 30000))
"$host_bin" --no-console --listen "127.0.0.1:$port" --data-dir "$data/state" --create-endpoint claude-code >"$out" 2>&1 &
pid=$!
trap 'kill "$pid" 2>/dev/null || true; rm -rf "$data"' EXIT

for _ in $(seq 1 50); do
  curl -sf "http://127.0.0.1:$port/healthz" >/dev/null 2>&1 && break
  sleep 0.2
done
curl -sf "http://127.0.0.1:$port/healthz" >/dev/null || { echo "the server did not start:"; cat "$out"; exit 1; }
token=$(grep -o 'cdt_[A-Za-z0-9_-]*' "$out" | head -1)
sleep 3
idle_kb=$(awk '/^VmRSS:/ {print $2}' "/proc/$pid/status")

# 400 hook posts from 60 sessions carrying ~3 KB of text each: exercises parsing, adapters, coalescing,
# eviction, persistence, and the announcer.
text=$(head -c 3000 /dev/zero | tr '\0' 'x')
for i in $(seq 1 400); do
  curl -s -o /dev/null -X POST "http://127.0.0.1:$port/hook/claude-code" \
    -H "Authorization: Bearer $token" -H 'Content-Type: application/json' \
    -d "{\"session_id\":\"s$((i % 60))\",\"cwd\":\"/home/dev/p$((i % 7))\",\"hook_event_name\":\"Stop\",\"last_assistant_message\":\"$text\"}"
done
sleep 2
peak_kb=$(awk '/^VmHWM:/ {print $2}' "/proc/$pid/status")
state_bytes=$(du -sb "$data/state" | cut -f1)

# --- report and gates -----------------------------------------------------------------------------
echo
echo "| Measure | Value | Limit | Result |"
echo "|---|---|---|---|"
check() { # check <label> <value-text> <limit-text> <over-hard:0|1> <over-target:0|1>
  local result="ok"
  if [ "$4" -eq 1 ]; then result="FAIL"; fail=1; elif [ "$5" -eq 1 ]; then result="over target"; warn=1; fi
  printf '| %s | %s | %s | %s |\n' "$1" "$2" "$3" "$result"
}
check "Binary size, x86_64 musl" "$(mb "$host_size")" "8 MiB" "$((host_size > MAX_BIN_BYTES))" 0
[ -n "$arm_size" ] && check "Binary size, armv7 musl" "$(mb "$arm_size")" "8 MiB" "$((arm_size > MAX_BIN_BYTES))" 0
check "Idle RSS (x86_64)" "$(kb "$idle_kb")" "24 MiB" "$((idle_kb > IDLE_KB))" 0
check "Peak RSS under load (x86_64)" "$(kb "$peak_kb")" "64 MiB hard, 48 MiB target" "$((peak_kb > HARD_KB))" "$((peak_kb > TARGET_KB))"
printf '| %s | %s | %s | %s |\n' "State on disk after load" "$(awk -v b="$state_bytes" 'BEGIN { printf "%.0f KiB", b / 1024 }')" "16 MiB" "$([ "$state_bytes" -le $((16 * 1048576)) ] && echo ok || echo FAIL)"
[ "$state_bytes" -le $((16 * 1048576)) ] || fail=1

if [ "$report_only" -eq 1 ]; then exit 0; fi
if [ "$fail" -ne 0 ]; then echo; echo "budget: FAILED (see REQ-NFR-114: tighten, never loosen, without an ADR)" >&2; exit 1; fi
[ "$warn" -eq 0 ] || echo "budget: over the design target but within the hard limit"
echo "budget: within limits"
