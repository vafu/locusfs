# Step 09: Test Hardening And Cleanup

## Reason

The async swap changes concurrency, lifecycle, and kernel-facing behavior. Passing `cargo check` is not enough; the project needs focused tests around the contracts most likely to regress.

This is also the point where temporary compatibility code, duplicate FUSE dependencies, and old sync paths should be removed.

## Outcome

- Remove the old `fuser` dependency and any temporary compatibility modules once fuse3 is serving all required operations.
- Ensure unit tests use async test helpers where graph APIs are async.
- Strengthen FUSE smoke tests around:
  - mount/unmount lifecycle
  - root and node directory listing
  - property read/write/truncate behavior
  - relation symlink behavior
  - kernel cache invalidation after graph changes
  - `/watch` poll wakeup and read-drain behavior
  - retargeting watches through relation changes
- Add timeout bounds to async tests so hangs fail clearly.
- Update refactor docs or README notes to describe the async provider contract and blocking-provider guidance.

## Way To Test

Run the full suite:

```sh
cargo fmt --check
cargo check --workspace
cargo test --workspace
cargo test -p locusfs-fuse --test fuse_smoke -- --ignored
```

Also run targeted manual validation for `/watch` with `epoll` tooling or `scripts/epoll_watch.py` if it remains in the repo:

```sh
python3 scripts/epoll_watch.py /tmp/locusfs-fuse3-smoke/watch /tmp/locusfs-fuse3-smoke/node/example/title
```

Expected result: watch fds wake on graph changes, drain on read, and do not spin when idle.
