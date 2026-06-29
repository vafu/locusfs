use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use locusfs_graph::{DynamicGraph, GraphChange, GraphError, Result};
use tokio::runtime::Handle;
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio::task::JoinHandle;
use zbus::fdo::DBusProxy;
use zbus::message::Header;
use zbus::names::{BusName, WellKnownName};
use zbus::object_server::SignalEmitter;
use zbus::zvariant::OwnedValue;

use crate::config::NotifydConfig;
use crate::image::normalize_image;
use crate::markup::{plain_text, sanitized_markup};
use crate::provider::NotifydCommand;
use crate::state::{NotificationRecord, NotificationUrgency, SharedNotifydState, make_action};

const NOTIFICATIONS_SERVICE: &str = "org.freedesktop.Notifications";
const NOTIFICATIONS_PATH: &str = "/org/freedesktop/Notifications";
const SPEC_VERSION: &str = "1.2";

#[derive(Debug, Default)]
pub struct NotifydRuntime;

impl NotifydRuntime {
    pub async fn start(
        graph: DynamicGraph,
        config: NotifydConfig,
        runtime: Handle,
    ) -> Result<(
        SharedNotifydState,
        mpsc::UnboundedSender<NotifydCommand>,
        JoinHandle<()>,
    )> {
        let state =
            crate::state::NotifydState::shared(config.server_name.clone(), config.dnd_enabled);
        let (commands, command_rx) = mpsc::unbounded_channel();
        let (ready_tx, ready_rx) = oneshot::channel();
        let service_state = state.clone();
        let task = runtime.spawn(async move {
            if let Err(error) =
                run_notifyd_service(service_state, graph, config, command_rx, ready_tx).await
            {
                eprintln!("locusfs-notifyd: notification daemon stopped: {error}");
            }
        });
        match ready_rx.await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                task.abort();
                let _ = task.await;
                return Err(GraphError::Io(error));
            }
            Err(_) => {
                task.abort();
                let _ = task.await;
                return Err(GraphError::Io(
                    "notification daemon startup task exited before readiness".to_owned(),
                ));
            }
        }
        Ok((state, commands, task))
    }
}

