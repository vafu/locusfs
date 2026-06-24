use std::collections::BTreeMap;

use futures_util::StreamExt;
use locusfs_graph::{DynamicGraph, GraphChange, GraphError, Result};
use serde::Deserialize;
use tokio::runtime::Handle;
use tokio::task::JoinHandle;
use tokio::time::{Duration, sleep};
use zbus::names::InterfaceName;
use zbus::zvariant::{Array, Dict, OwnedObjectPath, OwnedValue, Structure, Type, Value};

use crate::state::{BusKind, DbusMenuEndpoint, DbusMenuItem, SharedDbusMenuState, menu_local_id};

const WATCHER_SERVICE: &str = "org.kde.StatusNotifierWatcher";
const WATCHER_PATH: &str = "/StatusNotifierWatcher";
const WATCHER_INTERFACE: &str = "org.kde.StatusNotifierWatcher";
const STATUS_ITEM_INTERFACE: &str = "org.kde.StatusNotifierItem";
const DBUSMENU_INTERFACE: &str = "com.canonical.dbusmenu";
const DEFAULT_ITEM_PATH: &str = "/StatusNotifierItem";

#[derive(Debug, Default)]
pub struct DbusMenuRuntime;

#[derive(Clone, Debug)]
struct LayoutItem(i32, BTreeMap<String, OwnedValue>, Vec<LayoutItem>);

#[derive(Clone, Debug, Deserialize, Type)]
struct RawLayoutItem(i32, BTreeMap<String, OwnedValue>, Vec<OwnedValue>);

impl DbusMenuRuntime {
    pub fn start(
        graph: DynamicGraph,
        runtime: Handle,
        state: SharedDbusMenuState,
    ) -> JoinHandle<()> {
        runtime.spawn(async move {
            loop {
                match watch_status_notifier_menus(&graph, &state).await {
                    Ok(()) => {}
                    Err(error) => eprintln!("locusfs-dbusmenu: menu watcher stopped: {error}"),
                }
                sleep(Duration::from_secs(1)).await;
            }
        })
    }
}

pub async fn activate_item(target: DbusMenuItem) -> Result<()> {
    let connection = connection_for_bus(target.bus).await?;
    let proxy = menu_proxy(&connection, &target.service, &target.path).await?;
    let event_data = Value::new("");
    proxy
        .call_noreply("Event", &(target.item_id, "clicked", event_data, 0_u32))
        .await
        .map_err(|error| {
            GraphError::Io(format!(
                "activate DBusMenu item {} on {}{}: {error}",
                target.item_id, target.service, target.path
            ))
        })
}

async fn watch_status_notifier_menus(
    graph: &DynamicGraph,
    state: &SharedDbusMenuState,
) -> Result<()> {
    let connection = zbus::Connection::session()
        .await
        .map_err(|error| GraphError::Io(format!("connect to session D-Bus: {error}")))?;
    let watcher = zbus::Proxy::new(
        &connection,
        WATCHER_SERVICE,
        WATCHER_PATH,
        WATCHER_INTERFACE,
    )
    .await
    .map_err(|error| GraphError::Io(format!("create StatusNotifierWatcher proxy: {error}")))?;

    refresh_registered_items(&connection, graph, state).await?;

    let mut registered = watcher
        .receive_signal("StatusNotifierItemRegistered")
        .await
        .map_err(|error| GraphError::Io(format!("watch StatusNotifierItemRegistered: {error}")))?;
    let mut unregistered = watcher
        .receive_signal("StatusNotifierItemUnregistered")
        .await
        .map_err(|error| {
            GraphError::Io(format!("watch StatusNotifierItemUnregistered: {error}"))
        })?;

    loop {
        tokio::select! {
            signal = registered.next() => {
                let Some(signal) = signal else {
                    return Err(GraphError::Io("StatusNotifierItemRegistered stream ended".to_string()));
                };
                let service = signal
                    .body()
                    .deserialize::<String>()
                    .map_err(|error| GraphError::Io(format!("read StatusNotifierItemRegistered: {error}")))?;
                refresh_item(&connection, graph, state, &service, DEFAULT_ITEM_PATH).await;
            }
            signal = unregistered.next() => {
                let Some(signal) = signal else {
                    return Err(GraphError::Io("StatusNotifierItemUnregistered stream ended".to_string()));
                };
                let service = signal
                    .body()
                    .deserialize::<String>()
                    .map_err(|error| GraphError::Io(format!("read StatusNotifierItemUnregistered: {error}")))?;
                remove_service_items(graph, state, &service).await;
            }
        }
    }
}

