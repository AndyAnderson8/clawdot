//! ClawDot core.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod app;
pub mod fastpath;
pub mod inbox;
pub mod sched;
pub mod store;
