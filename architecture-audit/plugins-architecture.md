# Plugins Architecture Audit

Scope: `plugins/dbus/`, `plugins/niri/`, `plugins/pipewire/`, and `plugins/project/`.

## Observations

### Repeated read-only provider wrappers

Fact: `DbusProvider`, `NiriProvider`, and `PipeWireProvider` are structurally the same wrapper: each stores a `NodeKind` plus shared async state, exposes a `new` constructor, reads the state through `with_state`, and implements `NodeProvider`, `PropertyProvider`, and `RelationProvider` by delegation. See `plugins/dbus/src/provider.rs:9`, `plugins/dbus/src/provider.rs:20`, `plugins/dbus/src/provider.rs:27`, `plugins/niri/src/provider.rs:10`, `plugins/niri/src/provider.rs:21`, `plugins/niri/src/provider.rs:27`, `plugins/pipewire/src/provider.rs:9`, `plugins/pipewire/src/provider.rs:20`, and `plugins/pipewire/src/provider.rs:29`.

Fact: The graph crate already has wrappers like `TracedProvider`, but that wrapper adds instrumentation around an existing provider; it does not remove the repeated state-lock/delegate provider implementation. See `locusfs-graph/src/graph/trace.rs:13` and `locusfs-graph/src/graph/trace.rs:28`.

Recommendation: Introduce a small shared read-only projection provider abstraction, probably in `locusfs-graph` or `plugins/api` (`locusfs-plugin-api`), that can wrap `Arc<RwLock<S>>` where `S` implements a local projection trait. This would let plugin crates define their domain projection once and avoid three near-identical provider files. Keep it read-only; `ProjectPlugin` already uses `InMemoryProvider` for read-write behavior.

### Repeated plugin registration boilerplate

Fact: D-Bus, Niri, and PipeWire all register one provider per kind, wrap it with `TracedProvider`, and register node, property, and relation provider surfaces in a loop. See `plugins/dbus/src/lib.rs:67`, `plugins/dbus/src/lib.rs:73`, `plugins/dbus/src/lib.rs:75`, `plugins/dbus/src/lib.rs:77`, `plugins/niri/src/lib.rs:77`, `plugins/niri/src/lib.rs:81`, `plugins/niri/src/lib.rs:83`, `plugins/niri/src/lib.rs:85`, `plugins/pipewire/src/lib.rs:70`, `plugins/pipewire/src/lib.rs:74`, `plugins/pipewire/src/lib.rs:76`, and `plugins/pipewire/src/lib.rs:78`.

Fact: `ProjectPlugin` repeats the same registration shape for the mutable in-memory provider, but must register the mutation surfaces too. See `plugins/project/src/lib.rs:40`, `plugins/project/src/lib.rs:43`, `plugins/project/src/lib.rs:47`, `plugins/project/src/lib.rs:50`, `plugins/project/src/lib.rs:53`, `plugins/project/src/lib.rs:56`, and `plugins/project/src/lib.rs:59`.

Recommendation: Add a shared helper for "register this traced provider for read-only node/property/relation APIs" and possibly a second helper for full read-write provider registration. This is API cleanup more than behavior cleanup: it would make each plugin's `register_with_config_and_runtime` show only its domain startup and supported kinds.

### Repeated state projection APIs

Fact: D-Bus, Niri, and PipeWire states all expose the same query API: `contains_node`, `nodes`, `property_spec`, `properties`, `property`, `relations`, and `targets`. The implementations are similar enough that the control flow is duplicated, but each crate supplies domain-specific node selection, property maps, relation maps, and change computation. See `plugins/dbus/src/state.rs:97`, `plugins/dbus/src/state.rs:105`, `plugins/dbus/src/state.rs:128`, `plugins/dbus/src/state.rs:139`, `plugins/dbus/src/state.rs:147`, `plugins/dbus/src/state.rs:156`, `plugins/dbus/src/state.rs:160`; `plugins/niri/src/state.rs:45`, `plugins/niri/src/state.rs:55`, `plugins/niri/src/state.rs:83`, `plugins/niri/src/state.rs:94`, `plugins/niri/src/state.rs:102`, `plugins/niri/src/state.rs:111`, `plugins/niri/src/state.rs:115`; and `plugins/pipewire/src/state.rs:69`, `plugins/pipewire/src/state.rs:78`, `plugins/pipewire/src/state.rs:102`, `plugins/pipewire/src/state.rs:113`, `plugins/pipewire/src/state.rs:121`, `plugins/pipewire/src/state.rs:130`, `plugins/pipewire/src/state.rs:134`.

