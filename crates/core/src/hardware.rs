//! What this device is, found by looking at it: its model string, processor and memory, and which parts it has (a
//! ring of lights, Bluetooth, Wi-Fi, a screen, a USB serial adapter that could be a radio). It reads only what the
//! operating system shows (`/proc`, `/sys`) and names no brand, product or board: the core is device-agnostic.
//!
//! What a device is CALLED, and a part that exists but that the system cannot show (a light ring another service
//! drives, so the kernel lists no LED), come from the `device` section of the configuration file
//! ([`crate::config::DeviceConfig`]), chosen by whoever set the device up. A preset for one device is data, not code
//! (ADR-0012). On a system without `/proc` and `/sys` (Windows, macOS) the parts are reported as not found.
//! The page uses it to say what the device is and to hide settings it cannot use.

use crate::config::DeviceConfig;
use serde::Serialize;
use std::path::Path;
use std::sync::OnceLock;

/// The parts found on the device.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Parts {
    /// A ring or row of lights the software could drive (`/sys/class/leds`).
    pub leds: bool,
    /// A Bluetooth adapter (a controller under `/sys/class/bluetooth`).
    pub bluetooth: bool,
    /// A wireless network adapter (an interface the kernel marks as wireless).
    pub wifi: bool,
    /// A screen (a framebuffer in `/sys/class/graphics`, or a display connector that reports a screen attached).
    pub screen: bool,
    /// A USB serial adapter (a tty the kernel hangs off a USB device): the kind a Zigbee or Z-Wave radio shows up as.
    /// Finding one does not say what it is.
    pub usb_serial: bool,
}

/// The names of the parts, in the order of [`Parts::slots`].
const PART_NAMES: [&str; 5] = ["leds", "bluetooth", "wifi", "screen", "usb_serial"];

impl Parts {
    fn slots(&mut self) -> [&mut bool; 5] {
        [
            &mut self.leds,
            &mut self.bluetooth,
            &mut self.wifi,
            &mut self.screen,
            &mut self.usb_serial,
        ]
    }
}

/// The board identifier reported when none is configured.
pub const UNKNOWN_BOARD: &str = "unknown";

/// What this device is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Hardware {
    /// The identifier set in `device.board` (for example the `board` value of a file in `boards/`), or `unknown`
    /// when none is configured. Never guessed from the model string.
    pub board: String,
    /// What to show a person: `device.label`, or empty when none is configured (then show `model`).
    pub label: String,
    /// The model as the device calls itself (device tree or firmware), when it says.
    pub model: String,
    /// The processor, as named in `/proc/cpuinfo`.
    pub cpu: String,
    /// How many processor cores.
    pub cores: usize,
    /// Installed memory in megabytes.
    pub ram_mb: u64,
    /// The processor family the program was built for (`arm`, `aarch64`, `x86_64`).
    pub arch: String,
    /// The operating system (`linux`, `windows`, `macos`).
    pub os: String,
    /// The parts: as detected, except those named in `configured_parts`.
    pub parts: Parts,
    /// Which parts take their value from `device.parts` instead of from detection, so a page can say so.
    pub configured_parts: Vec<&'static str>,
}

impl Default for Hardware {
    fn default() -> Self {
        Self {
            board: UNKNOWN_BOARD.to_owned(),
            label: String::new(),
            model: String::new(),
            cpu: String::new(),
            cores: 0,
            ram_mb: 0,
            arch: String::new(),
            os: String::new(),
            parts: Parts::default(),
            configured_parts: Vec::new(),
        }
    }
}

impl Hardware {
    /// This hardware as the person who set the device up described it: their board identifier and label, and their
    /// word on any part (`true` or `false`) over what was detected.
    #[must_use]
    pub fn with_device(mut self, d: &DeviceConfig) -> Self {
        if let Some(b) = &d.board {
            self.board.clone_from(b);
        }
        if let Some(l) = &d.label {
            self.label = l.trim().to_owned();
        }
        let p = &d.parts;
        let said = [p.leds, p.bluetooth, p.wifi, p.screen, p.usb_serial];
        for ((slot, name), say) in self.parts.slots().into_iter().zip(PART_NAMES).zip(said) {
            if let Some(v) = say {
                *slot = v;
                self.configured_parts.push(name);
            }
        }
        self
    }
}

