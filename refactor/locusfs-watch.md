# locusfs-watch Refactor Notes

## Current Role

`locusfs-watch` owns the typed watch event vocabulary and text encode/decode contract. With the default `client` feature, it also provides async filesystem helpers for opening a locusfs mount's `/watch` file, subscribing to a data path, waiting for events, and reading the watched value.

## Public Surface

- `watch/src/lib.rs` exposes the always-available protocol types: `WatchAction`, `WatchChange`, `WatchEvent`, `WatchState`, and `WatchValue`.
- With `client` enabled, `watch/src/lib.rs` also exports `Watch`, path helpers, existence/read helpers, directory-name reads, symlink resolution, and UTF-8 read helpers.
- `watch/Cargo.toml` gates `libc`, `tokio`, and `tracing` behind the `client` feature, keeping the protocol-only surface dependency-light with `default-features = false`.

## Step-By-Step File Walkthrough

1. `watch/Cargo.toml`: feature and dependency shape. Read first to understand the crate's public split between protocol-only and client-enabled use.
2. `watch/src/lib.rs`: crate documentation and curated re-exports. This is where external consumers discover the intended public API.
3. `watch/src/protocol.rs`: typed event/state/change vocabulary and the text wire format. This is the stable contract shared with FUSE and external clients.
4. `watch/src/protocol/test.rs`: round-trip coverage for the text protocol.
5. `watch/src/client.rs`: optional async client helper and `/watch` file handling. Read after the protocol because it consumes the typed events and defines user-facing read-after-watch policy.
6. `watch/src/client/test.rs`: coverage for mount-relative path conversion and missing-path retry timeout.
7. Cross-check later with `fuse/src/fs/watch.rs` and `bin/src/watch.rs`, because this crate's protocol is only half of the producer/consumer contract.

## Internal Structure

- `protocol.rs` is private and re-exported through `lib.rs`, which keeps the public paths stable.
- `client.rs` is private behind `#[cfg(feature = "client")]` and re-exported through `lib.rs`.
- Tests are colocated as sibling `test.rs` modules, matching the workspace's feature-directory test style.

## Behavior Summary

- Protocol encoding is newline-terminated text. Parsing trims input and dispatches `unset`, `set <payload>`, or change forms like `node changed <node>`, `property changed <key>`, and `relation removed <node> <relation>`.
- `WatchValue` currently infers `Path` versus `Property` from whether the `set` payload starts with `/`.
- The client finds a mount root by walking ancestors until it sees a file named `watch`, writes the logical path plus newline to that file, seeks back to the start, converts the file to nonblocking `AsyncFd`, then waits for readable events.
- Client reads retry `NotFound` every 25 ms until the configured timeout, giving FUSE time to materialize paths after an event.

## User Notes

- User wants the codebase reviewed top down, with the most public-facing API reviewed first.

## Findings

- Pending interactive review.

## Suspected Refactor Themes

- The `set` protocol's path-vs-property distinction may be ambiguous for property values that begin with `/`; this needs cross-checking against FUSE's producer semantics before treating it as a finding.
- `next_event` decodes one `WatchEvent` from whatever `drain_watch_events` returns. This needs cross-checking against FUSE read behavior to confirm whether multiple queued newline events can be returned in one drain.
- Protocol tests cover representative round trips but not invalid events, node change round trips, or whitespace/escaping boundaries.

## Tests And Verification

- Existing tests cover protocol round trips for unset, set path, set property, property change, relation change, logical path conversion, logical path rejection, and read timeout for a missing watched path.
- No verification run has been performed in this review session yet.

## Open Questions

- Is the watch text protocol considered a stable external contract, or is it allowed to change with the FUSE implementation during this refactor cycle?
- Should `WatchValue::Property` mean a property payload value, or should it be renamed to avoid confusion with property-change events?

