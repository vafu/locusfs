mod attr;
mod entry;
mod filesystem;
mod inode;
mod name;
mod value;

pub use filesystem::LocusFs;

#[cfg(test)]
use entry::{FsEntry, relation_link_target};
#[cfg(test)]
use inode::InodeTable;
#[cfg(test)]
use value::slice_for_read;

#[cfg(test)]
mod test;
