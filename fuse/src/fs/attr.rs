use std::time::{Duration, SystemTime};

use fuse3::raw::reply::FileAttr;
use fuse3::{FileType, Timestamp};

pub const TTL: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EntryTimes {
    pub accessed: SystemTime,
    pub modified: SystemTime,
    pub changed: SystemTime,
    pub created: SystemTime,
}

impl EntryTimes {
    pub fn now() -> Self {
        let now = SystemTime::now();
        Self {
            accessed: now,
            modified: now,
            changed: now,
            created: now,
        }
    }

    pub fn touch(&mut self) {
        let now = SystemTime::now();
        self.modified = now;
        self.changed = now;
    }
}

pub fn file_attr(ino: u64, kind: FileType, perm: u16, size: u64, times: EntryTimes) -> FileAttr {
    FileAttr {
        ino,
        size,
        blocks: size.div_ceil(512),
        atime: Timestamp::from(times.accessed),
        mtime: Timestamp::from(times.modified),
        ctime: Timestamp::from(times.changed),
        #[cfg(target_os = "macos")]
        crtime: Timestamp::from(times.created),
        kind,
        perm,
        // TODO: compute directory link counts from child directory entries instead of using a
        // single simplified value for every FUSE entry.
        nlink: 1,
        uid: unsafe { libc::getuid() },
        gid: unsafe { libc::getgid() },
        rdev: 0,
        blksize: 4096,
        #[cfg(target_os = "macos")]
        flags: 0,
    }
}
