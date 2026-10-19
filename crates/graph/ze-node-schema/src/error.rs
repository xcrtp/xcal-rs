//! Errors raised by the property model. All operations return errors rather
//! than panicking, so dynamic / script-driven use stays safe.

use thiserror::Error;

/// Property read/write/validation error.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PropertyError {
    /// The named property does not exist on the node.
    #[error("unknown property `{0}`")]
    UnknownProperty(String),
    /// A value of the wrong kind was supplied.
    #[error("type mismatch: expected {expected}, got {got}")]
    TypeMismatch { expected: String, got: String },
    /// A value was outside the declared range / bounds.
    #[error("out of range: {which} not in [{min}, {max}]")]
    OutOfRange {
        which: String,
        min: String,
        max: String,
    },
    /// A structurally invalid value (e.g. bad enum option, `lo > hi`).
    #[error("invalid value: {reason}")]
    InvalidValue { reason: String },
    /// A dynamic field already exists with that name.
    #[error("duplicate field `{0}`")]
    DuplicateField(String),
    /// A required field was missing.
    #[error("missing field `{0}`")]
    MissingField(String),
}
