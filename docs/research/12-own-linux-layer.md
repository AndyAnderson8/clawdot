# R-12: ClawDot's own Linux layer for the Echo Dot 2 (roadmap B4b)

Date: 2026-11-11 · Feeds: roadmap B4 and B4b, B5, ADR-0015 (follow-up on the image recipe), spec 0006 (REQ-DEV-032..037) · Refines: [R-07](07-echo-dot-gen2-hardware.md), [R-08](08-existing-echo-dot-projects.md), [R-09](09-echo-unlock-history.md), [R-10](10-libreecho-evaluation.md)

Method: read-only study on 2026-11-11 through the GitHub API and web pages. Nothing was cloned, built, installed or run; no device was touched and no LAN address was contacted; no code was copied. The main source is the **TECHO5 Dot** repository (`HuskerMinion/techo5-dot`, "T5D") at the release tag **`dot-v1.0.61`** (tag object `13482c94cd75`, published 2026-11-09; the default branch was at `6ef8102` on 2026-11-10), read in full for `NOTICE`, `README.md`, `docs/hardware.md`, `docs/porting-plan.md`, `docs/building.md`, `tools/linux/build-kernel.sh`, `tools/linux/rootfs/etc/techo5/boot.sh`, `tools/linux/rootfs/etc/inittab`, `tools/linux/rootfs/usr/local/sbin/slotctl` (header), `tools/linux/packages-rootfs.txt`, `tools/linux/mkimage.py` (header) and the heads of `tools/install-dot.py` and `tools/dotimage.py`, plus the release notes of `dot-v1.0.61`. Other primary sources: `aslater3/LibreEcho-Linux` and `aslater3/LibreEcho-mt8163-connectivity` (README files), kernel.org, buildroot.org and alpinelinux.org pages. Paths below are relative to T5D unless said otherwise. The Dot projects say they were written largely with an AI assistant, so I weight code, scripts and measured numbers over narrative. Claims I could not confirm from a primary file are marked **(unverified)**.

This note adds to R-08, which already compared the projects and recommended using T5D as an unmodified base. ADR-0015 later decided that ClawDot owns the device layer. R-12 asks the next question: **what, concretely, is "the layer", where does each piece come from, and how does ClawDot stop needing someone else's image?**

## 1. Summary

1. **The T5D image is a small, well-documented recipe, not a monolith.** Seven pieces matter: Amazon's boot chain (untouched), Amazon's 3.18.19 GPL kernel rebuilt with Bluetooth and 48 security backports, a per-unit boot image in the `recovery` partition, a tiny rescue initramfs, an Alpine 3.24 armv7 root filesystem (49 MB unpacked) in two slot directories on the `cache` partition, a Go daemon (`echod`, MIT), and a Python installer plus a signed-release updater.
2. **Almost everything is redistributable; four classes of file are not.** Redistributable: T5D's scripts and Go code (MIT, copyright notice kept), the kernel build recipe and the kernel binary (GPL-2.0, with source), Alpine packages (each its own licence). **Never ship**: Amazon's boot partitions, Amazon's Wi-Fi/Bluetooth firmware and audio tuning files (read from the user's own unit at install time), the unlock tooling (link only), and a boot image (it contains the unit's own header; T5D never publishes one either).
3. **The hard dependency is not code, it is the unit.** Wi-Fi works only because `boot.sh` copies MediaTek's firmware and Amazon's audio coefficients off the unit's `system_b` partition on the first boot. A ClawDot installer must do the same, which means **Fire OS 6574.1 has to stay on the unit** (or the user's backup of those files must be kept).
4. **The weak points to fix are update safety for the kernel and boot image, and the 3.18 kernel itself.** Root filesystem updates are A/B with rollback. The boot image sits in a single partition and is updated over SSH with no second slot. The kernel line (3.18) has had no upstream fixes since 3.18.140 in May 2019; LibreEcho already runs a 6.1.178 stable-based kernel with a forward-ported MediaTek connectivity stack on this SoC.
5. **Recommendation (section 5):** build the layer in stages by **forking T5D's design, not its image**: Alpine userland, the slot store, signed releases and a new installer in a ClawDot-owned repository, our daemon in place of `echod`, T5D's kernel binary pinned by hash at first and replaced by our own reproducible build later, and a separate 6.1 kernel spike before any kernel decision. Buildroot and a pure initramfs were considered and rejected (section 4).

## 2. Inventory of the TECHO5 Dot image

