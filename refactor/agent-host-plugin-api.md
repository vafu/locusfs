# Host Binary And Plugin API Review

## Current Role

Reviewer: host binary and plugin API reviewer for the LocusFS review/refactor pass.

Scope covered:

- Fully reviewed `plugins/api/` and `bin/`.
- Inspected concrete plugin entrypoints/config modules only where needed to understand API usage patterns.
- Did not edit source files or other reports.

Verification run:

- `cargo test -p locusfs-bin -p locusfs-plugin-api`
- Result: passed. `locusfs-bin` ran 6 unit tests; `locusfs-plugin-api` currently has 0 unit/doc tests.

## Public API And Entrypoints

- Plugin crate contract: `PluginManifest`, `PluginContext`, `LocusFsPlugin`, `PluginHandle`, `PluginRuntime`, and `enter_runtime` are exported from one public module in `plugins/api/src/lib.rs:15`, `plugins/api/src/lib.rs:31`, `plugins/api/src/lib.rs:68`, `plugins/api/src/lib.rs:90`, `plugins/api/src/lib.rs:101`, and `plugins/api/src/lib.rs:138`.
- The API crate explicitly documents that the current dynamic ABI is not stable across compilers/toolchains because Rust trait objects cross the dylib boundary (`plugins/api/src/lib.rs:1`).
- Host CLI entrypoint is `#[tokio::main] async fn main`, delegating to `run()` in `bin/src/main.rs:19` and `bin/src/main.rs:30`.
- CLI forms are manual: `locusfs [--config <path>] <mountpoint>` and `locusfs --watch <path>` (`bin/src/main.rs:217`, `bin/src/main.rs:260`).
- Host config surface is private to the binary but effectively controls plugin loading: `Config { plugin_dirs, plugins }` and `PluginConfig { enabled, library, config }` (`bin/src/config/mod.rs:14`, `bin/src/config/mod.rs:22`).
- Dynamic plugin loading expects `_locusfs_plugin_init` with type `unsafe extern "C" fn() -> *mut dyn LocusFsPlugin` (`bin/src/plugin/mod.rs:16`). Every current dynamic plugin exports that symbol, for example `plugins/dbus/src/lib.rs:119`, `plugins/project/src/lib.rs:88`, and `plugins/statusnotifier/src/lib.rs:98`.
- Perfetto tracing is enabled by `LOCUSFS_PERFETTO_TRACE` (`bin/src/perfetto.rs:24`) and initialized in `init_tracing()` (`bin/src/main.rs:159`).
- Watch CLI calls `locusfs_watch::Watch` directly from `bin/src/watch.rs:7`.

## Step-By-Step Walkthrough

### Mount Command

1. `main()` initializes the Tokio runtime and maps `run()` errors to stderr plus failure exit code (`bin/src/main.rs:19`).
2. `run()` initializes tracing first, then parses CLI args (`bin/src/main.rs:31`, `bin/src/main.rs:32`).
3. `--watch` is handled as an early return and bypasses config, plugin loading, and FUSE mounting (`bin/src/main.rs:34`).
4. Mount mode prepares the mountpoint and creates it if needed (`bin/src/main.rs:41`, `bin/src/main.rs:42`).
5. Config loading reads the explicit or default config path, treating a missing implicit config as `Config::default()` (`bin/src/config/mod.rs:43`, `bin/src/config/mod.rs:48`).
6. `default_graph()` creates a `DynamicGraph` and asks `PluginManager::load_enabled()` to populate it (`bin/src/main.rs:264`, `bin/src/main.rs:267`, `bin/src/main.rs:268`).
7. Enabled plugins are loaded sequentially in `BTreeMap` key order; the manager shuts down already-loaded plugins if a later plugin fails (`bin/src/plugin/mod.rs:32`, `bin/src/plugin/mod.rs:34`, `bin/src/plugin/mod.rs:44`).
8. FUSE is mounted only after plugins register their providers (`bin/src/main.rs:61`).
9. Shutdown waits for SIGINT/SIGTERM or Ctrl-C, then currently shuts down plugins before unmounting FUSE (`bin/src/main.rs:73`, `bin/src/main.rs:74`, `bin/src/main.rs:75`).
10. Unmount falls back to `fusermount3 -u -z` by matching the error string `"Device or resource busy"` (`bin/src/main.rs:88`, `bin/src/main.rs:90`, `bin/src/main.rs:129`).

