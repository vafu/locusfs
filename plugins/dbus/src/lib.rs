//! D-Bus graph provider for `locusfs`.

mod provider;
mod runtime;
mod state;

pub use provider::DbusProvider;

use locusfs_graph::{DynamicGraph, NodeKind, Result, TracedProvider};
use tokio::task::JoinHandle;

use crate::runtime::DbusRuntime;

pub const DBUS_SERVICE_KIND: &str = "dbus-service";
pub const DBUS_OBJECT_KIND: &str = "dbus-object";

const PROVIDER_NAME: &str = "dbus";

/// Registers read-only D-Bus service providers on the graph.
#[derive(Debug)]
pub struct DbusPluginHandle {
    _watchers: Vec<JoinHandle<()>>,
}

pub async fn register(graph: &DynamicGraph) -> Result<DbusPluginHandle> {
    let (state, watchers) = DbusRuntime::start(graph.clone())?;
    for kind_name in [DBUS_SERVICE_KIND, DBUS_OBJECT_KIND] {
        let kind = NodeKind::new(kind_name)?;
        let provider = DbusProvider::new(kind.clone(), state.clone());
        let provider = TracedProvider::new(PROVIDER_NAME, provider);

        graph.register_node_provider(provider.clone()).await?;
        graph
            .register_property_provider(kind.clone(), provider.clone())
            .await?;
        graph.register_relation_provider(kind, provider).await?;
    }
    Ok(DbusPluginHandle {
        _watchers: watchers,
    })
}
