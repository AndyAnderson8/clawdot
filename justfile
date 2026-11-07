# Thin convenience wrappers. The scripts in scripts/ are the source of truth and need only bash and git.
# Install `just` from https://github.com/casey/just (optional).

default:
    @just --list

# Enable git hooks and verify identity (run once after cloning)
setup:
    scripts/setup.sh

# Every gate (what land.sh runs)
check:
    scripts/check.sh

# Pre-commit subset
check-fast:
    scripts/check.sh --fast

# Land a working branch into dev: just land feature/timer-tool
land branch:
    scripts/land.sh {{branch}}

# Release step 1 (anyone): branch release/X.Y.Z with a regenerated changelog
release-prepare version="":
    scripts/release.sh prepare {{version}}

# Release step 3 (HUMAN ONLY, interactive terminal): merge dev into main, tag, push the tag
release-publish version:
    scripts/release.sh publish {{version}}
