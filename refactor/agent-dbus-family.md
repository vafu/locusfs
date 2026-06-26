# D-Bus-Style Plugin Family Review

## Current Role

This review unit covers the D-Bus-style plugin family:

- `plugins/dbus/`: generic configured D-Bus service/object/property/method provider.
- `plugins/dbusmenu/`: DBusMenu endpoint and item provider, currently discovered through StatusNotifier items plus static config.
- `plugins/mpris/`: MPRIS media player discovery and property provider.
- `plugins/statusnotifier/`: StatusNotifier/AppIndicator watcher and item provider.

This is a report-only pass. No source files were edited. The requested output is this Markdown report for coordinator arbitration and later implementation.

## Public API And Entrypoints

- All four crates are both `rlib` and `cdylib` plugin crates (`plugins/dbus/Cargo.toml:6`, `plugins/dbusmenu/Cargo.toml:6`, `plugins/mpris/Cargo.toml:6`, `plugins/statusnotifier/Cargo.toml:6`).
- Dynamic loading entrypoint is `_locusfs_plugin_init` in each crate (`plugins/dbus/src/lib.rs:119`, `plugins/dbusmenu/src/lib.rs:105`, `plugins/mpris/src/lib.rs:98`, `plugins/statusnotifier/src/lib.rs:98`). The host expects this symbol and validates the returned manifest id (`bin/src/plugin/mod.rs:68`).
- Public plugin manifests are:
  - `dbus` (`plugins/dbus/src/lib.rs:99`)
  - `dbusmenu` (`plugins/dbusmenu/src/lib.rs:85`)
  - `mpris` (`plugins/mpris/src/lib.rs:81`)
  - `statusnotifier` (`plugins/statusnotifier/src/lib.rs:81`)
- Rust registration helpers exist for workspace-local use:
  - `dbus::register` and `dbus::register_with_config` (`plugins/dbus/src/lib.rs:54`, `plugins/dbus/src/lib.rs:58`)
  - `dbusmenu::register` and `dbusmenu::register_with_config` (`plugins/dbusmenu/src/lib.rs:45`, `plugins/dbusmenu/src/lib.rs:49`)
  - `mpris::register` (`plugins/mpris/src/lib.rs:51`)
  - `statusnotifier::register` (`plugins/statusnotifier/src/lib.rs:51`)
- Public provider types are re-exported from each crate root (`plugins/dbus/src/lib.rs:8`, `plugins/dbusmenu/src/lib.rs:8`, `plugins/mpris/src/lib.rs:7`, `plugins/statusnotifier/src/lib.rs:7`). Their constructors are `pub(crate)`, so the re-exports are mainly type exposure rather than useful construction API.
- `dbus` and `dbusmenu` expose public config modules (`plugins/dbus/src/lib.rs:3`, `plugins/dbusmenu/src/lib.rs:3`). `mpris` and `statusnotifier` accept but ignore plugin config (`plugins/mpris/src/lib.rs:89`, `plugins/statusnotifier/src/lib.rs:89`).

## Step-By-Step Walkthrough

1. `plugins/*/Cargo.toml`: dependency shape is almost identical. All four depend on `locusfs-graph` with `dynamic`, `provider-tracing`, and `watch-provider`; all four depend on `zbus = 5.16` with `default-features = false, features = ["tokio"]`.
2. `plugins/*/src/lib.rs`: each crate defines node kind constants, a plugin struct, a plugin handle, registration logic, `LocusFsPlugin`, and `_locusfs_plugin_init`.
3. `plugins/*/src/provider.rs`: each provider is a thin `NodeProvider`, `PropertyProvider`, `RelationProvider`, and usually `PathProvider` adapter over an `Arc<RwLock<State>>`.
4. `plugins/*/src/runtime.rs`: owns async D-Bus connections, signal streams, snapshots, retries, and graph change publication.
5. `plugins/*/src/state.rs`: owns pure snapshot state, node id generation, property maps, relation maps, path-provider layout, and graph diff generation.
6. `plugins/dbus/src/state/test.rs`, inline config/runtime/state tests elsewhere: cover current layouts and state changes, but unevenly.

## Behavior Summary

### `dbus`

