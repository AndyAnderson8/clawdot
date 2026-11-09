//! `clawdot-device`: the portable device daemon (spec 0006, ADR-0015 Proposed).
//!
//! One program for every board. What differs between boards is a JSON board file ([`board_file`]); the daemon has
//! no per-device code path.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod board_file;

pub use board_file::{BoardError, BoardFile};
