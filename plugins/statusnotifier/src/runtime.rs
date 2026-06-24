use std::collections::BTreeMap;

use futures_util::StreamExt;
use locusfs_graph::{DynamicGraph, GraphChange, GraphError, Result};
use tokio::runtime::Handle;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::{Duration, sleep};
use zbus::fdo::{DBusProxy, PropertiesProxy};
use zbus::message::Header;
use zbus::names::{BusName, InterfaceName, WellKnownName};
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue};

use crate::state::{SharedStatusNotifierState, StatusNotifierItem, item_id};

const WATCHER_SERVICE: &str = "org.kde.StatusNotifierWatcher";
const XAPP_WATCHER_SERVICE: &str = "org.x.StatusNotifierWatcher";
const WATCHER_PATH: &str = "/StatusNotifierWatcher";
const ITEM_INTERFACE: &str = "org.kde.StatusNotifierItem";
const DEFAULT_ITEM_PATH: &str = "/StatusNotifierItem";
const ITEM_PREFIX: &str = "org.kde.StatusNotifierItem-";

#[derive(Debug, Default)]
pub struct StatusNotifierRuntime;

impl StatusNotifierRuntime {
    pub fn start(
        graph: DynamicGraph,
        runtime: Handle,
    ) -> (SharedStatusNotifierState, JoinHandle<()>) {
        let state = crate::state::StatusNotifierState::shared();
        let task_state = state.clone();
        let watcher_runtime = runtime.clone();
        let task = runtime.spawn(async move {
            run_status_notifier_watcher(task_state, graph, watcher_runtime).await;
        });
        (state, task)
    }
}

#[derive(Debug)]
enum WatcherCommand {
    RegisterItem {
        service_or_path: String,
        sender: String,
    },
    RegisterHost,
}

#[derive(Debug)]
struct WatchedItem {
    service_name: String,
    task: JoinHandle<()>,
}

#[derive(Clone)]
struct StatusNotifierWatcher {
    commands: mpsc::UnboundedSender<WatcherCommand>,
    state: SharedStatusNotifierState,
}

#[zbus::interface(name = "org.kde.StatusNotifierWatcher")]
impl StatusNotifierWatcher {
    async fn register_status_notifier_item(
        &self,
        service: &str,
        #[zbus(header)] header: Header<'_>,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> zbus::fdo::Result<()> {
        let sender = header.sender().map(ToString::to_string).unwrap_or_default();
        self.commands
            .send(WatcherCommand::RegisterItem {
                service_or_path: service.to_owned(),
                sender,
            })
            .map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?;
        Self::status_notifier_item_registered(&emitter, service).await?;
        Ok(())
    }

