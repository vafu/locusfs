# Locusfs Refactor Arbitration

Started: 2026-06-25T20:54:19-07:00

## Inputs

This file arbitrates the subagent reports in this directory and records the implementation decisions for this pass. The reports reviewed were:

- `agent-watch-api.md`
- `agent-graph-core.md`
- `agent-fuse-runtime.md`
- `agent-host-plugin-api.md`
- `agent-dbus-family.md`
- `agent-native-plugins.md`

Prior architecture notes in `architecture-audit/` were used as context, not as binding decisions.

## External Checks

- Rust external blocks: the Rust Reference documents `extern "Rust"` as the native Rust ABI and states it has no stability guarantees. This supports treating the plugin boundary as same-toolchain only for now rather than pretending it is a stable cross-compiler ABI. Source: https://doc.rust-lang.org/reference/items/external-blocks.html
- zbus ObjectManager: `ObjectManagerProxy::get_managed_objects` returns child object paths with interface/property maps, which fits an object-tree path index under a configured ObjectManager root. Source: https://docs.rs/zbus/latest/zbus/fdo/struct.ObjectManagerProxy.html
- zbus Properties: `PropertiesProxy` exposes `get`, `set`, `get_all`, and properties-changed stream helpers. This supports keeping property reads/writes behind the plugin runtime and not exposing zbus types through graph/FUSE APIs. Source: https://docs.rs/zbus/latest/zbus/fdo/struct.PropertiesProxy.html

## Decisions

### 1. Public compatibility is not stable yet

Decision: prioritize clarity and correctness over preserving the current D-Bus `object/@properties/@methods` layout.

Reasoning: the user explicitly identified that layout as hard to maintain and requested a clearer structure. No report identified a stable downstream contract that requires legacy aliases. The implementation should skip compatibility aliases unless tests or a real consumer breakage force them.

### 2. D-Bus path layout becomes bus-native

Decision: replace `/dbus/<service>/object/.../@properties`, `/dbus/<service>/object/.../@methods`, and the interim `/objects`/`/methods` split with a bus-native object-path projection:

```text
/dbus/system/<actual/dbus/path>/<property>
/dbus/system/<actual/dbus/path>/<method>.call
/dbus/session/<actual/dbus/path>/<property>
/dbus/session/<actual/dbus/path>/<method>.call
```

Rules:

- The path after `/dbus/system` or `/dbus/session` is the actual D-Bus object path without the leading slash. The configured ObjectManager root is not stripped from public paths.
- There is no `_absolute`, `objects`, or `methods` directory. Full D-Bus paths avoid the earlier duplicated service/root distinction and keep UPower, BlueZ, NetworkManager, and AgentDBus predictable.
- Normal property files map to real D-Bus properties.
- Callable method files use a `.call` suffix and map to the existing hidden method node's write-only `call` property.
- Canonical `interface.member` names remain available. Short aliases remain available only when already unambiguous in the state resolver.
- Child object directory names win over property/method file names on collision.
- Service/object/method metadata remains graph properties and is not projected into the object property tree.

### 3. Watch protocol replacement is deferred, but framing is fixed now

Decision: do not replace the text wire format in this pass. Fix the client so one readiness drain containing multiple newline frames produces multiple `next_event` results.

Reasoning: the protocol has real problems around `set /tmp`, empty strings, whitespace, and lossy change streams. A wire-format migration affects FUSE, CLI, and consumers, so it needs a dedicated compatibility decision. The multi-frame client bug is a narrower correctness fix with low blast radius.

### 4. FUSE watch lag recovery should recover state watches

Decision: on broadcast lag, invalidate known inodes as before, then re-resolve and queue current state for all configured state-mode watch handles. Change-mode watches still receive generic `change`.

Reasoning: `notify_all` currently queues generic changes that state-mode watches ignore. A lag event should restore the strongest guarantee available: current state after re-resolution.

### 5. Graph registration helpers are worthwhile, but not a new abstraction layer

Decision: add helper methods on `DynamicGraph` for common read-only and read-write provider bundles while keeping the granular registration API.

Reasoning: project registration and FUSE root `mkdir` repeated the same six calls. A helper removes real duplication without hiding provider traits or introducing a plugin-common crate too early.

### 6. Plugin ABI and runtime policy are not changed in this pass

Decision: keep `_locusfs_plugin_init`, same-toolchain trait-object loading, and plugin-owned runtimes as-is. Record them as future architecture decisions.

Reasoning: the current API is explicit about Rust extension semantics. The Rust Reference check confirms that a stable ABI story cannot be assumed. Changing runtime ownership also affects all loaded plugins and host shutdown ordering.

## Deferred Work

- Design an unambiguous watch state wire format and explicit subscription modes.
- Decide whether graph should expose `GraphChangeSubscription` instead of raw `broadcast::Receiver`.
- Split large modules only after behavior-level changes land.
- Extract D-Bus/common plugin helpers after the new D-Bus layout and DBusMenu correctness are settled.
- Decide StatusNotifier coexistence policy with an existing watcher.
- Decide Niri availability policy: fail registration when unavailable versus register and retry.

## Acceptance Criteria For This Pass

- D-Bus generic plugin tests assert the new bus-native paths and `.call` method files.
- Graph feature matrix includes `provider-tracing` without `watch-provider`.
- Watch client has a regression test for two frames read together.
- FUSE registry tests cover readiness while a partial watch event is buffered.
- Workspace tests pass.
