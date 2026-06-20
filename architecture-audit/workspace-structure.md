# Workspace Structure Architecture Audit

Scope: repo-level structure, workspace manifests, crate dependency direction, scripts/docs/configuration files, feature leakage, redundant/stale files, and cross-crate organization.

## Findings

### 1. Plugin ABI is exposed as a Rust trait object across `cdylib` boundaries

Observation: `plugins/api` packages `locusfs-plugin-api` and defines the public plugin contract as `LocusFsPlugin` and `PluginHandle` trait objects, with plugin registration returning `Result<Box<dyn PluginHandle>>` ([plugins/api/src/lib.rs:41](../plugins/api/src/lib.rs#L41), [plugins/api/src/lib.rs:49](../plugins/api/src/lib.rs#L49), [plugins/api/src/lib.rs:56](../plugins/api/src/lib.rs#L56)). The host loads `_locusfs_plugin_init` as `unsafe extern "C" fn() -> *mut dyn LocusFsPlugin` ([locusfs/src/plugin/mod.rs:16](../locusfs/src/plugin/mod.rs#L16), [locusfs/src/plugin/mod.rs:80](../locusfs/src/plugin/mod.rs#L80)). Each plugin exports the same symbol returning a boxed trait object ([plugins/dbus/src/lib.rs:106](../plugins/dbus/src/lib.rs#L106), [plugins/niri/src/lib.rs:115](../plugins/niri/src/lib.rs#L115), [plugins/pipewire/src/lib.rs:108](../plugins/pipewire/src/lib.rs#L108), [plugins/project/src/lib.rs:88](../plugins/project/src/lib.rs#L88)).

Observation: `config.md` already calls out the caveat that Rust trait objects over `.so` are not a stable ABI and are acceptable only if plugins are built with the same workspace/toolchain ([config.md:143](../config.md#L143)).

Recommendation: If plugins are intended to stay workspace-local and rebuilt together, make that contract explicit in `plugins/api` docs and/or `locusfs-plugin-api` package metadata. If third-party binary plugins are a goal, move this boundary to a C-vtable or `abi_stable`-style ABI before external users depend on the current shape.

### 2. Plugin crates duplicate registration and lifecycle boilerplate

Observation: `plugins/dbus`, `plugins/niri`, and `plugins/pipewire` all repeat the same pattern: public config module, private provider/runtime/state modules, public provider re-export, kind constants, plugin struct, handle struct, `Drop`, `PluginHandle::shutdown`, `register`, `register_with_config`, private runtime-aware registration, `LocusFsPlugin` impl, exported `_locusfs_plugin_init`, and `config_error` ([plugins/dbus/src/lib.rs:3](../plugins/dbus/src/lib.rs#L3), [plugins/dbus/src/lib.rs:23](../plugins/dbus/src/lib.rs#L23), [plugins/dbus/src/lib.rs:40](../plugins/dbus/src/lib.rs#L40), [plugins/dbus/src/lib.rs:50](../plugins/dbus/src/lib.rs#L50), [plugins/dbus/src/lib.rs:84](../plugins/dbus/src/lib.rs#L84); [plugins/niri/src/lib.rs:3](../plugins/niri/src/lib.rs#L3), [plugins/niri/src/lib.rs:27](../plugins/niri/src/lib.rs#L27), [plugins/niri/src/lib.rs:44](../plugins/niri/src/lib.rs#L44), [plugins/niri/src/lib.rs:54](../plugins/niri/src/lib.rs#L54), [plugins/niri/src/lib.rs:93](../plugins/niri/src/lib.rs#L93); [plugins/pipewire/src/lib.rs:3](../plugins/pipewire/src/lib.rs#L3), [plugins/pipewire/src/lib.rs:25](../plugins/pipewire/src/lib.rs#L25), [plugins/pipewire/src/lib.rs:41](../plugins/pipewire/src/lib.rs#L41), [plugins/pipewire/src/lib.rs:51](../plugins/pipewire/src/lib.rs#L51), [plugins/pipewire/src/lib.rs:86](../plugins/pipewire/src/lib.rs#L86)).

Observation: Each plugin manifest declares `crate-type = ["rlib", "cdylib"]` independently ([plugins/dbus/Cargo.toml:6](../plugins/dbus/Cargo.toml#L6), [plugins/niri/Cargo.toml:6](../plugins/niri/Cargo.toml#L6), [plugins/pipewire/Cargo.toml:6](../plugins/pipewire/Cargo.toml#L6), [plugins/project/Cargo.toml:6](../plugins/project/Cargo.toml#L6)).

Recommendation: Add a small helper layer in `plugins/api` (`locusfs-plugin-api`) or a local macro crate only for the repeated, policy-bearing pieces: manifest construction, config parse error mapping, runtime-aware register adapter, and common shutdown handle for task lists. Keep provider-specific graph registration local, because the kinds and mutation capabilities differ.

### 3. The `locusfs` library facade exposes binary-owned configuration and plugin loading as public API

Observation: `locusfs/src/lib.rs` publicly exports `config` and `plugin` modules, and re-exports the watch/FUSE/graph crates ([locusfs/src/lib.rs:3](../locusfs/src/lib.rs#L3), [locusfs/src/lib.rs:6](../locusfs/src/lib.rs#L6)). The binary then imports `Config`, `PluginManager`, `FuseMountConfig`, and `DynamicGraph` through that facade ([locusfs/src/main.rs:6](../locusfs/src/main.rs#L6), [locusfs/src/main.rs:7](../locusfs/src/main.rs#L7), [locusfs/src/main.rs:8](../locusfs/src/main.rs#L8), [locusfs/src/main.rs:9](../locusfs/src/main.rs#L9)).

Observation: `locusfs::plugin` owns dynamic loading details including `libloading`, plugin search paths, filename conventions, TOML merging, and unsafe symbol loading ([locusfs/src/plugin/mod.rs:72](../locusfs/src/plugin/mod.rs#L72), [locusfs/src/plugin/mod.rs:119](../locusfs/src/plugin/mod.rs#L119), [locusfs/src/plugin/mod.rs:140](../locusfs/src/plugin/mod.rs#L140), [locusfs/src/plugin/mod.rs:151](../locusfs/src/plugin/mod.rs#L151), [locusfs/src/plugin/mod.rs:161](../locusfs/src/plugin/mod.rs#L161)).

Recommendation: Decide whether `locusfs` is meant to be a reusable library facade or only the binary crate's support library. If reusable, keep `client`, `fuse`, and `graph` as the clean facade and move dynamic plugin loading behind a narrower host API or into binary-private modules. If not reusable, avoid making `config` and `plugin` public to reduce accidental API surface.

### 4. Workspace manifest centralization is partial, which invites dependency drift

Observation: The root workspace centralizes only a subset of shared dependencies: `async-trait`, `bytes`, `futures-*`, `serde`, `serde_json`, `tokio`, and `toml` ([Cargo.toml:15](../Cargo.toml#L15)). Several repeated dependencies remain versioned per crate, including `libc` in `locusfs`, `locusfs-watch`, and `locusfs-fuse` ([locusfs/Cargo.toml:8](../locusfs/Cargo.toml#L8), [locusfs-watch/Cargo.toml:10](../locusfs-watch/Cargo.toml#L10), [locusfs-fuse/Cargo.toml:12](../locusfs-fuse/Cargo.toml#L12)), `thiserror` in graph and FUSE ([locusfs-graph/Cargo.toml:10](../locusfs-graph/Cargo.toml#L10), [locusfs-fuse/Cargo.toml:14](../locusfs-fuse/Cargo.toml#L14)), and `tracing` in multiple crates ([locusfs/Cargo.toml:17](../locusfs/Cargo.toml#L17), [locusfs-watch/Cargo.toml:12](../locusfs-watch/Cargo.toml#L12), [locusfs-graph/Cargo.toml:12](../locusfs-graph/Cargo.toml#L12), [locusfs-fuse/Cargo.toml:16](../locusfs-fuse/Cargo.toml#L16)).

Observation: `serde_json` is centralized in the workspace ([Cargo.toml:21](../Cargo.toml#L21)), but `plugins/niri` pins it directly as `"1.0.145"` while `plugins/pipewire` uses the workspace dependency ([plugins/niri/Cargo.toml:15](../plugins/niri/Cargo.toml#L15), [plugins/pipewire/Cargo.toml:14](../plugins/pipewire/Cargo.toml#L14)).

Recommendation: Move repeated shared versions into `[workspace.dependencies]` consistently, especially `libc`, `thiserror`, `tracing`, and `serde_json`. This is not urgent architecture debt, but it reduces manifest noise and makes version policy obvious.

### 5. `config.md` is stale plan documentation living at the repo root

Observation: `config.md` is titled as a "Configurable Plugin Loading Plan" and uses future-tense implementation steps such as "Add `locusfs-plugin-api`" and "Add `locusfs/src/config/mod.rs`" ([config.md:1](../config.md#L1), [config.md:23](../config.md#L23), [config.md:61](../config.md#L61), [config.md:257](../config.md#L257)). Those crates/modules now exist in the workspace ([Cargo.toml:7](../Cargo.toml#L7), [locusfs/src/config/mod.rs:1](../locusfs/src/config/mod.rs#L1), [plugins/api/src/lib.rs:1](../plugins/api/src/lib.rs#L1)).

Observation: Some parts still describe live design constraints, such as plugin ABI caveats and dynamic loading flow ([config.md:110](../config.md#L110), [config.md:143](../config.md#L143), [config.md:213](../config.md#L213)).

Recommendation: Split this into either archived implementation notes under an explicit archive path, or convert the still-current parts into a short user/admin configuration reference. Keeping a completed plan at the root makes it harder to tell whether it is normative documentation.

### 6. `scripts/proj` appears coupled to older project naming and FUSE layout assumptions

Observation: `scripts/proj` defaults `mountpoint` to `${LOCUSFS_MOUNT:-${LOCUS_MOUNT:-/tmp/rsynapse}}`, so the final default still names `rsynapse` rather than `locusfs` ([scripts/proj:7](../scripts/proj#L7)).

Observation: The script writes directly into FUSE paths such as `$mountpoint/project/<encoded-root>` and `$mountpoint/workspace/$workspace/project` ([scripts/proj:85](../scripts/proj#L85), [scripts/proj:94](../scripts/proj#L94), [scripts/proj:111](../scripts/proj#L111), [scripts/proj:279](../scripts/proj#L279)). That duplicates part of the public layout contract already owned by `locusfs-fuse`, whose crate docs say it owns the public filesystem layout ([locusfs-fuse/src/lib.rs:1](../locusfs-fuse/src/lib.rs#L1), [locusfs-fuse/src/lib.rs:15](../locusfs-fuse/src/lib.rs#L15)).

Recommendation: Treat the script as either an intentionally external integration test/client or migrate its path construction onto a small CLI/client helper that consumes the same layout API as Rust callers. At minimum, update the stale default mount path and document required external tools (`jq`, `git`, `realpath`) near the script.

### 7. Public plugin crate APIs leak implementation providers

Observation: Plugin crates publicly re-export provider structs (`DbusProvider`, `NiriProvider`, `PipeWireProvider`) while keeping runtime and state private ([plugins/dbus/src/lib.rs:3](../plugins/dbus/src/lib.rs#L3), [plugins/dbus/src/lib.rs:8](../plugins/dbus/src/lib.rs#L8), [plugins/niri/src/lib.rs:3](../plugins/niri/src/lib.rs#L3), [plugins/niri/src/lib.rs:8](../plugins/niri/src/lib.rs#L8), [plugins/pipewire/src/lib.rs:3](../plugins/pipewire/src/lib.rs#L3), [plugins/pipewire/src/lib.rs:8](../plugins/pipewire/src/lib.rs#L8)). Their manifests build both `rlib` and `cdylib`, so those provider types are part of the Rust library surface as well as implementation details of the dynamic plugin.

Recommendation: If direct in-process registration is a supported testing or embedding mode, keep a small public surface such as `register`/`register_with_config` and the typed config. Otherwise, make provider structs private to avoid coupling consumers to plugin internals. If the `rlib` target exists only for tests, consider whether integration tests can use crate-internal unit tests instead.

### 8. Dependency direction is mostly clean, but the graph crate is both contract and implementation

Observation: The workspace dependency direction is acyclic and broadly coherent: `locusfs-graph` has no workspace-crate dependencies; `locusfs-fuse` depends on graph and protocol-only `locusfs-watch`; `plugins/api` (`locusfs-plugin-api`) depends on graph; plugins depend on graph plus plugin API; the top-level `locusfs` crate depends on watch, graph, FUSE, and plugin API ([locusfs-graph/Cargo.toml:6](../locusfs-graph/Cargo.toml#L6), [locusfs-fuse/Cargo.toml:13](../locusfs-fuse/Cargo.toml#L13), [plugins/api/Cargo.toml:8](../plugins/api/Cargo.toml#L8), [plugins/dbus/Cargo.toml:12](../plugins/dbus/Cargo.toml#L12), [locusfs/Cargo.toml:9](../locusfs/Cargo.toml#L9)).

Observation: `locusfs-graph` exposes provider traits, routing, identity/value types, watch types, tracing wrapper, and `InMemoryProvider` from a single crate root ([locusfs-graph/src/lib.rs:11](../locusfs-graph/src/lib.rs#L11), [locusfs-graph/src/lib.rs:12](../locusfs-graph/src/lib.rs#L12), [locusfs-graph/src/lib.rs:18](../locusfs-graph/src/lib.rs#L18), [locusfs-graph/src/lib.rs:19](../locusfs-graph/src/lib.rs#L19)). `DynamicGraph` stores provider registries and overlay implementation details in the same module as public subscription types ([locusfs-graph/src/graph/dynamic.rs:20](../locusfs-graph/src/graph/dynamic.rs#L20), [locusfs-graph/src/graph/dynamic.rs:30](../locusfs-graph/src/graph/dynamic.rs#L30), [locusfs-graph/src/graph/dynamic.rs:70](../locusfs-graph/src/graph/dynamic.rs#L70)).

Recommendation: No immediate split is required, but this crate is the central API stability point. If alternate graph implementations or non-host consumers grow, consider separating pure contracts/types from in-memory/dynamic routing implementations, or at least keep implementation helpers out of the root prelude-like re-export list.

## Lower-risk observations

- The root `.gitignore` ignores only `target/` ([.gitignore:1](../.gitignore#L1)). The untracked `.project.json` file is project-local metadata ([.project.json:1](../.project.json#L1)) and may be intentional, but the repo has no explicit policy for local project metadata.
- `Cargo.toml` lists all workspace members explicitly ([Cargo.toml:2](../Cargo.toml#L2)). This is clear for a small workspace. If more plugins are expected, a `plugins/*` member pattern would reduce manifest churn, but explicit membership is safer when plugin directories may contain experiments.
- The root package metadata is minimal: crates do not declare descriptions, licenses, repository URLs, or readme paths in their manifests. This is fine for private iteration, but it weakens the public API boundary if crates are intended to be consumed independently.

## Summary

The workspace has a sensible high-level dependency direction and uses separate crates for graph contracts, FUSE, shared watch protocol/client helpers, plugin API under `plugins/api`, concrete plugins, and the host binary. The main architecture risks are the unstable Rust trait-object plugin ABI, repeated plugin crate boilerplate, public exposure of binary/plugin-loader internals through the `locusfs` facade, stale root documentation, and partial manifest centralization.
