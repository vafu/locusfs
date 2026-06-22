use std::collections::{BTreeMap, BTreeSet, HashMap};

use futures_util::StreamExt;
use locusfs_graph::{DynamicGraph, GraphChange, GraphError, Result};
use locusfs_plugin_api::enter_runtime;
use tokio::runtime::Handle;
use tokio::task::JoinHandle;
use tokio::time::{Duration, sleep};
use zbus::fdo::{DBusProxy, PropertiesProxy};
use zbus::names::{BusName, InterfaceName};
use zbus::zvariant::OwnedValue;

use crate::state::{
    MprisPlayer, SharedMprisState, player_id_from_service, playerctl_name_from_service,
};

const MPRIS_PREFIX: &str = "org.mpris.MediaPlayer2.";
const MPRIS_ROOT_INTERFACE: &str = "org.mpris.MediaPlayer2";
const MPRIS_PLAYER_INTERFACE: &str = "org.mpris.MediaPlayer2.Player";
const MPRIS_OBJECT_PATH: &str = "/org/mpris/MediaPlayer2";

#[derive(Debug, Default)]
pub struct MprisRuntime;

impl MprisRuntime {
    pub fn start(graph: DynamicGraph, runtime: Handle) -> (SharedMprisState, JoinHandle<()>) {
        let state = crate::state::MprisState::shared();
        let task_runtime = runtime.clone();
        let watcher_runtime = runtime.clone();
        let task_state = state.clone();
        let task = runtime.spawn(enter_runtime(task_runtime, async move {
            run_mpris_watcher(task_state, graph, watcher_runtime).await;
        }));
        (state, task)
    }
}

async fn run_mpris_watcher(state: SharedMprisState, graph: DynamicGraph, runtime: Handle) {
    loop {
        match zbus::Connection::session().await {
            Ok(connection) => {
                if let Err(error) =
                    watch_mpris_bus(connection, state.clone(), graph.clone(), runtime.clone()).await
                {
                    eprintln!("locusfs-mpris: session watcher stopped: {error}");
                }
            }
            Err(error) => {
                eprintln!("locusfs-mpris: failed to connect to session D-Bus: {error}");
            }
        }
        sleep_retry().await;
    }
}

async fn watch_mpris_bus(
    connection: zbus::Connection,
    state: SharedMprisState,
    graph: DynamicGraph,
    runtime: Handle,
) -> Result<()> {
    let dbus = DBusProxy::new(&connection)
        .await
        .map_err(|error| GraphError::Io(format!("create D-Bus proxy: {error}")))?;
    let mut owner_changed = dbus
        .receive_name_owner_changed()
        .await
        .map_err(|error| GraphError::Io(format!("watch NameOwnerChanged: {error}")))?;
    let mut players = BTreeMap::new();

    reconcile_players(&connection, &dbus, &state, &graph, &runtime, &mut players).await?;

    while let Some(signal) = owner_changed.next().await {
        let args = signal
            .args()
            .map_err(|error| GraphError::Io(format!("read NameOwnerChanged args: {error}")))?;
        if args.name().as_str().starts_with(MPRIS_PREFIX) {
            reconcile_players(&connection, &dbus, &state, &graph, &runtime, &mut players).await?;
        }
    }

    Err(GraphError::Io(
        "D-Bus NameOwnerChanged stream ended".to_string(),
    ))
}

async fn reconcile_players(
    connection: &zbus::Connection,
    dbus: &DBusProxy<'_>,
    state: &SharedMprisState,
    graph: &DynamicGraph,
    runtime: &Handle,
    watchers: &mut BTreeMap<String, JoinHandle<()>>,
) -> Result<()> {
    let current = dbus
        .list_names()
        .await
        .map_err(|error| GraphError::Io(format!("list D-Bus names: {error}")))?
        .into_iter()
        .map(|name| name.to_string())
        .filter(|name| name.starts_with(MPRIS_PREFIX))
        .collect::<BTreeSet<_>>();

    let watched = watchers.keys().cloned().collect::<BTreeSet<_>>();

    for service_name in watched.difference(&current) {
        if let Some(task) = watchers.remove(service_name) {
            task.abort();
        }
        let id = player_id_from_service(service_name);
        publish_player_removed(state, graph, &id).await;
    }

    for service_name in current.difference(&watched) {
        let task = spawn_player_watcher(
            connection.clone(),
            service_name.clone(),
            state.clone(),
            graph.clone(),
            runtime.clone(),
        );
        watchers.insert(service_name.clone(), task);
    }

    Ok(())
}