async fn refresh_registered_items(
    connection: &zbus::Connection,
    graph: &DynamicGraph,
    state: &SharedDbusMenuState,
) -> Result<()> {
    let watcher = zbus::Proxy::new(connection, WATCHER_SERVICE, WATCHER_PATH, WATCHER_INTERFACE)
        .await
        .map_err(|error| GraphError::Io(format!("create StatusNotifierWatcher proxy: {error}")))?;
    let items = watcher
        .get_property::<Vec<String>>("RegisteredStatusNotifierItems")
        .await
        .map_err(|error| GraphError::Io(format!("read RegisteredStatusNotifierItems: {error}")))?;
    for service in items {
        refresh_item(connection, graph, state, &service, DEFAULT_ITEM_PATH).await;
    }
    Ok(())
}

async fn refresh_item(
    connection: &zbus::Connection,
    graph: &DynamicGraph,
    state: &SharedDbusMenuState,
    service: &str,
    item_path: &str,
) {
    let result: Result<Option<DbusMenuEndpoint>> = async {
        let menu_path = status_item_menu_path(connection, service, item_path).await?;
        if menu_path.is_empty() {
            return Ok(None);
        }
        let endpoint = snapshot_menu(connection, service, &menu_path).await?;
        Ok(Some(endpoint))
    }
    .await;

    match result {
        Ok(Some(endpoint)) => publish_upsert(graph, state, endpoint).await,
        Ok(None) => {}
        Err(error) => eprintln!("locusfs-dbusmenu: failed to refresh menu for {service}: {error}"),
    }
}

async fn remove_service_items(graph: &DynamicGraph, state: &SharedDbusMenuState, service: &str) {
    let changes = {
        let mut state = state.write().await;
        match state.remove_service(service) {
            Ok(changes) => changes,
            Err(error) => {
                eprintln!("locusfs-dbusmenu: failed to remove menus for {service}: {error}");
                Vec::new()
            }
        }
    };
    publish_changes(graph, changes);
}

async fn publish_upsert(
    graph: &DynamicGraph,
    state: &SharedDbusMenuState,
    endpoint: DbusMenuEndpoint,
) {
    let changes = {
        let mut state = state.write().await;
        match state.upsert_menu(endpoint) {
            Ok(changes) => changes,
            Err(error) => {
                eprintln!("locusfs-dbusmenu: failed to update menu: {error}");
                Vec::new()
            }
        }
    };
    publish_changes(graph, changes);
}

fn publish_changes(graph: &DynamicGraph, changes: Vec<GraphChange>) {
    for change in changes {
        if let Err(error) = graph.emit_global_change(change) {
            eprintln!("locusfs-dbusmenu: failed to emit graph change: {error}");
        }
    }
}

async fn status_item_menu_path(
    connection: &zbus::Connection,
    service: &str,
    path: &str,
) -> Result<String> {
    let proxy = zbus::fdo::PropertiesProxy::builder(connection)
        .destination(service)
        .map_err(|error| GraphError::Io(format!("create StatusNotifier destination: {error}")))?
        .path(path)
        .map_err(|error| GraphError::Io(format!("create StatusNotifier path {path}: {error}")))?
        .build()
        .await
        .map_err(|error| {
            GraphError::Io(format!("create StatusNotifier properties proxy: {error}"))
        })?;
    let interface =
        InterfaceName::try_from(STATUS_ITEM_INTERFACE).map_err(|_| GraphError::InvalidValue {
            kind: "StatusNotifier interface",
            value: STATUS_ITEM_INTERFACE.to_string(),
            reason: "invalid interface name",
        })?;
    let value = proxy.get(interface, "Menu").await.map_err(|error| {
        GraphError::Io(format!("read StatusNotifier Menu for {service}: {error}"))
    })?;
    OwnedObjectPath::try_from(value)
        .map(|path| path.to_string())
        .map_err(|error| {
            GraphError::Io(format!("decode StatusNotifier Menu for {service}: {error}"))
        })
}

async fn snapshot_menu(
    connection: &zbus::Connection,
    service: &str,
    path: &str,
) -> Result<DbusMenuEndpoint> {
    let proxy = menu_proxy(connection, service, path).await?;
    let (revision, layout): (u32, RawLayoutItem) = proxy
        .call("GetLayout", &(0_i32, -1_i32, Vec::<&str>::new()))
        .await
        .map_err(|error| {
            GraphError::Io(format!("read DBusMenu layout for {service}{path}: {error}"))
        })?;
    let layout = parse_layout(layout)?;

    let local_id = menu_local_id(service, path);
    let mut endpoint = DbusMenuEndpoint::new(
        local_id,
        BusKind::Session,
        service.to_string(),
        path.to_string(),
    );
    endpoint.revision = revision;
    flatten_layout(&mut endpoint, None, 0, layout);
    Ok(endpoint)
}

