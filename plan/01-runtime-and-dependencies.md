# Step 01: Runtime And Dependencies

## Reason

The current workspace has no async runtime dependency at the graph or binary boundary. `fuse3` requires either `tokio-runtime` or `async-io-runtime`, and RxRust/futures interop will be cleaner if the project standardizes on one runtime before changing provider traits.

Tokio is the conservative first choice because it has mature channels, signal handling, task spawning, `spawn_blocking`, and broad ecosystem support for future provider sources.

## Outcome

- Add workspace-level dependency choices for async support:
  - `tokio` with runtime, macros, sync, signal, and time features where needed.
  - `futures-core` / `futures-util` for stream traits and adapters where useful.
  - `async-trait` only if trait-object async provider ergonomics require it.
- Add `fuse3` to `locusfs-fuse` with the `tokio-runtime` feature.
- Keep `fuser` temporarily during the migration if it helps compile intermediate states, but make the intended end state explicit: `locusfs-fuse` should not depend on both FUSE crates after the port.
- Convert the `locusfs` binary entrypoint to own a Tokio runtime, either through `#[tokio::main]` or an explicit runtime builder.

## Way To Test

Run:

```sh
cargo check --workspace
```

At this step, no behavior should change. The check proves dependency features and edition-2024 compatibility are sound before API churn starts.

Also inspect the dependency tree for accidental duplicate runtime stacks:

```sh
cargo tree -p locusfs-fuse
cargo tree -p locusfs
```
