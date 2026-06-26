# Watch API Review

## Current Role

This review covers the public `locusfs-watch` API and the immediate watch producer/consumer boundaries:

- `watch/` in full.
- Watch-producing FUSE registry and invalidation code in `fuse/src/fs/watch.rs` and `fuse/src/invalidation.rs`.
- The CLI watch consumer in `bin/src/watch.rs`.
- Watch contract tests in `watch/src/*/test.rs`, `fuse/src/fs/test.rs`, and the ignored real FUSE smoke test in `fuse/tests/fuse_smoke.rs`.

The current design makes `locusfs-watch` the public crate for the typed text protocol and optional async client helpers. FUSE is the only in-workspace producer of the text protocol, and the binary is the only in-workspace consumer of the async client. `cargo tree -i locusfs-watch` confirms the workspace consumers are `locusfs-fuse` and `locusfs-bin`.

## Public API And Entrypoints

- `watch/Cargo.toml:6-13` defines the split between protocol-only use and the default `client` feature. `locusfs-fuse` depends on `locusfs-watch` with `default-features = false` at `fuse/Cargo.toml:13-14`, while `locusfs-bin` uses the default client feature at `bin/Cargo.toml:16`.
- `watch/src/lib.rs:1-6` documents the crate-level role and feature split.
- `watch/src/lib.rs:8-10` privately owns `protocol` and re-exports `WatchAction`, `WatchChange`, `WatchEvent`, `WatchState`, and `WatchValue` as the stable discovery surface.
- `watch/src/lib.rs:12-19` privately owns `client` behind `#[cfg(feature = "client")]` and re-exports `Watch`, path helpers, read helpers, and symlink helpers.
- `watch/src/protocol.rs:4-46` defines the public event vocabulary:
  - `WatchEvent::State` and `WatchEvent::Change`.
  - `WatchState::Unset` and `WatchState::Set`.
  - `WatchValue::Path` and `WatchValue::Property`.
  - `WatchChange::{Change, Node, Property, Relation}`.
  - `WatchAction::{Added, Changed, Removed}`.
- `watch/src/protocol.rs:48-80`, `watch/src/protocol.rs:83-98`, and `watch/src/protocol.rs:100-179` define public text encode/decode methods and action display.
- `watch/src/client.rs:16-52` exposes convenience reads: bytes, string, directory names, symlink target resolution, and existence.
- `watch/src/client.rs:71-130` exposes path helpers: `absolute_path`, `find_mount_root`, and `logical_watch_path`.
- `watch/src/client.rs:132-284` exposes `Watch`, including `open`, `open_with_parts`, path accessors, event waiting, state waiting, raw event reads, watched-path reads, and wait-then-read helpers.
- FUSE exposes the runtime filesystem entrypoint as a root `watch` file through `FsEntry::WatchFile`, opened by `fuse/src/fs/filesystem.rs:596-604`, read by `fuse/src/fs/filesystem.rs:649-679`, written by `fuse/src/fs/filesystem.rs:951-1010`, and polled by `fuse/src/fs/filesystem.rs:753-770`.
- `fuse/src/fs/resolve.rs:15-23` parses the subscription write payload, and `fuse/src/fs/resolve.rs:25-151` resolves it into a `WatchTarget`.
- `fuse/src/fs/watch.rs:130-740` is the concrete registry that tracks open watch handles, pending events, dependency indexes, poll handles, state coalescing, and change fanout.
- `fuse/src/invalidation.rs:77-342` maps `GraphChange` events into FUSE invalidations, watch event fanout, state refresh, and relation retargeting.
- `bin/src/watch.rs:7-18` is the CLI consumer: it opens a `Watch`, prints raw events for directory paths, and uses read-after-watch behavior for non-directories.

## Step-By-Step Walkthrough