Fact: The three crates also duplicate the same "derive `PropertySpec` from a materialized property map" pattern and the same "remove requested key from a temporary map or return `NotFound`" pattern. See `plugins/dbus/src/state.rs:128`, `plugins/dbus/src/state.rs:139`, `plugins/dbus/src/state.rs:147`; `plugins/niri/src/state.rs:83`, `plugins/niri/src/state.rs:94`, `plugins/niri/src/state.rs:102`; and `plugins/pipewire/src/state.rs:102`, `plugins/pipewire/src/state.rs:113`, `plugins/pipewire/src/state.rs:121`.

Recommendation: Separate the repeated graph-facing query mechanics from each plugin's projection data. A trait like `ProjectedState { fn nodes_for_kind; fn node_properties; fn node_relations; }` would allow one default implementation of `property_spec`, `properties`, `property`, `relations`, and `targets`. This should stay small; the plugin-specific projection functions are where the domain knowledge belongs.

### Repeated helper functions and error shapes

Fact: D-Bus, Niri, and PipeWire each define local helpers for `node_not_found`, `relation`, `string`, and property insertion. See `plugins/dbus/src/state.rs:557`, `plugins/dbus/src/state.rs:575`, `plugins/dbus/src/state.rs:579`, `plugins/dbus/src/state.rs:586`; `plugins/niri/src/state.rs:656`, `plugins/niri/src/state.rs:689`, `plugins/niri/src/state.rs:753`, `plugins/niri/src/state.rs:760`; and `plugins/pipewire/src/state.rs:535`, `plugins/pipewire/src/state.rs:539`, `plugins/pipewire/src/state.rs:546`, `plugins/pipewire/src/state.rs:550`.

Fact: Each plugin crate has a private `config_error` function with the same conversion shape and only the `kind` string changed. See `plugins/dbus/src/lib.rs:112`, `plugins/niri/src/lib.rs:121`, `plugins/pipewire/src/lib.rs:114`, and `plugins/project/src/lib.rs:94`.

Recommendation: Move the tiny graph projection helpers into a shared helper module if a projection abstraction is introduced. Also consider a `plugin_config_error(plugin_name, error)` helper in `plugins/api` so config modules do not depend on crate-private boilerplate in `lib.rs`.

### Runtime loop consistency

Fact: Long-running plugins all spawn background tasks and hold the join handle in a plugin handle. D-Bus stores a vector of watcher handles for configured services, aborts them in `Drop`, and aborts/awaits them in `shutdown`. See `plugins/dbus/src/lib.rs:28`, `plugins/dbus/src/lib.rs:32`, `plugins/dbus/src/lib.rs:42`, and `plugins/dbus/src/runtime.rs:29`. Niri and PipeWire store a single optional event stream handle and use the same abort/await pattern. See `plugins/niri/src/lib.rs:32`, `plugins/niri/src/lib.rs:36`, `plugins/niri/src/lib.rs:45`, `plugins/pipewire/src/lib.rs:29`, `plugins/pipewire/src/lib.rs:33`, and `plugins/pipewire/src/lib.rs:42`.

Fact: The retry loops use hardcoded retry delays and local logging through `eprintln!`: D-Bus sleeps one second after a watcher error, Niri sleeps one second around event stream reconnects, and PipeWire sleeps two seconds after `pactl subscribe` failures or exits. See `plugins/dbus/src/runtime.rs:47`, `plugins/dbus/src/runtime.rs:54`, `plugins/dbus/src/runtime.rs:173`, `plugins/niri/src/ipc.rs:48`, `plugins/niri/src/ipc.rs:58`, `plugins/niri/src/ipc.rs:107`, `plugins/pipewire/src/runtime.rs:39`, `plugins/pipewire/src/runtime.rs:49`, and `plugins/pipewire/src/runtime.rs:185`.

