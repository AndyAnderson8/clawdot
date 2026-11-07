#!/usr/bin/env bash
# One-time setup for building ClawDot inside WSL (Ubuntu) WITHOUT sudo.
# rustup installs Rust in your home directory; Zig supplies the C compiler and linker, so no
# build-essential package is needed. Run it from WSL:  bash scripts/setup-wsl.sh
# Afterwards scripts/cargo.sh (and therefore scripts/check.sh) can run cargo from a Windows shell.
set -euo pipefail

zig_version="0.17.1"
zig_url="https://ziglang.org/download/${zig_version}/zig-x86_64-linux-${zig_version}.tar.xz"

if [ ! -f "$HOME/.cargo/env" ]; then
  curl -sSf https://sh.rustup.rs -o /tmp/rustup.sh
  sh /tmp/rustup.sh -y --profile minimal --default-toolchain stable
fi

curl -sSf -L "$zig_url" -o /tmp/zig.tar.xz
rm -rf "$HOME/zig"
mkdir -p "$HOME/zig" "$HOME/bin"
tar -xJf /tmp/zig.tar.xz -C "$HOME/zig" --strip-components=1
echo "zig $("$HOME/zig/zig" version)"

# cc, c++, ar, and ranlib wrappers so cargo and build scripts find a C toolchain.
# shellcheck disable=SC2016  # $HOME and "$@" must expand when the wrapper runs, not now
for tool in cc c++ ar ranlib; do
  printf '#!/usr/bin/env bash\nexec "$HOME/zig/zig" %s "$@"\n' "$tool" >"$HOME/bin/$tool"
  chmod +x "$HOME/bin/$tool"
done

cat >"$HOME/.clawdot-env.sh" <<'ENV'
# ClawDot build environment (WSL). Sourced by scripts/cargo.sh.
. "$HOME/.cargo/env"
export PATH="$HOME/bin:$HOME/zig:$PATH"
export CC="$HOME/bin/cc" CXX="$HOME/bin/c++" AR="$HOME/bin/ar"
export CARGO_TARGET_DIR="$HOME/.cache/clawdot-target"
ENV

# Smoke test: a proc-macro crate must compile and link.
# shellcheck disable=SC1091
. "$HOME/.clawdot-env.sh"
work=$(mktemp -d)
cd "$work"
cargo new --quiet smoke
cd smoke
printf 'serde = { version = "1", features = ["derive"] }\nserde_json = "1"\n' >>Cargo.toml
cat >src/main.rs <<'RS'
use serde::Serialize;
#[derive(Serialize)]
struct P { name: &'static str }
fn main() { println!("{}", serde_json::to_string(&P { name: "linked" }).unwrap()); }
RS
cargo run --quiet
rm -rf "$work"
echo "WSL build environment ready."
