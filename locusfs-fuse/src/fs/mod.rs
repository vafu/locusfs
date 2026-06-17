mod attr;
mod directory;
mod entry;
mod filesystem;
mod inode;
mod name;
mod resolve;
mod value;
mod watch;

pub use filesystem::LocusFs;

pub(crate) use entry::FsEntry;
pub(crate) use filesystem::SharedKernelNotify;
pub(crate) use inode::{InodeTable, SharedInodeTable};
pub(crate) use resolve::resolve_watch_path;
pub(crate) use watch::{SharedWatchRegistry, WatchEvent, WatchKey, WatchRegistry};

#[cfg(test)]
use entry::{direct_relation_link_target, nested_relation_link_target};
#[cfg(test)]
use value::slice_for_read;

#[cfg(test)]
mod test;
