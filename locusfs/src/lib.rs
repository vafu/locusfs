//! Top-level facade for the Locus filesystem API.

pub mod config;
pub mod plugin;

pub use locusfs_fuse as fuse;
pub use locusfs_graph as graph;
pub use locusfs_watch as watch;
