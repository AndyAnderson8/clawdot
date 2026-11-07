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

# cc and c++ rewrite arguments zig rejects: the "unknown" vendor in --target and -march=x86-64 (from the cc crate).
# shellcheck disable=SC2016  # $HOME and "$@" must expand when the wrapper runs, not now
for tool in cc c++; do
  cat >"$HOME/bin/$tool" <<WRAP
#!/usr/bin/env bash
args=()
for a in "\$@"; do
  case "\$a" in
    --target=*) a="\${a/-unknown-/-}" ;;
    -march=x86-64) a=-march=x86_64 ;;
  esac
  args+=("\$a")
done
exec "\$HOME/zig/zig" $tool "\${args[@]}"
WRAP
  chmod +x "$HOME/bin/$tool"
done
# shellcheck disable=SC2016  # as above
for tool in ar ranlib; do
  printf '#!/usr/bin/env bash\nexec "$HOME/zig/zig" %s "$@"\n' "$tool" >"$HOME/bin/$tool"
  chmod +x "$HOME/bin/$tool"
done

# Target-specific wrappers for static musl builds (scripts/budget.sh). Rust's own bundled C startup files
# are disabled (link-self-contained=no) and zig supplies musl, so the two do not collide.
# shellcheck disable=SC2016  # as above
for pair in musl64:x86_64-linux-musl armv7:arm-linux-musleabihf; do
  name=${pair%%:*}; target=${pair#*:}
  cat >"$HOME/bin/zcc-$name" <<WRAP
#!/usr/bin/env bash
args=()
for a in "\$@"; do
  case "\$a" in
    --target=*|-march=x86-64) ;;
    *) args+=("\$a") ;;
  esac
done
exec "\$HOME/zig/zig" cc -target $target "\${args[@]}"
WRAP
  chmod +x "$HOME/bin/zcc-$name"
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
