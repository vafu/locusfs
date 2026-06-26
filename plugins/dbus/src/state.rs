use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use locusfs_graph::{
    GraphChange, GraphError, GraphPathChild, GraphPathDirectory, GraphPathEntry, GraphWatchTarget,
    LocusValue, NodeId, NodeKind, PathName, PropertyKey, PropertySpec, RelationName, Result,
};
use tokio::sync::RwLock;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue};

use crate::{DBUS_METHOD_KIND, DBUS_OBJECT_KIND, DBUS_SERVICE_KIND};

pub type SharedDbusState = Arc<RwLock<DbusState>>;

pub const OBJECT_RELATION: &str = "object";
pub const METHODS_RELATION: &str = "methods";
pub const SERVICE_RELATION: &str = "dbus";

const VIRTUAL_BUS: &str = "bus";
const VIRTUAL_SEPARATOR: &str = "|";
const CALL_PROPERTY: &str = "call";
const METHOD_CALL_SUFFIX: &str = ".call";

const SOURCE: &str = "dbus";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServiceConfig {
    pub local_id: String,
    pub bus: BusKind,
    pub name: String,
    pub object_manager_path: String,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum BusKind {
    System,
    Session,
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
    pub interfaces: BTreeMap<String, BTreeMap<String, DbusPropertySnapshot>>,
    pub methods: BTreeMap<String, BTreeMap<String, DbusMethodSnapshot>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DbusPropertySnapshot {
    pub value: LocusValue,
    pub writable: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DbusMethodSnapshot {
    pub input_signature: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DbusWritableProperty {
    pub service: ServiceConfig,
    pub object_path: String,
    pub interface: String,
    pub property: String,
    pub current: LocusValue,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DbusCallableMethod {
    pub service: ServiceConfig,
    pub object_path: String,
    pub interface: String,
    pub method: String,
    pub input_signature: Vec<String>,
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
            DBUS_SERVICE_KIND => self.contains_bus_node(node.local()),
            DBUS_OBJECT_KIND => self.object(node).is_some(),
            DBUS_METHOD_KIND => self.method(node).is_some(),
            _ => false,
        })
    }

    pub fn nodes(&self, kind: &NodeKind) -> Result<Vec<NodeId>> {
        let mut nodes = match kind.as_str() {
            DBUS_SERVICE_KIND => self
                .configured_buses()
                .into_iter()
                .map(|bus| service_node(bus.as_str()))
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
            DBUS_METHOD_KIND => self
                .services
                .values()
                .flat_map(|service| {
                    service.objects.values().flat_map(|object| {
                        object.methods.iter().flat_map(|(interface, methods)| {
                            methods.keys().map(|method| {
                                service_method_node(service, object, interface, method)
                            })
                        })
                    })
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
        if self.callable_method(subject, key).is_ok() {
            Ok(PropertySpec::write_only(key.clone(), value.kind()))
        } else if self.writable_property(subject, key).is_ok() {
            Ok(PropertySpec::read_write(key.clone(), value.kind()))
        } else {
            Ok(PropertySpec::new(key.clone(), value.kind()))
        }
    }

    pub fn properties(&self, subject: &NodeId) -> Result<Vec<PropertySpec>> {
        Ok(self
            .node_properties(subject)?
            .into_iter()
            .map(|(key, value)| {
                if self.callable_method(subject, &key).is_ok() {
                    PropertySpec::write_only(key, value.kind())
                } else if self.writable_property(subject, &key).is_ok() {
                    PropertySpec::read_write(key, value.kind())
                } else {
                    PropertySpec::new(key, value.kind())
                }
            })
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

    pub fn path_lookup_child(
        &self,
        parent: &GraphPathDirectory,
        name: &PathName,
    ) -> Result<Option<GraphPathEntry>> {
        match parent {
            GraphPathDirectory::Node(node) if node.kind().as_str() == DBUS_SERVICE_KIND => {
                if !self.contains_node(node)? {
                    return Ok(None);
                }
                let Some(bus) = bus_kind_from_local(node.local()) else {
                    return Ok(None);
                };
                self.bus_tree_lookup(bus, &[], name)
            }
            GraphPathDirectory::Virtual { owner, local } if owner.as_str() == DBUS_SERVICE_KIND => {
                self.virtual_path_lookup(local, name)
            }
            _ => Ok(None),
        }
    }

    pub fn path_children(
        &self,
        parent: &GraphPathDirectory,
    ) -> Result<Option<Vec<GraphPathChild>>> {
        match parent {
            GraphPathDirectory::Node(node) if node.kind().as_str() == DBUS_SERVICE_KIND => {
                if !self.contains_node(node)? {
                    return Ok(None);
                }
                let Some(bus) = bus_kind_from_local(node.local()) else {
                    return Ok(None);
                };
                self.bus_tree_children(bus, &[]).map(Some)
            }
            GraphPathDirectory::Virtual { owner, local } if owner.as_str() == DBUS_SERVICE_KIND => {
                self.virtual_path_children(local).map(Some)
            }
            _ => Ok(None),
        }
    }

    pub fn path_watch_target(
        &self,
        directory: &GraphPathDirectory,
    ) -> Result<Option<GraphWatchTarget>> {
        match directory {
            GraphPathDirectory::Node(node) if node.kind().as_str() == DBUS_SERVICE_KIND => {
                if !self.contains_node(node)? {
                    return Ok(None);
                }
                Ok(Some(GraphWatchTarget::Node(node.clone())))
            }
            GraphPathDirectory::Virtual { owner, local } if owner.as_str() == DBUS_SERVICE_KIND => {
                self.virtual_path_watch_target(local).map(Some)
            }
            _ => Ok(None),
        }
    }

    pub fn writable_property(
        &self,
        subject: &NodeId,
        key: &PropertyKey,
    ) -> Result<DbusWritableProperty> {
        let Some(resolved) = self.resolve_dbus_property(subject, key)? else {
            return Err(GraphError::NotFound {
                kind: "property",
                name: format!("{subject}/{key}"),
            });
        };
        if !resolved.snapshot.writable {
            return Err(GraphError::InvalidValue {
                kind: "D-Bus property",
                value: format!("{subject}/{key}"),
                reason: "property is not writable",
            });
        }
        Ok(DbusWritableProperty {
            service: resolved.service.config.clone(),
            object_path: resolved.object.path.clone(),
            interface: resolved.interface.to_string(),
            property: resolved.property.to_string(),
            current: resolved.snapshot.value.clone(),
        })
    }

    pub fn callable_method(
        &self,
        subject: &NodeId,
        key: &PropertyKey,
    ) -> Result<DbusCallableMethod> {
        if key.as_str() != "call" {
            return Err(GraphError::NotFound {
                kind: "property",
                name: format!("{subject}/{key}"),
            });
        }
        let Some((service, object, interface, method, snapshot)) = self.method_entry(subject)
        else {
            return Err(node_not_found(subject));
        };
        Ok(DbusCallableMethod {
            service: service.config.clone(),
            object_path: object.path.clone(),
            interface: interface.to_string(),
            method: method.to_string(),
            input_signature: snapshot.input_signature.clone(),
        })
    }

    pub fn update_cached_property(
        &mut self,
        subject: &NodeId,
        key: &PropertyKey,
        value: LocusValue,
    ) -> Result<()> {
        let writable = self.writable_property(subject, key)?;
        let service = self
            .services
            .get_mut(&writable.service.local_id)
            .ok_or_else(|| node_not_found(subject))?;
        let object = service
            .objects
            .get_mut(&writable.object_path)
            .ok_or_else(|| node_not_found(subject))?;
        let property = object
            .interfaces
            .get_mut(&writable.interface)
            .and_then(|properties| properties.get_mut(&writable.property))
            .ok_or_else(|| GraphError::NotFound {
                kind: "property",
                name: format!("{subject}/{key}"),
            })?;
        property.value = value;
        Ok(())
    }

    fn node_properties(&self, node: &NodeId) -> Result<BTreeMap<PropertyKey, LocusValue>> {
        match node.kind().as_str() {
            DBUS_SERVICE_KIND => {
                let bus = bus_kind_from_local(node.local()).ok_or_else(|| node_not_found(node))?;
                if !self.contains_bus(bus) {
                    return Err(node_not_found(node));
                }
                Ok(bus_properties(bus, self.services_on_bus(bus))?)
            }
            DBUS_OBJECT_KIND => self
                .object_entry(node)
                .map(|(service, object)| object_properties(service, object))
                .transpose()?
                .ok_or_else(|| node_not_found(node)),
            DBUS_METHOD_KIND => self
                .method_entry(node)
                .map(|(service, object, interface, method, snapshot)| {
                    method_properties(service, object, interface, method, snapshot)
                })
                .transpose()?
                .ok_or_else(|| node_not_found(node)),
            _ => Err(node_not_found(node)),
        }
    }

    fn node_relations(&self, node: &NodeId) -> Result<BTreeMap<RelationName, Vec<NodeId>>> {
        match node.kind().as_str() {
            DBUS_SERVICE_KIND => {
                let bus = bus_kind_from_local(node.local()).ok_or_else(|| node_not_found(node))?;
                if !self.contains_bus(bus) {
                    return Err(node_not_found(node));
                }
            }
            DBUS_OBJECT_KIND => {
                self.object_entry(node)
                    .ok_or_else(|| node_not_found(node))?;
            }
            DBUS_METHOD_KIND => {
                self.method_entry(node)
                    .ok_or_else(|| node_not_found(node))?;
            }
            _ => return Err(node_not_found(node)),
        }
        Ok(BTreeMap::new())
    }

    fn object(&self, node: &NodeId) -> Option<&ObjectSnapshot> {
        self.object_entry(node).map(|(_, object)| object)
    }

    fn virtual_path_lookup(&self, local: &str, name: &PathName) -> Result<Option<GraphPathEntry>> {
        let parts = virtual_parts(local);
        match parts.first().map(String::as_str) {
            Some(VIRTUAL_BUS) if parts.len() >= 2 => {
                let Some(bus) = bus_kind_from_local(parts[1].as_str()) else {
                    return Ok(None);
                };
                self.bus_tree_lookup(bus, &parts[2..], name)
            }
            _ => Ok(None),
        }
    }

    fn virtual_path_children(&self, local: &str) -> Result<Vec<GraphPathChild>> {
        let parts = virtual_parts(local);
        match parts.first().map(String::as_str) {
            Some(VIRTUAL_BUS) if parts.len() >= 2 => {
                let Some(bus) = bus_kind_from_local(parts[1].as_str()) else {
                    return Ok(Vec::new());
                };
                self.bus_tree_children(bus, &parts[2..])
            }
            _ => Ok(Vec::new()),
        }
    }

    fn virtual_path_watch_target(&self, local: &str) -> Result<GraphWatchTarget> {
        let parts = virtual_parts(local);
        match parts.first().map(String::as_str) {
            Some(VIRTUAL_BUS) if parts.len() >= 2 => {
                let bus =
                    bus_kind_from_local(parts[1].as_str()).ok_or_else(|| GraphError::NotFound {
                        kind: "D-Bus virtual path",
                        name: local.to_string(),
                    })?;
                if self.exact_object_on_bus(bus, &parts[2..])?.is_some() {
                    return Ok(GraphWatchTarget::Kind(NodeKind::new(DBUS_OBJECT_KIND)?));
                }
                Ok(GraphWatchTarget::Node(service_node(bus.as_str())?))
            }
            _ => Err(GraphError::NotFound {
                kind: "D-Bus virtual path",
                name: local.to_string(),
            }),
        }
    }

    fn bus_tree_lookup(
        &self,
        bus: BusKind,
        segments: &[String],
        name: &PathName,
    ) -> Result<Option<GraphPathEntry>> {
        let mut child = segments.to_vec();
        child.push(name.as_str().to_string());
        if self.has_object_prefix_on_bus(bus, &child)? {
            return Ok(Some(GraphPathEntry::Directory(
                GraphPathDirectory::Virtual {
                    owner: NodeKind::new(DBUS_SERVICE_KIND)?,
                    local: tree_virtual_local(VIRTUAL_BUS, bus.as_str(), &child),
                },
            )));
        }

        let Some(object_node) = self.exact_object_on_bus(bus, segments)? else {
            return Ok(None);
        };
        let key = PropertyKey::new(name.as_str())?;
        if self.resolve_dbus_property(&object_node, &key)?.is_some() {
            return Ok(Some(GraphPathEntry::Property {
                node: object_node,
                key,
            }));
        }

        let Some(display) = method_display_from_call_file(name.as_str()) else {
            return Ok(None);
        };
        let Some(method_node) = self.method_by_display(&object_node, display)? else {
            return Ok(None);
        };
        Ok(Some(GraphPathEntry::Property {
            node: method_node,
            key: PropertyKey::new(CALL_PROPERTY)?,
        }))
    }

    fn bus_tree_children(&self, bus: BusKind, segments: &[String]) -> Result<Vec<GraphPathChild>> {
        let child_segments = self.child_object_segment_names_on_bus(bus, segments)?;
        let mut used_names = child_segments.iter().cloned().collect::<BTreeSet<_>>();
        let mut children = child_segments
            .into_iter()
            .map(|segment| {
                let mut child = segments.to_vec();
                child.push(segment.clone());
                Ok(GraphPathChild {
                    name: PathName::new(segment)?,
                    entry: GraphPathEntry::Directory(GraphPathDirectory::Virtual {
                        owner: NodeKind::new(DBUS_SERVICE_KIND)?,
                        local: tree_virtual_local(VIRTUAL_BUS, bus.as_str(), &child),
                    }),
                })
            })
            .collect::<Result<Vec<_>>>()?;

        if let Some(object_node) = self.exact_object_on_bus(bus, segments)? {
            for name in self.dbus_property_names(&object_node, &used_names)? {
                if !used_names.insert(name.clone()) {
                    continue;
                }
                let key = PropertyKey::new(name.as_str())?;
                children.push(GraphPathChild {
                    name: PathName::new(name)?,
                    entry: GraphPathEntry::Property {
                        node: object_node.clone(),
                        key,
                    },
                });
            }
            for (name, method_node) in self.dbus_method_call_names(&object_node, &used_names)? {
                if !used_names.insert(name.clone()) {
                    continue;
                }
                children.push(GraphPathChild {
                    name: PathName::new(name)?,
                    entry: GraphPathEntry::Property {
                        node: method_node,
                        key: PropertyKey::new(CALL_PROPERTY)?,
                    },
                });
            }
        }

        Ok(children)
    }

    fn dbus_property_names(
        &self,
        object_node: &NodeId,
        reserved_names: &BTreeSet<String>,
    ) -> Result<Vec<String>> {
        let Some((_, object)) = self.object_entry(object_node) else {
            return Err(node_not_found(object_node));
        };

        let mut property_counts = BTreeMap::<String, usize>::new();
        for interface_properties in object.interfaces.values() {
            for property in interface_properties.keys() {
                *property_counts.entry(property.clone()).or_default() += 1;
            }
        }

        let mut names = BTreeSet::new();
        for (interface, interface_properties) in &object.interfaces {
            for property in interface_properties.keys() {
                let canonical = format!("{interface}.{property}");
                if !reserved_names.contains(&canonical) {
                    names.insert(canonical);
                }
                if property_counts.get(property) == Some(&1) && !reserved_names.contains(property) {
                    names.insert(property.clone());
                }
            }
        }
        Ok(names.into_iter().collect())
    }

    fn dbus_method_call_names(
        &self,
        object_node: &NodeId,
        reserved_names: &BTreeSet<String>,
    ) -> Result<Vec<(String, NodeId)>> {
        let Some((service, object)) = self.object_entry(object_node) else {
            return Err(node_not_found(object_node));
        };

        let mut method_counts = BTreeMap::<String, usize>::new();
        for interface_methods in object.methods.values() {
            for method in interface_methods.keys() {
                *method_counts.entry(method.clone()).or_default() += 1;
            }
        }

        let mut names = BTreeMap::new();
        for (interface, interface_methods) in &object.methods {
            for method in interface_methods.keys() {
                let method_node = service_method_node(service, object, interface, method)?;
                let canonical = method_call_file_name(&format!("{interface}.{method}"));
                if !reserved_names.contains(&canonical) {
                    names.insert(canonical, method_node.clone());
                }
                let short = method_call_file_name(method);
                if method_counts.get(method) == Some(&1) && !reserved_names.contains(&short) {
                    names.insert(short, method_node);
                }
            }
        }
        Ok(names.into_iter().collect())
    }

    fn exact_object_on_bus(&self, bus: BusKind, segments: &[String]) -> Result<Option<NodeId>> {
        let matches = self
            .object_views_on_bus(bus)?
            .into_iter()
            .filter(|view| view.segments == segments)
            .map(|view| view.node)
            .collect::<Vec<_>>();

        match matches.as_slice() {
            [] => Ok(None),
            [node] => Ok(Some(node.clone())),
            _ => Err(GraphError::InvalidValue {
                kind: "D-Bus object path",
                value: dbus_path_from_segments(segments),
                reason: "multiple configured services expose this object path",
            }),
        }
    }

    fn has_object_prefix_on_bus(&self, bus: BusKind, segments: &[String]) -> Result<bool> {
        Ok(self
            .object_views_on_bus(bus)?
            .iter()
            .any(|view| view.segments.starts_with(segments)))
    }

    fn child_object_segment_names_on_bus(
        &self,
        bus: BusKind,
        segments: &[String],
    ) -> Result<Vec<String>> {
        let mut names = self
            .object_views_on_bus(bus)?
            .into_iter()
            .filter_map(|view| {
                if view.segments.len() > segments.len() && view.segments.starts_with(segments) {
                    Some(view.segments[segments.len()].clone())
                } else {
                    None
                }
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        names.sort();
        Ok(names)
    }

    fn object_views_on_bus(&self, bus: BusKind) -> Result<Vec<ObjectPathView>> {
        self.services_on_bus(bus)
            .flat_map(|service| {
                service
                    .objects
                    .values()
                    .map(move |object| (service, object))
            })
            .map(|(service, object)| {
                Ok(ObjectPathView {
                    node: service_object_node(service, object)?,
                    segments: object_path_segments(&object.path),
                })
            })
            .collect()
    }

    fn services_on_bus(&self, bus: BusKind) -> impl Iterator<Item = &ServiceSnapshot> {
        self.services
            .values()
            .filter(move |service| service.config.bus == bus)
    }

    fn contains_bus(&self, bus: BusKind) -> bool {
        self.services_on_bus(bus).next().is_some()
    }

    fn contains_bus_node(&self, local: &str) -> bool {
        bus_kind_from_local(local)
            .map(|bus| self.contains_bus(bus))
            .unwrap_or(false)
    }

    fn configured_buses(&self) -> Vec<BusKind> {
        self.services
            .values()
            .map(|service| service.config.bus)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    fn method_by_display(&self, object: &NodeId, display: &str) -> Result<Option<NodeId>> {
        let Some((service, object)) = self.object_entry(object) else {
            return Err(node_not_found(object));
        };

        let mut method_counts = BTreeMap::<String, usize>::new();
        for interface_methods in object.methods.values() {
            for method in interface_methods.keys() {
                *method_counts.entry(method.clone()).or_default() += 1;
            }
        }

        let mut matches = Vec::new();
        for (interface, methods) in &object.methods {
            for method in methods.keys() {
                let canonical = format!("{interface}.{method}");
                let short = method_counts.get(method) == Some(&1) && display == method;
                if short || display == canonical {
                    matches.push(service_method_node(service, object, interface, method)?);
                }
            }
        }

        Ok(if matches.len() == 1 {
            matches.pop()
        } else {
            None
        })
    }

    fn object_entry(&self, node: &NodeId) -> Option<(&ServiceSnapshot, &ObjectSnapshot)> {
        let (service_local_id, local_path) = object_local_parts(node.local())?;
        let service = self.services.get(service_local_id)?;
        let object = service.objects.get(local_path)?;
        Some((service, object))
    }

    fn method(&self, node: &NodeId) -> Option<&DbusMethodSnapshot> {
        self.method_entry(node)
            .map(|(_, _, _, _, snapshot)| snapshot)
    }

    fn method_entry(
        &self,
        node: &NodeId,
    ) -> Option<(
        &ServiceSnapshot,
        &ObjectSnapshot,
        &str,
        &str,
        &DbusMethodSnapshot,
    )> {
        if node.kind().as_str() != DBUS_METHOD_KIND {
            return None;
        }
        let (object_local_id, method_display) = method_local_parts(node.local())?;
        let object_node =
            NodeId::new(NodeKind::new(DBUS_OBJECT_KIND).ok()?, object_local_id).ok()?;
        let (service, object) = self.object_entry(&object_node)?;
        let mut matches = object
            .methods
            .iter()
            .flat_map(|(interface, methods)| {
                methods
                    .iter()
                    .filter(move |(method, _)| {
                        method_display == method.as_str()
                            || method_display == format!("{interface}.{method}")
                    })
                    .map(move |(method, snapshot)| {
                        (
                            service,
                            object,
                            interface.as_str(),
                            method.as_str(),
                            snapshot,
                        )
                    })
            })
            .collect::<Vec<_>>();

        if matches.len() == 1 {
            matches.pop()
        } else {
            None
        }
    }

    fn resolve_dbus_property<'a>(
        &'a self,
        subject: &NodeId,
        key: &PropertyKey,
    ) -> Result<Option<ResolvedDbusProperty<'a>>> {
        if subject.kind().as_str() != DBUS_OBJECT_KIND {
            return Ok(None);
        }
        let Some((service, object)) = self.object_entry(subject) else {
            return Err(node_not_found(subject));
        };

        let mut matches = Vec::new();
        for (interface, properties) in &object.interfaces {
            for (property, snapshot) in properties {
                if key.as_str() == property || key.as_str() == format!("{interface}.{property}") {
                    matches.push(ResolvedDbusProperty {
                        service,
                        object,
                        interface,
                        property,
                        snapshot,
                    });
                }
            }
        }

        Ok(if matches.len() == 1 {
            matches.into_iter().next()
        } else {
            None
        })
    }
}

struct ResolvedDbusProperty<'a> {
    service: &'a ServiceSnapshot,
    object: &'a ObjectSnapshot,
    interface: &'a str,
    property: &'a str,
    snapshot: &'a DbusPropertySnapshot,
}

struct ObjectPathView {
    node: NodeId,
    segments: Vec<String>,
}

impl ServiceConfig {
    #[cfg(test)]
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
            Self::Session => "session",
        }
    }
}

pub fn object_snapshot(
    service_local_id: impl Into<String>,
    path: impl Into<String>,
    interfaces: BTreeMap<String, BTreeMap<String, DbusPropertySnapshot>>,
) -> ObjectSnapshot {
    ObjectSnapshot {
        service_local_id: service_local_id.into(),
        path: path.into(),
        interfaces,
        methods: BTreeMap::new(),
    }
}

pub fn object_snapshot_from_managed(
    service_local_id: &str,
    path: &OwnedObjectPath,
    interfaces: &BTreeMap<String, BTreeMap<String, DbusPropertySnapshot>>,
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
) -> BTreeMap<String, BTreeMap<String, DbusPropertySnapshot>> {
    interfaces
        .iter()
        .map(|(interface, properties)| {
            (
                interface.to_string(),
                properties
                    .iter()
                    .map(|(key, value)| {
                        (
                            key.clone(),
                            DbusPropertySnapshot {
                                value: locus_value_from_dbus(value),
                                writable: false,
                            },
                        )
                    })
                    .collect(),
            )
        })
        .collect()
}

pub fn service_node(local_id: &str) -> Result<NodeId> {
    NodeId::new(NodeKind::new(DBUS_SERVICE_KIND)?, local_id)
}

fn service_object_node(_service: &ServiceSnapshot, snapshot: &ObjectSnapshot) -> Result<NodeId> {
    NodeId::new(
        NodeKind::new(DBUS_OBJECT_KIND)?,
        object_local_id(&snapshot.service_local_id, &snapshot.path),
    )
}

fn service_method_node(
    _service: &ServiceSnapshot,
    object: &ObjectSnapshot,
    interface: &str,
    method: &str,
) -> Result<NodeId> {
    NodeId::new(
        NodeKind::new(DBUS_METHOD_KIND)?,
        method_local_id(
            &object_local_id(&object.service_local_id, &object.path),
            method_display_name(object, interface, method),
        ),
    )
}

fn bus_properties<'a>(
    bus: BusKind,
    services: impl Iterator<Item = &'a ServiceSnapshot>,
) -> Result<BTreeMap<PropertyKey, LocusValue>> {
    let mut properties = BTreeMap::new();
    insert(&mut properties, "kind", string(DBUS_SERVICE_KIND))?;
    insert(&mut properties, "source", string(SOURCE))?;
    insert(&mut properties, "bus", string(bus.as_str()))?;
    let services = services.collect::<Vec<_>>();
    insert(
        &mut properties,
        "services",
        string(
            services
                .iter()
                .map(|service| service.config.local_id.as_str())
                .collect::<Vec<_>>()
                .join(",")
                .as_str(),
        ),
    )?;
    insert(
        &mut properties,
        "active-services",
        string(
            services
                .iter()
                .filter(|service| service.owner.is_some())
                .map(|service| service.config.local_id.as_str())
                .collect::<Vec<_>>()
                .join(",")
                .as_str(),
        ),
    )?;
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
        for (key, snapshot) in interface_properties {
            insert_owned(
                &mut properties,
                format!("{interface}.{key}"),
                snapshot.value.clone(),
            )?;
            if property_counts.get(key) == Some(&1) {
                insert_owned(&mut properties, key.clone(), snapshot.value.clone())?;
            }
        }
    }
    Ok(properties)
}

fn method_properties(
    service: &ServiceSnapshot,
    object: &ObjectSnapshot,
    interface: &str,
    method: &str,
    snapshot: &DbusMethodSnapshot,
) -> Result<BTreeMap<PropertyKey, LocusValue>> {
    let mut properties = BTreeMap::new();
    insert(&mut properties, "kind", string(DBUS_METHOD_KIND))?;
    insert(&mut properties, "source", string(SOURCE))?;
    insert(&mut properties, "service", string(&object.service_local_id))?;
    insert(
        &mut properties,
        "service-name",
        string(&service.config.name),
    )?;
    insert(&mut properties, "object-path", string(&object.path))?;
    insert(&mut properties, "interface", string(interface))?;
    insert(&mut properties, "method", string(method))?;
    insert(
        &mut properties,
        "input-signature",
        string(snapshot.input_signature.join(",").as_str()),
    )?;
    insert(&mut properties, "call", string(""))?;
    Ok(properties)
}

fn service_snapshot_changes(
    old: &ServiceSnapshot,
    new: &ServiceSnapshot,
) -> Result<Vec<GraphChange>> {
    let bus_node = service_node(new.config.bus.as_str())?;
    let mut changes = vec![
        GraphChange::NodeKindChanged {
            kind: NodeKind::new(DBUS_SERVICE_KIND)?,
        },
        GraphChange::NodeChanged {
            node: bus_node.clone(),
        },
        GraphChange::PropertyChanged {
            node: bus_node.clone(),
            key: PropertyKey::new("services")?,
        },
        GraphChange::PropertyChanged {
            node: bus_node.clone(),
            key: PropertyKey::new("active-services")?,
        },
    ];

    let old_paths = old.objects.keys().cloned().collect::<BTreeSet<_>>();
    let new_paths = new.objects.keys().cloned().collect::<BTreeSet<_>>();
    if old_paths != new_paths {
        changes.push(GraphChange::NodeKindChanged {
            kind: NodeKind::new(DBUS_OBJECT_KIND)?,
        });
        changes.push(GraphChange::NodeKindChanged {
            kind: NodeKind::new(DBUS_METHOD_KIND)?,
        });
        changes.push(GraphChange::RelationChanged {
            source: bus_node.clone(),
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
        for (interface, methods) in &object.methods {
            for method in methods.keys() {
                changes.push(GraphChange::NodeRemoved {
                    node: service_method_node(old, object, interface, method)?,
                });
            }
        }
    }

    for path in new_paths {
        let Some(new_object) = new.objects.get(&path) else {
            continue;
        };
        let new_node = service_object_node(new, new_object)?;
        let old_methods = old
            .objects
            .get(&path)
            .map(object_method_keys)
            .unwrap_or_default();
        let new_methods = object_method_keys(new_object);
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
        if old_methods != new_methods {
            changes.push(GraphChange::NodeKindChanged {
                kind: NodeKind::new(DBUS_METHOD_KIND)?,
            });
            changes.push(GraphChange::NodeChanged {
                node: new_node.clone(),
            });
            changes.push(GraphChange::RelationChanged {
                source: new_node.clone(),
                relation: relation(METHODS_RELATION)?,
            });
        }
        for (interface, method) in old_methods.difference(&new_methods) {
            if let Some(old_object) = old.objects.get(&path) {
                changes.push(GraphChange::NodeRemoved {
                    node: service_method_node(old, old_object, interface, method)?,
                });
            }
        }
        for (interface, method) in new_methods {
            let method_node = service_method_node(new, new_object, &interface, &method)?;
            if old_methods.contains(&(interface.clone(), method.clone())) {
                changes.push(GraphChange::NodeChanged { node: method_node });
            } else {
                changes.push(GraphChange::NodeAdded { node: method_node });
            }
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

fn object_method_keys(object: &ObjectSnapshot) -> BTreeSet<(String, String)> {
    object
        .methods
        .iter()
        .flat_map(|(interface, methods)| {
            methods
                .keys()
                .map(|method| (interface.clone(), method.clone()))
        })
        .collect()
}

fn tree_virtual_local(root: &str, local_id: &str, segments: &[String]) -> String {
    std::iter::once(root.to_string())
        .chain(std::iter::once(local_id.to_string()))
        .chain(segments.iter().cloned())
        .map(|part| encode_virtual_part(&part))
        .collect::<Vec<_>>()
        .join(VIRTUAL_SEPARATOR)
}

fn virtual_parts(local: &str) -> Vec<String> {
    local
        .split(VIRTUAL_SEPARATOR)
        .map(decode_virtual_part)
        .collect()
}

fn object_path_segments(path: &str) -> Vec<String> {
    path.trim_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn dbus_path_from_segments(segments: &[String]) -> String {
    if segments.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", segments.join("/"))
    }
}

fn method_call_file_name(display: &str) -> String {
    format!("{display}{METHOD_CALL_SUFFIX}")
}

fn method_display_from_call_file(name: &str) -> Option<&str> {
    name.strip_suffix(METHOD_CALL_SUFFIX)
        .filter(|display| !display.is_empty())
}

fn encode_virtual_part(value: impl AsRef<str>) -> String {
    value.as_ref().replace('%', "%25").replace('|', "%7C")
}

fn decode_virtual_part(value: &str) -> String {
    let mut decoded = String::new();
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '%' {
            let hi = chars.next();
            let lo = chars.next();
            match (hi, lo) {
                (Some('2'), Some('5')) => decoded.push('%'),
                (Some('7'), Some('C')) | (Some('7'), Some('c')) => decoded.push('|'),
                (Some(hi), Some(lo)) => {
                    decoded.push('%');
                    decoded.push(hi);
                    decoded.push(lo);
                }
                (hi, lo) => {
                    decoded.push('%');
                    if let Some(hi) = hi {
                        decoded.push(hi);
                    }
                    if let Some(lo) = lo {
                        decoded.push(lo);
                    }
                }
            }
        } else {
            decoded.push(ch);
        }
    }
    decoded
}

fn object_local_id(service_local_id: &str, path: &str) -> String {
    format!("{service_local_id}:{path}")
}

fn object_local_parts(local_id: &str) -> Option<(&str, &str)> {
    local_id.split_once(':')
}

fn method_local_id(object_local_id: &str, method_display: impl AsRef<str>) -> String {
    format!("{}:{}", object_local_id, method_display.as_ref())
}

fn method_local_parts(local_id: &str) -> Option<(&str, &str)> {
    local_id.rsplit_once(':')
}

fn method_display_name(object: &ObjectSnapshot, interface: &str, method: &str) -> String {
    let count = object
        .methods
        .values()
        .filter(|methods| methods.contains_key(method))
        .count();
    if count == 1 {
        method.to_string()
    } else {
        format!("{interface}.{method}")
    }
}

fn bus_kind_from_local(local: &str) -> Option<BusKind> {
    match local {
        "system" => Some(BusKind::System),
        "session" => Some(BusKind::Session),
        _ => None,
    }
}

#[cfg(test)]
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