- Config declares services by bus name, bus kind, optional local id, and optional ObjectManager path (`plugins/dbus/src/config/mod.rs:4`).
- Registration creates a plugin-owned runtime, starts one watcher per configured service, and registers providers for `dbus`, `dbus-object`, and `dbus-method` (`plugins/dbus/src/lib.rs:69`).
- Runtime connects to each service bus, watches `NameOwnerChanged`, subscribes to `org.freedesktop.DBus.Properties` and `org.freedesktop.DBus.ObjectManager` signals, and republishes a full service snapshot on relevant events (`plugins/dbus/src/runtime.rs:59`, `plugins/dbus/src/runtime.rs:98`, `plugins/dbus/src/runtime.rs:129`).
- Snapshot discovery tries `ObjectManager.GetManagedObjects` at the configured path, then `/`, then recursive introspection (`plugins/dbus/src/runtime.rs:235`).
- Introspection XML is parsed with hand-written string scanning for interface names, child node names, property access, and methods (`plugins/dbus/src/runtime.rs:469`, `plugins/dbus/src/runtime.rs:480`, `plugins/dbus/src/runtime.rs:510`).
- State exposes service nodes, hidden object nodes, hidden method nodes, D-Bus property values, writable D-Bus properties, and callable methods as a write-only `call` property (`plugins/dbus/src/state.rs:138`, `plugins/dbus/src/state.rs:183`, `plugins/dbus/src/state.rs:333`).
- Current custom path layout is `/dbus/<service>/object/...` with addressable `@properties` and `@methods` virtual directories (`plugins/dbus/src/state.rs:19`, `plugins/dbus/src/state.rs:468`).

### `dbusmenu`

- Config declares explicit DBusMenu endpoints (`plugins/dbusmenu/src/config/mod.rs:4`).
- Registration seeds state from config, registers three provider kinds, registers mutation only for `dbusmenu-item`, and starts a StatusNotifier-driven menu watcher (`plugins/dbusmenu/src/lib.rs:53`).
- Runtime reads StatusNotifier watcher registrations and snapshots menus by calling DBusMenu `GetLayout` (`plugins/dbusmenu/src/runtime.rs:63`, `plugins/dbusmenu/src/runtime.rs:231`).
- State exposes facade nodes `dbusmenu:menu` and `dbusmenu:item`, hidden menu nodes, hidden item nodes, item activation as writable `activate`, and virtual child directories (`plugins/dbusmenu/src/state.rs:14`, `plugins/dbusmenu/src/state.rs:82`, `plugins/dbusmenu/src/state.rs:163`).

### `mpris`

- Registration starts a session-bus watcher and registers `mpris` plus hidden `mpris-player` providers (`plugins/mpris/src/lib.rs:55`).
- Runtime lists names with the `org.mpris.MediaPlayer2.` prefix, spawns one watcher per player, snapshots root/player properties through `org.freedesktop.DBus.Properties.GetAll`, and refreshes on `PropertiesChanged` (`plugins/mpris/src/runtime.rs:85`, `plugins/mpris/src/runtime.rs:140`, `plugins/mpris/src/runtime.rs:183`).
- State exposes facade node `mpris:player`, hidden player nodes, player properties, and one relation per player id from the facade (`plugins/mpris/src/state.rs:14`, `plugins/mpris/src/state.rs:69`, `plugins/mpris/src/state.rs:209`).

### `statusnotifier`

- Registration starts a session-bus watcher and registers `statusnotifier` plus hidden `statusnotifier-item` providers (`plugins/statusnotifier/src/lib.rs:55`).
- Runtime implements `org.kde.StatusNotifierWatcher` using `#[zbus::interface]`, owns the watcher name when available, accepts item registrations, also scans passive item bus names, snapshots item properties, and refreshes on `PropertiesChanged` (`plugins/statusnotifier/src/runtime.rs:63`, `plugins/statusnotifier/src/runtime.rs:141`, `plugins/statusnotifier/src/runtime.rs:333`).
- State exposes facade node `statusnotifier:item`, hidden item nodes, item properties, registered item strings, and one relation per item id from the facade (`plugins/statusnotifier/src/state.rs:14`, `plugins/statusnotifier/src/state.rs:66`, `plugins/statusnotifier/src/state.rs:214`).

