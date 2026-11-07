#!/usr/bin/env bash
# One-time local setup after cloning: enable the git hooks and verify the git identity.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
git config core.hooksPath .githooks
git config pull.ff only
git config merge.ff false
chmod +x scripts/*.sh .githooks/* .claude/hooks/*.sh 2>/dev/null || true
name=$(git config user.name || true)
email=$(git config user.email || true)
if [ -z "$name" ] || [ -z "$email" ]; then
  echo "Set your git identity (a human identity, never an AI):" >&2
  echo "  git config user.name 'Your Name'; git config user.email you@example.com" >&2
  exit 1
fi
echo "Hooks enabled. Commits will be authored as: $name <$email>"
