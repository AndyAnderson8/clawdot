//! Wake-word runtime for the streaming int8 microWakeWord models (spec 0007, R-11).
//!
//! Three small parts and a decision rule, with no dependencies and no `unsafe`:
//!
//! * [`model`] reads a TFLite flat buffer (over the bounds-checked reader in [`flat`]) and refuses anything
//!   outside the operator set these models use;
//! * a frontend (the TensorFlow Lite Micro audio frontend in integer arithmetic);
//! * an interpreter that plans a tensor arena from the model and runs it with int8 kernels;
//! * an engine that ties them together: sliding-window probability average against a per-word cutoff.
//!
//! A model file is untrusted input: every path from bytes to numbers returns an [`Error`] instead of panicking.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod error;
pub mod flat;
pub mod model;

pub use error::Error;
pub use model::Model;

/// Sample rate the models are trained for.
pub const SAMPLE_RATE: u32 = 16_000;
