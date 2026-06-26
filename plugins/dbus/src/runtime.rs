use std::collections::{BTreeMap, BTreeSet};

use futures_util::StreamExt;
use locusfs_graph::{DynamicGraph, GraphChange, GraphError, Result};
use tokio::runtime::Handle;
use tokio::task::JoinHandle;
use tokio::time::{Duration, sleep};
use zbus::fdo::{DBusProxy, IntrospectableProxy, ObjectManagerProxy, PropertiesProxy};
use zbus::message::Type as MessageType;
use zbus::names::{BusName, InterfaceName};
use zbus::{MatchRule, MessageStream};

use crate::state::{
    BusKind, DbusMethodSnapshot, DbusPropertySnapshot, ObjectSnapshot, ServiceConfig,
    SharedDbusState, convert_managed_interfaces, object_snapshot_from_managed,
};
use crate::{DBUS_METHOD_KIND, DBUS_OBJECT_KIND, DBUS_SERVICE_KIND};

#[derive(Debug, Default)]
pub struct DbusRuntime;

impl DbusRuntime {
    pub fn start(
        graph: DynamicGraph,
        config: crate::config::DbusConfig,
        runtime: Handle,
    ) -> Result<(SharedDbusState, Vec<JoinHandle<()>>)> {
        let configs = config.into_runtime_services()?;
        let state = crate::state::DbusState::shared(configs.clone());
        let watchers = configs
            .into_iter()
            .map(|config| {
                spawn_service_watcher(config, state.clone(), graph.clone(), runtime.clone())
            })
            .collect();
        Ok((state, watchers))
    }
}

fn spawn_service_watcher(
    config: ServiceConfig,
    state: SharedDbusState,
    graph: DynamicGraph,
    runtime: Handle,
) -> JoinHandle<()> {
    runtime.spawn(async move {
        loop {
            if let Err(error) = watch_service(config.clone(), state.clone(), graph.clone()).await {
                eprintln!(
                    "locusfs-dbus: service watcher for {} stopped: {error}",
                    config.name
                );
            }
            sleep_retry().await;
        }
    })
}

async fn watch_service(
    config: ServiceConfig,
    state: SharedDbusState,
    graph: DynamicGraph,
) -> Result<()> {
    let connection = match config.bus {
        BusKind::System => zbus::Connection::system()
            .await
            .map_err(|error| GraphError::Io(format!("connect to system D-Bus: {error}")))?,
        BusKind::Session => zbus::Connection::session()
            .await
            .map_err(|error| GraphError::Io(format!("connect to session D-Bus: {error}")))?,
    };
    let dbus = DBusProxy::new(&connection)
        .await
        .map_err(|error| GraphError::Io(format!("create D-Bus proxy: {error}")))?;
    let mut owner_changed = dbus
        .receive_name_owner_changed()
        .await
        .map_err(|error| GraphError::Io(format!("watch NameOwnerChanged: {error}")))?;

    let mut owner = publish_current_snapshot(&connection, &dbus, &config, &state, &graph).await;

    loop {
        let Some(owner_name) = owner.as_deref() else {
            let Some(signal) = owner_changed.next().await else {
                return Err(GraphError::Io(
                    "D-Bus NameOwnerChanged stream ended".to_string(),
                ));
            };
            let args = signal
                .args()
                .map_err(|error| GraphError::Io(format!("read NameOwnerChanged args: {error}")))?;
            if args.name().as_str() == config.name {
                owner = publish_current_snapshot(&connection, &dbus, &config, &state, &graph).await;
            }
            continue;
        };

        let mut property_changes = dbus_signal_stream(
            &connection,
            owner_name,
            "org.freedesktop.DBus.Properties",
            None,
        )
        .await?;
        let mut object_changes = dbus_signal_stream(
            &connection,
            owner_name,
            "org.freedesktop.DBus.ObjectManager",
            None,
        )
        .await?;

        loop {
            tokio::select! {
                signal = owner_changed.next() => {
                    let Some(signal) = signal else {
                        return Err(GraphError::Io(
                            "D-Bus NameOwnerChanged stream ended".to_string(),
                        ));
                    };
                    let args = signal
                        .args()
                        .map_err(|error| GraphError::Io(format!("read NameOwnerChanged args: {error}")))?;
                    if args.name().as_str() == config.name {
                        owner = publish_current_snapshot(&connection, &dbus, &config, &state, &graph).await;
                        break;
                    }
                }
                message = property_changes.next() => {
                    read_signal_message(message, "PropertiesChanged")?;
                    let _ = publish_current_snapshot(&connection, &dbus, &config, &state, &graph).await;
                }
                message = object_changes.next() => {
                    read_signal_message(message, "ObjectManager")?;
                    owner = publish_current_snapshot(&connection, &dbus, &config, &state, &graph).await;
                    break;
                }
            }
        }
    }
}