### Plugin Load Path

1. Host resolves the library path from an explicit `library`, configured `plugin_dirs`, the executable directory, or `~/.local/lib/locusfs/plugins` (`bin/src/plugin/mod.rs:115`, `bin/src/plugin/mod.rs:136`).
2. Host loads the dylib with `libloading::Library::new`, looks up `_locusfs_plugin_init`, calls it, and converts the raw pointer into `Box<dyn LocusFsPlugin>` (`bin/src/plugin/mod.rs:75`, `bin/src/plugin/mod.rs:76`, `bin/src/plugin/mod.rs:77`, `bin/src/plugin/mod.rs:85`).
3. Host reads `plugin.manifest()` and verifies only `manifest.id == config id` (`bin/src/plugin/mod.rs:87`, `bin/src/plugin/mod.rs:88`).
4. Host merges plugin defaults and user config as raw TOML values (`bin/src/plugin/mod.rs:95`, `bin/src/plugin/mod.rs:157`).
5. Host constructs `PluginContext` from the current Tokio runtime and calls `plugin.register()` (`bin/src/plugin/mod.rs:97`, `plugins/api/src/lib.rs:49`).
6. `LoadedPlugin` retains the handle, plugin object, and `Library` so the library stays loaded while trait objects exist (`bin/src/plugin/mod.rs:24`).

### Concrete Plugin Pattern

1. Plugin root modules define manifest literals, optional typed config, provider kind constants, a handle type, and `_locusfs_plugin_init`.
2. Configured plugins parse raw `toml::Value` through a local `from_value` helper, for example `DbusConfig::from_value` (`plugins/dbus/src/config/mod.rs:29`) and `PipeWireConfig::from_value` (`plugins/pipewire/src/config/mod.rs:18`).
3. Runtime-backed plugins create a `PluginRuntime` inside registration, not from `PluginContext.runtime`, for example D-Bus and PipeWire (`plugins/dbus/src/lib.rs:69`, `plugins/pipewire/src/lib.rs:69`).
4. Providers are registered directly against `DynamicGraph`, usually in repeated loops over provider kinds (`plugins/dbus/src/lib.rs:72`, `plugins/mpris/src/lib.rs:59`, `plugins/statusnotifier/src/lib.rs:59`).

### Watch Command

1. `watch_path()` opens a watch client for the supplied path (`bin/src/watch.rs:7`).
2. Directory paths print raw watch events forever (`bin/src/watch.rs:9`, `bin/src/watch.rs:11`).
3. Non-directory paths print an initial read, then read after each change (`bin/src/watch.rs:15`, `bin/src/watch.rs:17`).

### Perfetto/Tracing

1. `init_tracing()` enables in-process Perfetto only when `LOCUSFS_PERFETTO_TRACE` is set (`bin/src/main.rs:159`, `bin/src/main.rs:160`, `bin/src/main.rs:162`).
2. It always installs an env-filtered fmt tracing layer and conditionally installs Perfetto layers (`bin/src/main.rs:172`, `bin/src/main.rs:175`, `bin/src/main.rs:179`).
3. Plugin load/shutdown spans include a `plugin` field, and the custom layer maps that field onto Perfetto tracks (`bin/src/plugin/mod.rs:39`, `bin/src/plugin/mod.rs:63`, `bin/src/perfetto.rs:70`).
4. Trace data is flushed and written synchronously in `PerfettoTraceSession::drop` (`bin/src/perfetto.rs:191`, `bin/src/perfetto.rs:197`, `bin/src/perfetto.rs:222`).

## Behavior Summary

The current design is intentionally simple and mostly coherent for same-workspace plugins: the binary owns config parsing, dynamic loading, graph creation, FUSE mounting, signal handling, and tracing. Plugins own typed config interpretation, provider registration, background task startup, and shutdown. The host and plugins share graph types, Tokio types, TOML values, and boxed Rust traits.

The highest-risk area is the dynamic ABI boundary. The code documents that it is not stable (`plugins/api/src/lib.rs:3`), but the host still loads arbitrary configured dylibs and calls into a Rust trait-object constructor before any ABI/version/capability validation (`bin/src/plugin/mod.rs:75`). The next highest-risk area is lifecycle ordering: the host shuts plugin runtimes down while the FUSE mount is still active (`bin/src/main.rs:73`), leaving provider objects in the graph with their runtime/task handles already torn down.

## API Findings

