# Fuse3 Async Swap Overview

## Reason

LocusFS is expected to host many future plugins and provider sources. The current graph and FUSE boundary are synchronous: graph provider traits return immediate `Result<T>`, change delivery uses `std::sync::mpsc`, `locusfs-fuse` implements `fuser::Filesystem`, and plugins such as Niri bridge blocking IPC through threads.

That shape works for the current small implementation, but it makes async-native consumers and providers awkward. The main library consumer is expected to be RxRust, which composes naturally with futures and async/await, so async should be a first-class graph/provider contract rather than a wrapper around blocking calls.

## Outcome

The refactor should produce an async-first runtime:

- `locusfs-graph` exposes async provider traits and async graph methods.
- graph changes are delivered through async-friendly streams/channels.
- `locusfs-fuse` uses `fuse3::raw` with a selected async runtime, initially Tokio unless a concrete blocker appears.
- watch and invalidation behavior remain event-driven and compatible with `/watch` fd-per-path usage.
- existing sync or blocking providers can still be adapted without forcing all plugins to block the async executor.
- the binary runtime owns executor startup and graceful shutdown.

## Way To Test

The final async swap is considered successful when these pass from the new worktree:

```sh
cargo check --workspace
cargo test --workspace
cargo test -p locusfs-fuse --test fuse_smoke -- --ignored
```

The smoke test command may require local FUSE privileges and should be run where FUSE mounts are available.

## Step Order

1. `01-runtime-and-dependencies.md`
2. `02-async-graph-contract.md`
3. `03-provider-adapters.md`
4. `04-change-streams.md`
5. `05-fuse3-mount-skeleton.md`
6. `06-fuse3-filesystem-port.md`
7. `07-watch-poll-invalidation.md`
8. `08-runtime-cli-and-plugins.md`
9. `09-test-hardening.md`

Each step is intended to leave the repo in a compiling or narrowly diagnosable state. The highest-risk step is watch/poll/invalidation because `fuse3` marks `poll` as unstable, while the current user-facing reactive behavior depends on poll wakeups.
