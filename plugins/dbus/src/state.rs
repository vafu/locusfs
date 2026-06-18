use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use locusfs_graph::{
    GraphChange, GraphError, LocusValue, NodeId, NodeKind, PropertyKey, PropertySpec, RelationName,
    Result,
};
use tokio::sync::RwLock;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue};

use crate::{DBUS_OBJECT_KIND, DBUS_SERVICE_KIND};

pub type SharedDbusState = Arc<RwLock<DbusState>>;

pub const OBJECT_RELATION: &str = "object";
pub const SERVICE_RELATION: &str = "dbus-service";

const SOURCE: &str = "dbus";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServiceConfig {
    pub local_id: String,
    pub bus: BusKind,
    pub name: String,
    pub object_manager_path: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BusKind {
    System,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ServiceSnapshot {
    pub config: ServiceConfig,
    pub owner: Option<String>,
    pub objects: BTreeMap<String, ObjectSnapshot>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ObjectSnapshot {
    pub service_local_id: String,
    pub path: String,
    pub interfaces: BTreeMap<String, BTreeMap<String, LocusValue>>,
}

#[derive(Debug)]
pub struct DbusState {
    services: BTreeMap<String, ServiceSnapshot>,
}

impl DbusState {
    pub fn shared(configs: Vec<ServiceConfig>) -> SharedDbusState {
        Arc::new(RwLock::new(Self::new(configs)))
    }

    pub fn new(configs: Vec<ServiceConfig>) -> Self {
        Self {
            services: configs
                .into_iter()
                .map(|config| {
                    (
                        config.local_id.clone(),
                        ServiceSnapshot {
                            config,
                            owner: None,
                            objects: BTreeMap::new(),
                        },
                    )
                })
                .collect(),
        }
    }

    pub fn set_service_snapshot(
        &mut self,
        local_id: &str,
        owner: Option<String>,
        objects: BTreeMap<String, ObjectSnapshot>,
    ) -> Result<Vec<GraphChange>> {
        let old = self
            .services
            .get(local_id)
            .ok_or_else(|| GraphError::NotFound {
                kind: "D-Bus service",
                name: local_id.to_string(),
            })?
            .clone();
        let service = self.services.get_mut(local_id).expect("service exists");
        service.owner = owner;
        service.objects = objects;

        Ok(service_snapshot_changes(&old, service)?)
    }

    pub fn contains_node(&self, node: &NodeId) -> Result<bool> {
        Ok(match node.kind().as_str() {
            DBUS_SERVICE_KIND => self.services.contains_key(node.local()),
            DBUS_OBJECT_KIND => self.object(node).is_some(),
            _ => false,
        })
    }

    pub fn nodes(&self, kind: &NodeKind) -> Result<Vec<NodeId>> {
        let mut nodes = match kind.as_str() {
            DBUS_SERVICE_KIND => self
                .services
                .keys()
                .map(|local| service_node(local))
                .collect::<Result<Vec<_>>>()?,
            DBUS_OBJECT_KIND => self
                .services
                .values()
                .flat_map(|service| {
                    service
                        .objects
                        .values()
                        .map(|object| service_object_node(service, object))
                })
                .collect::<Result<Vec<_>>>()?,
            _ => Vec::new(),
        };
        nodes.sort();
        Ok(nodes)
    }

    pub fn property_spec(&self, subject: &NodeId, key: &PropertyKey) -> Result<PropertySpec> {
        let value =
            self.node_properties(subject)?
                .remove(key)
                .ok_or_else(|| GraphError::NotFound {
                    kind: "property",
                    name: format!("{subject}/{key}"),
                })?;
        Ok(PropertySpec::new(key.clone(), value.kind()))
    }

    pub fn properties(&self, subject: &NodeId) -> Result<Vec<PropertySpec>> {
        Ok(self
            .node_properties(subject)?
            .into_iter()
            .map(|(key, value)| PropertySpec::new(key, value.kind()))
            .collect())
    }

    pub fn property(&self, subject: &NodeId, key: &PropertyKey) -> Result<LocusValue> {
        self.node_properties(subject)?
            .remove(key)
            .ok_or_else(|| GraphError::NotFound {
                kind: "property",
                name: format!("{subject}/{key}"),
            })
    }

    pub fn relations(&self, source: &NodeId) -> Result<Vec<RelationName>> {
        Ok(self.node_relations(source)?.into_keys().collect())
    }

    pub fn targets(&self, source: &NodeId, relation: &RelationName) -> Result<Vec<NodeId>> {
        self.node_relations(source)?
            .remove(relation)
            .ok_or_else(|| GraphError::NotFound {
                kind: "relation",
                name: format!("{source}/{relation}"),
            })
    }

    fn node_properties(&self, node: &NodeId) -> Result<BTreeMap<PropertyKey, LocusValue>> {
        match node.kind().as_str() {
            DBUS_SERVICE_KIND => self
                .services
                .get(node.local())
                .map(service_properties)
                .transpose()?
                .ok_or_else(|| node_not_found(node)),
            DBUS_OBJECT_KIND => self
                .object_entry(node)
                .map(|(service, object)| object_properties(service, object))
                .transpose()?
                .ok_or_else(|| node_not_found(node)),
            _ => Err(node_not_found(node)),
        }
    }

    fn node_relations(&self, node: &NodeId) -> Result<BTreeMap<RelationName, Vec<NodeId>>> {
        let mut relations = BTreeMap::new();
        match node.kind().as_str() {
            DBUS_SERVICE_KIND => {
                let service = self
                    .services
                    .get(node.local())
                    .ok_or_else(|| node_not_found(node))?;
                relations.insert(
                    relation(OBJECT_RELATION)?,
                    service
                        .objects
                        .values()
                        .map(|object| service_object_node(service, object))
                        .collect::<Result<Vec<_>>>()?,
                );
            }
            DBUS_OBJECT_KIND => {
                let (_, object) = self
                    .object_entry(node)
                    .ok_or_else(|| node_not_found(node))?;
                relations.insert(
                    relation(SERVICE_RELATION)?,
                    vec![service_node(&object.service_local_id)?],
                );
            }
            _ => return Err(node_not_found(node)),
        }
        Ok(relations)
    }

    fn object(&self, node: &NodeId) -> Option<&ObjectSnapshot> {
        self.object_entry(node).map(|(_, object)| object)
    }

    fn object_entry(&self, node: &NodeId) -> Option<(&ServiceSnapshot, &ObjectSnapshot)> {
        let (service_local_id, local_path) = object_local_parts(node.local())?;
        let service = self.services.get(service_local_id)?;
        let path = object_full_path(&service.config, local_path);
        let object = service.objects.get(path.as_str())?;
        Some((service, object))
    }
}

pub fn default_service_configs() -> Vec<ServiceConfig> {
    vec![ServiceConfig::system("org.freedesktop.UPower")]
}

impl ServiceConfig {
    pub fn system(name: impl Into<String>) -> Self {
        let name = name.into();
        let local_id = service_local_id(&name);
        let object_manager_path = format!("/{}", name.replace('.', "/"));
        Self {
            local_id,
            bus: BusKind::System,
            name,
            object_manager_path,
        }
    }
}

impl BusKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
        }
    }
}

