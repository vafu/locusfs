use std::collections::BTreeMap;
use std::sync::Arc;

use locusfs_graph::{
    GraphChange, GraphError, LocusValue, NodeId, NodeKind, PropertyKey, PropertySpec, RelationName,
    Result,
};
use tokio::sync::RwLock;

use crate::{DBUSMENU_ITEM_KIND, DBUSMENU_KIND, DBUSMENU_MENU_KIND};

pub type SharedDbusMenuState = Arc<RwLock<DbusMenuState>>;

pub const MENU_NODE: &str = "menu";
pub const MENU_RELATION: &str = "menu";
pub const ITEM_RELATION: &str = "item";
pub const CHILD_RELATION: &str = "child";

const SOURCE: &str = "dbusmenu";
const ACTIVATE_PROPERTY: &str = "activate";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BusKind {
    Session,
    System,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DbusMenuEndpoint {
    pub local_id: String,
    pub bus: BusKind,
    pub service: String,
    pub path: String,
    pub revision: u32,
    pub root_items: Vec<i32>,
    pub items: BTreeMap<i32, DbusMenuItem>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DbusMenuItem {
    pub menu_id: String,
    pub bus: BusKind,
    pub service: String,
    pub path: String,
    pub item_id: i32,
    pub parent_id: Option<i32>,
    pub position: u32,
    pub child_ids: Vec<i32>,
    pub label: String,
    pub enabled: bool,
    pub visible: bool,
    pub item_type: String,
    pub toggle_type: String,
    pub toggle_state: i32,
    pub icon_name: String,
    pub disposition: String,
}

#[derive(Debug)]
pub struct DbusMenuState {
    menus: BTreeMap<String, DbusMenuEndpoint>,
}

impl DbusMenuState {
    pub fn shared(menus: Vec<DbusMenuEndpoint>) -> SharedDbusMenuState {
        Arc::new(RwLock::new(Self::new(menus)))
    }

    pub fn new(menus: Vec<DbusMenuEndpoint>) -> Self {
        Self {
            menus: menus
                .into_iter()
                .map(|menu| (menu.local_id.clone(), menu))
                .collect(),
        }
    }

    pub fn contains_node(&self, node: &NodeId) -> Result<bool> {
        Ok(match node.kind().as_str() {
            DBUSMENU_KIND => node.local() == MENU_NODE,
            DBUSMENU_MENU_KIND => self.menus.contains_key(node.local()),
            DBUSMENU_ITEM_KIND => self.item(node.local()).is_some(),
            _ => false,
        })
    }

    pub fn nodes(&self, kind: &NodeKind) -> Result<Vec<NodeId>> {
        let mut nodes = match kind.as_str() {
            DBUSMENU_KIND => vec![dbusmenu_node(MENU_NODE)?],
            DBUSMENU_MENU_KIND => self
                .menus
                .keys()
                .map(|local| menu_node(local))
                .collect::<Result<Vec<_>>>()?,
            DBUSMENU_ITEM_KIND => self
                .menus
                .values()
                .flat_map(|menu| menu.items.values())
                .map(|item| item_node(&item.local_id()))
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
        if subject.kind().as_str() == DBUSMENU_ITEM_KIND && key.as_str() == ACTIVATE_PROPERTY {
            return Ok(PropertySpec::read_write(key.clone(), value.kind()));
        }
        Ok(PropertySpec::new(key.clone(), value.kind()))
    }

    pub fn properties(&self, subject: &NodeId) -> Result<Vec<PropertySpec>> {
        Ok(self
            .node_properties(subject)?
            .into_iter()
            .map(|(key, value)| {
                if subject.kind().as_str() == DBUSMENU_ITEM_KIND
                    && key.as_str() == ACTIVATE_PROPERTY
                {
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

    fn node_properties(&self, node: &NodeId) -> Result<BTreeMap<PropertyKey, LocusValue>> {
        match node.kind().as_str() {
            DBUSMENU_KIND if node.local() == MENU_NODE => facade_properties(),
            DBUSMENU_MENU_KIND => self
                .menus
                .get(node.local())
                .map(endpoint_properties)
                .transpose()?
                .ok_or_else(|| node_not_found(node)),
            DBUSMENU_ITEM_KIND => self
                .item(node.local())
                .map(item_properties)
                .transpose()?
                .ok_or_else(|| node_not_found(node)),
            _ => Err(node_not_found(node)),
        }
    }

    fn node_relations(&self, node: &NodeId) -> Result<BTreeMap<RelationName, Vec<NodeId>>> {
        let mut relations = BTreeMap::new();
        match node.kind().as_str() {
            DBUSMENU_KIND if node.local() == MENU_NODE => {
                relations.insert(
                    relation(MENU_RELATION)?,
                    self.menus
                        .keys()
                        .map(|local| menu_node(local))
                        .collect::<Result<Vec<_>>>()?,
                );
            }
            DBUSMENU_MENU_KIND => {
                let menu = self
                    .menus
                    .get(node.local())
                    .ok_or_else(|| node_not_found(node))?;
                relations.insert(
                    relation(ITEM_RELATION)?,
                    menu.item_targets(&menu.root_items)?,
                );
            }
            DBUSMENU_ITEM_KIND => {
                let item = self
                    .item(node.local())
                    .ok_or_else(|| node_not_found(node))?;
                relations.insert(relation(CHILD_RELATION)?, item.child_targets()?);
            }
            _ => return Err(node_not_found(node)),
        }
        Ok(relations)
    }

    pub fn upsert_menu(&mut self, menu: DbusMenuEndpoint) -> Result<Vec<GraphChange>> {
        let old = self.menus.insert(menu.local_id.clone(), menu.clone());
        Ok(menu_changes(old.as_ref(), Some(&menu))?)
    }

    pub fn remove_service(&mut self, service: &str) -> Result<Vec<GraphChange>> {
        let ids = self
            .menus
            .iter()
            .filter_map(|(id, menu)| (menu.service == service).then_some(id.clone()))
            .collect::<Vec<_>>();
        let mut changes = Vec::new();
        for id in ids {
            if let Some(old) = self.menus.remove(&id) {
                changes.extend(menu_changes(Some(&old), None)?);
            }
        }
        Ok(changes)
    }

    pub fn activation_target(&self, node: &NodeId, key: &PropertyKey) -> Result<DbusMenuItem> {
        if node.kind().as_str() != DBUSMENU_ITEM_KIND || key.as_str() != ACTIVATE_PROPERTY {
            return Err(GraphError::NotFound {
                kind: "writable DBusMenu property",
                name: format!("{node}/{key}"),
            });
        }
        self.item(node.local())
            .filter(|item| item.enabled && item.visible)
            .ok_or_else(|| GraphError::NotFound {
                kind: "activatable DBusMenu item",
                name: node.to_string(),
            })
            .cloned()
    }

    fn item(&self, local: &str) -> Option<&DbusMenuItem> {
        let (prefix, item_id) = local.rsplit_once(':')?;
        let (menu_id, _) = prefix.rsplit_once(':')?;
        let item_id = item_id.parse::<i32>().ok()?;
        self.menus.get(menu_id)?.items.get(&item_id)
    }
}

impl BusKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::System => "system",
        }
    }
}

impl DbusMenuEndpoint {
    pub fn new(local_id: String, bus: BusKind, service: String, path: String) -> Self {
        Self {
            local_id,
            bus,
            service,
            path,
            revision: 0,
            root_items: Vec::new(),
            items: BTreeMap::new(),
        }
    }

    fn item_targets(&self, item_ids: &[i32]) -> Result<Vec<NodeId>> {
        item_ids
            .iter()
            .filter_map(|id| self.items.get(id))
            .map(|item| item_node(&item.local_id()))
            .collect()
    }
}

impl DbusMenuItem {
    pub fn local_id(&self) -> String {
        format!("{}:{:04}:{}", self.menu_id, self.position, self.item_id)
    }

    fn child_targets(&self) -> Result<Vec<NodeId>> {
        Ok(Vec::new())
    }
}

fn facade_properties() -> Result<BTreeMap<PropertyKey, LocusValue>> {
    let mut properties = BTreeMap::new();
    insert(&mut properties, "kind", string(DBUSMENU_KIND))?;
    insert(&mut properties, "source", string(SOURCE))?;
    Ok(properties)
}

fn endpoint_properties(endpoint: &DbusMenuEndpoint) -> Result<BTreeMap<PropertyKey, LocusValue>> {
    let mut properties = BTreeMap::new();
    insert(&mut properties, "kind", string(DBUSMENU_MENU_KIND))?;
    insert(&mut properties, "source", string(SOURCE))?;
    insert(&mut properties, "bus", string(endpoint.bus.as_str()))?;
    insert(&mut properties, "service", string(&endpoint.service))?;
    insert(&mut properties, "path", string(&endpoint.path))?;
    insert(
        &mut properties,
        "revision",
        LocusValue::U32(endpoint.revision),
    )?;
    Ok(properties)
}

fn item_properties(item: &DbusMenuItem) -> Result<BTreeMap<PropertyKey, LocusValue>> {
    let mut properties = BTreeMap::new();
    insert(&mut properties, "kind", string(DBUSMENU_ITEM_KIND))?;
    insert(&mut properties, "source", string(SOURCE))?;
    insert(&mut properties, "menu-id", string(&item.menu_id))?;
    insert(&mut properties, "item-id", LocusValue::I32(item.item_id))?;
    insert(&mut properties, "position", LocusValue::U32(item.position))?;
    insert(
        &mut properties,
        "parent-id",
        LocusValue::I32(item.parent_id.unwrap_or(0)),
    )?;
    insert(&mut properties, "label", string(&item.label))?;
    insert(&mut properties, "enabled", LocusValue::Bool(item.enabled))?;
    insert(&mut properties, "visible", LocusValue::Bool(item.visible))?;
    insert(&mut properties, "type", string(&item.item_type))?;
    insert(&mut properties, "toggle-type", string(&item.toggle_type))?;
    insert(
        &mut properties,
        "toggle-state",
        LocusValue::I32(item.toggle_state),
    )?;
    insert(&mut properties, "icon-name", string(&item.icon_name))?;
    insert(&mut properties, "disposition", string(&item.disposition))?;
    insert(&mut properties, ACTIVATE_PROPERTY, LocusValue::Bool(false))?;
    Ok(properties)
}

fn dbusmenu_node(local: &str) -> Result<NodeId> {
    NodeId::new(NodeKind::new(DBUSMENU_KIND)?, local)
}

fn menu_node(local: &str) -> Result<NodeId> {
    NodeId::new(NodeKind::new(DBUSMENU_MENU_KIND)?, local)
}

fn item_node(local: impl AsRef<str>) -> Result<NodeId> {
    NodeId::new(NodeKind::new(DBUSMENU_ITEM_KIND)?, local.as_ref())
}

fn menu_changes(
    old: Option<&DbusMenuEndpoint>,
    new: Option<&DbusMenuEndpoint>,
) -> Result<Vec<GraphChange>> {
    let mut changes = Vec::new();
    let facade = dbusmenu_node(MENU_NODE)?;

    match (old, new) {
        (None, Some(new)) => {
            changes.push(GraphChange::NodeAdded {
                node: menu_node(&new.local_id)?,
            });
            changes.push(GraphChange::RelationChanged {
                source: facade,
                relation: relation(MENU_RELATION)?,
            });
            changes.extend(item_kind_changes(None, Some(new))?);
            changes.push(GraphChange::RelationChanged {
                source: menu_node(&new.local_id)?,
                relation: relation(ITEM_RELATION)?,
            });
        }
        (Some(old), None) => {
            changes.extend(item_kind_changes(Some(old), None)?);
            changes.push(GraphChange::NodeRemoved {
                node: menu_node(&old.local_id)?,
            });
            changes.push(GraphChange::RelationChanged {
                source: facade,
                relation: relation(MENU_RELATION)?,
            });
        }
        (Some(old), Some(new)) => {
            if old != new {
                changes.push(GraphChange::NodeChanged {
                    node: menu_node(&new.local_id)?,
                });
                changes.extend(item_kind_changes(Some(old), Some(new))?);
                if old.root_items != new.root_items {
                    changes.push(GraphChange::RelationChanged {
                        source: menu_node(&new.local_id)?,
                        relation: relation(ITEM_RELATION)?,
                    });
                }
            }
        }
        (None, None) => {}
    }

    Ok(changes)
}

fn item_kind_changes(
    old: Option<&DbusMenuEndpoint>,
    new: Option<&DbusMenuEndpoint>,
) -> Result<Vec<GraphChange>> {
    let mut changes = Vec::new();
    let old_items = old
        .map(|menu| menu.items.keys().copied().collect::<Vec<_>>())
        .unwrap_or_default();
    let new_items = new
        .map(|menu| menu.items.keys().copied().collect::<Vec<_>>())
        .unwrap_or_default();
    for id in old_items.iter().filter(|id| !new_items.contains(id)) {
        if let Some(item) = old.and_then(|menu| menu.items.get(id)) {
            changes.push(GraphChange::NodeRemoved {
                node: item_node(item.local_id())?,
            });
        }
    }
    for id in new_items.iter().filter(|id| !old_items.contains(id)) {
        if let Some(item) = new.and_then(|menu| menu.items.get(id)) {
            changes.push(GraphChange::NodeAdded {
                node: item_node(item.local_id())?,
            });
        }
    }
    if let (Some(old), Some(new)) = (old, new) {
        for id in old_items.iter().filter(|id| new_items.contains(id)) {
            let Some(old_item) = old.items.get(id) else {
                continue;
            };
            let Some(new_item) = new.items.get(id) else {
                continue;
            };
            if old_item != new_item {
                changes.push(GraphChange::NodeChanged {
                    node: item_node(new_item.local_id())?,
                });
                if old_item.child_ids != new_item.child_ids {
                    changes.push(GraphChange::RelationChanged {
                        source: item_node(new_item.local_id())?,
                        relation: relation(CHILD_RELATION)?,
                    });
                }
            }
        }
    }

    Ok(changes)
}

fn insert(
    properties: &mut BTreeMap<PropertyKey, LocusValue>,
    key: &'static str,
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

pub fn menu_local_id(service: &str, path: &str) -> String {
    let service = service.trim_start_matches(':').replace(['.', '/'], "_");
    let path = path.trim_start_matches('/').replace('/', "_");
    if path.is_empty() {
        service
    } else {
        format!("{service}:{path}")
    }
}

#[cfg(test)]
mod test {
    use std::collections::BTreeMap;

    use locusfs_graph::{LocusValue, NodeKind, PropertyKey, RelationName};

    use super::{
        BusKind, DBUSMENU_ITEM_KIND, DBUSMENU_KIND, DBUSMENU_MENU_KIND, DbusMenuEndpoint,
        DbusMenuItem, DbusMenuState, MENU_RELATION, dbusmenu_node, menu_node,
    };

    #[test]
    fn exposes_configured_menu_endpoint() {
        let state = DbusMenuState::new(vec![DbusMenuEndpoint {
            local_id: "app:Menu".to_string(),
            bus: BusKind::Session,
            service: "org.example.App".to_string(),
            path: "/Menu".to_string(),
            revision: 0,
            root_items: Vec::new(),
            items: BTreeMap::new(),
        }]);

        let facade = dbusmenu_node("menu").unwrap();
        let endpoint = menu_node("app:Menu").unwrap();

        assert_eq!(
            state.nodes(&NodeKind::new(DBUSMENU_KIND).unwrap()).unwrap(),
            vec![facade.clone()]
        );
        assert_eq!(
            state
                .nodes(&NodeKind::new(DBUSMENU_MENU_KIND).unwrap())
                .unwrap(),
            vec![endpoint.clone()]
        );
        assert_eq!(
            state
                .targets(&facade, &RelationName::new(MENU_RELATION).unwrap())
                .unwrap(),
            vec![endpoint.clone()]
        );
        assert_eq!(
            state
                .property(&endpoint, &PropertyKey::new("service").unwrap())
                .unwrap(),
            LocusValue::String("org.example.App".to_string())
        );
    }

    #[test]
    fn exposes_activate_item_property_as_writable() {
        let mut menu = DbusMenuEndpoint {
            local_id: "app:Menu".to_string(),
            bus: BusKind::Session,
            service: "org.example.App".to_string(),
            path: "/Menu".to_string(),
            revision: 0,
            root_items: vec![1],
            items: BTreeMap::new(),
        };
        let item = DbusMenuItem {
            menu_id: menu.local_id.clone(),
            bus: BusKind::Session,
            service: menu.service.clone(),
            path: menu.path.clone(),
            item_id: 1,
            parent_id: None,
            position: 0,
            child_ids: Vec::new(),
            label: "Open".to_string(),
            enabled: true,
            visible: true,
            item_type: String::new(),
            toggle_type: String::new(),
            toggle_state: 0,
            icon_name: String::new(),
            disposition: String::new(),
        };
        let item_node = locusfs_graph::NodeId::new(
            NodeKind::new(DBUSMENU_ITEM_KIND).unwrap(),
            &item.local_id(),
        )
        .unwrap();
        menu.items.insert(item.item_id, item);
        let state = DbusMenuState::new(vec![menu]);

        let spec = state
            .property_spec(&item_node, &PropertyKey::new("activate").unwrap())
            .unwrap();

        assert!(spec.is_readable());
        assert!(spec.is_writable());
    }
}
