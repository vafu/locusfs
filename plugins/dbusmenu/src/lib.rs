//! DBusMenu graph provider for `locusfs`.

pub mod config;
mod provider;
mod runtime;
mod state;

pub use provider::DbusMenuProvider;

use async_trait::async_trait;
use locusfs_graph::{DynamicGraph, GraphError, NodeKind, Result, TracedProvider};
use locusfs_plugin_api::{
    LocusFsPlugin, PluginContext, PluginHandle, PluginManifest, PluginRuntime,
};
use tokio::task::JoinHandle;

use crate::config::DbusMenuConfig;
use crate::state::DbusMenuState;

pub const DBUSMENU_KIND: &str = "dbusmenu";
pub const DBUSMENU_MENU_KIND: &str = "dbusmenu-menu";
pub const DBUSMENU_ITEM_KIND: &str = "dbusmenu-item";

const PROVIDER_NAME: &str = "dbusmenu";
const PROVIDER_KINDS: &[&str] = &[DBUSMENU_KIND, DBUSMENU_MENU_KIND, DBUSMENU_ITEM_KIND];

#[derive(Debug, Default)]
pub struct DbusMenuPlugin;

#[derive(Debug)]
pub struct DbusMenuPluginHandle {
    _state: state::SharedDbusMenuState,
    task: JoinHandle<()>,
    _runtime: PluginRuntime,
}

#[async_trait]
impl PluginHandle for DbusMenuPluginHandle {
    async fn shutdown(mut self: Box<Self>) {
        self.task.abort();
        let _ = (&mut self.task).await;
    }
}

pub async fn register(graph: &DynamicGraph) -> Result<DbusMenuPluginHandle> {
    register_with_config(graph, DbusMenuConfig::default()).await
}

pub async fn register_with_config(
    graph: &DynamicGraph,
    config: DbusMenuConfig,
) -> Result<DbusMenuPluginHandle> {
    let runtime = PluginRuntime::new("locusfs-dbusmenu")?;
    let runtime_handle = runtime.handle();
    let state = DbusMenuState::shared(config.into_runtime_menus()?);
    for kind_name in PROVIDER_KINDS {
        let kind = NodeKind::new(*kind_name)?;
        let provider = DbusMenuProvider::new(kind.clone(), state.clone(), runtime_handle.clone());
        let provider = TracedProvider::new(PROVIDER_NAME, provider);

        graph.register_node_provider(provider.clone()).await?;
        graph
            .register_property_provider(kind.clone(), provider.clone())
            .await?;
        graph.register_relation_provider(kind, provider).await?;
    }
    let item_kind = NodeKind::new(DBUSMENU_ITEM_KIND)?;
    let provider = DbusMenuProvider::new(item_kind.clone(), state.clone(), runtime_handle.clone());
    let provider = TracedProvider::new(PROVIDER_NAME, provider);
    graph
        .register_property_mutation_provider(item_kind, provider)
        .await?;

    let task = runtime::DbusMenuRuntime::start(graph.clone(), runtime_handle, state.clone());
    Ok(DbusMenuPluginHandle {
        _state: state,
        task,
        _runtime: runtime,
    })
}

#[async_trait]
impl LocusFsPlugin for DbusMenuPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: "dbusmenu",
            name: "DBusMenu",
            version: env!("CARGO_PKG_VERSION"),
        }
    }

    async fn register(
        &self,
        context: PluginContext,
        config: toml::Value,
    ) -> Result<Box<dyn PluginHandle>> {
        let config = DbusMenuConfig::from_value(config)?;
        Ok(Box::new(
            register_with_config(&context.graph, config).await?,
        ))
    }
}

#[allow(improper_ctypes_definitions)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _locusfs_plugin_init() -> *mut dyn LocusFsPlugin {
    Box::into_raw(Box::new(DbusMenuPlugin))
}

fn config_error(error: toml::de::Error) -> GraphError {
    GraphError::InvalidValue {
        kind: "dbusmenu plugin config",
        value: error.to_string(),
        reason: "invalid TOML shape",
    }
}