Recommendation: Add a small runtime task helper for plugin-owned background loops: common abort-on-drop handle storage, retry delay policy, and graph-change emission logging. The domain loops should remain plugin-specific because D-Bus, Niri, and PipeWire have different connection models.

### Runtime startup semantics differ

Fact: D-Bus startup succeeds even if a watched service snapshot fails later; `DbusRuntime::start` builds state from config and spawns per-service watchers, while snapshot errors are logged in `publish_snapshot`. See `plugins/dbus/src/runtime.rs:22`, `plugins/dbus/src/runtime.rs:27`, `plugins/dbus/src/runtime.rs:39`, and `plugins/dbus/src/runtime.rs:407`.

Fact: PipeWire startup also succeeds immediately; `PipeWireRuntime::start` returns state plus a spawned task and does not report initial `pactl` failure to registration. Initial refresh errors are logged in the task. See `plugins/pipewire/src/runtime.rs:17`, `plugins/pipewire/src/runtime.rs:21`, `plugins/pipewire/src/runtime.rs:37`, and `plugins/pipewire/src/runtime.rs:125`.

Fact: Niri startup requires an initial IPC connection and outputs request before registration can complete. `IpcNiriClient::start` connects and requests outputs before spawning the event stream. See `plugins/niri/src/ipc.rs:25`, `plugins/niri/src/ipc.rs:29`, `plugins/niri/src/ipc.rs:30`, and `plugins/niri/src/ipc.rs:32`.

Recommendation: Decide whether runtime-backed plugins should have a consistent "register even when backend is unavailable" policy. If Niri should behave like D-Bus and PipeWire, initialize with empty state and reconnect in the background. If Niri's current fail-fast behavior is intentional, document that as a plugin-specific API contract.

### Graph change emission is duplicated and partly inconsistent

Fact: D-Bus emits leading `NodeKindChanged` changes for both D-Bus kinds on every successful snapshot publish, before the state-derived changes. See `plugins/dbus/src/runtime.rs:426` and `plugins/dbus/src/runtime.rs:431`.

Fact: PipeWire emits only the state-derived changes from `PipeWireState::apply_snapshot`, and Niri emits only the changes returned by `NiriState::apply_event`. See `plugins/pipewire/src/runtime.rs:133`, `plugins/pipewire/src/runtime.rs:144`, `plugins/niri/src/ipc.rs:85`, and `plugins/niri/src/ipc.rs:88`.

Recommendation: Centralize graph-change publication in a helper and define when a provider should emit broad `NodeKindChanged` changes versus narrower node/property/relation changes. This would make watcher behavior easier to reason about and reduce unnecessary downstream invalidation if D-Bus does not need to emit kind changes on every snapshot.

### Projection files are carrying several responsibilities

Fact: `plugins/dbus/src/state.rs` contains runtime config structs, shared state type, snapshot structs, graph query APIs, D-Bus value conversion, graph change diffing, node ID encoding, relation/property helpers, and tests module wiring. See `plugins/dbus/src/state.rs:13`, `plugins/dbus/src/state.rs:20`, `plugins/dbus/src/state.rs:34`, `plugins/dbus/src/state.rs:48`, `plugins/dbus/src/state.rs:97`, `plugins/dbus/src/state.rs:383`, `plugins/dbus/src/state.rs:476`, and `plugins/dbus/src/state.rs:516`.

Fact: `plugins/niri/src/state.rs` similarly combines event application, graph query APIs, property projection, relation projection, change derivation, helper construction, and panic adaptation in one file. See `plugins/niri/src/state.rs:19`, `plugins/niri/src/state.rs:35`, `plugins/niri/src/state.rs:124`, `plugins/niri/src/state.rs:230`, `plugins/niri/src/state.rs:475`, `plugins/niri/src/state.rs:581`, and `plugins/niri/src/state.rs:764`.

Fact: `plugins/pipewire/src/state.rs` combines snapshot model types, pactl DTOs, snapshot conversion, graph query APIs, change diffing, property projection, and audio presentation helpers in one file. See `plugins/pipewire/src/state.rs:23`, `plugins/pipewire/src/state.rs:53`, `plugins/pipewire/src/state.rs:143`, `plugins/pipewire/src/state.rs:195`, `plugins/pipewire/src/state.rs:222`, `plugins/pipewire/src/state.rs:320`, and `plugins/pipewire/src/state.rs:445`.

