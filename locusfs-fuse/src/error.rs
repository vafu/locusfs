use fuser::Errno;
use locusfs_graph::GraphError;
use thiserror::Error;

pub type Result<T> = std::result::Result<T, FuseError>;

/// Errors from FUSE mount setup.
#[derive(Debug, Error)]
pub enum FuseError {
    #[error(transparent)]
    Graph(#[from] GraphError),
    #[error("FUSE mount failed: {0}")]
    Mount(String),
}

pub(crate) fn graph_error_to_errno(error: GraphError) -> Errno {
    match error {
        GraphError::NotFound { .. } => Errno::ENOENT,
        GraphError::InvalidIdentifier { .. }
        | GraphError::InvalidPathSegment { .. }
        | GraphError::InvalidEncoding { .. }
        | GraphError::InvalidValue { .. } => Errno::EINVAL,
        GraphError::Unsupported { .. } => Errno::ENOSYS,
        GraphError::Internal { .. } | GraphError::Io(_) => Errno::EIO,
    }
}
