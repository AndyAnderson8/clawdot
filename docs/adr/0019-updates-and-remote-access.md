# ADR-0019: Updates, and what a smart speaker should and should not share

- **Status:** Proposed
- **Date:** 2026-11-10
- **Deciders:** Maintainer

## Context

The maintainer wants a speaker that checks for updates on its own, lets the person check again by hand, and installs only after they confirm. They also asked whether the speaker should offer SSH, file access over USB, or an SMB file share, and left the answer to us.

A speaker is a small, always-on computer that holds secrets: the name of the variable that holds the language-model key, the admin password hash, a log of recent problems, and the person's memory. Every service it exposes is surface that someone on the home network, or a guest, can reach. The device also has no keyboard or screen, so it must be maintainable without a shell on the box. The Echo Dot 2 image in use has no SSH and no adb; the only root access is a USB serial console.

## Decision

### 1. Updates: check on a schedule, install on confirmation, only if signed

- ClawDot looks for a newer release once a day, and when the person presses **Check now**. The check is one `GET` of the project's public release list. It sends nothing about the person or the device beyond what any web request carries, and it can be turned off (a switch under Features).
- A newer version is **announced** on Home and in Settings > About. It is **installed only** when the person confirms, and only if the release is signed.
- A release carries the program for the device, `SHA256SUMS` and `SHA256SUMS.sig`, an Ed25519 signature made with a key the maintainer holds. The public half is compiled into official builds or set in the configuration. A build with no key never installs by itself; it only says where to download.
- The old program is kept as `<name>.prev`. A new version that runs two minutes is considered proven. A supervisor can put the old one back if the new one dies at once.
- Nothing is installed silently. Silent self-update was rejected: a compromised release channel would then reach every speaker without anyone looking.

Details: [updates guide](../guides/updates.md), requirements REQ-UPD-001 to REQ-UPD-011.

### 2. SSH: opt-in, key-only, off by default

SSH is the right tool for a maintainer and for a device with no screen, and it is a bad default for a household appliance. So:

- It is **off**. Turning it on is a deliberate act: signed in to the portal, a switch, and a public key pasted in. A password is never accepted.
- The user is `clawdot`, not `root`, where the operating system allows it. Where the image only has `root`, root is allowed with a key only.
- It listens only on the local network, never on a public address, and has no port forward.
- Turning it off removes the keys.

This is a decision about the policy. Whether the Dot image can run an SSH server at all is a device question that is answered on the device (B0), so the switch appears only where a daemon exists.

### 3. USB file access and SMB: no

- **USB mass storage** would expose the data directory, which holds keys and the admin hash, to whatever is plugged in, and exporting a filesystem that the speaker is also writing to corrupts it. The Dot's USB port is also its power port.
- **SMB** is a large, historically weak service for a small device, costs memory the budget does not have, and shares the same secrets. There is no use of it that the alternatives do not cover.

What replaces them, all already built: **Save my settings / Load a settings file** (no secrets in the file), **Download diagnostics**, **Download skills file**, the signed **update** above, and **SSH with scp** for the maintainer when switched on.

## Consequences

- A speaker updates itself without anyone plugging it in, but only with the person's consent and a key the project holds. Until the maintainer creates the key and signs a release, updates are announced and the Update button explains why it cannot install.
- The maintainer must keep the signing key safe. If it is lost, updates stop being accepted; if it is stolen, an attacker who can also publish a release can push a program. Rotation is a rebuild with a new public key, installed by hand.
- No SMB or USB storage code will be written, which closes two questions for good. SSH is revisited at B0 on the real device.

## Alternatives rejected

- **Silent automatic updates:** fastest for the person, but a single compromised release reaches every device without review.
- **Update by link only:** the safest and the least useful; a speaker in a cupboard would never be updated.
- **SSH on by default with a password:** convenient, and the classic way appliances get taken over.
