//! What this device is, found by looking at it: the kind of board, its processor and memory, and which parts it has (a
//! ring of lights, Bluetooth, a screen, a USB serial adapter that could be a radio). It reads the operating system's own
//! files (`/proc`, `/sys`, `/dev`) and guesses nothing it cannot see; on a system without them (Windows, macOS) the
//! parts are simply reported as not found. The page uses it to say what the device is and to hide settings it cannot use.

use serde::Serialize;
use std::path::Path;
use std::sync::OnceLock;

/// The parts found on the device.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Parts {
    /// A ring or row of lights the software could drive (`/sys/class/leds`).
    pub leds: bool,
    /// A Bluetooth adapter (`/sys/class/bluetooth`).
    pub bluetooth: bool,
    /// A wireless network adapter.
    pub wifi: bool,
    /// A screen (`/sys/class/graphics/fb*` or a display connector).
    pub screen: bool,
    /// A USB serial adapter (`/dev/ttyACM*`, `/dev/ttyUSB*`): the kind a Zigbee or Z-Wave radio shows up as. Finding one
    /// does not say what it is.
    pub usb_serial: bool,
}

/// What this device is.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Hardware {
    /// A short stable name for the kind of board: `echo-dot`, `raspberry-pi`, `pc`, or `unknown`.
    pub board: String,
    /// The model as the device calls itself, when it says.
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
    /// The parts that were found.
    pub parts: Parts,
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

/// A short stable name for the board, from what it calls itself and its processor.
#[must_use]
pub fn classify(model: &str, cpu: &str) -> &'static str {
    let m = model.to_lowercase();
    let c = cpu.to_lowercase();
    if m.contains("raspberry pi") {
        "raspberry-pi"
    } else if m.contains("echo")
        || m.contains("biscuit")
        || m.contains("mt8163")
        || c.contains("mt8163")
    {
        "echo-dot"
    } else if c.contains("intel") || c.contains("amd") || c.contains("apple") {
        "pc"
    } else {
        "unknown"
    }
}

fn read(path: &str) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .trim_matches(|c: char| c == '\0' || c.is_whitespace())
        .to_owned()
}

fn any_in(dir: &str, keep: impl Fn(&str) -> bool) -> bool {
    std::fs::read_dir(dir).is_ok_and(|d| {
        d.filter_map(Result::ok)
            .any(|e| e.file_name().to_str().is_some_and(&keep))
    })
}

/// Look at the device. Cheap, but not free: use [`detected`] to do it once.
#[must_use]
pub fn detect() -> Hardware {
    let linux = cfg!(target_os = "linux");
    let (cpu, cores) = if linux {
        parse_cpu(&read("/proc/cpuinfo"))
    } else {
        (
            String::new(),
            std::thread::available_parallelism().map_or(0, std::num::NonZero::get),
        )
    };
    let model = if linux {
        read("/proc/device-tree/model")
    } else {
        String::new()
    };
    let model = if model.is_empty() {
        read("/sys/firmware/devicetree/base/model")
    } else {
        model
    };
    let parts = if linux {
        Parts {
            leds: any_in("/sys/class/leds", |n| !n.is_empty()),
            bluetooth: any_in("/sys/class/bluetooth", |n| n.starts_with("hci")),
            wifi: any_in("/sys/class/net", |n| {
                n.starts_with("wlan") || n.starts_with("wlp")
            }),
            screen: Path::new("/dev/fb0").exists()
                || any_in("/sys/class/drm", |n| n.contains("card") && n.contains('-')),
            usb_serial: any_in("/dev", |n| {
                n.starts_with("ttyACM") || n.starts_with("ttyUSB")
            }),
        }
    } else {
        Parts::default()
    };
    let board = classify(&model, &cpu);
    let mut parts = parts;
    if board == "echo-dot" {
        // The light ring is there, but the device's own voice service drives it, so the kernel lists no LED for it.
        parts.leds = true;
    }
    Hardware {
        board: board.to_owned(),
        model,
        cpu,
        cores,
        ram_mb: if linux {
            parse_ram_mb(&read("/proc/meminfo"))
        } else {
            0
        },
        arch: std::env::consts::ARCH.to_owned(),
        os: std::env::consts::OS.to_owned(),
        parts,
    }
}

/// The hardware, found once.
#[must_use]
pub fn detected() -> &'static Hardware {
    static HW: OnceLock<Hardware> = OnceLock::new();
    HW.get_or_init(detect)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_and_processor_are_read_from_the_proc_files() {
        assert_eq!(parse_ram_mb("MemTotal:  498432 kB\nMemFree: 1 kB\n"), 486);
        assert_eq!(parse_ram_mb("nothing"), 0);
        let arm = "processor\t: 0\nmodel name\t: ARMv7 Processor rev 3 (v7l)\nprocessor\t: 1\nHardware\t: MT8163\n";
        assert_eq!(
            parse_cpu(arm),
            ("MT8163".to_owned(), 2),
            "a board name beats the generic core name"
        );
        let pc = "processor : 0\nmodel name : Intel(R) Core(TM) i7\nprocessor : 1\nmodel name : Intel(R) Core(TM) i7\n";
        assert_eq!(parse_cpu(pc), ("Intel(R) Core(TM) i7".to_owned(), 2));
    }

    #[test]
    fn the_kind_of_board_comes_from_what_it_calls_itself() {
        assert_eq!(
            classify("Raspberry Pi 4 Model B Rev 1.4", ""),
            "raspberry-pi"
        );
        assert_eq!(classify("", "MT8163"), "echo-dot");
        assert_eq!(
            classify("MT8163", "ARMv7 Processor rev 3 (v7l)"),
            "echo-dot",
            "the Dot names itself only by its chip"
        );
        assert_eq!(classify("Amazon Echo Dot 2", ""), "echo-dot");
        assert_eq!(classify("", "Intel(R) Core(TM) i7"), "pc");
        assert_eq!(classify("", "something else"), "unknown");
    }

    #[test]
    fn detecting_never_panics_and_names_the_platform() {
        let h = detect();
        assert!(!h.arch.is_empty() && !h.os.is_empty());
        assert_eq!(
            detected().arch,
            h.arch,
            "the cached copy is the same machine"
        );
    }
}
