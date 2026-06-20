# Graph/FUSE Architecture Audit

Scope: `graph/`, `fuse/`, and the graph/FUSE boundary. This is an architecture/API review only; no source changes were made.

## Findings

### 1. The watch/change model is split across three overlapping APIs

Observations:
- `locusfs-graph` exposes a global change stream as `GraphChange` via `subscribe_global_changes`/`subscribe_changes` and `GraphChangeSubscription` (`graph/src/graph/dynamic.rs:30`, `graph/src/graph/dynamic.rs:103`, `graph/src/graph/dynamic.rs:107`, `graph/src/graph/dynamic.rs:111`).
- The same crate also exposes target-scoped watches as `GraphWatchTarget`/`GraphWatchEvent`/`GraphWatch` (`graph/src/graph/watch.rs:6`, `graph/src/graph/watch.rs:15`, `graph/src/graph/watch.rs:29`) and implements fallback conversion from global changes to watch events inside `DynamicGraph` (`graph/src/graph/dynamic.rs:244`, `graph/src/graph/dynamic.rs:788`).
- `locusfs-fuse` defines a near-isomorphic `WatchChange` enum and converts `GraphWatchEvent` into it (`fuse/src/fs/watch.rs:31`, `fuse/src/fs/watch.rs:45`), while `invalidation.rs` separately converts `GraphChange` variants into `WatchChange` variants (`fuse/src/invalidation.rs:84`).
- FUSE sometimes uses graph-scoped watches and sometimes the global invalidation worker. `write_owned` only starts a `GraphWatch` when the resolved watch has no dependencies and uses change mode (`fuse/src/fs/filesystem.rs:883`, `fuse/src/fs/filesystem.rs:886`); otherwise retargeting and queueing are handled by `InvalidationWorker` and `WatchRegistry` (`fuse/src/invalidation.rs:385`, `fuse/src/fs/watch.rs:503`).

Recommendation:
- Pick one graph-level event contract and make FUSE adapt from that once. A likely shape is: graph owns `GraphChange` plus filtering/subscription helpers, FUSE owns only kernel-specific state (`poll`, file handles, unread queues, text formatting). If `GraphWatchEvent` remains public, avoid duplicating its variants as `WatchChange`; use graph events directly until the final serialization step.
- The `subscribe_global_*` and non-global aliases (`subscribe_changes`, `emit_change`) appear redundant (`graph/src/graph/dynamic.rs:103`, `graph/src/graph/dynamic.rs:107`, `graph/src/graph/dynamic.rs:119`, `graph/src/graph/dynamic.rs:124`). Prefer one naming convention unless there is a planned non-global channel.

### 2. FUSE path resolution is duplicated and can drift

Observations:
- Runtime lookup resolves graph layout in `LocusFs::lookup_entry` by matching `FsEntry` parents, decoding names, checking properties, checking relations, and choosing file vs symlink vs directory (`fuse/src/fs/filesystem.rs:83`, `fuse/src/fs/filesystem.rs:117`).
- Watch subscription resolution repeats much of that path traversal independently in `resolve_watch_path`, including property/relation ambiguity handling and relation target traversal (`fuse/src/fs/resolve.rs:22`, `fuse/src/fs/resolve.rs:50`, `fuse/src/fs/resolve.rs:58`, `fuse/src/fs/resolve.rs:100`).
- Directory rendering repeats the same property/relation collision rule and relation cardinality rule again (`fuse/src/fs/directory.rs:68`, `fuse/src/fs/directory.rs:86`, `fuse/src/fs/directory.rs:91`).
- There is a public `Layout` builder (`fuse/src/layout/mod.rs:9`), but runtime code does not use it for parsing or relation target display names. Runtime path strings are also constructed in `watch_subject_path` (`fuse/src/fs/watch.rs:801`) and relation symlink helpers (`fuse/src/fs/entry.rs:63`).

