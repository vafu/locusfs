use std::time::{Duration, SystemTime};

use fuser::{FileAttr, FileType, INodeNo};

pub const TTL: Duration = Duration::from_millis(250);

pub fn file_attr(ino: u64, kind: FileType, perm: u16, size: u64) -> FileAttr {
    // TODO: source atime/mtime/ctime/crtime from provider-backed metadata once graph entries
    // expose stable filesystem timestamps.
    let now = SystemTime::now();
    FileAttr {
        ino: INodeNo(ino),
        size,
        blocks: size.div_ceil(512),
        atime: now,
        mtime: now,
        ctime: now,
        crtime: now,
        kind,
        perm,
        // TODO: compute directory link counts from child directory entries instead of using a
        // single simplified value for every FUSE entry.
        nlink: 1,
        uid: unsafe { libc::getuid() },
        gid: unsafe { libc::getgid() },
        rdev: 0,
        blksize: 4096,
        flags: 0,
    }
}
