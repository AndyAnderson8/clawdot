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

# Start / finish a release
release-start version="":
    scripts/release.sh start {{version}}

release-finish version:
    scripts/release.sh finish {{version}}