## API Findings

- Provider type re-exports are probably unnecessary public API. `DbusProvider`, `DbusMenuProvider`, `MprisProvider`, and `StatusNotifierProvider` are public names but cannot be constructed outside their crates (`plugins/dbus/src/provider.rs:22`, `plugins/dbusmenu/src/provider.rs:20`, `plugins/mpris/src/provider.rs:17`, `plugins/statusnotifier/src/provider.rs:17`). Either document them as intentionally public for tests/embedding, or stop re-exporting them.
- Public config types are under-documented. `DbusConfig`, `ServiceConfig`, `DbusMenuConfig`, and `MenuConfig` are public field structs with behavioral defaults that are only visible from implementation (`plugins/dbus/src/config/mod.rs:4`, `plugins/dbusmenu/src/config/mod.rs:4`). Rust-guide expects public API docs at discovery points.
- `dbus` validates service name emptiness but does not validate `local_id` uniqueness, `local_id` graph suitability, supplied bus names with `zbus::names::BusName`, or supplied object manager paths with `zbus::zvariant::ObjectPath` during config parsing (`plugins/dbus/src/config/mod.rs:39`). Errors can surface later during runtime or graph access.
- `dbusmenu` config is misleading today. Static config endpoints are inserted into state at registration (`plugins/dbusmenu/src/lib.rs:55`), but the runtime never snapshots configured endpoints, and discovered endpoints are hard-coded as session bus (`plugins/dbusmenu/src/runtime.rs:245`). A configured system-bus menu can exist as an empty graph node without active runtime behavior.
- Method invocation in `dbus` is a stringly write to `dbus-method:<...>/call` (`plugins/dbus/src/state.rs:1048`, `plugins/dbus/src/provider.rs:127`). This is serviceable for a filesystem, but the API currently supports only a small subset of D-Bus input signatures (`s`, `b`, `u`, `i`, `d`, `o`) and comma-splitting has no escaping (`plugins/dbus/src/provider.rs:270`).
- `mpris` and `statusnotifier` expose read-only state only. That is coherent for status inspection, but MPRIS naturally has control methods. If control is in scope later, it should follow the same callable-method filesystem pattern chosen for generic D-Bus instead of inventing another write property shape.

## Redundancy Findings

- All four plugin crates duplicate the same registration skeleton: create `PluginRuntime`, start runtime/state, loop provider kinds, wrap with `TracedProvider`, register node/property/path/relation providers, and store a task handle (`plugins/mpris/src/lib.rs:55`, `plugins/statusnotifier/src/lib.rs:55`, `plugins/dbusmenu/src/lib.rs:53`, `plugins/dbus/src/lib.rs:69`).
- All four provider files repeat the same forwarding adapter from graph provider traits into state methods (`plugins/mpris/src/provider.rs:22`, `plugins/statusnotifier/src/provider.rs:22`, `plugins/dbusmenu/src/provider.rs:29`, `plugins/dbus/src/provider.rs:31`).
- `mpris` and `statusnotifier` state modules are near twins: snapshot map, facade node, hidden item nodes, path listing, relation-per-id, `snapshot_changes`, `changed_property_keys`, `node_id`, `relation`, `insert`, `string`, and `node_not_found` (`plugins/mpris/src/state.rs:41`, `plugins/statusnotifier/src/state.rs:38`).
- Runtime helper duplication appears across the family:
  - `publish_changes` is repeated in `dbusmenu`, `mpris`, and `statusnotifier` (`plugins/dbusmenu/src/runtime.rs:192`, `plugins/mpris/src/runtime.rs:299`, `plugins/statusnotifier/src/runtime.rs:501`).
  - `owned_string` and `owned_bool` repeat in `dbusmenu`, `mpris`, and `statusnotifier` (`plugins/dbusmenu/src/runtime.rs:402`, `plugins/mpris/src/runtime.rs:307`, `plugins/statusnotifier/src/runtime.rs:520`).
  - `connection_for_bus`/bus-kind conversion repeats in `dbus` and `dbusmenu` (`plugins/dbus/src/provider.rs:249`, `plugins/dbusmenu/src/runtime.rs:391`).
