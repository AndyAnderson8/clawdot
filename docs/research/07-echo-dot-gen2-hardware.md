# R-07: Echo Dot Gen 2 hands-on hardware findings (SPIKE-001, hands-on phase)

Date: 2026-11-08 · Feeds: ADR-0012, spec 0001 §4.10 (REQ-BRD), spec 0002 REQ-NFR-115, roadmap "Beyond v1.0" items 1-2 · Refines: [R-04](04-echo-dot-gen2.md)

Method: one Echo Dot Gen 2 reachable over `adb` as root, inspected **read-only** (properties, `/proc`, `/sys`, `getevent -lp`, `/dev` listings, ALSA info, `tinymix` with no arguments, `ps`, `dmesg`, `strings` on system binaries, `ledctrl -g`/`-l`). Nothing was pushed, written, stopped or reconfigured. Credentials, the serial number, MAC addresses, the SSID and the LAN address are deliberately **not** recorded here.

## 1. Summary

1. The unit is **not** the stock locked-down device R-04 describes. It runs Fire OS 6.5.7.5 (Android 7.1.2, 32-bit kernel and userland: `armv7l`; the boot line `bootopt=64S3,32N2,32N2` says only the secure world is 64-bit), exposes `adb` with root, and boots with `androidboot.unlocked_kernel=true`, `buildvariant=userdebug`, dm-verity mode `disabled`. How it was unlocked is outside this note. Retail units are still believed to be locked (R-04's conclusion stands for them).
2. Every peripheral the board layer needs is reachable from userspace as **plain files**: buttons through `/dev/input/event*` (evdev), the 12-LED ring through a vendor LED driver's sysfs files or Amazon's `ledctrl` command, Wi-Fi through the `wpa_supplicant` control socket, audio through ALSA character devices. No custom kernel module is needed.
3. Amazon's stack owns all of those devices today (`ledcontroller`, `acebuttond`, `mixer`, `wifisvc`/`netmgrd`, `PuffinApp`, `oobed`, `sntp`). The audio devices in particular are held open exclusively, so running ClawDot as the voice assistant means taking them over. Which Amazon services can be stopped without a watchdog reboot is **untested** (§9).
4. One real hazard: **reading `/sys/devices/soc/10010000.keypad/amz_privacy/power_button_state` panics the kernel** (NULL GPIO descriptor in `get_power_button_state`) and reboots the device. No ClawDot code may read it (§10).
5. After a reboot the wall clock is restored from a stale saved value (hours wrong), not from 2010. The "clock unset" check of REQ-TMR-009 is not sufficient on this device (§7).

## 2. Platform

| Item | Finding |
|---|---|
| Product | Amazon `biscuit_puffin`, model family AEOBC; board platform `mt8163` |
| OS | Fire OS 6.5.7.5, Android 7.1.2 (SDK 25), security patch 2026-03-01; kernel 3.18.19 SMP PREEMPT |
| CPU | 4 × Cortex-A53 (`CPU part 0xd03`) reported as `ARMv7 Processor rev 3 (v7l)`, NEON and VFPv4. `ro.product.cpu.abilist` is `armeabi-v7a,armeabi` only. Target `armv7-unknown-linux-musleabihf` is correct (a ClawDot build already ran on the unit from `/data/local/tmp`) |
| RAM | `MemTotal` 489,064 kB (≈ 477 MiB). With the stock Amazon stack idle, `MemAvailable` ≈ 290-300 MB |
| Storage | `/` 750 MB ext4 on dm-verity, mounted **read-only** (33% used); `/data` 1.2 GB ext4 rw (10% used); `/cache` 770 MB. ClawDot's 16 MB budget (REQ-NFR-106) is trivially met |
| Load | Load average sat around 7 while `top` showed ≈ 78% idle. The inflation comes from kernel threads in uninterruptible sleep (`hps_main`, `dynamic_boost`, `GCPU`); it is not CPU use. Use `top`, not load average, for the budget baseline |
| Userland | Android bionic, `toybox`-style shell (no `awk`, no `head -c`, no `ip`), `ifconfig`, `route`, `netstat`, `ping`, `wpa_cli`, `tinyalsa` tools. Static musl binaries run fine |
| Security posture | SELinux present with mixed permissive/enforcing denials in `dmesg`; root `adb`; verity disabled |

## 3. Input devices and buttons

`getevent -lp` and `/system/etc/acebutton_config.json` agree on three evdev nodes:

| Node | Name | Keys it can emit | Role in Amazon's config |
|---|---|---|---|
| `/dev/input/event2` | `keys` (GPIO keys) | `KEY_VOLUMEUP` (115), `KEY_VOLUMEDOWN` (114) | `Button_GPIO`: `volumeUp`, `volumeDown` |
| `/dev/input/event1` | `mtk-kpd` (matrix keypad, `10010000.keypad`) | `KEY_MUTE` (113), `KEY_VOLUMEDOWN` (114), `KEY_HELP` (138) | `Mute_Button_Privacy`: `mute` = 113 |
| `/dev/input/event0` | `ACCDET` | none (3.5 mm accessory detect; the Dot 2 has an unused jack path) | not used |

- Amazon's config maps `KEY_HELP` (138) to a button it calls `uber`: this is the **action** button (the dot). It appears on `event1`, although the config file lists it under `event2`. The kernel capability bitmaps are authoritative: `event1` = {113, 114, 138}, `event2` = {114, 115}.
- `KEY_VOLUMEDOWN` appears on **both** nodes (the keypad's 114 is the kernel's reset-key binding, `sw-rstkey = 114`). Which physical press produces which node is **unverified**; see §11, item 1.
- States: `down` = 1, `up` = 0, `repeat` = 2. Amazon's autorepeat is 300 ms then 150 ms. Long-press classes (`long`, `doubleLong`, `veryLong`, `superLong`) are computed in userspace by `acebuttond`, not by the kernel, so ClawDot computes its own from down/up timestamps. Amazon's special holds: 5 s on a button enters setup (OOBE), 21 s factory-resets; ClawDot must not take over a key while it is also being read by `acebuttond` unless that daemon is stopped, or a long hold will still trigger Amazon behaviour.
- Readers today: `acebuttond`, `aceinputmanager`, `aceusagestatd` all hold the three nodes open. evdev allows multiple readers, so ClawDot can read concurrently without stopping anything, but should not suppress events (`EVIOCGRAB`).
- Input device nodes are `root:input 0660`; running as root is fine.

## 4. Microphone mute (privacy circuit)

- The mute key is `KEY_MUTE` (113) on `event1`, but mute is not just a key: a kernel driver, `amz_privacy`, lives at `/sys/devices/soc/10010000.keypad/amz_privacy/` with `enable`, `state`, `privacy_state`, `privacy_timer_on`, `privacy_trigger`, `privacy_brightness`, `power_button_state`, `shutdown_dialog_state`.
- `acebuttond` reads `amz_privacy/state` and `enable`, logs "SET Mute LIPC state prop: 8 (enable mic) / 9 (disable mic)", and `ledcontroller` writes `privacy_brightness` (the mute indicator is driven from the same driver).
- The AIC3101 codec also exposes a mixer control `MFP Gpio Mute` (currently `On`), a probable link between the privacy GPIO and the ADCs. Whether the mute path is **electrically** cutting the microphones is not provable from software and must not be claimed (§11, item 3).
- Values observed with the unit un-muted: `state` = 0, `enable` = 0.
- **Do not read `power_button_state`** (§10). `privacy_trigger` and `shutdown_dialog_state` were deliberately not touched; their semantics are unknown and the driver has proven fragile.

## 5. LED ring

Amazon's own LED chain (all userspace is open to inspection):

```
callers (puffin, oobed, factory reset, OTA, ledctrl)  --AIPC/LIPC or Binder-->
  ledcontroller (/system/bin, Binder service "LedController", amazon::ILedController)
    --libacehal_led.so (aceLedHal_*: open, setState, setMultiState, setParams, getInfo)-->
      is31fl3236 kernel driver  (I2C bus 0, address 0x3f)
        sysfs: /sys/bus/i2c/devices/0-003f/{frame, led_current, boot_animation}
```

- **Hardware:** an **ISSI IS31FL3236** 36-channel LED driver at I2C `0-003f` (the HAL symbols are `initIssiLed`/`detectIssiLed`/`setLedFrame`), driving **12 RGB LEDs** (every animation frame has 12 colour triplets). `/sys/class/leds` is empty because the driver is custom, not the Linux LED class. `led_current` reads `2`. A second privacy/mute indicator uses `pwmchip0/pwm1` and `amz_privacy/privacy_brightness`. Four `lp55231` chips are probed but fail detection (`dmesg`: device detection err -6): unused footprint from a sibling product.
- **Files owned by `ledcontroller`:** `frame`, `led_current`, `boot_animation` are `ledcontroller:ledcontroller`, `-rw-r--r--`. Running as root, ClawDot may write them. The `frame` write format is **not documented and not yet verified**; reading it returns nothing.
- **Preloaded animations:** `/system/etc/led-resources/*.animation` (about 230 files; names such as `active-start`, `active-talking`, `active-thinking`, `_thinking`, `_mic-mute`, `ready-timer`, `ready-alarm`, `error`, `volume_step-NN`, `solid_blue`, `solid_red`, `off`) plus `layer_config_common.json` (six priority layers: the volume layer outranks listening, OOBE outranks do-not-disturb). **Animation file format** (plain text): an optional `loop` line, then frames `DURATION_MS:RGB,RGB,...` with exactly 12 entries per frame, each colour three hex nibbles in R,G,B order (so `solid_blue` is `100:00F,00F,...`).
- **A shell interface already exists:** `ledctrl -s PATTERN` (set a preloaded pattern), `-u PATTERN` (unset), `-c` (clear), `-g` (list active patterns), `-b 0..100` (brightness), `-a on|off` (auto brightness), `-l` (read brightness). `ledctrl_aipc` is the same through AIPC. Read-only calls worked: `-g` reported `anim_start_phase2` and `-l` reported 43.
- **Ambient light sensor:** `tsl2584` at `0-0029`, exposed as `iio:device0/calibrated_lux`, already read by `ledcontroller` for auto brightness.

Two ways to drive the ring (decision in ADR-0012):

| Route | How | Pros | Cons |
|---|---|---|---|
| A. Ask `ledcontroller` | spawn `ledctrl -s NAME` / `-u NAME` / `-c` (or call Binder, which needs unsafe FFI) | Smallest code (no driver knowledge), uses Amazon's animations and brightness handling, can coexist with the rest of the stack | Only the preloaded patterns (no arbitrary colours or ClawDot-specific states); a process spawn per change; Amazon daemons keep setting their own patterns on their own events |
| B. Own the ring | stop `ledcontroller`, write frames to `0-003f/frame` | Arbitrary per-LED colours and animation, no process spawn, tiny | `frame` format unknown; must reimplement brightness, thermal throttling (`led_cooler`) and the privacy indicator; conflicts if Amazon services restart it |

Recommendation: route A first (the `ledctrl` interface is verified present); route B as a later `Board` implementation detail behind the same logical-pattern API.

## 6. Audio

- ALSA card 0, `mt-snd-card`, 26 PCM devices (a MediaTek SoC audio front end with many unused routes). The relevant two:
  - **Playback** `pcmC0D23p` = `TLV320AIC3204 Playback`, driven by the **TLV320AIC32x4** DAC/amp (I2C `2-0018`). Observed parameters: 2 channels, 48 kHz, S16_LE, period 1536, buffer 3072.
  - **Capture** `pcmC0D24c` = `TLV320AIC3101 Capture`, the **four TLV320AIC3101 ADCs** (I2C `0-0018` to `0-001b`), i.e. 8 inputs. Observed parameters: **9 channels**, 16 kHz, S24_3LE, period 256, buffer 2560. Nine channels most likely = the 7 microphones plus 1-2 echo-reference channels. The channel map is unverified (§11, item 5).
- **Ownership:** `mixer` (`audio:audio`, ≈ 15 MB RSS) holds `controlC0`, `pcmC0D23p` and `pcmC0D24c` open, and the capture stream is in state RUNNING right now (the always-on wake-word path). Plain hardware PCM is single-open, so ClawDot cannot capture while this is true. The audio stack (`mixer`, `PuffinApp` wake word and Alexa client, `dacd`, `BTSinkPlayer`) must be stopped or replaced. Untested.
- **Mixer setup** is applied from `/system/etc/audio_device.xml` (TI ADC routing, PGA gains: ADC_A..D MICPGA 40, digital volume 88). ClawDot must replay that setup (or re-use the existing state) after taking over; `tinymix` (241 controls) can do it.
- **Tools present:** `tinyplay`, `tinycap`, `tinymix`, `tinypcminfo`. No `libasound`, no PulseAudio. `/dev/snd/*` is `system:audio 0660` (root can open).
- There is **no hardware AEC or beamformer**. Echo cancellation and beamforming in the stock firmware run in `PuffinApp` software. Barge-in therefore needs software AEC (REQ-VOI-005 stays opt-in), consistent with R-04.
- **Defaults to know:** persisted volumes `persist.mixer.init.{main,tts,alarm}.volume = 70`. Mic input is the primary 16 kHz format ClawDot needs (REQ-SAT-004), so the capture stream only needs downmixing/channel selection (one mic or a simple sum), not resampling.

## 7. Clock

- `date` is correct while online. Time comes from Amazon's `sntp` daemon (NTP, plus an "external HTTP source" fallback that is ignored when the HTTP time is older than the local clock) and the `securetime` service.
- The RTC (`/dev/rtc0`) reads **2010-01-01** and is not battery-backed, so it never carries real time.
- At boot, `sntp_time_update.sh` sets the clock from the property `persist.sys.saved_time` if it is later than the current time. The property was last written at an earlier boot, so after a reboot the clock restored to a time **about 3.5 hours behind** the previous session, plausible-looking but wrong. It stays wrong until the first successful sync (which needs a working network).
- `persist.sys.timezone` is empty (GMT). ClawDot already carries its own `zone`.
- Consequence: spec REQ-TMR-009 detects an *unset* clock (the 2010 epoch). On this device the clock is usually *stale but plausible*. The board must report a trust level (§ADR-0012 `ClockStatus`), and the Dot board has no cheap "synced" signal. Candidate signals: log the `sntp` result (`logcat -s sntp`), or do an HTTPS `Date` header check in the core (ClawDot already carries rustls). Unresolved (§11, item 8).

## 8. Wi-Fi and networking

- Chip: MediaTek CONSYS (`mediatek.wlan.chip = CONSYS_MT8163`), kernel interface `wlan0`, plus `p2p0` and a tethering interface `ap0` (Amazon's setup softAP). Driver `nl80211`, WMT firmware loader running (`wmt_launcher`).
- Supplicant command line (from `/proc`): `wpa_supplicant -iwlan0 -Dnl80211 -c/data/misc/wifi/wpa_supplicant.conf -I/system/etc/wifi/wpa_supplicant_overlay.conf ... -O/data/misc/wifi/sockets -g@android:wpa_wlan0`.
- Configuration: `/data/misc/wifi/wpa_supplicant.conf` (`wifi:wifi 0660`, 197 bytes) has `ctrl_interface=/data/misc/wifi/sockets`, **`update_config=1`** and one saved `network={}` block holding the credentials. Credentials therefore live only in that file (and `/data/misc/wifi/keystore/` for enterprise certs). Never read it back into logs.
- Control: `wpa_cli -p /data/misc/wifi/sockets -i wlan0 status` works as root (verified, read-only). `wpa_state=COMPLETED` when joined. From a shell, joining a network is the standard supplicant sequence (not executed here): `add_network`, `set_network N ssid '"NAME"'`, `set_network N psk '"SECRET"'`, `enable_network N`, `save_config`. `update_config=1` means `save_config` persists it.
- DHCP: `dhcpcd wlan0 -AdLK` runs as the `dhcpcd-wlan0` service; lease and DNS land in `dhcp.wlan0.*` properties (`getprop`). There is no `ip` binary; use `ifconfig`, `route`, `netstat`.
- Competing managers: `wifisvc` (the `aceWifiMgr_*` API: addNetwork, connect, getScanResults, createSoftAP, captive-portal evaluation), `netmgrd`, `halo_bssid_scan`, `halo_regiond`, and `oobed` (setup). They also drive the supplicant, so a ClawDot join via `wpa_cli` might be overridden by them. Unverified.
- Listening ports on the stock stack (relevant to REQ-CFG-014): 8080 (`oobed`), 9091, 4070, 1555, 55442, 55443, 55445, 5555 (adb), plus loopback 5037, 5556, 5557, 5560, 7802 and a DNS listener on the `ap0` address. ClawDot's default 8765 is free.
- mDNS: `avahi-daemon` already runs (`linux.local`), so `clawdot.local` (REQ-CFG-009) would need a second name registered with it or its own responder.

## 9. Amazon services, memory, and what may be stopped

About 160 processes; the userland that matters, with RSS in MB (approximate, idle):

| Group | Processes | RSS |
|---|---|---|
| Voice | `PuffinApp` 45, `puffinmrmd` 24, `AlexaHybridExecutionControllerLitespeed` 19, `SkillContainer` 15, `SmartHomed` 32, `LocalAutomationExecutionD` 8, `dacd` 7 | ≈ 150 |
| Audio | `mixer` 16, `BTSinkPlayer` 4 | 20 |
| Setup / net | `oobed` 10, `wifisvc` 6, `netmgrd` 2, `wpa_supplicant` 5, `dhcpcd` 3, `sntp` 6, `securetime` 1, `avahi` 2, `halo_*` ≈ 12, `provisionerd` 9 | ≈ 55 |
| Hardware | `ledcontroller` 8, `acebuttond` 5, `aceinputmanager` 5, `pwrsvcd` 6, `ace_powerd` 4, `ace_thermald` 3, `ace_sensorsd` 2 | ≈ 35 |
| Telemetry / OTA / misc | `uxeventd` 18, `commsd` 11, `trackerd` 8, `otad`/`ace_otad` 13, `ace_metric*`, `aceusagestatd`, `logmgr`, `perfmonitord`, `perfrecoveryd`, `UdssCampD`, `amakit_server`, `blemesh`, `btmanagerd` … | ≈ 100 |

- Android-style `start`/`stop` and `setprop ctl.stop` exist. **No service was stopped for this note**, so the following are *hypotheses to test in this order*, safest first: telemetry and metrics (`ace_metricd`, `aceusagestatd`, `uxeventd`, `trackerd`, `acedropboxd`), OTA (`otad`, `ace_otad`, `update_engine`: stopping it also protects the unlock from being overwritten), voice stack (`PuffinApp`, `puffinmrmd`, `AlexaHybrid...`, `SkillContainer`, `SmartHomed`, `LocalAutomationExecutionD`) with `mixer`/`dacd` last because they hold the audio devices.
- Must be kept (hypothesis): `servicemanager`, `logd`, `vold`, `keystore`, `ueventd`, `netmgrd`/`wifisvc`/`wpa_supplicant`/`dhcpcd` (Wi-Fi), `sntp`/`securetime` (clock), `ledcontroller` and `acebuttond` while on route A, `pwrsvcd`/`ace_powerd`/`ace_thermald` (power and thermal safety). `oobed` can only be understood once we know whether it re-launches setup when it sees no network or a long press (`oobed_on_boot`, `oobed_on_press`, `oobed_on_no_nw` services).
- Watchdogs to fear: `perfrecoveryd`, `pwrsvcd`, `ace_thermald`. Whether killing `PuffinApp` makes one of them reboot the unit is unknown.
- Budget conclusion: with the voice stack stopped, `MemAvailable` should rise by roughly 150-200 MB, so ClawDot's 64 MB hard gate (REQ-NFR-102) is comfortable. With the stack left running, ≈ 290 MB is still free, so ClawDot's *footprint* is not the constraint on this device; **device ownership** is.

## 10. Hazards (do not repeat)

| Hazard | Effect | Rule |
|---|---|---|
| Reading `amz_privacy/power_button_state` | Kernel NULL dereference in `get_power_button_state` → `gpiod_get_raw_value` → panic → watchdog reboot. Everything running in RAM is lost and the Wi-Fi did not rejoin automatically within several minutes afterwards | Never read or write this file. A `cat amz_privacy/*` glob is not safe |
| Touching `amz_privacy/privacy_trigger` or `shutdown_dialog_state` | Unknown | Leave alone until understood |
| Stopping `pwrsvcd`, `ace_thermald`, `perfrecoveryd` | Possible watchdog reboot or thermal hazard | Not before a reboot-safe test plan exists |
| OTA | May replace the unlocked system | Stop `otad`/`ace_otad` first; keep the unit offline from Amazon until then (unverified) |

## 11. Unknowns and how to resolve them

| # | Unknown | How to resolve | Needs |
|---|---|---|---|
| 1 | Which physical button emits which key on which node (volume up/down, action, mute) | Run `getevent -l` and press each of the four buttons in turn, once each, then long-press each | **The maintainer pressing buttons** |
| 2 | `0-003f/frame` write format; whether a direct write is safe and sticks with `ledcontroller` running | First try `ledctrl -s solid_blue` then `-u`/`-c`; for `frame`, stop `ledcontroller` and write a frame of the animation-file shape | Writes (not allowed in this read-only phase) |
| 3 | Is the mute electrically cutting the mics; can software set mute (`privacy_trigger`)? | Capture with `tinycap` while muted and un-muted; read driver source/strings; never touch `power_button_state` | Stopped audio stack |
| 4 | Can ClawDot own audio: stop `mixer`/`PuffinApp` safely, open `pcmC0D24c`/`pcmC0D23p`, replay `audio_device.xml`? | Staged service-stop test with a console watching `dmesg` for watchdogs | Writes, reboot risk |
| 5 | 9-channel capture map (which channels are mics, which are echo reference) | `tinycap` a tone while the speaker plays; inspect per-channel energy | Stopped audio stack |
| 6 | Which services can be stopped without a reboot (§9) | Staged test, one at a time, with `start` to recover | Writes |
| 7 | Does a `wpa_cli` join survive `wifisvc`/`netmgrd`/`oobed`; why the unit is `INACTIVE` after reboot with a saved network | Join via `wpa_cli` on a deliberately forgotten network; read `logcat -s wifisvc netmgrd` | Writes |
| 8 | A reliable "clock is synced" signal | Read `logcat -s sntp` after a sync; test an HTTPS `Date` header check | Network |
| 9 | How to autostart ClawDot at boot (`/system` is a read-only verity mount; `/data` is rw; is there a `/data` boot hook, or must `/system` be remounted) | Inspect `init.rc`/`init.*.rc` `on property`/`on boot` hooks; test `mount -o remount,rw /` | Writes |
| 10 | How the unit came to be unlocked and whether it survives an OTA | Ask the maintainer; stop OTA services | Maintainer |
| 11 | Ambient light and thermal: usable as optional board inputs (`calibrated_lux`, `led_cooler`) | Read `iio:device0/calibrated_lux` | nothing risky |

## 12. Conclusions for the design

1. A `Board` layer is **achievable and small** on this hardware: the facilities are files and sockets (evdev, sysfs, `ledctrl`, wpa control socket, ALSA devices). No kernel work and no mandatory dependency.
2. The hard part is **ownership**, not capability: Amazon's daemons hold the audio, LED, button and Wi-Fi devices. The board must declare what it takes over (a `takeover` list in the board file) and restore it on exit.
3. Not achievable today: proving the microphone mute is electrical; hardware AEC/beamforming (software only); using Amazon's wake word (proprietary); an arbitrary-colour LED ring without taking over the LED driver (route B, unverified); autostart at boot (unknown); a trustworthy "clock synced" signal from the platform.
4. R-04's budget conclusions hold and are strengthened: ≈ 477 MiB RAM, 4 cores, 32-bit userland, with ≈ 290 MB free beside the stock stack.

## Spec impact (recommended; applied in spec 0001 v0.9 and spec 0002 v0.5)
- New area `BRD` (board support) in spec 0001 §4.10, with requirements that cover the trait, the null and desktop boards, the Dot 2 board, per-device mapping, the clock trust level, mic-mute honesty, takeover and restore, and the build profile.
- REQ-NFR-115: board code is optional-feature code and counts against the budget.
- Spec 0002 §2 and R-04: note that an adb-root unit exists; unlock status "none known for retail units".

## Confidence
Peripheral inventory, device names, key codes, animation format and process list: **high** (read directly from the device). LED chip identity: medium-high (HAL symbols, I2C driver name `is31fl3236`, device bound). Mute electrical behaviour, channel map, stoppable services, autostart: **low / untested**.
