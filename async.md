# Async Audit

The repository-wide `*.rs` audit originally found blocking standard-library
sync/threading primitives in graph state, FUSE state, invalidation, niri IPC,
client retry code, and real-FUSE tests. Those have been addressed in this
branch.

## Resolved

- `locusfs-graph/src/graph/memory.rs`
  - Replaced `std::sync::RwLock` with `tokio::sync::RwLock`.

- `locusfs-fuse/src/fs/inode.rs`
  - Replaced `std::sync::Mutex` with `tokio::sync::Mutex`.

- `locusfs-fuse/src/fs/watch.rs`
  - Replaced `std::sync::Mutex` with `tokio::sync::Mutex`.

- `locusfs-fuse/src/fs/filesystem.rs`
  - Replaced shared notify/watch/inode `std::sync::Mutex` use with
    `tokio::sync::Mutex`.
  - Converted helper methods that touch shared FUSE state to async.

- `locusfs-fuse/src/mount.rs`
  - Replaced notify-state `std::sync::Mutex` construction with
    `tokio::sync::Mutex`.

- `locusfs-fuse/src/invalidation.rs`
  - Replaced raw `std::thread` worker ownership with a Tokio-owned blocking task
    and Tokio `JoinHandle`.
  - Converted shared-state access to async locks.

- `plugins/niri/src/ipc.rs`
  - Replaced `std::sync::RwLock` with `tokio::sync::RwLock`.
  - Replaced raw `std::thread` event-stream ownership with Tokio
    `spawn_blocking` and Tokio `JoinHandle`.

- `plugins/niri/src/provider.rs`
  - Converted niri provider state access to await the Tokio `RwLock`.

- `locusfs-client/src/lib.rs`
  - Removed `std::thread::sleep` and `std::time::Duration` from the retry loop.

- `locusfs-fuse/src/fs/test.rs`
  - Removed `std::thread::sleep` from timestamp tests.

- `locusfs-fuse/tests/fuse_smoke.rs`
  - Replaced `std::sync::mpsc` with `tokio::sync::mpsc`.
  - Replaced `std::thread::spawn` with `tokio::task::spawn_blocking`.
  - Replaced std sleeps/deadlines with Tokio timers.

## Intentional Remaining Standard Types

- `std::sync::Arc`
  - Still used for shared ownership of provider/state handles.
  - The synchronization inside those handles is now Tokio-based where mutable
    shared state is involved.

- `std::time::{Duration, SystemTime}`
  - Still used for FUSE TTLs and file timestamps.

- `std::time::Instant`
  - Still used only for elapsed-time tracing.

No remaining `std::sync::Mutex`, `std::sync::RwLock`, `std::sync::mpsc`,
`std::thread`, `std::thread::sleep`, `std::sync::Condvar`, `std::sync::Once`,
`std::sync::OnceLock`, or `std::sync::atomic::*` usages were found in Rust
sources after the cleanup.
