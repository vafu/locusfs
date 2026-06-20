use std::path::PathBuf;

use super::{Config, PluginConfig, expand_tilde};

#[test]
fn parses_plugin_config_sections() {
    let config = Config::parse(
        r#"
plugin_dirs = ["./target/debug"]

[plugins.project]
enabled = true

[plugins.dbus]
enabled = true
library = "/tmp/liblocusfs_plugin_dbus.so"

[plugins.dbus.config]
services = [
  { name = "org.example.Power", bus = "system", local_id = "power" },
]
"#,
    )
    .unwrap();

    assert_eq!(config.plugin_dirs, vec![PathBuf::from("./target/debug")]);
    assert_eq!(
        config.plugins.get("project"),
        Some(&PluginConfig {
            enabled: true,
            ..PluginConfig::default()
        })
    );
    assert_eq!(
        config.plugins["dbus"].library,
        Some(PathBuf::from("/tmp/liblocusfs_plugin_dbus.so"))
    );
    assert_eq!(
        config.plugins["dbus"].config["services"][0]["local_id"].as_str(),
        Some("power")
    );
}

#[test]
fn expands_home_relative_paths() {
    let expanded = expand_tilde(&PathBuf::from("~/plugins"));
    assert!(expanded.ends_with("plugins"));
    assert!(!expanded.starts_with("~"));
}
