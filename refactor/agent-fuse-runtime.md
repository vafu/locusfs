# locusfs-fuse Runtime Review

## Current Role

`locusfs-fuse` is the runtime adapter that exposes a `DynamicGraph` as a FUSE filesystem. It owns mount lifecycle, FUSE request translation, public path encoding helpers, inode tracking, kernel invalidation, property file reads/writes, symlink-backed relation navigation, and `/watch` event production.

The crate-level contract says graph semantics should stay in `locusfs-graph` while this crate owns mount lifecycle, public filesystem layout, and kernel request translation (`fuse/src/lib.rs:1`). The implementation mostly follows that direction, but several graph layout and provider policies are embedded directly in FUSE operation code.

## Public API And Entrypoints

- `FuseError` and `Result` are re-exported from `lib.rs` (`fuse/src/lib.rs:12`) with mount/unmount and graph error variants (`fuse/src/error.rs:5`).
- `LocusFs` is public (`fuse/src/lib.rs:14`) and constructible from a `DynamicGraph` through `LocusFs::new` (`fuse/src/fs/filesystem.rs:59`). This is the low-level FUSE adapter for custom sessions.
- `layout` is public (`fuse/src/lib.rs:9`) and re-exports `Layout`, `encode_segment`, and `decode_segment` (`fuse/src/lib.rs:15`, `fuse/src/layout/mod.rs:5`).
- `FuseMountConfig`, `FuseMount`, and `mount` are the high-level API (`fuse/src/lib.rs:16`). `mount` builds `MountOptions`, creates a `Session<LocusFs>`, starts the invalidation worker, and returns a live mount handle (`fuse/src/mount.rs:51`).
- `FuseMountConfig` currently carries only a mountpoint (`fuse/src/mount.rs:15`), and `mount` hard-codes fs name and subtype (`fuse/src/mount.rs:58`).
- `FuseMount::unmount` shuts down the change worker and then unmounts the `MountHandle` (`fuse/src/mount.rs:39`).

## Step-By-Step Walkthrough

1. `fuse/src/lib.rs`: crate boundary and public re-exports. It is the discovery surface for consumers.
2. `fuse/src/mount.rs`: high-level mount lifecycle. It creates shared inode/watch/notifier state, mounts the FUSE session, and spawns the graph-change invalidation worker.
3. `fuse/src/layout/mod.rs`: public path builder for generic graph paths. It currently covers `/watch`, kind dirs, node dirs, property files, relation dirs, and nominal relation target links.
4. `fuse/src/layout/segment.rs`: percent-style path segment encoding and decoding.
5. `fuse/src/error.rs`: public mount errors and crate-local `GraphError` to `Errno` conversion.
6. `fuse/src/fs/mod.rs`: private module boundary for the FUSE implementation. It re-exports crate-local state types to invalidation and mount code.
7. `fuse/src/fs/entry.rs`: internal `FsEntry` inode identity model, relation symlink target builders, root/watch constants, and parent entry logic.
8. `fuse/src/fs/filesystem.rs`: main `fuse3::raw::Filesystem` implementation. It handles lookup, attrs, reads, writes, mkdir/create/symlink/unlink/rmdir, poll, readdir, readdirplus, and helper methods.
9. `fuse/src/fs/directory.rs`: directory listing policy for root, kind dirs, node dirs, relation dirs, and provider-owned path dirs.
10. `fuse/src/fs/name.rs`: conversion between FUSE path segments and graph identifiers plus relation target display-name policy.
11. `fuse/src/fs/resolve.rs`: `/watch` subscription parsing and path-to-watch-target resolution.
12. `fuse/src/fs/watch.rs`: watch registry, property-file poll state, `/watch` event queues, graph-watch forwarder state, dependency indexes, and protocol event conversion.
13. `fuse/src/fs/inode.rs`: inode allocation, lookup counts, entry-to-inode indexes, and cached timestamps.
14. `fuse/src/fs/value.rs`: property read/write formatting, parsing, default missing-property spec, and permission mapping.
15. `fuse/src/fs/attr.rs`: FUSE `FileAttr` construction, TTL, timestamps, uid/gid, and simplified link count.
16. `fuse/src/invalidation.rs`: background worker translating `GraphChange` into inode invalidations, watch notifications, and watch retargeting.
17. `fuse/src/fs/test.rs`, `fuse/src/layout/test.rs`, `fuse/tests/fuse_smoke.rs`: unit coverage for inodes, layout encoding, watch registry/resolution, relation naming, property writes, and one ignored real-FUSE smoke test.

