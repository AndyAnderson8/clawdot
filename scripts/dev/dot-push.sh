#!/usr/bin/env bash
# Put a new ClawDot build on an Echo Dot over SSH, with no USB cable.
#
#   scripts/dev/dot-push.sh HOST PATH-TO-CLAWDOT-BINARY [PRIVATE-KEY]
#
# HOST is the Dot's address (or name). The key defaults to ~/.ssh/clawdot_dot. The Dot has no scp or sftp, so the file
# is streamed through ssh. The old binary is kept as clawdot.prev (the supervisor puts it back if the new one keeps
# crashing), the new one is checked by its hash and `--version` before it replaces anything, and the script waits for
# the portal to answer. One-time setup of the key: docs/guides/dot-ssh.md.
set -euo pipefail

host="${1:?usage: dot-push.sh HOST PATH-TO-CLAWDOT-BINARY [PRIVATE-KEY]}"
bin="${2:?usage: dot-push.sh HOST PATH-TO-CLAWDOT-BINARY [PRIVATE-KEY]}"
key="${3:-$HOME/.ssh/clawdot_dot}"
dir=/data/clawdot

[ -f "$bin" ] || { echo "dot-push: no such file: $bin" >&2; exit 1; }
[ -f "$key" ] || { echo "dot-push: no such key: $key (see docs/guides/dot-ssh.md)" >&2; exit 1; }

ssh_dot() {
  ssh -i "$key" -o IdentitiesOnly=yes -o PasswordAuthentication=no -o ConnectTimeout=10 "root@$host" "$@"
}

want="$(sha256sum "$bin" | cut -d' ' -f1)"
echo "dot-push: sending $(wc -c < "$bin") bytes (sha256 ${want:0:12})"
ssh_dot "cat > $dir/clawdot.new && chmod +x $dir/clawdot.new" < "$bin"

got="$(ssh_dot "sha256sum $dir/clawdot.new" | cut -d' ' -f1)"
[ "$got" = "$want" ] || { echo "dot-push: the copy on the device does not match (got ${got:0:12}); nothing was replaced" >&2; exit 1; }

ver="$(ssh_dot "$dir/clawdot.new --version")" || { echo "dot-push: the new binary does not run here; nothing was replaced" >&2; exit 1; }
echo "dot-push: the device runs it: $ver"

# Keep the old one, swap, and stop the running copy by its own name (never by a pattern that could match this shell):
# the supervisor starts the new one within a few seconds.
ssh_dot "cp $dir/clawdot $dir/clawdot.prev && mv $dir/clawdot.new $dir/clawdot && kill \$(pidof clawdot)"

echo -n "dot-push: waiting for the portal"
for _ in $(seq 1 30); do
  sleep 2
  echo -n "."
  if curl -fsS -m 3 -o /dev/null "http://$host:3335/" 2>/dev/null; then
    echo " up."
    echo "dot-push: done. The old build is $dir/clawdot.prev on the device."
    exit 0
  fi
done
echo
echo "dot-push: the portal did not come back within a minute. The supervisor puts the old build back after three quick failures; check $dir/clawdot.log." >&2
exit 1
