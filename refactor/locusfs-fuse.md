# locusfs-fuse Refactor Notes

## Current Role

Owns FUSE mount lifecycle, filesystem layout, inode/timestamp bookkeeping, graph-to-filesystem request translation, kernel cache invalidation, and FUSE poll integration.

## Public Surface

- `lib.rs`: exports mount API, `LocusFs`, and layout helpers.
- `mount.rs`: mount configuration and FUSE session setup.
- `invalidation.rs`: graph-change invalidation, inode timestamp updates, and watcher wakeups.
- `fs/mod.rs`: crate-local filesystem module API.
- `fs/filesystem.rs`: main `fuser::Filesystem` implementation.
- `fs/entry.rs`: virtual filesystem entry model.
- `fs/inode.rs`: inode table, lookup counts, timestamps.
- `fs/watch.rs`: direct property polling and root `/watch` registry.
- `layout/mod.rs`: public path/segment layout helpers.

## Step-By-Step File Walkthrough

1. `src/lib.rs` and `src/mount.rs`: public runtime entrypoints and graph change wiring.
2. `src/fs/entry.rs`: virtual tree shape and relation symlink targets.
3. `src/fs/inode.rs` and `src/fs/attr.rs`: stable inode and attribute/timestamp behavior.
4. `src/fs/filesystem.rs`: FUSE operation behavior and graph data flow.
5. `src/fs/watch.rs`: poll handles, per-open event queues, and dependency wakeups.
6. `src/layout/mod.rs`: path encoding and reserved segment policy.
7. `tests/fuse_smoke.rs` and `src/fs/test.rs`: verification of real FUSE and internal state.

## Internal Structure

Pending review.

## Behavior Summary

Pending review.

## User Notes

- Preferred public watch endpoint is `/watch`, not `/meta/watch`.
- Split graph-change invalidation out of `mount.rs` into a separate file/module.
- Remove `/props`; prefer a flatter node layout where properties and relations live directly under the node. Schema should prevent property/relation name clashes later.
- Everything exposed under a node is property-like from the user view; relation names and property names must not duplicate. Until schema/codegen enforces this at compile time, duplicate names should be a hard runtime error.
- Expose the watch control path from `Layout` because external consumers should be able to discover/use it.
- Current symlink target generation using fixed `../..` / `../../..` depth is brittle; review whether relation symlinks should target paths relative to mount root instead.
- `filesystem.rs` is enormous and should be split into smaller chunks. Some current branches may become irrelevant after the flat property/watch approach settles.
- Symlink handling should support explicit user-installed symlinks eventually; all normal symlink forms should be usable where possible.
- `/watch` should be owner-writable only.
- Manual `mkdir` for relation directories should be supported.
- `/watch` should use one open fd per watched path for the first design. Consumers that need many paths should open `/watch` many times and use `epoll`.
- `/watch` input can stay raw path writes.
- `/watch` output can be a minimal readiness marker because the fd maps to exactly one watched path.

## Findings

- Relation symlink target generation is depth-sensitive. This works for current direct/nested link shapes, but will become brittle if layout depth changes.
- `filesystem.rs` owns too many responsibilities: lookup, mutation semantics, data reads, watch dependency parsing, and directory listing.

## Refactor Plan

1. Move invalidation helpers from `mount.rs` into a dedicated module, likely `src/invalidation.rs` or `src/mount/invalidation.rs`, keeping `mount.rs` focused on session setup.
2. Flatten the node layout by removing `PropsDir`/`PROPS_DIR_NAME` and making property files direct children of `NodeDir`.
3. Add runtime collision detection for duplicate flat property/relation names under the same node.
4. Expose the public watch control path from `Layout` as `/watch`.
5. Revisit relation symlink targets and prefer a layout-stable target strategy if FUSE/kernel behavior supports it.
6. Change `/watch` permissions to owner-write.
7. Rename `PollRegistry` to `WatchRegistry`.
8. Rework `/watch` around one fd per watched path:
   - `open("/watch")` creates one `WatchHandle` slot.
   - writing a raw path configures that fd's watched path.
   - each handle stores `original_path`, current `resolved_path`, crossed `dependencies`, `pending: bool`, and drained `poll_handles`.
   - shared `Subject`s are keyed by resolved absolute/canonical data path and contain attached file handles.
   - graph property/data changes mark every handle attached to the resolved subject as pending and wake their poll handles.
   - graph relation/symlink changes find affected handles through `dependency_index`, re-resolve their original path, move them between subjects, mark them pending, and wake them.
   - `read` drains the pending flag; `poll` is readable iff pending is true.
   - `release` detaches the fd from its subject/dependency indexes and removes empty subjects.
9. Split `filesystem.rs` into focused modules after deciding which branches survive flat layout.
10. Preserve manual `mkdir` support for relation directory creation in the flat layout.
11. Update path lookup, directory listing, create/unlink, watch dependency parsing, smoke tests, and CLI examples to use flat property paths.

## Implemented In This Pass

- Moved graph-change invalidation out of `mount.rs` into `src/invalidation.rs`.
- Removed `PropsDir` and flattened property files under node directories.
- Added runtime collision errors for duplicate property/relation names in flat node directories.
- Exposed `Layout::watch()` and switched the public watch endpoint to `/watch`.
- Changed `/watch` to owner-only permissions.
- Renamed `PollRegistry` to `WatchRegistry`.
- Reworked `/watch` around one fd per watched path, shared resolved subjects, retarget dependency indexing, and per-fd pending readiness.
- Updated the CLI watcher to discover and write to `/watch`.
- Updated unit tests and real FUSE smoke coverage for flat paths and `/watch`.

## Watch Design

Use one `/watch` file descriptor per watched path. Consumers that need many paths should open `/watch` many times and register those fds with `epoll`.

```rust
struct WatchRegistry {
    next_handle: u64,
    handles: HashMap<FileHandle, WatchHandle>,
    subjects: HashMap<ResolvedPath, WatchSubject>,
    dependency_index: HashMap<WatchKey, Vec<FileHandle>>,
}
```

`handles` is per-open-fd state. `subjects` is shared resolved data state. `dependency_index` maps retarget dependencies to handles that must re-resolve.

```rust
struct WatchHandle {
    original_path: WatchPath,
    resolved_path: ResolvedPath,
    dependencies: Vec<WatchKey>,
    pending: bool,
    poll_handles: Vec<PollHandle>,
}
```

`original_path` is the path the consumer wrote to `/watch`. `resolved_path` is the current concrete data path after resolving relation/symlink hops. `dependencies` are only the intermediate relation/symlink hops that can retarget the original path; static direct paths normally have an empty dependency list. `pending` is the drainable output state for poll/read. `poll_handles` are single-use kernel wake tokens and must be drained after notification.

```rust
struct WatchSubject {
    resolved_path: ResolvedPath,
    watchers: Vec<FileHandle>,
}
```

Multiple handles can share one subject when different original paths resolve to the same concrete data path. Example: `/context/selected/window/title` and `/window/10/title` share the same subject while selected window is `10`.

On data/property change, mark all handles attached to the resolved subject as pending and wake their poll handles. On relation/symlink change, use `dependency_index` to find affected handles, detach them from old subjects, re-resolve their original paths, attach them to new subjects, update dependencies, mark them pending, and wake them.

## Tests And Verification

Pending review.

## Open Questions

- Does `filesystem.rs` need further splitting after the layout/watch changes already made?
- Should watch dependencies over-wake on node changes by design, or become more precise?
- Should `/watch` read output be `change\n`, an empty read with readiness only, or a small binary marker?