Licence column: **P** = proprietary, **GPL** = GPL-2.0 (or as stated), **MIT** and so on as written. "May we ship it?" column: **Yes** (with the notice named), **Source only** (we may publish the source and the build recipe; a binary is allowed under GPL if we also offer the source), **Link** (point at it, do not host it), **Device** (read from the user's own unit at install time, never in our repository or release), **No**.

### 2.1 Boot chain and recovery (all stays as it is on the unit)

| Piece | What it is | Licence | May we ship it? | Source |
|---|---|---|---|---|
| BootROM | Fixed in the SoC | P (silicon) | n/a | R-09 section 3 |
| Preloader (`boot0`, plus `kb`, `dkb`) | First stage; checks LK; keeps boot-attempt and anti-rollback counters | P (Amazon/MediaTek) | No. Back up the unit's own | R-09 section 3; T5D `docs/hardware.md` "Partitions" |
| LK (`lk_a`, `lk_b`) and the kaeru payload in `expdb` | Bootloader with fastboot; after the unlock it runs unsigned boot images | LK is P (Amazon); kaeru is R0rt1z2's, repository licence `NOASSERTION` with `LICENSE.GPL2` and `LICENSE.MIT` side by side (per-file mapping **unverified**) | Link only | T5D `docs/hardware.md` "Unlock"; [amonet](https://github.com/R0rt1z2/amonet) |
| TEE (`tee1`, `tee2`) | TrustZone images; amonet v2 writes an old signed one | P | No. Back up the unit's own | T5D `docs/hardware.md` |
| TWRP in `recovery` | Recovery with adb; replaced by the Linux boot image at install, its copy kept in the store (`twrp.img`) for the way back | TWRP and the biscuit tree by R0rt1z2: licence **unverified** | Link only | T5D `docs/porting-plan.md` M3b |
| Fire OS (`boot_a/b`, `system_a/b`) | Left untouched; the unit still boots it on a normal power-on | P (Amazon) | No | T5D `docs/porting-plan.md` "It goes in the recovery partition" |

Partition table of the bench unit (4 GB eMMC, 3 817 472 KiB): `recovery` p12 16 MiB; `misc` p8 512 KiB; `system_a/b` p13/p14 768 MiB each; `cache` p15 784 MiB; `userdata` p16 1.2 GiB (T5D `docs/hardware.md` "First unit"). The maintainer's unit must be read, not assumed (B0 plan step B2).

### 2.2 Kernel, device tree and drivers

| Piece | What it is | Licence | May we ship it? | Source |
|---|---|---|---|---|
| Kernel 3.18.19, 32-bit ARM | Built by `tools/linux/build-kernel.sh` from Amazon's GPL release `Echo_Dot_src-6.5.7.1-20251024` (`kernel/mediatek/mt8163/3.18_hl` inside `platform.tar`), config taken from the running kernel (`docs/dumps/biscuit-fireos6-echolocal-boot-3.18.19.config`), toolchain AOSP `arm-eabi-4.8`. Release asset `techo5-dot-kernel-bt.zImage-dtb`, 5 951 508 bytes | GPL-2.0 | **Source only / Yes with source**. T5D itself publishes the binary and the full recipe. A rebuild "is not guaranteed to come out byte for byte the same" (T5D `NOTICE`) | T5D `NOTICE`; `docs/building.md` section 4 |
| Bluetooth stack | Amazon's tarball lacks `net/bluetooth`, `drivers/bluetooth`; taken from upstream 3.18.19, plus **48 backported security patches** (BlueBorne CVE-2017-1000251, BadChoice CVE-2020-12352 and others, each naming its upstream commit). KNOB and BIAS are **not** fixed | GPL-2.0 | Source only | T5D `docs/hardware.md` "Bluetooth kernel" |
| Device tree | Two DTBs appended to the zImage (`biscuit_evt.dtb`, `biscuit_min_evt.dtb`), reproduced "byte for byte" from Amazon's source | GPL-2.0 (in the kernel tree) | Source only | `build-kernel.sh` header |
| Audio: `mt-snd-card`, four TLV320ADC3101 (I2C `0-0018`..`001b`), TLV320 DAC (playback device 23, capture device 24) | In the kernel, from Amazon's tree | GPL-2.0 | Source only | `docs/hardware.md` "Board", "Audio" |
| Wi-Fi/Bluetooth: MediaTek CONSYS (WMT/STP/BTIF, gen2 `wlan`, MT6625L RF) | Vendor drivers built **into** the kernel (about 213 000 lines in the LibreEcho forward port) | Vendor GPL-2.0 sources; per-file headers not checked **(unverified)**; the LibreEcho connectivity repository has no `LICENSE` file | Source only | T5D `docs/hardware.md`; [LibreEcho-mt8163-connectivity](https://github.com/aslater3/LibreEcho-mt8163-connectivity) README |
| LED ring IS31FL3236 (sysfs `frame`, `led_current`, `boot_animation`), keypad `mtk-kpd`, GPIO keys, `amz_privacy`, ACCDET jack | In the kernel, from Amazon's tree | GPL-2.0 | Source only | `docs/hardware.md` "Board"; R-08 section 3 |
| Missing in this kernel | No devtmpfs, no overlayfs/squashfs/namespaces, no configfs gadget (Android gadget only), no nftables (legacy iptables), no pstore (RAM console only, `/proc/last_kmsg`) | n/a | n/a | `docs/hardware.md`; `techo5-firewall` note |

### 2.3 Blobs read from the device at install or first boot

| File | What it is | Licence | How T5D gets it | Source |
|---|---|---|---|---|
| `WMT_SOC.cfg`, `WIFI_RAM_CODE_8163`, `ROMv2_lm_patch*` | MediaTek combo-chip firmware, from Fire OS `/vendor/firmware` | P | `boot.sh` mounts `system_b` (p14) read-only once and copies it to the store (`/techo5/firmware`), then never mounts it again. "Amazon's, so it is never in a published rootfs" | `boot.sh` "Wi-Fi firmware" |
| `/system/vendor/etc/audio-algorithms/*` (`MBCL.cfg`, `AFE.cfg`, beamformer and speaker tuning) | Amazon's DSP tuning | P | Same, `adopt_audio` | `boot.sh` |
| `/system/etc/audio_device.xml` | Per-output codec paths; needed for the Echo 2nd gen speaker EQ | P | Same, `adopt_audio_device` | `boot.sh` |
| `/proc/idme` (seven microphone calibrations, device type id, factory MAC) | Per-unit factory data | P (and personal to the unit) | Read live; `device_type_id` picks `biscuit` or `radar` | `docs/hardware.md`; `boot.sh` |
| Home Assistant Noise key and device name | Daemon state in `/data/misc/echolocal` | Personal | Created by the installer | `install-dot.py` header |

All of these are **Device**: the installer or first boot reads them from the unit and a repository check (section 5, M0) must reject any such file name or hash in our tree. The kernel has path quirks that need them placed exactly (`/system/vendor/firmware`, `/vendor/firmware`, `/vendor/etc/audio-algorithms`), otherwise the driver panics or the chip powers on without creating `wlan0` (comments in `boot.sh`).

### 2.4 Boot image, initramfs, installer

| Piece | What it is | Licence | May we ship it? | Source |
|---|---|---|---|---|
| Boot image in `recovery` (about 13 MB, limit 16 MiB) | Android boot format: **header copied from the unit's own recovery backup** (load addresses, command line), T5D's kernel, a ramdisk made on the PC | Composite | **No**: built per unit, "Boot images ... are built per unit and not distributed" | T5D `NOTICE`; `tools/linux/mkimage.py` header; `tools/dotimage.py` |
| Rescue initramfs | PID 1 `tools/linux/init` under static busybox: hand-made `/dev` (`mdev -s`), USB gadget with `acm` (`mt_usb/cmode=1`, `f_acm/instances=1`, `bDeviceClass=02`), root shell on `ttyGS0`, slot choice, tries counter in MISC, Wi-Fi, SSH, firewall | MIT (T5D); busybox GPL-2.0; Alpine packages | Yes (MIT notice); busybox with source | `docs/porting-plan.md` M2, M3 |
| `cmd/wmtup` | Go program that does the WMT detect ioctls and patch download loop so Wi-Fi works with no Android | MIT, derived from EchoMuse emOS (MIT, Wil Bowes) | Yes with notices | `docs/hardware.md` "Wi-Fi without Fire OS" |
| `tools/install-dot.py`, `techo5lib.py`, `dotimage.py`, `update-boot.py` | Installer: check unit, md5-verified backups of 13 partitions, download and verify signed release, build boot image, write `recovery`, unpack slot a, arm boot, wait for a healthy console | MIT | Yes with notice. Re-derive rather than copy where practical | `install-dot.py` header |
| `tools/test_install_dot.py`, `tools/micbench/` | Tests, mic benchmarks | MIT | Learn | file list |

### 2.5 Root filesystem and services

| Piece | Licence | May we ship it? | Source |
|---|---|---|---|
| Alpine 3.24 armv7 userland (49 MB unpacked, 37.2 MB release asset), `busybox` init, musl | per package (MIT, GPL-2.0, BSD ...) | Yes, normally by fetching Alpine packages at build time; shipping a rootfs means shipping or offering the source of GPL packages (T5D attaches `techo5-third-party-sources.tar.gz`, 92.9 MB) | `NOTICE`; `packages-rootfs.txt` |
| `echod` (Go, 31.4 MB static binary) | MIT, from EchoLocal (Yuri Gelfand, MIT) via TECHO5 | **Replaced** by our daemon | `NOTICE` |
| `wpa_supplicant` 2.10-class (static or Alpine), `udhcpc` | BSD, GPL-2.0 | Yes | `docs/hardware.md`; `wpa-patches/` |
| `dropbear` (SSH), legacy `iptables` firewall script `techo5-firewall`, busybox `ntpd` | MIT, GPL-2.0, MIT (script) | Yes | `porting-plan.md` M3b, M3c |
| BlueZ, `bluealsa` 4.3.1 + upstream patch `6ccf455`, `btbridge` | GPL-2.0, MIT, MIT | Yes | `NOTICE` |
| `shairport-sync` 5.6.1 (FFmpeg LGPL parts linked statically), `nqptp` (GPL-2.0), `librespot` 0.8.0 (MIT) | mixed | **Not wanted** by default. Spotify Connect and AirPlay carry the terms notice of spec 0006 section 5.4 | `NOTICE` |
| Wake-word models (12 downloaded by the installer, pinned) | Apache-2.0 for `esphome/micro-wake-word-models`; the community set records no licence | Only the provenance-clean set (REQ-VOI-009) | R-08 section 6 |

### 2.6 Audio path, mixer routes, LED and buttons

These are **facts about the hardware**, not copyrightable files. T5D and EchoMuse state them and R-08 section 3 already summarises them; the new points from T5D's own docs are:

- Capture is `pcmC0D24c`, only **16 kHz, S24_3LE, 9 channels** (six ring mics, centre mic, two playback loopback channels); playback is `pcmC0D23p`, 48 kHz S16_LE stereo. Without Android **nothing opens the codec routes**: userspace must set the mixer (ADC volumes, MICPGA, `HP Driver Gain Volume` 6 for the speaker, `Audio_DacMux_Setting`, `Ext_Speaker_Amp_Switch`), and the speaker amplifier hisses unless fed silence.
- LED frame: one write of 12 x `RRGGBB` ASCII hex to the `frame` file. Buttons by evdev name: `mtk-kpd` (mute 113, action 138), `keys` (volume 114, 115).
- Mute is a software mute (see R-08 section 3.3). Never read the `amz_privacy` file `power_button_state`: it crashes the kernel. Never call `EXT_CHIP_DETECT` on `/dev/wmtdetect`: it panics this kernel.

All of this belongs in the board file (`boards/echo-dot2.json`), which already exists.

### 2.7 Update and slot (A/B) mechanism

- **Store:** directory `/techo5` on the `cache` partition (p15, mounted at `/store`) holds `active`, `slots/a`, `slots/b` (plain directories, not partitions), `slots/<x>.state` (`good`, `trial n`, `bad`), `firmware/`, `twrp.img`, `linux.img`, a `rescue` marker (`slotctl` header; `porting-plan.md` M3).
- **Boot choice:** the recovery initramfs bind-mounts the active slot and `switch_root`s into it. `boot.sh` is `sysinit`; `techo5-run` (the daemon), the console shell and the log keeper are `respawn` entries in busybox init (`inittab`).
- **Trial and rollback:** a new slot has 2 tries; the commit comes when the same daemon process has lived long enough (the plan says 30 s past the first minute, the `slotctl` header says 90 s; **the two disagree and I did not resolve which is current**); a trial that is not healthy within three minutes reboots itself; out of tries it is marked bad and the good slot boots. Tries are also counted in `misc` block 14, and **after five unhealthy boots the unit stays in the rescue initramfs** (USB console, SSH, firewall) until `techo5-retry`. Fire OS only starts when asked (`/sbin/to-android`).
- **Signed releases:** `manifest.json` plus `manifest.json.sig`, an ed25519 signature over the exact bytes; the public key is built into `echod`; the updater uses its own HTTP client and fetches nothing before the clock is set (`porting-plan.md` M3c). Verified on the bench unit by T5D, including a deliberately broken root filesystem going `trial 1`, `trial 0`, `bad`.
- **What is not A/B:** the **kernel and boot image** (in the single `recovery` partition). They go out through the installer or `tools/update-boot.py` over SSH (`docs/building.md` section 7); "the boot image ... is per unit and never published, so changes to `tools/linux/init` still go out through the installer". A failed write there is recoverable only through the unit's `linux.img` or `twrp.img` copies and USB, and I found no automatic fallback for a bad recovery image **(unverified)**. Because the bootloader decides, a normal power-on without a recovery request boots Fire OS.

### 2.8 Install procedure

From the `install-dot.py` header and `README.md` "What you need": prerequisites are an **already unlocked** Dot (amonet), **Fire OS 6574.1 in its system slots**, in TWRP or Fire OS with root adb, a PC with Python 3, adb and git, and two files fetched by the user from the amonet XDA thread (amonet release ZIP with `boot-root.zip`, and Amazon's Fire OS update `update-kindle-biscuit_puffin-NS6574_...bin`). Steps: check unit; back up 13 partitions with md5 (`preloader`, `kb`, `dkb`, `lk_a/b`, `tee1/2`, `expdb`, `misc`, `persist`, `boot_a/b`, `recovery`); download and verify the release; build the boot image from the unit's own recovery backup; keep or create the Home Assistant identity; write `recovery` and read it back; unpack slot a; reboot; wait on the USB console for `healthy`. There is **no confirmation prompt** in a real run (B0 plan section 2.2): the operator's go is the only gate. `--dry-run` stops before any device write. A `--for-someone-else` mode leaves a clean unit that starts its own setup (Improv over Bluetooth).

### 2.9 SSH handling

`dropbear` on port 22, keys only. It is started by `echod` **only while a switch in Home Assistant is on**, with keys that Home Assistant sends over its encrypted link; a new install starts with it off; the installer can add a PC key with `--ssh-key`. Host keys live in `/data/techo5-linux/dropbear`, user keys in `/data/misc/echolocal/ssh`. The rescue initramfs runs its own server with the same keys, without the switch. The firewall allows inbound only 6053, 5353/udp, 22, 8928, DHCP, ping and IPv6 neighbour discovery (`techo5-firewall`). For ClawDot the consequence is that **SSH control currently depends on Home Assistant**, which we do not have (the maintainer's unit has a root shell only over USB serial; see `docs/guides/b0-techo5-trial-plan.md`).

## 3. Other things found that bear on the choice

| Source | What it offers | Catch |
|---|---|---|
| [LibreEcho-Linux](https://github.com/aslater3/LibreEcho-Linux) (`main`, pushed 2026-11-01) | A kernel on **v6.1.178 stable history** plus per-subsystem MT8163 commits, with a nightly canary that replays them onto 6.18.y; the MediaTek connectivity stack (213k lines) moved to an out-of-tree module repository so stable merges do not touch it. T5D already ships LibreEcho's 6.1 kernel as an opt-in for the Echo 2nd gen (`--linux-61`, upstream commit `56a648f0`) | GitHub reports the licence as `NOASSERTION` (a kernel mirror, GPL-2.0 by nature); the connectivity repository has **no `LICENSE` file**; its README says the repository "must not be used to flash a device directly" and a boot image still needs the product pipeline; Dot (`biscuit`) support is "experimental" (R-10) |
| [LibreEcho-mt8163-connectivity](https://github.com/aslater3/LibreEcho-mt8163-connectivity) (pushed 2026-10-24) | Out-of-tree modules `mt8163_wmt_detect`, `mt8163_btif`, `mt8163_stp_wmt`, `mt8163_stp_bt`, `mt8163_wmt_wifi`, `mt8163_wlan`, with a `compat/kcompat.h` layer | Needs hardware acceptance per its own README; init-order changes from the built-in build |
| [postmarketOS biscuit port](https://gitlab.postmarketos.org/postmarketOS/pmaports/-/tree/main/device/archived/device-amazon-biscuit) (archived 2026-06-22) | A distro-grade kernel config for 3.18.19 that enables devtmpfs; T5D keeps its dump | Archived; same 3.18 base |
| [`echo-pmos/amazon-biscuit-kernel`](https://gitlab.com/echo-pmos/amazon-biscuit-kernel) | Fire OS 6.5.5.9 kernel with `biscuit_defconfig`, per T5D | Not opened by me; older than Amazon's 6.5.7.1 release **(unverified)** |
| Mainline Linux for MT8163 | The mainline tree has MediaTek dts work for related SoCs | I could **not** confirm mainline support for the MT8163 audio front end or CONSYS **(unverified)**; assume not usable |
| Amazon GPL source | Public at `https://www.amazon.com/gp/help/customer/display.html?nodeId=G202096090`; T5D downloads `Echo_Dot_src-6.5.7.1-20251024.tar.bz2` from an S3 URL and checks its sha256 | If Amazon moves or removes it, GPL lets us **mirror the source** we obtained, which we should do before depending on it |

Kernel lifetime (primary): Linux **3.18.140 was the last 3.18.y release, announced 2019-05-16** by Greg Kroah-Hartman, who had marked the branch end-of-life years earlier ([LWN](https://lwn.net/Articles/788688), [lkml](https://lkml.indiana.edu/hypermail/linux/kernel/1905.2/00686.html)). Amazon's source is a vendor tree, so some Amazon patches exist, but Amazon's own security cadence for the 2016 Dot is not something I could verify. **6.1 is projected to reach end of life in December 2027** and 6.6 the same month, 6.12 and 6.18 in December 2028 ([kernel.org releases](https://www.kernel.org/category/releases.html)); those dates can be extended.

## 4. Options for ClawDot's own layer

Axes that matter for one maintainer: how much we must write and keep up, update safety on a 16 MiB recovery partition with no second slot, and whether the userland matches our target (`armv7-unknown-linux-musleabihf`, a static musl binary that has working DNS where bionic does not). **Size, boot time and memory figures are T5D's measurements or my estimates, and are marked.** Memory budget: spec 0002 reserves a hard 64 MB for the ClawDot daemon on the 481 MB the Dot has (T5D: about 445 MB free with its whole stack, Go daemon and Bluetooth running).

| | A. Fork T5D's recipe (Alpine, initramfs, slot store) | B. Buildroot | C. Alpine, our own scripts from the facts | D. Minimal initramfs, no rootfs |
|---|---|---|---|---|
| What we write | Port T5D's `init`, `boot.sh`, `slotctl`, `techo5-net`, firewall, installer under MIT notice; swap `echod` for our daemon; keep Alpine packages | Buildroot external tree: defconfig, init scripts, our packages; kernel via Amazon source | Same userland as A, but our own init, store and installer re-derived from T5D's docs | One cpio in `recovery`: kernel, static busybox, our daemon, `wmtup`, wpa_supplicant; mutable state on `userdata` |
| Size (rootfs) | 49 MB unpacked (T5D, measured); trim by dropping BT, AirPlay, Spotify, Go daemon: **estimate 20 to 30 MB** | **Estimate 8 to 20 MB** for busybox, musl, wpa_supplicant, dropbear | As A | **Estimate 10 to 14 MB compressed image** (T5D's rescue image is 9.6 MB of 16 MiB without our daemon), and the whole cpio sits in RAM, **estimate 25 to 40 MB** |
| Boot time | T5D: 21 s to Wi-Fi, 26 s to wake word, 45 to 46 s from the reboot command to the API (about 19 s is shutdown, bootloader and kernel load, which no option changes) | Same chain; **expect the same, within seconds**, because the time goes to the bootloader and the WMT firmware download, not to userland | As A | Same; skips `switch_root` (negligible) |
| Memory floor | Alpine + busybox init is already light; the cost is the daemon and DSP | Slightly lower than A; **not measured** | As A | Highest, because the root is RAM |
| Maintenance one maintainer carries | Moderate: fork of ~10 scripts; Alpine publishes security fixes for a branch until its end date (v3.24 supported to 2028-06-01) and `apk` can fetch them; we follow T5D by diff | **High**: we own every package version and its CVEs; Buildroot LTS is `2025.02.x` to March 2028, the stable `2026.08.x` ends December 2026, so a bump is due each year ([buildroot.org](https://buildroot.org/download.html)) | Moderate to high: A plus the cost of rediscovering what T5D already debugged (`/dev` by hand, USB gadget order, WMT firmware paths, `skip_initramfs`) | Low packages, but **no atomic update** |
| Update safety | A/B rootfs with trial and rollback (T5D, verified on the bench unit). The kernel/boot image is single-slot (section 2.7) | Same constraint: slot logic still needed | Same | **Worst**: replacing the only image is the update, and a bad one needs USB recovery. Boot slots cannot take a custom ramdisk (`skip_initramfs`), so a second slot is not available. The only fix is a small loader image that picks a slot, which is A again |
| Licences | MIT notices; GPL for busybox, kernel, iptables with source offer | Buildroot is GPL-2.0 (the tool); output is per-package; produces a licence manifest (`legal-info`) | As A | As A |
| Risk | Tracks one upstream | Highest effort | Re-learns solved bugs | Dead end for updates |

Verdict: **D collapses into A** the moment it needs safe updates. **B** buys a reproducible build system at the cost of owning the whole package set, which one maintainer should avoid on a device whose real risk is the bootloader chain and kernel, not the userland. **C** only differs from A by discarding working, field-tested scripts. **A**, executed as "fork the design, replace the daemon, re-derive each script with the notice", has the lowest risk, and the "own" in "our own layer" is delivered by owning the repository, build, signing key, installer and kernel pin, not by rewriting a boot sequence that took T5D and EchoMuse weeks to get right.

Kernel options (independent of the userland choice):

| | K1. Stay on 3.18.19 (T5D's kernel) | K2. Move to LibreEcho's 6.1.y | K3. Mainline |
|---|---|---|---|
| Security | No upstream fixes since 2019; only T5D's 48 Bluetooth backports. Wi-Fi driver is a 213k-line vendor stack. Mitigated by the inbound firewall and no exposure to the Internet | Upstream stable fixes arrive by merge; end of life projected December 2027 (extendable); but vendor drivers are ported by one person | Not confirmed possible |
| Effort | Pinned binary now, own reproducible build later | Spike first; hardware acceptance of Wi-Fi, BT, audio, LED, buttons; installer must handle the changed boot envelope (R-10 notes LibreEcho flashes both boot slots and formats `userdata`, which we must not) | Large, unknown |
| Risk | Known to work on the maintainer's unit (TECHO5 runs it) | Dot support "experimental"; a bad kernel in the single recovery partition is the brick-adjacent risk | Unknown |

## 5. Recommendation

**Fork the T5D design into a ClawDot-owned `clawdot-os` repository (Option A) and own the layer in this order: recipe, updates, installer, then kernel.** Keep T5D's kernel binary pinned by hash for the first milestones, so there is one change at a time on the maintainer's only unit.

In five lines:

1. Create `clawdot-os` (the maintainer does this, per ADR-0015) with Alpine 3.24 armv7, T5D's boot design (recovery-partition boot, slot store on `cache`, rescue initramfs, five-boot rescue) ported script by script under MIT notices, and **our daemon in place of `echod`**; no Home Assistant, no ESPHome, no Go daemon, no Bluetooth/AirPlay/Spotify by default.
2. Treat **Amazon-derived files as install-time inputs only** (firmware, audio tuning, `audio_device.xml`, the unit's recovery header), enforced by a repository check; keep the unlock out of scope and linked.
3. Replace T5D's update path with a **ClawDot signing key, a ClawDot manifest and slot logic** in the image recipe (REQ-DEV-007), and add what T5D lacks: a power-cut test and a two-slot or loader-assisted path for the boot image.
4. Take the kernel in two steps: pin T5D's `techo5-dot-kernel-bt.zImage-dtb` by sha256 first; later build our own from Amazon's GPL source with a smaller config and publish the source and a mirror; run a **6.1 kernel spike** as a separate, evidence-gated milestone, never as part of an install milestone.
5. Be honest about the limit: the unlock, Fire OS 6574.1 and a PC with Python remain prerequisites, and a 3.18 kernel stays a documented risk until K2 passes.

### 5.1 Milestones and exit tests

Each milestone leaves the previous one working. Every step that writes to a device needs the maintainer's explicit go (roadmap rule); milestones M0, M1 and M3 are PC-only.

| ID | Milestone | Exit test |
|---|---|---|
| B4b-0 | **Repository and rules.** The maintainer creates `clawdot-os`; `LICENSE`, `NOTICE` (T5D, EchoLocal, EchoMuse, amonet credited), a vendoring policy, and a CI check that fails on any Amazon, MediaTek or boot-image file by name, size or hash, and on any file without a licence line | The check passes on the repository and fails on a planted `WIFI_RAM_CODE_8163` and a planted `boot.img` (a test that the check catches them) |
| B4b-1 | **Reproducible PC build.** Rootfs tarball and ramdisk built from pinned Alpine packages and the pinned T5D kernel asset, with our daemon (`clawdot-device`) as the only application; a source bundle for the GPL parts; `install --dry-run` against a recorded fixture of the maintainer's backups | Two clean builds give the same rootfs hash (`SOURCE_DATE_EPOCH`); the dry run lists the same writes as T5D's in section 2.8 and no others; the licence manifest names every package |
| B4b-2 | **First boot from our recipe on the maintainer's unit.** One device-write session with his go and the B0 restore path proven first | Console shows `healthy`; Wi-Fi up; our daemon runs; one voice turn through the brain; restore to the previous state tested in the same session; recorded in `docs/reports/` |
| B4b-3 | **Own signed A/B updates** (this is roadmap B5 with the recipe in hand). ClawDot manifest and key; trial, commit, rollback; update messages of spec 0003 | A signed update installs and is confirmed; an unsigned one is refused; a rootfs with a crashing daemon rolls back unaided; a power cut during unpack leaves the old slot booting; run recorded |
| B4b-4 | **Own installer and guide for a normal person** (roadmap B4 exit). One command, `--dry-run`, verifies backups before the first write, adopts firmware and tuning, ends on a health check | A person who has not seen the repository, on a unit already unlocked, reaches a voice turn following only `docs/guides/install-on-echo-dot-2.md` |
| B4b-5 | **Kernel ownership.** Our build script from Amazon's GPL source (mirrored), config diff against the dump reviewed, patches listed with CVEs, size recorded | Our zImage boots on the unit with the same functions as the pinned one; `uname`, config diff and size recorded; the source bundle is complete enough to rebuild |
| B4b-6 | **Boot-image update safety.** Either a loader stage that selects between two ramdisk images, or a verified write plus automatic return to the previous image | A deliberately truncated boot-image write is detected and does not leave the unit unbootable; recorded. If neither is possible, the guide says "kernel updates are manual and the rescue path is X" (REQ-DEV-007 `manual` for that part) |
| B4b-7 | **Kernel 6.1 spike (decision, not delivery).** LibreEcho's 6.1.y with the connectivity modules under our rootfs on the spare or sacrificed unit | Written finding: Wi-Fi, audio capture and playback, LED, buttons, memory and boot time measured; go or no-go on K2 with the end-of-life date stated |
| B4b-8 | **Sever the dependency.** Nothing in install or update downloads from a TECHO5 or LibreEcho repository | A network-blocked build and install dry run succeeds from ClawDot-hosted assets only |

### 5.2 Risks

| Risk | What happens | Mitigation |
|---|---|---|
| **Bricking** | Preloader keeps boot attempts per slot; if both run out the unit stops booting and recovery means opening the case (R-08 section 3.8). amonet v2 cannot be undone by flashing old bootloaders. A bad write to the single `recovery` partition | Never touch preloader, LK or TEE; md5-verified backups before the first write; read-back after every write; recovery-partition boot only (T5D design leaves Fire OS in the boot slots); B4b-6; the maintainer's go for each device write; start new work on a spare unit |
| **Licences** | GPL-2.0 kernel and busybox need source offers; MIT needs the notice; a rootfs that carries LGPL FFmpeg or GPL nqptp needs more. LibreEcho connectivity repository has no `LICENSE` file. Amazon firmware and tuning must not ship. Piper and openWakeWord models have licence limits (R-08 section 6). Wake-word models without a recorded licence fail REQ-VOI-009 | NOTICE and an automatic licence manifest per release; Alpine packages fetched not vendored; the B4b-0 check; GPL source bundle attached to each release as T5D does; drop LGPL/GPL extras we do not need |
| **Kernel 3.18 security** | Unfixed upstream since May 2019 beyond the 48 Bluetooth patches; reachable surface: Wi-Fi vendor driver, TCP/IP stack, Bluetooth, anything we listen on | Inbound firewall dropping everything except named ports (as T5D); Bluetooth off by default; no listener the portal does not need; the portal's own token and TLS; a documented "do not expose this device to the Internet" rule; K2 spike; an honest note in the guide |
| **Blobs** | Wi-Fi and audio need Amazon/MediaTek files that exist only on the unit; wiping Fire OS or a repartition loses them | Keep Fire OS `system_b` untouched; copy the adopted files into the store and into the user's backup folder; the installer refuses to continue if it cannot read them |
| **Amazon source link** | The S3 link or the notices page may change | Mirror the GPL source we use (permitted by GPL) before B4b-5 |
| **Upstream drift** | T5D changes layout, licence or disappears | We fork the design, pin by hash, and keep our notices; B4b-8 removes the live dependency |
| **Unlock dependency** | The unlock (amonet, `firebreak`) is third-party and may change or break | Link only; pin the tested versions in the guide (R-09 section 9) |
| **One maintainer** | Hardware work needs him present | Keep the PC-only milestones (0, 1) large and the device milestones small and scripted |

## 6. Install and update experience for a normal person

**What T5D asks today:** unlock the Dot with a third-party tool, find two files on a forum thread, install Python 3, adb and git, run a script with no confirmation prompt, and then pair with Home Assistant. That is a hobbyist path. ClawDot can lower it but not remove the unlock.

**Target experience** (what the guide and tool must deliver by B4b-4):

1. **One honest page before anything.** What the project changes, that the unlock is third-party and can brick a unit, that Amazon's service stops working, that a 3.18 kernel is old, and that the first attempt should be on a spare Dot. The page lists what is needed (Dot 2nd gen with Fire OS 6574.1, a PC, a micro-USB cable) and why each item is needed.
2. **Unlock stays outside our tooling.** The guide links the unlock procedure and the pinned tested versions, credits the people involved (R-09 section 9), and says which Fire OS version to keep.
3. **One command to install.** `clawdot-install` (name open) that runs the checks of section 2.8, refuses to write until the backups are verified, shows the plan, then asks for an explicit yes. `--dry-run` writes nothing. It adopts the unit's own firmware and tuning, flashes, and waits for a health check. No Home Assistant, no SSH, no Python packages beyond a single downloadable binary or a small script **(open question 4)**.
4. **Setup without a shell.** After first boot the Dot shows its state on the ring, offers Wi-Fi setup without a keyboard (access point or Improv over Bluetooth; T5D already does Improv), and the portal first-run asks the administrator to set a password (ADR-0013). The ring and a spoken line say when it is ready.
5. **Updates look like the portal.** The portal shows "update available" with the version and notes; the user accepts; the Dot installs to the spare slot, reboots, confirms, and the ring shows `updating` (spec 0006 LED state); a failure rolls back and the portal says so. Updates are signed with the ClawDot key; an unsigned or tampered one is refused and the reason shown.
6. **Kernel and boot-image updates are rare and guarded.** They are separate from application updates, require the verified-write path from B4b-6 (or are declared manual), and the portal says clearly when one is pending.
7. **A way back that is documented and tested.** "Restore my Dot to what it was" is a single guide section and a command, using the backups the installer made, and a USB-console section for a unit that will not boot. Rescue mode after five bad boots (T5D design) keeps the unit reachable.

## 7. Open questions for the maintainer

1. **Licence of `clawdot-os`.** Apache-2.0 like the main repository, with MIT notices kept for ported T5D material (compatible), or a different one for the scripts?
2. **Is a pinned dependency on T5D's published kernel acceptable for the first milestones**, or must the first boot already use our own build (which moves B4b-5 ahead of B4b-2 and doubles the unknowns on one unit)?
3. **Fire OS must stay on the unit** for the firmware adoption to work. Is that acceptable, or do you want us to later extract the firmware once into the user's backup and let the install proceed without `system_b`?
4. **Installer form for normal people:** a Python script like T5D's, a single static Rust binary built from this workspace, or a browser-based installer (LibreEcho has a preview; R-10 notes an open Dot failure)? A Rust binary fits the toolchain but needs adb/fastboot handled.
5. **Bluetooth, AirPlay and Spotify Connect:** drop by default (assumed here), or keep T5D's Bluetooth kernel because B6 wants Bluetooth speaker mode? This decides whether the 48 backports and the BlueZ chain stay.
6. **Second unit.** B4b-2, B4b-6 and B4b-7 are safer on a spare Dot. Do you have one, or should the plan assume a single unit and keep the restore path as the safety net?
7. **Raspberry Pi and other boards:** do you want `clawdot-os` to host their recipes too (B7), which affects the repository layout now?
8. **How hard to try for 6.1:** is a kernel with a December 2027 end of life (extendable) enough to justify the spike, given that the unit is a 2016 product?
9. **Hosting of GPL source and the release assets:** where, and who holds the signing key (ADR-0015 says the maintainer)?
10. **Slot commit time:** T5D's own documents disagree (30 s past the first minute versus 90 s). Worth reading `boot.sh` and `echod` on the unit during B4b-2 before we copy the number.

## 8. Sources

- T5D: `https://github.com/HuskerMinion/techo5-dot`, tag `dot-v1.0.61` (`NOTICE`, `README.md`, `docs/hardware.md`, `docs/porting-plan.md`, `docs/building.md`, `tools/linux/*`, `tools/install-dot.py`, `tools/dotimage.py`); main repository `https://github.com/HuskerMinion/techo5` (daemon; MIT).
- EchoLocal: `https://github.com/ygelfand/echolocal` (MIT). EchoMuse / emOS: `https://github.com/wilbowes/EchoMuse` (MIT).
- Unlock: `https://github.com/R0rt1z2/amonet` (branch `mt8163-biscuit`), `https://github.com/bboe/firebreak` (BSD-2-Clause), history in [R-09](09-echo-unlock-history.md).
- LibreEcho: `https://github.com/aslater3/LibreEcho-Linux`, `https://github.com/aslater3/LibreEcho-mt8163-connectivity`; evaluation in [R-10](10-libreecho-evaluation.md).
- Kernel lifetimes: `https://www.kernel.org/category/releases.html`; 3.18.140 final: `https://lwn.net/Articles/788688`.
- Alpine support dates: `https://alpinelinux.org/releases/`; armv7 downloads: `https://alpinelinux.org/downloads/`. Buildroot releases: `https://buildroot.org/download.html`.
- Amazon GPL source notices: `https://www.amazon.com/gp/help/customer/display.html?nodeId=G202096090` (cited by T5D; not opened by me).
- Repository documents: [ADR-0015](../adr/0015-own-the-device-layer.md), [spec 0006](../specs/0006-device-layer-contract.md), [B0 trial plan](../guides/b0-techo5-trial-plan.md), [R-07](07-echo-dot-gen2-hardware.md), [R-08](08-existing-echo-dot-projects.md).

## Spec impact (recommended; applied as spec 0006 version 0.5)

REQ-DEV-032..037: no Amazon or MediaTek file in the recipe repository, install-time reading of proprietary files, verified backups and dry run before a first write, a recoverable kernel and boot-image update, source offer for GPL components, and a licence manifest per release. Roadmap B4b added.

## Confidence

Medium-high for the inventory (primary files read at one tag, cross-checked with R-08 and R-07); medium for the option comparison (sizes and boot times marked as estimates where not from T5D); low for anything about 6.1 on the Dot, mainline MT8163 support, the licence of the MediaTek connectivity code, and the behaviour of a bad recovery image (all marked unverified). Nothing here has been run on a device.
