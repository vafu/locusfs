# Graph Core Review

## Current Role

`locusfs-graph` is the workspace's shared graph contract crate. It owns identity types, scalar values, property metadata, provider traits, the optional `DynamicGraph` runtime facade, `InMemoryProvider`, provider tracing, and graph/watch change vocabulary. It currently sits below FUSE, the host binary, plugin API, and all plugins; consumers import it directly rather than through a narrower host capability.

This review covers `graph/` only, with bounded consumer checks in FUSE and plugins to validate API pressure points.

## Public API And Entrypoints

- Crate root exports `graph`, `identity`, `value`, `GraphError`, `Result`, optional implementations, graph traits, watch types, identity types, value types, and a prelude (`graph/src/lib.rs:17`, `graph/src/lib.rs:22`, `graph/src/lib.rs:23`, `graph/src/lib.rs:31`, `graph/src/lib.rs:36`, `graph/src/lib.rs:38`, `graph/src/lib.rs:41`).
- Feature flags advertise a stable contract-only surface with `default-features = false`, and optional `dynamic`, `watch-provider`, `in-memory`, and `provider-tracing` APIs (`graph/src/lib.rs:6`, `graph/Cargo.toml:6`).
- Public identity types are `NodeKind`, `PathName`, `PropertyKey`, `RelationName`, and `NodeId` (`graph/src/identity/names.rs:45`, `graph/src/identity/node.rs:8`). `NodeId` uses `<kind>:<local>` parsing and allows colons in the local id by using `split_once` (`graph/src/identity/node.rs:21`).
- Public value types are `LocusValue`, `ValueKind`, and `PropertySpec` (`graph/src/value/scalar.rs:5`, `graph/src/value/kind.rs:1`, `graph/src/value/property.rs:5`).
- Public provider traits are split by capability: node, node mutation, property, property mutation, relation, relation mutation, path, and watch (`graph/src/graph/mod.rs:57`, `graph/src/graph/mod.rs:67`, `graph/src/graph/mod.rs:73`, `graph/src/graph/mod.rs:80`, `graph/src/graph/mod.rs:91`, `graph/src/graph/mod.rs:97`, `graph/src/graph/mod.rs:119`, `graph/src/graph/watch.rs:53`).
- `DynamicGraph` is the main runtime entrypoint. It exposes provider registration, graph queries, mutations, global change subscription/emission, target-scoped watch fallback, and provider-owned path routing (`graph/src/graph/dynamic.rs:71`, `graph/src/graph/dynamic.rs:95`, `graph/src/graph/dynamic.rs:130`, `graph/src/graph/dynamic.rs:236`, `graph/src/graph/dynamic.rs:398`).
- `InMemoryProvider` is a concrete read-write provider with per-kind node, property, and relation storage (`graph/src/graph/memory.rs:16`, `graph/src/graph/memory.rs:22`, `graph/src/graph/memory.rs:82`).
- `TracedProvider<P>` is an optional instrumentation wrapper around providers (`graph/src/graph/trace.rs:13`).

## Step-By-Step Walkthrough

