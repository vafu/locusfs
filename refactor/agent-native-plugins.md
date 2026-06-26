# Native Plugin Review: Niri, PipeWire, Project

## Current Role

Scoped review agent for non-D-Bus concrete plugins and support tooling. This report covers `plugins/niri/`, `plugins/pipewire/`, `plugins/project/`, and `plugins/project/scripts/proj`. D-Bus-family plugins were sampled only to compare visible plugin patterns and filesystem layout conventions. No source files or other reports were edited.

## Public API And Entrypoints

Shared host/plugin entrypoints:

- The host loads enabled plugins from config in `bin/src/plugin/mod.rs:32`, resolves a dynamic library in `bin/src/plugin/mod.rs:74`, calls `_locusfs_plugin_init` in `bin/src/plugin/mod.rs:76`, validates the manifest id in `bin/src/plugin/mod.rs:87`, merges `default_config` with user config in `bin/src/plugin/mod.rs:95`, and calls `register` with `PluginContext::try_new` in `bin/src/plugin/mod.rs:96`.
- `PluginContext` exposes `DynamicGraph` and a Tokio `Handle` as public fields in `plugins/api/src/lib.rs:31`. `LocusFsPlugin` defines `manifest`, `default_config`, and async `register` in `plugins/api/src/lib.rs:68`.
- `PluginRuntime` creates a two-thread plugin-owned runtime in `plugins/api/src/lib.rs:105` and shuts it down in `Drop` with `shutdown_background` in `plugins/api/src/lib.rs:129`.

Niri public surface:

- `plugins/niri/src/lib.rs:3` exposes `config`; `plugins/niri/src/lib.rs:8` re-exports `NiriProvider`.
- Public graph kinds are generic: `window`, `workspace`, `output`, and `context` in `plugins/niri/src/lib.rs:20`.
- Public registration helpers are `register` and `register_with_config` in `plugins/niri/src/lib.rs:55`.
- Dynamic plugin entrypoint is `_locusfs_plugin_init` in `plugins/niri/src/lib.rs:119`.
- Config is currently an empty public struct in `plugins/niri/src/config/mod.rs:4`.

PipeWire public surface:

- `plugins/pipewire/src/lib.rs:3` exposes `config`; `plugins/pipewire/src/lib.rs:8` re-exports `PipeWireProvider`.
- Public graph kinds are `pipewire`, `pipewire-sink`, and `pipewire-source` in `plugins/pipewire/src/lib.rs:20`.
- Public registration helpers are `register` and `register_with_config` in `plugins/pipewire/src/lib.rs:54`.
- Dynamic plugin entrypoint is `_locusfs_plugin_init` in `plugins/pipewire/src/lib.rs:116`.
- Config exposes a public `pactl: String` command field in `plugins/pipewire/src/config/mod.rs:5`.

Project public surface:

- `plugins/project/src/lib.rs:3` exposes `config`.
- Public graph kind is `project` in `plugins/project/src/lib.rs:11`.
- Public registration helpers are `register` and `register_with_config` in `plugins/project/src/lib.rs:24`.
- Dynamic plugin entrypoint is `_locusfs_plugin_init` in `plugins/project/src/lib.rs:90`.
- Config exposes `state_path: Option<PathBuf>` in `plugins/project/src/config/mod.rs:6`, but registration rejects it in `plugins/project/src/lib.rs:32`.

Project script entrypoints:

- `proj` supports `init`, `root`, `metadata`, `update`, `set-current`, `publish`, and `clear` in `plugins/project/scripts/proj:9`.
- It resolves the mountpoint from `LOCUS_ROOT`, `LOCUSFS_MOUNT`, `LOCUS_MOUNT`, or `/tmp/locusfs` in `plugins/project/scripts/proj:7`.
- It writes graph state through the mounted filesystem under `/project/<encoded-root>` in `plugins/project/scripts/proj:95`.

## Step-By-Step Walkthrough

### Niri

