use locusfs_graph::{GraphError, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct NotifydConfig {
    pub server_name: String,
    pub vendor: String,
    pub default_timeout_ms: i32,
    pub low_timeout_ms: i32,
    pub critical_timeout_ms: i32,
    #[serde(
        alias = "max-active",
        alias = "max_active",
        alias = "max-history",
        alias = "max_history"
    )]
    pub max_notifications: usize,
    pub max_body_bytes: usize,
    pub max_actions: usize,
    pub markup: bool,
    pub actions: bool,
    pub body_images: bool,
    pub close_on_action: bool,
    pub dnd_enabled: bool,
}

impl Default for NotifydConfig {
    fn default() -> Self {
        Self {
            server_name: "Locus Notifyd".to_owned(),
            vendor: "Locus".to_owned(),
            default_timeout_ms: 7000,
            low_timeout_ms: 4000,
            critical_timeout_ms: 0,
            max_notifications: 128,
            max_body_bytes: 16 * 1024,
            max_actions: 12,
            markup: true,
            actions: true,
            body_images: true,
            close_on_action: true,
            dnd_enabled: false,
        }
    }
}

impl NotifydConfig {
    pub fn from_value(value: toml::Value) -> Result<Self> {
        let config: Self = value.try_into().map_err(super::config_error)?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<()> {
        if self.server_name.trim().is_empty() {
            return Err(GraphError::InvalidValue {
                kind: "notifyd server_name",
                value: self.server_name.clone(),
                reason: "must not be empty",
            });
        }
        if self.max_notifications == 0 {
            return Err(GraphError::InvalidValue {
                kind: "notifyd max_notifications",
                value: self.max_notifications.to_string(),
                reason: "must be greater than zero",
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::NotifydConfig;

    #[test]
    fn config_defaults_are_valid() {
        let value = toml::Value::try_from(NotifydConfig::default()).unwrap();
        let config = NotifydConfig::from_value(value).unwrap();
        assert_eq!(config.server_name, "Locus Notifyd");
    }

    #[test]
    fn config_rejects_empty_server_name() {
        let value = toml::from_str("server-name = ''").unwrap();
        assert!(NotifydConfig::from_value(value).is_err());
    }
}
