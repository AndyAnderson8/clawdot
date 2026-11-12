#!/bin/sh
# The supervisor that runs on an Echo Dot running the TECHO5 Linux image. The image's init starts it (an inittab
# respawn entry); it keeps ClawDot running, opens the portal port to the home network, and puts the previous binary
# back if a new one keeps crashing at start. (SSH is not started here: the image's own voice service owns it, see
# docs/guides/dot-ssh.md.)
# Copy it to /data/clawdot/run.sh on the Dot.
cd /data/clawdot || exit 1
# shellcheck source=/dev/null
. ./env
export CLAWDOT_SUPERVISED=1

# ClawDot's core knows no device by name. What this one is called, and the light ring the kernel does not list, come
# from the "device" section of clawdot.json (examples/devices/echo-dot2.json, docs/guides/dot-ssh.md). Say so in the
# log when it is missing; nothing else changes.
note_device() {
  grep -q '"device"' clawdot.json 2>/dev/null ||
    echo "run.sh: clawdot.json has no \"device\" section (board, label, parts.leds); see examples/devices/echo-dot2.json" >> clawdot.log
}

ensure_rule() {
  iptables-legacy -C INPUT -p tcp --dport 3335 -s 192.168.0.0/24 -j ACCEPT 2>/dev/null ||
    iptables-legacy -I INPUT -p tcp --dport 3335 -s 192.168.0.0/24 -j ACCEPT
}

fails=0
while true; do
  : > clawdot.log
  note_device
  start=$(date +%s)
  ./clawdot --config clawdot.json --no-console --echo-speech >> clawdot.log 2>&1 &
  pid=$!
  while kill -0 "$pid" 2>/dev/null; do
    ensure_rule
    sleep 15
  done
  ran=$(( $(date +%s) - start ))
  if [ "$ran" -ge 0 ] && [ "$ran" -lt 30 ]; then fails=$((fails + 1)); else fails=0; fi
  if [ "$fails" -ge 3 ] && [ -f clawdot.prev ]; then
    echo "run.sh: three quick failures, putting the previous version back" >> clawdot.log
    mv clawdot clawdot.bad
    mv clawdot.prev clawdot
    fails=0
  fi
  sleep 3
done
