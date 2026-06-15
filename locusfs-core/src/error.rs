use std::io;

use thiserror::Error;

/// Result type used by `locusfs-core`.
pub type Result<T> = std::result::Result<T, LocusFsError>;

/// Typed error surface for graph and filesystem-domain operations.
#[derive(Debug, Error)]
pub enum LocusFsError {
    #[error("invalid {kind} {value:?}: {reason}")]
    InvalidIdentifier {
        kind: &'static str,
        value: String,
        reason: &'static str,
    },
    #[error("invalid path segment {segment:?}: {reason}")]
    InvalidPathSegment {
        segment: String,
        reason: &'static str,
    },
    #[error("invalid percent encoding in segment {segment:?}")]
    InvalidEncoding { segment: String },
    #[error("{kind} not found: {name}")]
    NotFound { kind: &'static str, name: String },
    #[error("unsupported filesystem operation: {operation}")]
    Unsupported { operation: &'static str },
    #[error("invalid {kind} value {value:?}: {reason}")]
    InvalidValue {
        kind: &'static str,
        value: String,
        reason: &'static str,
    },
    #[error("I/O failed: {0}")]
    Io(String),
}

impl LocusFsError {
    pub(crate) fn invalid_identifier(
        kind: &'static str,
        value: impl Into<String>,
        reason: &'static str,
    ) -> Self {
        Self::InvalidIdentifier {
            kind,
            value: value.into(),
            reason,
        }
    }

    pub(crate) fn invalid_value(
        kind: &'static str,
        value: impl Into<String>,
        reason: &'static str,
    ) -> Self {
        Self::InvalidValue {
            kind,
            value: value.into(),
            reason,
        }
    }
}

impl From<io::Error> for LocusFsError {
    fn from(error: io::Error) -> Self {
        Self::Io(error.to_string())
    }
}