### A1. Dynamic ABI Is Not Replacement-Friendly Or Strongly Validated

`PluginInit` returns `*mut dyn LocusFsPlugin` from an `extern "C"` function and suppresses `improper_ctypes_definitions` (`bin/src/plugin/mod.rs:16`). This is only safe for plugins built with exactly compatible Rust ABI assumptions. The manifest id check happens after `init()` has already returned a Rust trait object (`bin/src/plugin/mod.rs:77`, `bin/src/plugin/mod.rs:87`), so it cannot protect against ABI mismatches.

Recommended direction:

- Decide whether plugins are same-build internal extensions or third-party binary extensions.
- If same-build only, make that a hard contract: encode plugin API crate version/build id, validate an ABI/version symbol before `init`, and improve diagnostics.
- If third-party binary compatibility is a goal, stop exposing Rust trait objects across `cdylib`; use a C-compatible descriptor/vtable, a crate designed for stable Rust ABI, or an out-of-process plugin protocol.

### A2. Runtime Ownership Contract Is Contradictory

`PluginContext` says plugins receive the host Tokio handle they should use for long-lived async work (`plugins/api/src/lib.rs:27`, `plugins/api/src/lib.rs:34`). `PluginRuntime` says long-lived plugin work should use a plugin-owned runtime instead of the host runtime (`plugins/api/src/lib.rs:95`). Current runtime-backed plugins follow `PluginRuntime`, not `PluginContext.runtime` (`plugins/dbus/src/lib.rs:69`, `plugins/mpris/src/lib.rs:56`, `plugins/pipewire/src/lib.rs:69`).

Recommended direction:

- Pick one policy.
- Prefer a narrow host-provided spawning/registration abstraction if the host owns runtime policy.
- Prefer removing `runtime` from `PluginContext` if plugins must own runtime-bound resources inside their dylib.
- If plugin-owned runtimes stay, make thread count, shutdown timeout, and task tracking explicit API decisions.

### A3. `PluginContext` Leaks Host Implementation Details

`PluginContext` exposes public `DynamicGraph` and `tokio::runtime::Handle` fields (`plugins/api/src/lib.rs:31`). This makes plugins depend on the host's current graph implementation and Tokio runtime model. It is easy to use, but replacement-unfriendly: a future host cannot swap graph registration, isolate plugins, or validate capabilities without changing plugin code.

Recommended direction:

- Introduce a plugin-facing registrar/capability object with methods for provider registration.
- Keep `DynamicGraph` available only where concrete in-process graph access is intentional.
- Make context fields private and provide documented accessors during transition.

### A4. Config Ownership Is Split And Raw TOML Leaks Through The Contract

The host owns global config and raw plugin TOML (`bin/src/config/mod.rs:14`, `bin/src/config/mod.rs:22`). Plugins own typed config parsing (`plugins/dbus/src/config/mod.rs:29`, `plugins/project/src/config/mod.rs:12`). The API also has `default_config() -> toml::Value` (`plugins/api/src/lib.rs:72`), and the host recursively merges defaults before handing the value back to plugins (`bin/src/plugin/mod.rs:95`, `bin/src/plugin/mod.rs:157`).

In practice, current plugins use serde defaults and local `Default` impls instead of overriding `default_config`; `rg default_config` found no concrete override. That makes `default_config` a mostly unused second defaulting mechanism.

Recommended direction:

- Either remove `default_config` and let plugins parse/default their own config, or make defaults part of a real config-schema/export story.
- Pass `toml::Table` or a `PluginConfig` wrapper instead of arbitrary `toml::Value`, so non-table user config is rejected at the boundary with better diagnostics.
- Preserve source/path context in config errors. Current plugin parse errors become graph-domain `InvalidValue` strings (`plugins/dbus/src/lib.rs:125`).

### A5. `PluginHandle::shutdown` Cannot Report Failure

`PluginHandle::shutdown(self: Box<Self>)` returns `()` by default (`plugins/api/src/lib.rs:90`). Host shutdown ignores all plugin teardown failures because there is no error channel (`bin/src/plugin/mod.rs:108`).

Recommended direction:

- Change shutdown to return `Result<()>`.
- Have `PluginManager::shutdown` collect and report per-plugin errors.
- Keep a best-effort shutdown path, but do not make failure invisible.

### A6. Manifest Is Too Small For Host Decisions

