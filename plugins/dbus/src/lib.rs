//! D-Bus graph provider for `locusfs`.
//!
//! The first version is intentionally hardcoded to expose UPower service
//! ownership. A config-backed service list can replace `WATCHED_SERVICES`
//! without changing the provider traits.

mod provider;
mod runtime;
mod state;

pub use provider::DbusProvider;

use locusfs_graph::{DynamicGraph, NodeKind, Result, TracedProvider};
use tokio::task::JoinHandle;

use crate::runtime::DbusRuntime;

pub const DBUS_SERVICE_KIND: &str = "dbus-service";

const PROVIDER_NAME: &str = "dbus";

/// Registers read-only D-Bus service providers on the graph.
#[derive(Debug)]
pub struct DbusPluginHandle {
    _upower_watcher: JoinHandle<()>,
}

pub async fn register(graph: &DynamicGraph) -> Result<DbusPluginHandle> {
    let kind = NodeKind::new(DBUS_SERVICE_KIND)?;
    let (state, upower_watcher) = DbusRuntime::start(graph.clone())?;
    let provider = DbusProvider::new(kind.clone(), state);
    let provider = TracedProvider::new(PROVIDER_NAME, provider);

    graph.register_node_provider(provider.clone()).await?;
    graph.register_property_provider(kind, provider).await?;
    Ok(DbusPluginHandle {
        _upower_watcher: upower_watcher,
    })
}
