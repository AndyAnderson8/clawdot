#!/usr/bin/env bash
# Documentation checks: relative Markdown links resolve; requirement IDs are unique within a spec.
#
# PERFORMANCE: process creation is slow on Windows (about 150 ms each), so links are collected with ONE
# grep over the whole tree and checked with bash builtins only.
set -u
cd "$(git rev-parse --show-toplevel)" || exit 2

fail=0

# Each line looks like "./docs/x.md:](../y.md)".
while IFS= read -r line; do
  file=${line%%:\](*}
  link=${line#*:\](}
  link=${link%)}
  target=${link%%#*}
  [[ -n $target ]] || continue
  case $target in http://* | https://* | mailto:*) continue ;; esac
  if [[ ! -e ${file%/*}/$target ]]; then
    echo "docs: broken link in $file -> $link" >&2
    fail=1
  fi
done < <(grep -rHoE --include='*.md' --exclude-dir=.git --exclude-dir=node_modules --exclude-dir=target '\]\([^)]+\)' .)

for spec in docs/specs/[0-9]*.md; do
  [[ -f $spec ]] || continue
  declare -A seen=()
  while IFS= read -r id; do
    if [[ -n ${seen[$id]:-} ]]; then
      echo "docs: duplicate requirement ID $id in $spec" >&2
      fail=1
    fi
    seen[$id]=1
  done < <(grep -oE '^\| REQ-[A-Z]+-[0-9]+' "$spec")
  unset seen
done

exit "$fail"
