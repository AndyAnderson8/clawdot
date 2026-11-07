@AGENTS.md

## Claude Code specifics

- A `PreToolUse` hook (`.claude/hooks/guard-git.sh`, configured in `.claude/settings.json`) blocks git commands that break the policy in AGENTS.md. If it blocks you, change your approach; do not try to evade it.
- Prefer the dedicated file tools over shell for reading and editing; use Bash for git and for running `scripts/check.sh`.
- Do not add attribution to commits or PRs.