`PluginManifest` has `id`, `name`, and `version` only (`plugins/api/src/lib.rs:17`). The host can validate identity but cannot validate ABI version, required host/plugin API version, graph capabilities, path namespace, provided node kinds, config schema version, or dependencies.

Recommended direction:

- Add ABI/API compatibility metadata.
- Add optional declared capabilities/provided namespaces.
- Keep human-readable fields separate from machine-validated fields.

## Redundancy Findings

### R1. Plugin Entry Boilerplate Is Repeated In Every Plugin

Every plugin repeats the same pattern: manifest literal, `LocusFsPlugin` impl, `register(context, config)`, config conversion, and `_locusfs_plugin_init`. Examples: `plugins/dbus/src/lib.rs:97`, `plugins/project/src/lib.rs:66`, `plugins/statusnotifier/src/lib.rs:79`.

Recommended direction:

- Add a small `declare_locusfs_plugin!` macro or helper in `locusfs-plugin-api`.
- The macro should export the dynamic symbol, attach ABI/version metadata, and remove repeated unsafe/allow attributes from plugin crates.

### R2. Config Conversion And Error Mapping Are Repeated

Configured plugins repeat `from_value(value).try_into().map_err(config_error)` and local `config_error` functions (`plugins/dbus/src/config/mod.rs:29`, `plugins/dbus/src/lib.rs:125`, `plugins/dbusmenu/src/config/mod.rs:28`, `plugins/dbusmenu/src/lib.rs:111`, `plugins/pipewire/src/config/mod.rs:18`, `plugins/pipewire/src/lib.rs:120`).

Recommended direction:

- Add `parse_plugin_config<T>(value, plugin_id)` or a `PluginConfigExt` helper.
- Use one diagnostic format for config shape errors.

### R3. Provider Registration Loops Duplicate Capabilities

Plugins repeatedly register the same provider object into node/property/path/relation slots with local conditionals (`plugins/dbus/src/lib.rs:72`, `plugins/dbusmenu/src/lib.rs:56`, `plugins/mpris/src/lib.rs:59`, `plugins/pipewire/src/lib.rs:72`).

Recommended direction:

- Add a graph/plugin helper for provider capability bundles.
- Model capabilities explicitly, for example read nodes, read properties, mutate properties, path layout, relations.
- This would also make manifest capability declarations easier to validate.

### R4. Task Shutdown Patterns Are Repeated And Inconsistent

Several handles store one `JoinHandle<()>`, abort it, and await it (`plugins/mpris/src/lib.rs:28`, `plugins/mpris/src/lib.rs:43`, `plugins/pipewire/src/lib.rs:31`, `plugins/pipewire/src/lib.rs:46`). D-Bus stores a vector and has both `Drop` and async shutdown (`plugins/dbus/src/lib.rs:31`, `plugins/dbus/src/lib.rs:36`, `plugins/dbus/src/lib.rs:46`). Project has no async tasks and uses the default shutdown (`plugins/project/src/lib.rs:16`, `plugins/project/src/lib.rs:21`).

Recommended direction:

- Add a `PluginTaskGroup`/`PluginRuntimeHandle` helper that aborts, awaits, and reports `JoinError`s consistently.
- Prefer explicit shutdown over relying on `Drop` for async tasks.

## Performance And Concurrency Findings

### P1. Per-Plugin Runtime Defaults Are Expensive And Not Tunable

`PluginRuntime::new()` always creates a multi-thread runtime with two worker threads (`plugins/api/src/lib.rs:106`, `plugins/api/src/lib.rs:109`). With six runtime-backed plugins, this can create a dozen plugin worker threads before considering the host runtime.

Recommended direction:

- Decide whether each plugin needs isolation strongly enough to justify dedicated runtimes.
- Consider current-thread runtimes for mostly I/O-bound plugins, a shared plugin runtime, or a configurable runtime policy.
- Add shutdown timeout semantics instead of unconditional `shutdown_background()` (`plugins/api/src/lib.rs:129`).

### P2. Plugin Shutdown Happens Before FUSE Unmount

The current shutdown sequence calls `plugins.shutdown().await` before `mount.unmount().await` (`bin/src/main.rs:73`, `bin/src/main.rs:74`, `bin/src/main.rs:75`). The graph still contains provider objects registered by those plugins, and FUSE can still receive operations during unmount.

Recommended direction:

- Prefer unmounting or otherwise quiescing FUSE before tearing down plugin runtimes.
- If plugins need a pre-unmount signal, make lifecycle two-phase: stop external watchers, unmount, release providers/runtime.
- Add tests around shutdown ordering with a fake provider/handle.

### P3. Async Drop/Lifetime Around Dynamic Libraries Needs Hardening

`LoadedPlugin` stores handle, plugin object, and library together (`bin/src/plugin/mod.rs:24`). This is directionally correct because the library must outlive the Rust values created from it, but it relies on field/drop ordering and on every plugin handle shutting down cleanly before the library unloads. `Drop` fallbacks in plugin handles abort tasks without awaiting them (`plugins/dbus/src/lib.rs:36`, `plugins/niri/src/lib.rs:37`), which is risky for dylib-owned futures/resources.

Recommended direction:

- Make `PluginManager::shutdown` consume the manager or mark it fully shut down.
- Document and test the drop-order invariant.
- Avoid unloading the library until all plugin-owned tasks are awaited or a timeout path has been taken.

### P4. Graph Provider Lookup Pattern Is Good

`DynamicGraph` reads provider maps under a lock, clones an `Arc`, then awaits provider work outside the registry lock (`graph/src/graph/dynamic.rs:323`, `graph/src/graph/dynamic.rs:348`). That is the right high-level concurrency shape for provider calls.

No immediate refactor recommended here for the host/plugin API pass.

### P5. Minor Hot-Path Issues Are Startup/Diagnostic Only

Config merge clones raw TOML once at startup (`bin/src/plugin/mod.rs:95`). Path de-duplication is O(n^2) (`bin/src/plugin/mod.rs:172`) and `normalize_path` is currently a no-op (`bin/src/plugin/mod.rs:186`), but plugin search dirs are small. Watch CLI recreates a stdout handle per print (`bin/src/watch.rs:21`), which is minor.

Recommended direction:

- Fix path canonicalization for correctness/diagnostics, not performance.
- Reuse stdout in watch CLI only if that module is otherwise being refactored.

## Tidiness And Docs Findings

### T1. Public Plugin API Docs Are Present But Missing Lifecycle Invariants

The API crate has useful top-level docs and item docs (`plugins/api/src/lib.rs:1`, `plugins/api/src/lib.rs:62`, `plugins/api/src/lib.rs:85`). Missing details:

- Whether dynamic plugins are same-build only or expected to be independently built.
- Whether plugins should use host runtime or plugin-owned runtime.
- Whether `register()` may partially register providers before failing.
- Whether handles must be shut down before FUSE unmount and before library unload.
- What `default_config()` is for if plugins use serde defaults.

### T2. Unsafe Blocks Need Safety Documentation

The host has unsafe dynamic loading blocks (`bin/src/plugin/mod.rs:75`, `bin/src/plugin/mod.rs:76`, `bin/src/plugin/mod.rs:77`). `RuntimeEntered` uses unchecked pin projection without a safety explanation (`plugins/api/src/lib.rs:157`, `plugins/api/src/lib.rs:164`).

Recommended direction:

- Add `SAFETY:` comments when implementation happens.
- If `RuntimeEntered` stays, replace manual projection with `pin-project-lite` or a small, audited implementation plus tests.
- If no plugin uses `enter_runtime`, remove it until a concrete need exists.

### T3. Host Errors Are Too Stringly

The binary uses `Box<dyn Error + Send + Sync>` aliases (`bin/src/main.rs:17`, `bin/src/config/mod.rs:12`, `bin/src/plugin/mod.rs:15`) and creates `io::Error::other` for plugin diagnostics (`bin/src/plugin/mod.rs:190`). Unmount fallback matches an error string (`bin/src/main.rs:90`).

Recommended direction:

- Add a typed `HostError`/`PluginLoadError` with `thiserror`.
- Preserve path, plugin id, symbol name, and phase in errors.
- Avoid string matching for known OS errors where possible.

### T4. Host Module Boundaries Are Too Coarse For Testing

`bin/src/plugin/mod.rs` owns loading, path resolution, config merge, manager lifecycle, and errors in one file (`bin/src/plugin/mod.rs:31`). `bin/src/main.rs` owns CLI parsing, mountpoint cleanup, tracing init, lifecycle orchestration, and shutdown (`bin/src/main.rs:30`).

Recommended direction:

- Split plugin host into `manager`, `loader`, `paths`, `config_merge`, and `error` modules, or move host logic into a reusable `locusfs-host` library crate.
- Keep the binary as CLI orchestration once host behavior is reusable/testable.