Recommendation:
- Introduce one internal layout resolver that maps encoded paths/names to `FsEntry` plus enough metadata for watch dependency/state decisions. Keep `Layout` as the public builder if useful, but align it with the runtime display rules. Today `Layout::node_relation_target_link` encodes `target.to_string()` (`fuse/src/layout/mod.rs:42`), while directory entries use `encode_relation_target_name` and can display shortened names (`fuse/src/fs/directory.rs:127`, `fuse/src/fs/name.rs:97`).
- Treat property/relation name collision and single-vs-many relation display as one layout policy, not scattered checks.

### 3. FUSE reaches through the boundary to create graph provider implementation details

Observations:
- `locusfs-fuse` imports `InMemoryProvider` and creates/registers one directly when users `mkdir` a root child (`fuse/src/fs/filesystem.rs:20`, `fuse/src/fs/filesystem.rs:306`, `fuse/src/fs/filesystem.rs:321`).
- Creating a kind directory requires six separate registrations for the same provider instance: node, node mutation, property, property mutation, relation, and relation mutation (`fuse/src/fs/filesystem.rs:321`, `fuse/src/fs/filesystem.rs:343`).
- Tests repeat the same provider-registration sequence (`fuse/tests/fuse_smoke.rs:128`, `fuse/tests/fuse_smoke.rs:132`).

Recommendation:
- FUSE should not need to know that the writable fallback implementation is `InMemoryProvider`. Move this behind a graph API such as `register_full_provider`, `register_in_memory_kind`, or an application-level provisioning layer. That would keep FUSE focused on request translation and reduce registration boilerplate.
- If multi-capability providers are expected, add a trait or helper for registering a provider that implements the complete bundle. The current API makes the common case verbose and easy to register incompletely.

### 4. Relation overlay ownership is surprising and weakens provider boundaries

Observations:
- `DynamicGraph` owns a `RelationOverlay` side table in addition to provider-owned relation state (`graph/src/graph/dynamic.rs:72`, `graph/src/graph/dynamic.rs:88`).
- `set_link` writes to a provider when possible, but silently falls back to the overlay when the mutation provider is unsupported or missing/not found (`graph/src/graph/dynamic.rs:539`, `graph/src/graph/dynamic.rs:547`).
- `targets` merges provider targets with overlay targets and returns `NotFound` when the merged result is empty (`graph/src/graph/dynamic.rs:497`, `graph/src/graph/dynamic.rs:507`, `graph/src/graph/dynamic.rs:510`).
- Removing a node cleans overlay outbound/inbound links and then scans every provider node/relation to remove inbound provider links (`graph/src/graph/dynamic.rs:360`, `graph/src/graph/dynamic.rs:590`).

Recommendation:
- Make overlay semantics explicit in the API. If overlays are a first-class graph layer, document and name them that way, and expose their ownership/lifetime clearly. If they are only a compatibility fallback, avoid silently persisting graph state outside the provider selected for the source node.
- Consider moving cross-provider relation handling behind a dedicated relation store/provider rather than burying it inside `DynamicGraph`. That would clarify who owns indexes, lifecycle cleanup, and change emission.

### 5. State/index ownership in FUSE is split across inode, watch, and invalidation modules

Observations:
- `LocusFs` owns the graph plus three shared mutable state objects: `InodeTable`, `WatchRegistry`, and kernel `Notify` (`fuse/src/fs/filesystem.rs:52`, `fuse/src/fs/filesystem.rs:54`, `fuse/src/fs/filesystem.rs:55`, `fuse/src/fs/filesystem.rs:56`).
- `InodeTable` owns inode maps and timestamps (`fuse/src/fs/inode.rs:13`, `fuse/src/fs/inode.rs:16`, `fuse/src/fs/inode.rs:18`), while invalidation mutates timestamps and sends kernel invalidations based on graph changes (`fuse/src/invalidation.rs:310`, `fuse/src/invalidation.rs:337`, `fuse/src/invalidation.rs:488`).
- `WatchRegistry` owns open file state, property poll generations, subject indexes, dependency indexes, task handles, pending reads, and pending events in one struct (`fuse/src/fs/watch.rs:100`, `fuse/src/fs/watch.rs:121`).
- The invalidation worker understands watch dependencies and retargeting by calling back into `resolve_watch_state` (`fuse/src/invalidation.rs:385`, `fuse/src/invalidation.rs:416`, `fuse/src/invalidation.rs:430`, `fuse/src/invalidation.rs:443`).

