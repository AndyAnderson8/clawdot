# R-04: Echo Dot Gen 2 reality check (SPIKE-001, desk phase)

Date: 2026-11-07 · Feeds: ADR-0003, spec 0002 §2, roadmap M7

## Findings

Hardware (teardowns: [All About Circuits](https://www.allaboutcircuits.com/news/teardown-tuesday-amazon-echo-dot-v2/), [EDN Asia](https://www.ednasia.com/galleries/teardown-amazon-echo-dot/), [EEVblog](https://www.eevblog.com/forum/reviews/teardown-amazon-echo-dot-2/)):
- SoC **MediaTek MT8163V**, quad-core ARM; **4 Gbit LPDDR3 (= 512 MB)**; **4 GB eMMC**. (One source says "512KB", an evident typo.)
- **Seven microphones** feeding **four TLV320ADC3101 ADCs** (EEVblog). The mics are plain ADC inputs, so beamforming and echo cancellation are done by the SoC or software, not by a dedicated mic-array module.
- Firmware ([kilworks/echo-dot-rea](https://github.com/kilworks/echo-dot-rea)): Fire OS 5.5.0.3 on Android 5.1.1; **ARM64 kernel with 32-bit (armeabi-v7a) userland**; codename "biscuit".

Security and unlock:
- Secure boot chain: preloader → LK (RSA-2048/PSS-verified, Lab126 certificate chain) → TrustZone (encrypted Trustonic container) → boot. A/B slots.
- The firmware analysis is static only and claims no unlock. The page lists "idme unlock" as a boot-chain topic without explaining it.
- No ADB, no data path, USB is power only, Bluetooth is audio only.
- A university capstone ([jhautry/echo-dot](https://github.com/jhautry/echo-dot)) tried MediaTek flash tooling and **could not root it**: no scatter file for the MT8163, and the preloader window was brief.
- Deeper access would likely need eMMC or UART teardown.
- **No published unlock or custom firmware found.**

## Conclusions

1. **The budget assumptions hold:** ≈512 MB RAM, 4-core A53, 32-bit userland → `armv7-unknown-linux-musleabihf` is the correct target. Spec 0002 §2 can move from "assumed" to "reported by teardowns", still with a measured baseline outstanding.
2. **The Dot port currently has no known path.** It remains a stretch goal (as ADR-0003 already says), and should be removed from M7 *planning dependencies*. Anyone who finds a way in should be able to use our protocol, and that is all we promise.
3. **Audio caveat:** mics wired to raw ADCs mean a Dot port would also need software beamforming/AEC (we assumed a mic array might provide it). This reinforces the satellite capability model (AEC presence is declared, otherwise the core or satellite supplies it) and SPIKE-004.
4. **Flash is 4 GB shared with the OS**, so the storage budget (16 MB) is safe.
5. The 512 MB RAM must also hold the OS and vendor stack; a custom OS would free most of it, but that is unmeasured.

## Spec impact (recommended)
- Spec 0002 §2: update the reference-device table with the sourced numbers; keep "unlock status: none known".
- Roadmap M7: relabel "Echo Dot Gen 2 port" as *opportunistic, not planned*.
- A cheaper, accessible stand-in for validating the budget: any 512 MB armv7 Linux board or a constrained VM/cgroup (CI already plans qemu-user).

## Correction (2026-11-08)
A hands-on inspection ([R-07](07-echo-dot-gen2-hardware.md)) of one Dot that is reachable over `adb` as root found Fire OS 6.5.7.5 (Android 7.1.2, 32-bit kernel and userland), not 5.5.0.3, with an unlocked kernel flag and dm-verity disabled. So "no ADB" and "no unlock" describe retail units as reported by the sources above, not every unit in the maintainer's hands. The budget conclusions are unchanged; RAM measured at about 477 MiB total.

## Confidence
Specs: medium-high (several teardowns agree). Unlock status: medium ("not found" ≠ "impossible").
