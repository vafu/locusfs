# Step 05: Fuse3 Mount Skeleton

## Reason

Before porting every operation, verify that `fuse3::raw` can mount, serve at least root metadata, and shut down cleanly under the selected runtime. This limits risk before touching the entire `filesystem.rs` implementation.

## Outcome

- Replace the `fuser::spawn_mount2` mount path with a `fuse3::raw::Session`-based mount path.
- Keep `FuseMountConfig` and public `mount` API shape as stable as possible, but allow `mount` to become async if needed.
- Introduce an async `FuseMount` lifecycle handle that keeps the mount task alive and unmounts on drop or explicit shutdown.
- Implement only the minimum raw filesystem methods needed for a mount skeleton:
  - `init`
  - `destroy`
  - `lookup` for root or known static entries as needed
  - `getattr` for root
  - `opendir` / `readdir` for root if needed by smoke checks
- Keep the old `fuser` implementation temporarily behind a module boundary if that makes incremental porting easier.

## Way To Test

Run:

```sh
cargo check -p locusfs-fuse
```

Then run a manual mount smoke check where FUSE is available:

```sh
cargo run -p locusfs -- /tmp/locusfs-fuse3-smoke
ls -la /tmp/locusfs-fuse3-smoke
```

Expected result: mount starts, root can be statted/listed, shutdown unmounts without leaving a stuck mount.