1. A client opens a path through `Watch::open` at `watch/src/client.rs:141-148`.
2. `Watch::open` converts the input to an absolute path, finds the nearest ancestor with a file named `watch`, derives a mount-relative logical path, and calls `open_with_parts`.
3. `open_with_parts` opens `<mount_root>/watch` read-write, writes `logical_path + "\n"`, logs it, seeks back to offset 0, converts the file into a nonblocking `OwnedFd`, and wraps it in `AsyncFd` at `watch/src/client.rs:150-175`.
4. FUSE receives that write in `LocusFs::write_owned` for `FsEntry::WatchFile` at `fuse/src/fs/filesystem.rs:981-1010`.
5. FUSE parses a UTF-8 absolute subscription path with `parse_watch_subscription` at `fuse/src/fs/resolve.rs:15-23`.
6. `resolve_watch_path` decides whether the watch is a change stream or a state stream. The mode is mostly driven by `path.ends_with('/')` at `fuse/src/fs/resolve.rs:29`, with special cases for kind and relation directories.
7. If the target has no dependencies and is a change stream, FUSE opens a graph-native watch and spawns a forwarder at `fuse/src/fs/filesystem.rs:984-1008` and `fuse/src/fs/filesystem.rs:1017-1033`.
8. Otherwise FUSE records the watch target and dependency list in `WatchRegistry::configure_watch` at `fuse/src/fs/watch.rs:187-215`.
9. Graph changes enter FUSE through `spawn_change_invalidator` and `invalidate_change` at `fuse/src/invalidation.rs:36-83`.
10. Property changes call `invalidate_property_change` at `fuse/src/invalidation.rs:252-301`. This notifies property, node-child, and node subject watchers, refreshes state watchers for property and node-child subjects, and invalidates FUSE inode state.
11. Relation changes call `invalidate_relation_change` and `retarget_relation_watchers` at `fuse/src/invalidation.rs:303-342` and `fuse/src/invalidation.rs:419-476`. Dependent watches are re-resolved with `resolve_watch_state`, then queued with `apply_retarget_result`.
12. Node changes call `notify_node_watchers` and `refresh_node_state_watchers` at `fuse/src/invalidation.rs:93-170` and `fuse/src/invalidation.rs:505-524`.
13. `WatchRegistry` queues state events through `queue_watch_state` at `fuse/src/fs/watch.rs:566-581`, retaining only the latest pending state event.
14. `WatchRegistry` queues change events through `queue_watch_change` at `fuse/src/fs/watch.rs:583-604`, bounding pending change events to `MAX_PENDING_WATCH_EVENTS` from `fuse/src/fs/watch.rs:75`.
15. On FUSE read, `read_watch_chunk` at `fuse/src/fs/watch.rs:237-274` pops one pending event, encodes it through `WatchEvent::encode_text`, supports offset/size slicing, and can move to the next queued event after a completed offset read.
16. On the client, `next_raw_event` waits for `AsyncFd` readability and drains all immediately readable bytes with `drain_watch_events` at `watch/src/client.rs:223-232` and `watch/src/client.rs:286-325`.
17. `next_event` decodes the drained bytes as a single `WatchEvent` at `watch/src/client.rs:202-205`.
18. The CLI command in `bin/src/watch.rs:7-18` uses raw event output for directories and read-after-watch output for non-directories.

## Behavior Summary

