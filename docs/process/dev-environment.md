# Development Environment

Status: Draft. Commands are the intended setup and will be validated when the workspace is created (TASK-001). The host is Windows 11; CI runs on Linux.

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

## Known risks to validate in TASK-001
- Windows to WSL2 localhost forwarding behaves with raw TCP and long-lived connections.
- Zig-based linking works with the chosen C dependencies (libopus, TFLite Micro).
- qemu-user RSS measurements are representative (they track guest memory but not all kernel effects). The real-hardware run is the final check.