fn spawn_player_watcher(
    connection: zbus::Connection,
    service_name: String,
    state: SharedMprisState,
    graph: DynamicGraph,
    runtime: Handle,
) -> JoinHandle<()> {
    let task_runtime = runtime.clone();
    runtime.spawn(enter_runtime(task_runtime, async move {
        if let Err(error) = watch_player(connection, service_name.clone(), state, graph).await {
            eprintln!("locusfs-mpris: player watcher for {service_name} stopped: {error}");
        }
    }))
}

async fn watch_player(
    connection: zbus::Connection,
    service_name: String,
    state: SharedMprisState,
    graph: DynamicGraph,
) -> Result<()> {
    let mut properties_changed = player_properties_proxy(&connection, &service_name)
        .await?
        .receive_properties_changed()
        .await
        .map_err(|error| {
            GraphError::Io(format!(
                "watch MPRIS properties for {service_name}: {error}"
            ))
        })?;

    publish_player(
        &state,
        &graph,
        snapshot_player(&connection, &service_name).await?,
    )
    .await;

    while let Some(signal) = properties_changed.next().await {
        let args = signal
            .args()
            .map_err(|error| GraphError::Io(format!("read MPRIS PropertiesChanged: {error}")))?;
        let interface = args.interface_name().as_str();
        if interface == MPRIS_ROOT_INTERFACE || interface == MPRIS_PLAYER_INTERFACE {
            publish_player(
                &state,
                &graph,
                snapshot_player(&connection, &service_name).await?,
            )
            .await;
        }
    }

    Err(GraphError::Io(format!(
        "MPRIS property stream ended for {service_name}"
    )))
}

async fn snapshot_player(connection: &zbus::Connection, service_name: &str) -> Result<MprisPlayer> {
    let root_proxy = root_properties_proxy(connection, service_name).await?;
    let player_proxy = player_properties_proxy(connection, service_name).await?;

    let root_properties = get_all(&root_proxy, MPRIS_ROOT_INTERFACE, service_name).await?;
    let player_properties = get_all(&player_proxy, MPRIS_PLAYER_INTERFACE, service_name).await?;
    let metadata = owned_map(&player_properties, "Metadata").unwrap_or_default();

    Ok(MprisPlayer {
        id: player_id_from_service(service_name),
        service_name: service_name.to_string(),
        playerctl_name: playerctl_name_from_service(service_name),
        identity: owned_string(&root_properties, "Identity").unwrap_or_default(),
        artist: owned_vec_string(&metadata, "xesam:artist")
            .unwrap_or_default()
            .join(", "),
        title: owned_string(&metadata, "xesam:title").unwrap_or_default(),
        album: owned_string(&metadata, "xesam:album").unwrap_or_default(),
        art_url: owned_string(&metadata, "mpris:artUrl").unwrap_or_default(),
        playback_status: owned_string(&player_properties, "PlaybackStatus").unwrap_or_default(),
        can_play: owned_bool(&player_properties, "CanPlay").unwrap_or(false),
        can_pause: owned_bool(&player_properties, "CanPause").unwrap_or(false),
        can_go_next: owned_bool(&player_properties, "CanGoNext").unwrap_or(false),
        can_go_previous: owned_bool(&player_properties, "CanGoPrevious").unwrap_or(false),
        length_us: owned_i64(&metadata, "mpris:length"),
    })
}

