//! MPRIS graph provider for `locusfs`.

mod provider;
mod runtime;
mod state;

pub use provider::MprisProvider;

use async_trait::async_trait;
use locusfs_graph::{DynamicGraph, NodeKind, Result, TracedProvider};
use locusfs_plugin_api::{
    LocusFsPlugin, PluginContext, PluginHandle, PluginManifest, PluginRuntime,
};
use tokio::task::JoinHandle;

use crate::runtime::MprisRuntime;

pub const MPRIS_KIND: &str = "mpris";
pub const MPRIS_PLAYER_KIND: &str = "mpris-player";

const PROVIDER_NAME: &str = "mpris";
const PROVIDER_KINDS: &[&str] = &[MPRIS_KIND, MPRIS_PLAYER_KIND];

#[derive(Debug, Default)]
pub struct MprisPlugin;

#[derive(Debug)]
pub struct MprisPluginHandle {
    event_stream: Option<JoinHandle<()>>,
    _runtime: PluginRuntime,
}

impl Drop for MprisPluginHandle {
    fn drop(&mut self) {
        if let Some(event_stream) = &self.event_stream {
            event_stream.abort();
        }
    }
}

#[async_trait]
impl PluginHandle for MprisPluginHandle {
    async fn shutdown(mut self: Box<Self>) {
        if let Some(event_stream) = self.event_stream.take() {
            event_stream.abort();
            let _ = event_stream.await;
        }
    }
}

pub async fn register(graph: &DynamicGraph) -> Result<MprisPluginHandle> {
    register_with_runtime(graph).await
}

async fn register_with_runtime(graph: &DynamicGraph) -> Result<MprisPluginHandle> {
    let runtime = PluginRuntime::new("locusfs-mpris")?;
    let (state, event_stream) = MprisRuntime::start(graph.clone(), runtime.handle());

    for kind_name in PROVIDER_KINDS {
        let kind = NodeKind::new(*kind_name)?;
        let provider = MprisProvider::new(kind.clone(), state.clone());
        let provider = TracedProvider::new(PROVIDER_NAME, provider);
        graph.register_node_provider(provider.clone()).await?;
        graph
            .register_property_provider(kind.clone(), provider.clone())
            .await?;
        if *kind_name == MPRIS_KIND {
            graph.register_path_provider(provider.clone()).await?;
        }
        graph.register_relation_provider(kind, provider).await?;
    }

    Ok(MprisPluginHandle {
        event_stream: Some(event_stream),
        _runtime: runtime,
    })
}

#[async_trait]
impl LocusFsPlugin for MprisPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: "mpris",
            name: "MPRIS",
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
    Box::into_raw(Box::new(MprisPlugin))
}
