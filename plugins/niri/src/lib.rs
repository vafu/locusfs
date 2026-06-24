//! Niri graph provider for `locusfs`.

pub mod config;
mod ipc;
mod provider;
mod state;

pub use provider::NiriProvider;

use async_trait::async_trait;
use locusfs_graph::{DynamicGraph, GraphError, NodeKind, Result, TracedProvider};
use locusfs_plugin_api::{
    LocusFsPlugin, PluginContext, PluginHandle, PluginManifest, PluginRuntime,
};
use tokio::task::JoinHandle;

use crate::config::NiriConfig;
use crate::ipc::IpcNiriClient;

pub const WINDOW_KIND: &str = "window";
pub const WORKSPACE_KIND: &str = "workspace";
pub const OUTPUT_KIND: &str = "output";
pub const CONTEXT_KIND: &str = "context";

const PROVIDER_KINDS: &[&str] = &[WINDOW_KIND, WORKSPACE_KIND, OUTPUT_KIND, CONTEXT_KIND];

#[derive(Debug, Default)]
pub struct NiriPlugin;

/// Registers read-only Niri providers on the graph.
#[derive(Debug)]
pub struct NiriPluginHandle {
    event_stream: Option<JoinHandle<()>>,
    _runtime: PluginRuntime,
}

impl Drop for NiriPluginHandle {
    fn drop(&mut self) {
        if let Some(event_stream) = &self.event_stream {
            event_stream.abort();
        }
    }
}

#[async_trait]
impl PluginHandle for NiriPluginHandle {
    async fn shutdown(mut self: Box<Self>) {
        if let Some(event_stream) = self.event_stream.take() {
            event_stream.abort();
            let _ = event_stream.await;
        }
    }
}

pub async fn register(graph: &DynamicGraph) -> Result<NiriPluginHandle> {
    register_with_config(graph, NiriConfig::default()).await
}

pub async fn register_with_config(
    graph: &DynamicGraph,
    _config: NiriConfig,
) -> Result<NiriPluginHandle> {
    register_with_config_and_runtime(graph, _config).await
}

async fn register_with_config_and_runtime(
    graph: &DynamicGraph,
    _config: NiriConfig,
) -> Result<NiriPluginHandle> {
    let runtime = PluginRuntime::new("locusfs-niri")?;
    let runtime_handle = runtime.handle();
    let init_runtime = runtime_handle.clone();
    let (state, event_stream) = runtime_handle
        .spawn(IpcNiriClient::start(graph.clone(), init_runtime))
        .await
        .map_err(|error| GraphError::Io(format!("start Niri IPC task failed: {error}")))??;

    for kind in PROVIDER_KINDS {
        let kind = NodeKind::new(*kind)?;
        let provider = NiriProvider::new(kind.clone(), state.clone());
        let provider = TracedProvider::new("niri", provider);
        graph.register_node_provider(provider.clone()).await?;
        graph
            .register_property_provider(kind.clone(), provider.clone())
            .await?;
        graph.register_relation_provider(kind, provider).await?;
    }

    Ok(NiriPluginHandle {
        event_stream: Some(event_stream),
        _runtime: runtime,
    })
}

#[async_trait]
impl LocusFsPlugin for NiriPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: "niri",
            name: "Niri",
            version: env!("CARGO_PKG_VERSION"),
        }
    }

    async fn register(
        &self,
        context: PluginContext,
        config: toml::Value,
    ) -> Result<Box<dyn PluginHandle>> {
        let config = NiriConfig::from_value(config)?;
        Ok(Box::new(
            register_with_config_and_runtime(&context.graph, config).await?,
        ))
    }
}

#[allow(improper_ctypes_definitions)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _locusfs_plugin_init() -> *mut dyn LocusFsPlugin {
    Box::into_raw(Box::new(NiriPlugin))
}

fn config_error(error: toml::de::Error) -> GraphError {
    GraphError::InvalidValue {
        kind: "niri plugin config",
        value: error.to_string(),
        reason: "invalid TOML shape",
    }
}
