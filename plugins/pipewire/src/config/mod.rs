use locusfs_graph::Result;
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct PipeWireConfig {
    #[serde(default = "default_pactl")]
    pub pactl: String,
}

impl Default for PipeWireConfig {
    fn default() -> Self {
        Self {
            pactl: default_pactl(),
        }
    }
}

impl PipeWireConfig {
    pub fn from_value(value: toml::Value) -> Result<Self> {
        value.try_into().map_err(super::config_error)
    }
}

fn default_pactl() -> String {
    "pactl".to_string()
}

#[cfg(test)]
mod test {
    use super::PipeWireConfig;

    #[test]
    fn pipewire_config_defaults_to_pactl() {
        let config = PipeWireConfig::from_value(toml::Value::Table(toml::map::Map::new()))
            .expect("empty config parses");

        assert_eq!(config.pactl, "pactl");
    }
}
