#[cfg(test)]
mod test;

use std::collections::BTreeMap;
use std::env;
use std::error::Error;
use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;

pub type ConfigResult<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct Config {
    #[serde(default)]
    pub plugin_dirs: Vec<PathBuf>,
    #[serde(default)]
    pub plugins: BTreeMap<String, PluginConfig>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct PluginConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub library: Option<PathBuf>,
    #[serde(default = "empty_table")]
    pub config: toml::Value,
}

impl Default for PluginConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            library: None,
            config: empty_table(),
        }
    }
}

impl Config {
    pub async fn load(path: Option<PathBuf>) -> ConfigResult<Self> {
        let explicit = path.is_some();
        let path = path.unwrap_or_else(default_config_path);
        let contents = match tokio::fs::read_to_string(&path).await {
            Ok(contents) => contents,
            Err(error) if !explicit && error.kind() == io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => {
                return Err(io::Error::new(
                    error.kind(),
                    format!("failed to read config {}: {error}", path.display()),
                )
                .into());
            }
        };
        Self::parse(&contents)
    }

    pub fn parse(contents: &str) -> ConfigResult<Self> {
        let mut config = toml::from_str::<Self>(contents)?;
        config.expand_paths();
        Ok(config)
    }

    fn expand_paths(&mut self) {
        self.plugin_dirs = self
            .plugin_dirs
            .iter()
            .map(|path| expand_tilde(path))
            .collect();
        for plugin in self.plugins.values_mut() {
            plugin.library = plugin.library.as_deref().map(expand_tilde);
        }
    }
}

pub fn default_config_path() -> PathBuf {
    if let Some(base) = env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(base).join("locusfs/config.toml");
    }
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config/locusfs/config.toml")
}

pub(crate) fn expand_tilde(path: &Path) -> PathBuf {
    let Some(value) = path.to_str() else {
        return path.to_path_buf();
    };
    if value == "~" {
        return home_dir();
    }
    if let Some(rest) = value.strip_prefix("~/") {
        return home_dir().join(rest);
    }
    path.to_path_buf()
}

fn home_dir() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn empty_table() -> toml::Value {
    toml::Value::Table(toml::map::Map::new())
}
