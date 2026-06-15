use std::collections::HashMap;

use fuser::Errno;

use super::entry::{FsEntry, NODES_INO, ROOT_INO};

#[derive(Debug)]
pub struct InodeTable {
    next: u64,
    by_ino: HashMap<u64, InodeEntry>,
    by_entry: HashMap<FsEntry, u64>,
}

#[derive(Debug)]
struct InodeEntry {
    entry: FsEntry,
    lookups: u64,
}

impl InodeTable {
    pub fn new() -> Self {
        let mut table = Self {
            next: 3,
            by_ino: HashMap::new(),
            by_entry: HashMap::new(),
        };
        table.insert(ROOT_INO, FsEntry::Root);
        table.insert(NODES_INO, FsEntry::NodesDir);
        table
    }

    pub fn entry(&self, ino: u64) -> Option<FsEntry> {
        self.by_ino.get(&ino).map(|record| record.entry.clone())
    }

    pub fn inode(&mut self, entry: FsEntry) -> std::result::Result<u64, Errno> {
        if let Some(ino) = self.by_entry.get(&entry) {
            return Ok(*ino);
        }
        let ino = self.next;
        self.next = self.next.checked_add(1).ok_or(Errno::EOVERFLOW)?;
        self.insert(ino, entry);
        Ok(ino)
    }

    pub fn acquire(&mut self, entry: FsEntry) -> std::result::Result<u64, Errno> {
        let ino = self.inode(entry)?;
        let Some(record) = self.by_ino.get_mut(&ino) else {
            return Err(Errno::EIO);
        };
        record.lookups = record.lookups.checked_add(1).ok_or(Errno::EOVERFLOW)?;
        Ok(ino)
    }

    pub fn forget(&mut self, ino: u64, nlookup: u64) {
        if matches!(ino, ROOT_INO | NODES_INO) {
            return;
        }

        let Some(record) = self.by_ino.get_mut(&ino) else {
            return;
        };

        if record.lookups > nlookup {
            record.lookups -= nlookup;
            return;
        }

        self.remove_inode(ino);
    }

    pub fn forget_entry(&mut self, entry: &FsEntry) {
        if let Some(ino) = self.by_entry.get(entry).copied() {
            self.remove_inode(ino);
        }
    }

    fn insert(&mut self, ino: u64, entry: FsEntry) {
        self.by_entry.insert(entry.clone(), ino);
        self.by_ino.insert(ino, InodeEntry { entry, lookups: 0 });
    }

    fn remove_inode(&mut self, ino: u64) {
        if let Some(record) = self.by_ino.remove(&ino) {
            self.by_entry.remove(&record.entry);
        }
    }
}
