use fuse3::Errno;
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
    #[error("FUSE unmount failed: {0}")]
    Unmount(String),
}

pub(crate) fn graph_error_to_errno(error: GraphError) -> Errno {
    match error {
        GraphError::NotFound { .. } => errno(libc::ENOENT),
        GraphError::AlreadyExists { .. } => errno(libc::EEXIST),
        GraphError::InvalidIdentifier { .. }
        | GraphError::InvalidPathSegment { .. }
        | GraphError::InvalidEncoding { .. }
        | GraphError::InvalidValue { .. } => errno(libc::EINVAL),
        GraphError::Unsupported { .. } => errno(libc::ENOSYS),
        GraphError::Internal { .. } | GraphError::Io(_) => errno(libc::EIO),
    }
}

pub(crate) fn errno(code: libc::c_int) -> Errno {
    Errno::from(code)
}