1. The crate root exposes the public contract from module roots and gates optional runtime implementations behind features (`graph/src/lib.rs:17`, `graph/src/lib.rs:23`, `graph/src/lib.rs:27`, `graph/src/lib.rs:36`).
2. Identity wrappers validate only empty strings, `.`, `..`, NUL, and `:` for node kinds (`graph/src/identity/validation.rs:3`, `graph/src/identity/validation.rs:20`). This keeps node locals flexible enough for external IDs and filesystem encoding.
3. Values are a small scalar enum with a `ValueKind` classifier and display conversion (`graph/src/value/scalar.rs:7`, `graph/src/value/scalar.rs:16`, `graph/src/value/scalar.rs:26`). `PropertySpec` stores key, value kind, and readable/writable flags (`graph/src/value/property.rs:5`).
4. Providers are capability traits. `NodeProvider` and `PathProvider` expose `kind()`, but property, relation, and mutation traits do not, so `DynamicGraph` registration has mixed ergonomics (`graph/src/graph/mod.rs:59`, `graph/src/graph/mod.rs:120`, `graph/src/graph/dynamic.rs:130`, `graph/src/graph/dynamic.rs:155`).
5. `DynamicGraph` keeps one `BTreeMap<NodeKind, Arc<dyn ...>>` per capability plus a private relation overlay and a global broadcast sender (`graph/src/graph/dynamic.rs:20`, `graph/src/graph/dynamic.rs:78`, `graph/src/graph/dynamic.rs:90`).
6. Registration inserts a provider into the capability map after duplicate checks (`graph/src/graph/dynamic.rs:130`, `graph/src/graph/dynamic.rs:1010`).
7. Query methods route by `NodeId.kind()` and clone provider `Arc`s out of the registry before awaiting provider calls (`graph/src/graph/dynamic.rs:323`, `graph/src/graph/dynamic.rs:448`, `graph/src/graph/dynamic.rs:489`).
8. Mutations compose provider calls with change emission. Node creation emits `NodeAdded` and `NodeKindChanged`; property mutation probes `property_spec` to choose added vs changed; relation mutation compares before/after target sets (`graph/src/graph/dynamic.rs:398`, `graph/src/graph/dynamic.rs:496`, `graph/src/graph/dynamic.rs:575`).
9. If source relation mutation is unsupported or missing, `DynamicGraph` writes a private relation overlay after verifying source and target nodes exist (`graph/src/graph/dynamic.rs:581`, `graph/src/graph/dynamic.rs:594`, `graph/src/graph/dynamic.rs:602`, `graph/src/graph/dynamic.rs:731`).
10. Target-scoped watch fallback subscribes to global `GraphChange`, spawns one task per watch, maps changes through a private filter, and sends `GraphWatchEvent` on an mpsc channel (`graph/src/graph/dynamic.rs:299`, `graph/src/graph/dynamic.rs:843`).
11. `InMemoryProvider` stores nodes, properties, and outbound relation sets under a `tokio::sync::RwLock` and implements all core read-write provider traits (`graph/src/graph/memory.rs:19`, `graph/src/graph/memory.rs:24`, `graph/src/graph/memory.rs:82`).
12. `TracedProvider` repeats the same span/timing wrapper pattern for each provider method, forwarding to the inner provider (`graph/src/graph/trace.rs:29`, `graph/src/graph/trace.rs:119`, `graph/src/graph/trace.rs:236`, `graph/src/graph/trace.rs:341`).

## Behavior Summary

The crate is mostly a contract plus a routing facade, not a single authoritative graph store. Provider implementations own node/property/relation state per node kind. `DynamicGraph` routes calls by node kind, coordinates cross-provider operations, and owns fallback relation state only when the source provider cannot mutate relations.

Global changes and target watches are separate public concepts. `GraphChange` is the broad semantic mutation event (`graph/src/graph/change.rs:3`), while `GraphWatchEvent` is an almost identical target-scoped event enum plus a generic `Change` (`graph/src/graph/watch.rs:15`). FUSE duplicates that shape again as `WatchChange` (`fuse/src/fs/watch.rs:34`).

The existing graph unit tests are useful and pass under all features. They cover identity validation, values, provider registration, in-memory mutations, relation overlay basics, change emission, and fallback watch filtering (`graph/src/graph/test.rs:10`, `graph/src/graph/test.rs:260`, `graph/src/graph/test.rs:327`). They do not cover feature matrix compatibility or every watch target/change combination.

Verification performed:

- `cargo check -p locusfs-graph --no-default-features`: passed.
- `cargo check -p locusfs-graph --no-default-features --features dynamic`: passed.
- `cargo check -p locusfs-graph --no-default-features --features in-memory`: passed.
- `cargo check -p locusfs-graph --no-default-features --features provider-tracing,watch-provider`: passed.
- `cargo check -p locusfs-graph --all-features`: passed.
- `cargo test -p locusfs-graph --all-features`: passed, 29 tests.
- `cargo check -p locusfs-graph --no-default-features --features provider-tracing`: failed. `trace.rs` implements `watch_target` when `PathProvider` does not have that method and `GraphWatchTarget` is not exported (`graph/src/graph/trace.rs:398`, `graph/src/graph/mod.rs:130`, `graph/src/lib.rs:36`).

