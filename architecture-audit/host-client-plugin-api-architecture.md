# Host, Watch, and Plugin API Architecture Audit

Scope: `locusfs/`, `locusfs-watch/`, `plugins/api/` (`locusfs-plugin-api`), and the plugin API call sites needed to evaluate API shape and dependency direction. This is a read-only architecture review; no source fixes were made.

## Findings

### 1. The dynamic plugin boundary is a Rust trait-object ABI, not a stable plugin ABI

Observation: the host loads `_locusfs_plugin_init` from a dynamic library and treats it as `unsafe extern "C" fn() -> *mut dyn LocusFsPlugin` in `locusfs/src/plugin/mod.rs:16-17`, then reconstructs the plugin with `Box::from_raw` in `locusfs/src/plugin/mod.rs:80-90`. Each sampled plugin exports the same `extern "C"` symbol returning `*mut dyn LocusFsPlugin`, with `#[allow(improper_ctypes_definitions)]` in `plugins/project/src/lib.rs:88-91`, `plugins/niri/src/lib.rs:115-118`, `plugins/dbus/src/lib.rs:106-109`, and `plugins/pipewire/src/lib.rs:108-111`. The plugin crates are built as both `rlib` and `cdylib` in `plugins/project/Cargo.toml:6-7`, `plugins/niri/Cargo.toml:6-7`, `plugins/dbus/Cargo.toml:6-7`, and `plugins/pipewire/Cargo.toml:6-7`.

Recommendation: either make plugins an explicitly same-build Rust extension mechanism, or define a real ABI boundary: a C-compatible vtable/handle, a versioned symbol, and explicit ownership/destructor functions. The current shape looks like a C ABI because of `extern "C"`, but the actual contract depends on Rust compiler/layout compatibility and shared crate versions.

### 2. `plugins/api` (`locusfs-plugin-api`) leaks graph implementation and Tokio runtime ownership into every plugin

Observation: `PluginContext` exposes `DynamicGraph` and `tokio::runtime::Handle` as public fields in `plugins/api/src/lib.rs:18-22`. The API crate depends directly on `locusfs-graph` and `tokio` in `plugins/api/Cargo.toml:7-10`, and plugin implementations also depend directly on `locusfs-graph` plus `locusfs-plugin-api` in their manifests, for example `plugins/project/Cargo.toml:10-14` and `plugins/dbus/Cargo.toml:10-16`. Plugins use the context graph/runtime directly at registration time, such as `plugins/dbus/src/lib.rs:99-102` and `plugins/niri/src/lib.rs:108-110`.

Recommendation: consider making the plugin API expose a smaller host capability trait, such as a provider registry plus task-spawn/runtime services, instead of the full `DynamicGraph` implementation. That would let plugin authors depend on a contract crate rather than the graph runtime implementation and would make host/plugin dependency direction clearer.

### 3. Plugin configuration ownership is split between host raw TOML and plugin-local typed parsing

Observation: host config stores per-plugin settings as raw `toml::Value` in `locusfs/src/config/mod.rs:22-30`. The host merges `plugin.default_config()` with user overrides in `locusfs/src/plugin/mod.rs:99`, then passes the merged raw TOML into `register` in `locusfs/src/plugin/mod.rs:100-102`. The public plugin trait requires `default_config() -> toml::Value` and `register(..., config: toml::Value)` in `plugins/api/src/lib.rs:45-53`. Each plugin then parses the raw value into its own typed config at the edge, for example `plugins/project/src/lib.rs:76-84`, `plugins/dbus/src/lib.rs:94-102`, and `plugins/pipewire/src/lib.rs:96-104`.

Recommendation: keep plugin-specific config ownership in plugins, but move the repeated raw-TOML edge into a helper or trait pattern in `plugins/api` (`locusfs-plugin-api`). A versioned config schema hook, typed deserialization helper, or host-side validation callback would make config errors and defaults part of the API contract instead of a convention repeated per plugin.

### 4. The host lifecycle is manually sequenced in `main.rs`, with cleanup paths duplicated around each startup phase