- `zbus` dependency declarations are repeated in every D-Bus-family `Cargo.toml`. This should move to `[workspace.dependencies]` with the current common feature set.
- `dbus` alone contains enough path/state/model/runtime responsibilities in one state file to hide duplicated concepts. The file mixes model DTOs, graph API, path layout, DBus value conversion, diffing, id encoding, and tests (`plugins/dbus/src/state.rs:31`, `plugins/dbus/src/state.rs:238`, `plugins/dbus/src/state.rs:1052`, `plugins/dbus/src/state.rs:1355`).

## Correctness Findings

- `dbusmenu` item child relations are effectively broken. `node_relations` asks each item for `child_targets`, but `DbusMenuItem::child_targets` always returns an empty vector (`plugins/dbusmenu/src/state.rs:430`, `plugins/dbusmenu/src/state.rs:549`). Virtual path children may show descendants, while graph relations do not.
- `dbusmenu` does not subscribe to DBusMenu `LayoutUpdated`, so menu labels, enabled state, and children can become stale after initial snapshot (`plugins/dbusmenu/src/runtime.rs:81`, `plugins/dbusmenu/src/runtime.rs:231`).
- `statusnotifier` becomes inactive if another process already owns `org.kde.StatusNotifierWatcher`; it returns `Ok(())` and retries later, but does not switch into client mode against the existing watcher (`plugins/statusnotifier/src/runtime.rs:141`, `plugins/statusnotifier/src/runtime.rs:227`). If this filesystem is expected to coexist with a desktop shell panel, this is a domain decision rather than a pure bug.
- `dbus` method identity can become ambiguous. The display name is the short method name when unique per object, otherwise `interface.method` (`plugins/dbus/src/state.rs:1308`). This is reasonable, but callers can lose stable names if a service adds another same-named method on a different interface.
- `dbus` path layout hides `@properties` and `@methods` from listings but keeps them addressable by lookup (`plugins/dbus/src/state.rs:577`). That is surprising filesystem behavior and directly matches the domain-specific concern.

## Performance And Concurrency Findings

- Locking discipline is mostly good: provider methods hold a read lock while executing synchronous state lookups and do not await inside those closures (`plugins/dbus/src/provider.rs:31`). D-Bus property writes release the read lock before awaiting D-Bus I/O, then reacquire a write lock to update cached state (`plugins/dbus/src/provider.rs:147`).
- `dbus` fully re-snapshots a service on every matching Properties or ObjectManager signal (`plugins/dbus/src/runtime.rs:129`). For large services this means repeated `GetManagedObjects`, introspection, property reads, tree rebuilds, and graph diffing.
- `dbus` path lookup recomputes object path views from the service object map on each lookup/listing (`plugins/dbus/src/state.rs:661`, `plugins/dbus/src/state.rs:698`). The proposed `/objects` and `/methods` layout should precompute a per-service path tree when a snapshot is applied.
- `dbus` introspects objects sequentially after `GetManagedObjects` to annotate writability and methods (`plugins/dbus/src/runtime.rs:275`). This is simpler and safe, but slow for object-heavy services. A bounded concurrent stream would improve refresh latency without flooding the bus.
- Method and property mutation in `dbus` opens a fresh D-Bus connection for each call/write (`plugins/dbus/src/provider.rs:172`, `plugins/dbus/src/provider.rs:178`). If writes become common, cache one connection per bus in the runtime or a small connection provider.
- Each plugin creates a two-worker Tokio runtime through `PluginRuntime::new` (`plugins/api/src/lib.rs:106`). Loading all four D-Bus-family plugins costs eight worker threads before considering spawned watcher tasks. The ABI note explains why plugin-owned runtimes exist (`plugins/api/src/lib.rs:95`), but these mostly I/O-bound plugins may not need two workers each.
- Nested watcher tasks are not always explicitly owned at shutdown. `mpris` and `statusnotifier` abort the supervisor task, but child player/item watcher `JoinHandle`s live inside that task and rely on runtime shutdown after the handle drops (`plugins/mpris/src/runtime.rs:85`, `plugins/statusnotifier/src/runtime.rs:182`). A supervisor with explicit child aborts would be easier to reason about.
- `dbusmenu` has explicit `shutdown`, but unlike `dbus`, `mpris`, and `statusnotifier`, it has no `Drop` abort fallback (`plugins/dbusmenu/src/lib.rs:37`). Dropping a `JoinHandle` detaches unless the runtime shutdown happens immediately after.

