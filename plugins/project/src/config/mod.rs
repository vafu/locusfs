use std::path::PathBuf;

use locusfs_graph::Result;
use serde::Deserialize;

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
pub struct ProjectConfig {
    #[serde(default)]
    pub state_path: Option<PathBuf>,
}

impl ProjectConfig {
    pub fn from_value(value: toml::Value) -> Result<Self> {
        value.try_into().map_err(super::config_error)
    }
}