1. Plugin registration parses TOML into `NiriConfig` in `plugins/niri/src/lib.rs:110`, but `register_with_config` names the value `_config` and does not use it in `plugins/niri/src/lib.rs:59`.
2. Registration creates a plugin-owned runtime in `plugins/niri/src/lib.rs:70`.
3. It spawns `IpcNiriClient::start` onto that runtime and immediately awaits the join handle in `plugins/niri/src/lib.rs:73`. This makes initial Niri IPC availability part of registration success.
4. `IpcNiriClient::start` connects to the Niri socket, requests outputs, creates shared `RwLock<NiriState>`, and starts the event stream in `plugins/niri/src/ipc.rs:24`.
5. The socket path is read from Niri's environment variable in `plugins/niri/src/ipc.rs:127`.
6. The event task loops over `read_event_stream`, sleeps, reconnects, and retries forever in `plugins/niri/src/ipc.rs:44`.
7. Each event takes a write lock, applies `NiriState::apply_event`, and emits each returned `GraphChange` in `plugins/niri/src/ipc.rs:80`.
8. Registration creates one `NiriProvider` per Niri kind and registers node, property, and relation provider capabilities in `plugins/niri/src/lib.rs:78`.
9. Provider calls read the shared state through `with_state` in `plugins/niri/src/provider.rs:21`.
10. `NiriState` projects the event-stream snapshot into nodes, properties, relations, and change events in one file, starting at `plugins/niri/src/state.rs:19`.

### PipeWire

1. Plugin registration parses TOML into `PipeWireConfig` in `plugins/pipewire/src/lib.rs:107`.
2. Registration creates a plugin-owned runtime in `plugins/pipewire/src/lib.rs:69`.
3. `PipeWireRuntime::start` creates empty shared state and spawns `run_pipewire_watcher` without waiting for `pactl` availability in `plugins/pipewire/src/runtime.rs:13`.
4. The watcher does an initial `refresh_and_publish` in `plugins/pipewire/src/runtime.rs:33`.
5. It then starts `pactl subscribe`, reads stdout lines, and refreshes on relevant sink/source/server events in `plugins/pipewire/src/runtime.rs:35`.
6. A refresh runs three `pactl -f json` commands: `info`, `list sinks`, and `list sources` in `plugins/pipewire/src/runtime.rs:115`.
7. Parsed DTOs are converted to `PipeWireSnapshot` in `plugins/pipewire/src/state.rs:326`.
8. `PipeWireState::apply_snapshot` diffs old and new snapshots, stores the new snapshot, and returns graph changes in `plugins/pipewire/src/state.rs:61`.
9. The runtime drops the state write lock before emitting graph changes in `plugins/pipewire/src/runtime.rs:97`.
10. Registration creates one provider per kind, registers node/property/relation capabilities for each, and registers a path provider for the `pipewire` facade kind in `plugins/pipewire/src/lib.rs:72`.

### Project

1. Plugin registration parses TOML into `ProjectConfig` in `plugins/project/src/lib.rs:81`.
2. If `state_path` is set, registration fails because persistence is not implemented in `plugins/project/src/lib.rs:32`.
3. Registration creates an `InMemoryProvider` for the `project` kind in `plugins/project/src/lib.rs:40`.
4. It registers node, property, relation, and all mutation provider capabilities for that kind in `plugins/project/src/lib.rs:43`.
5. The returned handle holds a clone of the provider in `plugins/project/src/lib.rs:16`.
6. The script discovers `.project.json` by walking parents in `plugins/project/scripts/proj:44`.
7. `init` creates `.project.json` with `name` and `icon` in `plugins/project/scripts/proj:157`.
8. `update` ensures `/project/<encoded-root>` exists, then writes properties by truncating/creating files under the FUSE mount in `plugins/project/scripts/proj:236`.
9. `set-current` runs `update`, reads `/context/selected/workspace`, and symlinks `/workspace/<id>/project` to the project node in `plugins/project/scripts/proj:293`.
10. `clear` removes that workspace relation symlink in `plugins/project/scripts/proj:304`.

## Behavior Summary

