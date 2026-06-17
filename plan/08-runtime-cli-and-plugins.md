# Step 08: Runtime CLI And Plugins

## Reason

Once graph and FUSE are async, the binary and plugins should stop pretending they are synchronous. Runtime assembly should own task lifecycles, shutdown, and plugin registration. Blocking plugins should be isolated so they do not block async FUSE request handling.

## Outcome

- Convert `locusfs/src/main.rs` to async runtime ownership.
- Replace signal handling based on atomics and sleep loops with async signal handling.
- Make `mount` awaitable or return an async lifecycle handle that can be selected against shutdown.
- Convert plugin registration to async where provider startup performs IO.
- Port the Niri plugin:
  - initial IPC state fetch can use `spawn_blocking` if `niri_ipc` remains blocking.
  - event stream can remain on a dedicated blocking thread at first, but must bridge into async graph change emission cleanly.
  - if an async Niri IPC path is available later, replace the bridge with a native async task.
- Ensure plugin state locks are not held across async graph emission unless explicitly safe.

## Way To Test

Run:

```sh
cargo check --workspace
cargo test --workspace
```

Manual checks:

```sh
cargo run -p locusfs -- /tmp/locusfs-fuse3-smoke
Ctrl-C
mountpoint -q /tmp/locusfs-fuse3-smoke
```

Expected result: Ctrl-C exits cleanly and the mountpoint is no longer mounted.

With Niri available, run with tracing enabled and confirm provider startup, event ingestion, and graph changes continue to flow.
