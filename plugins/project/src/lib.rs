//! Project graph provider for `locusfs`.

pub mod config;

use async_trait::async_trait;
use locusfs_graph::{DynamicGraph, GraphError, InMemoryProvider, NodeKind, Result};
use locusfs_plugin_api::{LocusFsPlugin, PluginContext, PluginHandle, PluginManifest};

use crate::config::ProjectConfig;

pub const PROJECT_KIND: &str = "project";

#[derive(Debug, Default)]
pub struct ProjectPlugin;

#[derive(Debug)]
pub struct ProjectPluginHandle {
    _provider: InMemoryProvider,
}

#[async_trait]
impl PluginHandle for ProjectPluginHandle {}

pub async fn register(graph: &DynamicGraph) -> Result<ProjectPluginHandle> {
    register_with_config(graph, ProjectConfig::default()).await
}

pub async fn register_with_config(
    graph: &DynamicGraph,
    _config: ProjectConfig,
) -> Result<ProjectPluginHandle> {
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

#[async_trait]
impl LocusFsPlugin for ProjectPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: "project",
            name: "Project",
            version: env!("CARGO_PKG_VERSION"),
        }
    }

    async fn register(
        &self,
        context: PluginContext,
        config: toml::Value,
    ) -> Result<Box<dyn PluginHandle>> {
        let config = ProjectConfig::from_value(config)?;
        Ok(Box::new(
            register_with_config(&context.graph, config).await?,
        ))
    }
}

#[allow(improper_ctypes_definitions)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _locusfs_plugin_init() -> *mut dyn LocusFsPlugin {
    Box::into_raw(Box::new(ProjectPlugin))
}

fn config_error(error: toml::de::Error) -> GraphError {
    GraphError::InvalidValue {
        kind: "project plugin config",
        value: error.to_string(),
        reason: "invalid TOML shape",
    }
}
