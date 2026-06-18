//! PipeWire graph provider for `locusfs`.

pub mod config;
mod provider;
mod runtime;
mod state;

pub use provider::PipeWireProvider;

use async_trait::async_trait;
use locusfs_graph::{DynamicGraph, GraphError, NodeKind, Result, TracedProvider};
use locusfs_plugin_api::{LocusFsPlugin, PluginContext, PluginHandle, PluginManifest};
use tokio::task::JoinHandle;

use crate::config::PipeWireConfig;
use crate::runtime::PipeWireRuntime;

pub const PIPEWIRE_KIND: &str = "pipewire";
pub const PIPEWIRE_SINK_KIND: &str = "pipewire-sink";
pub const PIPEWIRE_SOURCE_KIND: &str = "pipewire-source";

const PROVIDER_NAME: &str = "pipewire";
const PROVIDER_KINDS: &[&str] = &[PIPEWIRE_KIND, PIPEWIRE_SINK_KIND, PIPEWIRE_SOURCE_KIND];

#[derive(Debug, Default)]
pub struct PipeWirePlugin;

#[derive(Debug)]
pub struct PipeWirePluginHandle {
    event_stream: Option<JoinHandle<()>>,
}

impl Drop for PipeWirePluginHandle {
    fn drop(&mut self) {
        if let Some(event_stream) = &self.event_stream {
            event_stream.abort();
        }
    }
}

#[async_trait]
impl PluginHandle for PipeWirePluginHandle {
    async fn shutdown(mut self: Box<Self>) {
        if let Some(event_stream) = self.event_stream.take() {
            event_stream.abort();
            let _ = event_stream.await;
        }
    }
}

pub async fn register(graph: &DynamicGraph) -> Result<PipeWirePluginHandle> {
    register_with_config(graph, PipeWireConfig::default()).await
}

pub async fn register_with_config(
    graph: &DynamicGraph,
    config: PipeWireConfig,
) -> Result<PipeWirePluginHandle> {
    let runtime = tokio::runtime::Handle::current();
    register_with_config_and_runtime(graph, config, runtime).await
}

async fn register_with_config_and_runtime(
    graph: &DynamicGraph,
    config: PipeWireConfig,
    runtime: tokio::runtime::Handle,
) -> Result<PipeWirePluginHandle> {
    let (state, event_stream) = PipeWireRuntime::start(graph.clone(), config, runtime);

    for kind in PROVIDER_KINDS {
        let kind = NodeKind::new(*kind)?;
        let provider = PipeWireProvider::new(kind.clone(), state.clone());
        let provider = TracedProvider::new(PROVIDER_NAME, provider);
        graph.register_node_provider(provider.clone()).await?;
        graph
            .register_property_provider(kind.clone(), provider.clone())
            .await?;
        graph.register_relation_provider(kind, provider).await?;
    }

    Ok(PipeWirePluginHandle {
        event_stream: Some(event_stream),
    })
}

#[async_trait]
impl LocusFsPlugin for PipeWirePlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: "pipewire",
            name: "PipeWire",
            version: env!("CARGO_PKG_VERSION"),
        }
    }

    async fn register(
        &self,
        context: PluginContext,
        config: toml::Value,
    ) -> Result<Box<dyn PluginHandle>> {
        let config = PipeWireConfig::from_value(config)?;
        Ok(Box::new(
            register_with_config_and_runtime(&context.graph, config, context.runtime).await?,
        ))
    }
}

#[allow(improper_ctypes_definitions)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _locusfs_plugin_init() -> *mut dyn LocusFsPlugin {
    Box::into_raw(Box::new(PipeWirePlugin))
}

fn config_error(error: toml::de::Error) -> GraphError {
    GraphError::InvalidValue {
        kind: "pipewire plugin config",
        value: error.to_string(),
        reason: "invalid TOML shape",
    }
}
