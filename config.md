# Configurable Plugin Loading Plan

## Target Shape

`locusfs` should not know concrete plugins at compile time.

Runtime flow:

```text
locusfs config
    -> plugin manager
        -> loads enabled plugin .so files
        -> asks each plugin for default config / manifest
        -> merges user config
        -> calls plugin.register(graph, plugin_config)
    -> mount graph
```

Manual `mkdir /foo` stays as an ad hoc writable in-memory kind feature. `/project` will normally exist because the `project` plugin is enabled.

## Config Layout

Add:

```text
locusfs/src/config/mod.rs
plugins/dbus/src/config/mod.rs
plugins/niri/src/config/mod.rs
plugins/project/src/config/mod.rs
```

Example config:

```toml
plugin_dirs = ["./target/debug", "~/.local/lib/locusfs/plugins"]

[plugins.project]
enabled = true

[plugins.project.config]
# later:
# persistence = "json"
# state_path = "~/.local/state/locusfs/project.toml"

[plugins.niri]
enabled = true

[plugins.dbus]
enabled = true

[plugins.dbus.config]
services = [
  { name = "org.freedesktop.UPower", bus = "system", local_id = "upower", object_manager_path = "/org/freedesktop/UPower" }
]
```

Important: `dbus` default config should not include UPower. If no services are configured, it can register empty `dbus-service` / `dbus-object` providers or no-op cleanly.

## Plugin API

Add a shared API crate:

```text
locusfs-plugin-api/
  src/lib.rs
```

Both `locusfs` and plugins depend on it. It depends on `locusfs-graph`.

Sketch:

```rust
pub struct PluginManifest {
    pub id: &'static str,
    pub name: &'static str,
    pub version: &'static str,
}

pub struct PluginContext {
    pub graph: DynamicGraph,
}

#[async_trait]
pub trait LocusFsPlugin: Send + Sync {
    fn manifest(&self) -> PluginManifest;
    fn default_config(&self) -> toml::Value;
    async fn register(
        &self,
        context: PluginContext,
        config: toml::Value,
    ) -> locusfs_graph::Result<Box<dyn PluginHandle>>;
}

pub trait PluginHandle: Send + Sync {}
```

Plugin configs are typed inside each plugin:

```rust
// plugins/dbus/src/config/mod.rs
#[derive(Debug, Clone, Deserialize)]
pub struct DbusConfig {
    #[serde(default)]
    pub services: Vec<ServiceConfig>,
}
```

The API passes `toml::Value` so the host does not need plugin-specific types.

## Dynamic Loading

Follow `../de/rsynapse`:

Plugin exports:

```rust
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _locusfs_plugin_init() -> *mut dyn LocusFsPlugin {
    Box::into_raw(Box::new(DbusPlugin))
}
```

Host uses `libloading`:

```rust
type Init = unsafe extern "C" fn() -> *mut dyn LocusFsPlugin;

let lib = Library::new(path)?;
let init: Symbol<Init> = lib.get(b"_locusfs_plugin_init")?;
let plugin = Box::from_raw(init());
```

Keep `Library` alive for the whole mount lifetime:

```rust
struct LoadedPlugin {
    plugin: Box<dyn LocusFsPlugin>,
    handle: Box<dyn PluginHandle>,
    _library: Library,
}
```

Caveat: this Rust trait-object-over-`.so` pattern is not a stable ABI. It is acceptable if plugins are built with the same workspace/toolchain, like rsynapse. If external binary compatibility becomes important, switch to `abi_stable` or a C vtable ABI.

## Plugin Crate Shape

Each plugin becomes `cdylib`:

```toml
[lib]
crate-type = ["rlib", "cdylib"]
```

DBus:

```text
plugins/dbus/src/
  lib.rs
  config/mod.rs
  provider.rs
  runtime.rs
  state.rs
```

`lib.rs` owns only plugin entrypoint and trait impl.

Project:

```text
plugins/project/src/
  lib.rs
  config/mod.rs
  provider.rs?   # later, once persistence exists
```

For now project config can be:

```rust
#[derive(Debug, Clone, Deserialize, Default)]
pub struct ProjectConfig {
    pub state_path: Option<PathBuf>,
}
```

Implementation still uses `InMemoryProvider`.

## Main Binary

`locusfs/src/main.rs` should stop importing concrete plugin crates.

Replace:

```rust
locusfs_plugin_dbus::register(...)
locusfs_plugin_niri::register(...)
locusfs_plugin_project::register(...)
```

with:

```rust
let config = Config::load(...)?;
let plugins = PluginManager::load_enabled(&config, graph.clone()).await?;
```

`PluginManager` lives under:

```text
locusfs/src/plugin/mod.rs
locusfs/src/plugin/loader.rs
```

## Config Composition

Process:

1. Load generic `locusfs` config.
2. For each `[plugins.<id>]` with `enabled = true`:
   - resolve `.so` path
   - load plugin
   - get `default_config()`
   - merge user `[plugins.<id>.config]` over defaults
   - call `plugin.register(context, merged_config)`
3. Store all loaded plugin handles until unmount.

Resolution:

```text
explicit library path if configured
else plugin_dirs/liblocusfs_plugin_<id>.so
```

## DBus Refactor

Move this out of hardcoded runtime:

```rust
default_service_configs()
```

Replace with:

```rust
DbusRuntime::start(graph, DbusConfig)
```

`DbusConfig.services` drives watchers.

Behavior:

- configured services create `dbus-service` nodes
- no services means no service nodes
- UPower appears only if config asks for it

## Implementation Order

1. Add `locusfs-plugin-api`.
2. Add `locusfs/src/config/mod.rs`.
3. Add `locusfs/src/plugin/mod.rs` loader using `libloading`.
4. Convert project plugin to API + `cdylib`.
5. Convert dbus plugin config and remove hardcoded UPower.
6. Convert niri plugin.
7. Remove direct plugin dependencies from `locusfs/Cargo.toml`.
8. Add config examples and tests.

## Tests

Minimum coverage:

- parse config with enabled/disabled plugins
- resolve plugin library names from `plugin_dirs`
- dbus config deserializes services
- dbus with empty services does not expose UPower
- project plugin loaded dynamically exposes `/project`
- disabled plugin is not loaded
- missing enabled plugin gives clear startup error
