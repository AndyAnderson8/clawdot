#!/usr/bin/env bash
# Release procedure (git flow). Full runbook with examples: docs/process/runbook.md
#
#   scripts/release.sh prepare [X.Y.Z]   branch release/X.Y.Z from dev, regenerate CHANGELOG.md, commit
#                                        (anyone, including agents; then land it with scripts/land.sh)
#   scripts/release.sh publish X.Y.Z     HUMAN ONLY. Merge dev into main WITH A MERGE COMMIT (after CI
#                                        passes on that exact commit), tag the merge commit, push the tag.
#                                        The tag triggers the release workflow (build + GitHub Release).
#   scripts/release.sh tag X.Y.Z         HUMAN ONLY. Tag the tip of main (used after a hotfix landed on main).
#
# `prepare` needs git-cliff (https://git-cliff.org); with no version it computes the next SemVer from the
# Conventional Commits since the last tag. Publishing is outward-facing, so `publish` and `tag` refuse to run
# without an interactive terminal and the typed confirmation.
# NOTE: first exercised for real at v0.1.0; rehearse on a throwaway repository first.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
# shellcheck source=scripts/lib-git.sh
. scripts/lib-git.sh

cmd="${1:-}"
ver="${2:-}"
semver_re='^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$'

confirm_human() { # publishing needs a person at a terminal who types the tag back
  if [ ! -t 0 ] || [ ! -t 1 ]; then
    die "this step publishes a release and must be run by the human in an interactive terminal. Agents: stop here and ask the human to run: scripts/release.sh $cmd $ver"
  fi
  local answer
  read -r -p "Type v$ver to confirm: " answer
  [ "$answer" = "v$ver" ] || die "not confirmed"
}

tag_main() { # annotated tag on the tip of main (signed if signing is configured), pushed if there is a remote
  local sign=(-a)
  [ "$(git config --get commit.gpgsign || true)" = "true" ] && sign=(-s)
  git checkout -q main
  git tag "${sign[@]}" "v$ver" -m "ClawDot v$ver"
  if has_remote; then CLAWDOT_LANDING=1 git push origin "v$ver"; fi
  echo "Tagged v$ver on $(git rev-parse --short main). The release workflow now builds and publishes it."
}

case "$cmd" in
  prepare)
    [ "$(current_branch)" = "dev" ] || die "run prepare from dev"
    [ -z "$(git status --porcelain)" ] || die "working tree is not clean"
    command -v git-cliff >/dev/null 2>&1 || die "git-cliff is required to generate the changelog"
    sync_ff dev
    [ -n "$ver" ] || ver=$(git cliff --bumped-version | sed 's/^v//')
    [[ "$ver" =~ $semver_re ]] || die "'$ver' is not a SemVer version"
    git switch -c "release/$ver"
    git cliff --tag "v$ver" --output CHANGELOG.md
    if [ -f Cargo.toml ] && command -v cargo-set-version >/dev/null 2>&1; then cargo set-version --workspace "$ver"; fi
    git add -A
    git commit -m "chore(release): v$ver" -m "Update the changelog and version for v$ver."
    echo "release/$ver is ready. Review CHANGELOG.md, then:"
    echo "  1. scripts/land.sh release/$ver             (lands it into dev like any branch)"
    echo "  2. the human runs: scripts/release.sh publish $ver"
    ;;
  publish)
    [[ "$ver" =~ $semver_re ]] || die "usage: scripts/release.sh publish X.Y.Z"
    [ -z "$(git status --porcelain)" ] || die "working tree is not clean"
    if has_remote; then git fetch --quiet --tags origin; fi
    sync_ff main
    sync_ff dev
    ! git rev-parse -q --verify "refs/tags/v$ver" >/dev/null || die "tag v$ver already exists"
    git show dev:CHANGELOG.md | grep -q "^## \[$ver\]" ||
      die "dev's CHANGELOG.md has no entry for $ver. Run 'scripts/release.sh prepare $ver' and land the release branch into dev first."
    git merge-base --is-ancestor main dev ||
      die "main has commits that dev lacks (a hotfix?). Back-merge main into dev on a chore/ branch first (docs/process/runbook.md)."
    git checkout -q dev
    scripts/check.sh --release "$ver"
    confirm_human
    merge_into main dev "Merge dev into main (release v$ver)" ||
      die "CI or publishing failed; main was reset to its previous commit"
    tag_main
    git checkout -q dev
    ;;
  tag)
    [[ "$ver" =~ $semver_re ]] || die "usage: scripts/release.sh tag X.Y.Z"
    [ -z "$(git status --porcelain)" ] || die "working tree is not clean"
    sync_ff main
    ! git rev-parse -q --verify "refs/tags/v$ver" >/dev/null || die "tag v$ver already exists"
    git show main:CHANGELOG.md | grep -q "^## \[$ver\]" || die "main's CHANGELOG.md has no entry for $ver"
    confirm_human
    tag_main
    git checkout -q dev
    ;;
  *) die "usage: scripts/release.sh prepare [X.Y.Z] | publish X.Y.Z | tag X.Y.Z" ;;
esac