async fn publish_current_snapshot(
    connection: &zbus::Connection,
    dbus: &DBusProxy<'_>,
    config: &ServiceConfig,
    state: &SharedDbusState,
    graph: &DynamicGraph,
) -> Option<String> {
    match snapshot_service(connection, dbus, config).await {
        Ok(snapshot) => {
            let owner = snapshot.owner.clone();
            publish_snapshot(state, graph, Ok(snapshot)).await;
            owner
        }
        Err(error) => {
            publish_snapshot(state, graph, Err(error)).await;
            None
        }
    }
}

async fn dbus_signal_stream(
    connection: &zbus::Connection,
    owner: &str,
    interface: &'static str,
    member: Option<&'static str>,
) -> Result<MessageStream> {
    let mut builder = MatchRule::builder()
        .msg_type(MessageType::Signal)
        .sender(owner)
        .map_err(|error| GraphError::Io(format!("create D-Bus signal sender match: {error}")))?
        .interface(interface)
        .map_err(|error| GraphError::Io(format!("create D-Bus signal interface match: {error}")))?;
    if let Some(member) = member {
        builder = builder.member(member).map_err(|error| {
            GraphError::Io(format!("create D-Bus signal member match: {error}"))
        })?;
    }
    MessageStream::for_match_rule(builder.build(), connection, Some(256))
        .await
        .map_err(|error| GraphError::Io(format!("subscribe to D-Bus {interface} signals: {error}")))
}

fn read_signal_message(
    message: Option<zbus::Result<zbus::Message>>,
    signal: &'static str,
) -> Result<()> {
    match message {
        Some(Ok(_)) => Ok(()),
        Some(Err(error)) => Err(GraphError::Io(format!(
            "read D-Bus {signal} signal: {error}"
        ))),
        None => Err(GraphError::Io(format!(
            "D-Bus {signal} signal stream ended"
        ))),
    }
}

async fn snapshot_service(
    connection: &zbus::Connection,
    dbus: &DBusProxy<'_>,
    config: &ServiceConfig,
) -> Result<ServiceRuntimeSnapshot> {
    let owner = current_owner(dbus, &config.name).await?;
    let objects = if owner.is_some() {
        managed_objects(connection, config).await?
    } else {
        BTreeMap::new()
    };
    Ok(ServiceRuntimeSnapshot {
        local_id: config.local_id.clone(),
        owner,
        objects,
    })
}

async fn current_owner(dbus: &DBusProxy<'_>, service_name: &str) -> Result<Option<String>> {
    let service = BusName::try_from(service_name).map_err(|_| GraphError::InvalidValue {
        kind: "D-Bus service",
        value: service_name.to_string(),
        reason: "invalid bus name",
    })?;
    match dbus.get_name_owner(service).await {
        Ok(owner) => Ok(Some(owner.to_string())),
        Err(zbus::fdo::Error::NameHasNoOwner(_)) | Err(zbus::fdo::Error::ServiceUnknown(_)) => {
            Ok(None)
        }
        Err(error) => Err(GraphError::Io(format!(
            "read D-Bus owner for {service_name}: {error}"
        ))),
    }
}

async fn managed_objects(
    connection: &zbus::Connection,
    config: &ServiceConfig,
) -> Result<BTreeMap<String, ObjectSnapshot>> {
    let mut errors = Vec::new();
    match managed_objects_at(connection, config, &config.object_manager_path).await {
        Ok(objects) if !objects.is_empty() => {
            return annotate_introspection(connection, config, objects).await;
        }
        Ok(_) => {}
        Err(error) => errors.push(error.to_string()),
    }

    if config.object_manager_path != "/" {
        match managed_objects_at(connection, config, "/").await {
            Ok(objects) if !objects.is_empty() => {
                return annotate_introspection(connection, config, objects).await;
            }
            Ok(_) => {}
            Err(error) => errors.push(error.to_string()),
        }
    }

    match introspected_objects(connection, config).await {
        Ok(objects) => Ok(objects),
        Err(error) => {
            errors.push(error.to_string());
            Err(GraphError::Io(format!(
                "snapshot D-Bus objects for {} failed: {}",
                config.name,
                errors.join("; ")
            )))
        }
    }
}

