//! `clawdot-device`: the portable device daemon (spec 0006, ADR-0015 Proposed).
//!
//! One program for every board. What differs between boards is a JSON board file ([`board_file`]); the daemon has
//! no per-device code path. It speaks the satellite protocol through `clawdot-satellite` and adds what the device
//! layer owns: capability declaration ([`caps`]), LED states ([`led`]), buttons and gestures ([`button`]), built-in
//! tones ([`tone`]), quiet hours ([`quiet`]), and the wake-engine seam ([`wake`]). The first wake engine (spec 0007) is
//! behind the off-by-default `mww` feature (module `wake_mww`); no echo canceller
//! or device-specific driver is implemented yet; the capability report says so.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod board;
pub mod board_file;
pub mod button;
pub mod caps;
pub mod daemon;
pub mod led;
pub mod quiet;
pub mod tone;
pub mod wake;
#[cfg(feature = "mww")]
pub mod wake_mww;

pub use board::{Board, ConfiguredBoard, NullBoard, Parts};
pub use board_file::{BoardError, BoardFile};
pub use daemon::{DaemonConfig, DaemonDevice, run_forever};
