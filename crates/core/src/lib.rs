//! ClawDot core.
//!
//! * [`inbox`]: events from agents and services, announced and answered by voice.
//! * [`admin`]: the web UI administrator (password or open mode), sessions, and the local reset window.
//! * [`app`]: the shared state behind one lock, with every operation a front end needs.
//! * [`http`]: webhook ingress and the SSE reply stream for agent bridges.
//! * [`sched`]: timers, alarms, and reminders.
//! * [`fastpath`] and [`assistant`]: local command recognition and dispatch, with no LLM or network.
//! * [`audit`]: the append-only record of every action, the daily caps and dry-run.
//! * [`store`]: atomic, versioned JSON persistence.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod admin;
mod announce_api;
pub mod app;
pub mod assistant;
pub mod audit;
pub mod cancel;
pub mod cli;
pub mod commands;
pub mod config;
pub mod connects;
pub mod diag;
pub mod env;
#[cfg(feature = "esphome-satellite")]
pub mod esphome;
#[cfg(feature = "esphome-satellite")]
pub mod esphome_cfg;
pub mod extensions;
pub mod fastpath;
pub mod hardware;
#[cfg(feature = "hive")]
pub mod hive;
pub mod http;
pub mod httptools;
pub mod inbox;
pub mod limit;
pub mod llm;
pub mod mcp;
mod mcp_server;
pub mod media;
pub mod notes;
#[cfg(feature = "realtime")]
pub mod realtime;
mod routes;
pub mod rtvoice;
pub mod runtime;
pub mod satellite;
pub mod say;
pub mod sched;
pub mod secrets;
pub mod seed;
pub mod session;
pub mod settings;
pub mod skillfolder;
pub mod speaker;
pub mod speakers;
pub mod speech;
pub mod speechio;
pub mod startup;
pub mod stopwatch;
pub mod store;
pub mod transport;
pub mod turns;
pub mod ui;
#[cfg(feature = "tls")]
pub mod update;
pub mod usage;
pub mod vad;
pub mod voice;