pub fn object_snapshot(
    service_local_id: impl Into<String>,
    path: impl Into<String>,
    interfaces: BTreeMap<String, BTreeMap<String, LocusValue>>,
) -> ObjectSnapshot {
    ObjectSnapshot {
        service_local_id: service_local_id.into(),
        path: path.into(),
        interfaces,
    }
}

pub fn object_snapshot_from_managed(
    service_local_id: &str,
    path: &OwnedObjectPath,
    interfaces: &BTreeMap<String, BTreeMap<String, LocusValue>>,
) -> ObjectSnapshot {
    object_snapshot(
        service_local_id.to_string(),
        path.as_str().to_string(),
        interfaces.clone(),
    )
}

pub fn convert_managed_interfaces(
    interfaces: &std::collections::HashMap<
        zbus::names::OwnedInterfaceName,
        std::collections::HashMap<String, OwnedValue>,
    >,
) -> BTreeMap<String, BTreeMap<String, LocusValue>> {
    interfaces
        .iter()
        .map(|(interface, properties)| {
            (
                interface.to_string(),
                properties
                    .iter()
                    .map(|(key, value)| (key.clone(), locus_value_from_dbus(value)))
                    .collect(),
            )
        })
        .collect()
}

pub fn service_node(local_id: &str) -> Result<NodeId> {
    NodeId::new(NodeKind::new(DBUS_SERVICE_KIND)?, local_id)
}

