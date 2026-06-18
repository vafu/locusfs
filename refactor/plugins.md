# Plugin Notes

## Current Role

`dbus`, `niri`, and `pipewire` expose read-only external state through graph providers.

`project` exposes writable in-memory project nodes and relations.

Each plugin currently follows a local shape:

- `config/mod.rs`
- `lib.rs`
- `provider.rs`
- `runtime.rs` for external event sources
- `state.rs`

## Findings

- Addressed 2026-06-18: D-Bus object node IDs now round-trip for object paths outside `object_manager_path`.
- Addressed 2026-06-18: D-Bus snapshot errors no longer collapse live object state to empty through `unwrap_or_default`.
- Addressed 2026-06-18: D-Bus and Niri runtimes reconnect after stream/socket failure.
- D-Bus leaks dynamic validation error strings via `Box::leak`.
- D-Bus realtime tests live in production runtime code and depend on local system services.
- D-Bus and Niri change emission often uses changed events where added/removed semantics exist.
- PipeWire source filtering may include pseudo/monitor sources when expected properties are absent.
- `ProjectConfig::state_path` is public but unused.
- Provider adapter files are structurally identical across read-only plugins.
- Registration loops and small helper functions repeat across plugins.

## Refactor Plan

1. Done: fix D-Bus object ID round-tripping for outside paths.
2. Done: make transient D-Bus snapshot failures non-destructive; preserve previous state unless absence is confirmed.
3. Done: add reconnect/backoff loops for D-Bus and Niri runtimes.
4. Replace leaked static error strings with owned error messages or a different error shape.
5. Emit lifecycle-specific graph changes where state diff detects add/remove.
6. Tighten PipeWire source filtering using fixture-based `pactl` JSON tests.
7. Remove `ProjectConfig::state_path` until persistence exists, or implement it.
8. After behavior is stable, extract a shared read-only snapshot provider adapter and plugin registration helper.

## Tests And Verification

- `cargo test -p locusfs-plugin-dbus`
- `cargo test -p locusfs-plugin-niri`
- `cargo test -p locusfs-plugin-pipewire`
- `cargo test -p locusfs-plugin-project`
- Add fixture tests for D-Bus object path mapping and PipeWire JSON source filtering.
