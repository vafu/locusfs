//! Reusable graph/filesystem contract for Locus.
//!
//! `locusfs-core` owns FUSE-independent concepts: graph identity, filesystem
//! path layout, typed values, mutations, validation hooks, provider contracts,
//! and typed errors. FUSE request handlers translate kernel operations into
//! this API; no FUSE types belong here.

mod error;
pub mod graph;
pub mod identity;
pub mod layout;
pub mod value;

pub use error::{LocusFsError, Result};
pub use graph::{GraphFilesystem, InMemoryGraph, ProjectEntry};
pub use identity::{NodeId, NodeKind, PathName, ProjectName, PropertyKey, RelationName};
pub use layout::{Layout, decode_segment, encode_segment};
pub use value::{LocusValue, ValueKind};