- The public protocol is newline-terminated text. Examples covered by tests include `unset\n`, `set /workspace/3\n`, `set true\n`, `property changed selected\n`, and `relation removed context:selected workspace\n` at `watch/src/protocol/test.rs:3-64`.
- `WatchValue::Path` versus `WatchValue::Property` is inferred during decode from whether a `set` payload starts with `/` at `watch/src/protocol.rs:71-76`.
- `WatchState::encode_text` writes only `set {payload}\n`, so the `WatchValue` variant is not present on the wire at `watch/src/protocol.rs:83-88`.
- Change events are parsed with `split_whitespace` at `watch/src/protocol.rs:120-149`, so the current protocol assumes action tokens, node IDs, property keys, and relation names have no whitespace.
- FUSE state values for properties come from `LocusValue::display_string` at `fuse/src/fs/resolve.rs:347-350`; string properties are returned verbatim by `graph/src/value/scalar.rs:26-29`.
- FUSE state values for node and relation targets are canonical filesystem paths built through `watch_subject_path` at `fuse/src/fs/watch.rs:772-819`.
- FUSE emits subject-relative property/relation change events only when the watched subject is the node itself. `property_event` emits `property changed title` for a node watch but `property changed node:57 title` for concrete property watches at `fuse/src/fs/watch.rs:828-848`; `relation_event` has the same behavior at `fuse/src/fs/watch.rs:850-870`.
- State watch messages coalesce to the latest state before read, as tested at `fuse/src/fs/test.rs:827-858`.
- Change watch messages are bounded to 256 pending events and drop the oldest on overflow, as implemented at `fuse/src/fs/watch.rs:75` and `fuse/src/fs/watch.rs:598-600`, and tested at `fuse/src/fs/test.rs:661-686`.
- Retargeted symlink-like watches can emit `unset` while the target is transiently missing and later emit `set /new/path`, as tested at `fuse/src/fs/test.rs:300-409`.
- The real FUSE smoke test validates property poll wakeups and a meta-watch over a relation path, but it is ignored because it requires host `/dev/fuse` access at `fuse/tests/fuse_smoke.rs:13-15`.

## API Findings

1. The state protocol is not injective for property values.

   `WatchState::encode_text` writes only `set {payload}\n` at `watch/src/protocol.rs:83-88`, and `WatchEvent::parse_text` classifies `set` payloads by leading slash at `watch/src/protocol.rs:71-76`. A property string value of `"/tmp"` is decoded as `WatchValue::Path("/tmp")`, not `WatchValue::Property("/tmp")`. An empty property string encodes as `set \n`, then `trim()` turns it into `set`, which is invalid at `watch/src/protocol.rs:66-79`. Trailing whitespace is also lost. This matters because FUSE emits property state from verbatim string values at `fuse/src/fs/resolve.rs:347-350` and `graph/src/value/scalar.rs:26-29`.

2. The typed client API cannot safely decode multiple queued events from one readiness drain.

   `Watch::next_event` decodes all bytes returned by `next_raw_event` as one event at `watch/src/client.rs:202-205`. `drain_watch_events` keeps reading until the fd returns `WouldBlock` or EOF at `watch/src/client.rs:293-319`. FUSE can return a subsequent queued event on a continued offset read after the previous event completes, and this is explicitly tested at `fuse/src/fs/test.rs:510-538`. If the kernel/client read loop drains `node changed ...\nnode removed ...\n` as one buffer, `WatchEvent::parse_text` sees one string with too many whitespace tokens and fails. The raw CLI path works because it prints bytes, but `wait_event`, `next_event`, and `next_state` expose a fragile public contract.

3. Subscription mode is hidden in a trailing slash convention.

   `resolve_watch_path` derives `directory_watch` from `path.ends_with('/')` at `fuse/src/fs/resolve.rs:29`. The CLI decides whether the input path is a directory only after opening the watch at `bin/src/watch.rs:8-11`; it does not explicitly request change mode. This makes ordinary directory paths such as `/mount/node/57` vulnerable to state-mode subscription unless the logical path preserves or appends a trailing slash. The test at `fuse/src/fs/test.rs:804-824` confirms that the trailing slash is what flips `/node/57/` to change mode.

4. `Watch::open_with_parts` exposes implementation details as public API.

   `open_with_parts` takes raw `PathBuf`, mount root, and logical path strings at `watch/src/client.rs:150-175`. It does not validate that the logical path is absolute, UTF-8-safe, mount-relative, or mode-correct. This is useful internally but leaks the subscription mechanics and makes invalid watch handles easy for external callers to construct.

