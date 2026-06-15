use std::io;

use thiserror::Error;

/// Result type used by `locusfs-graph`.
pub type Result<T> = std::result::Result<T, GraphError>;

/// Typed error surface for graph-domain operations.
#[derive(Debug, Error)]
pub enum GraphError {
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
    #[error("unsupported graph operation: {operation}")]
    Unsupported { operation: &'static str },
    #[error("internal graph failure: {reason}")]
    Internal { reason: &'static str },
    #[error("invalid {kind} value {value:?}: {reason}")]
    InvalidValue {
        kind: &'static str,
        value: String,
        reason: &'static str,
    },
    #[error("I/O failed: {0}")]
    Io(String),
}

impl GraphError {
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
}

impl From<io::Error> for GraphError {
    fn from(error: io::Error) -> Self {
        Self::Io(error.to_string())
    }
}
