#!/usr/bin/env bash
# One-time local setup after cloning: install git hook stubs and verify the git identity.
#
# The stubs live in .git/hooks and delegate to the versioned hooks in .githooks/. Branches that
# predate the hooks (for example `main`, which holds only releases and was the empty base commit
# before the first release) have no .githooks/, so the stubs fail closed there.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

# Always the real hooks directory. Never `git rev-parse --git-path hooks`: it honours a hooks-path
# setting and could point at the versioned .githooks/ directory, which these stubs would overwrite.
git config --unset core.hooksPath 2>/dev/null || true
hooks_dir="$(git rev-parse --git-common-dir)/hooks"
mkdir -p "$hooks_dir"
for name in pre-commit commit-msg pre-merge-commit pre-push; do
  cat >"$hooks_dir/$name" <<'STUB'
#!/usr/bin/env bash
# ClawDot hook stub (installed by scripts/setup.sh). Delegates to the versioned hook in .githooks/.
name=$(basename "$0")
root=$(git rev-parse --show-toplevel)
if [ -f "$root/.githooks/$name" ]; then exec bash "$root/.githooks/$name" "$@"; fi
branch=$(git symbolic-ref --quiet --short HEAD 2>/dev/null || true)
case "$name" in
  pre-commit | pre-merge-commit)
    if [ "$branch" = main ] && git rev-parse --verify -q HEAD >/dev/null; then
      echo "error: commits and merges on main are never allowed (the versioned hooks are absent on this branch)." >&2
      exit 1
    fi
    ;;
esac
exit 0
STUB
  chmod +x "$hooks_dir/$name"
done

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
echo "Hooks installed. Commits will be authored as: $name <$email>"