5. Several public names are ambiguous.

   `WatchValue::Property` means "property payload value", not a property identity, while `WatchChange::Property` means a property event. `wait_event` and `next_event` are aliases at `watch/src/client.rs:197-205`, as are `wait_raw_event` and `next_raw_event` at `watch/src/client.rs:218-224`; the names do not communicate different behavior because there is none.

6. Event payloads are semantic graph IDs, not filesystem paths, but the API does not state this.

   For example, property changes for a concrete property watch encode as `property changed node:57 title` in `fuse/src/fs/test.rs:147-174`, not as a path under the mount. Node-subject watches use subject-relative names at `fuse/src/fs/test.rs:177-210` and `fuse/src/fs/test.rs:411-440`. That may be the right domain model, but it is a consumer-visible contract and needs explicit documentation.

## Redundancy Findings

- There are four overlapping change vocabularies:
  - `GraphChange` in graph.
  - `GraphWatchEvent` in `graph/src/graph/watch.rs:15-27`.
  - FUSE-local `WatchChange` in `fuse/src/fs/watch.rs:34-46`.
  - Public protocol `WatchChange` in `watch/src/protocol.rs:22-39`.
- FUSE maps `GraphWatchEvent` to its local `WatchChange` at `fuse/src/fs/watch.rs:48-67`, then maps local `WatchChange` to protocol `WatchChange` at `fuse/src/fs/watch.rs:742-870`. The duplication is understandable because `locusfs-watch` should not depend on `locusfs-graph`, but the mapping should live in one clearly named FUSE boundary module rather than inside the registry.
- `resolve_watch_path` and `resolve_graph_node_path` duplicate similar property/relation traversal logic at `fuse/src/fs/resolve.rs:64-150` and `fuse/src/fs/resolve.rs:245-327`. This is watch-related because it defines the subscription contract and dependency list. It is a good candidate for a small shared traversal helper after the contract is made explicit.
- Client API aliases add surface without behavior: `wait_event` versus `next_event`, and `wait_raw_event` versus `next_raw_event` in `watch/src/client.rs:192-237`.
- FUSE has two very similar poll wake paths: `wake_poll_handles` in `fuse/src/fs/filesystem.rs:480-486` and `notify_poll_handles` in `fuse/src/invalidation.rs:526-534`. They are in different modules but do the same "clone notifier and wake handles" operation.

## Performance And Concurrency Findings

- The central `SharedWatchRegistry` is an `Arc<tokio::sync::Mutex<WatchRegistry>>` at `fuse/src/fs/watch.rs:16`. Current call sites generally hold it for short synchronous registry operations and release it before graph awaits, which is good. Examples: relation retargeting copies paths before awaiting at `fuse/src/invalidation.rs:427-431`, and state refresh copies paths before resolving at `fuse/src/invalidation.rs:484-491`.
- Fanout uses `Vec<FileHandle>` in `subjects` and `dependency_index` at `fuse/src/fs/watch.rs:85-92` and clones watcher lists before queueing at `fuse/src/fs/watch.rs:551-555`. This is simple and fine for small fanout, but removal and membership are linear. Under many watch handles, `HashSet<FileHandle>` or an ordered set would reduce detach and duplicate-prevention cost.
- Node changes scan all `property_watches` and subject keys at `fuse/src/fs/watch.rs:396-444`. This is acceptable for the current scale, but it is the hot path to revisit if LocusFS becomes a general event bus with many long-lived watches.
- Pending change overflow silently drops the oldest event at `fuse/src/fs/watch.rs:598-600`. That avoids unbounded memory but gives clients no lag signal. If consumers rely on lifecycle events, the public contract should either expose a lag/coalesced event or document that change streams are lossy and state reads are the recovery path.
- State coalescing at `fuse/src/fs/watch.rs:573-579` is a good fit for "current value" subscriptions, but it should be explicit in docs and tests because it differs from change-stream semantics.
- Client `drain_watch_events` accumulates all currently readable bytes into an unbounded `Vec` at `watch/src/client.rs:293-319`. The FUSE producer bounds event count, but event payload sizes are not bounded by the protocol. Buffered decoding should still cap individual frame length or at least make memory behavior explicit.
- `wake_poll_handles` and `notify_poll_handles` clone the current notifier under a mutex for every handle at `fuse/src/fs/filesystem.rs:480-486` and `fuse/src/invalidation.rs:526-534`. Cloning the notifier once per batch would reduce lock churn.
- The client read retry loop sleeps every 25 ms until an outer timeout at `watch/src/client.rs:335-345`. This is pragmatic for transient FUSE retargeting, but the default five-second timeout at `watch/src/client.rs:14` is policy embedded in a public client helper. Expose the timeout policy clearly or make callers opt into it.