## Behavior Summary

- Root contains a fixed `watch` file plus readable node-kind directories (`fuse/src/fs/directory.rs:28`).
- Kind directories contain node-local directories for nodes of that kind (`fuse/src/fs/directory.rs:62`).
- Node directories are an overlay of provider path children, graph properties, and graph relations (`fuse/src/fs/directory.rs:78`). Provider path children win over generic graph children on name collision; property/relation collisions outside provider names return `EIO` (`fuse/src/fs/directory.rs:97`, `fuse/src/fs/directory.rs:116`).
- A property is a regular file. Reads return `display_string()` plus a newline (`fuse/src/fs/value.rs:104`), writes parse strings, bools, integers, and finite floats after stripping one trailing newline (`fuse/src/fs/value.rs:49`).
- A relation with one target appears as a symlink at the relation name, while a relation with multiple targets appears as a directory containing target symlinks (`fuse/src/fs/filesystem.rs:217`, `fuse/src/fs/directory.rs:124`).
- Relation target names in multi-target relation directories use a compact display-name heuristic and fall back to the full `NodeId` when needed (`fuse/src/fs/name.rs:97`).
- Symlinks point back to `../../<kind>/<local>` or `../../../<kind>/<local>` depending on whether the symlink is direct or nested (`fuse/src/fs/entry.rs:72`).
- Root `mkdir` creates a new writable `InMemoryProvider` for the requested kind and registers it for node, property, and relation reads/writes (`fuse/src/fs/filesystem.rs:396`).
- Property-file `poll` is generation based: opening a property captures the current generation, graph changes bump the generation, and reading marks the handle as seen (`fuse/src/fs/watch.rs:145`, `fuse/src/fs/watch.rs:172`).
- Writing a logical path to `/watch` configures that open file handle. Non-directory paths produce state watches, trailing-slash paths produce change watches, and simple dependency-free change watches may be forwarded from `graph.watch` instead of the FUSE global-change invalidator (`fuse/src/fs/filesystem.rs:981`).
- `/watch` state events coalesce to the latest state; change events are bounded to 256 pending events and drop the oldest when full (`fuse/src/fs/watch.rs:75`, `fuse/src/fs/watch.rs:566`, `fuse/src/fs/watch.rs:583`).
- The invalidation worker subscribes to global graph changes and translates each change into inode invalidation, watch event notification, and state-watch retargeting (`fuse/src/invalidation.rs:36`, `fuse/src/invalidation.rs:77`).
- On broadcast lag, invalidation touches and invalidates all known inodes and queues generic changes for watch handles (`fuse/src/invalidation.rs:60`, `fuse/src/invalidation.rs:344`).

## API Findings

