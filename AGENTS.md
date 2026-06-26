# LocusFS Agent Guide

This repository owns the LocusFS graph runtime, FUSE filesystem surface, watch
protocol, host binary, plugin API, and concrete native plugins. Treat this file
as the durable guide for future agents.

## First Steps

- Read this file before changing code.
- Use `rust-guide` for Rust design, implementation, testing, and refactors.
- Inspect the crate that owns the behavior before editing; do not infer current
  contracts only from old audit files.
- Check `git status --short --branch` before edits. Work with user changes; do
  not revert unrelated files.

## Crate Boundaries

- `graph/` (`locusfs-graph`) owns graph contracts: identity/value types,
  provider traits, dynamic graph routing, watches, and in-memory providers.
- `fuse/` (`locusfs-fuse`) owns the public filesystem projection, path layout,
  inode/cache behavior, read/write operations, symlinks, and `/watch` event
  production.
- `watch/` (`locusfs-watch`) owns the typed watch protocol and optional async
  filesystem client helpers. FUSE depends on it protocol-only; clients may use
  the default client feature.
- `plugins/api/` (`locusfs-plugin-api`) owns the host/plugin contract,
  manifest/config entry points, plugin handles, runtime helpers, and graph
  registration context.
- `plugins/*/` own concrete domain projections. A plugin should keep its config,
  state model, runtime watcher, provider adapter, and tests local unless a
  shared abstraction removes real duplication.
- `bin/` (`locusfs-bin`, executable `locusfs`) is the private host
  implementation. Do not turn it into a public library facade without a concrete
  embedding use case.

## Public Filesystem Contract

The filesystem layout is a public API. Keep it readable, stable, and testable.

- Generic graph nodes live at `/<kind>/<local-id>`.
- Properties are regular files under node directories.
- Relations are symlink-like entries under node directories.
- Path segments must use the repository's existing encoding/decoding helpers;
  do not build filesystem paths with ad hoc string concatenation.
- New public paths need focused FUSE/layout tests and, when practical, live
  mount verification.

## D-Bus Plugin Contract

The current generic D-Bus projection is bus-native. This supersedes the earlier
`object`, `@properties`, `@methods`, `objects`, `methods`, and `_absolute`
ideas.

Public layout:

```text
/dbus/system/<actual/dbus/object/path>/<Property>
/dbus/system/<actual/dbus/object/path>/<Method>.call
/dbus/session/<actual/dbus/object/path>/<Property>
/dbus/session/<actual/dbus/object/path>/<Method>.call
```

Rules:

- `/dbus/system` and `/dbus/session` are the public bus roots.
- The path after the bus root is the actual D-Bus object path without the
  leading slash.
- Do not expose configured service `local_id` values in the public path. They
  are internal graph identity/config details.
- Do not duplicate service names in the path just for readability. For example,
  UPower should be exposed as `/dbus/system/org/freedesktop/UPower/...`, not as
  `/dbus/system/org.freedesktop.UPower/org/freedesktop/UPower/...`.
- There is no `_absolute` namespace. Objects outside an ObjectManager root are
  still exposed by their full native D-Bus path under the bus root.
- There are no public `objects` or `methods` directories. Properties and method
  call files are children of the object directory itself.
- Real D-Bus properties appear as property files. Short names and
  fully-qualified `Interface.Property` aliases may both be listed where needed
  for disambiguation.
- Writable D-Bus properties must be represented as read/write property files.
  Read-only properties stay read-only.
- Callable D-Bus methods appear as write-only files named `<Method>.call`.
  Fully-qualified `Interface.Method.call` aliases are used when short method
  names are ambiguous or when callers need canonical names.
- Do not add a `/call` child directory for methods. The `.call` suffix is the
  human-readable call marker.
- Keep service/object metadata such as `service-name`, `path`, `interface`, and
  graph relations as graph metadata. Do not mix fake metadata files into the
  object-property tree unless they are already part of the established contract.

