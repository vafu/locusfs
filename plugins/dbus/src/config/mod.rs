use locusfs_graph::{GraphError, Result};
use serde::Deserialize;

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
pub struct DbusConfig {
    #[serde(default)]
    pub services: Vec<ServiceConfig>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct ServiceConfig {
    pub name: String,
    #[serde(default)]
    pub bus: BusKind,
    #[serde(default)]
    pub local_id: Option<String>,
    #[serde(default)]
    pub object_manager_path: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum BusKind {
    #[default]
    System,
    Session,
}

impl DbusConfig {
    pub fn from_value(value: toml::Value) -> Result<Self> {
        value.try_into().map_err(super::config_error)
    }

    pub(crate) fn into_runtime_services(self) -> Result<Vec<crate::state::ServiceConfig>> {
        self.services.into_iter().map(TryInto::try_into).collect()
    }
}

impl TryFrom<ServiceConfig> for crate::state::ServiceConfig {
    type Error = GraphError;

    fn try_from(config: ServiceConfig) -> Result<Self> {
        if config.name.trim().is_empty() {
            return Err(GraphError::InvalidValue {
                kind: "D-Bus service name",
                value: config.name,
                reason: "must not be empty",
            });
        }
        let local_id = config
            .local_id
            .unwrap_or_else(|| service_local_id(&config.name));
        let object_manager_path = config
            .object_manager_path
            .unwrap_or_else(|| format!("/{}", config.name.replace('.', "/")));
        Ok(Self {
            local_id,
            bus: config.bus.into(),
            name: config.name,
            object_manager_path,
        })
    }
}

impl From<BusKind> for crate::state::BusKind {
    fn from(kind: BusKind) -> Self {
        match kind {
            BusKind::System => Self::System,
            BusKind::Session => Self::Session,
        }
    }
}

fn service_local_id(name: &str) -> String {
    name.rsplit('.').next().unwrap_or(name).to_ascii_lowercase()
}

#[cfg(test)]
mod test {
    use super::{BusKind, DbusConfig};

    #[test]
    fn dbus_config_defaults_service_fields_from_name() {
        let config = DbusConfig::from_value(
            toml::from_str(
                r#"
services = [{ name = "org.freedesktop.UPower" }]
"#,
            )
            .unwrap(),
        )
        .unwrap();

        let services = config.into_runtime_services().unwrap();

        assert_eq!(services[0].local_id, "upower");
        assert_eq!(services[0].object_manager_path, "/org/freedesktop/UPower");
        assert_eq!(services[0].bus.as_str(), "system");
    }

    #[test]
    fn dbus_config_accepts_session_bus() {
        let config = DbusConfig::from_value(
            toml::from_str(
                r#"
services = [{ name = "org.example.App", bus = "session" }]
"#,
            )
            .unwrap(),
        )
        .unwrap();

        assert_eq!(config.services[0].bus, BusKind::Session);
    }
}
