# plugins/niri Refactor Notes

## Current Role

Projects live Niri IPC state into graph providers for window, workspace, output, and context nodes.

## Public Surface

- `src/lib.rs`: plugin registration and provider kind list.
- `src/provider.rs`: graph provider trait implementation over Niri state.
- `src/state.rs`: in-memory projection of Niri data into nodes, properties, and relations.
- `src/ipc.rs`: Niri IPC client and update loop.

## Step-By-Step File Walkthrough

1. `src/lib.rs`: registration contract and dependency direction into `locusfs-graph`.
2. `src/ipc.rs`: side effects, thread/runtime behavior, graph change emission.
3. `src/state.rs`: data model and projection rules.
4. `src/provider.rs`: provider trait implementation and read behavior.
5. `src/state/test.rs`: contract examples for live projection behavior.

## Internal Structure

Pending review.

## Behavior Summary

Pending review.

## User Notes

Pending.

## Findings

Pending.

## Refactor Plan

Pending.

## Tests And Verification

Pending review.

## Open Questions

- Which Niri entities are expected to be stable node IDs, and which are ephemeral?
- Should IPC lifecycle failures surface through graph/provider state or runtime logs only?

