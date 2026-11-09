//! ClawDot core.
//!
//! * [`inbox`]: events from agents and services, announced and answered by voice.
//! * [`admin`]: the web UI administrator (password or open mode), sessions, and recovery.
//! * [`app`]: the shared state behind one lock, with every operation a front end needs.
//! * [`http`]: webhook ingress and the SSE reply stream for agent bridges.
//! * [`sched`]: timers, alarms, and reminders.
//! * [`fastpath`] and [`assistant`]: local command recognition and dispatch, with no LLM or network.
//! * [`audit`]: the append-only record of every action, the daily caps and dry-run.
//! * [`store`]: atomic, versioned JSON persistence.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod admin;
pub mod app;
pub mod assistant;
pub mod audit;
pub mod cancel;
pub mod config;
#[cfg(feature = "esphome-satellite")]
pub mod esphome;
#[cfg(feature = "esphome-satellite")]
pub mod esphome_cfg;
pub mod extensions;
pub mod fastpath;
pub mod http;
pub mod httptools;
pub mod inbox;
pub mod limit;
pub mod llm;
pub mod mcp;
pub mod media;
pub mod notes;
pub mod satellite;
pub mod sched;
pub mod session;
pub mod settings;
pub mod speech;
pub mod store;
pub mod transport;
pub mod ui;
pub mod usage;
pub mod vad;
pub mod voice;
