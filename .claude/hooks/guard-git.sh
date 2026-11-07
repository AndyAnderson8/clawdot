#!/usr/bin/env bash
# Claude Code PreToolUse guard for the Bash tool. Blocks git/gh actions that violate
# docs/process/git-workflow.md. Exit 2 = block; stderr is shown to the agent.
# This is the first of several layers: git hooks (.githooks), CI, and server rules back it up.
input=$(cat)
h=$(printf '%s' "$input" | tr '[:upper:]' '[:lower:]')

block() {
  echo "BLOCKED by the ClawDot git-flow policy: $1" >&2
  echo "Read AGENTS.md and docs/process/git-workflow.md. If you believe this is wrong, ask the human instead of working around it." >&2
  exit 2
}
has() { printf '%s' "$h" | LC_ALL=C grep -Eq "$1"; }

# Only commands that touch git or gh are relevant.
has '(^|[^a-z])(git|gh)[[:space:]]' || exit 0

cd "${CLAUDE_PROJECT_DIR:-.}" 2>/dev/null || true
# shellcheck source=../../scripts/lib-git.sh
. scripts/lib-git.sh 2>/dev/null || exit 0
branch=$(current_branch)

# 1. Never bypass hooks, signing, or identity controls.
has '(--no-verify|--no-gpg-sign|git[[:space:]]+commit[^|;&\]*[[:space:]]-[a-z]*n[a-z]*([[:space:]]|\\|"|$))' &&
  block "bypassing hooks (--no-verify / -n) is not allowed."
# Reading identity or hook settings (git config --get / --list) is fine; changing them is not.
reading_config=0
has 'git[[:space:]]+config[[:space:]]+(--get|--list|-l)' && reading_config=1
if [ "$reading_config" -eq 0 ] && has 'core\.hookspath|git[[:space:]]+config[^|;&\]*(user\.name|user\.email|user\.signingkey)'; then
  block "do not change git identity or hook configuration (read it with 'git config --get'). Commits are authored by the configured human only."
fi
has 'rm[^|;&\]*\.githooks|[[:space:]]-c[[:space:]]+(user\.(name|email)|core\.hookspath)|--author|git_(author|committer)_(name|email)' &&
  block "do not change git identity or hook configuration. Commits are authored by the configured human only."

# 2. No AI attribution anywhere.
has 'co-authored-by|generated (with|by)[^|;&\]*(claude|anthropic|chatgpt|openai|copilot|gemini|codex|cursor|aider|opencode)|noreply@anthropic\.com|claude\.ai/code|🤖' &&
  block "no AI attribution in commits or PRs. Commits are authored by the human identity only."

# 3. Shared-history protection.
has 'git[[:space:]]+push[^|;&\]*(--force|--mirror|[[:space:]]-f([[:space:]]|\\|"|$)|[[:space:]]\+[a-z])' &&
  block "force-pushing is not allowed."
has 'git[[:space:]]+push[^|;&\]*([[:space:]]|:|/)(main|dev)([[:space:]"\\]|$)' &&
  block "never push to main or dev directly. Use scripts/land.sh (dev) or scripts/release.sh (main)."
has 'git[[:space:]]+push[^|;&\]*(--delete|[[:space:]]:[a-z])' &&
  block "deleting remote branches by hand is not allowed; scripts/land.sh cleans up merged branches."
has 'git[[:space:]]+branch[^|;&\]*-d[^|;&\]*[[:space:]](main|dev)([[:space:]"\\]|$)' &&
  block "never delete main or dev."
has 'git[[:space:]]+tag[[:space:]]+(-[asdf]|[^-])' &&
  block "tags are created only by scripts/release.sh."
has 'gh[[:space:]]+(pr[[:space:]]+merge|release[[:space:]]+(create|delete)|repo[[:space:]]+(create|delete))' &&
  block "outward-facing GitHub actions require the human."

# 4. Branch discipline.
if has 'git[[:space:]]+(commit|merge|cherry-pick|revert|rebase|am)([[:space:]"\\]|$)'; then
  case "$branch" in
    main)
      block "never commit, merge, or rebase on main. Work on a branch; releases use scripts/release.sh."
      ;;
    dev)
      if has 'git[[:space:]]+commit'; then
        tiny_staged_ok ||
          block "direct commits to dev are only for ONE tiny change (max ${TINY_MAX_LINES} lines, ${TINY_MAX_FILES} files, already staged in a previous step). Create a branch: git switch -c feature/<name> (or bugfix/, docs/, chore/)."
      else
        block "on dev, integrate with scripts/land.sh; do not merge, rebase, or cherry-pick here."
      fi
      ;;
    *)
      if has 'git[[:space:]]+commit' && [ "$branch" != "(detached)" ] && ! valid_branch_name "$branch"; then
        block "branch '$branch' is not a valid git-flow name: (feature|bugfix|hotfix|release|docs|chore|spike)/<kebab-name>."
      fi
      ;;
  esac
fi
exit 0
