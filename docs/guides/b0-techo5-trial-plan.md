# B0 trial plan: TECHO5 Dot on the maintainer's Echo Dot 2

Status: **plan for review, nothing in it has been run.** Written 2026-11-09 from primary sources only. No agent touched the device while writing it, and no agent will run any step in it: **the maintainer types every command and gives every go.** Roadmap step: [B0](../roadmap.md#b0-trial-of-the-techo5-dot-base-image-spike). Background: [R-07](../research/07-echo-dot-gen2-hardware.md), [R-08](../research/08-existing-echo-dot-projects.md), [R-09](../research/09-echo-unlock-history.md), [open-questions.md](../open-questions.md).

The unit is an Echo Dot 2nd gen (`biscuit`, AEOBC), unlocked, rooted Fire OS 6.5.7.5 with root `adb`, TWRP in the recovery partition, ClawDot running from `/data/local/tmp` (R-07). The trial installs the TECHO5 Dot base image ([HuskerMinion/techo5-dot](https://github.com/HuskerMinion/techo5-dot), MIT) and answers the B0 questions in section 10.

## 0. How to read this plan

**Evidence tags.** Every claim carries one.

| Tag | Meaning |
|---|---|
| **[S: source]** | Read in a primary source on 2026-11-09 (file, line of code, issue). The source is named. |
| **[I]** | My inference from sources, not stated by them. |
| **[U]** | Unverified. Nobody has shown it on a unit like this one. Treat as a question. |

**Source short names.** `T5D` = `HuskerMinion/techo5-dot` at commit `13482c9` (main, 2026-11-08), release `dot-v1.0.60` (2026-11-09). `T5` = `HuskerMinion/techo5` (the daemon `echod`; its source is under `echod/`) at commit `e7a8420` (main, 2026-11-09). `FB` = `bboe/firebreak` v1.0.0, commit `a309fa6`. `#n` = an issue in `T5D` unless stated. The upstream moves daily (a release roughly every day), so **pin the installer to the commit above and the release to `dot-v1.0.60`, and re-read the diffs before the day of the trial** (step A2).

**Marking.** Each step is labelled **READ-ONLY** (nothing on the Dot changes), **PC-ONLY** (writes on the computer only) or **WRITES TO DEVICE** (changes flash or files on the Dot). Every WRITES TO DEVICE step has an expected output and a stop condition. Stop means: do nothing more, keep the Dot powered and plugged in, and go to section 11.

**Rules from the project.** `docs/roadmap.md` requires the maintainer's explicit go before each device write. Section 12 is the sign-off sheet. AGENTS.md rules (no secrets in logs or docs, backups never committed) apply to everything you produce.

## 1. What this trial is, in one page

1. TECHO5 Dot replaces Fire OS **at runtime** with Alpine Linux and one Go daemon, `echod`. Linux boots **from the recovery partition**; Fire OS stays in the boot and system slots and is not started [S: T5D README "What happened to the Dot"; T5D docs/porting-plan.md "It goes in the recovery partition"].
2. The Dot becomes an **ESPHome voice satellite**: `echod` is the ESPHome API **server** on port 6053 and Home Assistant is the client. ClawDot would be a different client [S: T5 `echod/internal/feature/api/api.go`; R-08 section 4].
3. **TWRP is replaced** by the Linux image. A copy of the unit's own TWRP is kept in `/cache/techo5/twrp.img` and `to-twrp --yes` puts it back [S: T5D tools/install-dot.py steps 5 and 6; tools/linux/rootfs/usr/local/sbin/to-twrp].
4. The install does **not** touch the boot slots, system slots, `lk`, `tee`, `preloader`, `expdb` or `persist` [S: install-dot.py writes only `recovery`, `misc`, the cache store and files on `/data`]. That is why the rollback is small.
5. **Three things can stop the trial before any write**, in order of seriousness:
   - **K1. Fire OS build.** TECHO5 documents and tests Fire OS **6574.1** (NS6574, builds 7623, 8138, 8146, 8149) and says "other builds don't match" [S: T5D README "What you need"; T5 docs/getting-started.md step 2]. R-07 found **6.5.7.5**. Firebreak maps 6.5.7.5 to **NS6575, build 8349**, the newest biscuit build [S: FB docs/unlocking.md "6.5.7.5 (NS6575/8349)"; FB docs/devices.md builds table]. A search of all `T5D` issues for 8349, 8338, 8344 and 6.5.7.5 found **no report** [S: `gh search issues`, 2026-11-09]. Every installer log in the issues is on 6.5.7.4 or 6574.1 [S: #8, #12, #19, #3]. The installer itself only checks that the build name contains the text `Fire OS 6` [S: install-dot.py line 450]. So the installer will probably accept the unit; whether the result works is **[U]**. Decision in step B5 (decision K1).
   - **K2. Tools for the worst case.** The maintainer's PC is Windows 11 build 26300. Firebreak's bootrom path is reported broken on Windows 11 25H2 (build 26200) because of UsbDk, and is tested only from an administrator PowerShell [S: FB docs/troubleshooting.md "Windows"]. The worst-case recovery (section 11, R-D) therefore needs a **Linux live USB** ready *before* the trial.
   - **K3. Home Assistant coupling.** TECHO5's setup page is reported unreachable without Home Assistant, even after the action-button press [S: #40, open; the reporter tried port 80, the page is on 8181, so the report may be partly a user error, **[U]**]. The SSH switch is documented as a Home Assistant switch, the remote API cannot change it, and a setup-page equivalent was not found [S: T5 docs/setup.md "The setup page", "11. The remote API"; **[U]** whether one exists]. Without Home Assistant the **only** shell is the USB serial console (root, no password) [S: tools/linux/rootfs/etc/inittab `techo5-console`; tools/linux/rootfs/etc/techo5/boot.sh "USB serial console"]. The plan uses it.

## 2. Prerequisites, and what is installed, replaced and erased

### 2.1 What you need on hand (before the first command)

| Item | Why | Source |
|---|---|---|
| The Dot, powered **only through its micro-USB cable** from the PC | The Dot's only power input is micro-USB; adb and the serial console use the same cable | [S: T5 docs/getting-started.md step 1] |
| A **data** micro-USB cable, short, known good; the PC's own **USB 3 port**, no hub or dock | A bad cable breaks backups and the console | [S: install-dot.py `CABLE`; FB docs/troubleshooting.md "USB"] |
| Windows power plan: USB selective suspend **off**, sleep off, mains on | A power drop during a write is the main brick risk | [I] |
| Python 3, Android platform-tools (`adb`, `fastboot`), `git` | The installer is a Python script | [S: T5D README "What you need"] |
| 15 GB free on the PC, on an encrypted volume | Backups hold your Wi-Fi password, the Noise key and device identity | [S: install-dot.py reads `/data/misc/wifi/wpa_supplicant.conf`; R-07 section 8] |
| A second storage place off the PC (NAS share or USB disk) | Backups on the machine you may be recovering from are one copy | [I] |
| A **Linux live USB** (Ubuntu) with `uv`, `adb`, `fastboot`, `libusb` and the firebreak udev rules, tested by booting it once | Worst-case unbrick (section 11, R-D) | [S: FB README "Install"; T5 docs/getting-started.md "On macOS": a live USB is more reliable than a VM] |
| Torx T8 screwdriver, a fine wire or tweezers, photos of the XDA short point (print or save offline) | Only for R-D | [S: FB docs/troubleshooting.md "An Echo Dot (2nd Generation) with no light"] |
| Two offline copies of: the XDA thread text (all of it, with the unbrick section), the amonet-biscuit release ZIP, `update-kindle-biscuit_puffin-NS6574_user_7623_...bin`, `firebreak` v1.0.0 wheel | XDA needs a login and may change; research could not fetch the thread (HTTP 403) [R-09 section 10]. The thread is "[UNLOCK][ROOT][TWRP][UNBRICK] Echo Dot 2nd Gen / 2016 (biscuit)" | [S: T5 docs/getting-started.md; R-09] |
| A Home Assistant instance, **optional**, version 2026.9 or later for "Add it", 2025.7.1 was too old | Needed only to switch on SSH and to use the setup page's HA flows. Not needed for the install or the measurements | [S: T5 docs/setup.md "First run"; #8] |
| A serial terminal for Windows (PuTTY serial, or `plink -serial COMx`) | The TECHO5 console is a USB CDC serial port (`18d1:4ee7`) | [S: boot.sh "USB serial console"; T5D README "Echo 2nd gen" note] |

### 2.2 What is installed, replaced, erased

Partition numbers are `biscuit` as read on TECHO5's bench unit (4 GB eMMC); **read yours in step B2 and compare** [S: T5D docs/hardware.md "First unit"; U for the maintainer's unit].

| Partition | Size | Touched by the install? | What happens |
|---|---|---|---|
| `recovery` (p12) | 16 MiB | **Yes, overwritten** | TWRP replaced by the Linux boot image (~13 MB; must be under 16 MiB) [S: install-dot.py step 5; log in #19] |
| `misc` (p8) | 512 KiB | **Yes, a few bytes** | Block 14 gets `TECHO5-TRIES 0 installed`; at every Linux boot the init writes `boot-recovery` into the first 32 bytes so the next boot is Linux again, and breadcrumbs at sector 17 on and from sector 128 [S: install-dot.py step 7; tools/linux/init] |
| `cache` (p15) | 784 MiB | **Yes** | New directory `/cache/techo5` holding the slots (rootfs, ~49 MB unpacked), `linux.img`, `twrp.img`, `back-to-linux.sh`, `firmware/` [S: install-dot.py `SLOT_SCRIPT`; slotctl header] |
| `userdata` (p16) | 1.2 GiB | **Yes, additively** | Adds `/data/misc/echolocal/{name,psk,models/,ssh/}`, `/data/techo5-linux/{wifi.conf,logs}` and later logs. **Nothing is erased**: ClawDot in `/data/local/tmp` and Fire OS data stay [S: install-dot.py step 6, no wipe command; boot.sh mounts p16 at `/data`] |
| `boot_a`, `boot_b` | 16 MiB each | No | Fire OS boot images stay as they are [S: porting-plan.md "Both boot slots stay untouched"] |
| `system_a`, `system_b` | 768 MiB each | No (read only) | At first Linux boot `boot.sh` mounts **p14 (system_b)** read-only to copy Wi-Fi firmware, audio coefficients and `audio_device.xml` into the store, once. It does not follow the active slot [S: boot.sh `SYSTEM=/dev/mmcblk0p14`, `adopt_audio`, firmware block] |
| `preloader` (boot0), `kb`, `dkb`, `lk_a/b`, `tee1/2`, `expdb`, `persist` | small | No | Read only, for backups |
| Fire OS, Alexa | | Not started | Fire OS can be started again on request; see section 11 |

Installer facts to keep in mind [S: install-dot.py]:
- There is **no confirmation prompt** anywhere in the real run. Your go is the only gate.
- Run from TWRP, the installer stages files in `/tmp/techo5` (RAM). Run from Fire OS, it stages in `/data/local/tmp/techo5`, next to ClawDot, and removes only `rootfs.tar.gz`, `busybox`, `slot.sh` from it afterwards.
- It downloads 12 wake-word models from two GitHub repositories at pinned commits and checks sha256 for each [S: `MODEL_SHA256`, `EXTRA_MODELS_COMMIT`]. The image also carries the models and copies any missing ones at every boot, so you cannot keep the community ones off the unit by an installer flag [S: boot.sh "The wake word models the image carries"]. Licence stance in section 10, Q6.
- It makes a Home Assistant name and a Noise key if the unit has none, saves the key at `backups/<serial>/home-assistant.key` and prints it once. **That key is a secret.** ClawDot will need it later.

## 3. Phase A: prepare the computer (PC-ONLY)

**A1. Fresh working folder on the encrypted volume.** Example `E:\b0\`. Everything below runs from there. Nothing from it goes into the ClawDot repository.

**A2. Get the installer at the pinned commit, then read what changed.**

```powershell
git clone https://github.com/HuskerMinion/techo5-dot E:\b0\techo5-dot
cd E:\b0\techo5-dot
git checkout 13482c94cd7559a8fb2d9e23aaec6f9c5f77f8d1
git log --oneline 13482c9..origin/main     # what changed since this plan was written
```

Stop condition: if anything under `tools/install-dot.py`, `tools/techo5lib.py`, `tools/dotimage.py`, `tools/linux/init` or `tools/linux/rootfs/etc/techo5/boot.sh` changed after `13482c9`, read the diff and update this plan *before* going on.

**A3. Check the tools.**

```powershell
python --version ; adb version ; fastboot --version ; git --version
```

Expected: Python 3.x, platform-tools of the current Google release [S: T5 docs/getting-started.md "Set up your computer"]. Plug the Dot in once and run `adb devices`; it must list the Dot as `device` (Fire OS) with no authorization prompt [S: T5 docs/getting-started.md step 4: the Dot has no screen, the adb key must be trusted]. If it says `unauthorized`, stop; fixing it writes `/data/misc/adb/adb_keys` on the Dot, which is a device write and needs its own go.

**A4. Read the other side's rescue text offline.** Open `docs/hardware.md` "Brick recovery" and `docs/troubleshooting.md` of both repositories, and the XDA thread's unbrick section, and save them next to the backups. Confirm you can name the key combinations for your unlock version (step B2 tells you which version you have): Vol-Down at power-on gives amonet's fastboot, Vol-Up gives TWRP (v2.0.0); with TECHO5 installed Vol-Up starts TECHO5, not TWRP [S: T5D docs/hardware.md "Unlock"; T5 docs/getting-started.md "Going back to TWRP or Fire OS"].

## 4. Phase B: read-only inventory and baseline (READ-ONLY)

Run from Fire OS with root adb, the unit's current state. All commands read.

**B1. Baseline of the current system** (needed to compare idle numbers later; R-07 measured `MemAvailable` about 290 to 300 MB with the stock stack idle):

```powershell
adb shell "uname -a; cat /proc/meminfo | head -5; cat /proc/loadavg; df /data /cache /system; getprop ro.build.version.name; getprop ro.build.fingerprint; getprop ro.boot.slot_suffix"
adb shell "top -n 1 -b | head -25"
adb shell "ps -A -o PID,RSS,NAME | sort -k2 -n -r | head -20"
```

Record the output (no serials, MACs, SSID). Note the ClawDot process RSS and CPU in this state; it is the "before" row of the report.

**B2. Partition table and unlock version.**

```powershell
adb shell "cat /proc/partitions; ls -l /dev/block/platform/bootdevice/by-name/ | cut -c30-; cat /proc/cmdline | tr ' ' '\n' | grep -E 'bootopt|unlocked|buildvariant'; cat /proc/idme/device_type_id | tr -d '\0'; echo"
```

Expected [S: T5D docs/hardware.md "First unit"; install-dot.py]:
- 3,817,472 KiB disk, `mmcblk0p1..p16`, names `boot_a`/`boot_b` 16384 KiB and **no** `boot_a_x` or p17/p18 means amonet **v2.0.0** layout. `boot_a_x`/`boot_b_x` plus two 110 MB partitions means v1.1.0.
- `device_type_id` is `A3S5BH2HU6VAYF` for a `biscuit` [S: dotimage.py `BOARD_TYPE_IDS`]. Anything else: **stop**.
- `bootopt=64S3,32N2,32N2`, `unlocked_kernel=true`, `buildvariant=userdebug` as in R-07.

Stop condition: p12 is not `recovery` of 16 MiB, p8 is not `misc`, or p14/p15/p16 are not `system_b`/`cache`/`userdata`. The init and `boot.sh` use these numbers hard-coded [S: tools/linux/init `MISC_DEV=8 ... DATA_DEV=16`]. A different table means a different layout and the image would write to the wrong places. Do not proceed.

**B3. OTA state.** TECHO5 stays off Fire OS, but a rollback boots it. Check that Amazon cannot update the unit meanwhile.

```powershell
adb shell "cat /system/etc/hosts | head; ps -A | grep -iE 'otad|ace_otad|update_engine'"
```

Firebreak disables `otad` and points the update hosts at `127.0.0.1` in each Fire OS 6 slot [S: FB CHANGELOG, R-09 section 6]. If the hosts file has no such lines and `otad` is running, the unit is **not** blocked: keep it off the network whenever it boots Fire OS (R-07 section 10 rule, [U] whether it is a real risk on 8349).

**B4. Read the Fire OS builds of both slots from TWRP** (this one needs the Dot in TWRP, so do it in step C1 while there). Compare slot `_a` and `_b` `ro.build.version.name` and `ro.build.fingerprint`. TECHO5 reads firmware from **p14 (system_b)** regardless of the active slot, so a mismatch matters [S: boot.sh].

**B5. Decision K1 (maintainer).** Choose one, write the choice and reason in the report.

| Option | What | Writes | Risk |
|---|---|---|---|
| **A. Try as is** (recommended first) | Install on 6.5.7.5 / 8349 after the dry run | recovery, misc, cache, userdata only | Image or Wi-Fi firmware mismatch is **[U]**. Failure is recoverable by section 11 R-A or R-B because boot slots, LK and TEE are not touched |
| B. Reflash 6574.1 first, as T5 documents | `twrp wipe cache`, `twrp wipe data`, install `update-kindle-biscuit_puffin-NS6574_user_7623_0013121734532.bin` twice (both slots), then `boot-root.zip`, trust the adb key | `system_a`, `system_b`, `boot_*`, **erases `/data` including ClawDot and the Wi-Fi setup** | Downgrade of system under a newer bootloader (the 2026-06-17 LK of builds 8338 to 8349) is **[U]**. Biggest write of the trial. Do not do this without a separate review |
| C. Stop | Wait for upstream to document 6.5.7.5 | none | Costs time only |

Why A first: the only things TECHO5 takes from the Fire OS system partition are Wi-Fi firmware, audio coefficients and `audio_device.xml` [S: boot.sh]; the kernel is the release's own, built from Amazon's 6.5.7.1 source [S: T5D NOTICE]. Whether those files differ between 6574.1 and 6575 is **[U]**. Option A's worst case is "Wi-Fi never comes up, unit stays in rescue", and rescue has a USB console [S: tools/linux/init `usb_console`, `wifi`].

## 5. Phase C: full backups (READ-ONLY on the device, PC-ONLY on the computer)

Take every backup before the first write. Do all of C in one TWRP session so the files describe one state. The installer's own backup (C3) comes from the same session.

**C1. Enter TWRP.** WRITES a few bytes to `misc` (Android's reboot-to-recovery request) and starts TWRP from RAM; nothing else.

```powershell
adb reboot recovery
adb wait-for-recovery ; adb devices
```

Expected: the Dot's ring turns white, `adb devices` lists it as `recovery`, `adb shell id` says `uid=0` [S: T5D README, T5 getting-started step 1]. If the ring stays blue or cycles blue/cyan: bad cable or port, not TWRP [S: FB docs/troubleshooting.md]. Stop and change cable and port.

**C2. Read both Fire OS builds (B4) and keep the proof.**

```powershell
adb shell "mkdir -p /tmp/s; for s in a b; do mount -t ext4 -o ro /dev/block/platform/bootdevice/by-name/system_\$s /tmp/s 2>/dev/null; echo slot \$s; grep -E '^ro.build.version.name|^ro.build.fingerprint' /tmp/s/system/build.prop; umount /tmp/s; done; getprop ro.boot.slot_suffix"
```

[U] on exact paths: the installer reads `/tmp/t5sys/system/build.prop` after a read-only mount of `system<slot>`, so `system/build.prop` under the mount is right [S: install-dot.py lines 442-443]. If the by-name `system_a` link does not exist, `ls` the by-name folder first.

**C3. The installer's partition backup and dry run.** This is the installer's own "full backup" of everything that boots the unit, md5-checked against the device, with the read-only dry run [S: install-dot.py steps 1 to 4, `--dry-run`].

```powershell
cd E:\b0\techo5-dot
python tools/install-dot.py --serial <serial> --dry-run --name "ClawDot Dot" --wake-words okay_nabu,hey_jarvis,hey_mycroft --backups E:\b0\backups --work E:\b0\build
```

Choices: `--wake-words` limits the models the installer *downloads* to the three Apache-2.0 ones [S: R-08 section 6; argparse in install-dot.py]; `--name` avoids a prompt; add `--ssh-key <file.pub>` if you want a key placed for a later SSH switch [S: install-dot.py]. Do not use `--for-someone-else` (it clears state). Do not pass `--wifi` if the unit already holds the Wi-Fi network from Fire OS; the installer says `saved Wi-Fi network <name>` and keeps it.

Expected output (shape from #19, #12; values differ):

```
== device <serial>
   biscuit, Fire OS 6.5.7.5 (NS6575/8349?), slot _x, in TWRP
== backups to E:\b0\backups\<serial>
   preloader 4194304 bytes, md5 ok
   kb ... lk_a ... lk_b ... tee1 ... tee2 ... expdb ... misc ... persist ... boot_a ... boot_b ... recovery ... md5 ok
== the release, and this unit's boot image
   release manifest signed by the project key
   TECHO5 Dot v1.0.60: root filesystem, Bluetooth kernel and rescue packages checked
   kernel: techo5-dot-kernel-bt.zImage-dtb (Bluetooth)
   boot image 132xxxxx bytes, root filesystem 37xxxxxx bytes
Dry run: checks, backups and builds done; nothing written to <serial>.
```

The exact `Fire OS` text on the unit is **[U]**.

Stop conditions (every one stops before any write because it is a dry run):
- `adb does not see` / `is not an Echo Dot 2nd gen` / `device type id ... says` mismatch.
- `this installer is for Fire OS 6`: the build name does not contain `Fire OS 6`. Do not edit the installer to get past it.
- `<partition> copy does not match the device`: cable or port [S: `CABLE`]. Change both and re-run; repeated failure means a failing eMMC, run nothing else.
- `... points at a decoy with no real partition beside it`: you are not in amonet's TWRP, or the TWRP differs from the expected one. Stop.
- `recovery backup is not an Android boot image` or `is a TECHO5 Linux boot image, not the unit's TWRP`: the file in `recovery.img` is not TWRP. The way back would be missing. Stop.
- Image size line shows more than 16 MiB (`... (xx% of 16777216)`): stop.
- A manifest signature failure or checksum failure: stop; retry once; if repeated, stop [S: README step 2].

**C4. What the installer's backup does not cover; take it.** The installer saves 13 small partitions, about 94 MB. It does **not** save `system_a`, `system_b`, `cache`, `userdata` [S: install-dot.py `PARTS`; T5D hardware.md "Not yet backed up: system_a/b, cache, userdata" on its bench unit], nor `mmcblk0boot1`, nor RPMB. Take a raw copy of the whole disk. `cmd /c` keeps the bytes unchanged; PowerShell redirection of binary output is not safe.

```powershell
adb shell "ls -l /dev/block/mmcblk0*; cat /sys/block/mmcblk0/size; ls /dev/block/mmcblk0boot1 2>&1"
cmd /c "adb -s <serial> exec-out dd if=/dev/block/mmcblk0 bs=1048576 > E:\b0\backups\<serial>\emmc-full.img"
cmd /c "adb -s <serial> exec-out dd if=/dev/block/mmcblk0boot0 bs=1048576 > E:\b0\backups\<serial>\mmcblk0boot0.img"
cmd /c "adb -s <serial> exec-out dd if=/dev/block/mmcblk0boot1 bs=1048576 > E:\b0\backups\<serial>\mmcblk0boot1.img"   # skip if absent
```

Expected: `emmc-full.img` is exactly 3,817,472 KiB = 3,908,067,328 bytes for the bench-unit size; it is the disk size from `/sys/block/mmcblk0/size` times 512 on yours. T5 reports about 3.9 GB and about 30 minutes over soldered wires [S: T5D README "Back up"]; micro-USB should be faster, **[U]** by how much. Stop if the file is shorter than the disk or the Dot drops off USB (cable).

Note `mmcblk0boot0` is the preloader area that the installer also saves as `preloader.img`; the second copy is a cross-check. `emmc-full.img` holds `persist` and the Amazon identity (the serial, device keys): treat it as **secret**.

**C5. TWRP backup as asked for (boot, recovery, system, data).** The raw image in C4 is the authoritative backup; a TWRP backup is a second, differently made copy and is handy to restore single partitions from the TWRP menu on a headless device. On this A/B, headless TWRP the partition letters and slot handling are **[U]**; read the on-device help first and do not guess.

```powershell
adb shell twrp             # prints usage, if the build has one; [U]
adb shell twrp backup SDBR b0-pre-techo5      # S system, D data, B boot, R recovery; [U] flags for TWRP 3.7.0_9-bboe2
adb pull /sdcard/TWRP/BACKUPS E:\b0\backups\<serial>\twrp
```

`/sdcard` is on the Dot's own `userdata` (1.2 GB; about 10% used per R-07), so a TWRP backup of `system` and `data` may not fit. If TWRP reports no space, **skip C5**; the raw image covers it [I]. Delete `/sdcard/TWRP/BACKUPS/b0-pre-techo5` from the Dot afterwards (it sits on the partition the install writes to).

**C6. Verify every backup.** Three checks, none optional.

1. **Device md5 equals file md5** for each partition: the installer already did this (`md5 ok`). For the extras:

   ```powershell
   adb shell "md5sum /dev/block/mmcblk0boot0"        # compare with Get-FileHash below
   adb shell "dd if=/dev/block/mmcblk0 bs=1048576 count=64 2>/dev/null | md5sum"
   Get-FileHash -Algorithm MD5 E:\b0\backups\<serial>\mmcblk0boot0.img
   ```

   For the full disk, hash on the device and on the PC: `adb shell "md5sum /dev/block/mmcblk0"` (several minutes) against `Get-FileHash -Algorithm MD5 emmc-full.img`. They must match exactly.
2. **Content spot checks:** `recovery.img` and `boot_a.img`/`boot_b.img` start with the bytes `ANDROID!`; sizes are 16,777,216; `recovery.img` is *not* a TECHO5 image (the installer checks) [S: install-dot.py `head_is_android`, `is_techo5_image`].
3. **Cross-check the two backups agree:** the 16 MiB at the offset of `recovery` inside `emmc-full.img` has the same SHA-256 as `recovery.img`. Take the offset from `/sys/block/mmcblk0/mmcblk0p12/start` (sectors of 512 bytes) in TWRP [I].

Then `Get-FileHash -Algorithm SHA256` over every file into `SHA256SUMS.txt`. Copy the folder to the second storage place and verify the hashes there.

**C7. Where to store them.** Two copies, one off the PC, both encrypted. Keep `home-assistant.key`, `emmc-full.img`, `persist.img` and `mmcblk0boot*.img` out of cloud sync and out of any repository (check `.gitignore` of any repo you work in). Record only sizes and hashes in the report, never contents.

**C8. Restore rehearsal, no writes (do now, before D).** Proves the PC can talk to the recovery path:
- Power-cycle the Dot (unplug, wait 10 s) holding **Vol-Down** while plugging in: the ring shows amonet fastboot (rainbow/multicolour per #1, #19, T5 radar notes). On the PC: `fastboot devices` must list the Dot. Then `fastboot reboot` and let Fire OS or TWRP start normally. Do not type any `fastboot flash` or `erase`. [S: T5D hardware.md "Keys at power-on"; T5 getting-started "From amonet's fastboot mode"]
- Boot TWRP once with **Vol-Up** held at power-on: white ring [S: T5 getting-started step 1]. If either entry fails now, the rollback is unproven: **do not install**.

## 6. Phase D: pre-install gate (no device change)

**D1. Review the dry-run artefacts** in `E:\b0\build` and `backups\<serial>`: the boot image size, `SHA256SUMS`, the release version, the manifest signature line.

**D2. Fill in section 12 (go/no-go).** Gate 1 must be fully signed before step E1.

**D3. Arrange the console.** In Windows Device Manager note that a **USB Serial Device** will appear after the first Linux boot; if it shows as an ADB interface instead, bind it to the Windows serial driver (README "Echo 2nd gen" note on `18d1:4ee7`) [S: T5D README]. Start the terminal after the install, not before.

## 7. Phase E: install (WRITES TO DEVICE)

All of E happens in one installer run. The installer has no pause between its sub-steps, so the maintainer's go in section 12 (Gate 1) covers E1. Do not walk away. Keep the cable still. Do not touch the Dot's buttons during the install. Do not close the terminal.

### E1. The install command. **WRITES TO DEVICE** (sub-steps W-a to W-f below)

```powershell
cd E:\b0\techo5-dot
python tools/install-dot.py --serial <serial> --name "ClawDot Dot" --wake-words okay_nabu,hey_jarvis,hey_mycroft --backups E:\b0\backups --work E:\b0\build
```

Same options as the dry run, minus `--dry-run`. Run it from the TWRP session of phase C, so the backups and the write describe the same state.

| Sub-step | What it writes | Expected output line | Stop if |
|---|---|---|---|
| W-a boot image to `recovery` | `dd` of the 13 MB image to p12, then reads back and compares md5 | `written and read back, md5 <32 hex>` | `partition reads back <x>, wanted <y>`. The TWRP backup is at `recovery.img`: go to R-B at once. The unit still has Fire OS; the old TWRP is gone from p12 until restored [S: install-dot.py step 5] |
| W-b root filesystem to the cache store | pushes ~37 MB tar (49 MB unpacked), unpacks into `/cache/techo5/slots/a`, marks it `good`, sets `active` | `slot a: techo5-dot v1.0.60 (<sha>), 83 commands in /bin` | `installing the slot failed`. "Nothing else has changed yet; run again" [S: install-dot.py step 6]. Rerun once. A second failure: go to R-A or R-B because recovery already holds Linux |
| W-c TWRP copy kept on the unit | pushes `recovery.img` to `/cache/techo5/twrp.img`, md5 checked; pushes `back-to-linux.sh` | `TWRP copy kept on the unit (to-twrp puts it back; /cache/techo5/back-to-linux.sh returns)` | `the TWRP copy on the unit does not match` |
| W-d provisioning | `/data/misc/echolocal/name`, `psk`, `models/*`, optional `ssh/authorized_keys`, `/data/techo5-linux/` dirs | `== provisioning 'ClawDot Dot'` and `new key written to ...\home-assistant.key` | any `adb push` failure after the retries |
| W-e arming | writes `TECHO5-TRIES 0 installed` at `misc` sector 14, checks it | no line on success | `the boot count in misc could not be reset` |
| W-f reboot | `adb reboot recovery`, which now boots Linux | `waiting for the unit's Linux console on USB`, then `slot a, techo5-dot v1.0.60 (...), address <ip>, echod running` | see E2 |

The installer needs about 8 minutes at most for the console wait; the first boot takes about 45 s from reboot to the API [S: T5D README "Boot time"; install-dot.py `8 * 60`].

### E2. First boot. Expected and stop conditions

Expected final lines:

```
Home Assistant will discover '<name>' as an ESPHome device. When it asks for the encryption key, paste:
    <key>        (also saved in ...\backups\<serial>\home-assistant.key)
Done: <serial> is running TECHO5 Linux, healthy, and Home Assistant's API port answers at <ip>:6053.
```

[S: install-dot.py ending]. The ring pulses cyan while it waits for Home Assistant and stops after five minutes with none [S: release notes dot-v1.0.60].

Stop and go to section 11 if:
- `is in rescue: no slot booted, and the boot count has reached five`: the unit is in the rescue environment. It is **not bricked**: USB console and SSH-by-key exist. Try `techo5-retry` once on the console; read the crumbs with `tools/linux/readmisc.sh` through adb (needs the unit in TWRP or Fire OS, so it is the R-A path) [S: install-dot.py; tools/linux/init].
- `Rebooted, but the unit was not confirmed up`. Wait 3 minutes, power-cycle once, check the router for the Dot's address. If the ring spins blue and never turns teal, it matches #19 (the unit got an address and answered ping but not :6053) and #8 (an old Home Assistant was the cause of the spinning turquoise, not the Dot) [S: #19, #8]. Collect the console log (E3), then decide.
- `This PC never saw the unit's USB console` is **not** a failure by itself [S: install-dot.py]; check the router and the console again (Device Manager).
- Anything that boots **Fire OS or the Alexa setup** instead of Linux. Cause seen in the wild: a power cut leaving the bootloader message in `misc` empty (#37). Fixed in the boot image of `dot-v1.0.59` and later; the trial installs `dot-v1.0.60`, so it should not recur, but if it does: `adb reboot recovery` from Fire OS returns to Linux with all settings [S: #37].

### E3. Open the console and collect first evidence. READ-ONLY (a root shell on the Dot)

```powershell
# Windows; pick the COM port that Device Manager shows for "USB Serial Device"
plink -serial COM7 -sercfg 115200,8,n,1,N
```

Press Enter. A `#` prompt is root [S: boot.sh; inittab]. Run:

```sh
cat /etc/techo5-release ; cat /run/techo5/slot ; slotctl status
dmesg | tail -50 | grep -v 'wlan\]' | tail -30
cat /data/techo5-linux/echod.log | tail -40       # may hold the device name, not secrets; do not paste the key
```

Do not run `echod --version` by hand: it starts a second daemon [S: T5D hardware.md]. Do not read `/data/misc/echolocal/psk` out loud into any log.

## 8. Phase F: measurements (READ-ONLY on a running Linux)

Everything here runs in the console shell of E3. Record the output (strip addresses, names) into the report. Take each measurement **idle for at least 5 minutes after the boot-health line**, with no Home Assistant and no ClawDot, then again with ClawDot connected (section 9). `top`, `free` and `ps` are BusyBox applets; if one is missing, say so in the report **[U]**.

| # | Measure | Command in the console | Compare with |
|---|---|---|---|
| F1 | `echod` RSS and peak | `grep -E 'VmRSS|VmHWM|Threads' /proc/$(pidof echod)/status` | T5D bench: `echod` is an 18.7 MB binary [S: R-08 section 7.3]; no RSS figure published |
| F2 | Whole-system memory | `free -m ; grep -E 'MemTotal|MemAvailable|MemFree|Cached' /proc/meminfo` | T5D claim "about 445 MB of 481 MB free with everything running, Bluetooth included" [S: T5D README]; Fire OS baseline ≈ 290-300 MB available (R-07) |
| F3 | Per-process RSS ranking | `ps -o pid,rss,args | sort -n -k2 -r | head -15` | |
| F4 | CPU idle, 3 samples of 10 s | `top -b -n 2 -d 10 | head -20` (read the second sample) | T5D: wake word about 30% of one core (microWakeWord); #4: 36% busy for microWakeWord and 78% for openWakeWord on one Dot; setup.md: "about a tenth" vs "three quarters of a core" [S: T5D README; #4; T5 setup.md "7. Voice"]. Spike gate: SPIKE-002 asks ≤ 15% idle CPU as share of a core, so this will land in "Investigate" (R-08 section 9.1 item 7) |
| F5 | Boot time | On the PC: `$t=Get-Date; adb reboot` (or `reboot` on the console), then `while(-not (Test-NetConnection <ip> -Port 6053 -InformationLevel Quiet)){Start-Sleep 1}; (Get-Date)-$t` | T5D: 45.5 to 46.4 s reboot to API (three runs) [S: T5D README]. Also read the kernel clock at ready: `dmesg | grep 'techo5-dot boot: ready'` |
| F6 | Free flash | `df -h / /store /data` ; `du -sh /store/techo5/slots/a` | Rootfs 49 MB [S: T5D README]; `/cache` 784 MiB, `/data` 1.2 GiB. ClawDot budget 16 MB (REQ-NFR-106) |
| F7 | Wi-Fi join method | `sed -n 1,25p /usr/local/sbin/techo5-net` (the comments) and `ls -l /data/techo5-linux/ | grep -E 'wifi|wpa'`; `wpa_cli -p /run/wpa status | grep -E 'wpa_state|freq'` | Join order: network set with `wifi-set`, else the one Fire OS saved at `/data/misc/wifi/wpa_supplicant.conf`, copied once into `/data/techo5-linux/wpa_supplicant.conf` [S: techo5-net comments]. WPA3/SAE is **not** available [S: T5D hardware.md]. Never print the conf file |
| F8 | Clock and NTP | `date ; ls /run/techo5/ntp-set ; dmesg | grep 'techo5-dot boot: clock' ; cat /tmp/ntpd.log` | `ntpd` against `time.cloudflare.com`, `time.google.com`, `0.pool.ntp.org`, `1.pool.ntp.org`, the gateway, and an optional own server from `/data/misc/echolocal/ntp-server`; RTC written after sync; marker `/run/techo5/ntp-set` [S: boot.sh]. Compare with R-07 section 7 (stale plausible clock on Fire OS). Check that a reboot starts near real time |
| F9 | SSH access | `ps | grep dropbear` ; try `ssh root@<ip>` | SSH runs only while the daemon's SSH switch is on, keys only, and the switch is in Home Assistant [S: setup.md; boot.sh]. Expect it **off**. State this in the report |
| F10 | Listening ports and firewall | `netstat -ltn ; iptables-legacy -S INPUT ; ip6tables-legacy -S INPUT` | INPUT default DROP; accepts 6053, 5353/udp, 22, 8928, 8181, 1900/udp, loopback, established, DHCP, ping; on Wi-Fi 5000, 6001-6010/udp, 4070 [S: techo5-firewall]. Scan from the PC: `Test-NetConnection <ip> -Port 3335` must fail before section 9 |
| F11 | Remote API inventory | After a key is made on the setup page (needs a button press and the page open), `GET http://<ip>:8181/api/v1/entities` | The ESPHome entity list as JSON with ids, kinds, writable flags. The simplest way to learn the **entity names** for the ring, mute and wake-word selects [S: T5 setup.md "11. The remote API"]. Not available until a key is made (setup page) [U] whether that is possible with no Home Assistant, see #40 |
| F12 | Link stability over a group-key rekey | Leave it 90 minutes; ping the Dot from another LAN host every 30 s | #3: unicast to the gateway worked while LAN hosts lost the Dot after a WPA2 group-key rekey (about 13 minutes at the router's 3600 s setting?). Closed with a planned `echod` recovery; whether shipped in `dot-v1.0.60` is **[U]** [S: #3] |
| F13 | Reset reason and crash records | `cat /run/techo5/boot-reason ; ls /data/techo5-linux/crash` | |

Also check, once, that **nothing else of Amazon's is running** (no Android process): `ps | grep -i -E 'zygote|system_server|puffin' ` should print nothing [S: T5D README "Fire OS is gone at runtime"].

## 9. Phase G: ClawDot placement and the open port (WRITES TO DEVICE, small and reversible)

Each G step writes only under `/data/clawdot` or to RAM (iptables). None touches a partition node. Maintainer's go for each (section 12, Gate 2).

**G1. Where the binary goes.** A static `armv7-unknown-linux-musleabihf` ClawDot build runs on the Alpine musl userland, and a musl binary has working DNS (bionic ones do not) [S: R-08 section 5; T5D hardware.md "Other things emOS learned"]. Put it on **userdata** so an update cannot remove it: `/data/clawdot/clawdot` (+ `config`, `state`). An update replaces the *rootfs slot*, never `/data` [S: T5 setup.md "Running something of your own at boot"]. Keep the Fire OS-era copy in `/data/local/tmp` untouched as the rollback copy. **WRITES TO DEVICE** (userdata):

```powershell
adb -s <serial> push target\armv7-unknown-linux-musleabihf\release\clawdot /data/clawdot/clawdot   # only while the unit is in TWRP or has adb
```

Linux has **no `adb`** (the USB function is the serial console, not adb) [S: boot.sh gadget `acm`]. So either push while the unit is in TWRP (`/data` is mounted there and stays), or over the network if SSH is on (not available without Home Assistant), or copy through the console with a small `base64` paste (slow, fine for a 5 MB binary) **[U]**. Simplest: push in TWRP, then boot Linux.

**G2. Start it by hand first.** From the console:

```sh
chmod 755 /data/clawdot/clawdot
/data/clawdot/clawdot --listen 0.0.0.0:3335 >> /data/clawdot/run.log 2>&1 &
ps | grep clawdot ; grep -E 'VmRSS|VmHWM' /proc/$(pidof clawdot)/status
```

(Use ClawDot's real flags; `--listen` and the 3335 default are in `crates/core/src/main.rs`.) Take the same F1 to F4 measurements again with ClawDot idle and connected, and write the deltas.

**G3. Start at boot.** The documented hook does not exist on the Dot: `rc.local` is run on Show builds only and the Dot, Echo and Spot builds "do not run it yet" [S: T5 setup.md "Running something of your own at boot"; boot.sh has no such block]. The init is BusyBox `init` with `/etc/inittab` in the **slot**. The only precedent is a user's respawn line, added to `/etc/inittab` in both slots because an update replaces it [S: #3, thtro]:

```
::respawn:/bin/sh -c '[ -f /data/clawdot/run.sh ] && exec /bin/sh /data/clawdot/run.sh || exec sleep 3600'
```

`/data/clawdot/run.sh` must run in the foreground (a respawn that forks looks like an exit) [S: inittab comment], wait for `/run/techo5/ready`, open the port (G4), then `exec /data/clawdot/clawdot ...`. After any TECHO5 update, the line is gone from the new slot and must be added again to **both** slots (`slotctl status` shows which is active) **[S/I]**. Upstream request for the Dot (the `rc.local` hook) is the clean fix; record it as "upstream request" per roadmap B4. **WRITES TO DEVICE** (the slot's `/etc/inittab`, in `/cache`).

**G4. Open the port.** The firewall script rebuilds `INPUT` from scratch at every run (boot and rescue) and has no include [S: techo5-firewall: `iptables-legacy -F INPUT`]. A rule must be added **after** it, by your own script, for both address families, and survives only until the next firewall run:

```sh
iptables-legacy  -I INPUT 2 -p tcp --dport 3335 -j ACCEPT
ip6tables-legacy -I INPUT 2 -p tcp --dport 3335 -j ACCEPT
```

[I: syntax from the script; position 2 keeps the loopback rule first]. Restrict the source to the LAN subnet if you can: `-s 192.168.0.0/24` (your subnet) because ClawDot's HTTP is plain and the satellite path carries a token (REQ-CFG-005, SECURITY.md). Verify with `iptables-legacy -S INPUT` and `Test-NetConnection <ip> -Port 3335`. The daemon opens its own ports itself too (`firewall.Open(firewall.API, ...)`) [S: api.go]; nothing there touches 3335.

**G5. ClawDot to `echod` on the same box.** Dial `127.0.0.1:6053`: loopback is accepted by the firewall [S: techo5-firewall]. `echod` serves **Noise only** once a key is installed, so the client needs the 32-byte base64 key from `home-assistant.key` and the `Noise_NNpsk0_25519_ChaChaPoly_SHA256` initiator (roadmap B2) [S: api.go `loadPSK`; plainhint.go; R-08 section 4.2]. Until B2 exists, G5 cannot be completed; B0 therefore ends at "port open, binary runs, measurements done" plus the questions that can be answered from the wire (section 10 test 1).

## 10. Questions to answer for ClawDot

For each: what the sources say now, how to answer it on the unit, and what each answer means. Record answers with the tags above.

| # | Question | What the sources say now | How to answer | If the answer is bad |
|---|---|---|---|---|
| Q1 | Does `echod` accept a **non-Home-Assistant** ESPHome client for **voice subscribe**? | The server accepts any connection that completes Noise with the device key (no HA identity check). `Voice.Handle` accepts `SubscribeVoiceAssistantRequest` from any connection and "a subscription on a new connection takes over from the one held" [S: T5 `echod/internal/feature/voice/voice.go` Handle; `api.go`]. The voice satellite object is `github.com/ygelfand/go-esphome-device` `VoiceSatellite`, whose rules were not read **[U]**. A turn needs `vs.Subscribed()` or a direct brain [S: voice/backend.go `ha.Ready`, `Voice.Ready`] | Needs a client. Cheapest, on a PC: `aioesphomeapi` (MIT) script that connects with the key, subscribes with `API_AUDIO`, says "okay nabu" near the Dot, and prints the `VoiceAssistantRequest` and audio frames. Also run it with HA absent | Fall back per R-08 section 9.2: EchoMuse controller for early tests, "replace `echod`" long term |
| Q2 | How many **simultaneous clients**? Voice subscribers? | One voice subscription at a time, last subscriber wins (above) [S]. Open connections are counted but no maximum was found [S: api/clients.go]. Entity subscriptions and `Broadcast` to all clients exist [S: api.go `call`, `FireEvent`]. A second client's cap is **[U]** | Open 2, then 5 plain `aioesphomeapi` clients; subscribe voice from two and see who keeps the audio; with Home Assistant also connected, see whether adding ClawDot steals the pipeline from HA | If HA and ClawDot cannot share, ClawDot replaces HA as *the* client, which matches the standalone goal; state it |
| Q3 | **Announce** (`VoiceAssistantAnnounceRequest`) and **media_player** supported? | `Features` include `FeatureAnnounce`, `FeatureStartConversation`, `FeatureSpeaker`, `FeatureTimers` [S: voice.go]. `announce()` plays `preannounce_media_id` then `media_id`, sends `AnnounceFinished`, and with `start_conversation` starts a turn with no wake word [S: voice/announce.go]. It fetches a URL: the core must serve it over HTTP the Dot can reach. The media player entity is registered (README "a Home Assistant media player") [S]; the `MediaPlayerCommandRequest` handling was not read **[U]** | From the same PC client: send `VoiceAssistantAnnounceRequest` with a URL to a short mp3/wav served by ClawDot or a Python http.server; expect audio, then `AnnounceFinished`. Then `start_conversation: true` and speak | If announce works but media_player does not, the inbox flow still works (announce-then-listen) |
| Q4 | How are **LED ring, mute and buttons** exposed? | Ring: a light entity on a "ring" sub-device with effects, and per-turn effect selects ("Wake word effect", "Thinking effect", "Replying effect"); configuration category [S: T5 setup.md "7. Voice"; api.go `subDevices`; README]. Mute: an entity called "Microphone mute" and the physical mute button drives the hardware line; echod stops wake models when muted [S: T5 setup.md ~line 1929; #4]. Buttons: the action button's long hold arrives as a `long_hold` button event; volume buttons act locally [S: porting-plan.md; setup.md]. R-08: "mute is software on the Dot 2" vs T5D's "hardware mute line" is unresolved; ClawDot's declaration stays `software` (REQ-BRD-008) [S: R-08 section 3.3] | `ListEntitiesRequest` from the PC client; or `GET /api/v1/entities` on port 8181 after a remote-API key (F11) | Anything missing becomes a documented gap or upstream request, not ClawDot code (roadmap B4) |
| Q5 | **Wake word** options and licences | microWakeWord models "Okay Nabu", "Hey Jarvis", "Hey Mycroft", "Alexa" from `esphome/micro-wake-word-models` (Apache-2.0 incl. weights); eight more from `fwartner/home-assistant-wakewords-collection`, a repository under MIT that "records no separate licence, author or training data for each model" [S: R-08 section 6; install-dot.py]. Custom `.tflite` + `.json` model pairs are accepted (microWakeWord, e.g. from `TaterTotterson/microWakeWords`, or trained with `wakesmith`) [S: T5 setup.md "A wake word of your own"; docs/training-wake-word.md]. openWakeWord models run but cost about 7 times the CPU, 78% busy vs 36% [S: setup.md; #4]. openWakeWord pretrained weights are CC BY-NC-SA: never bundle them [S: R-08 section 6]. Wake word selection is made through `VoiceAssistantSetConfiguration` and the Assist satellite select [U: whether a non-HA client may set it] | Read `GetConfiguration` result in the client; try `SetConfiguration` | ClawDot's licence rule (REQ-VOI-009) allows the Apache-2.0 four (avoid "alexa" on trademark grounds, R-08) and any model we train with recorded provenance; the eight community ones fail the provenance rule |
| Q6 | **Bluetooth speaker mode**: how it works, and can it stay on beside ClawDot? | A2DP sink (phone plays through the Dot) and source (Dot plays to a speaker), over BlueZ + bluealsa on a rebuilt Bluetooth kernel with 48 backported fixes; verified with a phone on the bench unit [S: T5D README, porting-plan.md "M5 Bluetooth"]. Pairing window: hold the action button 5 s (ring pulses blue), or the **Bluetooth phone** switch in Home Assistant, two minutes, then the phone is remembered; **Bluetooth pairing cannot be changed through the remote API** [S: T5 setup.md]. Playback arbitration: "whichever of the phone and Home Assistant media started last plays"; the wake word ducks or pauses the phone's music; pausing the phone releases the speaker after 5 s [S: porting-plan.md]. Cautions: turn the Bluetooth **proxy** off on a device a phone plays to; Wi-Fi and Bluetooth share one antenna [S: setup.md; R-08 section 3.4]. Whether a *ClawDot announcement* ducks or interrupts a phone stream is **[U]**; so is whether the sink stays reachable with no HA | Pair a phone with a 5 s hold; play music; while it plays, send Q3's announce and a voice turn; note ducking, interruption, resume, Wi-Fi stability (ping), and the CPU (T5D: bluealsa about 2% CPU) | Keep Bluetooth speaker mode as a supported base capability only if announcements are heard over music and the voice path survives; record the exact behaviour in the B4 guide |
| Q7 | Is there a **non-HA brain** inside `echod`? (new finding) | Yes: the "direct" pipeline answers a turn with **no Home Assistant**: speech to a Wyoming STT server (faster-whisper), the words to a chat model behind an OpenAI-style endpoint, the answer to a Wyoming TTS server (Piper), played on the speaker. It needs all three addresses and is set on the setup page or in state [S: T5 `echod/internal/config/brain.go`; voice/direct.go; setup.md "Voice without Home Assistant"]. It cannot control the house (only the device's timers, alarms, radio and so on) | The setup page is needed to set it (button press, port 8181; #40 says non-HA users cannot reach it, with port 80 tried, which may be the wrong port **[U]**). Treat as an interim route to a voice turn that does not need the B2 client | If the setup page cannot be reached with no HA, the direct brain is not an option for ClawDot until upstream fixes #40 |
| Q8 | Who owns the clock, Wi-Fi, firewall? | The base; ClawDot is a guest on `/data` (F7, F8, F10) | | |

**Test 1 (on the wire, no ClawDot code).** `pip install aioesphomeapi` in a venv on the PC; a short script: connect to `<ip>:6053` with the key, `device_info()`, `list_entities_services()`, `subscribe_voice_assistant(...)`. Do not send the key anywhere else; keep the script out of the repository. This answers Q1 to Q5 **before** ClawDot has a client. Mark answers [S: observed, date, release].

## 11. Rollback and recovery

Tested **on paper** against TECHO5's own documented rescue mode and issues #1, #19, #37: each step below is taken from their scripts or comments, not run. Rehearsal C8 proves only that the PC can reach amonet's fastboot and TWRP; no restore has been executed. Every rollback step that writes needs the maintainer's go (Gate 3).

**What you need at the bench:** the whole `E:\b0\backups\<serial>\` folder (and its off-PC copy); `recovery.img`, `misc.img`, `boot_a.img`, `boot_b.img` from it; platform-tools; the printed key combinations; the Linux live USB; section 2.1's hardware list.

### R-A. Linux is running or reachable: back to TWRP, then to rooted Fire OS. **WRITES TO DEVICE**

From the USB console (or SSH if on):

```sh
to-twrp           # prints what it will do; nothing changes
to-twrp --yes     # writes the unit's own TWRP copy to recovery, checks md5, sets boot-recovery, reboots to TWRP
```

[S: tools/linux/rootfs/usr/local/sbin/to-twrp]. A failed readback puts `linux.img` back automatically. In TWRP (`adb devices` shows `recovery`), clear the "boot Linux next" request the same way TECHO5's own `to-android` does, then boot Fire OS:

```powershell
adb shell "dd if=/dev/zero of=/dev/block/platform/bootdevice/by-name/misc bs=32 count=1 conv=notrunc; sync"
adb reboot
```

[S: tools/linux/init `to-android` clears the first 32 bytes of misc with `dd ... bs=32 count=1 conv=notrunc`]. Expected: Fire OS (rooted, root adb) boots from the untouched boot slot with `/data` intact, ClawDot in `/data/local/tmp` as before. To start Linux again from TWRP: `adb shell sh /cache/techo5/back-to-linux.sh` [S: back-to-linux.sh]. **Leftovers** from the trial (harmless, remove only if you want a clean unit): `/cache/techo5`, `/data/techo5-linux`, `/data/misc/echolocal`, `/data/clawdot`. The Wi-Fi settings of Fire OS were only read [S: techo5-net]. Stop if `adb reboot` does not bring Fire OS within 3 minutes: go to R-B.

### R-B. Linux will not boot or has no console: fastboot from the PC. **WRITES TO DEVICE**

1. Unplug the Dot. Hold **Vol-Down** and plug it in: amonet's fastboot (multicolour ring). `fastboot devices` must list it. [S: T5D hardware.md; T5 getting-started "From amonet's fastboot mode"]. If instead Linux starts (Vol-Up or no key), you are in TECHO5's rescue or normal boot; use R-A.
2. From the backups:

   ```powershell
   fastboot -s <serial> flash recovery E:\b0\backups\<serial>\recovery.img
   fastboot -s <serial> erase misc
   fastboot -s <serial> reboot
   ```

   `erase misc`, not flashing `misc.img`: the maintainer of TECHO5 corrected the first advice because the saved `misc.img` can carry a stale "boot recovery" request (#1). Keep `misc.img` as an archive. Then Vol-Up at power-on gives TWRP; no key gives Fire OS [S: #1; T5 getting-started].
3. If it still goes straight to fastboot, the boot image is the suspect (#1 theory by the maintainer, unconfirmed): `fastboot flash boot_a ...\boot_a.img`, `fastboot flash boot_b ...\boot_b.img`, `fastboot flash recovery ...`, `fastboot erase misc`, `fastboot reboot` [S: #1]. The partition names `boot_a`, `boot_b` match the v2.0.0 layout read in B2 **[U]**; check with `fastboot getvar all` first (read-only).
4. kaeru refuses `lk`, preloader and `tee` flashes unless `flashing unlock_critical` is run [S: T5D hardware.md]. **Never** run `unlock_critical`, never flash anything but the four items above. Never try to go back from amonet v2.0.0 by flashing Fire OS 5 or an older amonet: that hand-rewrites bootloaders and is how units hard-brick [S: T5D hardware.md; R-08 section 3.8].

### R-C. Back to exactly the pre-trial state, or to stock

- **Exact pre-trial state:** from TWRP, write the raw images of `cache` and `userdata` (and `recovery`, `misc`) from `emmc-full.img`. Offsets from the table in `/proc/partitions` at backup time. Slow, and only needed if `/data` or `/cache` was damaged. A full-disk write is **not** advised: it would rewrite `preloader`/`lk`/`tee` and is the unbrick route of last resort [I].
- **Stock Fire OS:** `uvx --managed-python firebreak@latest stock` installs build 8149 and **erases the whole Dot, including `/data` and ClawDot** [S: FB README usage]. `firebreak builds` lists builds; the unit's own 8349 can be restored if listed (`stock 8349`) [S: FB docs/unlocking.md "stock takes every OTA ftvdb lists", 42 biscuit builds verified]. Then re-run the unlock (`firebreak unlock`) to be rooted again. This is a deliberate change of state, not a rollback: ask first.

### R-D. Worst case: no light, no fastboot, no adb (the unbrick). **WRITES TO DEVICE**

Meaning: neither Vol-Up, Vol-Down nor plain power gives any ring colour, and the PC sees no `18d1` / `0e8d` device. Causes in the sources: a failed or interrupted bootloader step, two A/B slots out of boot attempts, an anti-rollback mismatch, interrupting the unlock's grace period [S: T5D hardware.md "Brick recovery"; FB docs/troubleshooting.md; R-09 section 7]. **TECHO5's install does not touch any of those partitions**, so reaching this state from this trial would be a surprise **[I]**; the plan still covers it.

1. **Stop trying to boot it.** The preloader counts boot attempts per slot; when both run out the Dot stops booting [S: R-08 section 3.8, EM docs/rooting.md]. Unplug it.
2. **Switch to the Linux live USB** (not the Windows 11 build 26300 PC; UsbDk is reported broken on 25H2 build 26200 and untested on later ones [S: FB docs/troubleshooting.md "Windows"]).
3. Open the Dot with a Torx T8 and lift the eMMC shield lid by its corners. Find the passive in the row between R60 and C52 (not C52, a capacitor), using the saved photo and Andy Goetz's page [S: FB docs/troubleshooting.md; R-09 section 4].
4. With the Dot unplugged: `uvx firebreak@latest short --device biscuit`. When it says "Short ... test point and plug it in", hold a wire between that passive and the shield frame (ground) while plugging the Dot in; on "The short may come off now", remove the wire; leave the Dot plugged in [S: FB docs/troubleshooting.md].
5. Then run the target you want: `uvx firebreak@latest unlock` (rooted Fire OS 6; leaves the Dot's data) or `... unlock --factory-reset` / `stock` (erases the Dot) [S: FB README usage]. A power cycle loses the hold.
6. Support: `uvx firebreak@latest report` and the 0x7E Labs Discord `#echo-support` [S: FB README "Reporting a problem"]; the XDA thread's unbrick section (not read by research, **[U]**).
7. Afterwards the Dot is a fresh unit: restore ClawDot from the backups' `/data` content and redo B1.

Do not improvise with bootloader images [S: T5 getting-started "If something goes wrong"].

## 12. Go/no-go sheet (the maintainer signs before each write)

Copy this into the report. A write does not start until every box of its gate is ticked and the date and initials are filled in.

**Gate 0. Before the first command that boots the Dot into TWRP (C1)**
- [ ] A2 done: installer at commit `13482c9`, diff since read, no change in the files named.
- [ ] Power, cable, port, sleep settings as in 2.1.
- [ ] Linux live USB booted once and firebreak/uv works from it (or R-D accepted as unavailable).
- [ ] Offline copies of XDA thread text, amonet ZIP, firebreak wheel, TECHO5 docs.
- [ ] B1 to B3 recorded; B2 partition layout matches section 2.2.

**Gate 1. Before E1 (the install: recovery, misc, cache, userdata)**
- [ ] C3 dry run completed with no stop condition; boot image under 16 MiB.
- [ ] C4 full-disk image and boot-area copies complete; C6 hashes equal on device and PC; copy verified on the second storage place.
- [ ] `recovery.img` starts with `ANDROID!` and is not a Linux image.
- [ ] C8 rehearsal done: fastboot and TWRP entries both worked; no flash typed.
- [ ] K1 decision (B5) written down with the Fire OS build of both slots; option A or B chosen on purpose.
- [ ] Maintainer accepts: no prompt in the installer, unit unusable for voice until Home Assistant or a client connects, TWRP gone from p12 (copy kept), the single shell is the USB console.
- [ ] Unit stays on its cable for the whole run; nobody will touch it.
- Go: ______ (initials, date, time)

**Gate 2. Before each step in section 9 (G1 push, G3 inittab, G4 firewall rule)**
- [ ] Phase E finished healthy (`Done:` line or a recorded, understood alternative).
- [ ] Measurements F1 to F10 taken *before* ClawDot was started.
- [ ] The step writes only under `/data/clawdot`, the slot's `/etc/inittab`, or RAM.
- Go per step: G1 ____ G3 ____ G4 ____

**Gate 3. Before each rollback write (R-A, R-B, R-C, R-D)**
- [ ] The backup file for that write exists, hashes match.
- [ ] The cable and port are good; nothing else is plugged in.
- [ ] For R-B: `fastboot devices` lists the unit; the command list is exactly section 11 R-B. No `unlock_critical`, no bootloader images.
- [ ] For R-C/R-D: the maintainer chose it knowing it erases the Dot.
- Go: ______

**Stop rule.** Any gate item that cannot be ticked is a "no". A "no" costs nothing; a write on a "maybe" can cost the unit.

## 13. What to record afterwards in `docs/reports/`

One file: `docs/reports/2026-MM-DD-b0-techo5-trial.md`, named by the day of the install. Sections (the same tags as here):

1. **Header.** Date, release `dot-vX.Y.Z`, installer commit, Fire OS build of both slots, amonet version, the decision at B5 and why.
2. **What was done.** The commands actually typed (no secrets), each with its gate and time; deviations from this plan.
3. **Backups.** File names, sizes, SHA-256 (not contents), where stored, restore rehearsal result.
4. **Install result.** Installer output (redacted), boot-to-API time, anything that went wrong, the stop conditions hit.
5. **Measurements.** F1 to F13 as a table: before (Fire OS), TECHO5 idle, with ClawDot idle, with ClawDot connected, with Bluetooth playing. Include the method, sample sizes, and how many units (one).
6. **Answers** to Q1 to Q8, each tagged. Include the exact `aioesphomeapi` version and the entity list (names only).
7. **Where ClawDot runs.** Path, start method, firewall rule, what an update does to it.
8. **Rollback.** Whether R-A was executed as a test, its result and time (strongly recommended before leaving the unit on TECHO5: run `to-twrp --yes` and `back-to-linux.sh` once while the maintainer is at the bench).
9. **Findings and requests.** What changes for ADR-0013 ("Dot front end base", Proposed), B1/B2/B4/B5, spec 0002's gates, open-questions rows; upstream issues to raise (Dot `rc.local`, setup page without Home Assistant #40, 6.5.7.5 support).
10. **Confidence** line, as R-07 and R-08 do.

Update [open-questions.md](../open-questions.md) rows "Dot front end (R-08)" and "Dot unlock (R-08)" and the roadmap B0 text from the report, in the same branch.

## 14. Sources

Primary, read on 2026-11-09: `HuskerMinion/techo5-dot` (README, NOTICE, `docs/hardware.md`, `docs/porting-plan.md`, `tools/install-dot.py`, `tools/techo5lib.py`, `tools/dotimage.py`, `tools/linux/init`, `tools/linux/back-to-linux.sh`, `tools/linux/rootfs/etc/inittab`, `etc/techo5/boot.sh`, `usr/local/sbin/{to-twrp,slotctl,techo5-firewall,techo5-net,wifi-set}`, release `dot-v1.0.60`, issues #1, #2, #3, #4, #8, #12, #19, #25, #37, #39, #40); `HuskerMinion/techo5` (`docs/getting-started.md`, `docs/setup.md`, `docs/standalone-plan.md`, `docs/external-asp.md`, `echod/internal/feature/api/{api,clients}.go`, `echod/internal/feature/voice/{voice,backend,announce,direct}.go`, `echod/internal/config/brain.go`, `echod/internal/feature/assistant/assistant.go`); `bboe/firebreak` v1.0.0 (README, `docs/troubleshooting.md`, `docs/unlocking.md`, `docs/devices.md`, CHANGELOG). Not read: the XDA threads (login, HTTP 403 for research), `go-esphome-device`'s voice satellite source, the runtime behaviour of any of it. One unit, one day.