- The high-level `mount` API is usable and small, but not yet flexible enough for likely consumers. `FuseMountConfig` only supports a mountpoint (`fuse/src/mount.rs:15`), while fs name, subtype, unprivileged mount, TTL, max write, dynamic in-memory kind creation, and layout policy are hard-coded (`fuse/src/mount.rs:58`, `fuse/src/fs/attr.rs:6`, `fuse/src/fs/filesystem.rs:489`). A conservative next step is a builder with defaults, not a large config enum.
- `LocusFs` is public but documented only as a FUSE request adapter (`fuse/src/fs/filesystem.rs:50`). If custom mounting is an intended external API, it needs docs about lifecycle, `DynamicGraph` requirements, thread model, and what operations mutate the graph. If not, consider keeping `LocusFs` public only behind a clearly documented advanced API.
- The public `Layout` API does not fully match runtime behavior. `Layout::node_relation_target_link` encodes `target.to_string()` as the directory child (`fuse/src/layout/mod.rs:42`), but runtime relation directories use compact unique target names (`fuse/src/fs/directory.rs:148`, `fuse/src/fs/name.rs:97`). `Layout::node_relation_link` ignores the target and returns the relation path (`fuse/src/layout/mod.rs:34`), but that path is only a symlink when the relation has exactly one target (`fuse/src/fs/filesystem.rs:217`). This makes `Layout` unsafe as a stable consumer helper unless relation cardinality and display-name policy are part of its contract.
- `Layout` returns `locusfs_graph::Result` (`fuse/src/layout/mod.rs:7`) and path encoding returns `GraphError` (`fuse/src/layout/segment.rs:1`). That may be acceptable because graph identity validation is reused, but it means public layout helpers expose graph error vocabulary from the FUSE crate. A FUSE-local `LayoutError` would decouple consumers from graph internals if the layout is meant to be stable.
- Root `mkdir` creating an `InMemoryProvider` is the clearest API-boundary leak. A FUSE adapter should normally mutate existing graph data through registered mutation providers, not invent a concrete provider implementation at runtime (`fuse/src/fs/filesystem.rs:411`). If this behavior is needed for development, make it an explicit mount option or move it into the graph/bootstrap layer.
- Error conversion is centralized for graph errors (`fuse/src/error.rs:18`), but operation-level errno choices are scattered through `filesystem.rs`, `resolve.rs`, and `watch.rs`. This is normal for a FUSE adapter, but repeated semantic cases like "property/relation collision" and "missing relation means empty target set" should become named helper outcomes.

## Redundancy Findings

- Node-child resolution is duplicated across lookup, directory listing, watch resolution, and unlink. `lookup_graph_node_child` handles property-vs-relation collisions and relation cardinality (`fuse/src/fs/filesystem.rs:199`), `dir_entries` repeats the collision and cardinality policy for listing (`fuse/src/fs/directory.rs:78`), `resolve_watch_path` and `resolve_graph_node_path` duplicate the same walk for watch subscriptions (`fuse/src/fs/resolve.rs:64`, `fuse/src/fs/resolve.rs:245`), and `unlink` repeats property/relation detection (`fuse/src/fs/filesystem.rs:824`).
- Provider path entry conversion is duplicated as `LocusFs::path_entry` (`fuse/src/fs/filesystem.rs:228`) and a free `path_entry` in `directory.rs` (`fuse/src/fs/directory.rs:199`).
- Relation target lookup and display-name policy are split between `name.rs`, lookup, listing, symlink creation, and watch path resolution (`fuse/src/fs/name.rs:51`, `fuse/src/fs/filesystem.rs:130`, `fuse/src/fs/filesystem.rs:794`, `fuse/src/fs/resolve.rs:128`).
- `relation_targets` helpers exist both on `LocusFs` and in `resolve.rs`, with the same "NotFound means empty" policy (`fuse/src/fs/filesystem.rs:1093`, `fuse/src/fs/resolve.rs:394`).
- Path construction is spread across `Layout`, `entry.rs`, `watch_subject_path`, and direct formatting code (`fuse/src/layout/mod.rs:13`, `fuse/src/fs/entry.rs:72`, `fuse/src/fs/watch.rs:772`). This is why public `Layout` has already drifted from runtime relation-dir naming.
- Invalidation repeats the same clone-heavy shape for added/changed/removed node, property, and relation changes (`fuse/src/invalidation.rs:85`). This would be easier to audit as a data-driven `InvalidationPlan` per graph change.
- `invalidate_known_child` accepts a computed encoded name but ignores it (`fuse/src/invalidation.rs:371`). The current `fuse3` dependency exposes `Notify::invalid_entry`; either use entry invalidation or remove the name parameter and avoid encoding it.

## Performance And Concurrency Findings

