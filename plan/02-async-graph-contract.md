# Step 02: Async Graph Contract

## Reason

Async end-to-end support starts at `locusfs-graph`, not at FUSE. The current provider traits in `locusfs-graph/src/graph/mod.rs` are synchronous, so any slow provider must either block the caller or hide its own runtime/threading internally.

Provider sources should be able to await IPC, sockets, DBus, file IO, timers, or RxRust-driven state without blocking FUSE request handling.

## Outcome

- Convert the public graph provider traits to async-returning APIs:
  - `NodeProvider`
  - `NodeMutationProvider`
  - `PropertyProvider`
  - `PropertyMutationProvider`
  - `RelationProvider`
  - `RelationMutationProvider`
- Preserve trait-object registration in `DynamicGraph`.
- Keep `kind(&self) -> &NodeKind` synchronous unless there is a concrete reason to make it async.
- Convert `DynamicGraph` public read/mutation methods to async.
- Avoid holding provider registry locks across `.await`; clone the selected `Arc<dyn Provider>` while holding the lock, then drop the lock before awaiting provider work.
- Keep graph errors typed as `GraphError`.

## Way To Test

Run:

```sh
cargo check -p locusfs-graph
cargo test -p locusfs-graph
```

Targeted checks:

- provider registry lookups do not hold `RwLock` guards across `.await`.
- mutation methods still emit the same `GraphChange` variants after successful mutation.
- `remove_node` still removes inbound links before removing the node.
