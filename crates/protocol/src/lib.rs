//! ClawDot satellite protocol.
//!
//! The only contract a hardware port must implement (spec 0003, ADR-0005, ADR-0008). This crate has
//! no I/O and no async runtime: it turns bytes into [`Frame`]s, so
//! the same code serves the core, the reference satellites, and the conformance tests.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod frame;

pub use frame::{Decoder, Frame, FrameError, Limits, encode};