- The code generally avoids holding `tokio::Mutex` guards across graph `.await` calls. Inode and watch locks are usually held only for local state mutations, which is a good baseline.
- Directory reads allocate the full `Vec<DirEntry>` before applying the FUSE offset (`fuse/src/fs/filesystem.rs:541`, `fuse/src/fs/directory.rs:13`). `readdirplus` then reacquires entries and computes attrs for every returned item (`fuse/src/fs/filesystem.rs:897`). Large node kinds, relation directories, or provider directories will do avoidable allocation and graph work.
- `dir_entries` allocates inodes for all listed children even when the caller starts at a later offset (`fuse/src/fs/directory.rs:69`, `fuse/src/fs/directory.rs:111`, `fuse/src/fs/directory.rs:151`). Consider offset-aware listing or a paged iterator if large directories are expected.
- Relation target display names are effectively O(n^2): each target name checks uniqueness by scanning all targets (`fuse/src/fs/name.rs:97`, `fuse/src/fs/name.rs:120`), and listing calls this for every target (`fuse/src/fs/directory.rs:148`). Precompute a display-name map for each target set.
- `InodeTable` stores cloned `FsEntry` keys in both directions and a third timestamp map (`fuse/src/fs/inode.rs:14`). This is fine for small mounts, but `PathDir` and `PathLink` entries contain boxed parent chains (`fuse/src/fs/entry.rs:29`), so deep provider paths can amplify clone and hash cost.
- `resync_known_state` on broadcast lag does not refresh state-mode `/watch` subscriptions. It queues `WatchChange::Change` for all watch handles via `notify_all` (`fuse/src/invalidation.rs:344`, `fuse/src/fs/watch.rs:449`), but `queue_watch_change` ignores state-mode handles (`fuse/src/fs/watch.rs:592`). State watches can remain stale after a lag event unless a later targeted change refreshes them.
- `WatchRegistry::poll` for `/watch` checks only `pending_events`, not `pending_read` (`fuse/src/fs/watch.rs:306`). `has_unread_change` treats `pending_read` as unread (`fuse/src/fs/watch.rs:714`), so a client that partially reads an event and then polls for the remaining bytes may not wake.
- `notify_poll_handles` and `wake_poll_handles` lock and clone the notifier once per poll handle (`fuse/src/invalidation.rs:526`, `fuse/src/fs/filesystem.rs:480`). Clone the current notifier once before the loop.
- The watch registry is protected by one mutex (`fuse/src/fs/watch.rs:16`). That is acceptable for the current design, but high-cardinality watch fanout will serialize event queuing, retargeting, and poll registration. The refactor should preserve short critical sections and avoid graph calls while the lock is held.
- `Filesystem::write` copies every write into a `Vec<u8>` before entering the async body (`fuse/src/fs/filesystem.rs:682`). This is probably imposed by the `fuse3` async trait shape, but the configured `max_write` is 128 KiB (`fuse/src/fs/filesystem.rs:489`), so it should be documented as an expected bound.

## Tidiness And Docs Findings

- The crate root has a useful boundary statement (`fuse/src/lib.rs:1`), but public items need more documentation at their discovery points. `FuseMountConfig::new`, `FuseMount::unmount`, `mount`, `LocusFs::new`, `Layout`, and the segment encoding helpers all need invariants and expected usage documented.
- `/watch` behavior is underdocumented in this crate. The important rules are trailing slash means change mode, non-trailing path means state mode, simple change watches may be graph-provider backed, state events coalesce, change events are bounded, and overflow drops oldest events.
- The property file format is implicit. Reads add one trailing newline, writes strip one trailing newline, and missing property writes create string properties (`fuse/src/fs/value.rs:17`, `fuse/src/fs/value.rs:49`, `fuse/src/fs/value.rs:104`). This should be documented because it affects shell use and string values ending in newline.
- `filesystem.rs` is doing too much. It owns core lookup policy, mutation policy, FUSE trait methods, graph provider registration, watch configuration, and utility guards. Splitting by responsibility would make future changes safer without changing external behavior.
- `entry.rs` mixes inode identity, parent derivation, relation symlink target formatting, and constants. Relation target formatting belongs with layout/path policy, not inode identity (`fuse/src/fs/entry.rs:72`).
- `attr.rs` has a TODO for directory link counts and hard-codes `nlink = 1` (`fuse/src/fs/attr.rs:46`). That may be fine for now, but the TODO should be converted into an explicit known limitation in docs or tested if clients depend on link counts.
- Tests are broad and useful, but `fs/test.rs` is very large and covers multiple subsystems. Future refactors should move tests into sibling module test files by responsibility.

## Best-Practice And Crate Reuse Notes