## API Findings

1. Feature flag stability has a concrete bug. `provider-tracing` is advertised as independent (`graph/Cargo.toml:10`), but enabling only that feature fails because `TracedProvider<PathProvider>` unconditionally forwards `watch_target` (`graph/src/graph/trace.rs:398`). Either gate that method with `#[cfg(feature = "watch-provider")]` or make `provider-tracing` depend on `watch-provider`. The former preserves a smaller optional surface.
2. The crate root is discoverable, but most public items lack contract docs. Provider traits, `DynamicGraph`, `GraphChange`, `GraphWatchTarget`, `GraphWatchEvent`, identity constructors, and `PropertySpec` all need docs describing invariants, lifecycle, routing, event semantics, and compatibility expectations (`graph/src/graph/mod.rs:57`, `graph/src/graph/change.rs:3`, `graph/src/graph/watch.rs:6`, `graph/src/identity/names.rs:14`, `graph/src/value/property.rs:14`).
3. `GraphChangeReceiver` leaks the `tokio::sync::broadcast::Receiver` implementation into the public API (`graph/src/graph/dynamic.rs:31`). The wrapper `GraphChangeSubscription` is a better stability boundary because it owns lag/closed semantics (`graph/src/graph/dynamic.rs:39`). Prefer documenting and steering callers to the wrapper; keep the raw receiver only as an advanced escape hatch or deprecate it before wider API commitment.
4. `subscribe_global_changes`/`subscribe_changes` and `emit_global_change`/`emit_change` duplicate names without a documented distinction (`graph/src/graph/dynamic.rs:105`, `graph/src/graph/dynamic.rs:109`, `graph/src/graph/dynamic.rs:121`, `graph/src/graph/dynamic.rs:126`). Pick one vocabulary. If the graph only has one global stream, `subscribe_changes` and `emit_change` are enough.
5. `emit_global_change` returns `Result<()>` but currently cannot fail and discards the broadcast send result (`graph/src/graph/dynamic.rs:121`). That makes plugin publishing look fallible without preserving any useful failure. Return `()` or return a meaningful subscriber count/result.
6. Provider registration is correct but not ergonomic. A full read-write provider needs six calls in project registration and tests (`plugins/project/src/lib.rs:43`, `plugins/project/src/lib.rs:46`, `plugins/project/src/lib.rs:49`, `plugins/project/src/lib.rs:52`, `plugins/project/src/lib.rs:55`, `plugins/project/src/lib.rs:58`; `fuse/tests/fuse_smoke.rs:132`). Add bundle helpers while keeping the granular API.
7. Mixed kind ownership leaks implementation details. `register_node_provider` and `register_path_provider` infer kind from the provider, while property/relation/mutation registration accepts an external `NodeKind` (`graph/src/graph/dynamic.rs:134`, `graph/src/graph/dynamic.rs:155`, `graph/src/graph/dynamic.rs:185`, `graph/src/graph/dynamic.rs:215`). A shared provider-kind contract or registration builder would make accidental kind/provider mismatches harder.
8. `PathProvider` changes shape with `watch-provider` because `watch_target` is cfg-gated inside the trait (`graph/src/graph/mod.rs:130`). Consider splitting this into a `PathWatchProvider` extension trait or making watch targets part of the always-available path contract. Trait-shape cfgs are awkward for implementors and caused the tracing feature bug.
9. `GraphPathDirectory::Virtual { owner, local: String }` exposes an unstructured implementation string as a public routing key (`graph/src/graph/mod.rs:37`). D-Bus currently encodes virtual state by hand with constants and a separator (`plugins/dbus/src/state.rs:19`, `plugins/dbus/src/state.rs:23`, `plugins/dbus/src/state.rs:27`). A typed virtual path id or helper builder would reduce leakage while keeping provider-owned layouts.
10. `PathName` uses the same permissive validation as other identity names (`graph/src/identity/names.rs:45`, `graph/src/identity/validation.rs:3`). If `PathName` means a literal filesystem path segment, it should probably reject `/`; if it means an unencoded display child name, graph should document that FUSE must encode it.
11. Public enums are exhaustively matchable. For a contract crate that may add value kinds, change variants, watch targets, or error variants, consider `#[non_exhaustive]` on `GraphError`, `LocusValue`, `ValueKind`, `GraphChange`, `GraphWatchTarget`, and `GraphWatchEvent` before external users rely on exhaustive matches (`graph/src/error.rs:10`, `graph/src/value/scalar.rs:7`, `graph/src/value/kind.rs:3`, `graph/src/graph/change.rs:4`, `graph/src/graph/watch.rs:7`).
12. `GraphError::Io(String)` discards the source error (`graph/src/error.rs:38`, `graph/src/error.rs:56`). For library code, preserve the source with a transparent or `#[source]` variant unless there is a strong reason to make the error string-only.