Recommendation: Split large state files by role before adding more plugin behavior. A practical split is `model.rs` for domain snapshots/DTOs, `projection.rs` for graph properties/relations/node IDs, and `changes.rs` for diff/change derivation. For Niri, `events.rs` or `changes.rs` would isolate the long event match from query projection.

### Project plugin is intentionally different but exposes a partial config surface

Fact: `ProjectPlugin` uses `InMemoryProvider` directly and registers mutation providers, so it avoids the custom provider/state/runtime pattern used by D-Bus, Niri, and PipeWire. See `plugins/project/src/lib.rs:6`, `plugins/project/src/lib.rs:41`, `plugins/project/src/lib.rs:47`, `plugins/project/src/lib.rs:53`, and `plugins/project/src/lib.rs:59`.

Fact: `ProjectConfig` exposes `state_path`, but registration rejects it because persistence is not implemented. See `plugins/project/src/config/mod.rs:6`, `plugins/project/src/config/mod.rs:9`, `plugins/project/src/lib.rs:32`, and `plugins/project/src/lib.rs:33`.

Recommendation: Either remove `state_path` from the public config until persistence exists, or document the field as reserved/unsupported in the plugin manifest/default config path. Keeping an accepted TOML shape that fails at registration is a clean temporary guard, but it is also an API promise that persistence is planned.

### Public API surface is minimal but uneven

Fact: D-Bus, Niri, and PipeWire publicly export their provider types but keep `state` and `runtime` private. See `plugins/dbus/src/lib.rs:4`, `plugins/dbus/src/lib.rs:5`, `plugins/dbus/src/lib.rs:6`, `plugins/dbus/src/lib.rs:8`; `plugins/niri/src/lib.rs:5`, `plugins/niri/src/lib.rs:6`, `plugins/niri/src/lib.rs:8`; and `plugins/pipewire/src/lib.rs:4`, `plugins/pipewire/src/lib.rs:5`, `plugins/pipewire/src/lib.rs:6`, `plugins/pipewire/src/lib.rs:8`.

Fact: Those provider constructors are `pub(crate)`, so external crates can name the provider types but cannot construct useful instances without going through plugin registration. See `plugins/dbus/src/provider.rs:16`, `plugins/niri/src/provider.rs:17`, and `plugins/pipewire/src/provider.rs:16`.

Recommendation: If external construction is not intended, stop re-exporting the provider types and make registration/plugin structs the public surface. If tests or downstream users need provider construction, expose an explicit testing or builder API instead of a public type with private construction.

## Reusable helper opportunities

- `ReadOnlyStateProvider<S>`: owns `NodeKind` plus `Arc<RwLock<S>>`, delegates graph provider traits to a projection trait.
- `ProjectedState` trait: supplies `contains_node`, `nodes_for_kind`, `node_properties`, and `node_relations`; default methods derive property specs, properties, property lookup, relation names, and targets.
- `register_read_only_provider_kinds`: loops over kind names, constructs traced providers, and registers node/property/relation providers.
- `register_read_write_provider_kind`: registers all node/property/relation mutation surfaces for providers like `InMemoryProvider`.
- `PluginTasks` or `TaskSetHandle`: stores one or many `JoinHandle<()>`, aborts in `Drop`, aborts and awaits in `PluginHandle::shutdown`.
- `emit_graph_changes`: centralizes `graph.emit_global_change` error logging and plugin labels.
- `config_error_for`: maps `toml::de::Error` into the common `GraphError::InvalidValue` shape.

## Priority

1. Add shared read-only provider/projection helpers if more runtime-backed plugins are expected. This removes the clearest duplication across D-Bus, Niri, and PipeWire.
2. Add registration and task-handle helpers. These are low-risk API cleanup and make plugin entry points easier to scan.
3. Decide the runtime startup availability contract, especially whether Niri should fail registration when its compositor socket is unavailable while D-Bus and PipeWire register and retry in the background.
4. Split the large `state.rs` files once a plugin needs new behavior; doing it opportunistically will reduce review risk for future domain changes.
