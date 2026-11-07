#!/usr/bin/env bash
# Cut a release with git flow.
#   scripts/release.sh start [X.Y.Z]   branch release/X.Y.Z from dev, update CHANGELOG.md, commit
#   scripts/release.sh finish X.Y.Z    land the release branch into main, tag it, back-merge into dev
#
# `start` needs git-cliff (https://git-cliff.org). If X.Y.Z is omitted, git-cliff computes the next
# SemVer version from the Conventional Commits since the last tag.
# NOTE: first exercised for real at v0.1.0; treat the first run as a dry run on a throwaway clone.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
# shellcheck source=scripts/lib-git.sh
. scripts/lib-git.sh

cmd="${1:-}"
ver="${2:-}"

case "$cmd" in
  start)
    [ "$(current_branch)" = "dev" ] || die "start a release from dev"
    [ -z "$(git status --porcelain)" ] || die "working tree is not clean"
    command -v git-cliff >/dev/null 2>&1 || die "git-cliff is required to generate the changelog"
    if has_remote; then git fetch --quiet origin; git merge --ff-only -q origin/dev; fi
    [ -n "$ver" ] || ver=$(git cliff --bumped-version | sed 's/^v//')
    [[ "$ver" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$ ]] || die "'$ver' is not a SemVer version"
    git switch -c "release/$ver"
    git cliff --tag "v$ver" --output CHANGELOG.md
    if [ -f Cargo.toml ] && command -v cargo-set-version >/dev/null 2>&1; then cargo set-version --workspace "$ver"; fi
    git add -A
    git commit -m "chore(release): v$ver" -m "Update the changelog and version for v$ver."
    echo "release/$ver created. Review CHANGELOG.md, then:  scripts/release.sh finish $ver"
    ;;
  finish)
    [ -n "$ver" ] || die "usage: scripts/release.sh finish X.Y.Z"
    git rev-parse --verify -q "release/$ver" >/dev/null || die "no branch release/$ver"
    scripts/land.sh "release/$ver" --into main
    git checkout -q main
    sign=()
    if [ "$(git config --get commit.gpgsign || true)" = "true" ]; then sign=(-s); else sign=(-a); fi
    git tag "${sign[@]}" "v$ver" -m "ClawDot v$ver"
    if has_remote; then CLAWDOT_LANDING=1 git push origin "v$ver"; fi
    git checkout -q dev
    prev=$(git rev-parse dev)
    CLAWDOT_LANDING=1 git merge --no-ff -m "Merge main into dev (release v$ver)" main
    if ! publish dev; then git reset -q --hard "$prev"; die "back-merge failed CI; fix and retry"; fi
    git branch -d "release/$ver"
    echo "Released v$ver."
    ;;
  *) die "usage: scripts/release.sh start [X.Y.Z] | finish X.Y.Z" ;;
esac