## Tidiness And Docs Findings

- Public protocol enums and methods in `watch/src/protocol.rs:4-179` have no item-level docs. The crate root docs are helpful, but rust-guide standards call for public-facing API docs at the item or module root where consumers discover invariants.
- The most important missing docs are:
  - Wire format and compatibility expectations.
  - State versus change mode.
  - Whether watch streams are lossy.
  - Whether state events are coalesced.
  - Whether payloads are raw bytes, display strings, paths, graph IDs, or filesystem paths.
  - Subject-relative versus absolute event payload rules.
- `Watch::wait` is documented as "Waits until this subscription receives a change notification" at `watch/src/client.rs:192-195`, but it discards any `WatchEvent`, including state events. If state subscriptions remain public, this wording is too narrow.
- `Watch::next_event` and `Watch::wait_event` share the same doc text at `watch/src/client.rs:197-205`; same for raw variants at `watch/src/client.rs:218-224`. Keep one name or document a real difference.
- `WatchRegistry` is a large mixed-responsibility file. It owns handle allocation, property polling, watch subscription indexing, event queueing, protocol conversion, subject path formatting, and graph-watch suppression in `fuse/src/fs/watch.rs:85-880`. A later refactor should split registry state, protocol conversion, and target path formatting into sibling files.
- Existing tests are colocated in sibling `test.rs` modules, matching the code-structure guidance and the existing workspace style.

## Best-Practice And Crate Reuse Notes

- The current dependency split is good: FUSE uses `locusfs-watch` protocol-only with `default-features = false`, so it does not pull in Tokio client helpers or tracing through the protocol API.
- `tokio::io::unix::AsyncFd` is the right shape for readiness-driven `/watch` reads. The direct `libc` use for `fcntl`, `poll`, and `read` in `watch/src/client.rs:286-389` is small and contained. If more syscall wrappers appear, consider `rustix` or `nix`, but adding either just for these few calls is not necessary.
- Do not introduce an external parser for the current text format. A small project-owned codec is enough if the protocol stays text-based.
- If the protocol is allowed to change, consider a self-delimiting format for state payloads and event frames. Options include length-prefixed text frames or JSON Lines with explicit fields. JSON would reuse existing workspace `serde`/`serde_json` dependencies from `Cargo.toml:23-24`, but it adds public API and compatibility considerations. A length-prefixed text codec avoids a new public serialization dependency and handles empty strings, leading slashes, whitespace, and newlines.
- Avoid making `locusfs-graph` depend on `locusfs-watch` just to remove mapping duplication. The current acyclic dependency direction is healthy: graph is lower-level, FUSE adapts graph events to filesystem/watch protocol events.

## Domain-Specific Filesystem And Watch Layout Notes