Niri is a stateful read-only provider. It exposes compositor windows, workspaces, outputs, and a selected context directly as top-level graph kinds. It requires an initial Niri IPC connection at registration time, then reconnects the event stream after later failures.

PipeWire is a stateful read-only provider backed by `pactl`. It exposes a `pipewire` facade kind plus hidden sink/source endpoint kinds. Registration succeeds even if `pactl` or PipeWire is unavailable; failures are logged by the background task and the graph remains empty/stale until a snapshot succeeds.

Project is a writable in-memory provider over a single `project` kind. The Bash script is the real domain adapter: it extracts metadata/git/workspace context and writes graph nodes/properties/relations through the FUSE mount.

## API Findings

1. Runtime availability policy is inconsistent and user-visible. The host treats enabled plugin registration failure as fatal and shuts down already loaded plugins in `bin/src/plugin/mod.rs:42`. Niri fails registration when the socket env var is missing or the compositor is unavailable because it connects before returning in `plugins/niri/src/ipc.rs:28`. PipeWire registers even when `pactl` is missing because command failures happen after the watcher is spawned in `plugins/pipewire/src/runtime.rs:21`. D-Bus follows the register-and-retry shape: it builds state and spawns service watchers in `plugins/dbus/src/runtime.rs:23`, then logs snapshot failures later in `plugins/dbus/src/runtime.rs:678`.

2. Niri's graph kinds are too generic for a concrete plugin. `window`, `workspace`, `output`, and `context` in `plugins/niri/src/lib.rs:20` will collide with any future compositor, display, workspace, or generic context provider. PipeWire, MPRIS, StatusNotifier, and DBusMenu use facade plus namespaced concrete kinds such as `pipewire-sink`, `mpris-player`, and `statusnotifier-item` in `plugins/pipewire/src/lib.rs:20`, `plugins/mpris/src/lib.rs:18`, and `plugins/statusnotifier/src/lib.rs:18`.

3. Provider types are public but not constructible. `NiriProvider` and `PipeWireProvider` are re-exported in `plugins/niri/src/lib.rs:8` and `plugins/pipewire/src/lib.rs:8`, but their constructors are `pub(crate)` in `plugins/niri/src/provider.rs:17` and `plugins/pipewire/src/provider.rs:18`. This exposes implementation types without a useful external API.

4. Config parsing accepts silent typos. The config structs derive `Deserialize` without `deny_unknown_fields` in `plugins/niri/src/config/mod.rs:4`, `plugins/pipewire/src/config/mod.rs:4`, and `plugins/project/src/config/mod.rs:6`. Niri currently has no fields, so user-provided Niri config can be accepted and ignored.

5. Plugin defaults are not surfaced through `default_config`. `LocusFsPlugin::default_config` exists in `plugins/api/src/lib.rs:72`, and the host merges it in `bin/src/plugin/mod.rs:95`, but Niri, PipeWire, and Project rely on serde defaults instead of returning visible default TOML. This makes config discovery and generated examples harder.

6. `ProjectConfig::state_path` is an API promise without behavior. The field is public in `plugins/project/src/config/mod.rs:7`, but any value fails registration in `plugins/project/src/lib.rs:32`. That is a valid temporary guard, but it should be documented as reserved or removed until persistence exists.

7. PipeWire's config surface is too narrow for process policy. `pactl` is configurable in `plugins/pipewire/src/config/mod.rs:5`, but retry delay, debounce, command timeout, and required/optional backend behavior are hardcoded in `plugins/pipewire/src/runtime.rs:144` and `plugins/pipewire/src/runtime.rs:149`.

8. Niri's IPC endpoint is not configurable. The socket path is taken only from `NIRI_SOCKET` via `SOCKET_PATH_ENV` in `plugins/niri/src/ipc.rs:127`. A config override would improve tests, multiple-instance cases, and optional startup policy.

9. The Project script can bypass the Project plugin. `ensure_project_kind` creates `/project` with `mkdir` when absent in `plugins/project/scripts/proj:104`. FUSE root `mkdir` creates a generic writable in-memory kind in `fuse/src/fs/filesystem.rs:396`. That means `proj update` may appear to work even when `locusfs-plugin-project` is not loaded.

