# Step 03: Provider Adapters

## Reason

Not every provider needs true async internals immediately. The in-memory provider is naturally synchronous, and some future providers may expose blocking APIs first. The graph contract should be async, but provider authors need a low-friction path that does not force busy executor blocking.

## Outcome

- Port `InMemoryProvider` to the async provider traits with immediate-ready async methods.
- Port `TracedProvider` to wrap async provider calls and preserve useful tracing spans/events.
- Decide whether to use:
  - direct `async fn` trait methods through `async-trait`, or
  - explicit boxed futures for object safety.
- Add a small adapter pattern for blocking providers where useful, likely named around `BlockingProvider` or documented as `spawn_blocking` usage.
- Do not over-abstract before there are multiple real blocking providers; keep the adapter minimal.

## Way To Test

Run:

```sh
cargo test -p locusfs-graph
```

Review generated compiler diagnostics for accidental `Send` issues. Provider futures used by FUSE should be `Send` unless the chosen runtime architecture intentionally runs them on a local executor.

Add or update graph tests so the in-memory provider still covers:

- create/remove node
- property set/read/remove
- relation set/read/remove
- missing provider errors
- change emission after mutation
