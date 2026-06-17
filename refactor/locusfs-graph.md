# locusfs-graph Refactor Notes

## Current Role

Owns graph identity, typed values, property metadata, provider contracts, dynamic provider routing, graph changes, and typed graph errors.

## Public Surface

- `lib.rs`: re-exports graph API, identity types, values, and errors.
- `graph/mod.rs`: provider traits and public graph implementations/wrappers.
- `graph/dynamic.rs`: `DynamicGraph` provider registry and mutation/change behavior.
- `graph/change.rs`: semantic change events consumed by FUSE.
- `graph/trace.rs`: provider tracing wrapper.

## Step-By-Step File Walkthrough

1. `src/lib.rs`: crate boundary and public API exports.
2. `src/graph/mod.rs`: provider trait contracts; these define ownership boundaries for graph data.
3. `src/graph/dynamic.rs`: runtime composition and provider routing.
4. `src/graph/change.rs`: event model used by downstream filesystem invalidation and poll wakeups.
5. `src/graph/trace.rs`: cross-cutting diagnostics wrapper.
6. `src/graph/test.rs`: contract tests and mutation/change expectations.

## Internal Structure

- `graph/mod.rs` defines six independent provider traits: read and mutation variants for nodes, properties, and relations.
- `DynamicGraph` stores provider registries keyed by `NodeKind`; each capability has its own map.
- `DynamicGraph` owns a change subscriber list and emits `GraphChange` values after successful mutations.
- `InMemoryProvider` remains a concrete provider implementing all six capabilities for one `NodeKind`.
- `TracedProvider<P>` is a generic wrapper implementing every provider trait that its inner provider supports.

## Behavior Summary

- Reads route by `NodeKind` from `NodeId`.
- Mutations require a mutation provider for the subject node kind.
- `set_link` validates that the target node exists before delegating to the source node kind's relation mutation provider.
- `remove_node` first removes inbound links from every registered relation provider/mutation provider pair, then removes the node, then emits node removal and node-kind changes.
- Change subscribers receive best-effort `std::sync::mpsc` events; dead subscribers are dropped on the next emission.

## User Notes

- User accepted the current `locusfs-graph` direction and asked to jump to `locusfs-fuse`.

## Findings

- `GraphChange` is currently semantic and coarse enough for FUSE invalidation, but it does not describe old/new relation targets or property values.
- `remove_inbound_links` calls provider mutation methods directly, so relation removals caused by node deletion do not emit individual `RelationChanged` events.
- `TracedProvider` lives in graph core; this is practical now, but it makes tracing policy part of the public graph API.

## Refactor Plan

- No immediate graph refactor requested during this pass.

## Tests And Verification

Pending review.

## Open Questions

- Are `GraphChange` variants sufficiently precise for future provider-backed dynamic nodes?
- Should provider tracing remain in graph core, or move to a utility/adaptor layer?
