# Development Environment

Status: Draft. The host is Windows 11; CI runs on Linux. The WSL path below was validated on 2026-11-07; the audio and cross-run (qemu) parts are still intended, not yet exercised.

## Quick start (Windows + WSL2, no sudo)
1. Install WSL2 with Ubuntu. From a WSL shell run `bash scripts/setup-wsl.sh`. It installs Rust with rustup in your home directory and uses Zig as the C compiler and linker, so no `build-essential` or `sudo` is needed.
2. From any shell on the repository (Git Bash on Windows included), `scripts/cargo.sh <args>` runs cargo natively if it exists, and inside WSL otherwise. Build output lives in `~/.cache/clawdot-target` in WSL (fast), not in the repository.
3. `scripts/check.sh` uses that wrapper, so every gate works from Windows. Install `shellcheck` too (`winget install koalaman.shellcheck`) so local checks match CI.

## Principle
Do audio on the host, cross-compilation and budget checks in Linux. The project's own protocol makes this split natural: the core can run in Linux while the desktop satellite runs natively on Windows.

## Layout

| Task | Where | Why |
|---|---|---|
| Editing, git, desktop satellite (mic and speaker) | **Windows host** | WASAPI audio works natively; WSL2 audio is unreliable |
| Core build and tests, cross-compile to armv7, qemu budget gate, fuzzing, `cargo deny` | **WSL2 (Ubuntu)** | Matches CI; qemu-user and musl cross toolchains are straightforward |
| Real-hardware checks (Pi or similar) | **SSH to the device** | The only trustworthy CPU and RAM numbers |

Keep the Rust workspace on the WSL2 ext4 filesystem for build performance (not under `/mnt/c`). Use a Windows-side clone only for editing if preferred, or edit via your IDE's WSL integration.

## Toolchain
- `rustup` with the version pinned in `rust-toolchain.toml`, plus the target `armv7-unknown-linux-musleabihf`.
- Cross-compilation: `cargo-zigbuild` (Zig as the linker) or `cross` (needs Docker). Choose one in TASK-001 and record it in an ADR if it affects contributors.
- `qemu-user-static` and binfmt for running armv7 binaries on x86_64 (budget gate).
- `just` (task runner), `cargo-deny`, `cargo-audit`, `cargo-bloat`, `cargo-fuzz` (nightly), `cargo-nextest` (optional).
- A devcontainer definition reproduces the Linux side for contributors who don't use WSL2.

## Running core and satellite across the boundary
1. Start the core in WSL2 listening on a TCP port.
2. Start the desktop satellite on Windows, connecting to `localhost:<port>` (WSL2 forwards localhost to the VM).
3. This exercises the real satellite protocol, not the in-process shortcut.

## Git on Windows
- `core.autocrlf=false`; line endings are normalised by `.gitattributes` and `.editorconfig` (LF).
- Commit signing with SSH keys via the Windows OpenSSH agent: `git config gpg.format ssh` and `commit.gpgsign true`.
- `git commit -s` for the DCO sign-off once external contributors exist (see [SDLC §13](sdlc.md#13-proportionality-solo-maintainer-mode)).

## Secrets in development
- API keys live in a git-ignored `config.toml` or environment variables.
- **CI and default tests never use real providers** (see [testing strategy §7](../quality/testing-strategy.md#7-test-cost-policy)).

## Performance: process spawns are expensive on Windows
On a typical Windows machine every process launch costs about 150 ms (measured 2026-11-07), so a script's speed is set by **how many processes it starts**, not by what it computes. Rules for scripts in this repository:
- Prefer bash builtins (`[[ =~ ]]`, parameter expansion, `read`) over `grep`, `sed`, `tr`, `cat`, `dirname`, and `$(...)` subshells, especially in loops.
- Batch work: one `grep -r` over the tree beats one `grep` per file.
- The agent guard (`.claude/hooks/guard-git.sh`) runs before every shell command an agent issues. It is spawn-free on purpose (0.4 s instead of 2.7 s); keep it that way.
- Gate steps that spawn hundreds of processes (the policy tests) run only when the policy tooling changes; CI runs them always.

## Known risks to validate in TASK-001
- Windows to WSL2 localhost forwarding behaves with raw TCP and long-lived connections.
- Zig-based linking works with the chosen C dependencies (libopus, TFLite Micro).
- qemu-user RSS measurements are representative (they track guest memory but not all kernel effects). The real-hardware run is the final check.
