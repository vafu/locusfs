use locusfs_graph::{GraphError, Result};
use serde::Deserialize;

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
pub struct DbusMenuConfig {
    #[serde(default)]
    pub menus: Vec<MenuConfig>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct MenuConfig {
    pub service: String,
    pub path: String,
    #[serde(default)]
    pub bus: BusKind,
    #[serde(default)]
    pub local_id: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum BusKind {
    #[default]
    Session,
    System,
}

impl DbusMenuConfig {
    pub fn from_value(value: toml::Value) -> Result<Self> {
        value.try_into().map_err(crate::config_error)
    }

    pub(crate) fn into_runtime_menus(self) -> Result<Vec<crate::state::DbusMenuEndpoint>> {
        self.menus.into_iter().map(TryInto::try_into).collect()
    }
}

impl TryFrom<MenuConfig> for crate::state::DbusMenuEndpoint {
    type Error = GraphError;

    fn try_from(config: MenuConfig) -> Result<Self> {
        if config.service.trim().is_empty() {
            return Err(GraphError::InvalidValue {
                kind: "DBusMenu service",
                value: config.service,
                reason: "must not be empty",
            });
        }
        if !config.path.starts_with('/') {
            return Err(GraphError::InvalidValue {
                kind: "DBusMenu object path",
                value: config.path,
                reason: "must be an absolute D-Bus object path",
            });
        }
        let local_id = config
            .local_id
            .unwrap_or_else(|| crate::state::menu_local_id(&config.service, &config.path));
        Ok(Self::new(
            local_id,
            config.bus.into(),
            config.service,
            config.path,
        ))
    }
}

impl From<BusKind> for crate::state::BusKind {
    fn from(kind: BusKind) -> Self {
        match kind {
            BusKind::Session => Self::Session,
            BusKind::System => Self::System,
        }
    }
}

#[cfg(test)]
mod test {
    use super::{BusKind, DbusMenuConfig};

    #[test]
    fn config_defaults_to_session_bus_and_generated_local_id() {
        let config = DbusMenuConfig::from_value(
            toml::from_str(
                r#"
menus = [{ service = "org.example.App", path = "/Menu" }]
"#,
            )
            .unwrap(),
        )
        .unwrap();

        let menus = config.into_runtime_menus().unwrap();

        assert_eq!(menus[0].bus.as_str(), "session");
        assert_eq!(menus[0].local_id, "org_example_App:Menu");
    }

    #[test]
    fn config_accepts_system_bus() {
        let config = DbusMenuConfig::from_value(
            toml::from_str(
                r#"
menus = [{ service = "org.example.App", path = "/Menu", bus = "system" }]
"#,
            )
            .unwrap(),
        )
        .unwrap();

        assert_eq!(config.menus[0].bus, BusKind::System);
    }
}