## Best-Practice And Crate Reuse Notes

- `libloading` is an appropriate low-level loader, but it does not make Rust trait objects ABI-stable. The contract needs either same-build enforcement or a stable ABI/protocol layer.
- `async-trait` remains reasonable while `LocusFsPlugin` and provider traits must be object-safe. The issue is not async-trait itself; it is exposing boxed trait objects across a dylib boundary.
- The workspace already uses `thiserror` in `locusfs-graph` (`graph/src/error.rs:3`). The binary should use a direct typed error dependency instead of more boxed/string errors.
- Consider `clap` for CLI parsing once watch becomes a stable subcommand. Current manual parsing is small but will not scale well (`bin/src/main.rs:217`).
- Consider `directories`/`etcetera` for XDG paths and `shellexpand` or a local dedicated helper for tilde expansion if config path behavior grows. Current tilde expansion only applies to `plugin_dirs` and `library`, not plugin-owned path values such as `ProjectConfig.state_path` (`bin/src/config/mod.rs:68`, `plugins/project/src/config/mod.rs:7`).
- Consider `tokio_util::task::TaskTracker` or a small local equivalent for plugin task shutdown. Pulling a dependency is optional; the main goal is one consistent shutdown policy.
- If `RuntimeEntered` is retained, use `pin-project-lite` or equivalent rather than hand-rolled unchecked pinning.

## Domain-Specific Plugin Structure Notes

- Current plugin crate structure is mostly consistent: root `lib.rs`, optional `config`, `provider`, `runtime`, and `state` modules. This should be documented as the standard pattern.
- Some plugins expose `pub mod config` while others have no config module (`plugins/mpris/src/lib.rs:3`, `plugins/statusnotifier/src/lib.rs:3`). That is fine, but the host/API should define whether every plugin has a documented config schema, even if empty.
- Provider kind constants are locally defined but not declared to the host before registration, for example D-Bus defines three kinds (`plugins/dbus/src/lib.rs:20`) and Niri defines four (`plugins/niri/src/lib.rs:20`). Manifest capability declarations would help document and validate path/kind ownership.
- The API currently provides no standard for plugin filesystem layouts. `PathProvider` is powerful but low-level (`graph/src/graph/mod.rs:113`). This leaves domain-specific structures such as D-Bus methods/properties to each plugin without host-visible documentation.
- The D-Bus path-structure refactor noted in `AGENTS.md` should probably be paired with a plugin layout guideline: plugin roots should expose stable namespace folders and avoid magic `@property`/`@method` markers when real directories can communicate meaning.
- Config docs should live near plugin manifests or generated plugin docs, not only as serde structs.

## Concrete Refactor Plan

### Phase 1: Decide And Harden The Plugin Boundary

1. Decide whether dynamic plugins are same-build internal plugins or third-party ABI-stable plugins.
2. Add explicit plugin API/ABI metadata before calling the Rust trait-object initializer.
3. Add a helper or macro that exports `_locusfs_plugin_init` plus metadata consistently.
4. If third-party compatibility is required, replace the trait-object ABI with a C-compatible descriptor/vtable, a stable-ABI crate, or an out-of-process protocol.

### Phase 2: Clean Up The Plugin Contract

1. Replace public `PluginContext` fields with accessors or a narrower `PluginRegistrar`.
2. Resolve runtime policy: either host-owned spawner/cancellation or plugin-owned runtime helper, not both as competing recommendations.
3. Change `PluginHandle::shutdown` to return `Result<()>`.
4. Remove `default_config()` unless it becomes part of a concrete schema/default export feature.
5. Add manifest fields for API/ABI compatibility and optional capabilities/path namespaces.

### Phase 3: Refactor Host Plugin Management

1. Split `bin/src/plugin/mod.rs` into loader, manager, search paths, config merge, and errors.
2. Introduce typed host/plugin load errors.
3. Validate plugin ids before deriving library file names.
4. Improve path de-duplication and diagnostics by reporting all searched paths.
5. Store manifest data in `LoadedPlugin` for diagnostics and tracing.
6. Make shutdown consume or permanently mark the manager so the library is not unloaded with live async work.

### Phase 4: Fix Host Lifecycle Ordering

