//! Shared watch protocol and optional client helpers for locusfs mounts.

mod protocol;

pub use protocol::{WatchAction, WatchChange, WatchEvent, WatchState, WatchValue};

#[cfg(feature = "client")]
mod client;

#[cfg(feature = "client")]
pub use client::{
    Watch, absolute_path, exists, find_mount_root, logical_watch_path, read, read_dir_names,
    read_link, read_to_string,
};