- The `/watch` file is a combined control and data file: write a logical path to subscribe, read events from the same handle, and poll for wakeups. This is compact and shell-friendly, but it needs stronger documentation because it is not a normal file-content API.
- The subscription path is a filesystem path, while change events are semantic graph events. That split is powerful but surprising. A watch on `/node/57` receives `property added title`, while a watch on `/node/57/title` receives `property changed node:57 title`; both are valid per current tests but need a documented rationale.
- State watches return either `unset`, a property display value, or a canonical target path. This is currently encoded as `WatchValue::{Property, Path}`, but that variant is not preserved on the wire.
- Directory/change watches are selected by trailing slash for most resolved node targets. This is brittle for CLI users and any client that canonicalizes paths. The mode should be explicit in the subscription API or derived by the client before writing to `/watch`.
- Relation-dependent watches are one of the strongest pieces of the design. `dependent_watch_paths`, `apply_retarget_result`, and state coalescing let a symlink-like path survive transient relation target removal and retargeting, as tested at `fuse/src/fs/test.rs:300-409`. Preserve this behavior during refactor.
- The watch layout should be reconciled with the broader plugin/filesystem layout review. The user specifically called out D-Bus paths moving toward clearer `/dbus/<service>/methods` and `/dbus/<service>/objects` structures. Watch semantics should not bake in today's ambiguous "child name could be property or relation" behavior without an explicit conflict policy. `resolve_watch_path` currently returns `EIO` when a child segment is both property and relation at `fuse/src/fs/resolve.rs:83-85` and `fuse/src/fs/resolve.rs:260-262`.

## Concrete Refactor Plan

1. Document the current contract before changing it.

   Add item docs for the protocol types and client methods in `watch/src/protocol.rs` and `watch/src/client.rs`. Document state/change modes, event loss/coalescing, subject-relative payload rules, subscription write format, and compatibility expectations. This should be done even if the protocol changes later, because it gives tests and migration code a baseline.

2. Introduce an explicit codec boundary in `locusfs-watch`.

   Add a private codec module or protocol helpers that can encode and decode one frame at a time. The API should support buffered decoding: feed bytes, receive zero or more `WatchEvent`s, retain incomplete frames. Then update `Watch` to keep an internal pending event queue so `next_event` returns one decoded event even when one readiness drain contains multiple FUSE events.

3. Fix state payload encoding.

   Coordinator decision needed on compatibility:
   - If the protocol is not stable, replace `set <payload>` with explicit state frames such as `state unset`, `state path <encoded>`, and `state value <encoded>` or a length-prefixed equivalent.
   - If compatibility matters, decode legacy `unset` and `set <payload>` but emit a versioned unambiguous format from FUSE and the client tests.

   The new encoding must round-trip empty strings, leading slashes, spaces, trailing whitespace, and newlines.

4. Make subscription mode explicit.

   Add a public client-side concept such as `WatchMode`, `WatchSubscription`, `Watch::open_state(path)`, and `Watch::open_changes(path)`, or a builder. The FUSE parser can keep accepting legacy path-only writes, but it should also accept an explicit mode. Update `bin/src/watch.rs` so it checks metadata before opening and subscribes directories in change mode without relying on a trailing slash.

5. Narrow or validate `Watch::open_with_parts`.

   Either make it `pub(crate)` if no external caller needs it, or replace raw parts with a validated `WatchSubscription` type. If it remains public, validate leading slash, no parent escape, and explicit mode.

6. Move FUSE event adaptation out of `WatchRegistry`.

   Keep the registry focused on handles, queues, indexes, and poll handles. Move `GraphWatchEvent`/local change to protocol conversion, `watch_subject_path`, `node_event`, `property_event`, and `relation_event` into a sibling module such as `fuse/src/fs/watch/protocol.rs` or `fuse/src/fs/watch/event.rs`. After that, decide whether the FUSE-local `WatchChange` enum still earns its keep.

7. Reduce resolver duplication after mode/protocol decisions.

   Extract the repeated property/relation traversal from `resolve_watch_path` and `resolve_graph_node_path`. Keep the change small and behavior-preserving; tests already cover the risky cases.

