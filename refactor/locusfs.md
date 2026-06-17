# locusfs Binary Refactor Notes

## Current Role

Owns the executable runtime: CLI parsing, tracing setup, signal handling, default graph/provider composition, mount startup, and the `--watch` consumer helper.

## Public Surface

- `src/main.rs`: command dispatch and runtime assembly.
- `src/watch.rs`: `/watch` client that polls event notifications and re-reads the normal data path.

## Step-By-Step File Walkthrough

1. `src/main.rs`: CLI shape, default graph composition, mount lifecycle.
2. `src/watch.rs`: consumer-side event loop and subscription behavior.
3. `Cargo.toml`: runtime dependencies and plugin dependency direction.

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

- Should `--watch` print event metadata, only values, or support modes?
- Should runtime plugin loading stay as static dependency for now or be isolated behind a loader boundary before commit?
