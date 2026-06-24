//! D-Bus graph provider for `locusfs`.

pub mod config;
mod provider;
mod runtime;
mod state;

pub use provider::DbusProvider;

use async_trait::async_trait;
use locusfs_graph::{DynamicGraph, GraphError, NodeKind, Result, TracedProvider};
use locusfs_plugin_api::{
    LocusFsPlugin, PluginContext, PluginHandle, PluginManifest, PluginRuntime,
};
use tokio::task::JoinHandle;

use crate::config::DbusConfig;
use crate::runtime::DbusRuntime;

pub const DBUS_SERVICE_KIND: &str = "dbus";
pub const DBUS_OBJECT_KIND: &str = "dbus-object";
pub const DBUS_METHOD_KIND: &str = "dbus-method";

const PROVIDER_NAME: &str = "dbus";

#[derive(Debug, Default)]
pub struct DbusPlugin;

/// Registers D-Bus service providers on the graph.
#[derive(Debug)]
pub struct DbusPluginHandle {
    _watchers: Vec<JoinHandle<()>>,
    _runtime: PluginRuntime,
}

impl Drop for DbusPluginHandle {
    fn drop(&mut self) {
        for watcher in &self._watchers {
            watcher.abort();
        }
    }
}

#[async_trait]
impl PluginHandle for DbusPluginHandle {
    async fn shutdown(mut self: Box<Self>) {
        for watcher in self._watchers.drain(..) {
            watcher.abort();
            let _ = watcher.await;
        }
    }
}

pub async fn register(graph: &DynamicGraph) -> Result<DbusPluginHandle> {
    register_with_config(graph, DbusConfig::default()).await
}

pub async fn register_with_config(
    graph: &DynamicGraph,
    config: DbusConfig,
) -> Result<DbusPluginHandle> {
    register_with_config_and_runtime(graph, config).await
}

async fn register_with_config_and_runtime(
    graph: &DynamicGraph,
    config: DbusConfig,
) -> Result<DbusPluginHandle> {
    let runtime = PluginRuntime::new("locusfs-dbus")?;
    let runtime_handle = runtime.handle();
    let (state, watchers) = DbusRuntime::start(graph.clone(), config, runtime_handle.clone())?;
    for kind_name in [DBUS_SERVICE_KIND, DBUS_OBJECT_KIND, DBUS_METHOD_KIND] {
        let kind = NodeKind::new(kind_name)?;
        let provider = DbusProvider::new(kind.clone(), state.clone(), runtime_handle.clone());
        let provider = TracedProvider::new(PROVIDER_NAME, provider);

        graph.register_node_provider(provider.clone()).await?;
        graph
            .register_property_provider(kind.clone(), provider.clone())
            .await?;
        if matches!(kind_name, DBUS_OBJECT_KIND | DBUS_METHOD_KIND) {
            graph
                .register_property_mutation_provider(kind.clone(), provider.clone())
                .await?;
        }
        if kind_name == DBUS_SERVICE_KIND {
            graph.register_path_provider(provider.clone()).await?;
        }
        graph.register_relation_provider(kind, provider).await?;
    }
    Ok(DbusPluginHandle {
        _watchers: watchers,
        _runtime: runtime,
    })
}

#[async_trait]
impl LocusFsPlugin for DbusPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: "dbus",
            name: "D-Bus",
            version: env!("CARGO_PKG_VERSION"),
        }
    }

    async fn register(
        &self,
        context: PluginContext,
        config: toml::Value,
    ) -> Result<Box<dyn PluginHandle>> {
        let config = DbusConfig::from_value(config)?;
        Ok(Box::new(
            register_with_config_and_runtime(&context.graph, config).await?,
        ))
    }
}

#[allow(improper_ctypes_definitions)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _locusfs_plugin_init() -> *mut dyn LocusFsPlugin {
    Box::into_raw(Box::new(DbusPlugin))
}

fn config_error(error: toml::de::Error) -> GraphError {
    GraphError::InvalidValue {
        kind: "dbus plugin config",
        value: error.to_string(),
        reason: "invalid TOML shape",
    }
}