Important tests live in `plugins/dbus/src/state/test.rs`. Update or extend them
whenever the D-Bus layout, writability, method naming, or ObjectManager behavior
changes.

## Watch Protocol

- `locusfs-watch` is the text protocol boundary; keep encode/decode behavior
  explicit and covered by tests.
- `WatchEvent::State(WatchState::Set(WatchValue::Property(String::new())))`
  is valid and decodes from an empty set payload such as `set \n`.
- FUSE may translate graph events to watch protocol events, but
  `locusfs-watch` must not depend on `locusfs-graph` just to reduce mapping
  duplication.
- If event escaping/framing changes, update both producer and client tests. Do
  not silently broaden parsing in one side only.

## Plugin Runtime Rules

- Long-running plugins should own their runtime/watch tasks and stop them
  through `PluginHandle::shutdown` and `Drop`.
- Do not spawn untracked background tasks from plugins.
- If a plugin uses D-Bus, sockets, process subscriptions, or zbus resources,
  keep creation, polling, and cleanup inside the plugin's runtime boundary.
- Config parsing/defaults belong with the plugin unless the pattern is repeated
  enough to justify a helper in `plugins/api`.
- The current dynamic plugin ABI is workspace-local Rust trait objects loaded
  through `_locusfs_plugin_init`. Do not treat it as a stable third-party binary
  ABI.

## Do

- Keep public APIs discoverable from `lib.rs` or module roots.
- Put focused tests beside the owning module, usually in `test.rs` wired from
  `mod.rs`.
- Prefer existing graph/provider/path/value types over new parallel structs.
- Preserve dependency direction: graph below FUSE/plugins, watch protocol
  independent from graph, host binary above everything.
- Validate feature-sensitive graph changes with the relevant feature matrix,
  not only the default workspace test.
- Use live mount checks after filesystem-layout or plugin-runtime changes when
  the service can be run locally.

## Don't

- Do not resurrect service-local D-Bus roots such as `/dbus/upower`,
  `/dbus/agentdbus`, or `/dbus-service/...`.
- Do not reintroduce `@properties`, `@methods`, `/objects`, `/methods`,
  `_absolute`, or method `/call` paths.
- Do not expose implementation details in public filesystem paths just because
  they make a plugin implementation simpler.
- Do not add speculative abstraction layers around providers, plugins, or
  watches unless they remove active duplication with a clear invariant.
- Do not block FUSE request paths on slow external service calls when cached
  projection state can be maintained asynchronously.
- Do not use `cargo check` alone as validation for public layout or watch
  protocol changes; run the behavior tests that encode the contract.

## Verification

Useful commands:

```sh
CARGO_TARGET_DIR=/tmp/locusfs-target cargo test --workspace
cargo fmt --check
```

Narrow checks:

```sh
CARGO_TARGET_DIR=/tmp/locusfs-target cargo test -p locusfs-watch protocol
CARGO_TARGET_DIR=/tmp/locusfs-target cargo test -p locusfs-fuse watch
CARGO_TARGET_DIR=/tmp/locusfs-target cargo test -p locusfs-plugin-dbus state
CARGO_TARGET_DIR=/tmp/locusfs-target cargo test -p locusfs-graph --all-features
```

Feature checks for graph changes:

```sh
cargo check -p locusfs-graph --no-default-features
cargo check -p locusfs-graph --no-default-features --features dynamic
cargo check -p locusfs-graph --no-default-features --features in-memory
cargo check -p locusfs-graph --no-default-features --features watch-provider
cargo check -p locusfs-graph --no-default-features --features provider-tracing
cargo check -p locusfs-graph --all-features
```

Operational checks after installing local changes:

```sh
systemctl --user restart locusfs.service
systemctl --user status locusfs.service --no-pager
find /run/user/1000/locusfs/dbus/system -maxdepth 3 -type d | sort | head
```

Adjust `/run/user/1000/locusfs` when `LOCUS_ROOT` points elsewhere.