async fn sleep_retry() {
    sleep(Duration::from_secs(1)).await;
}

async fn annotate_introspection(
    connection: &zbus::Connection,
    config: &ServiceConfig,
    mut objects: BTreeMap<String, ObjectSnapshot>,
) -> Result<BTreeMap<String, ObjectSnapshot>> {
    for object in objects.values_mut() {
        let xml = match introspect_path(connection, config, &object.path).await {
            Ok(xml) => xml,
            Err(_) => continue,
        };
        for (interface, properties) in &mut object.interfaces {
            let writable_properties = parse_interface_property_access(&xml, interface);
            for (property, snapshot) in properties {
                if let Some(writable) = writable_properties.get(property) {
                    snapshot.writable = *writable;
                }
            }
        }
        object.methods = parse_interface_methods(&xml);
    }
    Ok(objects)
}

async fn managed_objects_at(
    connection: &zbus::Connection,
    config: &ServiceConfig,
    path: &str,
) -> Result<BTreeMap<String, ObjectSnapshot>> {
    let proxy = ObjectManagerProxy::builder(connection)
        .destination(config.name.as_str())
        .map_err(|error| GraphError::Io(format!("create object manager destination: {error}")))?
        .path(path)
        .map_err(|error| GraphError::Io(format!("create object manager path {path}: {error}")))?
        .build()
        .await
        .map_err(|error| {
            GraphError::Io(format!(
                "create object manager proxy for {} at {path}: {error}",
                config.name
            ))
        })?;

    let objects = proxy.get_managed_objects().await.map_err(|error| {
        GraphError::Io(format!(
            "read managed objects for {} at {path}: {error}",
            config.name
        ))
    })?;
    Ok(objects
        .iter()
        .map(|(path, interfaces)| {
            let interfaces = convert_managed_interfaces(interfaces);
            (
                path.as_str().to_string(),
                object_snapshot_from_managed(&config.local_id, path, &interfaces),
            )
        })
        .collect())
}

async fn introspected_objects(
    connection: &zbus::Connection,
    config: &ServiceConfig,
) -> Result<BTreeMap<String, ObjectSnapshot>> {
    let mut objects = BTreeMap::new();
    let mut visited = BTreeSet::new();
    let mut pending = vec![config.object_manager_path.clone()];
    if config.object_manager_path != "/" {
        pending.push("/".to_string());
    }

    while let Some(path) = pending.pop() {
        if !visited.insert(path.clone()) {
            continue;
        }
        let xml = match introspect_path(connection, config, &path).await {
            Ok(xml) => xml,
            Err(_) => continue,
        };

        let mut interfaces = BTreeMap::new();
        let all_methods = parse_interface_methods(&xml);
        let mut methods = BTreeMap::new();
        for interface in parse_interface_names(&xml) {
            if is_standard_interface(&interface) {
                continue;
            }
            let interface_methods = all_methods.get(&interface).cloned().unwrap_or_default();
            if !interface_methods.is_empty() {
                methods.insert(interface.clone(), interface_methods);
            }
            let writable_properties = parse_interface_property_access(&xml, &interface);
            let properties = match properties_for_interface(
                connection,
                config,
                &path,
                &interface,
                &writable_properties,
            )
            .await
            {
                Ok(properties) => properties,
                Err(_) => continue,
            };
            if !properties.is_empty() {
                interfaces.insert(interface, properties);
            }
        }

        if !interfaces.is_empty() || !methods.is_empty() {
            let object = ObjectSnapshot {
                service_local_id: config.local_id.clone(),
                path: path.clone(),
                interfaces,
                methods,
            };
            objects.insert(path.clone(), object);
        }

        for child in parse_child_node_names(&xml) {
            pending.push(join_object_path(&path, &child));
        }
    }

    Ok(objects)
}