/// The first value of `key:` in `/proc/cpuinfo`-style text.
fn field(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        (k.trim().eq_ignore_ascii_case(key)).then(|| v.trim().to_owned())
    })
}

/// Installed memory in megabytes, from `/proc/meminfo` text.
#[must_use]
pub fn parse_ram_mb(meminfo: &str) -> u64 {
    field(meminfo, "MemTotal")
        .and_then(|v| {
            v.split_whitespace()
                .next()
                .and_then(|n| n.parse::<u64>().ok())
        })
        .map_or(0, |kb| kb / 1024)
}

/// The processor name and core count from `/proc/cpuinfo` text. Boards with an ARM chip often name it as `Hardware`.
#[must_use]
pub fn parse_cpu(cpuinfo: &str) -> (String, usize) {
    let cores = cpuinfo
        .lines()
        .filter(|l| l.split(':').next().is_some_and(|k| k.trim() == "processor"))
        .count();
    let name = field(cpuinfo, "Hardware")
        .or_else(|| field(cpuinfo, "model name"))
        .or_else(|| field(cpuinfo, "Model"))
        .unwrap_or_default();
    (name, cores)
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .trim_matches(|c: char| c == '\0' || c.is_whitespace())
        .to_owned()
}

/// The entry names in a directory, or none when it cannot be listed.
fn names(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir).map_or_else(
        |_| Vec::new(),
        |d| {
            d.filter_map(Result::ok)
                .filter_map(|e| e.file_name().to_str().map(str::to_owned))
                .collect()
        },
    )
}

/// The model string the device gives itself: the device tree where there is one, otherwise the firmware's product
/// name (DMI). Empty when neither says.
fn model_at(root: &Path) -> String {
    [
        "proc/device-tree/model",
        "sys/firmware/devicetree/base/model",
        "sys/class/dmi/id/product_name",
    ]
    .iter()
    .map(|p| read(&root.join(p)))
    .find(|m| !m.is_empty())
    .unwrap_or_default()
}

/// Whether a path passes through a `usbN` bus node.
fn through_usb(p: &Path) -> bool {
    p.components().any(|c| {
        c.as_os_str().to_str().is_some_and(|s| {
            s.strip_prefix("usb")
                .is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()))
        })
    })
}

/// A tty the kernel attached to a USB device: where its `/sys/class/tty` entry really lives is under a USB bus.
fn usb_serial_at(root: &Path) -> bool {
    let Ok(real_root) = root.canonicalize() else {
        return false;
    };
    let tty = root.join("sys/class/tty");
    names(&tty).iter().any(|n| {
        std::fs::canonicalize(tty.join(n))
            .ok()
            .and_then(|p| p.strip_prefix(&real_root).map(Path::to_path_buf).ok())
            .is_some_and(|p| through_usb(&p))
    })
}

/// What the files under `root` (`/` on a real system) show. No OS check: the caller decides whether they exist.
fn probe(root: &Path) -> Hardware {
    let (cpu, cores) = parse_cpu(&read(&root.join("proc/cpuinfo")));
    let net = root.join("sys/class/net");
    let drm = root.join("sys/class/drm");
    let parts = Parts {
        // Any LED class device. (A light ring that another service drives is declared in `device.parts`.)
        leds: !names(&root.join("sys/class/leds")).is_empty(),
        // A controller, not the connection handles (`hci0:256`) the kernel lists beside it.
        bluetooth: names(&root.join("sys/class/bluetooth"))
            .iter()
            .any(|n| !n.is_empty() && !n.contains(':')),
        // The kernel marks a wireless interface with a `wireless` directory (or a `phy80211` link), whatever it is named.
        wifi: names(&net).iter().any(|n| {
            net.join(n).join("wireless").exists() || net.join(n).join("phy80211").exists()
        }),
        // A framebuffer, or a display connector with something attached.
        screen: names(&root.join("sys/class/graphics")).iter().any(|n| {
            n.strip_prefix("fb")
                .is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()))
        }) || names(&drm)
            .iter()
            .any(|n| n.contains('-') && read(&drm.join(n).join("status")) == "connected"),
        usb_serial: usb_serial_at(root),
    };
    Hardware {
        model: model_at(root),
        cpu,
        cores,
        ram_mb: parse_ram_mb(&read(&root.join("proc/meminfo"))),
        parts,
        ..Hardware::default()
    }
}

