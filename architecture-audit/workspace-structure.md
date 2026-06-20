# Workspace Structure Architecture Audit

Scope: repo-level structure, workspace manifests, crate dependency direction, scripts/docs/configuration files, feature leakage, redundant/stale files, and cross-crate organization.

## Findings

### 1. Plugin ABI is exposed as a Rust trait object across `cdylib` boundaries

Observation: `plugins/api` packages `locusfs-plugin-api` and defines the public plugin contract as `LocusFsPlugin` and `PluginHandle` trait objects, with plugin registration returning `Result<Box<dyn PluginHandle>>` ([plugins/api/src/lib.rs:41](../plugins/api/src/lib.rs#L41), [plugins/api/src/lib.rs:49](../plugins/api/src/lib.rs#L49), [plugins/api/src/lib.rs:56](../plugins/api/src/lib.rs#L56)). The host loads `_locusfs_plugin_init` as `unsafe extern "C" fn() -> *mut dyn LocusFsPlugin` ([bin/src/plugin/mod.rs:16](../bin/src/plugin/mod.rs#L16), [bin/src/plugin/mod.rs:80](../bin/src/plugin/mod.rs#L80)). Each plugin exports the same symbol returning a boxed trait object ([plugins/dbus/src/lib.rs:106](../plugins/dbus/src/lib.rs#L106), [plugins/niri/src/lib.rs:115](../plugins/niri/src/lib.rs#L115), [plugins/pipewire/src/lib.rs:108](../plugins/pipewire/src/lib.rs#L108), [plugins/project/src/lib.rs:88](../plugins/project/src/lib.rs#L88)).

Observation: `plugins/api` documents that the Rust trait-object-over-`.so` plugin boundary is a same-workspace/toolchain extension contract rather than a stable cross-compiler binary ABI ([plugins/api/src/lib.rs:3](../plugins/api/src/lib.rs#L3)).

Recommendation: If plugins are intended to stay workspace-local and rebuilt together, make that contract explicit in `plugins/api` docs and/or `locusfs-plugin-api` package metadata. If third-party binary plugins are a goal, move this boundary to a C-vtable or `abi_stable`-style ABI before external users depend on the current shape.

### 2. Plugin crates duplicate registration and lifecycle boilerplate

Observation: `plugins/dbus`, `plugins/niri`, and `plugins/pipewire` all repeat the same pattern: public config module, private provider/runtime/state modules, public provider re-export, kind constants, plugin struct, handle struct, `Drop`, `PluginHandle::shutdown`, `register`, `register_with_config`, private runtime-aware registration, `LocusFsPlugin` impl, exported `_locusfs_plugin_init`, and `config_error` ([plugins/dbus/src/lib.rs:3](../plugins/dbus/src/lib.rs#L3), [plugins/dbus/src/lib.rs:23](../plugins/dbus/src/lib.rs#L23), [plugins/dbus/src/lib.rs:40](../plugins/dbus/src/lib.rs#L40), [plugins/dbus/src/lib.rs:50](../plugins/dbus/src/lib.rs#L50), [plugins/dbus/src/lib.rs:84](../plugins/dbus/src/lib.rs#L84); [plugins/niri/src/lib.rs:3](../plugins/niri/src/lib.rs#L3), [plugins/niri/src/lib.rs:27](../plugins/niri/src/lib.rs#L27), [plugins/niri/src/lib.rs:44](../plugins/niri/src/lib.rs#L44), [plugins/niri/src/lib.rs:54](../plugins/niri/src/lib.rs#L54), [plugins/niri/src/lib.rs:93](../plugins/niri/src/lib.rs#L93); [plugins/pipewire/src/lib.rs:3](../plugins/pipewire/src/lib.rs#L3), [plugins/pipewire/src/lib.rs:25](../plugins/pipewire/src/lib.rs#L25), [plugins/pipewire/src/lib.rs:41](../plugins/pipewire/src/lib.rs#L41), [plugins/pipewire/src/lib.rs:51](../plugins/pipewire/src/lib.rs#L51), [plugins/pipewire/src/lib.rs:86](../plugins/pipewire/src/lib.rs#L86)).

Observation: Each plugin manifest declares `crate-type = ["rlib", "cdylib"]` independently ([plugins/dbus/Cargo.toml:6](../plugins/dbus/Cargo.toml#L6), [plugins/niri/Cargo.toml:6](../plugins/niri/Cargo.toml#L6), [plugins/pipewire/Cargo.toml:6](../plugins/pipewire/Cargo.toml#L6), [plugins/project/Cargo.toml:6](../plugins/project/Cargo.toml#L6)).

Recommendation: Add a small helper layer in `plugins/api` (`locusfs-plugin-api`) or a local macro crate only for the repeated, policy-bearing pieces: manifest construction, config parse error mapping, runtime-aware register adapter, and common shutdown handle for task lists. Keep provider-specific graph registration local, because the kinds and mutation capabilities differ.

### 3. The host package is now binary-scoped; keep host internals private

Observation: the executable remains named `locusfs`, but the package is now `locusfs-bin` under `bin/`. The old library facade was removed; `bin/src/main.rs` imports host-local modules with `mod config`, `mod plugin`, `mod watch`, and `mod perfetto`, while depending directly on stable public crates for FUSE, graph, and watch APIs ([bin/src/main.rs:1](../bin/src/main.rs#L1), [bin/src/main.rs:13](../bin/src/main.rs#L13), [bin/src/main.rs:15](../bin/src/main.rs#L15), [bin/Cargo.toml:1](../bin/Cargo.toml#L1)).

Observation: `bin::plugin` still owns dynamic loading details including `libloading`, plugin search paths, filename conventions, TOML merging, and unsafe symbol loading ([bin/src/plugin/mod.rs:72](../bin/src/plugin/mod.rs#L72), [bin/src/plugin/mod.rs:119](../bin/src/plugin/mod.rs#L119), [bin/src/plugin/mod.rs:140](../bin/src/plugin/mod.rs#L140), [bin/src/plugin/mod.rs:151](../bin/src/plugin/mod.rs#L151), [bin/src/plugin/mod.rs:161](../bin/src/plugin/mod.rs#L161)).

Recommendation: Keep `locusfs-bin` as the private host package unless a real embedding API appears. External consumers should depend on the public crates (`locusfs-watch`, `locusfs-graph`, `locusfs-fuse`, and `locusfs-plugin-api`) rather than the binary package.

### 4. Workspace manifest centralization is partial, which invites dependency drift

Observation: The root workspace centralizes only a subset of shared dependencies: `async-trait`, `bytes`, `futures-*`, `serde`, `serde_json`, `tokio`, and `toml` ([Cargo.toml:15](../Cargo.toml#L15)). Several repeated dependencies remain versioned per crate, including `libc` in `locusfs`, `locusfs-watch`, and `locusfs-fuse` ([bin/Cargo.toml:8](../bin/Cargo.toml#L8), [watch/Cargo.toml:10](../watch/Cargo.toml#L10), [fuse/Cargo.toml:12](../fuse/Cargo.toml#L12)), `thiserror` in graph and FUSE ([graph/Cargo.toml:10](../graph/Cargo.toml#L10), [fuse/Cargo.toml:14](../fuse/Cargo.toml#L14)), and `tracing` in multiple crates ([bin/Cargo.toml:17](../bin/Cargo.toml#L17), [watch/Cargo.toml:12](../watch/Cargo.toml#L12), [graph/Cargo.toml:12](../graph/Cargo.toml#L12), [fuse/Cargo.toml:16](../fuse/Cargo.toml#L16)).

Observation: `serde_json` is centralized in the workspace ([Cargo.toml:21](../Cargo.toml#L21)), but `plugins/niri` pins it directly as `"1.0.145"` while `plugins/pipewire` uses the workspace dependency ([plugins/niri/Cargo.toml:15](../plugins/niri/Cargo.toml#L15), [plugins/pipewire/Cargo.toml:14](../plugins/pipewire/Cargo.toml#L14)).

Recommendation: Move repeated shared versions into `[workspace.dependencies]` consistently, especially `libc`, `thiserror`, `tracing`, and `serde_json`. This is not urgent architecture debt, but it reduces manifest noise and makes version policy obvious.

### 5. Project helper script is plugin-specific but still duplicates FUSE layout strings

Observation: the project helper script now lives with the project plugin at `plugins/project/scripts/proj`, and its default mountpoint uses `/tmp/locusfs` through `${LOCUSFS_MOUNT:-${LOCUS_MOUNT:-/tmp/locusfs}}` ([plugins/project/scripts/proj:7](../plugins/project/scripts/proj#L7)).

Observation: The script writes directly into FUSE paths such as `$mountpoint/project/<encoded-root>` and `$mountpoint/workspace/$workspace/project` ([plugins/project/scripts/proj:85](../plugins/project/scripts/proj#L85), [plugins/project/scripts/proj:94](../plugins/project/scripts/proj#L94), [plugins/project/scripts/proj:111](../plugins/project/scripts/proj#L111), [plugins/project/scripts/proj:279](../plugins/project/scripts/proj#L279)). That duplicates part of the public layout contract already owned by `locusfs-fuse`, whose crate docs say it owns the public filesystem layout ([fuse/src/lib.rs:1](../fuse/src/lib.rs#L1), [fuse/src/lib.rs:15](../fuse/src/lib.rs#L15)).

Recommendation: Treat the script as a project-plugin support tool. If it becomes a supported user command, migrate its path construction onto a small CLI/client helper that consumes the same layout API as Rust callers.

### 6. Public plugin crate APIs leak implementation providers

Observation: Plugin crates publicly re-export provider structs (`DbusProvider`, `NiriProvider`, `PipeWireProvider`) while keeping runtime and state private ([plugins/dbus/src/lib.rs:3](../plugins/dbus/src/lib.rs#L3), [plugins/dbus/src/lib.rs:8](../plugins/dbus/src/lib.rs#L8), [plugins/niri/src/lib.rs:3](../plugins/niri/src/lib.rs#L3), [plugins/niri/src/lib.rs:8](../plugins/niri/src/lib.rs#L8), [plugins/pipewire/src/lib.rs:3](../plugins/pipewire/src/lib.rs#L3), [plugins/pipewire/src/lib.rs:8](../plugins/pipewire/src/lib.rs#L8)). Their manifests build both `rlib` and `cdylib`, so those provider types are part of the Rust library surface as well as implementation details of the dynamic plugin.

Recommendation: If direct in-process registration is a supported testing or embedding mode, keep a small public surface such as `register`/`register_with_config` and the typed config. Otherwise, make provider structs private to avoid coupling consumers to plugin internals. If the `rlib` target exists only for tests, consider whether integration tests can use crate-internal unit tests instead.

### 7. Dependency direction is mostly clean, but the graph crate is both contract and implementation

Observation: The workspace dependency direction is acyclic and broadly coherent: `locusfs-graph` has no workspace-crate dependencies; `locusfs-fuse` depends on graph and protocol-only `locusfs-watch`; `plugins/api` (`locusfs-plugin-api`) depends on graph; plugins depend on graph plus plugin API; the `locusfs-bin` host package depends on watch, graph, FUSE, and plugin API ([graph/Cargo.toml:6](../graph/Cargo.toml#L6), [fuse/Cargo.toml:13](../fuse/Cargo.toml#L13), [plugins/api/Cargo.toml:8](../plugins/api/Cargo.toml#L8), [plugins/dbus/Cargo.toml:12](../plugins/dbus/Cargo.toml#L12), [bin/Cargo.toml:9](../bin/Cargo.toml#L9)).

Observation: `locusfs-graph` exposes provider traits, routing, identity/value types, watch types, tracing wrapper, and `InMemoryProvider` from a single crate root ([graph/src/lib.rs:11](../graph/src/lib.rs#L11), [graph/src/lib.rs:12](../graph/src/lib.rs#L12), [graph/src/lib.rs:18](../graph/src/lib.rs#L18), [graph/src/lib.rs:19](../graph/src/lib.rs#L19)). `DynamicGraph` stores provider registries and overlay implementation details in the same module as public subscription types ([graph/src/graph/dynamic.rs:20](../graph/src/graph/dynamic.rs#L20), [graph/src/graph/dynamic.rs:30](../graph/src/graph/dynamic.rs#L30), [graph/src/graph/dynamic.rs:70](../graph/src/graph/dynamic.rs#L70)).

Recommendation: No immediate split is required, but this crate is the central API stability point. If alternate graph implementations or non-host consumers grow, consider separating pure contracts/types from in-memory/dynamic routing implementations, or at least keep implementation helpers out of the root prelude-like re-export list.

## Lower-risk observations

- The root `.gitignore` ignores only `target/` ([.gitignore:1](../.gitignore#L1)). The untracked `.project.json` file is project-local metadata ([.project.json:1](../.project.json#L1)) and may be intentional, but the repo has no explicit policy for local project metadata.
- `Cargo.toml` lists all workspace members explicitly ([Cargo.toml:2](../Cargo.toml#L2)). This is clear for a small workspace. If more plugins are expected, a `plugins/*` member pattern would reduce manifest churn, but explicit membership is safer when plugin directories may contain experiments.
- The root package metadata is minimal: crates do not declare descriptions, licenses, repository URLs, or readme paths in their manifests. This is fine for private iteration, but it weakens the public API boundary if crates are intended to be consumed independently.

## Summary

The workspace has a sensible high-level dependency direction and uses separate crates for graph contracts, FUSE, shared watch protocol/client helpers, plugin API under `plugins/api`, concrete plugins, and the `locusfs-bin` host binary. The main architecture risks are the unstable Rust trait-object plugin ABI, repeated plugin crate boilerplate, project-script layout coupling, and partial manifest centralization.
