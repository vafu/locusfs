use locusfs_graph::Result;
use serde::Deserialize;

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
pub struct NiriConfig {}

impl NiriConfig {
    pub fn from_value(value: toml::Value) -> Result<Self> {
        value.try_into().map_err(super::config_error)
    }
}

#[cfg(test)]
mod test {
    use super::NiriConfig;

    #[test]
    fn niri_config_accepts_empty_table() {
        NiriConfig::from_value(toml::Value::Table(toml::map::Map::new())).unwrap();
    }
}