## Redundancy Findings

1. Change/watch vocabulary is duplicated in graph and repeated again in FUSE. `GraphChange` and `GraphWatchEvent` share node/property/relation lifecycle variants (`graph/src/graph/change.rs:4`, `graph/src/graph/watch.rs:16`), and FUSE defines `WatchChange` with the same shape plus conversions (`fuse/src/fs/watch.rs:34`, `fuse/src/fs/watch.rs:48`). Graph should own the canonical event/filter vocabulary and let FUSE adapt only at the kernel/protocol edge.
2. `watch_event_for_change` is a reusable projection helper but is private inside `dynamic.rs` (`graph/src/graph/dynamic.rs:843`). FUSE and plugins need equivalent reasoning about target impact, which encourages copy/paste or divergent behavior.
3. Provider registry storage and registration are repetitive by capability (`graph/src/graph/dynamic.rs:20`, `graph/src/graph/dynamic.rs:130`). Keep the maps explicit for clarity, but add high-level registration helpers for common read-only and read-write provider bundles.
4. Relation storage is duplicated between `InMemoryProvider` and `DynamicGraph` overlay: both use `BTreeMap<NodeId, BTreeMap<RelationName, BTreeSet<NodeId>>>` (`graph/src/graph/memory.rs:27`, `graph/src/graph/dynamic.rs:90`). Extracting a small `RelationStore` would centralize sorted target handling, relation lifecycle diffs, and inbound cleanup.
5. `TracedProvider` repeats span/timing boilerplate across every trait method (`graph/src/graph/trace.rs:42`, `graph/src/graph/trace.rs:90`, `graph/src/graph/trace.rs:124`, `graph/src/graph/trace.rs:185`, `graph/src/graph/trace.rs:283`, `graph/src/graph/trace.rs:350`). A local helper/macro can reduce maintenance without changing the public wrapper.
6. Read-only plugin providers are structurally duplicated. D-Bus, Niri, PipeWire, and StatusNotifier all store a kind plus shared `RwLock` state, read state with `with_state`, and delegate `NodeProvider`/`PropertyProvider`/`RelationProvider`/optional `PathProvider` methods (`plugins/dbus/src/provider.rs:15`, `plugins/niri/src/provider.rs:10`, `plugins/pipewire/src/provider.rs:11`, `plugins/statusnotifier/src/provider.rs:11`). A graph-owned projection adapter is likely justified.

## Performance And Concurrency Findings

