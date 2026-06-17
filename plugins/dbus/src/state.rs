use std::collections::BTreeMap;
use std::sync::Arc;

use locusfs_graph::{
    GraphChange, GraphError, LocusValue, NodeId, NodeKind, PropertyKey, PropertySpec, Result,
};
use tokio::sync::RwLock;

use crate::DBUS_SERVICE_KIND;

pub type SharedDbusState = Arc<RwLock<DbusState>>;

const SOURCE: &str = "dbus";
const UPOWER_LOCAL_ID: &str = "upower";
const UPOWER_BUS: &str = "system";
const UPOWER_SERVICE: &str = "org.freedesktop.UPower";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServiceSnapshot {
    pub local_id: &'static str,
    pub bus: &'static str,
    pub service: &'static str,
    pub owner: Option<String>,
}

#[derive(Debug)]
pub struct DbusState {
    upower: ServiceSnapshot,
}

impl Default for DbusState {
    fn default() -> Self {
        Self {
            upower: ServiceSnapshot {
                local_id: UPOWER_LOCAL_ID,
                bus: UPOWER_BUS,
                service: UPOWER_SERVICE,
                owner: None,
            },
        }
    }
}

impl DbusState {
    pub fn shared() -> SharedDbusState {
        Arc::new(RwLock::new(Self::default()))
    }

    pub fn set_upower_owner(&mut self, owner: Option<String>) -> Result<Vec<GraphChange>> {
        if self.upower.owner == owner {
            return Ok(Vec::new());
        }

        self.upower.owner = owner;
        let node = upower_node()?;
        Ok(vec![
            GraphChange::NodeChanged { node: node.clone() },
            property_change(node.clone(), "active")?,
            property_change(node, "owner")?,
        ])
    }

    pub fn contains_node(&self, node: &NodeId) -> Result<bool> {
        Ok(node.kind().as_str() == DBUS_SERVICE_KIND && node.local() == UPOWER_LOCAL_ID)
    }

    pub fn nodes(&self, kind: &NodeKind) -> Result<Vec<NodeId>> {
        if kind.as_str() == DBUS_SERVICE_KIND {
            Ok(vec![upower_node()?])
        } else {
            Ok(Vec::new())
        }
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

    fn node_properties(&self, node: &NodeId) -> Result<BTreeMap<PropertyKey, LocusValue>> {
        if !self.contains_node(node)? {
            return Err(node_not_found(node));
        }
        service_properties(&self.upower)
    }
}

fn service_properties(snapshot: &ServiceSnapshot) -> Result<BTreeMap<PropertyKey, LocusValue>> {
    let mut properties = BTreeMap::new();
    insert(&mut properties, "kind", string(DBUS_SERVICE_KIND))?;
    insert(&mut properties, "source", string(SOURCE))?;
    insert(&mut properties, "bus", string(snapshot.bus))?;
    insert(&mut properties, "name", string(snapshot.service))?;
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

pub(crate) fn upower_node() -> Result<NodeId> {
    NodeId::new(NodeKind::new(DBUS_SERVICE_KIND)?, UPOWER_LOCAL_ID)
}

pub(crate) fn upower_service_name() -> &'static str {
    UPOWER_SERVICE
}

fn insert(
    properties: &mut BTreeMap<PropertyKey, LocusValue>,
    key: &'static str,
    value: LocusValue,
) -> Result<()> {
    properties.insert(PropertyKey::new(key)?, value);
    Ok(())
}

fn property_change(node: NodeId, key: &'static str) -> Result<GraphChange> {
    Ok(GraphChange::PropertyChanged {
        node,
        key: PropertyKey::new(key)?,
    })
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
