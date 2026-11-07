#!/usr/bin/env bash
# Validate commit messages and identities against docs/process/git-workflow.md.
#   check-commit-msg.sh --file <message-file>   validate one message (commit-msg hook)
#   check-commit-msg.sh --range <rev-range>     validate every commit in a range (CI, land)
set -u

TYPES='feat|fix|docs|spec|refactor|perf|test|build|ci|chore|revert'
SUBJECT_RE="^(${TYPES})(\([a-z0-9-]+\))?!?: [^ ].*[^.]$"
# Commits are authored by the human only: no AI identities or attribution.
AI_RE='claude|anthropic|chatgpt|openai|copilot|gemini|codex|cursor|aider|opencode'

fail=0
bad() { echo "commit-msg: $1" >&2; fail=1; }

check_message() { # check_message <file> <label>
  local file="$1" label="$2" subject second body
  body=$(grep -v '^#' "$file")
  subject=$(printf '%s\n' "$body" | sed -n '1p')
  second=$(printf '%s\n' "$body" | sed -n '2p')

  case "$subject" in
    Merge\ * | Revert\ \"*) ;; # merge commits and git-generated reverts are exempt
    *)
      [[ "$subject" =~ $SUBJECT_RE ]] ||
        bad "$label: subject must look like 'type(scope): summary' with type in ${TYPES//|/, } and no trailing period: '$subject'"
      [ "${#subject}" -le 72 ] || bad "$label: subject is ${#subject} characters (max 72)"
      ;;
  esac
  [ -z "$second" ] || bad "$label: second line must be blank (subject, blank line, then body)"

  if printf '%s\n' "$body" | grep -iqE "^co-authored-by:.*(${AI_RE})" ||
    printf '%s\n' "$body" | grep -iqE "generated (with|by).*(${AI_RE})" ||
    printf '%s\n' "$body" | grep -iqE 'noreply@anthropic\.com|claude\.ai/code' ||
    printf '%s\n' "$body" | grep -qF '🤖'; then
    bad "$label: AI attribution is not allowed in commit messages; commits are authored by the human only"
  fi
}

check_range() { # check_range <rev-range>
  local range="$1" sha ident tmp
  for sha in $(git rev-list "$range"); do
    ident=$(git log -1 --format='%an <%ae> | %cn <%ce>' "$sha")
    if printf '%s' "$ident" | grep -iqE "$AI_RE"; then
      bad "$(git log -1 --format=%h "$sha"): author/committer must be a human identity, got: $ident"
    fi
  done
  for sha in $(git rev-list --no-merges "$range"); do
    tmp=$(mktemp)
    git log -1 --format=%B "$sha" >"$tmp"
    check_message "$tmp" "$(git log -1 --format=%h "$sha")"
    rm -f "$tmp"
  done
}

case "${1:-}" in
  --file) [ -n "${2:-}" ] || { echo "usage: $0 --file <file>" >&2; exit 2; }; check_message "$2" "message" ;;
  --range) [ -n "${2:-}" ] || { echo "usage: $0 --range <rev-range>" >&2; exit 2; }; check_range "$2" ;;
  *) echo "usage: $0 --file <file> | --range <rev-range>" >&2; exit 2 ;;
esac
exit "$fail"