1. Provider registry lock hygiene is good. Public query paths clone provider `Arc`s out of the registry and release the registry lock before awaiting provider operations (`graph/src/graph/dynamic.rs:323`, `graph/src/graph/dynamic.rs:448`, `graph/src/graph/dynamic.rs:651`). This avoids holding the registry lock across external async calls.
2. `InMemoryProvider` lock usage is also straightforward: guards are held only around map operations and not across nested awaits (`graph/src/graph/memory.rs:41`, `graph/src/graph/memory.rs:92`, `graph/src/graph/memory.rs:188`).
3. Multi-step mutations are not atomic. `create_node` checks existence, calls the mutation provider, then emits changes (`graph/src/graph/dynamic.rs:398`). `set_property` probes `property_spec` before mutation (`graph/src/graph/dynamic.rs:502`). Relation mutation computes before/after snapshots with separate calls (`graph/src/graph/dynamic.rs:593`, `graph/src/graph/dynamic.rs:607`). Concurrent callers can race and emit added/changed events based on stale observations.
4. Relation change calculation swallows errors with `unwrap_or_default` around before/after target reads (`graph/src/graph/dynamic.rs:593`, `graph/src/graph/dynamic.rs:607`, `graph/src/graph/dynamic.rs:622`, `graph/src/graph/dynamic.rs:636`). That is acceptable for `NotFound` if documented, but it can also hide provider errors and produce misleading lifecycle events.
5. Removing a node scans every node provider, every source node, every relation, and every target to clean inbound links (`graph/src/graph/dynamic.rs:651`, `graph/src/graph/dynamic.rs:660`, `graph/src/graph/dynamic.rs:670`, `graph/src/graph/dynamic.rs:676`). That is reasonable for small desktop graphs but will not scale under large or high-churn provider state without indexes or a provider cleanup hook.
6. Fallback watches spawn one task per watch and every task processes the global broadcast stream (`graph/src/graph/dynamic.rs:299`, `graph/src/graph/dynamic.rs:302`). With many watch files, every graph change becomes O(number of watches). A target-indexed watch hub would be a better long-term fit if FUSE watch count grows.
7. Broadcast and mpsc capacities are hard-coded and undocumented: 1024 global changes and 64 fallback watch events (`graph/src/graph/dynamic.rs:97`, `graph/src/graph/dynamic.rs:301`). The public contract should state loss/backpressure behavior or make capacities configurable.
8. `GraphWatch::try_recv` collapses empty and disconnected receiver states to `None` (`graph/src/graph/watch.rs:38`). Low-level watch consumers may need to distinguish "no event yet" from "closed".
9. `TracedProvider` requires a `&'static str` label (`graph/src/graph/trace.rs:14`, `graph/src/graph/trace.rs:20`). Static plugin labels work today, but a dynamic plugin/runtime label may need `Cow<'static, str>` or `Arc<str>`.

## Tidiness And Docs Findings

1. Module roots follow the intended shape: public concepts are re-exported from `lib.rs`, `identity/mod.rs`, `value/mod.rs`, and `graph/mod.rs` (`graph/src/lib.rs:17`, `graph/src/identity/mod.rs:5`, `graph/src/value/mod.rs:5`, `graph/src/graph/mod.rs:18`). That is a good base.
2. `dynamic.rs` is too large for its responsibilities. It mixes subscription wrappers, provider registry, graph facade, relation overlay, watch fallback/filtering, error helpers, and debug formatting in one file (`graph/src/graph/dynamic.rs:31`, `graph/src/graph/dynamic.rs:78`, `graph/src/graph/dynamic.rs:398`, `graph/src/graph/dynamic.rs:843`). Split by responsibility while keeping `DynamicGraph` as the public facade.
3. Tests are module-owned and useful (`graph/src/graph/test.rs:10`, `graph/src/identity/test.rs:3`, `graph/src/value/test.rs:4`). They should be expanded around feature flags and watch filtering, not moved.
4. Public docs need to explain graph semantics that are currently only inferable from tests: hidden/read-only/read-write node access (`graph/src/graph/access.rs:1`), overlay fallback (`graph/src/graph/test.rs:193`), duplicate registration behavior (`graph/src/graph/test.rs:606`), and event emission contracts (`graph/src/graph/test.rs:260`).
5. Error naming is serviceable but broad. `Unsupported { operation: &'static str }` does not include node kind or capability context (`graph/src/error.rs:28`, `graph/src/graph/dynamic.rs:1032`). More context would make plugin registration and missing capability failures easier to diagnose.

## Best-Practice And Crate Reuse Notes

