# Pushing builds to an Echo Dot over SSH

The Echo Dot running the TECHO5 Linux image can be reached over SSH, so a new ClawDot build is copied over the network instead of by the USB serial cable. This guide is for the person building ClawDot; the signed updater in the portal is how everyone else updates (see [updates](updates.md) and [ADR-0019](../adr/0019-updates-and-remote-access.md)).

SSH is **off by default, key-only, and owned by the image's own voice service**: it starts Dropbear only while its SSH setting is on, and a password never works. ClawDot does not start or stop it.

## One-time setup (needs the USB serial console once)

1. Make a key on your computer, one for this device only:

   ```
   ssh-keygen -t ed25519 -N "" -C clawdot-dot -f ~/.ssh/clawdot_dot
   ```

2. On the Dot's serial console (115200 baud), put the public key where the image looks for it and turn the SSH setting on. The directory behind `/root/.ssh` is private to root, which is what Dropbear insists on (it refuses a key file under `/data` itself, which anyone can write to):

   ```
   echo 'ssh-ed25519 AAAA... clawdot-dot' > /root/.ssh/authorized_keys
   chmod 600 /root/.ssh/authorized_keys
   cp /data/misc/echolocal/state.json /data/misc/echolocal/state.json.bak
   sed -i 's/"ssh": false/"ssh": true/' /data/misc/echolocal/state.json
   reboot
   ```

   The voice service reads that setting when it starts, so the Dot restarts once. It is the same setting the image's Home Assistant integration switches; turning it off there (or setting it back to `false`) stops SSH.

3. Check it from your computer:

   ```
   ssh -i ~/.ssh/clawdot_dot root@HOST 'uname -m'
   ```

## Pushing a build

CI builds this same artifact on every push (job `dot build`, with `cross`; the binary is kept as the workflow artifact `clawdot-armv7-dot`, and `clawdot-armv7-full` adds `hive,realtime`) and fails above the 8 MiB budget. `clawdot --version` prints the version and the git commit, for example `clawdot 0.0.0 (a1b2c3d4e-dirty)`; the same text is in `GET /api/about` (`build.id`).

Build for the Dot (armv7 musl, features `esphome-satellite,esphome-noise`), then:

```
scripts/dev/dot-push.sh HOST path/to/clawdot
```

The script streams the file through `ssh` (the image has no `scp` or `sftp`), checks its SHA-256 on the device, runs `--version` there, keeps the old binary as `clawdot.prev`, stops the running copy, and waits for the portal to answer. If the new build crashes three times at start, the supervisor ([`dot-run.sh`](../../scripts/dev/dot-run.sh)) puts `clawdot.prev` back.

## Telling ClawDot what the device is

ClawDot's core knows no device by name. It looks at what the operating system shows (model string, processor, memory, the lights, radios, screen and USB serial adapters the kernel lists) and takes the rest from the `device` section of the configuration file. The Dot's light ring is driven by another service and the kernel lists no LED for it, so the Dot's `clawdot.json` (next to `run.sh`) carries the preset in [`examples/devices/echo-dot2.json`](../../examples/devices/echo-dot2.json):

```json
{ "device": { "board": "echo-dot2", "label": "Echo Dot (2nd generation)", "parts": { "leds": true } } }
```

Add that `device` object to the existing `clawdot.json` and restart ClawDot. `board` is the `board` value of the file in `boards/` that the device daemon uses; `label` is what the portal shows. `run.sh` writes a line to `clawdot.log` when the section is missing.

## What this does not do

- **No password login, ever.** Dropbear runs with `-s`.
- **It is root.** The image has no other login that can write ClawDot's directory. The key stays on your computer; anyone who has it can change the device. Keep it private and use one per device.
- **It does not replace the updater.** SSH is for the maintainer.
