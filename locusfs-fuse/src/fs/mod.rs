mod attr;
mod entry;
mod filesystem;
mod inode;
mod name;
mod value;
mod watch;

pub use filesystem::LocusFs;

pub(crate) use entry::FsEntry;
pub(crate) use filesystem::resolve_watch_path;
pub(crate) use inode::{InodeTable, SharedInodeTable};
pub(crate) use watch::{SharedWatchRegistry, WatchKey, WatchRegistry};

#[cfg(test)]
use entry::{direct_relation_link_target, nested_relation_link_target};
#[cfg(test)]
use value::slice_for_read;

#[cfg(test)]
mod test;
