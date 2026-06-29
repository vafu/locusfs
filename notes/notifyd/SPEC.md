# LocusFS Notifyd Plugin Spec

Status: v1 implementation contract for `locusfs-plugin-notifyd`.
Updated: 2026-06-29.

## Decision: One Notification Collection

`locusfs-plugin-notifyd` owns the Freedesktop notification daemon protocol and
the canonical retained notification list. It does not own popup-active UI
lifetime. Popup visibility is shell policy derived from notification timestamps
and `expire-timeout-ms`.

The public tree has one notification collection:

```text
/notifyd/notifications/<id>
```

There is intentionally no `/notifyd/active` and no `/notifyd/history` in the
backend contract. A notification remains in `/notifyd/notifications` until it is
explicitly discarded, removed by `CloseNotification`, closed by an action policy,
or trimmed by the daemon's bounded-list cap.

## References

- Freedesktop Desktop Notifications Specification: https://specifications.freedesktop.org/notification-spec/latest/
- Freedesktop D-Bus surface: `org.freedesktop.Notifications.Notify`,
  `CloseNotification`, `GetCapabilities`, `GetServerInformation`,
  `NotificationClosed`, and `ActionInvoked`.
- SwayNotificationCenter / swaync: https://github.com/ErikReider/SwayNotificationCenter
- Dunst documentation: https://dunst-project.org/documentation/

## Ownership

- `notifyd` owns `org.freedesktop.Notifications` on the session bus.
- `notifyd` normalizes incoming D-Bus requests into retained LocusFS records.
- `notifyd` owns discard, D-Bus close, action invocation, replacement IDs,
  bounded-list trimming, DND state, and payload normalization.
- `locus-shell` observes `/notifyd/notifications` and renders popups or a
  notification center. It may write `discard` and `actions/<key>/invoke`.
- Popup timeout, animation, placement, grouping presentation, and center-open
  state are shell UI policy.

## D-Bus Contract

The plugin owns:

```text
bus:       session
service:   org.freedesktop.Notifications
path:      /org/freedesktop/Notifications
interface: org.freedesktop.Notifications
```

Methods:

- `GetCapabilities() -> as`
- `Notify(app_name, replaces_id, app_icon, summary, body, actions, hints, expire_timeout) -> u`
- `CloseNotification(id)`
- `GetServerInformation() -> (name, vendor, version, spec_version)`

Signals:

- `NotificationClosed(id, reason)`
- `ActionInvoked(id, action_key)`

Close reasons:

- `1`: expired, reserved for future daemon-side expiry/trim policy
- `2`: dismissed/discarded by user
- `3`: closed by API call
- `4`: undefined fallback

Default capabilities:

```text
actions
body
body-markup
body-images
icon-static
persistence
```

## Lifecycle Semantics

- `Notify` creates or replaces a record under `/notifyd/notifications/<id>` and
  emits graph changes for watches.
- `replaces_id != 0` updates the retained notification with the matching D-Bus
  ID when it exists; unknown replacement IDs allocate a new ID.
- `expire_timeout` is normalized into `expire-timeout-ms` and retained as UI
  policy input. It does not remove the backend record.
- Writing `/notifyd/notifications/<id>/discard` removes the record and emits
  `NotificationClosed(id, 2)`.
- `CloseNotification(id)` removes the record and emits
  `NotificationClosed(id, 3)`.
- Invoking an action emits `ActionInvoked(id, action_key)`. If
  `close_on_action` is enabled and the notification is not resident, the record
  is removed and `NotificationClosed(id, 2)` is emitted.
- DND does not hide records from LocusFS. It is exposed as state so shell
  consumers can decide whether to show popups. `suppressed-count` may still
  count non-critical notifications received while DND is enabled.

## Public Path Contract

Collection root:

```text
/notifyd/notifications/
```

Notification properties:

