# LocusFS Review Notes

## Session Scope

Review the current uncommitted change set before committing. The diff introduces graph provider routing/change notifications, FUSE layout and poll/watch behavior, a runtime binary watcher, a Niri provider plugin, and one script artifact.

## Global Constraints

- Keep graph semantics in `locusfs-graph`.
- Keep kernel/filesystem translation and public filesystem layout in `locusfs-fuse`.
- Keep runtime assembly and CLI behavior in `locusfs`.
- Keep Niri-specific IPC projection in `plugins/niri`.
- Preserve native symlinks for graph relations.
- Reactive consumers should use event-driven FUSE poll via `/watch`; normal file reads remain regular graph data access.

## Review Queue

1. `locusfs-graph`: provider traits, dynamic graph routing, change stream, tracing wrapper.
2. `locusfs-fuse`: path layout, inode/timestamp state, FUSE operations, invalidation, poll registry, real mount tests.
3. `plugins/niri`: IPC client, state projection, provider implementation, tests.
4. `locusfs`: mount CLI, default graph composition, `--watch` client.
5. `scripts`: decide whether experimental scripts belong in the commit.

## Completed Units

- None yet.

## Cross-Cutting Findings

- Pending review.

## Open Questions

- Should `/watch` event payload format be formalized before commit?
- Should the current `props` directory remain in the committed layout, or should inline properties be handled first?
- Should experimental scripts be committed, moved under docs/tools, or dropped?