async fn introspect_path(
    connection: &zbus::Connection,
    config: &ServiceConfig,
    path: &str,
) -> Result<String> {
    let proxy = IntrospectableProxy::builder(connection)
        .destination(config.name.as_str())
        .map_err(|error| GraphError::Io(format!("create introspection destination: {error}")))?
        .path(path)
        .map_err(|error| GraphError::Io(format!("create introspection path {path}: {error}")))?
        .build()
        .await
        .map_err(|error| {
            GraphError::Io(format!(
                "create introspection proxy for {} at {path}: {error}",
                config.name
            ))
        })?;
    proxy
        .introspect()
        .await
        .map_err(|error| GraphError::Io(format!("introspect {} at {path}: {error}", config.name)))
}

async fn properties_for_interface(
    connection: &zbus::Connection,
    config: &ServiceConfig,
    path: &str,
    interface: &str,
    writable_properties: &BTreeMap<String, bool>,
) -> Result<BTreeMap<String, DbusPropertySnapshot>> {
    let proxy = PropertiesProxy::builder(connection)
        .destination(config.name.as_str())
        .map_err(|error| GraphError::Io(format!("create properties destination: {error}")))?
        .path(path)
        .map_err(|error| GraphError::Io(format!("create properties path {path}: {error}")))?
        .build()
        .await
        .map_err(|error| {
            GraphError::Io(format!(
                "create properties proxy for {} at {path}: {error}",
                config.name
            ))
        })?;
    let interface_name =
        InterfaceName::try_from(interface).map_err(|_| GraphError::InvalidValue {
            kind: "D-Bus interface",
            value: interface.to_string(),
            reason: "invalid interface name",
        })?;
    let properties = proxy.get_all(interface_name).await.map_err(|error| {
        GraphError::Io(format!("read {interface} properties at {path}: {error}"))
    })?;
    Ok(properties
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                DbusPropertySnapshot {
                    value: crate::state::locus_value_from_dbus(value),
                    writable: writable_properties.get(key).copied().unwrap_or(false),
                },
            )
        })
        .collect())
}

fn parse_interface_names(xml: &str) -> Vec<String> {
    parse_named_tags(xml, "interface")
}

fn parse_child_node_names(xml: &str) -> Vec<String> {
    parse_named_tags(xml, "node")
        .into_iter()
        .filter(|name| !name.starts_with('/'))
        .collect()
}

fn parse_interface_property_access(xml: &str, interface: &str) -> BTreeMap<String, bool> {
    let mut properties = BTreeMap::new();
    for body in interface_bodies(xml, interface) {
        let needle = "<property";
        let mut remaining = body;
        while let Some(index) = remaining.find(needle) {
            remaining = &remaining[index + needle.len()..];
            let Some(first) = remaining.chars().next() else {
                break;
            };
            if !(first.is_whitespace() || first == '/' || first == '>') {
                continue;
            }
            let Some(end) = remaining.find('>') else {
                break;
            };
            let tag = &remaining[..end];
            if let Some(name) = attr_value(tag, "name") {
                let writable = matches!(
                    attr_value(tag, "access").as_deref(),
                    Some("readwrite" | "write")
                );
                properties.insert(name, writable);
            }
            remaining = &remaining[end + 1..];
        }
    }
    properties
}

fn parse_interface_methods(xml: &str) -> BTreeMap<String, BTreeMap<String, DbusMethodSnapshot>> {
    let mut methods = BTreeMap::new();
    for interface in parse_interface_names(xml) {
        if is_standard_interface(&interface) {
            continue;
        }
        let mut interface_methods = BTreeMap::new();
        for body in interface_bodies(xml, &interface) {
            let needle = "<method";
            let mut remaining = body;
            while let Some(index) = remaining.find(needle) {
                remaining = &remaining[index + needle.len()..];
                let Some(first) = remaining.chars().next() else {
                    break;
                };
                if !(first.is_whitespace() || first == '/' || first == '>') {
                    continue;
                }
                let Some(tag_end) = remaining.find('>') else {
                    break;
                };
                let tag = &remaining[..tag_end];
                let Some(name) = attr_value(tag, "name") else {
                    remaining = &remaining[tag_end + 1..];
                    continue;
                };
                let body_start = tag_end + 1;
                let Some(body_end) = remaining[body_start..].find("</method>") else {
                    remaining = &remaining[body_start..];
                    continue;
                };
                let method_body = &remaining[body_start..body_start + body_end];
                interface_methods.insert(
                    name,
                    DbusMethodSnapshot {
                        input_signature: parse_input_arg_types(method_body),
                    },
                );
                remaining = &remaining[body_start + body_end + "</method>".len()..];
            }
        }
        if !interface_methods.is_empty() {
            methods.insert(interface, interface_methods);
        }
    }
    methods
}

