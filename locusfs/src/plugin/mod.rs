#[cfg(test)]
mod test;

use std::env;
use std::error::Error;
use std::io;
use std::path::{Path, PathBuf};

use crate::config::{Config, PluginConfig, expand_tilde};
use libloading::{Library, Symbol};
use locusfs_graph::DynamicGraph;
use locusfs_plugin_api::{LocusFsPlugin, PluginContext, PluginHandle};
use tracing::{Instrument, info_span};

type PluginResult<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;
#[allow(improper_ctypes_definitions)]
type PluginInit = unsafe extern "C" fn() -> *mut dyn LocusFsPlugin;

#[derive(Default)]
pub struct PluginManager {
    _loaded: Vec<LoadedPlugin>,
}

struct LoadedPlugin {
    id: String,
    handle: Option<Box<dyn PluginHandle>>,
    _plugin: Box<dyn LocusFsPlugin>,
    _library: Library,
}

impl PluginManager {
    pub async fn load_enabled(config: &Config, graph: DynamicGraph) -> PluginResult<Self> {
        let mut loaded = Vec::new();
        for (id, plugin_config) in &config.plugins {
            if !plugin_config.enabled {
                continue;
            }
            let plugin = match load_plugin(id, plugin_config, config, graph.clone())
                .instrument(info_span!("plugin.load", plugin = %id))
                .await
            {
                Ok(plugin) => plugin,
                Err(error) => {
                    shutdown_loaded(&mut loaded).await;
                    return Err(error);
                }
            };
            loaded.push(plugin);
        }
        Ok(Self { _loaded: loaded })
    }

    pub fn loaded_count(&self) -> usize {
        self._loaded.len()
    }

    pub async fn shutdown(&mut self) {
        shutdown_loaded(&mut self._loaded).await;
    }
}

async fn shutdown_loaded(loaded: &mut [LoadedPlugin]) {
    for plugin in loaded {
        let id = plugin.id.clone();
        plugin
            .shutdown()
            .instrument(info_span!("plugin.shutdown", plugin = %id))
            .await;
    }
}

async fn load_plugin(
    id: &str,
    plugin_config: &PluginConfig,
    config: &Config,
    graph: DynamicGraph,
) -> PluginResult<LoadedPlugin> {
    let path = resolve_library_path(id, plugin_config, config)?;
    let library = unsafe { Library::new(&path) }?;
    let init: Symbol<'_, PluginInit> = unsafe { library.get(b"_locusfs_plugin_init") }?;
    let plugin = unsafe {
        let raw = init();
        if raw.is_null() {
            return Err(plugin_error(format!(
                "plugin initializer returned null for {}",
                path.display()
            )));
        }
        Box::from_raw(raw)
    };
    let manifest = plugin.manifest();
    if manifest.id != id {
        return Err(plugin_error(format!(
            "plugin id mismatch: config requested {id}, library reported {}",
            manifest.id
        )));
    }

    let merged_config = merge_toml(plugin.default_config(), plugin_config.config.clone());
    let handle = plugin
        .register(PluginContext::try_new(graph)?, merged_config)
        .await?;
    Ok(LoadedPlugin {
        id: id.to_string(),
        handle: Some(handle),
        _plugin: plugin,
        _library: library,
    })
}

impl LoadedPlugin {
    async fn shutdown(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.shutdown().await;
        }
    }
}

fn resolve_library_path(
    id: &str,
    plugin_config: &PluginConfig,
    config: &Config,
) -> PluginResult<PathBuf> {
    if let Some(path) = &plugin_config.library {
        return Ok(path.clone());
    }

    let file_name = plugin_library_name(id);
    for dir in plugin_search_dirs(config) {
        let path = dir.join(&file_name);
        if path.is_file() {
            return Ok(path);
        }
    }
    Err(plugin_error(format!(
        "could not find {file_name} in configured plugin directories"
    )))
}

fn plugin_search_dirs(config: &Config) -> Vec<PathBuf> {
    let mut dirs = config.plugin_dirs.clone();
    if let Ok(exe) = env::current_exe() {
        if let Some(parent) = exe.parent() {
            dirs.push(parent.to_path_buf());
        }
    }
    dirs.push(expand_tilde(Path::new("~/.local/lib/locusfs/plugins")));
    dedup_paths(dirs)
}

fn plugin_library_name(id: &str) -> String {
    let id = id.replace('-', "_");
    format!(
        "{}locusfs_plugin_{}{}",
        env::consts::DLL_PREFIX,
        id,
        env::consts::DLL_SUFFIX
    )
}

fn merge_toml(defaults: toml::Value, overrides: toml::Value) -> toml::Value {
    match (defaults, overrides) {
        (toml::Value::Table(mut defaults), toml::Value::Table(overrides)) => {
            for (key, value) in overrides {
                let default = defaults
                    .remove(&key)
                    .unwrap_or_else(|| toml::Value::Table(toml::map::Map::new()));
                defaults.insert(key, merge_toml(default, value));
            }
            toml::Value::Table(defaults)
        }
        (_, overrides) => overrides,
    }
}

fn dedup_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut deduped: Vec<PathBuf> = Vec::new();
    for path in paths {
        if !deduped.iter().any(|existing| same_path(existing, &path)) {
            deduped.push(path);
        }
    }
    deduped
}

fn same_path(left: &Path, right: &Path) -> bool {
    normalize_path(left) == normalize_path(right)
}

fn normalize_path(path: &Path) -> PathBuf {
    path.to_path_buf()
}

fn plugin_error(message: impl Into<String>) -> Box<dyn Error + Send + Sync> {
    io::Error::other(message.into()).into()
}
