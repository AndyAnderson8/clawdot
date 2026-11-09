# R-09: How the Echo Dot Gen 2 came to be unlockable, and what firebreak automates

Date: 2026-11-09 · Feeds: ADR-0003 (Dot port), spec 0002 §2, roadmap "Beyond v1.0", the future install guide · Refines: [R-04](04-echo-dot-gen2.md), [R-07](07-echo-dot-gen2-hardware.md)

Scope: history and mechanism, explained for a developer who is not a security researcher. This is **not** an install guide. It contains no exploit code, payload bytes, or memory offsets, and it says nothing about devices other than the maintainer's own already-unlocked Dot 2 (codename `biscuit`, which R-07 inspected).

Evidence tags used below: **[V]** read directly in a primary source (repo, commit, post); **[S]** reported by press or a secondary page; **[I]** my inference from the sources, not stated by them; **[U]** unverified or not retrievable. The XDA threads themselves could not be fetched (HTTP 403), so claims about their contents come from search-result snippets and from pages that quote them, and are tagged accordingly.

## 1. Summary (plain language)

- A locked Echo Dot 2 will only run software signed by Amazon. Each boot stage checks the next one, starting from code burned into the chip, so you cannot just install your own system.
- The chip is a MediaTek MT8163, the same chip used in some Amazon Fire tablets. Hackers had already broken into Fire tablets using a bug in the chip's built-in USB download mode (the "bootrom"). That bug runs before any signature check, so it gives raw read and write access to the device's flash storage.
- On a Dot 2 you could only reach that mode by opening the case and shorting a point on the flash chip while powering up. People did that from 2022 and dumped the firmware (Andy Goetz, then Daniel Benge). This was a **tethered** root: it needed a computer on every boot.
- The step that made it practical was a second flaw, in Amazon's bootloader (LK), that can be triggered from the outside with one `fastboot` command. It was originally a private exploit of the researcher k4y0z for Fire tablets, and Rortiz2 spent months porting it to Echo devices in 2025. With it, a normal USB cable is enough to get code running, and the tool then uses the bootrom path to install a patched bootloader chain. That is a **persistent** unlock: the Dot boots on its own, with an unlocked bootloader, TWRP recovery, and root.
- firebreak (bboe, 2026) is a Python program that drives those published unlocks from one command, keeps track of which state the device is in, and can resume or restore stock firmware.
- It works because this specific chip and these specific older Amazon firmware builds have the bugs. Newer Echo models use different hardware or software, and Amazon can patch the bootloader stage on new firmware, so the "2nd generation" is not magic: it is an accident of silicon and age.
- Recommendation for ClawDot: **do not depend on firebreak.** Treat "device already unlocked and reachable over adb as root" as the entry state of our install guide (it is exactly the state R-07 found), link to the upstream projects, and credit the people in section 9.

## 2. Timeline

All dates are UTC dates taken from commit metadata, post dates, or page bylines. "Repo created" is the GitHub creation date, which can be later than the first commit if history was imported.

