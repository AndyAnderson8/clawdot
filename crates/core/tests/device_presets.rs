//! A device preset is data (ADR-0012, REQ-CFG-083): each file in `examples/devices/` is a valid `device` section,
//! and its `board` names a real file in `boards/`, so the core's and the device daemon's names for a device agree.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use clawdot_core::config::Config;
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn board_names() -> Vec<String> {
    std::fs::read_dir(root().join("boards"))
        .unwrap()
        .filter_map(|e| {
            let text = std::fs::read_to_string(e.unwrap().path()).ok()?;
            let v: serde_json::Value = serde_json::from_str(&text).ok()?;
            v["board"].as_str().map(str::to_owned)
        })
        .collect()
}

#[test]
fn every_device_preset_is_valid_and_names_a_board_file() {
    let boards = board_names();
    assert!(!boards.is_empty(), "the board files went missing");
    let mut seen = 0;
    for e in std::fs::read_dir(root().join("examples/devices")).unwrap() {
        let path = e.unwrap().path();
        if path.extension().is_none_or(|x| x != "json") {
            continue;
        }
        seen += 1;
        let cfg = Config::parse(&std::fs::read_to_string(&path).unwrap())
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let board = cfg
            .device
            .board
            .unwrap_or_else(|| panic!("{} sets no device.board", path.display()));
        assert!(
            cfg.device.label.is_some(),
            "{} sets no label",
            path.display()
        );
        assert!(
            boards.contains(&board),
            "{}: no file in boards/ has board {board:?}",
            path.display()
        );
    }
    assert!(seen >= 1, "the device presets went missing");
}

#[test]
fn the_echo_dot_preset_declares_the_light_ring_the_kernel_cannot_list() {
    let text = std::fs::read_to_string(root().join("examples/devices/echo-dot2.json")).unwrap();
    let cfg = Config::parse(&text).unwrap();
    assert_eq!(cfg.device.parts.leds, Some(true));
}