- Keep using `fuse3` as the FUSE boundary. The local `fuse3` version exposes `Notify::invalid_entry`, so child invalidation should use it where possible instead of invalidating whole parent inodes and ignoring child names.
- The custom percent encoder is small and has domain-specific validation for empty, dot, dot-dot, and NUL segments (`fuse/src/layout/segment.rs:52`). Replacing it with `percent-encoding` is optional; the higher-value improvement is documenting the encoding contract and making all path builders use one implementation.
- Continue using `locusfs-watch` for event vocabulary, but the producer/consumer protocol needs a cross-crate decision. FUSE emits raw `node.to_string()`, `key.to_string()`, and `relation.to_string()` into text events (`fuse/src/fs/watch.rs:821`, `fuse/src/fs/watch.rs:828`, `fuse/src/fs/watch.rs:850`), while the protocol parser splits on whitespace (`watch/src/protocol.rs:120`). Graph identifiers allow whitespace except NUL/reserved path segments, so some legal graph names cannot round-trip through watch events.
- Prefer extracting pure planning/resolution helpers over introducing new dependencies. Most complexity here is domain policy, not missing library functionality.
- Avoid adding an abstraction that hides FUSE semantics completely. The useful boundary is a FUSE-local resolver that returns `FsEntry`, watch target, invalidation plan, and directory entries from one shared policy.

## Domain-Specific Filesystem Layout Notes

- The current generic node layout is flat: provider path children, properties, and relations all compete for names directly under a node. That is simple for small graphs, but hard to maintain for rich plugins such as DBus where methods, objects, properties, and actions need readable structure.
- `PathProvider` already gives plugins a way to expose structured layouts (`graph/src/graph/mod.rs:113`), and FUSE consults it before generic properties/relations (`fuse/src/fs/filesystem.rs:165`, `fuse/src/fs/directory.rs:81`, `fuse/src/fs/resolve.rs:153`). The missing piece is a documented standard for how providers should use it.
- Provider children silently shadow generic graph properties/relations of the same encoded name (`fuse/src/fs/directory.rs:87`, `fuse/src/fs/directory.rs:105`). This should be a documented rule or changed to an explicit conflict error. Silent shadowing can hide real graph data from filesystem users.
- Relation cardinality changes the file type at the relation path. A relation path is a symlink with one target, a directory with multiple targets, and absent with zero targets (`fuse/src/fs/filesystem.rs:217`, `fuse/src/fs/directory.rs:124`). That is ergonomic for single-target relations but unstable for clients and watch paths when cardinality changes. A stable relation directory layout is worth considering.
- For the DBus direction in `AGENTS.md`, FUSE should not hard-code `/dbus/<service>/methods` and `/dbus/<service>/objects` itself. Instead, the FUSE crate should provide a clear layout contract and resolver behavior that make those plugin-owned paths first-class, testable, and non-conflicting.
- A likely standard is:
  - Generic graph fallback stays available for simple providers.
  - Complex providers expose structured `PathProvider` layouts such as `methods/`, `objects/`, `properties/`, and `relations/`.
  - FUSE documents provider children as the preferred public layout when present.
  - Generic flat properties/relations are treated as compatibility fallback, not the recommended plugin layout for complex domains.

## Concrete Refactor Plan

1. Write down the intended public filesystem contract before editing runtime code.
   - Decide whether relation paths should stay cardinality-dependent or become stable directories.
   - Decide whether provider children shadow generic graph children or collisions are errors.
   - Decide whether root `mkdir` may create in-memory kinds in production.
   - Update `Layout` docs and tests to match the chosen contract.

2. Introduce a FUSE-local layout resolver.
   - New module candidate: `fuse/src/fs/resolver.rs` or `fuse/src/fs/layout.rs`.
   - Own operations like `resolve_child(parent, name)`, `list_children(entry)`, `resolve_relation_target_name`, `entry_for_path_provider_child`, and `relation_targets`.
   - Return typed outcomes such as `ResolvedChild`, `NodeChildKind`, `RelationEntryKind`, and `CollisionPolicy`.
   - Make lookup, readdir, unlink/rmdir, symlink creation, and watch path resolution call the same resolver.

3. Make `Layout` accurate or narrower.
   - If `Layout` remains public, it should build only paths whose names are deterministic without live graph state.
   - Relation target child paths either need a graph-backed helper that can apply compact-name policy or should be removed/renamed to avoid promising a path the runtime may not expose.
   - Move relation symlink target construction out of `entry.rs` and behind the same layout module.

