//! FUSE adapter boundary for Locus.
//!
//! This crate owns mount lifecycle, public filesystem layout, and kernel
//! filesystem request translation. Graph semantics stay in `locusfs-graph`.

mod error;
mod fs;
pub mod layout;
mod mount;

pub(crate) use error::graph_error_to_errno;
pub use error::{FuseError, Result};
pub use fs::LocusFs;
pub use layout::{Layout, decode_segment, encode_segment};
pub use mount::{FuseMount, FuseMountConfig, mount};