## Tidiness And Documentation Findings

- The plugin crates have good high-level crate comments, but public structs, constants, and registration helpers are mostly undocumented. This conflicts with rust-guide public API guidance.
- `plugins/dbus/src/state.rs` is too large for its responsibility set. Good split candidates:
  - `model.rs`: `ServiceConfig`, `ServiceSnapshot`, `ObjectSnapshot`, property/method snapshot structs.
  - `ids.rs`: service/object/method node ids, display names, collision rules.
  - `properties.rs`: property map construction and `locus_value_from_dbus`.
  - `path.rs`: provider-owned filesystem layout.
  - `changes.rs`: graph diff generation.
  - `test.rs`: keep current tests as sibling module.
- `plugins/dbus/src/runtime.rs` also mixes watcher supervision, D-Bus snapshot acquisition, XML parsing, and tests. The XML parsing block should move behind a parser module with focused tests (`plugins/dbus/src/runtime.rs:469`).
- Error reporting uses `eprintln!` throughout runtime tasks (`plugins/dbus/src/runtime.rs:49`, `plugins/mpris/src/runtime.rs:43`, `plugins/statusnotifier/src/runtime.rs:135`). The workspace already uses `tracing` in the host/plugin manager; plugin runtimes should emit structured `tracing` events where possible.
- Test placement is inconsistent. `dbus` uses a separate `state/test.rs`, while `dbusmenu` and `mpris` inline tests in production files, and `statusnotifier` has no tests (`plugins/statusnotifier/src/state.rs` has no `#[cfg(test)]` module).

## Best-Practice And Crate-Reuse Notes

- Local dependency evidence: all four crates use `zbus 5.16.0`; no XML parser crate such as `quick-xml`, `roxmltree`, or `xmltree` is present in `Cargo.lock`.
- The code already uses important zbus/D-Bus conventions:
  - `zbus::fdo::DBusProxy`, `PropertiesProxy`, `ObjectManagerProxy`, and `IntrospectableProxy` for standard interfaces (`plugins/dbus/src/runtime.rs:8`).
  - `zbus::names::BusName` and `InterfaceName` for some validation boundaries (`plugins/dbus/src/runtime.rs:218`, `plugins/mpris/src/runtime.rs:229`).
  - `zbus::zvariant::ObjectPath`/`OwnedObjectPath` for object-path validation and conversion (`plugins/dbus/src/provider.rs:339`, `plugins/statusnotifier/src/runtime.rs:408`).
  - `#[zbus::interface]` for implementing StatusNotifierWatcher (`plugins/statusnotifier/src/runtime.rs:63`).
- More D-Bus strings should be validated at config boundaries with zbus typed names and object paths instead of later in runtime code.
- The hand-written XML scanner in `dbus` is a maintenance risk. Prefer a small XML parser dependency (`roxmltree` for read-only tree queries or `quick-xml` for streaming) unless local `zbus` exposes a typed introspection parser suitable for this use. Verify zbus first before adding a dependency.
- `PropertiesChanged` and ObjectManager signals contain enough data for incremental updates in many cases. Full re-snapshot is acceptable for a first version but should be documented as a simplicity tradeoff, then replaced where it hurts.
- `zbus::proxy` or generated typed proxy wrappers may reduce stringly calls for MPRIS, StatusNotifier, and DBusMenu. Use only if the generated surface stays private and does not leak zbus types into the LocusFS plugin public API.

## Domain-Specific Filesystem Layout Plan

The current D-Bus layout should be replaced. The code currently exposes a singular `object` directory and hidden-but-addressable `@properties` and `@methods` directories (`plugins/dbus/src/state.rs:19`). That makes object traversal, properties, and callable methods share one custom router with special names.

Proposed canonical layout:

```text
/dbus/<service>/
  objects/
    <relative-object-path>/
      <property>
      <interface>.<property>
      <child-object>/
  methods/
    <relative-object-path>/
      <method>
      <interface>.<method>
      <child-object>/
```

Rules:

- `/dbus/<service>/objects` mirrors the configured ObjectManager root. If the ObjectManager root object has properties, those property files live directly under `objects/`.
- `/dbus/<service>/methods` mirrors the same object path tree, but every non-directory entry is a callable method file.
- Property files under `objects` map only to real D-Bus properties. Service/object metadata such as `kind`, `source`, `path`, and `service-name` should remain graph properties, not entries in the D-Bus object property tree.
- Method files under `methods` map to the existing hidden method node's write-only `call` property, preserving the graph mutation mechanism while making the filesystem path direct.
- Always expose canonical `interface.member` filenames. Expose short `member` aliases only when the member name is unique for that object and does not collide with a child object directory.
- If a property or method still collides with an object child after canonical naming, use a documented percent-encoding helper for the file name. Do not introduce another magic `@...` namespace.
- Keep absolute/outside-object-manager objects in a documented reserved subtree such as `objects/_absolute/...` and `methods/_absolute/...`, replacing current `@absolute` (`plugins/dbus/src/state.rs:1250`).
- Watch targets should be stable:
  - `/objects` and object child directories watch the service object's object relation or a precomputed object-tree target.
  - object property directories watch the object node.
  - `/methods` and method object directories watch the object's `methods` relation.
  - method files watch the method node.
- Compatibility option: keep legacy `object`, `@properties`, and `@methods` lookup aliases for one migration window but stop listing them. If this project has no compatibility promise yet, skip aliases and simplify immediately.

Implementation shape:

- Introduce a `DbusPathLayout` or `ServicePathIndex` built when `set_service_snapshot` applies a new object map. It should contain object-tree children, method-tree children, and file-to-graph-entry mappings.
- Remove ad hoc `virtual_parts` routing for object/property/method state once the indexed layout owns lookup/list/watch behavior (`plugins/dbus/src/state.rs:468`, `plugins/dbus/src/state.rs:556`, `plugins/dbus/src/state.rs:637`).
- Preserve graph relations (`object`, `methods`, `dbus`) because they are useful for watchers and non-filesystem consumers (`plugins/dbus/src/state.rs:15`).

## Concrete Refactor Plan

1. Standardize module shape without changing behavior:
   - Move D-Bus state internals into `state/mod.rs` plus sibling files.
   - Move D-Bus runtime XML parsing into a parser module.
   - Move inline tests in `dbusmenu` and `mpris` to sibling `test.rs` modules when those files are touched.

2. Add low-risk shared helpers:
   - Move `zbus` to workspace dependencies.
   - Add a workspace-private `plugins/dbus-common` or `plugins/support` crate only if coordinator accepts another crate. It should own `BusKind`, `connection_for_bus`, owned zvariant extractors, `get_all_properties`, retry sleep policy, and `publish_changes`.
   - Avoid a broad generic provider abstraction initially. The provider wrappers are repetitive, but a generic trait-heavy wrapper could obscure graph behavior. Start with registration helper functions and runtime/zvariant helpers.

3. Replace the D-Bus filesystem layout:
   - Write tests for the new `/objects` and `/methods` paths first.
   - Build a path index from `ServiceSnapshot`.
   - Map property files to `GraphPathEntry::Property { node: object_node, key }`.
   - Map method files to `GraphPathEntry::Property { node: method_node, key: "call" }`.
   - Remove `PATH_PROPERTIES`, `PATH_METHODS`, `VIRTUAL_PROPERTIES`, `VIRTUAL_METHODS`, and `VIRTUAL_METHOD` after tests pass.

4. Fix DBusMenu correctness:
   - Make item child relations return actual child item node ids.
   - Decide whether static `menus` config is supported. If yes, runtime must snapshot configured endpoints and honor configured bus kind. If no, remove or clearly document the config until implemented.
   - Subscribe to DBusMenu `LayoutUpdated` for tracked menus and refresh affected endpoints.

