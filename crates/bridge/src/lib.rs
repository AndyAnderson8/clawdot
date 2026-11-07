//! ClawDot agent bridge: pulls voice replies from the device and resumes the agent.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod client;
pub mod sse;