fn flatten_layout(
    endpoint: &mut DbusMenuEndpoint,
    parent_id: Option<i32>,
    position: u32,
    item: LayoutItem,
) {
    let LayoutItem(item_id, properties, children) = item;
    let mut child_ids = Vec::new();
    for (position, child) in children.into_iter().enumerate() {
        child_ids.push(child.0);
        flatten_layout(endpoint, Some(item_id), position as u32, child);
    }

    if item_id == 0 {
        endpoint.root_items = child_ids;
        return;
    }

    endpoint.items.insert(
        item_id,
        DbusMenuItem {
            menu_id: endpoint.local_id.clone(),
            bus: endpoint.bus,
            service: endpoint.service.clone(),
            path: endpoint.path.clone(),
            item_id,
            parent_id,
            position,
            child_ids,
            label: owned_string(&properties, "label").unwrap_or_default(),
            enabled: owned_bool(&properties, "enabled").unwrap_or(true),
            visible: owned_bool(&properties, "visible").unwrap_or(true),
            item_type: owned_string(&properties, "type").unwrap_or_default(),
            toggle_type: owned_string(&properties, "toggle-type").unwrap_or_default(),
            toggle_state: owned_i32(&properties, "toggle-state").unwrap_or(-1),
            icon_name: owned_string(&properties, "icon-name").unwrap_or_default(),
            disposition: owned_string(&properties, "disposition").unwrap_or_default(),
        },
    );
}

fn parse_layout(item: RawLayoutItem) -> Result<LayoutItem> {
    let RawLayoutItem(item_id, properties, children) = item;
    Ok(LayoutItem(item_id, properties, parse_children(children)?))
}

fn parse_children(children: Vec<OwnedValue>) -> Result<Vec<LayoutItem>> {
    children
        .into_iter()
        .map(|value| {
            let child = raw_layout_from_value(value)?;
            parse_layout(child)
        })
        .collect()
}

fn raw_layout_from_value(value: OwnedValue) -> Result<RawLayoutItem> {
    let structure = Structure::try_from(value)
        .map_err(|error| invalid_layout(format!("invalid child structure: {error}")))?;
    raw_layout_from_structure(structure)
}

fn raw_layout_from_structure(structure: Structure<'_>) -> Result<RawLayoutItem> {
    let mut fields = structure.into_fields().into_iter();
    let item_id = fields
        .next()
        .ok_or_else(|| invalid_layout("missing item id"))?
        .try_into()
        .map_err(|error| invalid_layout(format!("invalid item id: {error}")))?;
    let properties = fields
        .next()
        .ok_or_else(|| invalid_layout("missing properties"))?
        .try_into()
        .map_err(|error| invalid_layout(format!("invalid properties: {error}")))?;
    let properties = parse_properties(properties)?;
    let children = fields
        .next()
        .ok_or_else(|| invalid_layout("missing children"))?
        .try_into()
        .map_err(|error| invalid_layout(format!("invalid children: {error}")))?;
    Ok(RawLayoutItem(
        item_id,
        properties,
        parse_child_values(children)?,
    ))
}

fn parse_properties(properties: Dict<'_, '_>) -> Result<BTreeMap<String, OwnedValue>> {
    properties
        .iter()
        .map(|(key, value)| {
            let key = String::try_from(key.try_clone().map_err(|error| {
                GraphError::Io(format!("clone DBusMenu property key: {error}"))
            })?)
            .map_err(|error| invalid_layout(format!("invalid property key: {error}")))?;
            let value = match value {
                Value::Value(value) => value.try_to_owned(),
                value => value.try_to_owned(),
            }
            .map_err(|error| GraphError::Io(format!("clone DBusMenu property value: {error}")))?;
            Ok((key, value))
        })
        .collect()
}

fn parse_child_values(children: Array<'_>) -> Result<Vec<OwnedValue>> {
    children
        .iter()
        .map(|value| {
            value
                .try_to_owned()
                .map_err(|error| GraphError::Io(format!("clone DBusMenu child value: {error}")))
        })
        .collect()
}

fn invalid_layout(reason: impl Into<String>) -> GraphError {
    GraphError::Io(format!("invalid DBusMenu layout: {}", reason.into()))
}

async fn menu_proxy<'a>(
    connection: &'a zbus::Connection,
    service: &'a str,
    path: &'a str,
) -> Result<zbus::Proxy<'a>> {
    zbus::Proxy::new(connection, service, path, DBUSMENU_INTERFACE)
        .await
        .map_err(|error| {
            GraphError::Io(format!(
                "create DBusMenu proxy for {service}{path}: {error}"
            ))
        })
}

async fn connection_for_bus(bus: BusKind) -> Result<zbus::Connection> {
    match bus {
        BusKind::Session => zbus::Connection::session()
            .await
            .map_err(|error| GraphError::Io(format!("connect to session D-Bus: {error}"))),
        BusKind::System => zbus::Connection::system()
            .await
            .map_err(|error| GraphError::Io(format!("connect to system D-Bus: {error}"))),
    }
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

fn owned_i32(values: &BTreeMap<String, OwnedValue>, key: &str) -> Option<i32> {
    values
        .get(key)
        .and_then(|value| i32::try_from(value.to_owned()).ok())
}