| Date | Event | Who | Source / tag |
|---|---|---|---|
| 2019-01-27 | `amonet` is published: a bootrom exploit plus a bootloader exploit for the Fire HD 8 (2018, `karnak`), which uses the MT8163. README links an XDA thread for the Fire HD 8. | xyzz (commit author Ilya Zhuravlev) | github.com/xyzz/amonet, README and first commits **[V]**; that this is the same MT8163 as the Echo Dot: Goetz and Benge both say so **[V]** |
| 2019-02-02 | `chaosmaster/amonet` created: amonet ported to many Amazon tablets (Fire 7 2015, 2017, others). Branches include `mt8163-biscuit`, `mt8163-karnak`, `mt8163-douglas`, `mt8173-suez`. | k4y0z (GitHub `chaosmaster`) | repo and branch list **[V]**; k4y0z = chaosmaster: firebreak README credits "k4y0z" with the link to that account **[V]** |
| 2019-02-21 to 2019-03-01 | Commits on the `mt8163-biscuit` branch: "initial-echo", "working: need to move payload (again) for fastboot flash", "echo-dot sync". The commits display the author name "Joel Stein" while the GitHub account is `chaosmaster`. A final "updates" commit follows on 2021-04-23. | k4y0z | commit list **[V]**. Reading: k4y0z experimented with the Dot 2 already in Feb 2019 and never published a release **[I]**. Whether it ever booted is **[U]** |
| 2022-05-10 | "Unlocking Echo Dot Part 1": picks the Dot 2 because it is a cheap generic-tablet SoC, expects amonet to work, describes the secure boot chain and anti-rollback. | Andy Goetz | andygoetz.org/blog/2022/05/unlocking-echo-dot-v2/ **[V]** (page read via fetch summary) |
| 2022-05-30 | Part 2: opens the case, shorts the flash chip's data bus (passive parts near R60) while powering up to force the bootrom; the amonet bootrom exploit works on the Dot. First partition table seen; amonet stops on a "bad_gpt" error because the layout differs from the Fire tablet's. | Andy Goetz | andygoetz.org/blog/2022/05/echo-dot-v2-dumping-emmc/ **[V]** |
| 2022-06-08 | Part 3: finds the boot UART on an unlabeled test point near the USB port with an oscilloscope, using the amonet dump as a way to keep the UART busy long enough to see the TX line. Logs show A/B slot selection and `lk_b` loading. He cites Clinton Cook's paper on the Echo (1st gen) as the earlier hardware-hacking reference. | Andy Goetz | andygoetz.org/blog/2022/06/echo-dot-v2-finding-the-uart/ **[V]**; the Cook paper itself **[U]** (not read) |
| 2023-03-16 | "Alexa, what is my Wi-Fi password?": uses a modified amonet with a tin-foil short to dump the Dot's flash and read saved Wi-Fi credentials (stored in plain text). Notes he bricked his own device trying to boot modified software. Credits the amonet repo, an "echohacking" wiki, and the iFixit teardown. | Daniel Benge (blog `dragon863.github.io`; GitHub Dragon863) | hugo-4tu.pages.dev/posts/alexa-1 (a mirror of his post) **[S]**; author identity: his later post links back to his own earlier post **[V]** |
| before 2023-07-19 | A user named j10hx40r on XDA had "already rooted the device by patching the little kernel" so the bootloader reports itself unlocked, which makes Amazon's `fos_flags` debug options available (root adb, dm-verity off, SELinux). Benge calls this "the key clue". | j10hx40r | quoted in danieldb.uk/posts/alexa-2/ **[V]**; the XDA post itself **[U]** |
| 2023-07-18 / 07-19 | EchoCLI repo created and "Rooting the Amazon Echo Dot" posted: a **tethered** root. Combines amonet (read/write flash), a patch to the bootloader (turning on the `fos_flags` debug features Amazon's hardware group Lab126 left in), and the kamakiri bootrom exploit to push a modified preloader over USB on every boot. | Daniel Benge | github.com/Dragon863/EchoCLI (created 2023-07-18), danieldb.uk/posts/alexa-2/ (2023-07-19) **[V]** |
| 2023-07-22 | Hackaday covers it ("Root, On An Amazon Echo Dot"); Hackster does too. | Jenny List (Hackaday); Gareth Halfacree (Hackster) | hackaday.com/2023/07/22/root-on-an-amazon-echo-dot **[V]**, Hackster **[S]** |
| 2023-11-03 | EchoGo created: a Go SDK on top of EchoCLI's tethered root (LEDs, microphones, buttons). | Binozo | github.com/Binozo/EchoGo **[V]** |
| 2025-09-21 to 2025-09-29 | R0rt1z2's `mt8163-biscuit` branch of `R0rt1z2/amonet`: new commits for the Dot ("led ring in hacked fastboot mode", "unconditionally enable UART", "prepare for release" on 09-29). The branch history **contains k4y0z's 2019 biscuit commits**, so it is built on that earlier work. | Rortiz2 (R0rt1z2, commit name Roger Ortiz) | `gh api repos/R0rt1z2/amonet/commits?sha=mt8163-biscuit` **[V]**. XDA biscuit thread post date **[U]**; it falls in this window or just after **[I]** |
| 2025-09-30 | "Initial port to Echo Show 5" (checkers), the first sign the work is spreading to the Show line. | Rortiz2 | commit d84e93e **[V]** |
| 2025-10-08 | Rortiz2 announces the Echo Show 5 (1st gen) unlock as "fully software-driven" and "a new exploit based on amonet", thanking @_AntiEngineer and k4y0z. | Rortiz2 | post on X, quoted in a search-result snippet **[S]** (page itself could not be fetched, HTTP 402) |
| 2025-10-14 | AFTVnews and Liliputing report Rortiz2's unlocks for the Show 5, Echo Spot and Echo Dot 2nd gen, with TWRP; bengris32 ships LineageOS 18.1 for the Show 5 and Spot. | Elias Saba (AFTVnews); Brad Linder (Liliputing) | aftvnews.com/?p=45507, liliputing.com **[V]** |
| 2026-01-02 / 2026-03 | "Jailbreaking The Amazon Echo Show" (Hackaday) and "The Amazon Echo Show Obeys Me Now" (Aftermath, about 2026-03-14 via Slashdot) explain the history: Rortiz2 had jailbroken Fire tablets and Fire TV first; a **demo unit** with vulnerable firmware was dumped to port stage one; stage two is k4y0z's private fastboot bug, ported over "months". | Rortiz2 interviewed | aftermath.site/amazon-echo-show-jailbreak-lineageos/ **[V]** (fetch summary); Hackaday **[S]** (search listing only) |
| 2026-02-06 | bengris32 fixes BCB (boot control block) reset on biscuit. 2026-02 to 2026-03: the bootloader payload gains slot switching, 32- and 64-bit kernel boot, BCB parsing and brick-prevention. | bengris32; Rortiz2 | amonet commit log **[V]** |
| 2026-04 to 2026-05 | EchoMuse (wilbowes) publishes a Dot 2 to Home Assistant satellite on a rooted Fire OS 5 Dot (EchoGo replaced by EchoMuse, v2.0, 2026-05-09). | wilbowes | EchoMuse JOURNAL.md changelog **[V]** |
| 2026-06-17 | An Amazon bootloader built on this date appears in the Dot's Fire OS 6 builds 8338 to 8349; a matching prebuilt fastbrick image exists, so the exploit was re-targeted rather than killed (ship date of the builds not established). | Amazon; Rortiz2 | firebreak docs/devices.md, "Builds" table **[V]** |
| 2026-08-28 | `overdub`, bboe's scripts for a rooted Dot, created. Its README says rooting needs firebreak. | bboe | github.com/bboe/overdub **[V]** |
| 2026-09-07 to 2026-09-10 | amonet gains "Preloader USBDL" support and "Final touches before release"; **amonet v2.0.0** (boots Fire OS 6 instead of Fire OS 5) is dated 2026-09-10 by EchoMuse's docs. | Rortiz2 | amonet commits **[V]**; version date: EchoMuse docs/rooting.md **[V]** |
| 2026-09-11 | "Port to radar (Echo 2nd Gen 2017)". A separate donut (Dot 3rd gen) unlock, kamakiri-donut, follows. | Rortiz2 | amonet commit 00f8d71 **[V]**; donut thread existence **[V]** (linked from firebreak README) |
| 2026-10-06 | `bboe/firebreak` created. v0.1.0 on 2026-10-12, 0.9.0 on 10-26, 0.10.0 on 10-31 (radar), 0.11.0 on 11-01, Dot 3rd gen added 11-04/05, **v1.0.0 on 2026-11-06**; 11-07/08: blocks Amazon's OTA hosts and offers every archived stock build. 77 stars at time of reading, BSD-2-Clause. | Bryce Boe (bboe) | `gh api repos/bboe/firebreak` releases and commits **[V]** |

What the timeline does **not** show: who first noticed that a Dot's boot chain was worth attacking (Goetz in 2022 is the earliest public write-up I found, but not necessarily the first person), the exact date of Rortiz2's biscuit XDA post, and anything in private channels.

## 3. What protected the device

From Goetz's Part 1 (**[V]**), R-04, and firebreak's docs:

1. **Secure boot chain.** The chip's mask ROM (the "bootrom") is fixed forever. It checks the **preloader** (first stage, in a special flash area called `boot0`) against a key fused into the chip. The preloader checks the **LK** (Little Kernel, the bootloader that also runs `fastboot`), which checks the Android **kernel**. Everything must be signed by Amazon. A **TrustZone** image (encrypted) also sits in the chain.
2. **Anti-rollback.** The preloader keeps three counters (preloader, TrustZone, LK) in a protected flash area (RPMB). It refuses older signed code. firebreak's docs describe this; a counter higher than the preloader's makes the Dot stop at "version mismatch" (read from code, not seen in practice, **[U]**).
3. **Locked bootloader.** On a locked unit, `fastboot` refuses writes and OEM commands ("restricted on locked hw", quoted in firebreak docs **[V]**).
4. **dm-verity and Fire OS hardening.** The system partition is checked block by block at run time; adb is off; SELinux is on. Amazon's debug switches (`fos_flags`) exist but are only honoured when the bootloader says it is unlocked (Benge **[V]**).
5. **Obscurity.** The retail Echo has no screen, no adb, and a USB port that is power only (R-04), and, unlike old tablets, no key combination to hold the SoC in download mode: Goetz notes Amazon disabled those so the kernel loads faster **[V]**.
6. **Update pressure.** Amazon can push new firmware that raises counters or replaces the bootloader. firebreak's README warns not to put a stock Echo on Wi-Fi before unlocking **[V]**.

## 4. The weaknesses, in plain terms

Think of the boot chain as three locked doors in a row, with a master key hidden under the mat at the very first one.

**Door 1: the bootrom's USB download mode (the "hardware" door).** MediaTek chips have a small program in ROM that can accept code over USB if the preloader is missing or broken. Researchers (xyzz, then others) found that this ROM code has a bug in how it handles USB messages, so an attacker can run their own code **before any signature is checked** and then read or write the whole flash. amonet's bootrom stage is that exploit for the MT8163 **[V]** (Goetz, Benge). The catch is *getting into* that mode:
- Shorting the flash chip's data lines while powering up makes the preloader fail to load, so the chip falls back to the bootrom (Goetz Part 2 **[V]**).
- Or, if the flash area `boot0` has been erased, the next power-up falls through to the bootrom with no soldering (firebreak docs **[V]**). This is what firebreak does on purpose, and why a failure in that window needs the case opened (section 7).
- On the Dot 2 the bootrom USB ID is `0e8d:0003` and the normal preloader is `0e8d:2000` (Goetz, firebreak) **[V]**.

**Door 2: the bootloader's fastboot handler (the "software" door).** A second flaw lets code run from a plain USB `fastboot` command on a *locked, unmodified* Dot. The sources say, in prose: it is "a buffer overflow that gives you arbitrary code execution after issuing a single fastboot command" (Rortiz2, quoted by Aftermath **[S]**), it began as k4y0z's private exploit for Fire tablets, and Rortiz2 ported it over months (Aftermath **[S]**). firebreak's reverse-engineering notes add that the prebuilt "fastbrick" images are tied to a specific bootloader build, that the payload checks it is on the right Echo model before doing anything, and that Amazon's LK source, not each build, determines where the flaw sits **[V]**. I do not reproduce those details.

**Door 3: persistence (the "make it stick" step).** Running code once is not an unlock. The unlock tools use door 1 or 2 to write a patched set of boot stages into flash: a patched LK that reports "unlocked" and allows `fastboot flash` and TWRP, a preloader and TrustZone image that still let the chain boot, a zeroed anti-rollback counter area, and a recovery (TWRP). After that the Dot boots by itself and the weakness no longer needs to be exploited (firebreak docs, XDA thread via snippets **[V]/[S]**). This is the "untethered" improvement over the 2023 tethered root.

Why Amazon left the door open is not something the sources establish. The ROM bug is in the chip vendor's immutable code, and the fastboot bug is in Amazon's LK (a bootloader family that, per the k4y0z story, also ships on Fire tablets **[I]**). A bug in mask ROM cannot be fixed by a software update at all; the mitigations open to Amazon are to patch the LK stage and, on some later products, to fuse off bootrom USB download (firebreak's docs mention a "BROM USBDL" bit and say some Echo models are *reported* to have no bootrom USB download at all, **[U]**).

## 5. How researchers probably found it (from the sources only)

None of the sources is a discovery diary for the persistent unlock, so this section is a reconstruction. What the sources do record:

1. **Same chip as something already broken.** Goetz chose the Dot 2 *because* the MT8163 is a common tablet SoC with existing exploits (his Part 1 and 2). Benge says the same: the Dot has the same MT8163 as the 2018 Fire HD 8, so amonet could be ported. Rortiz2 says the devices he hacked share MediaTek SoCs (Aftermath **[S]**). This was the single biggest clue, and it needed no new vulnerability research.
2. **Teardown and a physical short.** iFixit teardown, a Torx T8 screwdriver, a wire or tin foil across a passive component near the flash chip (Goetz Part 2, Benge, firebreak's own short procedure) **[V]**. The point is documented by photo on XDA and on Goetz's page; Goetz says there are no large, labelled eMMC test points, though firebreak calls it a "test point".
3. **Dumping the flash and reading it.** Once the bootrom exploit ran, they read the partition table (Goetz) and the whole filesystem (Benge), which is how the plaintext Wi-Fi file and the `fos_flags` debug hooks were noticed **[V]**.
4. **UART.** Goetz located the boot UART with an oscilloscope on unmarked pads and watched the bootloader log. AntiEngineer is credited by firebreak for "help[ing] find the pins and bring up UART on a Dot" **[V]**. UART output shows which stage is running and what the bootloader prints, which is how you learn what an LK will accept.
5. **Reading the bootloader.** j10hx40r's trick was to patch a value in the LK so a function that asks "is this device unlocked?" says yes, which exposes the `fos_flags` debug options (Benge, quoting j10hx40r **[V]**). That is a firmware-diffing and binary-patching result, not a new vulnerability.
6. **Reusing a private bug from the same bootloader family.** k4y0z's fastboot exploit was written for Fire tablets, whose LK shares code with Echo LKs. The 2019 biscuit commits show k4y0z at least tried the idea on a Dot (**[V]** commits; **[I]** that the motive was the Dot's shared LK). Rortiz2's port of it, and the need for a **demo unit** with older firmware on the Show 5 (because retail units had both stages patched, Aftermath **[S]**), show that having an unpatched firmware image was essential.
7. **Leaked or documented tooling.** No leak is claimed by any source. Everything used is public: amonet, kamakiri, mtkclient (bkerler; firebreak "agrees with two of mtkclient's bootrom addresses" and cites it as reference **[V]**), Amazon's GPL kernel source, and archived OTA images (firebreak credits ftvdb for an archive of Fire OS builds **[V]**).
8. **Unknown: CVEs.** I found no CVE number tied to the bootrom bug, the LK bug, or Amazon's `fos_flags`. Benge notes the 2nd-gen Dot is out of Amazon's bug bounty scope and receives no security updates, which is why he judged publication safe (a search-result paraphrase **[S]**).

## 6. What firebreak automates, step by step

firebreak (`bboe/firebreak`, BSD-2-Clause, from PyPI via `uvx`) does not contain a new exploit. Its README says "all of this is possible because of Rortiz2's work" and credits amonet, k4y0z, AntiEngineer, Magisk, Goetz's short-location write-up, mtkclient, and ftvdb **[V]**. Its job is orchestration. The README-level description:

1. **Discover the state.** It polls the device every 2 seconds over USB and works out whether it is stock and booted, in fastboot, in the bootrom or preloader, in TWRP, or already rooted (docs/unlocking.md table **[V]**). A stopped run resumes.
2. **Ask for a button gesture when it needs fastboot.** The ring colour tells the user which mode the Dot is in (docs/measurements.md exists for this).
3. **Run the published "fastbrick" step.** It flashes the model- and bootloader-specific image, retries up to 10 times, and treats a flash that runs more than about 8 seconds as "the exploit started". The payload refuses to proceed on the wrong model **[V]**.
4. **Install the patched boot chain through the bootrom path.** It writes the patched TrustZone, both LK slots, a helper payload, and the preloader (written last, so a stop part-way leaves the bootrom reachable), restores the stock partition table, zeroes the anti-rollback area, and sets a flag to land in fastboot (docs/unlocking.md **[V]**). Every write is read back.
5. **Install TWRP and, by default, `boot-root.zip` on the Fire OS 6 already present.** This keeps the Dot's setup. With `--factory-reset` it erases the Dot and installs Fire OS 6 build 8149 first. The zip is Rortiz2's, an XDA attachment; firebreak opens it in the user's browser (XDA serves a JavaScript challenge to scripts) and checks its SHA-256 **[V]**.
6. **Harden against Amazon updates.** On Fire OS 6 it disables the OTA service and points Amazon's update hosts at `127.0.0.1` (changelog **[V]**).
7. **Alternative target `unlock fireos5`** (Dot only) installs amonet v1.1.0, TWRP 3.7.0_9-bboe2 and rooted Fire OS 5.5.5.4 with Magisk 17.3 **[V]**.
8. **`stock`** puts Amazon's firmware back (the whole Dot is erased). `report` prints state for a support request; `short` helps when the Dot shows no light; `write-test` distinguishes a bad cable from a failing flash chip **[V]**.

Observed on the maintainer's unit (R-07): Fire OS 6.5.7.5, root adb, `unlocked_kernel=true`, userdebug, dm-verity disabled, no `su`-style prompt. That is consistent with `boot-root.zip` on Fire OS 6 (firebreak says it leaves "an adbd that runs as root and no `su` at all") **[I]**. Whether the maintainer used firebreak or the XDA zip directly is not recorded (answers R-07 §11 item 10 only in part; the OTA-survival question remains open, section 7).

## 7. Risks and recovery

What can go wrong, in the order a user is likely to meet it:

| Failure | Cause | Recovery | Source |
|---|---|---|---|
| Fastboot never appears; ring stays solid blue | Power-only cable, weak USB port, missing Windows driver | Another cable or port; install drivers; try a USB 3 port | firebreak docs/troubleshooting.md **[V]** |
| Update to an unsupported build | Dot connected to Wi-Fi before the unlock; Amazon pushes a newer bootloader | Probably not recoverable by software if the bootloader is newer than any published payload **[I]**; firebreak refuses a bootloader it does not know and says so **[V]**. Keep the Dot off Wi-Fi until unlocked, then block OTA hosts | README warning, docs/unlocking.md **[V]** |
| Interrupted write while `boot0` is erased | Cable pull, power loss, host crash | The Dot shows only the bootrom USB port; `firebreak` resumes from the bootrom with no hardware work **if** it was left there | docs/unlocking.md **[V]** |
| Dot shows no light and offers no fastboot | The preloader runs but cannot start the bootloader (a "soft brick") | Open the case (Torx T8), short the documented passive near R60 to the shield frame while powering on, use `firebreak short`, then run a target. Per the XDA title this is the "unbrick" route | docs/troubleshooting.md; Goetz Part 2 **[V]** |
| Anti-rollback mismatch loop | RPMB counter higher than the preloader's | Reachable only via the eMMC short; boots again once RPMB is zeroed or a preloader at least as new is written | firebreak devices.md, "read from the code, not seen" **[U]** |
| Stalled during the "grace" window | Interrupting after the first steps | The XDA thread warns that interrupting "after the 10-second grace period will brick the device" (search snippet) **[U]** for exact wording; treat as a warning to leave a running install alone | search snippet |
| Wiped data | Rooting formats userdata | Expected. The Dot loses Wi-Fi and Alexa registration | docs/unlocking.md **[V]** |
| Wrong firmware flashed to the wrong Echo | Cross-device mix-up | firebreak checks the model name in the bootloader and a payload-side device-type check before writing | docs/unlocking.md **[V]** |
| Panic after unlock | R-07 hazard: reading `amz_privacy/power_button_state` panics the kernel | Never read it | [R-07](07-echo-dot-gen2-hardware.md) §10 **[V]** |
| Unlock removed by an update | Amazon OTA overwrites the patched LK or boot image | `otad` disabled and hosts nulled by firebreak; EchoCLI also advised blocking the same hosts | changelog; EchoCLI README **[V]** |

Ways back to stock or to a known-good state:
- **TWRP restore of a saved boot image** (EchoMuse's wizard escrows the user's own boot image; restore takes about ten seconds and leaves `/data` alone) **[V]**.
- **`firebreak stock [BUILD]`**: restores a stock Fire OS 6 build (8149 by default), erases the Dot, and on the Dot 3rd gen locks it again **[V]**.
- **R0rt1z2's "unbrick" in the XDA thread** (contents not read **[U]**) and **mtkclient-based recovery** (EchoCLI's old `debrick.md`) **[V]** as pointers.

## 8. Why this fits the 2nd-gen Dot and not newer devices

1. **The silicon.** Biscuit, radar and (reportedly) donut use MediaTek parts with known bootrom bugs and USB download mode reachable (Goetz; firebreak devices.md). The Dot 2 is also the last Dot with a full USB port on the base, so the hardware path is easier (Goetz Part 1 **[V]**).
2. **The firmware age.** The LK flaw was patched out of retail Show 5 firmware, which is why a demo unit was needed (Aftermath **[S]**). On the Dot 2, Amazon's last Fire OS build still carries a bootloader the exploit can be re-targeted to (builds 8338 to 8349 use a 2026-06-17 LK with its own prebuilt image, firebreak devices.md **[V]**).
3. **Where Amazon moved.** AFTVnews and Liliputing note that Amazon is moving to its own Linux-based Vega OS on newer Echo devices, which "locked-down Android" tricks do not apply to **[S]**.
4. **Per-product fuses and rules.** firebreak's notes record that the Echo Show 5 (both generations) and Dot 3rd gen were *reported* to have bootrom USB download fused off, which would make a failed unlock permanent; firebreak itself disproves that claim for the Dot 3rd gen (kamakiri-donut drives its bootrom) and says the fuse is per product, so a shared SoC settles nothing **[V]**. One guide notes the Show 5 2nd gen shows anti-rollback fuses set **[S]**.

So "2nd gen only" is too narrow. firebreak v1.0.0 covers the Echo Dot 2nd gen, Echo 2nd gen (radar) and Echo Dot 3rd gen (donut); R0rt1z2's amonet covers some Show models. I found no source for the Dot 4th or 5th gen, and no source explaining *why* they are not unlockable beyond "different hardware and software" **[U]**.

## 9. What this means for ClawDot

1. **Do not depend on firebreak.** It is a community tool about five weeks old, built on another person's exploit work that is still changing, it needs a browser to fetch an XDA attachment, and its scope can shrink if Amazon patches. The install guide should say: "prerequisite: your Dot is already unlocked, running Fire OS 6 with root adb (or whichever state R-07 describes). Ways to get there are documented upstream at the links below. We do not support, test or ship them."
2. **Pin and record, do not vendor.** If the guide names a tested path, record the firebreak version (v1.0.0, 2026-11-06), the Fire OS build, and the date, and re-verify before each release. Do not copy amonet binaries or payloads into the repo: `brom-payload` is GPLv2 and Amazon-derived images are not ours.
3. **Entry state for the Board layer.** R-07's list of what the unlocked unit looks like (root adb, verity off, `userdebug`) is the contract the Dot board should assume, and the guide should say how to check it. R-07's open item 10 (does the unlock survive an OTA) is answered in part: both firebreak and EchoCLI block the OTA hosts and firebreak disables `otad`; ClawDot's installer should verify that, never assume it.
4. **Credits** (README or `docs/credits.md` when the guide exists): Rortiz2/R0rt1z2 (amonet for Echo devices, XDA threads); k4y0z/chaosmaster (the earlier biscuit work and the fastboot exploit); xyzz (original amonet and bootrom exploit); AntiEngineer (UART/pins); bkerler (mtkclient); Andy Goetz (the Dot 2 short point and UART write-ups); Daniel Benge/Dragon863 (EchoCLI, the first public tethered root); j10hx40r (the LK patch clue); Bryce Boe/bboe (firebreak); bengris32 (LineageOS and BCB fix); the EchoMuse, TECHO5, EchoLocal and EchoGo authors for prior art on the *software* side. Credit is for ideas and tools we link to, not code we ship.
5. **Positioning.** R-04's statement "No published unlock found" is now out of date for the Dot; see the dated correction there. The right public claim is: *ClawDot runs on a Dot 2 that the owner has unlocked by community methods; we do not provide the unlock.* That also keeps us clear of "circumvention tool" framing.
6. **Scope discipline.** The prior art (EchoMuse, TECHO5, EchoLocal) occupies "Dot as Home Assistant satellite". ClawDot's differentiation is the agent/inbox model, not the unlock; no new work is needed here beyond the guide and credits.
7. **Open items (no `docs/open-questions.md` exists yet, so recorded here):** (a) the exact date and contents of the XDA biscuit thread (first post, changelog, unbrick text); (b) whether the maintainer's unit was unlocked with firebreak or the zip, and its LK build; (c) whether Amazon's 2026-06-17 LK has a newer patch that breaks the current payload; (d) existence of any CVE for the fastboot flaw; (e) who is the "Joel Stein" name on k4y0z's commits.

## 10. Sources

Primary repos and posts (read)
- bboe/firebreak: README, docs/unlocking.md, docs/devices.md, docs/troubleshooting.md, CHANGELOG.md, releases and commit list via `gh api repos/bboe/firebreak`. https://github.com/bboe/firebreak
- bboe/overdub (precursor scripts). https://github.com/bboe/overdub
- R0rt1z2/amonet: README, branches `mt8163-biscuit` and `mt8163-radar`, commit history. https://github.com/R0rt1z2/amonet
- xyzz/amonet (2019 original, Fire HD 8 2018). https://github.com/xyzz/amonet
- chaosmaster/amonet (k4y0z; branches including `mt8163-biscuit`). https://github.com/chaosmaster/amonet
- Dragon863/EchoCLI. https://github.com/Dragon863/EchoCLI
- Binozo/EchoGo. https://github.com/Binozo/EchoGo
- wilbowes/EchoMuse: README, JOURNAL.md, docs/rooting.md. https://github.com/wilbowes/EchoMuse
- HuskerMinion/techo5-dot: README, credits. https://github.com/HuskerMinion/techo5-dot
- Andy Goetz, "Unlocking Echo Dot" Parts 1 to 3: http://andygoetz.org/blog/2022/05/unlocking-echo-dot-v2/ , https://andygoetz.org/blog/2022/05/echo-dot-v2-dumping-emmc/ , http://andygoetz.org/blog/2022/06/echo-dot-v2-finding-the-uart/
- Daniel Benge, "Rooting the Amazon Echo Dot" (2023-07-19): https://danieldb.uk/posts/alexa-2/ ; "Alexa, what is my Wi-Fi password?" (2023-03-16, mirror): https://hugo-4tu.pages.dev/posts/alexa-1
- Hackaday, "Root, On An Amazon Echo Dot" (2023-07-22): https://hackaday.com/2023/07/22/root-on-an-amazon-echo-dot/
- Aftermath, "The Amazon Echo Show Obeys Me Now" (2026-03): https://aftermath.site/amazon-echo-show-jailbreak-lineageos/
- AFTVnews (2025-10-14): https://www.aftvnews.com/?p=45507 · Liliputing (2025-10-14): https://liliputing.com/amazon-echo-hacks-you-can-unlock-the-bootloader-and-install-custom-roms-on-some-older-models/

Not retrievable (cited via search snippets or quotations on other pages)
- XDA, Rortiz2: Echo Dot 2nd gen (biscuit) https://xdaforums.com/t/unlock-root-twrp-unbrick-amazon-echo-dot-2nd-gen-2016-biscuit.4761416/ ; Echo 2nd gen (radar) https://xdaforums.com/t/unlock-root-twrp-unbrick-amazon-echo-2nd-gen-2017-radar.4801290/ ; Echo Dot 3rd gen (donut) https://xdaforums.com/t/unlock-root-twrp-unbrick-amazon-echo-dot-3rd-gen-2018-donut.4801400/ ; "Rooting/hacking/anti-Amazonifying an ancient Echo Dot" https://xdaforums.com/t/rooting-hacking-anti-amazonifying-an-ancient-echo-dot.4766429/ (all HTTP 403).
- Rortiz2 post on X, 2025-10-08 (HTTP 402). Hackster article (relative date only): https://www.hackster.io/news/this-tethered-root-lets-you-run-your-own-code-on-amazon-s-second-gen-echo-devices-32fc9cab77c9
- Clinton Cook's Echo 1st-gen paper (cited by Goetz; not read), bkerler/mtkclient (cited by firebreak and EchoCLI; not read), openlipc, the "echohacking" wiki (cited by Benge; not read).

## Spec impact (recommended)
- Roadmap "Beyond v1.0" and spec 0002 §2: change "unlock status: none known" to "community unlock exists (amonet-biscuit, firebreak); not provided or supported by ClawDot"; keep the Dot as opportunistic.
- Add a "Credits" section to the future install guide using the list in §9.4.
- No change to the budget or protocol specs.

## Confidence
Mechanism and firebreak workflow: **medium-high** (read from firebreak's own docs and the amonet repos; deliberately not independently verified on hardware). Discovery chronology 2019 to 2023: **medium-high** (dated primary posts and commits). Discovery of the persistent fastboot-based unlock: **low-medium** (rests on one interview summary, a social post snippet and commit history; the XDA threads were unreachable). Everything tagged [I] or [U] is exactly that.
