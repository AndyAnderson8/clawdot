#!/usr/bin/env bash
# Documentation checks: relative Markdown links resolve; requirement IDs are unique within a spec.
set -u
cd "$(git rev-parse --show-toplevel)" || exit 2

fail=0

while IFS= read -r -d '' f; do
  dir=$(dirname "$f")
  while IFS= read -r link; do
    target="${link%%#*}"
    [ -n "$target" ] || continue
    case "$target" in http://* | https://* | mailto:*) continue ;; esac
    if [ ! -e "$dir/$target" ]; then
      echo "docs: broken link in $f -> $link" >&2
      fail=1
    fi
  done < <(grep -oE '\]\([^)]+\)' "$f" | sed -E 's/^\]\(//; s/\)$//')
done < <(find . -name '*.md' -not -path './.git/*' -not -path './node_modules/*' -not -path './target/*' -print0)

for spec in docs/specs/[0-9]*.md; do
  [ -f "$spec" ] || continue
  dups=$(grep -oE '^\| REQ-[A-Z]+-[0-9]+' "$spec" | sort | uniq -d)
  if [ -n "$dups" ]; then
    echo "docs: duplicate requirement IDs in $spec:" >&2
    echo "$dups" >&2
    fail=1
  fi
done

exit "$fail"
