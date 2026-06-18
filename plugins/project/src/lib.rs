//! Project graph provider for `locusfs`.

use locusfs_graph::{DynamicGraph, InMemoryProvider, NodeKind, Result};

pub const PROJECT_KIND: &str = "project";

#[derive(Debug)]
pub struct ProjectPluginHandle {
    _provider: InMemoryProvider,
}

pub async fn register(graph: &DynamicGraph) -> Result<ProjectPluginHandle> {
    let kind = NodeKind::new(PROJECT_KIND)?;
    let provider = InMemoryProvider::new(kind.clone());
    graph.register_node_provider(provider.clone()).await?;
    graph
        .register_node_mutation_provider(kind.clone(), provider.clone())
        .await?;
    graph
        .register_property_provider(kind.clone(), provider.clone())
        .await?;
    graph
        .register_property_mutation_provider(kind.clone(), provider.clone())
        .await?;
    graph
        .register_relation_provider(kind.clone(), provider.clone())
        .await?;
    graph
        .register_relation_mutation_provider(kind, provider.clone())
        .await?;
    Ok(ProjectPluginHandle {
        _provider: provider,
    })
}
