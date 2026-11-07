#!/usr/bin/env bash
# One-time GitHub repository configuration (human-run). See docs/process/repo-settings.md.
# Usage: scripts/setup-github.sh OWNER/REPO
# Requires: gh CLI authenticated as a repository admin. NOT yet exercised against a real repository.
set -euo pipefail
repo="${1:-}"
[ -n "$repo" ] || { echo "usage: $0 OWNER/REPO" >&2; exit 2; }
command -v gh >/dev/null 2>&1 || { echo "gh CLI is required" >&2; exit 1; }

echo "Configuring $repo ..."
gh api -X PATCH "repos/$repo" \
  -f default_branch=dev \
  -F allow_merge_commit=true -F allow_squash_merge=false -F allow_rebase_merge=false \
  -F delete_branch_on_merge=true >/dev/null

ruleset() { # ruleset <name> <ref-pattern> <with-status-check: true|false>
  local name="$1" ref="$2" checks="$3" rules
  rules='{"type":"deletion"},{"type":"non_fast_forward"}'
  if [ "$checks" = "true" ]; then
    rules="$rules"',{"type":"required_status_checks","parameters":{"strict_required_status_checks_policy":false,"required_status_checks":[{"context":"gate"}]}}'
  fi
  gh api -X POST "repos/$repo/rulesets" --input - >/dev/null <<EOF
{
  "name": "$name",
  "target": "$( [ "${ref#refs/tags/}" != "$ref" ] && echo tag || echo branch )",
  "enforcement": "active",
  "conditions": { "ref_name": { "include": ["$ref"], "exclude": [] } },
  "rules": [ $rules ]
}
EOF
  echo "  ruleset: $name"
}

ruleset "main" "refs/heads/main" true
ruleset "dev" "refs/heads/dev" true
ruleset "release tags" "refs/tags/v*" false

echo "Done. Verify with the checklist in docs/process/repo-settings.md."
