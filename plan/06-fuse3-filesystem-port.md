# Step 06: Fuse3 Filesystem Port

## Reason

The current `locusfs-fuse/src/fs/filesystem.rs` implements all user-visible filesystem behavior through `fuser::Filesystem` reply callbacks. `fuse3::raw::Filesystem` uses async methods returning reply values and directory streams, so this is the main mechanical port.

The goal is to preserve the current public layout and behavior while changing the execution model.

## Outcome

- Port the inode-based implementation to `fuse3::raw::Filesystem`; avoid the path-based API because LocusFS already owns inode allocation, lookup counts, timestamps, invalidation, and relation symlink identity.
- Map fuser types to fuse3 equivalents:
  - inode wrappers to raw `Inode`/`u64`
  - `FileAttr` representation
  - open flags and file handles
  - entry, attr, data, write, create, open, and directory replies
- Convert filesystem helper methods that call graph APIs into async methods.
- Ensure no `std::sync::Mutex`/`RwLock` guard is held across `.await`; convert locks to async locks only when needed.
- Port directory listing to the stream type expected by `fuse3`.
- Preserve behavior for:
  - root `/watch`
  - kind directories
  - node directories
  - property files
  - direct relation symlinks
  - relation directories and nested target symlinks
  - create/write/unlink/rmdir/mkdir/symlink semantics

## Way To Test

Run:

```sh
cargo check -p locusfs-fuse
cargo test -p locusfs-fuse
```

Manual smoke checks:

```sh
cargo run -p locusfs -- /tmp/locusfs-fuse3-smoke
find /tmp/locusfs-fuse3-smoke -maxdepth 3 -print
```

Exercise:

- `mkdir` node creation.
- property file create/write/read.
- relation symlink creation/readlink.
- relation directory listing.
- property/relation unlink and node removal.