fn service_object_node(service: &ServiceSnapshot, snapshot: &ObjectSnapshot) -> Result<NodeId> {
    NodeId::new(
        NodeKind::new(DBUS_OBJECT_KIND)?,
        object_local_id(
            &snapshot.service_local_id,
            object_display_path(&service.config, &snapshot.path),
        ),
    )
}

fn service_properties(snapshot: &ServiceSnapshot) -> Result<BTreeMap<PropertyKey, LocusValue>> {
    let mut properties = BTreeMap::new();
    insert(&mut properties, "kind", string(DBUS_SERVICE_KIND))?;
    insert(&mut properties, "source", string(SOURCE))?;
    insert(&mut properties, "bus", string(snapshot.config.bus.as_str()))?;
    insert(&mut properties, "name", string(&snapshot.config.name))?;
    insert(
        &mut properties,
        "object-manager-path",
        string(&snapshot.config.object_manager_path),
    )?;
    insert(
        &mut properties,
        "active",
        LocusValue::Bool(snapshot.owner.is_some()),
    )?;
    if let Some(owner) = snapshot.owner.as_deref() {
        insert(&mut properties, "owner", string(owner))?;
    }
    Ok(properties)
}

fn object_properties(
    service: &ServiceSnapshot,
    snapshot: &ObjectSnapshot,
) -> Result<BTreeMap<PropertyKey, LocusValue>> {
    let mut properties = BTreeMap::new();
    insert(&mut properties, "kind", string(DBUS_OBJECT_KIND))?;
    insert(&mut properties, "source", string(SOURCE))?;
    insert(
        &mut properties,
        "service",
        string(&snapshot.service_local_id),
    )?;
    insert(
        &mut properties,
        "service-name",
        string(&service.config.name),
    )?;
    insert(&mut properties, "path", string(&snapshot.path))?;
    insert(
        &mut properties,
        "interfaces",
        string(
            snapshot
                .interfaces
                .keys()
                .cloned()
                .collect::<Vec<_>>()
                .join(","),
        ),
    )?;

    let mut property_counts = BTreeMap::<String, usize>::new();
    for interface_properties in snapshot.interfaces.values() {
        for key in interface_properties.keys() {
            *property_counts.entry(key.clone()).or_default() += 1;
        }
    }

    for (interface, interface_properties) in &snapshot.interfaces {
        for (key, value) in interface_properties {
            insert_owned(&mut properties, format!("{interface}.{key}"), value.clone())?;
            if property_counts.get(key) == Some(&1) {
                insert_owned(&mut properties, key.clone(), value.clone())?;
            }
        }
    }
    Ok(properties)
}

fn service_snapshot_changes(
    old: &ServiceSnapshot,
    new: &ServiceSnapshot,
) -> Result<Vec<GraphChange>> {
    let mut changes = vec![
        GraphChange::NodeKindChanged {
            kind: NodeKind::new(DBUS_SERVICE_KIND)?,
        },
        GraphChange::NodeChanged {
            node: service_node(&new.config.local_id)?,
        },
        GraphChange::PropertyChanged {
            node: service_node(&new.config.local_id)?,
            key: PropertyKey::new("active")?,
        },
        GraphChange::PropertyChanged {
            node: service_node(&new.config.local_id)?,
            key: PropertyKey::new("owner")?,
        },
    ];

    let old_paths = old.objects.keys().cloned().collect::<BTreeSet<_>>();
    let new_paths = new.objects.keys().cloned().collect::<BTreeSet<_>>();
    if old_paths != new_paths {
        changes.push(GraphChange::NodeKindChanged {
            kind: NodeKind::new(DBUS_OBJECT_KIND)?,
        });
        changes.push(GraphChange::RelationChanged {
            source: service_node(&new.config.local_id)?,
            relation: relation(OBJECT_RELATION)?,
        });
    }

    for path in old_paths.difference(&new_paths) {
        let Some(object) = old.objects.get(path) else {
            continue;
        };
        changes.push(GraphChange::NodeRemoved {
            node: service_object_node(old, object)?,
        });
    }

    for path in new_paths {
        let Some(new_object) = new.objects.get(&path) else {
            continue;
        };
        let new_node = service_object_node(new, new_object)?;
        if old.objects.get(&path) != Some(new_object) {
            let is_new = !old.objects.contains_key(&path);
            changes.push(if is_new {
                GraphChange::NodeAdded {
                    node: new_node.clone(),
                }
            } else {
                GraphChange::NodeChanged {
                    node: new_node.clone(),
                }
            });
            changes.push(GraphChange::RelationChanged {
                source: new_node.clone(),
                relation: relation(SERVICE_RELATION)?,
            });
        }
        let old_properties = old
            .objects
            .get(&path)
            .map(|object| object_properties(old, object))
            .transpose()?
            .unwrap_or_default();
        let new_properties = object_properties(new, new_object)?;
        for key in changed_property_keys(&old_properties, &new_properties) {
            changes.push(GraphChange::PropertyChanged {
                node: new_node.clone(),
                key,
            });
        }
    }
    Ok(changes)
}