- `async-trait` is justified because provider registries store `Arc<dyn Trait>` and native async trait methods are not enough for this object-safe dyn use case (`graph/src/graph/dynamic.rs:20`, `graph/src/graph/mod.rs:57`).
- `tokio::sync::broadcast` and `mpsc` are appropriate local choices for the current runtime surface; the issue is not crate choice, it is exposing the raw receiver and leaving capacity/loss semantics undocumented (`graph/src/graph/dynamic.rs:31`, `graph/src/graph/dynamic.rs:97`).
- `thiserror` is the right crate for the public error enum, but source preservation should be improved for I/O (`graph/src/error.rs:3`, `graph/src/error.rs:38`).
- A new external crate is not needed for projection helpers. The duplicated provider wrappers are graph-domain-specific, so a small local adapter trait/module is better than adding a generic abstraction dependency.
- CI should include feature-matrix checks. A tool like `cargo hack` could automate this later, but the immediate matrix can be covered with explicit `cargo check -p locusfs-graph --no-default-features --features ...` commands.

## Domain-Specific Filesystem And Plugin Implications

1. The existing `PathProvider`/`GraphPathDirectory::Virtual` model can support the desired D-Bus shape, including `/dbus/<service>/objects` and `/dbus/<service>/methods`, because providers can expose arbitrary virtual directories and map leaves back to properties (`graph/src/graph/mod.rs:31`, `graph/src/graph/mod.rs:42`, `plugins/dbus/src/state.rs:238`, `plugins/dbus/src/state.rs:468`).
2. The current D-Bus implementation still exposes hard-to-maintain metadata names such as `@properties` and `@methods` (`plugins/dbus/src/state.rs:19`, `plugins/dbus/src/state.rs:20`, `plugins/dbus/src/state.rs:21`). Graph should not hard-code D-Bus layout, but it should provide reusable virtual path builders, property-directory helpers, relation-directory helpers, and watch-target helpers so plugins do not each assemble these layouts by hand.
3. `GraphPathChild { name, entry }` is a good generic projection result (`graph/src/graph/mod.rs:51`), but the crate lacks helper APIs for "children from properties", "children from relation targets", or "watch target for this projected directory". D-Bus, PipeWire, and StatusNotifier all reimplement these patterns (`plugins/dbus/src/state.rs:556`, `plugins/pipewire/src/state.rs:168`, `plugins/statusnotifier/src/provider.rs:55`).
4. `PropertySpec::write_only` is enough to model callable file semantics such as method calls without adding a D-Bus-specific method primitive to graph (`graph/src/value/property.rs:37`). Keep method/call policy in plugins; make graph better at projecting those properties into stable path layouts.
5. Graph should own reusable projection helpers, but not plugin schemas. A practical shape is a feature-gated `projection` module with a pure `GraphProjection` trait plus an optional `RwLockProjectionProvider<S>` adapter for state snapshots. This belongs in graph because it only depends on graph contracts and removes repeated provider glue across plugin crates.

## Concrete Refactor Plan

