# LocusFS Notifyd Implementation Notes

Updated: 2026-06-29.

## Current State

- `plugins/notifyd` implements a v1 Freedesktop notification daemon plugin.
- It owns `org.freedesktop.Notifications` on the session bus at
  `/org/freedesktop/Notifications`.
- Retained notifications are exported under `/notifyd/notifications/<id>`.
- There is no backend `/notifyd/active` or `/notifyd/history` split. Popup
  lifetime is shell UI policy.
- Daemon state and commands are exported at `/notifyd/state` and
  `/notifyd/commands`.
- `rsynapse-shell` consumes `/notifyd/notifications` for both bottom-left
  popups and the clock-opened notification center. The shell filters popups by
  `expire-timeout-ms`; it does not ask notifyd to archive popup dismissals.

## Runtime Decisions

- Plugin startup waits until the D-Bus service has acquired the notification
  name before provider registration succeeds. This makes an existing daemon,
  such as `swaync`, a visible startup failure instead of a background-task log.
- The plugin handle owns the runtime task and aborts it on shutdown/drop.
- `Notify` creates or replaces one retained record in `/notifyd/notifications`.
- `expire-timeout-ms` is retained as UI policy input and no daemon-side timeout
  task removes notifications.
- Shell command writes are ordinary property writes:
  `/notifyd/notifications/<id>/discard` removes a notification,
  `/notifyd/notifications/<id>/actions/<action>/invoke` invokes actions, plus
  `/notifyd/commands/discard-all` and `/notifyd/commands/dnd-enabled`.
- DND no longer hides notifications from LocusFS. It remains observable state;
  shell consumers decide whether DND suppresses popups.
- `max-notifications` bounds the retained in-memory list. `max-active`,
  `max_active`, `max-history`, and `max_history` are accepted as config aliases
  during migration.

## Implemented Payload Surface

- Summary, body, optional body markup, app name, desktop entry, app icon,
  category, urgency, progress, timeout policy, resident/transient/suppress-sound
  hints, stack keys, image paths, icon names, and action buttons.
- `image-path`/`image_path` and absolute `app_icon` paths are exposed as
  `image-path` for GTK consumers.
- Raw `image-data` is detected as an image source, but encoding it to a runtime
  PNG cache is still future work.

## Verification

- `env CARGO_TARGET_DIR=/tmp/locusfs-target cargo test -p locusfs-plugin-notifyd`
  passed after the single-list schema refactor.
- `env CARGO_TARGET_DIR=/tmp/locus-shell-target cargo test -p rsynapse-shell notifications`
  passed after switching shell sources to `/notifyd/notifications`.

## Known Follow-Up

- Encode Freedesktop raw `image-data`/`icon_data` tuples into bounded runtime
  cache files.
- Add live validation after reinstall/restart:
  `notify-send` should create `/run/user/1000/locusfs/notifyd/notifications/<id>`,
  popup timeout should hide only the popup, and writing `discard` should remove
  the retained record.
- Persist notifications across daemon restarts only if there is a concrete need.