## Redundancy Findings

1. Provider wrappers are nearly identical. Niri and PipeWire both store a `NodeKind` plus shared state, implement `with_state`, and delegate `NodeProvider`, `PropertyProvider`, and `RelationProvider` in `plugins/niri/src/provider.rs:10` and `plugins/pipewire/src/provider.rs:11`. D-Bus and the D-Bus-family plugins follow the same pattern in `plugins/dbus/src/provider.rs:15`, `plugins/mpris/src/provider.rs`, and `plugins/statusnotifier/src/provider.rs`.

2. Registration loops are duplicated. Niri registers node/property/relation providers in `plugins/niri/src/lib.rs:78`; PipeWire does the same plus path provider in `plugins/pipewire/src/lib.rs:72`; D-Bus does the same plus mutation/path providers in `plugins/dbus/src/lib.rs:72`; Project repeats the full mutable registration sequence in `plugins/project/src/lib.rs:43`.

3. Plugin handle shutdown is repeated. Niri and PipeWire both keep `Option<JoinHandle<()>>`, abort on drop, and abort/await on shutdown in `plugins/niri/src/lib.rs:32` and `plugins/pipewire/src/lib.rs:31`. MPRIS and StatusNotifier repeat the same shape in `plugins/mpris/src/lib.rs:27` and `plugins/statusnotifier/src/lib.rs:27`.

4. State query mechanics are repeated. Niri and PipeWire both implement `property_spec`, `properties`, `property`, `relations`, and `targets` by materializing maps and removing a key in `plugins/niri/src/state.rs:83` and `plugins/pipewire/src/state.rs:100`. D-Bus has the same graph-facing mechanics in `plugins/dbus/src/state.rs:183`.

5. Property and identity helpers are repeated. Niri has local `insert`, `relation`, `node_id`, `node_not_found`, and `string` helpers in `plugins/niri/src/state.rs:672`. PipeWire has the same helper family in `plugins/pipewire/src/state.rs:642`. D-Bus repeats equivalents in `plugins/dbus/src/state.rs:1396`.

6. Config error conversion is duplicated. Niri, PipeWire, Project, and D-Bus all have crate-local `config_error` functions with only the plugin label changed in `plugins/niri/src/lib.rs:123`, `plugins/pipewire/src/lib.rs:120`, `plugins/project/src/lib.rs:94`, and `plugins/dbus/src/lib.rs:125`.

7. The Project script computes the same snapshot twice. `metadata --json` builds a snapshot in `plugins/project/scripts/proj:202`, while `update` recomputes metadata, branch, worktree, regex, and derived fields independently in `plugins/project/scripts/proj:236`.

## Performance And Concurrency Findings

1. PipeWire refreshes are process-heavy and not debounced. Every relevant `pactl subscribe` line can trigger `info`, `list sinks`, and `list sources` in `plugins/pipewire/src/runtime.rs:115`. Bursty sink/source events will serialize several full subprocess snapshots. Add a short debounce/coalescing window before refresh.

2. PipeWire subprocesses have no timeout. `pactl_json` awaits `Command::output` directly in `plugins/pipewire/src/runtime.rs:128`. A hung command can stall all future refreshes because the watcher loop awaits each refresh inline.

3. PipeWire loses useful subscribe diagnostics. `pactl subscribe` redirects stderr to null in `plugins/pipewire/src/runtime.rs:39`; when stdout ends, the code kills the child and logs only that subscribe ended in `plugins/pipewire/src/runtime.rs:67`.

4. Niri holds the state write guard while emitting changes. `read_event_stream` creates `let mut state = state.write().await` and emits inside the same match arm in `plugins/niri/src/ipc.rs:83`. There is no `.await` while the guard is held, but scoping should match PipeWire's explicit drop-before-emit block in `plugins/pipewire/src/runtime.rs:97`.

