//! Project graph provider for `locusfs`.

pub mod config;

use async_trait::async_trait;
use locusfs_graph::{DynamicGraph, GraphError, InMemoryProvider, NodeKind, Result, TracedProvider};
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
    config: ProjectConfig,
) -> Result<ProjectPluginHandle> {
    if let Some(state_path) = config.state_path {
        return Err(GraphError::InvalidValue {
            kind: "project plugin config",
            value: state_path.display().to_string(),
            reason: "state_path persistence is not implemented",
        });
    }

    let kind = NodeKind::new(PROJECT_KIND)?;
    let provider = InMemoryProvider::new(kind.clone());
    let traced_provider = TracedProvider::new("project", provider.clone());
    graph.register_read_write_provider(traced_provider).await?;
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

#[cfg(test)]
mod test {
    use std::path::PathBuf;

    use locusfs_graph::{DynamicGraph, GraphError};

    use super::{ProjectConfig, register_with_config};

    #[tokio::test]
    async fn state_path_is_rejected_until_persistence_exists() {
        let error = register_with_config(
            &DynamicGraph::new(),
            ProjectConfig {
                state_path: Some(PathBuf::from("/tmp/locusfs-project-state.toml")),
            },
        )
        .await
        .unwrap_err();

        assert!(matches!(error, GraphError::InvalidValue { .. }));
    }
}
