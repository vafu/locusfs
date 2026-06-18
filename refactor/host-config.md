# Host, Config, And Dynamic Loading Notes

## Current Role

`locusfs` loads config, dynamically loads enabled plugin libraries, registers plugins into a `DynamicGraph`, mounts FUSE, handles CLI watch mode, and coordinates shutdown.

## Findings

- The dynamic plugin boundary is a Rust trait object crossing a `.so` boundary. This is not a stable ABI.
- Addressed 2026-06-18: plugin shutdown now uses an async lifecycle method so the host can await task abort completion before eventual library unload.
- Addressed 2026-06-18: enabled plugin load failures now fail startup instead of being logged and skipped.
- Config-relative paths are resolved relative to process cwd, not the config file location.
- `normalize_path` in plugin search dedup currently just clones the path, so the helper name is misleading.
- `PluginManifest` has no API compatibility/version guard.
- Default Tokio worker count may be larger than the app needs for this workload.

## Refactor Plan

1. Document current plugin compatibility: same workspace, same toolchain, same dependency graph.
2. Add an API compatibility field to `PluginManifest`.
3. Decide long-term ABI strategy: stable C vtable/`abi_stable`, or intentionally same-workspace cdylibs only.
4. Done: replace drop-only plugin handles with explicit async shutdown and joined task completion.
5. Done: keep libraries loaded until all graph providers and plugin tasks are gone.
6. Done: enabled plugin load failure is fail-fast.
7. Resolve relative config paths against the config file directory.
8. Simplify or implement path normalization in plugin search dedup.
9. Consider reducing Tokio worker threads after measuring idle/task load.

## Tests And Verification

- `cargo test -p locusfs`
- Add plugin-manager tests for enabled plugin failure behavior, config-relative paths, manifest compatibility checks, and shutdown ordering.