5. Graph reads allocate whole property/relation maps. Niri's `property` and `property_spec` rebuild all node properties in `plugins/niri/src/state.rs:83`; PipeWire does the same in `plugins/pipewire/src/state.rs:100`. FUSE often asks for stat and read separately, so hot UI reads can allocate multiple maps per visible property.

6. Per-plugin runtimes create thread overhead. `PluginRuntime::new` always builds a two-thread multi-thread runtime in `plugins/api/src/lib.rs:108`. Niri and PipeWire each mostly run one background I/O loop. If several desktop plugins are enabled, the runtime footprint grows quickly.

7. Retry loops are fixed and noisy. Niri sleeps one second in `plugins/niri/src/ipc.rs:105`; PipeWire sleeps two seconds in `plugins/pipewire/src/runtime.rs:149`; D-Bus sleeps one second in `plugins/dbus/src/runtime.rs:271`. There is no shared backoff policy, jitter, health state, or tracing integration.

8. PipeWire endpoint node ids are based on pactl indices. `snapshot_from_pactl` keys endpoints by `endpoint.id.to_string()` in `plugins/pipewire/src/state.rs:331`. These ids are compact but may churn across server restarts; endpoint names may be more stable for filesystem identity.

## Tidiness And Documentation Findings

1. Public APIs need docs at their discovery points. The concrete plugin crates expose public structs, config modules, constants, and registration functions with minimal docs. Rust-guide expects public-facing API documentation in `lib.rs` or module roots.

2. Large state files mix multiple responsibilities. `plugins/niri/src/state.rs` contains state storage, graph projection, property construction, relation construction, event diffing, node id helpers, and panic adaptation. `plugins/pipewire/src/state.rs` contains domain models, pactl DTOs, snapshot conversion, path layout, graph projection, diffing, and presentation helpers. These are good candidates for `model.rs`, `projection.rs`, `changes.rs`, and `path.rs` when refactoring.

3. Tests are unevenly organized. Niri uses `state/test.rs` wired from `plugins/niri/src/state.rs:944`, which matches the Rust-guide pattern. PipeWire has a long inline test module inside `plugins/pipewire/src/state.rs:682`, and runtime/config tests are inline in `plugins/pipewire/src/runtime.rs:153` and `plugins/pipewire/src/config/mod.rs:28`.

4. Runtime logging uses `eprintln!`. Niri logs failures in `plugins/niri/src/ipc.rs:56` and `plugins/niri/src/ipc.rs:88`; PipeWire logs in `plugins/pipewire/src/runtime.rs:45` and `plugins/pipewire/src/runtime.rs:92`. The workspace already has tracing spans around plugin load/shutdown in `bin/src/plugin/mod.rs:38`, and providers use `TracedProvider`, so plugin runtimes should emit `tracing` events instead.

5. Filesystem layout is not documented per plugin. There are no plugin README files. Consumers must infer paths from state/path-provider code such as `plugins/pipewire/src/state.rs:141` or the Project script paths in `plugins/project/scripts/proj:95`.

6. The Project script has no test harness. Its behavior is domain-heavy, includes metadata parsing, git fallback, URI/path encoding, relation symlinks, and stale-property concerns, but no tests exercise it.

## Best-Practice And Crate Reuse Notes

1. Project correctly reuses `InMemoryProvider` rather than implementing another writable graph store in `plugins/project/src/lib.rs:40`.

2. PipeWire uses `pactl -f json` and serde DTOs rather than parsing human text in `plugins/pipewire/src/runtime.rs:115` and `plugins/pipewire/src/state.rs:353`. That is a good baseline if the project stays with `pactl`.

3. PipeWire should add a small command-runner abstraction before adding more process behavior. That enables tests for stdout/stderr/status/timeout without spawning real PipeWire commands.

4. Native PipeWire bindings may eventually reduce process overhead and improve event fidelity, but that should be a separate research task. Local evidence does not currently justify adding a heavy native dependency before tightening the existing `pactl` adapter.