4. Remove FUSE ownership of graph implementation policy.
   - Gate root `mkdir` in-memory provider creation behind an explicit config, or remove it from the default mount path.
   - If retained, document it as development/test behavior and add a mount option like `allow_dynamic_kinds`.

5. Refactor watch resolution and producer semantics.
   - Parse `/watch` paths through the same resolver used by lookup and directory listing.
   - Make state-vs-change mode explicit in a small type rather than inferring it repeatedly from trailing slash.
   - Treat `pending_read` as poll-readable.
   - On queue overflow, consider emitting a coarse `change`/resync event or documenting "oldest events are dropped".
   - Coordinate with `locusfs-watch` on escaping/encoding event identifiers and property values.

6. Refactor invalidation around an explicit plan.
   - Convert `GraphChange` to an `InvalidationPlan` containing affected parent entries, child entry names, specific entries, relation entries, and watch refresh subjects.
   - Use `Notify::invalid_entry(parent, name)` for child changes when available, and use inode invalidation for known entry attrs/content.
   - Fix lag resync to refresh state watches, not just change watches.
   - Clone the current notifier once per batch.

7. Split implementation files only where it removes real complexity.
   - Keep `filesystem.rs` as the thin `Filesystem` trait adapter and operation glue.
   - Move graph/path resolution into the resolver module.
   - Move watch registry tests near `watch.rs` and resolver tests near the resolver.
   - Keep small helpers private until a real external contract exists.

8. Optimize hot paths after semantic consolidation.
   - Make directory listing offset-aware or at least avoid unnecessary attr work before offsets.
   - Precompute relation display names for target sets.
   - Avoid duplicate encoding and cloning in invalidation and relation listing.

## Test Plan

- Keep `cargo test -p locusfs-fuse` as the narrow verification baseline. Current result: 52 unit tests passed; the real-FUSE smoke test is ignored because it requires `/dev/fuse`.
- Add resolver unit tests for:
  - provider child shadowing versus collision errors, depending on the chosen policy;
  - property/relation name collisions;
  - one-target, zero-target, and multi-target relations;
  - provider virtual directory lookup and listing;
  - parity between lookup, listing, unlink, and watch path resolution.
- Add layout tests for:
  - public `Layout` relation target behavior after the API decision;
  - symlink target paths from direct and nested relation entries;
  - encoded segments with spaces, slashes, percent signs, UTF-8, dot, dot-dot, and NUL.
- Add watch registry tests for:
  - partial event read followed by poll wakes because `pending_read` still exists;
  - state watch refresh on broadcast lag/resync;
  - overflow behavior for change watches and state-watch coalescing;
  - graph-watch backed handles still suppress duplicate global invalidation events;
  - identifiers with whitespace or an agreed escaping rule.
- Add invalidation-plan tests against pure data structures before involving `fuse3::Notify`.
- Add property value tests for newline stripping, strings that intentionally end with newlines, invalid UTF-8 writes, finite/non-finite floats, and missing property creation.
- Keep the ignored `fuse_smoke` test, but add a documented command for maintainers with `/dev/fuse` access to run it. Consider a separate `#[ignore]` smoke for provider path layouts once DBus-style paths exist.

## Open Questions For Coordinator Arbitration

- Should relation paths be cardinality-dependent symlink-or-directory entries, or should every relation be represented by a stable directory?
- Is root `mkdir` creating an `InMemoryProvider` intended production behavior, development scaffolding, or something to remove?
- Is public `Layout` meant to be a stable consumer API, or only a convenience helper for tests and examples?
- Should provider path children shadow generic graph children, or should collisions be visible errors?
- Should complex plugins be required to expose structured `PathProvider` layouts, with generic flat properties/relations treated as fallback?
- What is the compatibility expectation for the current `/watch` text protocol, especially identifiers with whitespace and property values beginning with `/`?
- On watch queue overflow or broadcast lag, should consumers receive a coarse "resync" event distinct from ordinary `change`?
- Are FUSE mount knobs such as TTL, max write size, unprivileged mount, fs name, subtype, and dynamic-kind creation expected to be public configuration now?