5. Improve StatusNotifier operating modes:
   - Keep current watcher-owner mode.
   - Add explicit inactive state/reason if another watcher owns the name, or add client mode that reads the existing watcher's `RegisteredStatusNotifierItems`.
   - Add tests around `registered_item_target`, `item_id`, state changes, and registered item lists.

6. Reduce MPRIS/StatusNotifier duplication:
   - Extract a keyed snapshot diff helper after StatusNotifier tests exist.
   - Keep domain-specific property map functions local.

7. Improve runtime observability and shutdown:
   - Replace `eprintln!` with `tracing`.
   - Make supervisor tasks explicitly abort child watchers during shutdown.
   - Consider one-worker plugin runtimes for these I/O-bound plugins if the plugin API allows configuration.

## Test Plan

Existing verification run:

```text
cargo test -p locusfs-plugin-dbus -p locusfs-plugin-dbusmenu -p locusfs-plugin-mpris -p locusfs-plugin-statusnotifier
```

Result: passed. D-Bus 14 tests, DBusMenu 4 tests, MPRIS 3 tests, StatusNotifier 0 tests.

Required new/updated tests:

- `dbus` layout tests:
  - `/dbus/<service>/objects` exists instead of `object`.
  - object root properties are direct files.
  - child object directories and property files coexist with collision policy.
  - canonical `interface.property` files are always exposed.
  - short property aliases are exposed only when unambiguous.
  - `/dbus/<service>/methods` exposes write-only callable method files.
  - methods with duplicate names use canonical interface-qualified names.
  - object-manager root `/` and outside-object-manager paths work without `@properties` or `@methods`.
  - watch targets for objects, object dirs, methods, and method dirs map to the intended graph targets.
- `dbus` parser/runtime tests:
  - malformed introspection XML behavior after replacing string scanning.
  - property access parsing for `read`, `write`, and `readwrite`.
  - input arg signature parsing for multiple args and unsupported signatures.
  - config validation for invalid bus names, invalid object paths, empty local ids, and duplicate local ids.
- `dbusmenu` tests:
  - item child relation returns child node ids.
  - configured menu endpoints are actually snapshotted or rejected by config policy.
  - `LayoutUpdated` refreshes endpoint state.
  - activation rejects disabled or invisible items consistently.
- `mpris` tests:
  - player id normalization for unusual bus names.
  - removal abort/diff behavior for multiple players.
  - optional future method/control files if implemented.
- `statusnotifier` tests:
  - state exposes item properties, relations, and path children.
  - item removal emits node and relation changes.
  - `registered_items` output.
  - `registered_item_target` for service names, object paths with sender, and missing sender.
  - behavior when watcher name is unavailable.
- Cross-plugin verification:
  - `cargo test -p locusfs-plugin-dbus`
  - `cargo test -p locusfs-plugin-dbusmenu`
  - `cargo test -p locusfs-plugin-mpris`
  - `cargo test -p locusfs-plugin-statusnotifier`
  - `cargo test` after shared helper extraction or workspace dependency edits.
  - `cargo clippy --all-targets --all-features` before finalizing the full refactor pass.

## Open Questions For Coordinator Arbitration

- Is the legacy D-Bus layout considered user-facing enough to need compatibility aliases for `object`, `@properties`, and `@methods`, or can the refactor break it immediately?
- Should `/objects` expose direct property files at object directories, or should canonical interface directories be mandatory to eliminate name collisions?
- Should method invocation stay as comma-separated string writes, or should LocusFS grow a structured call argument format before methods become a prominent filesystem API?
- Should DBusMenu static config be fully supported, or removed/deferred until the runtime can snapshot and watch configured endpoints?
- Should StatusNotifier operate as a watcher only, or also as a client of an existing watcher when another process owns `org.kde.StatusNotifierWatcher`?
- Is a new workspace-private D-Bus support crate acceptable, or should shared helpers live in each plugin until duplication proves more costly?
- Is adding an XML parser dependency acceptable for D-Bus introspection, and if so should the project prefer read-only tree parsing (`roxmltree`) or streaming parsing (`quick-xml`)?
- Should plugin runtimes remain fixed at two worker threads, or should plugin registration allow one-worker runtimes for mostly I/O-bound plugins?