8. Make overflow semantics explicit.

   Either document drop-oldest for change streams or add a protocol-level lag/coalesced event. For a filesystem watch API, a generic `change` event after overflow may be enough if clients are expected to re-read state.

9. Tune fanout only after correctness.

   If needed, replace subject/dependency watcher `Vec`s with `HashSet<FileHandle>` and clone the notifier once per wake batch. These are secondary to protocol correctness and should not be mixed into the same patch as the codec change unless tests force it.

## Test Plan

- Protocol tests in `watch/src/protocol/test.rs`:
  - Round-trip every event variant, including `Node`.
  - Round-trip state property values `""`, `" "`, `"trailing "`, `"/tmp"`, `"hello world"`, and a value containing `\n`.
  - Decode multiple frames from one byte buffer through the new buffered codec.
  - Reject malformed actions, incomplete frames, invalid UTF-8 where applicable, and extra tokens.
  - If compatibility is retained, decode legacy `unset`, `set /path`, and `set value`.
- Client tests in `watch/src/client/test.rs`:
  - Construct a `Watch` around a Unix stream or equivalent fd and verify that `next_event` returns two separate typed events when the fd receives two frames in one readiness drain.
  - Verify raw event reads still work for CLI-style output.
  - Verify directory/change mode helpers produce the intended subscription text.
  - Keep `read_timeout_bounds_missing_path_retry`.
- FUSE registry tests in `fuse/src/fs/test.rs`:
  - Preserve existing tests for partial reads, state coalescing, retargeting, graph-watch duplicate suppression, fanout, and queue bounds.
  - Add a test that explicit directory/change subscription does not rely on trailing slash.
  - Add a test for state property values that previously failed wire round-trip.
  - Add a test for overflow behavior once the coordinator chooses drop-oldest docs versus a lag event.
- CLI tests:
  - Factor the "choose watch mode from path metadata" logic into a small testable function or integration seam.
  - Verify directories request change mode and files request state/read-after-watch mode.
- Verification commands:
  - `cargo test -p locusfs-watch`
  - `cargo test -p locusfs-fuse watch`
  - `cargo test --workspace`
  - Optional host-dependent smoke: run the ignored `fuse_smoke` test only where `/dev/fuse` and `fusermount3` are available.

## Verification Performed

- `cargo test -p locusfs-watch`: passed, 9 tests.
- `cargo test -p locusfs-fuse watch`: passed, 30 watch-filtered tests.
- `cargo test --workspace`: passed. The real FUSE smoke test remains ignored because it requires host `/dev/fuse` access.

## Open Questions For Coordinator Arbitration

1. Is the `/watch` text protocol already an external compatibility contract, or may this refactor introduce a breaking wire-format change?
2. Should state events carry display strings, raw bytes, or typed `LocusValue` data? The current `display_string` path is simple but loses type information.
3. Should change events stay semantic (`property changed node:57 title`) or become path-oriented for filesystem consumers?
4. Should subject-relative event payloads remain, or should every change event include an absolute subject to simplify clients?
5. Should directory/change mode be explicit in the subscription format, or should the client library hide the trailing slash convention while FUSE keeps accepting it?
6. What should happen on change queue overflow: silent drop-oldest, generic `change`, explicit `lagged`, or forced state resync?
7. Should `Watch::open_with_parts` remain public for advanced clients, or is it an implementation detail that should become private/validated?
8. Are watch streams expected to be long-lived and numerous enough to justify `HashSet` indexes now, or should we defer fanout structure changes until after protocol fixes?
9. Should graph-native `GraphWatchEvent` and public protocol `WatchChange` be deliberately kept separate, with FUSE as the only adapter, to preserve dependency direction?
10. Should the CLI `watch` command print raw event protocol for directories, or should it render a more stable human-facing format separate from the wire protocol?