/// Look at the device. Cheap, but not free: use [`detected`] to do it once. The board identifier and label are
/// not here: they are configured ([`Hardware::with_device`]).
#[must_use]
pub fn detect() -> Hardware {
    let mut h = if cfg!(target_os = "linux") {
        probe(Path::new("/"))
    } else {
        Hardware {
            cores: std::thread::available_parallelism().map_or(0, std::num::NonZero::get),
            ..Hardware::default()
        }
    };
    h.arch = std::env::consts::ARCH.to_owned();
    h.os = std::env::consts::OS.to_owned();
    h
}

/// The hardware, found once.
#[must_use]
pub fn detected() -> &'static Hardware {
    // Deliberately process-wide: a fact about the machine, the same for every core in the process.
    static HW: OnceLock<Hardware> = OnceLock::new();
    HW.get_or_init(detect)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PartsConfig;
    use crate::store::testutil::TempDir;
    use std::fs;

    fn put(root: &TempDir, rel: &str, text: &str) {
        let p = root.0.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }

    fn dir(root: &TempDir, rel: &str) {
        fs::create_dir_all(root.0.join(rel)).unwrap();
    }

    #[test]
    fn memory_and_processor_are_read_from_the_proc_files() {
        assert_eq!(parse_ram_mb("MemTotal:  498432 kB\nMemFree: 1 kB\n"), 486);
        assert_eq!(parse_ram_mb("nothing"), 0);
        let arm = "processor\t: 0\nmodel name\t: ARMv7 Processor rev 3 (v7l)\nprocessor\t: 1\nHardware\t: SoC-X\n";
        assert_eq!(
            parse_cpu(arm),
            ("SoC-X".to_owned(), 2),
            "a board name beats the generic core name"
        );
        let pc = "processor : 0\nmodel name : Intel(R) Core(TM) i7\nprocessor : 1\nmodel name : Intel(R) Core(TM) i7\n";
        assert_eq!(parse_cpu(pc), ("Intel(R) Core(TM) i7".to_owned(), 2));
    }

    // REQ-CFG-083
    #[test]
    fn nothing_is_named_from_the_model_string() {
        // Whatever the system calls itself, detection alone never names a board or a label.
        let d = TempDir::new("hw-model");
        put(&d, "proc/device-tree/model", "Some Maker Speaker 9\0");
        put(&d, "proc/cpuinfo", "processor : 0\nHardware : SOC-1\n");
        let h = probe(&d.0);
        assert_eq!(h.model, "Some Maker Speaker 9");
        assert_eq!((h.board.as_str(), h.label.as_str()), ("unknown", ""));
        // A firmware product name is used when there is no device tree.
        let d = TempDir::new("hw-dmi");
        put(&d, "sys/class/dmi/id/product_name", "Desk Box 3\n");
        assert_eq!(probe(&d.0).model, "Desk Box 3");
    }

    // REQ-CFG-083
    #[test]
    fn parts_are_what_the_kernel_marks_not_what_things_are_named() {
        let d = TempDir::new("hw-parts");
        let none = probe(&d.0).parts;
        assert_eq!(none, Parts::default(), "an empty system has no parts");

        // An interface called `net7` is Wi-Fi because the kernel says it is wireless; `wlan0` without that mark is not.
        dir(&d, "sys/class/net/net7/wireless");
        dir(&d, "sys/class/net/wlan0");
        assert!(probe(&d.0).parts.wifi);
        let e = TempDir::new("hw-wlan-name");
        dir(&e, "sys/class/net/wlan0");
        dir(&e, "sys/class/net/wlp2s0");
        assert!(
            !probe(&e.0).parts.wifi,
            "a wireless-sounding name is not enough"
        );
        let f = TempDir::new("hw-phy");
        dir(&f, "sys/class/net/x1/phy80211");
        assert!(probe(&f.0).parts.wifi);

        // A Bluetooth controller counts; the connection handle the kernel lists beside it does not.
        let b = TempDir::new("hw-bt");
        dir(&b, "sys/class/bluetooth/hci0:256");
        assert!(!probe(&b.0).parts.bluetooth);
        dir(&b, "sys/class/bluetooth/bt-radio");
        assert!(probe(&b.0).parts.bluetooth);

        // Lights: any LED class device.
        let l = TempDir::new("hw-led");
        dir(&l, "sys/class/leds/status");
        assert!(probe(&l.0).parts.leds);

        // A screen: a framebuffer, or a connector that reports a screen attached.
        let s = TempDir::new("hw-screen");
        dir(&s, "sys/class/graphics/fbcon");
        assert!(
            !probe(&s.0).parts.screen,
            "fbcon is the console, not a framebuffer"
        );
        dir(&s, "sys/class/graphics/fb0");
        assert!(probe(&s.0).parts.screen);
        let c = TempDir::new("hw-drm");
        put(&c, "sys/class/drm/card0-HDMI-A-1/status", "disconnected\n");
        assert!(
            !probe(&c.0).parts.screen,
            "an empty connector is not a screen"
        );
        put(&c, "sys/class/drm/card0-DSI-1/status", "connected\n");
        assert!(probe(&c.0).parts.screen);
    }

    #[cfg(unix)]
    #[test]
    fn a_usb_serial_adapter_is_a_tty_the_kernel_hangs_off_usb() {
        use std::os::unix::fs::symlink;
        let d = TempDir::new("hw-usb");
        // A tty on a platform bus (a built-in UART) is not an adapter, whatever it is called.
        dir(&d, "sys/devices/platform/uart0/ttyUSB9");
        dir(&d, "sys/class/tty");
        symlink(
            "../../devices/platform/uart0/ttyUSB9",
            d.0.join("sys/class/tty/ttyUSB9"),
        )
        .unwrap();
        assert!(!probe(&d.0).parts.usb_serial);
        // One under a usb bus node is, whatever it is called.
        dir(&d, "sys/devices/pci0000:00/usb1/1-2/1-2:1.0/radio0");
        symlink(
            "../../devices/pci0000:00/usb1/1-2/1-2:1.0/radio0",
            d.0.join("sys/class/tty/radio0"),
        )
        .unwrap();
        assert!(probe(&d.0).parts.usb_serial);
    }

    // REQ-CFG-083
    #[test]
    fn the_configured_device_names_it_and_overrides_only_what_it_says() {
        let seen = Hardware {
            model: "Some Maker Speaker 9".into(),
            parts: Parts {
                wifi: true,
                bluetooth: true,
                ..Parts::default()
            },
            ..Hardware::default()
        };
        let same = seen.clone().with_device(&DeviceConfig::default());
        assert_eq!(same, seen, "no configuration changes nothing");
        let cfg = DeviceConfig {
            board: Some("my-board".into()),
            label: Some(" My Speaker ".into()),
            parts: PartsConfig {
                leds: Some(true),
                bluetooth: Some(false),
                ..PartsConfig::default()
            },
        };
        let h = seen.with_device(&cfg);
        assert_eq!(
            (h.board.as_str(), h.label.as_str()),
            ("my-board", "My Speaker")
        );
        assert!(
            h.parts.leds && !h.parts.bluetooth,
            "the person's word wins, both ways"
        );
        assert!(h.parts.wifi, "an unmentioned part stays as detected");
        assert_eq!(h.configured_parts, ["leds", "bluetooth"]);
        assert_eq!(h.model, "Some Maker Speaker 9");
    }

    #[test]
    fn detecting_never_panics_and_names_the_platform() {
        let h = detect();
        assert!(!h.arch.is_empty() && !h.os.is_empty());
        assert_eq!(h.board, UNKNOWN_BOARD, "detection never names a board");
        assert_eq!(
            detected().arch,
            h.arch,
            "the cached copy is the same machine"
        );
    }
}
