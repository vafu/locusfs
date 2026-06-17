# Step 04: Async Change Streams

## Reason

Graph changes currently use `std::sync::mpsc::Receiver`, and FUSE invalidation consumes them from a dedicated blocking thread. Async FUSE should receive graph changes through an async stream/channel so invalidation can run as a task and participate in runtime shutdown.

RxRust integration also benefits from graph changes being stream-shaped instead of blocking-receiver-shaped.

## Outcome

- Replace `DynamicGraph` change subscriptions with an async-friendly channel.
- Prefer `tokio::sync::broadcast` if each subscriber should independently see every change.
- Consider `tokio::sync::watch` only for state snapshots, not event streams.
- Expose a subscription type that can be adapted into `Stream<Item = GraphChange>` for RxRust/futures consumers.
- Preserve best-effort subscriber cleanup behavior.
- Update existing tests that read changes from `mpsc::Receiver` to async tests.

## Way To Test

Run:

```sh
cargo test -p locusfs-graph
```

Add or update tests for:

- multiple subscribers receiving the same mutation event.
- closed subscribers not breaking future emissions.
- mutations emitting changes after provider mutation succeeds, not before.

If a stream adapter is added, include a small test that awaits one emitted change through the stream interface.