1. Fix feature gates first. Add `#[cfg(feature = "watch-provider")]` to `TracedProvider<P as PathProvider>::watch_target` or adjust feature dependencies, then add feature-matrix verification covering `provider-tracing` alone.
2. Document the public contract. Focus on provider trait expectations, identity validation, `PathName` meaning, relation overlay fallback, watch loss semantics, and change/event variant meaning. Add `#[non_exhaustive]` to public enums if the coordinator wants API stability before downstream exhaustive matching hardens.
3. Pick one change vocabulary. Prefer `GraphChange` as the canonical semantic event, then expose a public target-filter helper such as `GraphWatchTarget::project_change(&self, change: &GraphChange) -> Option<GraphWatchEvent>` or `GraphChangeFilter`. Deprecate redundant `subscribe_global_*`/`emit_global_*` aliases after settling naming.
4. Make watch behavior reusable. Move `watch_event_for_change` out of `dynamic.rs`, add exhaustive tests, map `NodeKindChanged` for `GraphWatchTarget::Kind` if kind watches are intended to observe broad kind refreshes, and decide whether `GraphWatchEvent` should remain distinct from `GraphChange`.
5. Add provider registration helpers without removing granular registration. Suggested additions: `register_read_only_provider(kind/provider)` for node+property+relation, `register_read_write_provider` for all six core capabilities, and a builder for optional path/watch capabilities. Keep individual methods for unusual providers.
6. Normalize provider kind ownership. Either add a lightweight `ProviderKind` trait used by all provider capability adapters or introduce registration structs that pair `NodeKind` and capability once. Avoid requiring callers to pass the same kind repeatedly.
7. Extract relation overlay mechanics. Create a private `RelationStore` with outbound and inbound indexes, lifecycle diff helpers, and cleanup operations. Use it for both `InMemoryProvider` and `DynamicGraph` overlay if it remains private. Revisit whether the overlay should become an explicit provider instead of hidden `DynamicGraph` state.
8. Tighten mutation error semantics. Replace broad `unwrap_or_default` in relation before/after reads with explicit handling for `NotFound` only; propagate unexpected provider errors. Document that lifecycle events are best-effort under concurrent mutation unless stronger serialization is added.
9. Split `dynamic.rs` into focused sibling files: `subscription.rs`, `registry.rs`, `relation_overlay.rs`, `watch_filter.rs`, and the facade implementation. Keep `graph/mod.rs` as the public API surface.
10. Add projection helpers. Start small with graph-owned, feature-gated helpers for read-only state projection and virtual path construction. Migrate one duplicated plugin provider as a proof before moving all plugins.
11. Clean tracing wrapper repetition after behavior is stable. A local helper/macro can reduce instrumentation boilerplate and make it less likely future trait methods miss tracing or cfg gates.

## Test Plan

- Add feature-matrix checks:
  - `cargo check -p locusfs-graph --no-default-features`
  - `cargo check -p locusfs-graph --no-default-features --features dynamic`
  - `cargo check -p locusfs-graph --no-default-features --features in-memory`
  - `cargo check -p locusfs-graph --no-default-features --features watch-provider`
  - `cargo check -p locusfs-graph --no-default-features --features provider-tracing`
  - `cargo check -p locusfs-graph --no-default-features --features provider-tracing,watch-provider`
  - `cargo check -p locusfs-graph --all-features`
- Keep `cargo test -p locusfs-graph --all-features` as the main graph behavior check.
- Add unit tests for every `GraphWatchTarget` against relevant `GraphChange` variants, including `NodeKindChanged`, node removal invalidation for property/relation/node-child targets, and lag fallback behavior.
- Add tests for `GraphWatch::try_recv` closed/empty behavior if the API is changed to expose that distinction.
- Add relation overlay tests for provider errors during before/after snapshots, duplicate links, cross-provider links, inbound cleanup, outbound cleanup, and concurrent-looking operation ordering.
- Add provider registration helper tests that prove bundle registration detects duplicate capabilities and preserves granular registration behavior.
- Add projection helper tests using a tiny read-only state fixture before migrating plugin providers.
- After graph changes, run at least `cargo test -p locusfs-graph --all-features`, then broaden to workspace tests relevant to FUSE/plugin consumers.

## Open Questions For Coordinator Arbitration

1. Is `locusfs-graph` intended to be a stable public API now, or is this still pre-stabilization? This determines whether to add `#[non_exhaustive]`, deprecate aliases, and hide raw Tokio receiver types immediately.
2. Should `GraphChange` and `GraphWatchEvent` remain separate, or should target watches emit filtered `GraphChange` plus a generic invalidation signal?
3. Should relation overlay state be owned by `DynamicGraph`, by a dedicated graph-level relation provider/store, or by plugins/host registration policy?
4. Should `PathName` be a literal filesystem segment or an unencoded graph child display name? The current validation allows `/`, so the answer affects both docs and validation.
5. Should graph own the projection helper module directly, or should it live in `locusfs-plugin-api` to avoid putting provider-convenience runtime code in the core contract crate?
6. What watcher and graph sizes should be expected? If many watch files are normal, a target-indexed watch hub should be prioritized over the current per-watch task fallback.
7. Should plugins continue publishing changes through public `DynamicGraph::emit_*`, or should change publication move behind a narrower host/plugin context capability?
8. For D-Bus methods, is the write-only `call` property the long-term model, or does graph need a first-class callable value/property convention shared across plugins?
