# Updates

ClawDot looks for a newer version about once a day, tells you, and installs it only when you confirm. Updates are signed, so a speaker will not install a program the project did not make. The decisions are in [ADR-0019](../adr/0019-updates-and-remote-access.md) and the rules are REQ-UPD-001 to REQ-UPD-011 in [spec 0001](../specs/0001-product-requirements.md).

## What you see

- **Home** shows a notice when a newer version exists.
- **Settings > About > Updates** shows the version you run, when ClawDot last looked, and what is new in the newer version. **Check now** looks again. **Install this version** asks you to confirm, downloads the program, checks it, and replaces the old one.
- **Features > Check for updates** turns the daily check off. With it off, ClawDot never contacts the release list unless you press Check now.

## What is sent

One ordinary web request to the release list (by default the project's releases on GitHub), with the user agent `clawdot/<version>`. GitHub sees the address the request comes from, as with any web request. Nothing about your speaker, your settings, your name or what you said is sent.

## How an install is checked

A release has three files for each device: the program (`clawdot-<os>-<arch>`), `SHA256SUMS`, and `SHA256SUMS.sig`. ClawDot:

1. downloads the hash list and its signature, and checks the signature against the project's public key;
2. downloads the program and checks that its SHA-256 matches the signed hash;
3. writes it next to the running program and runs it with `--version`, and requires it to report the new version;
4. keeps the old program as `<name>.prev`, moves the new one into place and restarts if something supervises it.

A failure at any step leaves the running program exactly as it was.

A build with no public key never installs. It still tells you a new version exists and links to the release page.

## Setting up signing (the maintainer)

Make a key pair once and keep the private key off the speaker and out of the repository:

```
openssl genpkey -algorithm ed25519 -out clawdot-update.key
openssl pkey -in clawdot-update.key -pubout -outform DER | tail -c 32 | xxd -p -c 64
```

The second command prints the public key as 64 hexadecimal digits. Put it in the build as the environment variable `CLAWDOT_UPDATE_PUBKEY`, or in a device's configuration as `updates.public_key`.

To publish a release, build the program for each device, name it `clawdot-<os>-<arch>` (`linux` or `windows` for the system; `armv7`, `aarch64` or `x86_64` for the processor, with `.exe` on Windows), and sign the hash list:

```
sha256sum clawdot-* > SHA256SUMS
openssl pkeyutl -sign -rawin -inkey clawdot-update.key -in SHA256SUMS | xxd -p -c 512 > SHA256SUMS.sig
```

Attach the program files, `SHA256SUMS` and `SHA256SUMS.sig` to the release. Publishing is a human step ([release process](../process/runbook.md)).

## Restarting

ClawDot exits after installing so that whatever started it starts the new version. It does that only if it knows it is supervised: the environment variable `CLAWDOT_SUPERVISED` is set, or a service manager started it. Otherwise it keeps running the old version and says a restart is needed. A systemd service with `Restart=always` is supervised. On the Echo Dot the supervisor script is `/data/clawdot/run.sh`.

## Putting the old version back

The old program is `<name>.prev` next to the program. Stop ClawDot, move `<name>.prev` over `<name>`, and start it. After an update ClawDot leaves a file `update-pending` in its data directory for two minutes of running; a supervisor can use a marker that is still there, together with a crash soon after start, to do this on its own.
