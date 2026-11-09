#!/usr/bin/env bash
# Cross-build the Windows programs of the PC voice loop from WSL2, so Windows needs no Rust or Visual Studio.
#   scripts/in-wsl.sh scripts/dev/build-windows.sh [OUT_DIR]       (default OUT_DIR: target/windows)
# Produces clawdot-device.exe (with the wake engine) and clawdot-satellite.exe. Needs the WSL setup of
# scripts/setup-wsl.sh (it installs Zig, which links the Windows programs; no Windows SDK is involved).
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
out="${1:-target/windows}"
target=x86_64-pc-windows-gnu

[ -x "$HOME/zig/zig" ] || { echo "build-windows: Zig is missing; run scripts/setup-wsl.sh first" >&2; exit 1; }
rustup target add "$target" >/dev/null

# Rust asks for a MinGW gcc; Zig stands in. Rust's own runtime libraries (msvcrt, gcc, mingw) are dropped from the
# link line because Zig supplies them, and Zig's libunwind supplies the unwinder that std references.
wrapper="$HOME/bin/zcc-win"
mkdir -p "$HOME/bin"
cat >"$wrapper" <<'WRAP'
#!/usr/bin/env bash
args=()
for a in "$@"; do
  case "$a" in
    -fuse-ld=*|-Wl,--as-needed|-Wl,-Bdynamic|-Wl,--no-insert-timestamp|-Wl,--high-entropy-va) ;;
    -l:libpthread.a|-lmsvcrt|-lgcc|-lgcc_eh|-lmingwex|-lmingw32|-nodefaultlibs) ;;
    *) args+=("$a") ;;
  esac
done
exec "$HOME/zig/zig" cc -target x86_64-windows-gnu "${args[@]}" -lunwind
WRAP
chmod +x "$wrapper"

export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER="$wrapper"
cargo build --release --target "$target" -p clawdot-device --features mww -p clawdot-satellite
mkdir -p "$out"
for exe in clawdot-device clawdot-satellite; do
  cp "${CARGO_TARGET_DIR:-target}/$target/release/$exe.exe" "$out/"
done
ls -l "$out"
