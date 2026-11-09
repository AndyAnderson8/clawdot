//! Wake-word runtime for the streaming int8 microWakeWord models (spec 0007, R-11).
//!
//! Three small parts and a decision rule, with no dependencies and no `unsafe`:
//!
//! * [`model`] reads a TFLite flat buffer (over the bounds-checked reader in [`flat`]) and refuses anything
//!   outside the operator set these models use;
//! * [`frontend`] is the TensorFlow Lite Micro audio frontend in integer arithmetic;
//! * [`interp`] plans a tensor arena from the model and runs it with int8 [`kernels`];
//! * [`engine`] ties them together: sliding-window probability average against a per-word cutoff.
//!
//! A model file is untrusted input: every path from bytes to numbers returns an [`Error`] instead of panicking.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod engine;
pub mod error;
pub mod flat;
pub mod frontend;
pub mod interp;
pub mod kernels;
pub mod model;
pub mod sha256;

pub use engine::{Engine, RawDetection, WordConfig};
pub use error::Error;
pub use frontend::{Frontend, NUM_CHANNELS};
pub use interp::Interpreter;
pub use kernels::Rounding;
pub use model::Model;

/// Sample rate the models are trained for.
pub const SAMPLE_RATE: u32 = 16_000;