Observation: `run` prepares the mountpoint, creates it, loads config, loads plugins, mounts FUSE, waits for shutdown, then shuts plugins down and unmounts in `locusfs/src/main.rs:39-79`. Error paths manually call `cleanup_mountpoint` and sometimes `plugins.shutdown()` at each phase in `locusfs/src/main.rs:40-65`. Mountpoint state and cleanup helpers are local to the binary in `locusfs/src/main.rs:97-155`; plugin lifecycle is separately represented by `PluginManager::shutdown` in `locusfs/src/plugin/mod.rs:57-59`.

Recommendation: introduce a host runtime/session object that owns mountpoint state, loaded plugins, and the FUSE mount in acquisition order and tears them down in one place. That would reduce duplicated cleanup logic and make lifecycle ordering a reusable host API instead of a long binary function.

### 5. `locusfs` is both a host implementation crate and a broad facade over implementation crates

Observation: `locusfs/src/lib.rs:3-8` publicly exposes `config`, `plugin`, and re-exports `locusfs_watch`, `locusfs_fuse`, and `locusfs_graph`. The same package also contains the binary host orchestration in `locusfs/src/main.rs:28-79`, private command watch behavior in `locusfs/src/watch.rs:7-18`, and Perfetto composition in `locusfs/src/perfetto.rs:29-279`.

Recommendation: split the public facade from host internals, or narrow the facade to stable user-facing APIs. As written, downstream users of `locusfs` see host config/plugin internals and low-level implementation crates as one surface, which makes future boundary changes more expensive.

### 6. The watch contract now has a typed protocol crate, but snapshot/read policy still needs a clearer boundary

Observation: `locusfs-watch` now owns the typed watch vocabulary and text encode/decode contract in `locusfs-watch/src/protocol.rs`, while async filesystem client helpers live behind the crate's default `client` feature in `locusfs-watch/src/client.rs`. `Watch::open` still discovers a mount by walking upward until it finds a file named `watch`, converts a data path to a leading-slash logical path, writes that path plus a newline to `/watch`, and exposes a read-after-watch policy through helper methods. The host CLI still adds its own directory/file distinction in `locusfs/src/watch.rs:7-18`, using raw watch events for directory output.

Recommendation: the typed `WatchEvent` API is now in place; keep FUSE and external consumers on that shared contract. The remaining cleanup is to make snapshot-vs-event semantics and read retry policy explicit in the client helper layer, so higher-level consumers do not need to infer when to react to `set` payloads versus when to re-read a path.

### 7. Perfetto plugin-track composition depends on untyped tracing field conventions

Observation: plugin load/shutdown spans carry a `plugin` field in `locusfs/src/plugin/mod.rs:38-40` and `locusfs/src/plugin/mod.rs:64-68`. `PluginTrackLayer` discovers plugin identity by scanning span fields named exactly `"plugin"` in `locusfs/src/perfetto.rs:151-176`, then maps those values to Perfetto tracks in `locusfs/src/perfetto.rs:70-148`. Tracing setup is composed directly in the binary entry path in `locusfs/src/main.rs:157-203`.

Recommendation: move observability composition behind a small host tracing module/API and provide a typed helper for plugin-scoped spans. Keeping `"plugin"` as a string convention makes track assignment easy to break when call sites change span fields, and it is not discoverable from the plugin API.

## Lower-Severity Notes

Observation: `PluginManager` stores loaded plugins in a field named `_loaded` in `locusfs/src/plugin/mod.rs:19-22`, but the field is actively used by `loaded_count` and `shutdown` in `locusfs/src/plugin/mod.rs:53-59`. The underscore naming suggests intentionally unused storage even though this is the manager's core ownership.

Recommendation: rename the field when touching this code for lifecycle work; it would make ownership clearer.

Observation: `PluginContext::new` calls `Handle::current()` and can panic outside a Tokio runtime in `plugins/api/src/lib.rs:24-27`, while `PluginContext::try_new` returns a typed graph error in `plugins/api/src/lib.rs:29-34`. The host uses `try_new` in `locusfs/src/plugin/mod.rs:100-102`.

Recommendation: prefer the fallible constructor as the main API and reserve the panicking constructor for tests or remove it. Plugin API construction is a boundary where runtime errors should be explicit.
