use futures_util::StreamExt;
use locusfs_graph::{DynamicGraph, GraphChange, GraphError, NodeKind, Result};
use tokio::task::JoinHandle;
use zbus::fdo::DBusProxy;
use zbus::names::BusName;

use crate::DBUS_SERVICE_KIND;
use crate::state::{DbusState, SharedDbusState, upower_service_name};

#[derive(Debug, Default)]
pub struct DbusRuntime;

impl DbusRuntime {
    pub fn start(graph: DynamicGraph) -> Result<(SharedDbusState, JoinHandle<()>)> {
        let state = DbusState::shared();
        let upower_watcher = spawn_upower_watcher(state.clone(), graph);
        Ok((state, upower_watcher))
    }
}

fn spawn_upower_watcher(state: SharedDbusState, graph: DynamicGraph) -> JoinHandle<()> {
    tokio::spawn(async move {
        if let Err(error) = watch_upower(state, graph).await {
            eprintln!("locusfs-dbus: UPower watcher stopped: {error}");
        }
    })
}

async fn watch_upower(state: SharedDbusState, graph: DynamicGraph) -> Result<()> {
    let connection = zbus::Connection::system()
        .await
        .map_err(|error| GraphError::Io(format!("connect to system D-Bus: {error}")))?;
    let dbus = DBusProxy::new(&connection)
        .await
        .map_err(|error| GraphError::Io(format!("create D-Bus proxy: {error}")))?;
    let mut owner_changed = dbus
        .receive_name_owner_changed()
        .await
        .map_err(|error| GraphError::Io(format!("watch NameOwnerChanged: {error}")))?;

    publish_owner(&state, &graph, current_owner(&dbus).await?).await;

    while let Some(signal) = owner_changed.next().await {
        let args = signal
            .args()
            .map_err(|error| GraphError::Io(format!("read NameOwnerChanged args: {error}")))?;
        if args.name().as_str() == upower_service_name() {
            let owner = args.new_owner().as_ref().map(ToString::to_string);
            publish_owner(&state, &graph, owner).await;
        }
    }

    Err(GraphError::Io(
        "D-Bus NameOwnerChanged stream ended".to_string(),
    ))
}

async fn current_owner(dbus: &DBusProxy<'_>) -> Result<Option<String>> {
    let service =
        BusName::try_from(upower_service_name()).map_err(|error| GraphError::InvalidValue {
            kind: "D-Bus service",
            value: upower_service_name().to_string(),
            reason: Box::leak(error.to_string().into_boxed_str()),
        })?;
    match dbus.get_name_owner(service).await {
        Ok(owner) => Ok(Some(owner.to_string())),
        Err(zbus::fdo::Error::NameHasNoOwner(_)) | Err(zbus::fdo::Error::ServiceUnknown(_)) => {
            Ok(None)
        }
        Err(error) => Err(GraphError::Io(format!("read UPower owner: {error}"))),
    }
}

async fn publish_owner(state: &SharedDbusState, graph: &DynamicGraph, owner: Option<String>) {
    let changes = {
        let mut state = state.write().await;
        match state.set_upower_owner(owner) {
            Ok(changes) => changes,
            Err(error) => {
                eprintln!("locusfs-dbus: failed to update UPower owner: {error}");
                return;
            }
        }
    };

    if changes.is_empty() {
        return;
    }

    emit_change(
        graph,
        GraphChange::NodeKindChanged {
            kind: match NodeKind::new(DBUS_SERVICE_KIND) {
                Ok(kind) => kind,
                Err(error) => {
                    eprintln!("locusfs-dbus: invalid node kind: {error}");
                    return;
                }
            },
        },
    );
    for change in changes {
        emit_change(graph, change);
    }
}

fn emit_change(graph: &DynamicGraph, change: GraphChange) {
    if let Err(error) = graph.emit_change(change) {
        eprintln!("locusfs-dbus: failed to emit graph change: {error}");
    }
}

#[cfg(test)]
mod test {
    use locusfs_graph::{DynamicGraph, LocusValue, PropertyKey};
    use tokio::time::{Duration, Instant, sleep};
    use zbus::fdo::DBusProxy;

    use super::current_owner;
    use crate::register;
    use crate::state::upower_node;

    #[tokio::test]
    async fn realtime_upower_owner_matches_system_bus_snapshot_when_available() {
        let expected_owner = match system_upower_owner().await {
            Ok(owner) => owner,
            Err(error) => {
                eprintln!("skipping realtime UPower D-Bus test: {error}");
                return;
            }
        };

        let graph = DynamicGraph::new();
        let _plugin = register(&graph).await.expect("dbus plugin registers");
        let node = upower_node().expect("upower node id is valid");
        let active = PropertyKey::new("active").expect("active property key is valid");
        let expected = LocusValue::Bool(expected_owner.is_some());
        let deadline = Instant::now() + Duration::from_secs(5);

        loop {
            if graph.property(&node, &active).await.ok() == Some(expected.clone()) {
                return;
            }

            assert!(
                Instant::now() < deadline,
                "timed out waiting for UPower active state {expected:?}"
            );
            sleep(Duration::from_millis(50)).await;
        }
    }

    async fn system_upower_owner() -> locusfs_graph::Result<Option<String>> {
        let connection = zbus::Connection::system()
            .await
            .map_err(|error| locusfs_graph::GraphError::Io(error.to_string()))?;
        let dbus = DBusProxy::new(&connection)
            .await
            .map_err(|error| locusfs_graph::GraphError::Io(error.to_string()))?;
        current_owner(&dbus).await
    }
}
