use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use locusfs_graph::{
    GraphChange, GraphError, GraphPathChild, GraphPathDirectory, GraphPathEntry, GraphWatchTarget,
    LocusValue, NodeId, NodeKind, PathName, PropertyKey, PropertySpec, RelationName, Result,
};
use tokio::sync::RwLock;

use crate::{STATUS_NOTIFIER_ITEM_KIND, STATUS_NOTIFIER_KIND};

pub type SharedStatusNotifierState = Arc<RwLock<StatusNotifierState>>;

pub const ITEM_NODE: &str = "item";

const SOURCE: &str = "statusnotifier";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct StatusNotifierSnapshot {
    pub items: BTreeMap<String, StatusNotifierItem>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct StatusNotifierItem {
    pub id: String,
    pub service_name: String,
    pub path: String,
    pub category: String,
    pub title: String,
    pub status: String,
    pub icon_name: String,
    pub attention_icon_name: String,
    pub overlay_icon_name: String,
    pub menu_path: String,
    pub item_is_menu: bool,
}

#[derive(Debug, Default)]
pub struct StatusNotifierState {
    snapshot: StatusNotifierSnapshot,
}

impl StatusNotifierState {
    pub fn shared() -> SharedStatusNotifierState {
        Arc::new(RwLock::new(Self::default()))
    }

    pub fn apply_snapshot(&mut self, snapshot: StatusNotifierSnapshot) -> Result<Vec<GraphChange>> {
        let changes = snapshot_changes(&self.snapshot, &snapshot)?;
        self.snapshot = snapshot;
        Ok(changes)
    }

    pub fn upsert_item(&mut self, item: StatusNotifierItem) -> Result<Vec<GraphChange>> {
        let mut snapshot = self.snapshot.clone();
        snapshot.items.insert(item.id.clone(), item);
        self.apply_snapshot(snapshot)
    }

    pub fn remove_item(&mut self, id: &str) -> Result<Vec<GraphChange>> {
        let mut snapshot = self.snapshot.clone();
        snapshot.items.remove(id);
        self.apply_snapshot(snapshot)
    }

    pub fn registered_items(&self) -> Vec<String> {
        self.snapshot
            .items
            .values()
            .map(|item| item.service_name.clone())
            .collect()
    }

    pub fn contains_node(&self, node: &NodeId) -> Result<bool> {
        Ok(match node.kind().as_str() {
            STATUS_NOTIFIER_KIND => node.local() == ITEM_NODE,
            STATUS_NOTIFIER_ITEM_KIND => self.snapshot.items.contains_key(node.local()),
            _ => false,
        })
    }

    pub fn nodes(&self, kind: &NodeKind) -> Result<Vec<NodeId>> {
        let mut nodes = match kind.as_str() {
            STATUS_NOTIFIER_KIND => vec![statusnotifier_node(ITEM_NODE)?],
            STATUS_NOTIFIER_ITEM_KIND => self
                .snapshot
                .items
                .keys()
                .map(item_node)
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

    pub fn path_lookup_child(
        &self,
        parent: &GraphPathDirectory,
        name: &PathName,
    ) -> Result<Option<GraphPathEntry>> {
        match parent {
            GraphPathDirectory::Node(node)
                if node.kind().as_str() == STATUS_NOTIFIER_KIND && node.local() == ITEM_NODE =>
            {
                if self.snapshot.items.contains_key(name.as_str()) {
                    Ok(Some(GraphPathEntry::Directory(GraphPathDirectory::Node(
                        item_node(name.as_str())?,
                    ))))
                } else {
                    Ok(None)
                }
            }
            _ => Ok(None),
        }
    }

    pub fn path_children(
        &self,
        parent: &GraphPathDirectory,
    ) -> Result<Option<Vec<GraphPathChild>>> {
        match parent {
            GraphPathDirectory::Node(node)
                if node.kind().as_str() == STATUS_NOTIFIER_KIND && node.local() == ITEM_NODE =>
            {
                Ok(Some(
                    self.snapshot
                        .items
                        .keys()
                        .map(|id| {
                            Ok(GraphPathChild {
                                name: PathName::new(id)?,
                                entry: GraphPathEntry::Directory(GraphPathDirectory::Node(
                                    item_node(id)?,
                                )),
                            })
                        })
                        .collect::<Result<Vec<_>>>()?,
                ))
            }
            _ => Ok(None),
        }
    }

    pub fn path_watch_target(
        &self,
        directory: &GraphPathDirectory,
    ) -> Result<Option<GraphWatchTarget>> {
        match directory {
            GraphPathDirectory::Node(node)
                if node.kind().as_str() == STATUS_NOTIFIER_KIND && node.local() == ITEM_NODE =>
            {
                Ok(Some(GraphWatchTarget::Node(node.clone())))
            }
            _ => Ok(None),
        }
    }

    fn node_properties(&self, node: &NodeId) -> Result<BTreeMap<PropertyKey, LocusValue>> {
        match node.kind().as_str() {
            STATUS_NOTIFIER_KIND if node.local() == ITEM_NODE => Ok(BTreeMap::new()),
            STATUS_NOTIFIER_ITEM_KIND => self
                .snapshot
                .items
                .get(node.local())
                .map(item_properties)
                .transpose()?
                .ok_or_else(|| node_not_found(node)),
            _ => Err(node_not_found(node)),
        }
    }

    fn node_relations(&self, node: &NodeId) -> Result<BTreeMap<RelationName, Vec<NodeId>>> {
        let mut relations = BTreeMap::new();
        match node.kind().as_str() {
            STATUS_NOTIFIER_KIND if node.local() == ITEM_NODE => {
                for id in self.snapshot.items.keys() {
                    relations.insert(relation(id)?, vec![item_node(id)?]);
                }
            }
            STATUS_NOTIFIER_ITEM_KIND if self.snapshot.items.contains_key(node.local()) => {}
            _ => return Err(node_not_found(node)),
        }
        Ok(relations)
    }
}

fn snapshot_changes(
    old: &StatusNotifierSnapshot,
    new: &StatusNotifierSnapshot,
) -> Result<Vec<GraphChange>> {
    let mut changes = Vec::new();
    let old_ids = old.items.keys().cloned().collect::<BTreeSet<_>>();
    let new_ids = new.items.keys().cloned().collect::<BTreeSet<_>>();

    if old_ids != new_ids {
        changes.push(GraphChange::NodeKindChanged {
            kind: NodeKind::new(STATUS_NOTIFIER_ITEM_KIND)?,
        });
    }

    let facade = statusnotifier_node(ITEM_NODE)?;
    for id in old_ids.difference(&new_ids) {
        changes.push(GraphChange::NodeRemoved {
            node: item_node(id)?,
        });
        changes.push(GraphChange::RelationRemoved {
            source: facade.clone(),
            relation: relation(id)?,
        });
    }

    for id in new_ids.difference(&old_ids) {
        changes.push(GraphChange::NodeAdded {
            node: item_node(id)?,
        });
        changes.push(GraphChange::RelationAdded {
            source: facade.clone(),
            relation: relation(id)?,
        });
    }

    for id in old_ids.intersection(&new_ids) {
        let Some(old_item) = old.items.get(id) else {
            continue;
        };
        let Some(new_item) = new.items.get(id) else {
            continue;
        };
        if old_item != new_item {
            let node = item_node(id)?;
            changes.push(GraphChange::NodeChanged { node: node.clone() });
            for key in changed_property_keys(old_item, new_item)? {
                changes.push(GraphChange::PropertyChanged {
                    node: node.clone(),
                    key,
                });
            }
        }
    }

    Ok(changes)
}

fn changed_property_keys(
    old: &StatusNotifierItem,
    new: &StatusNotifierItem,
) -> Result<Vec<PropertyKey>> {
    let old = item_properties(old)?;
    let new = item_properties(new)?;
    Ok(old
        .keys()
        .chain(new.keys())
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|key| old.get(key) != new.get(key))
        .collect())
}

fn item_properties(item: &StatusNotifierItem) -> Result<BTreeMap<PropertyKey, LocusValue>> {
    let mut properties = BTreeMap::new();
    insert(&mut properties, "id", string(&item.id))?;
    insert(&mut properties, "source", string(SOURCE))?;
    insert(&mut properties, "service-name", string(&item.service_name))?;
    insert(&mut properties, "path", string(&item.path))?;
    insert(&mut properties, "category", string(&item.category))?;
    insert(&mut properties, "title", string(&item.title))?;
    insert(&mut properties, "status", string(&item.status))?;
    insert(&mut properties, "icon-name", string(&item.icon_name))?;
    insert(
        &mut properties,
        "attention-icon-name",
        string(&item.attention_icon_name),
    )?;
    insert(
        &mut properties,
        "overlay-icon-name",
        string(&item.overlay_icon_name),
    )?;
    insert(&mut properties, "menu-path", string(&item.menu_path))?;
    insert(
        &mut properties,
        "item-is-menu",
        LocusValue::Bool(item.item_is_menu),
    )?;
    Ok(properties)
}

pub fn item_id(service_name: &str, path: &str) -> String {
    format!("{service_name}:{path}")
        .chars()
        .map(|char| match char {
            '\0' | '/' | ':' => '_',
            char => char,
        })
        .collect()
}

fn statusnotifier_node(local: &str) -> Result<NodeId> {
    node_id(STATUS_NOTIFIER_KIND, local)
}

fn item_node(local: impl AsRef<str>) -> Result<NodeId> {
    node_id(STATUS_NOTIFIER_ITEM_KIND, local)
}

fn node_id(kind: &str, local: impl AsRef<str>) -> Result<NodeId> {
    NodeId::new(NodeKind::new(kind)?, local.as_ref())
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

fn insert(
    properties: &mut BTreeMap<PropertyKey, LocusValue>,
    key: &str,
    value: LocusValue,
) -> Result<()> {
    properties.insert(PropertyKey::new(key)?, value);
    Ok(())
}
