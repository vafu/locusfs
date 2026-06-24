//! StatusNotifier/AppIndicator graph provider for `locusfs`.

mod provider;
mod runtime;
mod state;

pub use provider::StatusNotifierProvider;

use async_trait::async_trait;
use locusfs_graph::{DynamicGraph, NodeKind, Result, TracedProvider};
use locusfs_plugin_api::{
    LocusFsPlugin, PluginContext, PluginHandle, PluginManifest, PluginRuntime,
};
use tokio::task::JoinHandle;

use crate::runtime::StatusNotifierRuntime;

pub const STATUS_NOTIFIER_KIND: &str = "statusnotifier";
pub const STATUS_NOTIFIER_ITEM_KIND: &str = "statusnotifier-item";

const PROVIDER_NAME: &str = "statusnotifier";
const PROVIDER_KINDS: &[&str] = &[STATUS_NOTIFIER_KIND, STATUS_NOTIFIER_ITEM_KIND];

#[derive(Debug, Default)]
pub struct StatusNotifierPlugin;

#[derive(Debug)]
pub struct StatusNotifierPluginHandle {
    event_stream: Option<JoinHandle<()>>,
    _runtime: PluginRuntime,
}

impl Drop for StatusNotifierPluginHandle {
    fn drop(&mut self) {
        if let Some(event_stream) = &self.event_stream {
            event_stream.abort();
        }
    }
}

#[async_trait]
impl PluginHandle for StatusNotifierPluginHandle {
    async fn shutdown(mut self: Box<Self>) {
        if let Some(event_stream) = self.event_stream.take() {
            event_stream.abort();
            let _ = event_stream.await;
        }
    }
}

pub async fn register(graph: &DynamicGraph) -> Result<StatusNotifierPluginHandle> {
    register_with_runtime(graph).await
}

async fn register_with_runtime(graph: &DynamicGraph) -> Result<StatusNotifierPluginHandle> {
    let runtime = PluginRuntime::new("locusfs-statusnotifier")?;
    let (state, event_stream) = StatusNotifierRuntime::start(graph.clone(), runtime.handle());

    for kind in PROVIDER_KINDS {
        let kind = NodeKind::new(*kind)?;
        let provider = StatusNotifierProvider::new(kind.clone(), state.clone());
        let provider = TracedProvider::new(PROVIDER_NAME, provider);
        graph.register_node_provider(provider.clone()).await?;
        graph
            .register_property_provider(kind.clone(), provider.clone())
            .await?;
        graph.register_relation_provider(kind, provider).await?;
    }

    Ok(StatusNotifierPluginHandle {
        event_stream: Some(event_stream),
        _runtime: runtime,
    })
}

#[async_trait]
impl LocusFsPlugin for StatusNotifierPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: "statusnotifier",
            name: "StatusNotifier",
            version: env!("CARGO_PKG_VERSION"),
        }
    }

    async fn register(
        &self,
        context: PluginContext,
        _config: toml::Value,
    ) -> Result<Box<dyn PluginHandle>> {
        Ok(Box::new(register_with_runtime(&context.graph).await?))
    }
}

#[allow(improper_ctypes_definitions)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _locusfs_plugin_init() -> *mut dyn LocusFsPlugin {
    Box::into_raw(Box::new(StatusNotifierPlugin))
}