Recommendation:
- Keep kernel mechanics (`poll` handles, open file handles, unread buffers) in FUSE, but consider a narrower `WatchRegistry` API that accepts graph-domain events and target state updates without exposing dependency/retargeting policy to `invalidation.rs`.
- Consider separating property poll generation tracking from path watch subscription state. They share file handles and wakeups, but their domain models are different.

### 6. Public API cleanliness: graph contracts are reusable, but `DynamicGraph` is doing too much

Observations:
- The graph crate has a reusable trait surface: node, property, relation, mutation, watch, value, and identity types are public and independent of FUSE (`graph/src/graph/mod.rs:21`, `graph/src/graph/mod.rs:38`, `graph/src/graph/mod.rs:56`, `graph/src/lib.rs:12`).
- `DynamicGraph` combines provider registry, mutation orchestration, relation overlay, global event bus, watch fallback, and graph cleanup in one 900+ line implementation file (`graph/src/graph/dynamic.rs:70`, `graph/src/graph/dynamic.rs:77`, `graph/src/graph/dynamic.rs:343`, `graph/src/graph/dynamic.rs:590`, `graph/src/graph/dynamic.rs:788`).
- `TracedProvider` wraps most provider traits but not `WatchProvider`, so tracing coverage is incomplete for a capability exported by the same graph API (`graph/src/graph/trace.rs:28`, `graph/src/graph/trace.rs:277`, `graph/src/graph/watch.rs:53`).

Recommendation:
- Split `DynamicGraph` by responsibility: registry, mutations/change emission, relation overlay/store, and watch filtering. This would preserve the public facade while making ownership and invariants easier to review.
- Add `WatchProvider` support to tracing if provider watches are meant to be part of the production provider contract.

### 7. Value/file conversion lives in FUSE, but it is effectively a graph serialization policy

Observations:
- FUSE parses writes into `LocusValue` based on `ValueKind` and formats property reads with `display_string` plus a newline (`fuse/src/fs/value.rs:49`, `fuse/src/fs/value.rs:104`).
- Graph exposes value kinds and display strings but no parse-from-string counterpart (`graph/src/value/scalar.rs:15`, `graph/src/value/scalar.rs:26`).
- Missing property specs become writable string specs at the FUSE layer (`fuse/src/fs/value.rs:17`, `fuse/src/fs/value.rs:24`), which is a graph creation policy rather than purely a kernel translation detail.

Recommendation:
- If text file representation is the canonical user API, consider moving parse/format policy to `locusfs-graph` or a shared adapter module. FUSE can still add newline/file slicing behavior, but value validation and string parsing would be reusable by non-FUSE clients.
- Make "unknown property writes create string properties" an explicit graph mutation option or documented FUSE-only behavior.

## Lower-Priority Notes

Observations:
- `NodeAccess` allows `readable=false, writable=true` (`graph/src/graph/access.rs:7`), and FUSE maps that to executable/write-only directory permissions (`fuse/src/fs/value.rs:40`). That may be intentional for traversal, but the graph API does not document what unreadable writable nodes mean.
- `GraphWatch::try_recv` collapses empty/disconnected/lagged receiver states into `None` because it calls `.ok()` (`graph/src/graph/watch.rs:38`). For low-level watch APIs, distinguishing "no event yet" from "closed" can matter.
- `GraphError::Io` stores only a string (`graph/src/error.rs:38`), which is portable but loses typed `io::ErrorKind` information before FUSE maps errors to errno (`fuse/src/error.rs:18`).

## Suggested Refactor Order

1. Consolidate graph change/watch event vocabulary first; it affects both crates and will simplify later FUSE watch cleanup.
2. Extract a single FUSE layout resolver and make lookup, directory rendering, watch resolution, and public `Layout` use the same policy.
3. Add graph helper APIs for full-provider registration and writable in-memory kind creation; then remove FUSE's direct dependency on `InMemoryProvider`.
4. Split `DynamicGraph` internals after the boundary contracts are cleaner, especially relation overlay/store and watch filtering.