1. Introduce a small mount/session owner that centralizes cleanup and preserves original errors when cleanup also fails.
2. Quiesce or unmount FUSE before final plugin runtime teardown.
3. Keep lazy unmount fallback but avoid string matching when a typed/OS error path is available.
4. Ensure mountpoint cleanup rules are tested for created/existing/stale mountpoints.

### Phase 5: Standardize Plugin Structure

1. Document plugin crate layout and lifecycle in `plugins/api` docs or a plugin author guide.
2. Add shared helpers for typed config parsing and provider capability registration.
3. Add a shared task group/shutdown helper for runtime-backed plugins.
4. Document plugin filesystem namespace conventions, including the planned D-Bus method/property directory layout.

### Phase 6: CLI And Tracing Cleanup

1. Move CLI parsing to a small tested parser or `clap`.
2. Consider making `watch` a subcommand instead of a global `--watch` mode.
3. Add tests for `PerfettoTraceConfig::from_env`, plugin field visitor behavior, and trace config generation.
4. Avoid panics in tracing layer callbacks where an absent span can be ignored (`bin/src/perfetto.rs:71`).

## Test Plan

### Current Coverage Observed

- `bin/src/config/test.rs` covers parsing plugin sections and tilde expansion (`bin/src/config/test.rs:5`, `bin/src/config/test.rs:44`).
- `bin/src/plugin/test.rs` covers library naming, explicit path precedence, recursive TOML merge, and missing plugin load failure (`bin/src/plugin/test.rs:8`, `bin/src/plugin/test.rs:20`, `bin/src/plugin/test.rs:35`, `bin/src/plugin/test.rs:58`).
- `plugins/api` has no tests.

### Tests To Add During Refactor

1. Plugin ABI/version metadata:
   - Valid metadata loads.
   - Mismatched API/ABI version fails before trait-object construction.
   - Null initializer fails with plugin id/path context.
2. Plugin manager lifecycle:
   - Enabled plugins load in deterministic order.
   - Failure rolls back already-loaded handles and reports all shutdown failures.
   - Library outlives plugin object and handle until shutdown completes.
3. Runtime/shutdown:
   - Task group aborts and awaits all tasks.
   - Shutdown errors are surfaced.
   - Plugin runtime shutdown timeout behavior is deterministic.
4. Config:
   - Non-table plugin config is rejected cleanly.
   - Plugin config parse errors include plugin id and source path.
   - `plugin_dirs`, explicit `library`, executable directory, and default local plugin dir search order are tested.
   - Tilde expansion behavior is explicit for global paths and plugin-owned paths.
5. Host lifecycle:
   - Mount failure cleans created mountpoints.
   - Config/plugin load failure preserves original error and records cleanup failure if both happen.
   - Shutdown order prevents provider calls after plugin runtime teardown.
6. CLI/watch:
   - CLI parser handles `--config`, duplicate options, missing args, `--watch`, and help.
   - Watch mode behavior for directory vs non-directory paths is tested with injectable output.
7. Perfetto/tracing:
   - Env var config parsing.
   - Plugin field extraction for string/debug fields.
   - Track id/name generation.
   - No panics when span lookup returns none.
8. Whole-project verification after implementation:
   - `cargo test`
   - `cargo clippy --all-targets --all-features`
   - `cargo fmt --check`

## Open Questions For Coordinator Arbitration

- Is dynamic plugin loading intended only for plugins built together with the same workspace/toolchain, or should independent third-party plugin binaries be supported?
- Should plugins own their own Tokio runtime, use a host-provided runtime/spawner, or move to a process boundary for stronger isolation?
- Should `PluginContext` expose `DynamicGraph`, or should plugins register providers through a narrower host-owned registrar?
- Should plugin config defaults be host-visible for config generation, or should each plugin own all defaulting through typed serde config?
- Should `PluginHandle::shutdown` be fallible, and should one plugin shutdown failure fail the overall unmount command or only be logged?
- Should FUSE unmount happen before plugin shutdown, or is a two-phase plugin lifecycle needed?
- Should plugin manifests declare node kinds/path namespaces/capabilities before registration?
- Is `watch` a stable public CLI mode that should become a subcommand with documented output contracts?
- Should host logic move into a reusable library crate so other binaries/tests can use plugin loading without depending on `main.rs` internals?
- What security model is assumed for plugin configs and library paths: trusted local config only, or should path validation/permissions/signatures be considered?
- Should Perfetto remain an always-compiled binary dependency, or should tracing backends become feature-gated?