5. Niri correctly reuses `niri_ipc` request/reply/event types in `plugins/niri/src/ipc.rs:8`. The custom async `UnixStream` wrapper is reasonable if `niri_ipc` does not provide a Tokio client, but the socket path and reconnect policy should be injectable for tests.

6. The Project script sensibly uses `jq` for JSON and URI encoding in `plugins/project/scripts/proj:34`, but the Bash surface is now large enough that snapshot generation and filesystem publishing should be separated and tested.

7. Unknown config fields should be denied for plugin configs unless there is a deliberate forward-compatibility policy. Silent ignore is not a good operational default for local desktop plumbing.

## Domain-Specific Plugin Structure Notes

1. PipeWire matches the strongest visible pattern: a readable facade kind (`pipewire`) plus hidden concrete endpoint kinds (`pipewire-sink`, `pipewire-source`) and a `PathProvider` that exposes endpoints under the facade. This is consistent with MPRIS and StatusNotifier, which expose facade nodes and hidden item/player kinds in `plugins/mpris/src/state.rs:133` and `plugins/statusnotifier/src/state.rs:138`.

2. Niri does not follow that pattern. It exposes concrete desktop concepts as root kinds directly. If those concepts are intended to be cross-plugin concepts, this needs documentation and ownership rules. If they are Niri-specific, prefer `niri` facade plus `niri-window`, `niri-workspace`, `niri-output`, and `niri-context` hidden or semi-hidden kinds.

3. Project is structurally different because it is writable user state rather than a read-only backend snapshot. That difference is justified, but the script should not silently create the generic `project` kind if the plugin is expected to own the domain.

4. PipeWire facade path naming should be standardized before more plugins copy it. Current paths are singular (`/pipewire/sink/<id>`, `/pipewire/source/<id>`, `/pipewire/default/sink`) in `plugins/pipewire/src/state.rs:15`. D-Bus refactor direction prefers plural, semantic directories such as `objects` and `methods`; decide whether facade collection directories should be plural workspace-wide.

5. Project relations assume a generic `/workspace/<id>/project` relation in `plugins/project/scripts/proj:298`. Today that is effectively coupled to Niri's generic `workspace` and `context` kinds. If Niri is namespaced, Project needs a stable selected-workspace API or configurable workspace provider path.

6. D-Bus currently uses `@properties`/`@methods` path constants in `plugins/dbus/src/state.rs:20`. PipeWire and Project do not repeat that `@` marker style. Future native plugin path providers should avoid provider-specific sigils unless there is a documented convention.

## Concrete Refactor Plan

1. Decide the runtime availability policy first.
   - Option A: all optional desktop integrations register successfully with empty state and retry in the background.
   - Option B: each plugin has `required = true/false`, defaulting to optional for desktop backends.
   - Apply the decision to Niri, PipeWire, and D-Bus-family plugins consistently because host plugin loading is fail-fast.

2. Define plugin filesystem naming policy.
   - Decide whether `window`, `workspace`, `output`, and `context` are global graph concepts or Niri-owned concepts.
   - If Niri-owned, introduce a `niri` facade path provider and namespaced concrete kinds.
   - If global, document the contract and how other providers must coexist.

3. Add shared plugin helpers.
   - Add a config error helper in `locusfs-plugin-api`.
   - Add registration helpers for traced read-only providers and traced read-write providers.
   - Add a small task handle/group helper that centralizes abort-on-drop/shutdown behavior.
   - Add `emit_graph_changes(label, graph, changes)` using `tracing` instead of repeated `eprintln!`.

4. Add a small read-only projection abstraction only if it stays simple.
   - A practical shape is a shared provider wrapper over `Arc<RwLock<S>>` where `S` supplies `contains_node`, `nodes_for_kind`, `node_properties`, and `node_relations`.
   - Keep domain projection functions in each plugin; only remove repeated graph-facing query mechanics.

5. Refactor Niri.
   - Add config fields for socket path override and availability policy.
   - If optional mode is chosen, initialize empty state and move initial connect/output request into the retry loop.
   - Scope the write lock so graph changes emit after the guard drops.
   - Split state into event/change derivation and graph projection modules.
   - Add facade/path-provider support if namespacing is chosen.

