//! ClawDot core.
//!
//! * [`inbox`]: events from agents and services, announced and answered by voice.
//! * [`app`]: the shared state behind one lock, with every operation a front end needs.
//! * [`http`]: webhook ingress and the SSE reply stream for agent bridges.
//! * [`sched`]: timers, alarms, and reminders.
//! * [`fastpath`] and [`assistant`]: local command recognition and dispatch, with no LLM or network.
//! * [`store`]: atomic, versioned JSON persistence.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod app;
pub mod assistant;
pub mod fastpath;
pub mod http;
pub mod inbox;
pub mod sched;
pub mod store;
