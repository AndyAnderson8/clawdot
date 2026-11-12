# Development environment

The scripts need only bash and git. Rust builds run natively on Linux and macOS. On Windows, builds run inside WSL2 (Ubuntu) through `scripts/cargo.sh`, while you edit and use git on the Windows side. CI runs on Linux.

## Linux and macOS

Install Rust with rustup (it reads the version from `rust-toolchain.toml`), then:

```
scripts/setup.sh          # once: git hooks and identity check
scripts/cargo.sh build --release -p clawdot-core
scripts/check.sh          # every gate
```

`scripts/cargo.sh` runs the `cargo` it finds. If `sccache` is installed it is used as the compiler cache, kept outside the repository (set `CLAWDOT_NO_SCCACHE=1` to turn that off).

## Windows with WSL2 (no sudo)

1. Install WSL2 with an Ubuntu distribution, and Git for Windows (for Git Bash).
2. In a WSL shell, in the repository, run `bash scripts/setup-wsl.sh`. It installs Rust with rustup in your home directory and Zig as the C compiler and linker, so no `build-essential` package and no `sudo` are needed. It writes `~/.clawdot-env.sh`.
3. From Git Bash, run `scripts/setup.sh` once.
4. From then on, `scripts/cargo.sh <args>` runs native cargo if it exists and otherwise runs it inside WSL. `scripts/check.sh` goes through it, so every gate works from Windows. Use `scripts/in-wsl.sh <command>` to run any other command in WSL at the same repository path. A distribution other than Ubuntu: set `CLAWDOT_WSL_DISTRO`.
5. Install `shellcheck` (`winget install koalaman.shellcheck`) so local shell checks match CI.

Build output lives in WSL, in `~/.cache/clawdot-target`, not in the repository. A linked git worktree gets its own directory, `~/.cache/clawdot-target-<worktree folder>`, because worktrees that share one target directory overwrite each other's builds. Delete a worktree's cache when you remove the worktree:

```
wsl -e bash -lc 'rm -rf ~/.cache/clawdot-target-<worktree folder>'
```

The repository can live on the Windows file system; only the build output needs the fast WSL disk. Process launches are slow on Windows (about 150 ms each), so scripts prefer bash builtins and one `grep -r` over many `grep` calls. The policy tests (`scripts/test-policy.sh`) spawn thousands of processes and re-run themselves inside WSL on Windows.

## The portal

Needs Node 24 or newer. `scripts/check.sh` installs the dependencies when `portal/node_modules` is missing; to do it by hand, see [portal/README.md](../../portal/README.md), which lists every npm script (`npm run check`, `npm test`, `npm run test:browser`, `npm run build`). The browser test needs Chrome, Chromium or Edge and skips itself without one. The built page `crates/core/src/ui.html` is committed; rebuild it with `npm run build` after a source change.

## Landing gate mode

`scripts/gate-mode` holds one word read by `scripts/land.sh`: `full` (the full gate and a CI run on the exact merge before `dev` moves) or `stabilization` (the fast local check only, with a visible "full gate skipped by mode" line). It is a temporary, reviewed setting ([ADR-0018](../adr/0018-landing-gate-mode.md)). Check the file to see which applies. Hotfixes into `main` and releases always run the full gate and CI.

## Cross-compiling and the size budget

The Echo Dot build is armv7 musl. CI builds it with `cross` (see `Cross.toml` and `.github/workflows/ci.yml`) and fails if the stripped binary is over 8 MiB. `scripts/budget.sh` measures the budget locally in WSL with Zig. The Windows programs of the PC voice loop are built from WSL by `scripts/dev/build-windows.sh` ([pc-voice-loop](../guides/pc-voice-loop.md)).

## Running the core and a satellite across a boundary

Start the core in WSL listening on a TCP port, and the reference satellite on Windows connecting to `localhost:<port>` (WSL2 forwards localhost). This exercises the real satellite protocol. Audio devices should be used from Windows; WSL2 audio is unreliable.

## Git on Windows

`core.autocrlf` is false; `.gitattributes` and `.editorconfig` keep line endings LF. `scripts/setup.sh` sets `pull.ff only` and `merge.ff false`. Never use `--no-verify`.

## Secrets in development

API keys come from environment variables (for example `OPENROUTER_API_KEY`) or are pasted into the portal, which stores them in the data directory. Do not put keys in the repository. CI and the default tests never call real providers ([testing strategy](../quality/testing-strategy.md)).
