//! The agent inbox: events from agents and services that the speaker announces, summarises, and lets
//! the user answer (REQ-INB-001..020).
//!
//! * [`event`]: the event model.
//! * [`adapter`]: declarative mappings from an agent's native payload to an event (REQ-INB-003, EXT-004).
//! * [`endpoint`]: named, token-protected sources (REQ-INB-001/002).
//! * [`queue`]: persistence, filtering, coalescing, and announcement order (REQ-INB-004..006, 012, 014).
//! * [`reply`]: at-least-once reply delivery to agent bridges (REQ-INB-008).
//! * [`summary`]: bounded spoken summaries (REQ-INB-018).

pub mod adapter;
pub mod endpoint;
pub mod event;
pub mod queue;
pub mod reply;
pub mod summary;