6. Refactor PipeWire.
   - Validate config, deny unknown fields, and expose defaults through `default_config`.
   - Add command timeout, refresh debounce, and retry policy config.
   - Capture/log subscribe exit status and stderr where useful.
   - Consider stable endpoint identity based on endpoint name, with index as a property, if filesystem path stability matters more than compact ids.
   - Split DTO/model/projection/path/change code out of `state.rs`.

7. Refactor Project.
   - Either remove `state_path` until persistence is implemented, or document it as reserved and keep the rejecting test.
   - Decide whether the script may create a generic `/project` kind. If not, make `ensure_project_kind` fail with a clear message when the plugin is absent.
   - Make `build_snapshot` the single source of metadata/git-derived fields and have `update` publish that snapshot.
   - Track script-managed metadata keys so removed keys can be removed safely without deleting user-owned graph properties.
   - Clarify or implement `display-main` and `display-secondary` handling.

8. Document plugin layouts.
   - Add crate-level docs or plugin docs that describe visible root kinds, hidden kinds, facade paths, properties, relations, mutation behavior, runtime availability behavior, and config fields.

## Test Plan

Niri:

- Keep existing state projection tests in `plugins/niri/src/state/test.rs`.
- Add config tests for `deny_unknown_fields`, socket path override, and availability policy.
- Add a Tokio UnixListener fake Niri socket test for initial outputs, event stream setup, reconnect, and missing-socket behavior.
- Add graph-level registration tests for the chosen optional/fail-fast policy.
- Add path-provider tests if Niri moves to a facade layout.

PipeWire:

- Move inline state/runtime tests into module `test.rs` files as files are split.
- Add tests for config validation/default TOML.
- Add a fake command runner or temp executable tests for `pactl_json`: success, non-zero exit, invalid JSON, timeout.
- Add debounce tests that coalesce bursty subscription lines into one refresh.
- Add path-provider tests for `/pipewire/default`, `/pipewire/sink`, `/pipewire/source`, and watch targets.
- Add snapshot identity tests if endpoint ids change from pactl index to endpoint name.

Project:

- Add graph tests that `register_with_config` creates a writable project kind and supports create/set/remove/link behavior through `DynamicGraph`.
- Keep or update the `state_path` rejection test in `plugins/project/src/lib.rs:110`.
- Add script integration tests using temp project roots and temp mount directories. Skip only when required tools such as `jq` are missing.
- Test `init`, `root`, `metadata --json`, `update`, `set-current`, `clear`, branch regex extraction, custom metadata values, removed metadata keys, and mountpoint env precedence.

Cross-cutting:

- Add tests for shared registration helpers so provider capabilities are registered exactly once and duplicate kind errors are clear.
- Add tests for shared task handle shutdown to ensure abort/await behavior is preserved.
- Run narrow plugin tests first: `cargo test -p locusfs-plugin-niri`, `cargo test -p locusfs-plugin-pipewire`, and `cargo test -p locusfs-plugin-project`.
- Then run broader verification: `cargo test` and `cargo fmt --check`.

## Open Questions For Coordinator Arbitration

1. Should runtime-backed desktop plugins be optional by default, or should an enabled plugin always be required for mount success?

2. Are `window`, `workspace`, `output`, and `context` intended to be global Locus graph concepts, or should Niri namespace them?

3. Should facade collection directories be singular (`sink`, `source`, `player`) or plural (`sinks`, `sources`, `players`) across plugins?

4. Should `ProjectPlugin` own the `project` kind strictly, or is the script allowed to create a generic in-memory `project` kind when the plugin is not loaded?

5. Is Project persistence planned soon enough to keep `state_path` in the public config, or should it be removed for now?

6. Should PipeWire node identity prefer stable endpoint names or compact pactl indices?

7. Should plugin runtime logging move fully to `tracing`, including retry loops and backend health state?

8. Is a shared projection/provider abstraction desired now, or should the first refactor only add smaller helpers for config errors, registration, task handles, and graph-change emission?
