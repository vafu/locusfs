# Step 07: Watch, Poll, And Invalidation

## Reason

Reactive behavior is the riskiest part of the swap. The current implementation stores FUSE poll handles and uses a `Notifier` to wake pollers and invalidate kernel cache entries after graph changes. The existing user model is one `/watch` fd per watched path, which should remain intact.

`fuse3` has notify APIs, but its README marks `poll` as unstable. This step should be treated as a validation spike plus production port.

## Outcome

- Port `WatchRegistry` away from fuser-specific types:
  - file handles become raw `u64` or a local newtype.
  - poll handles become the fuse3 notify/poll token representation.
  - poll event flags are translated at the FUSE boundary.
- Port invalidation to the fuse3 notify API:
  - invalidate entry
  - invalidate inode
  - delete directory entry
  - wake poll handles
- Convert the invalidation worker from a blocking thread to an async task consuming graph changes.
- Preserve watch semantics:
  - opening `/watch` creates per-fd state.
  - writing a path configures that fd.
  - poll is readable when the watched subject has a pending change.
  - read drains pending state and returns the marker payload.
  - relation retargets re-resolve dependent watches.
  - release detaches the fd and drains stale poll handles.
- If fuse3 poll is insufficient, document the exact blocker and evaluate fallback options:
  - keep `fuser` temporarily for FUSE while making graph/providers async.
  - replace FUSE poll watch endpoint with another async notification surface.
  - patch or upstream a fuse3 poll fix.

## Way To Test

Run:

```sh
cargo test -p locusfs-fuse
```

Run real FUSE smoke tests where supported:

```sh
cargo test -p locusfs-fuse --test fuse_smoke -- --ignored
```

Manual watch checks:

```sh
cargo run -p locusfs -- /tmp/locusfs-fuse3-smoke
cargo run -p locusfs -- --watch /tmp/locusfs-fuse3-smoke/node/example/title
```

In another shell, mutate the watched property and confirm the watcher wakes exactly after changes, not through polling sleeps.