fn parse_input_arg_types(method_body: &str) -> Vec<String> {
    let mut args = Vec::new();
    let needle = "<arg";
    let mut remaining = method_body;
    while let Some(index) = remaining.find(needle) {
        remaining = &remaining[index + needle.len()..];
        let Some(first) = remaining.chars().next() else {
            break;
        };
        if !(first.is_whitespace() || first == '/' || first == '>') {
            continue;
        }
        let Some(end) = remaining.find('>') else {
            break;
        };
        let tag = &remaining[..end];
        let direction = attr_value(tag, "direction");
        if direction.as_deref().unwrap_or("in") == "in"
            && let Some(arg_type) = attr_value(tag, "type")
        {
            args.push(arg_type);
        }
        remaining = &remaining[end + 1..];
    }
    args
}

fn interface_bodies<'a>(xml: &'a str, interface: &str) -> Vec<&'a str> {
    let mut bodies = Vec::new();
    let mut remaining = xml;
    while let Some(index) = remaining.find("<interface") {
        remaining = &remaining[index + "<interface".len()..];
        let Some(first) = remaining.chars().next() else {
            break;
        };
        if !(first.is_whitespace() || first == '/' || first == '>') {
            continue;
        }
        let Some(tag_end) = remaining.find('>') else {
            break;
        };
        let tag = &remaining[..tag_end];
        let body_start = tag_end + 1;
        if attr_value(tag, "name").as_deref() == Some(interface) {
            if let Some(body_end) = remaining[body_start..].find("</interface>") {
                bodies.push(&remaining[body_start..body_start + body_end]);
            }
        }
        remaining = &remaining[body_start..];
    }
    bodies
}

fn parse_named_tags(xml: &str, tag: &str) -> Vec<String> {
    let needle = format!("<{tag}");
    let mut names = Vec::new();
    let mut remaining = xml;
    while let Some(index) = remaining.find(&needle) {
        remaining = &remaining[index + needle.len()..];
        let Some(first) = remaining.chars().next() else {
            break;
        };
        if !(first.is_whitespace() || first == '/' || first == '>') {
            continue;
        }
        let Some(end) = remaining.find('>') else {
            break;
        };
        if let Some(name) = attr_value(&remaining[..end], "name") {
            names.push(name);
        }
        remaining = &remaining[end + 1..];
    }
    names
}

fn attr_value(tag_body: &str, attr: &str) -> Option<String> {
    let mut remaining = tag_body;
    while let Some(index) = remaining.find(attr) {
        let before = remaining[..index].chars().next_back();
        let after = remaining[index + attr.len()..].chars().next();
        let is_name_boundary = before
            .is_none_or(|character| character.is_whitespace() || character == '/')
            && after.is_some_and(|character| character.is_whitespace() || character == '=');
        remaining = &remaining[index + attr.len()..];
        if !is_name_boundary {
            continue;
        }
        remaining = remaining.trim_start();
        let value = remaining.strip_prefix('=')?.trim_start();
        let mut chars = value.chars();
        let quote = chars.next()?;
        if quote != '"' && quote != '\'' {
            return None;
        }
        let value = chars.as_str();
        let end = value.find(quote)?;
        return Some(value[..end].to_string());
    }
    None
}

fn is_standard_interface(interface: &str) -> bool {
    matches!(
        interface,
        "org.freedesktop.DBus.Introspectable"
            | "org.freedesktop.DBus.Peer"
            | "org.freedesktop.DBus.Properties"
            | "org.freedesktop.DBus.ObjectManager"
    )
}

fn join_object_path(parent: &str, child: &str) -> String {
    if parent == "/" {
        format!("/{child}")
    } else {
        format!("{parent}/{child}")
    }
}

