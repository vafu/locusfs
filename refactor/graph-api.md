# Graph And Plugin API Notes

## Current Role

`locusfs-graph` owns graph identifiers, values, provider traits, dynamic provider routing, graph changes, fallback watch filtering, the in-memory provider, and tracing wrappers.

`locusfs-plugin-api` owns the current plugin contract and runtime-enter helper used by dynamically loaded plugin tasks.

## Findings

- Provider registration silently replaces existing providers in `DynamicGraph`. This can hide plugin load-order bugs.
- Missing provider, unsupported capability, and missing graph data are too easy to conflate.
- Addressed 2026-06-18: removing a node now cleans overlay links where the removed node is the source.
- Mutations are not idempotence-aware. Recreating an existing node or setting an existing link can still emit lifecycle/change events.
- Watch semantics have two implementations: provider-specific watches and graph fallback watches. There is no conformance helper.
- Public aliases duplicate vocabulary: `subscribe_changes` vs `subscribe_global_changes`, `emit_change` vs `emit_global_change`.
- `PluginHandle` is only a drop-retained marker. The recent shutdown behavior implies it wants to become a lifecycle trait.
- `RuntimeEntered` uses unsafe pin handling. It is pragmatic, but a boxed future or pin-project style wrapper would be easier to audit.

## Refactor Plan

1. Done: add duplicate-provider tests and make registration return an error unless replacement is explicit.
2. Split error variants into provider missing, unsupported capability, and not-found data.
3. Done: fix outbound overlay cleanup on source node removal.
4. Add mutation outcome semantics or compare before/after state before emitting changes.
5. Partly done: FUSE now uses graph watch target/event types directly. Remaining: move graph watch filtering into a reusable helper and use it for fallback and provider-watch tests.
6. Collapse old watch/change aliases after call sites use the global vocabulary.
7. Promote `PluginHandle` into an async shutdown/lifecycle trait.

## Tests And Verification

- `cargo test -p locusfs-graph -p locusfs-plugin-api`
- Add tests for duplicate registration, source-node overlay cleanup, idempotent create/link operations, and watch filtering equivalence.
