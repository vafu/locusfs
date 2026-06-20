use std::collections::HashMap;
use std::sync::Arc;

use fuse3::Errno;
use tokio::sync::Mutex;

use super::attr::EntryTimes;
use super::entry::{FsEntry, ROOT_INO};
use crate::errno;

pub type SharedInodeTable = Arc<Mutex<InodeTable>>;

#[derive(Debug)]
pub struct InodeTable {
    next: u64,
    by_ino: HashMap<u64, InodeEntry>,
    by_entry: HashMap<FsEntry, u64>,
    times: HashMap<FsEntry, EntryTimes>,
}

#[derive(Debug)]
struct InodeEntry {
    entry: FsEntry,
    lookups: u64,
}

impl InodeTable {
    pub fn shared() -> SharedInodeTable {
        Arc::new(Mutex::new(Self::new()))
    }

    pub fn new() -> Self {
        let mut table = Self {
            next: 3,
            by_ino: HashMap::new(),
            by_entry: HashMap::new(),
            times: HashMap::new(),
        };
        table.insert(ROOT_INO, FsEntry::Root);
        table
    }

    pub fn entry(&self, ino: u64) -> Option<FsEntry> {
        self.by_ino.get(&ino).map(|record| record.entry.clone())
    }

    pub fn known_inode(&self, entry: &FsEntry) -> Option<u64> {
        self.by_entry.get(entry).copied()
    }

    pub fn entries(&self) -> Vec<(FsEntry, u64)> {
        self.by_entry
            .iter()
            .map(|(entry, ino)| (entry.clone(), *ino))
            .collect()
    }

    pub fn times(&mut self, entry: &FsEntry) -> EntryTimes {
        *self
            .times
            .entry(entry.clone())
            .or_insert_with(EntryTimes::now)
    }

    pub fn touch(&mut self, entry: &FsEntry) -> Option<u64> {
        self.times
            .entry(entry.clone())
            .or_insert_with(EntryTimes::now)
            .touch();
        self.known_inode(entry)
    }

    pub fn inode(&mut self, entry: FsEntry) -> std::result::Result<u64, Errno> {
        if let Some(ino) = self.by_entry.get(&entry) {
            return Ok(*ino);
        }
        let ino = self.next;
        self.next = self.next.checked_add(1).ok_or(errno(libc::EOVERFLOW))?;
        self.insert(ino, entry);
        Ok(ino)
    }

    pub fn acquire(&mut self, entry: FsEntry) -> std::result::Result<u64, Errno> {
        let ino = self.inode(entry)?;
        let Some(record) = self.by_ino.get_mut(&ino) else {
            return Err(errno(libc::EIO));
        };
        record.lookups = record
            .lookups
            .checked_add(1)
            .ok_or(errno(libc::EOVERFLOW))?;
        Ok(ino)
    }

    pub fn forget(&mut self, ino: u64, nlookup: u64) {
        if ino == ROOT_INO {
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

    #[cfg(test)]
    pub fn times_len(&self) -> usize {
        self.times.len()
    }

    fn insert(&mut self, ino: u64, entry: FsEntry) {
        self.by_entry.insert(entry.clone(), ino);
        self.by_ino.insert(ino, InodeEntry { entry, lookups: 0 });
    }

    fn remove_inode(&mut self, ino: u64) {
        if let Some(record) = self.by_ino.remove(&ino) {
            self.by_entry.remove(&record.entry);
            self.times.remove(&record.entry);
        }
    }
}