#[derive(Clone)]
struct NotificationsService {
    graph: DynamicGraph,
    state: SharedNotifydState,
    config: NotifydConfig,
    next_id: std::sync::Arc<Mutex<u32>>,
}

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl NotificationsService {
    async fn get_capabilities(&self) -> Vec<String> {
        let mut capabilities = vec![
            "body".to_owned(),
            "body-markup".to_owned(),
            "icon-static".to_owned(),
            "persistence".to_owned(),
        ];
        if self.config.actions {
            capabilities.push("actions".to_owned());
        }
        if self.config.body_images {
            capabilities.push("body-images".to_owned());
        }
        capabilities
    }

    async fn get_server_information(&self) -> (String, String, String, String) {
        (
            self.config.server_name.clone(),
            self.config.vendor.clone(),
            env!("CARGO_PKG_VERSION").to_owned(),
            SPEC_VERSION.to_owned(),
        )
    }

    async fn notify(
        &self,
        app_name: &str,
        replaces_id: u32,
        app_icon: &str,
        summary: &str,
        body: &str,
        actions: Vec<String>,
        hints: BTreeMap<String, OwnedValue>,
        expire_timeout: i32,
        #[zbus(header)] header: Header<'_>,
    ) -> zbus::fdo::Result<u32> {
        let id = self.notification_id(replaces_id).await.map_err(fdo_error)?;
        let sender = header.sender().map(ToString::to_string).unwrap_or_default();
        let now = now_unix_ms();
        let record = normalized_notification(
            id,
            sender,
            app_name,
            app_icon,
            summary,
            body,
            actions,
            hints,
            expire_timeout,
            now,
            &self.config,
        );

        let changes = {
            let mut state = self.state.write().await;
            let mut changes = Vec::new();
            let dnd_enabled = state.dnd_enabled();
            if dnd_enabled && record.urgency != NotificationUrgency::Critical {
                changes.extend(state.increment_suppressed().map_err(fdo_error)?);
            }
            changes.extend(
                state
                    .upsert_notification(record, self.config.max_notifications)
                    .map_err(fdo_error)?,
            );
            changes
        };
        publish_changes(&self.graph, changes);
        Ok(id)
    }

    async fn close_notification(
        &self,
        id: u32,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> zbus::fdo::Result<()> {
        self.remove_notification_by_id(id.to_string(), 3, &emitter)
            .await
            .map_err(fdo_error)
    }

    #[zbus(signal)]
    async fn notification_closed(
        signal_emitter: &SignalEmitter<'_>,
        id: u32,
        reason: u32,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn action_invoked(
        signal_emitter: &SignalEmitter<'_>,
        id: u32,
        action_key: &str,
    ) -> zbus::Result<()>;
}

impl NotificationsService {
    async fn notification_id(&self, replaces_id: u32) -> Result<u32> {
        if replaces_id != 0 && self.state.read().await.contains_dbus_id(replaces_id) {
            return Ok(replaces_id);
        }

        let mut next = self.next_id.lock().await;
        for _ in 0..u32::MAX {
            let candidate = if *next == 0 { 1 } else { *next };
            *next = candidate.wrapping_add(1).max(1);
            if !self.state.read().await.contains_dbus_id(candidate) {
                return Ok(candidate);
            }
        }
        Err(GraphError::Io(
            "all notification IDs are retained".to_owned(),
        ))
    }

    async fn remove_notification_by_id(
        &self,
        notification_id: String,
        reason: u32,
        emitter: &SignalEmitter<'_>,
    ) -> Result<()> {
        let dbus_id = self
            .state
            .read()
            .await
            .notification(&notification_id)
            .map(|record| record.dbus_id);
        let removed = {
            let mut state = self.state.write().await;
            state.discard_notification(&notification_id)?
        };
        let Some(dbus_id) = dbus_id else {
            return Ok(());
        };
        publish_changes(&self.graph, removed);
        Self::notification_closed(emitter, dbus_id, reason)
            .await
            .map_err(|error| GraphError::Io(format!("emit NotificationClosed: {error}")))
    }

    async fn discard_notification_by_id(
        &self,
        notification_id: String,
        emitter: &SignalEmitter<'_>,
    ) -> Result<()> {
        self.remove_notification_by_id(notification_id, 2, emitter)
            .await
    }

    async fn invoke_action(
        &self,
        notification_id: String,
        action_key: String,
        emitter: &SignalEmitter<'_>,
    ) -> Result<()> {
        let Some((notification, _action)) = self
            .state
            .read()
            .await
            .action_for_key(&notification_id, &action_key)
        else {
            return Err(GraphError::NotFound {
                kind: "notification action",
                name: format!("{notification_id}/{action_key}"),
            });
        };

        Self::action_invoked(emitter, notification.dbus_id, &action_key)
            .await
            .map_err(|error| GraphError::Io(format!("emit ActionInvoked: {error}")))?;
        if self.config.close_on_action && !notification.resident {
            self.remove_notification_by_id(notification_id, 2, emitter)
                .await?;
        }
        Ok(())
    }
}

async fn run_notifyd_service(
    state: SharedNotifydState,
    graph: DynamicGraph,
    config: NotifydConfig,
    mut command_rx: mpsc::UnboundedReceiver<NotifydCommand>,
    ready_tx: oneshot::Sender<std::result::Result<(), String>>,
) -> Result<()> {
    if let Err(error) = ensure_notification_name_available().await {
        let _ = ready_tx.send(Err(error.to_string()));
        return Err(error);
    }

    let service = NotificationsService {
        graph: graph.clone(),
        state,
        config,
        next_id: std::sync::Arc::new(Mutex::new(1)),
    };
    let connection = match zbus::connection::Builder::session()
        .map_err(|error| GraphError::Io(format!("create session connection: {error}")))?
        .serve_at(NOTIFICATIONS_PATH, service.clone())
        .map_err(|error| GraphError::Io(format!("serve notification interface: {error}")))?
        .name(well_known_name(NOTIFICATIONS_SERVICE)?)
        .map_err(|error| GraphError::Io(format!("own notification name: {error}")))?
        .build()
        .await
        .map_err(|error| GraphError::Io(format!("connect notification service: {error}")))
    {
        Ok(connection) => connection,
        Err(error) => {
            let _ = ready_tx.send(Err(error.to_string()));
            return Err(error);
        }
    };
    let emitter = match SignalEmitter::new(&connection, NOTIFICATIONS_PATH)
        .map_err(|error| GraphError::Io(format!("create notification signal emitter: {error}")))
    {
        Ok(emitter) => emitter,
        Err(error) => {
            let _ = ready_tx.send(Err(error.to_string()));
            return Err(error);
        }
    };
    let _ = ready_tx.send(Ok(()));

    while let Some(command) = command_rx.recv().await {
        let result = match command {
            NotifydCommand::Discard { notification_id } => {
                service
                    .discard_notification_by_id(notification_id, &emitter)
                    .await
            }
            NotifydCommand::InvokeAction {
                notification_id,
                action_key,
            } => {
                service
                    .invoke_action(notification_id, action_key, &emitter)
                    .await
            }
            NotifydCommand::DiscardAll => discard_all(&service, &emitter).await,
            NotifydCommand::SetDnd(enabled) => set_dnd(&service, enabled).await,
        };
        if let Err(error) = result {
            eprintln!("locusfs-notifyd: command failed: {error}");
        }
    }

    Ok(())
}

async fn discard_all(service: &NotificationsService, emitter: &SignalEmitter<'_>) -> Result<()> {
    let records = service
        .state
        .read()
        .await
        .notification_ids()
        .into_iter()
        .collect::<Vec<_>>();
    for id in records {
        service.remove_notification_by_id(id, 2, emitter).await?;
    }
    Ok(())
}

async fn set_dnd(service: &NotificationsService, enabled: bool) -> Result<()> {
    let changes = {
        let mut state = service.state.write().await;
        state.set_dnd_enabled(enabled)?
    };
    publish_changes(&service.graph, changes);
    Ok(())
}

fn normalized_notification(
    id: u32,
    _sender_unique_name: String,
    app_name: &str,
    app_icon: &str,
    summary: &str,
    body: &str,
    actions: Vec<String>,
    hints: BTreeMap<String, OwnedValue>,
    expire_timeout: i32,
    now: u64,
    config: &NotifydConfig,
) -> NotificationRecord {
    let local_id = id.to_string();
    let urgency = urgency(&hints);
    let timeout = effective_timeout(expire_timeout, urgency, config);
    let image = normalize_image(app_icon, &hints, config.body_images);
    let body_plain = plain_text(body);

    NotificationRecord {
        local_id: local_id.clone(),
        dbus_id: id,
        created_at_unix_ms: now,
        updated_at_unix_ms: now,
        expire_timeout_ms: timeout,
        app_name: trim_to_limit(app_name, 512),
        desktop_entry: hint_string(&hints, "desktop-entry").unwrap_or_default(),
        app_icon: app_icon.to_owned(),
        summary: trim_to_limit(summary, 1024),
        body: trim_to_limit(&body_plain, config.max_body_bytes),
        body_markup: sanitized_markup(body, config.markup),
        category: hint_string(&hints, "category").unwrap_or_default(),
        urgency,
        progress: progress(&hints),
        resident: hint_bool(&hints, "resident").unwrap_or(false),
        transient: hint_bool(&hints, "transient").unwrap_or(false),
        suppress_sound: hint_bool(&hints, "suppress-sound").unwrap_or(false),
        icon_name: image.icon_name,
        image_path: image.image_path,
        image_source: image.image_source,
        image_width: image.image_width,
        image_height: image.image_height,
        stack_key: stack_key(&hints),
        actions: normalize_actions(&local_id, actions, config),
    }
}

fn normalize_actions(
    notification_id: &str,
    actions: Vec<String>,
    config: &NotifydConfig,
) -> Vec<crate::state::NotificationAction> {
    if !config.actions {
        return Vec::new();
    }
    actions
        .chunks(2)
        .take(config.max_actions)
        .filter_map(|chunk| match chunk {
            [key, label] => Some(make_action(notification_id, key.clone(), label.clone())),
            _ => None,
        })
        .collect()
}

fn effective_timeout(
    expire_timeout: i32,
    urgency: NotificationUrgency,
    config: &NotifydConfig,
) -> i32 {
    if expire_timeout >= 0 {
        return expire_timeout;
    }
    match urgency {
        NotificationUrgency::Low => config.low_timeout_ms,
        NotificationUrgency::Normal => config.default_timeout_ms,
        NotificationUrgency::Critical => config.critical_timeout_ms,
    }
}

fn urgency(hints: &BTreeMap<String, OwnedValue>) -> NotificationUrgency {
    match hint_u8(hints, "urgency").unwrap_or(1) {
        0 => NotificationUrgency::Low,
        2 => NotificationUrgency::Critical,
        _ => NotificationUrgency::Normal,
    }
}

fn progress(hints: &BTreeMap<String, OwnedValue>) -> Option<u32> {
    hint_i32(hints, "value")
        .or_else(|| hint_i32(hints, "progress"))
        .map(|value| value.clamp(0, 100) as u32)
}

fn stack_key(hints: &BTreeMap<String, OwnedValue>) -> Option<String> {
    hint_string(hints, "x-dunst-stack-tag")
        .or_else(|| hint_string(hints, "x-canonical-private-synchronous"))
        .filter(|value| !value.trim().is_empty())
}

fn hint_string(values: &BTreeMap<String, OwnedValue>, key: &str) -> Option<String> {
    values
        .get(key)
        .and_then(|value| String::try_from(value.to_owned()).ok())
}

fn hint_bool(values: &BTreeMap<String, OwnedValue>, key: &str) -> Option<bool> {
    values
        .get(key)
        .and_then(|value| bool::try_from(value.to_owned()).ok())
}

fn hint_u8(values: &BTreeMap<String, OwnedValue>, key: &str) -> Option<u8> {
    values
        .get(key)
        .and_then(|value| u8::try_from(value.to_owned()).ok())
}

fn hint_i32(values: &BTreeMap<String, OwnedValue>, key: &str) -> Option<i32> {
    values
        .get(key)
        .and_then(|value| i32::try_from(value.to_owned()).ok())
}

fn trim_to_limit(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_owned();
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

async fn notification_name_available() -> Result<bool> {
    let connection = zbus::Connection::session()
        .await
        .map_err(|error| GraphError::Io(format!("connect to session D-Bus: {error}")))?;
    let dbus = DBusProxy::new(&connection)
        .await
        .map_err(|error| GraphError::Io(format!("create D-Bus proxy: {error}")))?;
    dbus.name_has_owner(bus_name(NOTIFICATIONS_SERVICE)?)
        .await
        .map(|owned| !owned)
        .map_err(|error| GraphError::Io(format!("check notification owner: {error}")))
}

async fn ensure_notification_name_available() -> Result<()> {
    if notification_name_available().await? {
        Ok(())
    } else {
        Err(GraphError::Io(format!(
            "{NOTIFICATIONS_SERVICE} is already owned"
        )))
    }
}

fn bus_name(name: &str) -> Result<BusName<'_>> {
    BusName::try_from(name).map_err(|_| GraphError::InvalidValue {
        kind: "D-Bus bus name",
        value: name.to_owned(),
        reason: "invalid bus name",
    })
}

fn well_known_name(name: &'static str) -> Result<WellKnownName<'static>> {
    WellKnownName::try_from(name).map_err(|_| GraphError::InvalidValue {
        kind: "D-Bus well-known name",
        value: name.to_owned(),
        reason: "invalid bus name",
    })
}

fn publish_changes(graph: &DynamicGraph, changes: Vec<GraphChange>) {
    for change in changes {
        if let Err(error) = graph.emit_global_change(change) {
            eprintln!("locusfs-notifyd: failed to emit graph change: {error}");
        }
    }
}

fn fdo_error(error: GraphError) -> zbus::fdo::Error {
    zbus::fdo::Error::Failed(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::{effective_timeout, trim_to_limit};
    use crate::config::NotifydConfig;
    use crate::state::NotificationUrgency;

    #[test]
    fn timeout_policy_uses_urgency_defaults() {
        let config = NotifydConfig::default();
        assert_eq!(
            effective_timeout(-1, NotificationUrgency::Low, &config),
            4000
        );
        assert_eq!(
            effective_timeout(-1, NotificationUrgency::Normal, &config),
            7000
        );
        assert_eq!(
            effective_timeout(-1, NotificationUrgency::Critical, &config),
            0
        );
        assert_eq!(
            effective_timeout(10, NotificationUrgency::Critical, &config),
            10
        );
    }

    #[test]
    fn trim_limit_preserves_utf8_boundaries() {
        assert_eq!(trim_to_limit("aé", 2), "a");
    }
}
