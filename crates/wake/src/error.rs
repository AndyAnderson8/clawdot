//! The one error type of the runtime.

use std::fmt;

/// Why a model was refused or could not run. Messages name the operator, tensor or limit so a user can act.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The bytes are not a well-formed model: an offset, length or index points outside the data.
    Malformed(&'static str),
    /// The model is well-formed but exceeds a limit of spec 0007 section 5.
    Limit(&'static str),
    /// The model uses something this runtime does not implement (operator, type, fusion, quantisation).
    Unsupported(String),
    /// A tensor, shape or option is inconsistent with the operator that uses it.
    Invalid(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Malformed(m) => write!(f, "malformed model: {m}"),
            Error::Limit(m) => write!(f, "model over a limit: {m}"),
            Error::Unsupported(m) => write!(f, "unsupported by the wake runtime: {m}"),
            Error::Invalid(m) => write!(f, "invalid model: {m}"),
        }
    }
}

impl std::error::Error for Error {}

/// Result alias.
pub type Result<T> = std::result::Result<T, Error>;
