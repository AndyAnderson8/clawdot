//! ClawDot core: the assistant program (see `docs/architecture/overview.md`).
//!
//! * [`startup`] and [`app_loop`]: the start-up sequence and the one loop that owns the state. [`runtime`] holds the
//!   brain; [`turns`] and [`speaker`] run model turns and speech as tasks beside the loop (ADR-0021).
//! * [`session`]: one model turn and the gate that decides every tool call. [`speakers`] keeps a conversation each.
//! * [`fastpath`] and [`assistant`]: local command recognition and the built-in tools, with no model or network.
//! * [`lang`]: the words the logic listens for, as data; today English.
//! * [`extensions`], [`httptools`], [`mcp`] and `mcp_server`: tools, connections and skills, and the MCP client and server.
//! * [`inbox`], [`app`] and [`http`]: events from agents, the shared state behind one lock, and the webhook, announce and bridge routes.
//! * [`ui`], [`settings`], [`admin`] and [`secrets`]: the portal's API, layered settings, the administrator, stored keys.
//! * [`satellite`], [`voice`], [`speech`] and [`vad`]: the satellite server and the voice dialog; the optional `realtime`, `esphome` and `hive` modules.
//! * [`sched`]: timers, alarms and reminders. [`audit`]: the record of every action and the daily caps.
//! * [`store`]: atomic, versioned JSON persistence. `update`: signed updates.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod admin;
mod announce_api;
pub mod apiclient;
pub mod app;
pub mod app_loop;
pub mod assistant;
pub mod audio;
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
mod hostguard;
pub mod http;
pub mod httptools;
pub mod inbox;
pub mod lang;
pub mod limit;
pub mod llm;
pub mod mcp;
mod mcp_server;
pub mod media;
pub mod netpolicy;
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
