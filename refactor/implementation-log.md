# Locusfs Refactor Implementation Log

## Milestones

- 2026-06-25T20:54:19-07:00 - Started coordinator arbitration after all six subagent reports were available.
- 2026-06-25T20:54:19-07:00 - Chose a scoped implementation pass: graph feature gate and registration helpers, watch client framing, FUSE watch readiness/resync, and generic D-Bus path layout.
- 2026-06-25T20:54:19-07:00 - Deferred plugin ABI/runtime ownership changes because they require a larger compatibility decision.
- 2026-06-25T21:03:19-07:00 - Implemented the scoped refactors and ran focused and workspace validation.
- 2026-06-26T10:09:48-07:00 - Started second-pass D-Bus layout update after deciding that `_absolute`, service-local public roots, and separate `objects`/`methods` directories duplicated too much D-Bus structure.
- 2026-06-26T10:19:11-07:00 - Rebuilt release artifacts, reinstalled `locusfs` and all plugins, restarted `locusfs.service`, and verified the live mount exposes `/dbus/system/<actual/dbus/path>` plus `.call` method files.

## Implementation Decisions

- Keep granular graph provider registration methods and add bundled helpers for common provider shapes.
- Keep the current watch text protocol for now, but make client reads frame-aware.
- Treat `/dbus/system` and `/dbus/session` as public D-Bus bus roots. The visible path below them is the actual D-Bus object path without a leading slash.
- Remove the D-Bus `_absolute`, `objects`, and `methods` projection layers. Method files now use a `.call` suffix beside property files in the owning object directory.
- Keep canonical `interface.member` property names and `interface.Method.call` method files available; expose short names only when unambiguous.
- Keep graph metadata properties on nodes; do not project `kind`, `source`, `path`, or service metadata into D-Bus object property files.
- Refresh all configured state-mode watches on broadcast lag rather than sending generic change events they cannot consume.

## Validation Commands

- `cargo check -p locusfs-plugin-dbus` - passed.
- `cargo check -p locusfs-watch` - passed.
- `cargo check -p locusfs-fuse` - passed.
- `cargo check -p locusfs-graph --no-default-features` - passed.
- `cargo check -p locusfs-graph --no-default-features --features dynamic` - passed.
- `cargo check -p locusfs-graph --no-default-features --features in-memory` - passed.
- `cargo check -p locusfs-graph --no-default-features --features watch-provider` - passed.
- `cargo check -p locusfs-graph --no-default-features --features provider-tracing` - passed.
- `cargo check -p locusfs-graph --no-default-features --features provider-tracing,watch-provider` - passed.
- `cargo check -p locusfs-graph --all-features` - passed.
- `cargo test -p locusfs-graph --all-features` - passed.
- `cargo test -p locusfs-watch` - passed.
- `cargo test -p locusfs-fuse watch` - passed.
- `cargo test -p locusfs-plugin-dbus state` - passed.
- `cargo test -p locusfs-plugin-dbus` - passed after the second-pass bus-native D-Bus layout update.
- `cargo test --workspace` - passed. The real FUSE smoke test remains ignored because it requires host `/dev/fuse` access.
- `cargo clippy --workspace --all-targets` - passed with warnings. Remaining warnings are general cleanup items outside this scoped refactor, including missing safety docs on plugin entrypoints, collapsible conditionals, and a `DynamicGraph::new` without `Default`.
- `cargo build --release --workspace` - passed for reinstall.
- `systemctl --user restart locusfs.service` - completed; service active with `/home/v47/.local/bin/locusfs /run/user/1000/locusfs`.
- Live mount verification: `/run/user/1000/locusfs/dbus/system/org/freedesktop/UPower` exposes properties and `.call` files, and no old service-local roots (`upower`, `bluez`, `agentdbus`, `powerprofiles`, `networkmanager`) are present directly under `/dbus`.

## Questions For User Validation

- Is removing the legacy D-Bus `object`, `@properties`, `@methods`, `@absolute`, interim `objects`/`methods`, and `_absolute` paths acceptable before any downstream compatibility window?
- Should bus roots expose configured service metadata only as comma-separated graph properties for now, or should service config get its own hidden node kind if consumers need per-service metadata later?
- Should short D-Bus property/method aliases remain visible when unique, or should only canonical `interface.member` names be exposed?
- Should watch state wire-format compatibility be broken now to fix leading slash, empty string, and whitespace values cleanly?
- Should root `mkdir` in the FUSE mount continue creating arbitrary writable in-memory node kinds, or should kind creation move behind an explicit plugin/host policy?
