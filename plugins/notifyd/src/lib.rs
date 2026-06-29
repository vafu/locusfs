//! Freedesktop notification daemon graph provider for `locusfs`.

mod config;
mod image;
mod markup;
mod provider;
mod runtime;
mod state;

pub use provider::NotifydProvider;

use async_trait::async_trait;
use locusfs_graph::{DynamicGraph, NodeKind, Result, TracedProvider};
use locusfs_plugin_api::{
    LocusFsPlugin, PluginContext, PluginHandle, PluginManifest, PluginRuntime,
};

use crate::config::NotifydConfig;
use crate::runtime::NotifydRuntime;

pub const NOTIFYD_KIND: &str = "notifyd";
pub const NOTIFICATION_KIND: &str = "notification";
pub const NOTIFICATION_ACTION_KIND: &str = "notification-action";

const PROVIDER_NAME: &str = "notifyd";
const PROVIDER_KINDS: &[&str] = &[NOTIFYD_KIND, NOTIFICATION_KIND, NOTIFICATION_ACTION_KIND];
const PATH_PROVIDER_KINDS: &[&str] = PROVIDER_KINDS;

#[derive(Debug, Default)]
pub struct NotifydPlugin;

#[derive(Debug)]
pub struct NotifydPluginHandle {
    event_stream: Option<tokio::task::JoinHandle<()>>,
    _runtime: PluginRuntime,
}

impl Drop for NotifydPluginHandle {
    fn drop(&mut self) {
        if let Some(event_stream) = &self.event_stream {
            event_stream.abort();
        }
    }
}

#[async_trait]
impl PluginHandle for NotifydPluginHandle {
    async fn shutdown(mut self: Box<Self>) {
        if let Some(event_stream) = self.event_stream.take() {
            event_stream.abort();
            let _ = event_stream.await;
        }
    }
}

pub async fn register(graph: &DynamicGraph) -> Result<NotifydPluginHandle> {
    register_with_config(graph, NotifydConfig::default()).await
}

pub async fn register_with_config(
    graph: &DynamicGraph,
    config: NotifydConfig,
) -> Result<NotifydPluginHandle> {
    register_with_config_and_runtime(graph, config).await
}

async fn register_with_config_and_runtime(
    graph: &DynamicGraph,
    config: NotifydConfig,
) -> Result<NotifydPluginHandle> {
    let runtime = PluginRuntime::new("locusfs-notifyd")?;
    let runtime_handle = runtime.handle();
    let (state, commands, event_stream) =
        NotifydRuntime::start(graph.clone(), config, runtime_handle.clone()).await?;

    for kind_name in PROVIDER_KINDS {
        let kind = NodeKind::new(*kind_name)?;
        let provider = NotifydProvider::new(kind.clone(), state.clone(), commands.clone());
        let provider = TracedProvider::new(PROVIDER_NAME, provider);

        graph.register_node_provider(provider.clone()).await?;
        graph
            .register_property_provider(kind.clone(), provider.clone())
            .await?;
        graph
            .register_property_mutation_provider(kind.clone(), provider.clone())
            .await?;
        if PATH_PROVIDER_KINDS.contains(kind_name) {
            graph.register_path_provider(provider.clone()).await?;
        }
        graph.register_relation_provider(kind, provider).await?;
    }

    Ok(NotifydPluginHandle {
        event_stream: Some(event_stream),
        _runtime: runtime,
    })
}

#[async_trait]
impl LocusFsPlugin for NotifydPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: "notifyd",
            name: "Notifyd",
            version: env!("CARGO_PKG_VERSION"),
        }
    }

    fn default_config(&self) -> toml::Value {
        toml::Value::try_from(NotifydConfig::default())
            .unwrap_or_else(|_| toml::Value::Table(toml::map::Map::new()))
    }

    async fn register(
        &self,
        context: PluginContext,
        config: toml::Value,
    ) -> Result<Box<dyn PluginHandle>> {
        let config = NotifydConfig::from_value(config)?;
        Ok(Box::new(
            register_with_config_and_runtime(&context.graph, config).await?,
        ))
    }
}

#[allow(improper_ctypes_definitions)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _locusfs_plugin_init() -> *mut dyn LocusFsPlugin {
    Box::into_raw(Box::new(NotifydPlugin))
}

fn config_error(error: toml::de::Error) -> locusfs_graph::GraphError {
    locusfs_graph::GraphError::InvalidValue {
        kind: "notifyd plugin config",
        value: error.to_string(),
        reason: "invalid TOML shape",
    }
}

#[cfg(test)]
mod tests {
    use super::{NOTIFICATION_ACTION_KIND, NOTIFICATION_KIND, NOTIFYD_KIND, PATH_PROVIDER_KINDS};

    #[test]
    fn path_providers_cover_all_notifyd_path_node_kinds() {
        assert_eq!(
            PATH_PROVIDER_KINDS,
            &[NOTIFYD_KIND, NOTIFICATION_KIND, NOTIFICATION_ACTION_KIND]
        );
    }
}