```text
/notifyd/notifications/<id>/id
/notifyd/notifications/<id>/dbus-id
/notifyd/notifications/<id>/source
/notifyd/notifications/<id>/created-at-unix-ms
/notifyd/notifications/<id>/updated-at-unix-ms
/notifyd/notifications/<id>/expire-timeout-ms
/notifyd/notifications/<id>/app-name
/notifyd/notifications/<id>/desktop-entry
/notifyd/notifications/<id>/app-icon
/notifyd/notifications/<id>/summary
/notifyd/notifications/<id>/body
/notifyd/notifications/<id>/body-markup        # optional
/notifyd/notifications/<id>/category
/notifyd/notifications/<id>/urgency            # low | normal | critical
/notifyd/notifications/<id>/urgency-level      # 0 | 1 | 2
/notifyd/notifications/<id>/progress           # optional
/notifyd/notifications/<id>/resident
/notifyd/notifications/<id>/transient
/notifyd/notifications/<id>/suppress-sound
/notifyd/notifications/<id>/icon-name
/notifyd/notifications/<id>/image-path         # optional
/notifyd/notifications/<id>/image-source
/notifyd/notifications/<id>/image-width        # optional
/notifyd/notifications/<id>/image-height       # optional
/notifyd/notifications/<id>/stack-key          # optional
```

Notification commands:

```text
/notifyd/notifications/<id>/discard            # write-only string/bool
```

Actions:

```text
/notifyd/notifications/<id>/actions/<action>/key
/notifyd/notifications/<id>/actions/<action>/label
/notifyd/notifications/<id>/actions/<action>/default
/notifyd/notifications/<id>/actions/<action>/icon-name   # optional
/notifyd/notifications/<id>/actions/<action>/invoke      # write-only string
```

Daemon state:

```text
/notifyd/state/dnd-enabled
/notifyd/state/notification-count
/notifyd/state/suppressed-count
/notifyd/state/server-name
```

Daemon commands:

```text
/notifyd/commands/discard-all     # write-only string; removes retained records
/notifyd/commands/dnd-enabled     # read-write bool
```

## Normalized Payload

The record stores:

- identity: `dbus-id`, path-local `id`, source, timestamps
- app identity: app name, desktop entry, app icon
- content: summary, plain body, optional sanitized body markup, category
- priority/progress: urgency string/level, optional progress
- lifecycle hints: resident, transient, suppress-sound, expire timeout
- media: icon name, image path/source/size when available
- grouping: stack key when available
- actions: key, label, default flag, optional icon name

Hints parsed explicitly include urgency, category, desktop-entry, resident,
transient, suppress-sound, sound-name, sound-file, image-data/image_data,
image-path/image_path, icon_data, value/progress,
`x-canonical-private-synchronous`, and `x-dunst-stack-tag`.

## Config

Example:

```toml
[plugins.notifyd]
enabled = true

[plugins.notifyd.config]
server-name = "Locus Notifyd"
vendor = "Locus"
default-timeout-ms = 7000
low-timeout-ms = 4000
critical-timeout-ms = 0
max-notifications = 128
max-body-bytes = 16384
max-actions = 12
markup = true
actions = true
body-images = true
close-on-action = true
dnd-enabled = false
```

`max-active`, `max_active`, `max-history`, and `max_history` are accepted as
compatibility aliases for `max-notifications` while local configs migrate.

## Tests

State/path tests should cover:

- `/notifyd/notifications` lists retained notification directories
- notification properties match the normalized model
- `discard` and action `invoke` specs are write-only
- discard removes the notification directory and updates `notification-count`
- action children and action properties are exposed
- replacement updates existing IDs
- urgency parsing handles numeric and string forms
- DND state is read/write and suppression count changes are observable
- config defaults and invalid values are validated

Runtime/live checks should cover:

- `notify-send` creates `/notifyd/notifications/<id>`
- popup timeout in the shell hides only the popup, not the LocusFS record
- writing `discard` removes the record
- `CloseNotification` emits reason 3
- action command emits `ActionInvoked`

## Shell Consumer Contract

Shell widgets consume:

- `/notifyd/notifications` children
- notification property files
- action child/property files
- `/notifyd/state/notification-count` for list presence indicators
- command writes to `discard` and `actions/<key>/invoke`

Shell widgets must not implement `org.freedesktop.Notifications`, call zbus
notification APIs directly, or parse raw Freedesktop hint dictionaries.

## Open Follow-Up

- Encode Freedesktop raw `image-data`/`icon_data` tuples into bounded runtime
  cache files.
- Decide whether future persisted notification storage belongs in notifyd and
  how much payload to retain across daemon restarts.
- Add rule support for app/category/urgency policy once concrete local needs are
  clear.
