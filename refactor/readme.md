# Locusfs Architecture Review

## Scope

Global review of the current Rust workspace:

- `locusfs-graph`
- `locusfs-plugin-api`
- `locusfs-fuse`
- `locusfs-client`
- `locusfs`
- `plugins/dbus`
- `plugins/niri`
- `plugins/pipewire`
- `plugins/project`

The review focused on app architecture, async/runtime behavior, graph/watch contracts, FUSE cache correctness, plugin loading, duplicated abstractions, and practical quality steps.

## High-Level Shape

`locusfs-graph` is the core contract: typed graph identifiers, provider traits, in-memory provider, change emission, and graph watch fallback.

`locusfs-fuse` adapts the graph into a filesystem layout and owns inode state, path resolution, FUSE callbacks, kernel invalidation, and `/watch`.

`locusfs-client` is a small async helper around mounted paths and `/watch`.

`locusfs-plugin-api` defines the dynamic plugin contract.

`locusfs` owns config, plugin loading, graph assembly, mount lifecycle, and CLI watch mode.

Plugins provide domain state snapshots and expose them through graph providers.

## Strengths

- The graph contract is small, typed, and flexible enough for read-only and writable providers.
- Provider capabilities are split cleanly across node/property/relation read and mutation traits.
- FUSE internals are divided into reasonable modules: filesystem, directory, inode, resolve, watch, invalidation, layout.
- Plugin-local config keeps the host generic while allowing typed validation inside each plugin.
- Domain plugins generally separate runtime I/O from state projection.
- Tests already cover graph basics, path encoding, FUSE unit behavior, plugin config, and plugin state projection.

## Priority Findings

Status from 2026-06-18 implementation pass:

- Addressed: explicit async plugin shutdown now aborts and awaits plugin tasks.
- Addressed: FUSE invalidation now runs even when watchers are notified.
- Addressed: graph overlay cleanup now removes outbound links for removed source nodes.
- Addressed: D-Bus object IDs now round-trip for objects outside the configured object manager path.
- Addressed: FUSE watch subjects/events now use graph watch target/event types directly.
- Addressed: duplicate provider registration is rejected instead of silently replacing providers.
- Addressed: enabled plugin load failures now fail graph/plugin startup.
- Addressed: D-Bus and Niri event streams reconnect after failure.
- Addressed: D-Bus object snapshot failures no longer publish empty object state.
- Addressed: watch pending event queues are bounded.
- Addressed: inode timestamp cache entries are removed when inodes are forgotten.
- Addressed: `locusfs-client` has timeout read and wait/read APIs.

Remaining priority findings:

1. The dynamic plugin ABI is a Rust trait object crossing a `.so` boundary. This is acceptable only for same-workspace/same-toolchain plugins; it should be documented and eventually replaced or constrained.

2. Graph fallback watch matching and FUSE dependency retargeting still overlap in behavior and need a tighter conformance boundary.

3. Runtime reconnection lacks dedicated simulated-failure tests for D-Bus and Niri.

4. Relation/property namespace collisions and relation file-type transitions still need an explicit long-term filesystem policy.

5. The plugin ABI and manifest compatibility story is still unstable.

## Redundancy Themes

- `GraphWatchTarget`/`GraphWatchEvent` duplicate FUSE `WatchSubjectKey`/`WatchEvent`.
- Graph fallback watch matching and FUSE watch fanout overlap.
- Read-only plugin provider adapters are nearly identical across D-Bus, Niri, and PipeWire.
- Plugin registration loops repeat: create kind, wrap `TracedProvider`, register node/property/relation providers.
- `config_error`, `node_not_found`, `relation`, `string`, and insert helpers repeat across plugins.
- `subscribe_changes`/`subscribe_global_changes`, `emit_change`/`emit_global_change`, and stream aliases duplicate vocabulary.
- `DynamicGraph` relation overlay and `InMemoryProvider` each maintain relation maps.

## Recommended Refactor Order

1. Fix correctness bugs first:
   - Done: FUSE invalidation must always run even when watchers are notified.
   - Done: Graph overlay must remove outbound links for removed source nodes.
   - Done: D-Bus object IDs must round-trip.

2. Stabilize lifecycle:
   - Done: Add explicit async plugin shutdown.
   - Done: Make PipeWire/D-Bus/Niri task cancellation joined before library unload.
   - Done: Add reconnect/backoff loops for long-lived provider runtimes.

3. Tighten graph contracts:
   - Done: Decide duplicate provider registration semantics.
   - Split missing provider/capability/data errors.
   - Make mutation changes idempotence-aware.
   - Partly done: FUSE uses graph watch target/event types; remaining work is shared filtering/conformance helpers.

4. Clean FUSE/client quality issues:
   - Done: Bound watch queues.
   - Done: Clean inode timestamp state on forget/removal.
   - Done: Add client timeout/cancellation option for disappearing paths.
   - Add real FUSE watch integration coverage behind the existing ignored gate.

5. Remove plugin boilerplate:
   - Extract read-only snapshot provider adapter only after behavior is stable.
   - Extract plugin kind registration helper.
   - Centralize simple plugin config/error helpers.

6. Make dynamic config operationally predictable:
   - Resolve config paths relative to the config file.
   - Done: Enabled plugin failure fails mount by default.
   - Remove or implement unused config fields like project `state_path`.

## Verification Baseline

Subagent verification runs reported:

- `cargo test -p locusfs-graph -p locusfs-plugin-api`
- `cargo test -p locusfs-fuse -p locusfs-client`
- `cargo test -p locusfs-plugin-dbus -p locusfs-plugin-niri`
- `cargo test -p locusfs -p locusfs-plugin-pipewire -p locusfs-plugin-project`

The real FUSE smoke test remains ignored unless run on a host with `/dev/fuse`:

```sh
cargo test -p locusfs-fuse --test fuse_smoke -- --ignored
```

## Open Questions

- Should dynamic plugins be same-workspace only for now, or should the next pass introduce a stable ABI?
- Should an explicitly enabled plugin failing to load fail the entire mount?
- Should filesystem relation paths keep the current single-target symlink/multi-target directory shape, or should relations always be directories to avoid file type changes?
- Should graph access permissions be declared by providers or derived from registered mutation capabilities?