fn changed_property_keys(
    old: &BTreeMap<PropertyKey, LocusValue>,
    new: &BTreeMap<PropertyKey, LocusValue>,
) -> Vec<PropertyKey> {
    old.keys()
        .chain(new.keys())
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|key| old.get(key) != new.get(key))
        .collect()
}

fn object_local_id(service_local_id: &str, path: &str) -> String {
    format!("{service_local_id}:{path}")
}

fn object_local_parts(local_id: &str) -> Option<(&str, &str)> {
    local_id.split_once(':')
}

fn object_display_path<'a>(config: &ServiceConfig, path: &'a str) -> &'a str {
    if path == config.object_manager_path {
        "@"
    } else if let Some(stripped) = path
        .strip_prefix(config.object_manager_path.as_str())
        .and_then(|path| path.strip_prefix('/'))
    {
        stripped
    } else if let Some(stripped) = path.strip_prefix('/') {
        stripped
    } else {
        path
    }
}

fn object_full_path(config: &ServiceConfig, local_path: &str) -> String {
    if local_path == "@" {
        config.object_manager_path.clone()
    } else if local_path.starts_with('/') {
        local_path.to_string()
    } else {
        format!(
            "{}/{}",
            config.object_manager_path.trim_end_matches('/'),
            local_path
        )
    }
}

fn service_local_id(name: &str) -> String {
    name.rsplit('.').next().unwrap_or(name).to_ascii_lowercase()
}

pub(crate) fn locus_value_from_dbus(value: &OwnedValue) -> LocusValue {
    if let Ok(value) = bool::try_from(value) {
        return LocusValue::Bool(value);
    }
    if let Ok(value) = u32::try_from(value) {
        return LocusValue::U32(value);
    }
    if let Ok(value) = i32::try_from(value) {
        return LocusValue::I32(value);
    }
    if let Ok(value) = f64::try_from(value) {
        return LocusValue::F64(value);
    }
    if let Ok(value) = <&str>::try_from(value) {
        return LocusValue::String(value.to_string());
    }
    if let Ok(value) = <&ObjectPath<'_>>::try_from(value) {
        return LocusValue::String(value.to_string());
    }
    if let Ok(value) = u8::try_from(value) {
        return LocusValue::U32(value.into());
    }
    if let Ok(value) = u16::try_from(value) {
        return LocusValue::U32(value.into());
    }
    if let Ok(value) = i16::try_from(value) {
        return LocusValue::I32(value.into());
    }
    if let Ok(value) = i64::try_from(value)
        && let Ok(value) = i32::try_from(value)
    {
        return LocusValue::I32(value);
    }
    if let Ok(value) = u64::try_from(value)
        && let Ok(value) = u32::try_from(value)
    {
        return LocusValue::U32(value);
    }
    LocusValue::String(format!("{value:?}"))
}

fn insert(
    properties: &mut BTreeMap<PropertyKey, LocusValue>,
    key: &'static str,
    value: LocusValue,
) -> Result<()> {
    properties.insert(PropertyKey::new(key)?, value);
    Ok(())
}

fn insert_owned(
    properties: &mut BTreeMap<PropertyKey, LocusValue>,
    key: String,
    value: LocusValue,
) -> Result<()> {
    properties.insert(PropertyKey::new(key)?, value);
    Ok(())
}

fn relation(name: &str) -> Result<RelationName> {
    RelationName::new(name)
}

fn node_not_found(node: &NodeId) -> GraphError {
    GraphError::NotFound {
        kind: "node",
        name: node.to_string(),
    }
}

fn string(value: impl Into<String>) -> LocusValue {
    LocusValue::String(value.into())
}

#[cfg(test)]
mod test;
