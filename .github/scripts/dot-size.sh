#!/usr/bin/env bash
# Size gate for the binary that runs on the Echo Dot (spec 0002, REQ-NFR-103: stripped release binary <= 8 MiB).
#
#   .github/scripts/dot-size.sh <dot|full> <path-to-binary>
#
# Prints the size and the change since the last recorded measurement to the job summary, and fails above the budget.
# The recorded sizes below are the 2026-11-11 measurements (docs/reports/2026-11-11-proving-pass.md). They are only
# for the printed delta and a warning; the limit itself is the spec's. Update them when a size change is accepted.
set -euo pipefail

label=${1:?label (dot or full)}
bin=${2:?path to the binary}

# Spec 0002 REQ-NFR-103.
limit=$((8 * 1024 * 1024))
# A growth beyond this since the recorded size is called out (not failed) so it gets justified in the PR.
warn_growth=$((100 * 1024))

case "$label" in
  dot) baseline=3247568 ;;
  full) baseline=3348880 ;;
  *) echo "unknown label: $label" >&2; exit 2 ;;
esac

[ -f "$bin" ] || { echo "no binary at $bin" >&2; exit 1; }
if command -v file >/dev/null 2>&1; then
  kind=$(file -b "$bin")
  case "$kind" in
    *ELF*32-bit*ARM*) ;;
    *) echo "not a 32-bit ARM ELF: $kind" >&2; exit 1 ;;
  esac
fi

size=$(stat -c %s "$bin")
delta=$((size - baseline))
sha=$(sha256sum "$bin" | cut -d' ' -f1)

status=ok
if [ "$size" -gt "$limit" ]; then status=FAIL; fi

{
  echo "### Dot binary ($label), armv7 musl"
  echo
  echo "| Size | Limit | Since 2026-11-11 | Headroom | Result |"
  echo "|---|---|---|---|---|"
  printf '| %s bytes | %s bytes | %+d bytes | %s bytes | %s |\n' "$size" "$limit" "$delta" "$((limit - size))" "$status"
  echo
  echo "SHA-256: \`$sha\`"
  echo
} | tee -a "${GITHUB_STEP_SUMMARY:-/dev/null}"

if [ "$size" -gt "$limit" ]; then
  echo "::error title=Dot binary over budget::$label is $size bytes; the limit is $limit (REQ-NFR-103)"
  exit 1
fi
if [ "$delta" -gt "$warn_growth" ]; then
  echo "::warning title=Dot binary grew::$label grew by $delta bytes since the recorded size; justify it in the PR (REQ-NFR-114)"
fi