async fn root_properties_proxy<'a>(
    connection: &'a zbus::Connection,
    service_name: &'a str,
) -> Result<PropertiesProxy<'a>> {
    properties_proxy(connection, service_name).await
}

async fn player_properties_proxy<'a>(
    connection: &'a zbus::Connection,
    service_name: &'a str,
) -> Result<PropertiesProxy<'a>> {
    properties_proxy(connection, service_name).await
}

async fn properties_proxy<'a>(
    connection: &'a zbus::Connection,
    service_name: &'a str,
) -> Result<PropertiesProxy<'a>> {
    let destination = BusName::try_from(service_name).map_err(|_| GraphError::InvalidValue {
        kind: "MPRIS service",
        value: service_name.to_string(),
        reason: "invalid bus name",
    })?;
    PropertiesProxy::builder(connection)
        .destination(destination)
        .map_err(|error| GraphError::Io(format!("create MPRIS destination: {error}")))?
        .path(MPRIS_OBJECT_PATH)
        .map_err(|error| GraphError::Io(format!("create MPRIS object path: {error}")))?
        .build()
        .await
        .map_err(|error| GraphError::Io(format!("create MPRIS properties proxy: {error}")))
}

async fn get_all(
    proxy: &PropertiesProxy<'_>,
    interface: &str,
    service_name: &str,
) -> Result<BTreeMap<String, OwnedValue>> {
    let interface_name = interface.to_string();
    let interface = InterfaceName::try_from(interface).map_err(|_| GraphError::InvalidValue {
        kind: "MPRIS interface",
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
                "read MPRIS properties for {service_name} {interface_name}: {error}"
            ))
        })
}

async fn publish_player(state: &SharedMprisState, graph: &DynamicGraph, player: MprisPlayer) {
    let changes = {
        let mut state = state.write().await;
        match state.upsert_player(player) {
            Ok(changes) => changes,
            Err(error) => {
                eprintln!("locusfs-mpris: failed to apply player snapshot: {error}");
                Vec::new()
            }
        }
    };
    publish_changes(graph, changes);
}

async fn publish_player_removed(state: &SharedMprisState, graph: &DynamicGraph, id: &str) {
    let changes = {
        let mut state = state.write().await;
        match state.remove_player(id) {
            Ok(changes) => changes,
            Err(error) => {
                eprintln!("locusfs-mpris: failed to remove player {id}: {error}");
                Vec::new()
            }
        }
    };
    publish_changes(graph, changes);
}

fn publish_changes(graph: &DynamicGraph, changes: Vec<GraphChange>) {
    for change in changes {
        if let Err(error) = graph.emit_global_change(change) {
            eprintln!("locusfs-mpris: failed to emit graph change: {error}");
        }
    }
}

fn owned_string(values: &BTreeMap<String, OwnedValue>, key: &str) -> Option<String> {
    values
        .get(key)
        .and_then(|value| String::try_from(value.to_owned()).ok())
}

fn owned_vec_string(values: &BTreeMap<String, OwnedValue>, key: &str) -> Option<Vec<String>> {
    values
        .get(key)
        .and_then(|value| Vec::<String>::try_from(value.to_owned()).ok())
}

fn owned_bool(values: &BTreeMap<String, OwnedValue>, key: &str) -> Option<bool> {
    values
        .get(key)
        .and_then(|value| bool::try_from(value.to_owned()).ok())
}

fn owned_i64(values: &BTreeMap<String, OwnedValue>, key: &str) -> Option<i64> {
    values
        .get(key)
        .and_then(|value| i64::try_from(value.to_owned()).ok())
}

fn owned_map(
    values: &BTreeMap<String, OwnedValue>,
    key: &str,
) -> Option<BTreeMap<String, OwnedValue>> {
    values
        .get(key)
        .and_then(|value| HashMap::<String, OwnedValue>::try_from(value.to_owned()).ok())
        .map(|values| values.into_iter().collect())
}

async fn sleep_retry() {
    sleep(Duration::from_secs(1)).await;
}
