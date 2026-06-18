# Fuse And Client Notes

## Current Role

`locusfs-fuse` maps graph state into a FUSE filesystem and owns lookup, attributes, directory listing, reads/writes, symlinks, inode cache, invalidation, and `/watch`.

`locusfs-client` provides async reads and a mounted-path watcher abstraction.

## Findings

- Addressed 2026-06-18: watch delivery no longer suppresses kernel invalidation when poll waiters are notified.
- Property and relation names share one node-local namespace. A same-name property and relation becomes `EIO`.
- Relation representation changes by cardinality: one target is a symlink, multiple targets become a directory.
- Addressed 2026-06-18: `WatchSubjectKey` is now an alias for `GraphWatchTarget`.
- Addressed 2026-06-18: `WatchEvent` is now an alias for `GraphWatchEvent`.
- FUSE watch fanout overlaps with graph fallback watch matching.
- Addressed 2026-06-18: `WatchHandle.pending_events` is bounded.
- Addressed 2026-06-18: `InodeTable.times` is cleaned when inode state is removed.
- Addressed 2026-06-18: `locusfs-client::Watch` has timeout variants for retrying reads.
- Real cross-process FUSE/watch coverage is thin; the smoke test is ignored by default.

## Refactor Plan

1. Done: decouple watch wakeups from kernel invalidation. Always invalidate affected entries, then notify watchers.
2. Done: reuse graph watch target/event types in FUSE where practical.
3. Keep FUSE-specific state limited to file handles, poll handles, dependency retargeting, and pending delivery.
4. Decide the filesystem shape for relations. If the current symlink/directory duality remains, add explicit transition tests.
5. Done: bound pending watch events with oldest-drop policy.
6. Done: clean inode timestamp state when entries are forgotten or removed.
7. Done: add client read timeout/cancellation options for disappeared paths.
8. Extend the ignored real FUSE smoke test to cover `Watch::open -> wait_event -> read`.

## Tests And Verification

- `cargo test -p locusfs-fuse -p locusfs-client`
- Host FUSE: `cargo test -p locusfs-fuse --test fuse_smoke -- --ignored`
- New unit tests for invalidation while watch waiters exist, queue bounds, timestamp cleanup, and relation cardinality transitions.
