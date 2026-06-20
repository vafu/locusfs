use std::path::PathBuf;

use crate::config::{Config, PluginConfig};
use locusfs_graph::DynamicGraph;

use super::{merge_toml, plugin_library_name, resolve_library_path};

#[test]
fn plugin_library_name_matches_cargo_cdylib_output() {
    assert_eq!(
        plugin_library_name("dbus"),
        format!(
            "{}locusfs_plugin_dbus{}",
            std::env::consts::DLL_PREFIX,
            std::env::consts::DLL_SUFFIX
        )
    );
}

#[test]
fn explicit_library_path_wins() {
    let plugin = PluginConfig {
        enabled: true,
        library: Some(PathBuf::from("/tmp/custom.so")),
        config: toml::Value::Table(toml::map::Map::new()),
    };
    let config = Config::default();

    assert_eq!(
        resolve_library_path("project", &plugin, &config).unwrap(),
        PathBuf::from("/tmp/custom.so")
    );
}

#[test]
fn user_config_overrides_plugin_defaults_recursively() {
    let defaults = toml::from_str(
        r#"
service = { bus = "system", retries = 3 }
enabled = true
"#,
    )
    .unwrap();
    let overrides = toml::from_str(
        r#"
service = { retries = 5 }
"#,
    )
    .unwrap();

    let merged = merge_toml(defaults, overrides);

    assert_eq!(merged["service"]["bus"].as_str(), Some("system"));
    assert_eq!(merged["service"]["retries"].as_integer(), Some(5));
    assert_eq!(merged["enabled"].as_bool(), Some(true));
}

#[tokio::test]
async fn enabled_missing_plugin_fails_load() {
    let mut config = Config::default();
    config.plugins.insert(
        "missing".to_string(),
        PluginConfig {
            enabled: true,
            library: Some(PathBuf::from("/tmp/locusfs-missing-plugin.so")),
            config: toml::Value::Table(toml::map::Map::new()),
        },
    );

    assert!(
        super::PluginManager::load_enabled(&config, DynamicGraph::new())
            .await
            .is_err()
    );
}