async fn publish_snapshot(
    state: &SharedDbusState,
    graph: &DynamicGraph,
    snapshot: Result<ServiceRuntimeSnapshot>,
) {
    let snapshot = match snapshot {
        Ok(snapshot) => snapshot,
        Err(error) => {
            eprintln!("locusfs-dbus: failed to read D-Bus service snapshot: {error}");
            return;
        }
    };

    let changes = {
        let mut state = state.write().await;
        match state.set_service_snapshot(&snapshot.local_id, snapshot.owner, snapshot.objects) {
            Ok(changes) => changes,
            Err(error) => {
                eprintln!("locusfs-dbus: failed to update service snapshot: {error}");
                return;
            }
        }
    };

    for change in leading_kind_changes().into_iter().chain(changes) {
        emit_global_change(graph, change);
    }
}

fn leading_kind_changes() -> Vec<GraphChange> {
    [DBUS_SERVICE_KIND, DBUS_OBJECT_KIND, DBUS_METHOD_KIND]
        .into_iter()
        .filter_map(|kind| {
            Some(GraphChange::NodeKindChanged {
                kind: locusfs_graph::NodeKind::new(kind).ok()?,
            })
        })
        .collect()
}

fn emit_global_change(graph: &DynamicGraph, change: GraphChange) {
    if let Err(error) = graph.emit_global_change(change) {
        eprintln!("locusfs-dbus: failed to emit graph change: {error}");
    }
}

#[derive(Debug)]
struct ServiceRuntimeSnapshot {
    local_id: String,
    owner: Option<String>,
    objects: BTreeMap<String, ObjectSnapshot>,
}

#[cfg(test)]
mod test {
    use locusfs_graph::{DynamicGraph, LocusValue, PropertyKey};
    use tokio::time::{Duration, Instant, sleep};
    use zbus::fdo::DBusProxy;

    use super::{current_owner, parse_child_node_names, parse_interface_names};
    use crate::config::{DbusConfig, ServiceConfig};
    use crate::register_with_config;
    use crate::state::service_node;

    #[test]
    fn introspection_parser_accepts_attribute_variants() {
        let xml = r#"
            <node>
              <interface name = 'org.example.First'/>
              <interface version="1" name="org.example.Second"></interface>
              <node name = "child"/>
              <node name='/absolute-is-ignored'/>
            </node>
        "#;

        assert_eq!(
            parse_interface_names(xml),
            vec![
                "org.example.First".to_string(),
                "org.example.Second".to_string()
            ]
        );
        assert_eq!(parse_child_node_names(xml), vec!["child".to_string()]);
    }

    #[tokio::test]
    async fn realtime_default_service_owner_matches_system_bus_snapshot_when_available() {
        let service_name = "org.freedesktop.UPower";
        let expected_owner = match system_service_owner(service_name).await {
            Ok(owner) => owner,
            Err(error) => {
                eprintln!("skipping realtime D-Bus service test: {error}");
                return;
            }
        };

        let graph = DynamicGraph::new();
        let _plugin = register_with_config(
            &graph,
            DbusConfig {
                services: vec![ServiceConfig {
                    name: service_name.to_string(),
                    bus: crate::config::BusKind::System,
                    local_id: Some("upower".to_string()),
                    object_manager_path: None,
                }],
            },
        )
        .await
        .expect("dbus plugin registers");
        let node = service_node("system").expect("service node id is valid");
        let active = PropertyKey::new("active-services").expect("active property key is valid");
        let expected = LocusValue::String(if expected_owner.is_some() {
            "upower".to_string()
        } else {
            String::new()
        });
        let deadline = Instant::now() + Duration::from_secs(5);

        loop {
            if graph.property(&node, &active).await.ok() == Some(expected.clone()) {
                return;
            }

            assert!(
                Instant::now() < deadline,
                "timed out waiting for service active state {expected:?}"
            );
            sleep(Duration::from_millis(50)).await;
        }
    }

    async fn system_service_owner(service_name: &str) -> locusfs_graph::Result<Option<String>> {
        let connection = zbus::Connection::system()
            .await
            .map_err(|error| locusfs_graph::GraphError::Io(error.to_string()))?;
        let dbus = DBusProxy::new(&connection)
            .await
            .map_err(|error| locusfs_graph::GraphError::Io(error.to_string()))?;
        current_owner(&dbus, service_name).await
    }
}
