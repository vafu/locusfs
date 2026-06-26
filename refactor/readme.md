# Locusfs Review Notes

## Session Scope

Review the current workspace top down, starting with the most public-facing API surfaces and moving inward toward implementation crates and concrete plugins.

Default deliverable is understanding, review notes, open questions, refactor planning, implementation, and verification. The user asked for this pass to be non-interactive until the final validation questions.

## Repository Context

- No `AGENTS.md` file was present at session start.
- Existing architecture notes live in `architecture-audit/` and are treated as prior read-only findings, not as decisions to apply automatically.
- Workspace members are `bin`, `watch`, `graph`, `fuse`, `plugins/api`, and concrete plugin crates under `plugins/`.
- The workspace is Rust edition 2024 with explicit workspace members.

## Global Constraints

- Public crates should expose their intended contracts through `lib.rs` or module roots, with implementation modules private unless external use is real.
- Public API review should move from external protocol/layout surfaces toward lower-level provider contracts and concrete implementations.
- `plugins/api` documents the current plugin boundary as a same-workspace/toolchain Rust extension contract, not a stable cross-compiler ABI.
- Prior audit highlights likely cross-cutting themes: graph/FUSE watch vocabulary overlap, FUSE path-layout duplication, plugin registration boilerplate, and partial dependency centralization.

## Review Queue

1. `locusfs-watch` (`watch/`): external watch protocol vocabulary and optional filesystem client helpers.
2. `locusfs-fuse` (`fuse/`): public filesystem layout, mount lifecycle, and kernel request translation.
3. `locusfs-plugin-api` (`plugins/api/`): plugin author and host/plugin boundary API.
4. `locusfs-graph` (`graph/`): shared graph contracts, identity/value types, provider traits, dynamic graph implementation, and watch/change contracts.
5. `locusfs-bin` (`bin/`): private host executable orchestration, config, plugin loading, watch command, and tracing/perfetto setup.
6. Concrete plugin crates: `project`, `dbus`, `dbusmenu`, `mpris`, `niri`, `pipewire`, and `statusnotifier`.

## Completed Units

- `locusfs-watch`: reviewed in `agent-watch-api.md`.
- `locusfs-graph`: reviewed in `agent-graph-core.md`.
- `locusfs-fuse`: reviewed in `agent-fuse-runtime.md`.
- `plugins/api` and host plugin loading: reviewed in `agent-host-plugin-api.md`.
- D-Bus plugin family: reviewed in `agent-dbus-family.md`.
- Native plugins: reviewed in `agent-native-plugins.md`.
- Coordinator arbitration: recorded in `arbitration.md`.

## Cross-Cutting Findings

- The generic D-Bus path layout is the clearest immediate domain-specific simplification.
- The graph crate has a concrete feature-gate bug and repeated read-write provider registration.
- The watch API needs a future protocol migration, but client framing and FUSE state resync can be fixed independently.
- The plugin ABI/runtime policy is intentionally left unchanged in this pass because it affects compatibility and all plugins.

## Global Open Questions

- Should the top-level public contract be treated as the filesystem layout first, or should crate API stability take precedence when the two conflict?
- Are the concrete plugin crates intended to provide reusable Rust library APIs, or are their `rlib` targets only for tests and workspace-local construction?