    async fn register_status_notifier_host(
        &self,
        _service: &str,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> zbus::fdo::Result<()> {
        self.commands
            .send(WatcherCommand::RegisterHost)
            .map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?;
        Self::status_notifier_host_registered(&emitter).await?;
        Ok(())
    }

    #[zbus(property)]
    async fn registered_status_notifier_items(&self) -> Vec<String> {
        self.state.read().await.registered_items()
    }

    #[zbus(property)]
    fn is_status_notifier_host_registered(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn protocol_version(&self) -> i32 {
        0
    }

    #[zbus(signal)]
    async fn status_notifier_item_registered(
        signal_emitter: &SignalEmitter<'_>,
        service: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn status_notifier_item_unregistered(
        signal_emitter: &SignalEmitter<'_>,
        service: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn status_notifier_host_registered(
        signal_emitter: &SignalEmitter<'_>,
    ) -> zbus::Result<()>;
}

async fn run_status_notifier_watcher(
    state: SharedStatusNotifierState,
    graph: DynamicGraph,
    runtime: Handle,
) {
    loop {
        match watch_status_notifier_bus(state.clone(), graph.clone(), runtime.clone()).await {
            Ok(()) => {}
            Err(error) => eprintln!("locusfs-statusnotifier: watcher stopped: {error}"),
        }
        sleep_retry().await;
    }
}

async fn watch_status_notifier_bus(
    state: SharedStatusNotifierState,
    graph: DynamicGraph,
    runtime: Handle,
) -> Result<()> {
    if !watcher_name_available().await? {
        return Ok(());
    }

    let (commands_tx, mut commands_rx) = mpsc::unbounded_channel();
    let watcher = StatusNotifierWatcher {
        commands: commands_tx,
        state: state.clone(),
    };
    let connection = zbus::connection::Builder::session()
        .map_err(|error| GraphError::Io(format!("create session connection: {error}")))?
        .serve_at(WATCHER_PATH, watcher)
        .map_err(|error| GraphError::Io(format!("serve StatusNotifierWatcher: {error}")))?
        .name(
            WellKnownName::try_from(WATCHER_SERVICE).map_err(|_| GraphError::InvalidValue {
                kind: "StatusNotifierWatcher service",
                value: WATCHER_SERVICE.to_string(),
                reason: "invalid service name",
            })?,
        )
        .map_err(|error| GraphError::Io(format!("own StatusNotifierWatcher name: {error}")))?
        .build()
        .await
        .map_err(|error| GraphError::Io(format!("connect StatusNotifierWatcher: {error}")))?;

    if let Err(error) = connection.request_name(XAPP_WATCHER_SERVICE).await {
        eprintln!("locusfs-statusnotifier: failed to own {XAPP_WATCHER_SERVICE}: {error}");
    }

    let dbus = DBusProxy::new(&connection)
        .await
        .map_err(|error| GraphError::Io(format!("create D-Bus proxy: {error}")))?;
    let mut owner_changed = dbus
        .receive_name_owner_changed()
        .await
        .map_err(|error| GraphError::Io(format!("watch NameOwnerChanged: {error}")))?;
    let mut items = BTreeMap::<String, WatchedItem>::new();

    reconcile_passive_items(&connection, &dbus, &state, &graph, &runtime, &mut items).await?;

    loop {
        tokio::select! {
            command = commands_rx.recv() => {
                let Some(command) = command else {
                    return Err(GraphError::Io("StatusNotifier command stream ended".to_string()));
                };
                match command {
                    WatcherCommand::RegisterItem { service_or_path, sender } => {
                        if let Some((service_name, path)) = registered_item_target(&service_or_path, &sender) {
                            watch_item_if_needed(
                                &connection,
                                &service_name,
                                &path,
                                &state,
                                &graph,
                                &runtime,
                                &mut items,
                            ).await;
                        }
                    }
                    WatcherCommand::RegisterHost => {}
                }
            }
            signal = owner_changed.next() => {
                let Some(signal) = signal else {
                    return Err(GraphError::Io("D-Bus NameOwnerChanged stream ended".to_string()));
                };
                let args = signal
                    .args()
                    .map_err(|error| GraphError::Io(format!("read NameOwnerChanged args: {error}")))?;
                let name = args.name().as_str().to_owned();
                if args.new_owner().as_ref().is_none() {
                    remove_items_for_service(&state, &graph, &mut items, &name).await;
                } else if name.starts_with(ITEM_PREFIX) {
                    reconcile_passive_items(&connection, &dbus, &state, &graph, &runtime, &mut items).await?;
                }
            }
        }
    }
}

async fn watcher_name_available() -> Result<bool> {
    let connection = zbus::Connection::session()
        .await
        .map_err(|error| GraphError::Io(format!("connect to session D-Bus: {error}")))?;
    let dbus = DBusProxy::new(&connection)
        .await
        .map_err(|error| GraphError::Io(format!("create D-Bus proxy: {error}")))?;
    let name = BusName::try_from(WATCHER_SERVICE).map_err(|_| GraphError::InvalidValue {
        kind: "StatusNotifierWatcher service",
        value: WATCHER_SERVICE.to_string(),
        reason: "invalid service name",
    })?;
    dbus.name_has_owner(name)
        .await
        .map(|owned| !owned)
        .map_err(|error| GraphError::Io(format!("check StatusNotifierWatcher owner: {error}")))
}

async fn reconcile_passive_items(
    connection: &zbus::Connection,
    dbus: &DBusProxy<'_>,
    state: &SharedStatusNotifierState,
    graph: &DynamicGraph,
    runtime: &Handle,
    watchers: &mut BTreeMap<String, WatchedItem>,
) -> Result<()> {
    let names = dbus
        .list_names()
        .await
        .map_err(|error| GraphError::Io(format!("list D-Bus names: {error}")))?;
    for name in names {
        let name = name.to_string();
        if name.starts_with(ITEM_PREFIX) {
            watch_item_if_needed(
                connection,
                &name,
                DEFAULT_ITEM_PATH,
                state,
                graph,
                runtime,
                watchers,
            )
            .await;
        }
    }
    Ok(())
}

async fn watch_item_if_needed(
    connection: &zbus::Connection,
    service_name: &str,
    path: &str,
    state: &SharedStatusNotifierState,
    graph: &DynamicGraph,
    runtime: &Handle,
    watchers: &mut BTreeMap<String, WatchedItem>,
) {
    let id = item_id(service_name, path);
    if watchers.contains_key(&id) {
        return;
    }

    let task = spawn_item_watcher(
        connection.clone(),
        service_name.to_owned(),
        path.to_owned(),
        state.clone(),
        graph.clone(),
        runtime.clone(),
    );
    watchers.insert(
        id,
        WatchedItem {
            service_name: service_name.to_owned(),
            task,
        },
    );
}

fn spawn_item_watcher(
    connection: zbus::Connection,
    service_name: String,
    path: String,
    state: SharedStatusNotifierState,
    graph: DynamicGraph,
    runtime: Handle,
) -> JoinHandle<()> {
    runtime.spawn(async move {
        let id = item_id(&service_name, &path);
        if let Err(error) = watch_item(
            connection,
            service_name.clone(),
            path.clone(),
            state.clone(),
            graph.clone(),
        )
        .await
        {
            eprintln!(
                "locusfs-statusnotifier: item watcher for {service_name}{path} stopped: {error}"
            );
        }
        publish_item_removed(&state, &graph, &id).await;
    })
}

async fn watch_item(
    connection: zbus::Connection,
    service_name: String,
    path: String,
    state: SharedStatusNotifierState,
    graph: DynamicGraph,
) -> Result<()> {
    let mut properties_changed = item_properties_proxy(&connection, &service_name, &path)
        .await?
        .receive_properties_changed()
        .await
        .map_err(|error| {
            GraphError::Io(format!(
                "watch StatusNotifier properties for {service_name}{path}: {error}"
            ))
        })?;

    publish_item(
        &state,
        &graph,
        snapshot_item(&connection, &service_name, &path).await?,
    )
    .await;

    while let Some(signal) = properties_changed.next().await {
        let args = signal.args().map_err(|error| {
            GraphError::Io(format!("read StatusNotifier PropertiesChanged: {error}"))
        })?;
        if args.interface_name().as_str() == ITEM_INTERFACE {
            publish_item(
                &state,
                &graph,
                snapshot_item(&connection, &service_name, &path).await?,
            )
            .await;
        }
    }

    Err(GraphError::Io(format!(
        "StatusNotifier property stream ended for {service_name}{path}"
    )))
}

async fn snapshot_item(
    connection: &zbus::Connection,
    service_name: &str,
    path: &str,
) -> Result<StatusNotifierItem> {
    let proxy = item_properties_proxy(connection, service_name, path).await?;
    let properties = get_all(&proxy, ITEM_INTERFACE, service_name, path).await?;
    Ok(StatusNotifierItem {
        id: item_id(service_name, path),
        service_name: service_name.to_owned(),
        path: path.to_owned(),
        category: owned_string(&properties, "Category").unwrap_or_default(),
        title: owned_string(&properties, "Title").unwrap_or_default(),
        status: owned_string(&properties, "Status").unwrap_or_default(),
        icon_name: owned_string(&properties, "IconName").unwrap_or_default(),
        attention_icon_name: owned_string(&properties, "AttentionIconName").unwrap_or_default(),
        overlay_icon_name: owned_string(&properties, "OverlayIconName").unwrap_or_default(),
        menu_path: owned_object_path(&properties, "Menu").unwrap_or_default(),
        item_is_menu: owned_bool(&properties, "ItemIsMenu").unwrap_or(false),
    })
}

async fn item_properties_proxy<'a>(
    connection: &'a zbus::Connection,
    service_name: &'a str,
    path: &'a str,
) -> Result<PropertiesProxy<'a>> {
    let destination = BusName::try_from(service_name).map_err(|_| GraphError::InvalidValue {
        kind: "StatusNotifier service",
        value: service_name.to_owned(),
        reason: "invalid bus name",
    })?;
    let path = ObjectPath::try_from(path).map_err(|_| GraphError::InvalidValue {
        kind: "StatusNotifier object path",
        value: path.to_owned(),
        reason: "invalid object path",
    })?;
    PropertiesProxy::builder(connection)
        .destination(destination)
        .map_err(|error| GraphError::Io(format!("create StatusNotifier destination: {error}")))?
        .path(path)
        .map_err(|error| GraphError::Io(format!("create StatusNotifier object path: {error}")))?
        .build()
        .await
        .map_err(|error| GraphError::Io(format!("create StatusNotifier properties proxy: {error}")))
}

async fn get_all(
    proxy: &PropertiesProxy<'_>,
    interface: &str,
    service_name: &str,
    path: &str,
) -> Result<BTreeMap<String, OwnedValue>> {
    let interface_name = interface.to_owned();
    let interface = InterfaceName::try_from(interface).map_err(|_| GraphError::InvalidValue {
        kind: "StatusNotifier interface",
        value: interface_name.clone(),
        reason: "invalid interface name",
    })?;
    proxy
        .get_all(interface)
        .await
        .map(|properties| {
            properties
                .into_iter()
                .map(|(key, value)| (key.to_string(), value))
                .collect()
        })
        .map_err(|error| {
            GraphError::Io(format!(
                "read StatusNotifier properties for {service_name}{path} {interface_name}: {error}"
            ))
        })
}

async fn remove_items_for_service(
    state: &SharedStatusNotifierState,
    graph: &DynamicGraph,
    watchers: &mut BTreeMap<String, WatchedItem>,
    service_name: &str,
) {
    let ids = watchers
        .iter()
        .filter_map(|(id, item)| (item.service_name == service_name).then_some(id.clone()))
        .collect::<Vec<_>>();
    for id in ids {
        if let Some(item) = watchers.remove(&id) {
            item.task.abort();
        }
        publish_item_removed(state, graph, &id).await;
    }
}

async fn publish_item(
    state: &SharedStatusNotifierState,
    graph: &DynamicGraph,
    item: StatusNotifierItem,
) {
    let changes = {
        let mut state = state.write().await;
        match state.upsert_item(item) {
            Ok(changes) => changes,
            Err(error) => {
                eprintln!("locusfs-statusnotifier: failed to apply item snapshot: {error}");
                Vec::new()
            }
        }
    };
    publish_changes(graph, changes);
}

async fn publish_item_removed(state: &SharedStatusNotifierState, graph: &DynamicGraph, id: &str) {
    let changes = {
        let mut state = state.write().await;
        match state.remove_item(id) {
            Ok(changes) => changes,
            Err(error) => {
                eprintln!("locusfs-statusnotifier: failed to remove item {id}: {error}");
                Vec::new()
            }
        }
    };
    publish_changes(graph, changes);
}

fn publish_changes(graph: &DynamicGraph, changes: Vec<GraphChange>) {
    for change in changes {
        if let Err(error) = graph.emit_global_change(change) {
            eprintln!("locusfs-statusnotifier: failed to emit graph change: {error}");
        }
    }
}

fn registered_item_target(service_or_path: &str, sender: &str) -> Option<(String, String)> {
    if service_or_path.starts_with('/') {
        if sender.is_empty() {
            return None;
        }
        return Some((sender.to_owned(), service_or_path.to_owned()));
    }

    Some((service_or_path.to_owned(), DEFAULT_ITEM_PATH.to_owned()))
}

fn owned_string(values: &BTreeMap<String, OwnedValue>, key: &str) -> Option<String> {
    values
        .get(key)
        .and_then(|value| String::try_from(value.to_owned()).ok())
}

fn owned_bool(values: &BTreeMap<String, OwnedValue>, key: &str) -> Option<bool> {
    values
        .get(key)
        .and_then(|value| bool::try_from(value.to_owned()).ok())
}

fn owned_object_path(values: &BTreeMap<String, OwnedValue>, key: &str) -> Option<String> {
    values
        .get(key)
        .and_then(|value| OwnedObjectPath::try_from(value.to_owned()).ok())
        .map(|path| path.to_string())
}

async fn sleep_retry() {
    sleep(Duration::from_secs(1)).await;
}
