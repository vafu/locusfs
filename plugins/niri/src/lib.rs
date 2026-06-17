//! Niri graph provider for `locusfs`.

mod ipc;
mod provider;
mod state;

pub use provider::NiriProvider;

use locusfs_graph::{DynamicGraph, NodeKind, Result, TracedProvider};
use tokio::task::JoinHandle;

use crate::ipc::IpcNiriClient;

pub const WINDOW_KIND: &str = "window";
pub const WORKSPACE_KIND: &str = "workspace";
pub const OUTPUT_KIND: &str = "output";
pub const CONTEXT_KIND: &str = "context";

const PROVIDER_KINDS: &[&str] = &[WINDOW_KIND, WORKSPACE_KIND, OUTPUT_KIND, CONTEXT_KIND];

/// Registers read-only Niri providers on the graph.
#[derive(Debug)]
pub struct NiriPluginHandle {
    _event_stream: JoinHandle<()>,
}

pub async fn register(graph: &DynamicGraph) -> Result<NiriPluginHandle> {
    let (state, event_stream) = IpcNiriClient::start(graph.clone()).await?;

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
        _event_stream: event_stream,
    })
}
