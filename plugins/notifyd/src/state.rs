#[cfg(test)]
mod test;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use locusfs_graph::{
    GraphChange, GraphError, GraphPathChild, GraphPathDirectory, GraphPathEntry, GraphWatchTarget,
    LocusValue, NodeId, NodeKind, PathName, PropertyKey, PropertySpec, RelationName, Result,
};
use tokio::sync::RwLock;

use crate::{NOTIFICATION_ACTION_KIND, NOTIFICATION_KIND, NOTIFYD_KIND};

pub type SharedNotifydState = Arc<RwLock<NotifydState>>;

pub const NOTIFICATIONS_NODE: &str = "notifications";
pub const STATE_NODE: &str = "state";
pub const COMMANDS_NODE: &str = "commands";

const ACTIONS_DIR: &str = "actions";
const DISCARD_PROPERTY: &str = "discard";
const INVOKE_PROPERTY: &str = "invoke";
const DISCARD_ALL_PROPERTY: &str = "discard-all";
const DND_ENABLED_PROPERTY: &str = "dnd-enabled";
const SOURCE: &str = "notifyd";
const VIRTUAL_ACTIONS: &str = "actions";
const VIRTUAL_SEPARATOR: &str = "|";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum NotificationUrgency {
    Low,
    #[default]
    Normal,
    Critical,
}

impl NotificationUrgency {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Normal => "normal",
            Self::Critical => "critical",
        }
    }

    pub fn level(self) -> u32 {
        match self {
            Self::Low => 0,
            Self::Normal => 1,
            Self::Critical => 2,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NotificationAction {
    pub local_id: String,
    pub path_name: String,
    pub key: String,
    pub label: String,
    pub is_default: bool,
    pub icon_name: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NotificationRecord {
    pub local_id: String,
    pub dbus_id: u32,
    pub created_at_unix_ms: u64,
    pub updated_at_unix_ms: u64,
    pub expire_timeout_ms: i32,
    pub app_name: String,
    pub desktop_entry: String,
    pub app_icon: String,
    pub summary: String,
    pub body: String,
    pub body_markup: Option<String>,
    pub category: String,
    pub urgency: NotificationUrgency,
    pub progress: Option<u32>,
    pub resident: bool,
    pub transient: bool,
    pub suppress_sound: bool,
    pub icon_name: String,
    pub image_path: Option<String>,
    pub image_source: String,
    pub image_width: Option<u32>,
    pub image_height: Option<u32>,
    pub stack_key: Option<String>,
    pub actions: Vec<NotificationAction>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum NotifydCommandTarget {
    Discard {
        notification_id: String,
    },
    InvokeAction {
        notification_id: String,
        action_key: String,
    },
    DiscardAll,
    SetDnd(bool),
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NotifydSnapshot {
    pub notifications: BTreeMap<String, NotificationRecord>,
    pub dnd_enabled: bool,
    pub suppressed_count: u32,
    pub server_name: String,
}

#[derive(Debug)]
pub struct NotifydState {
    snapshot: NotifydSnapshot,
}

impl NotifydState {
    pub fn shared(server_name: String, dnd_enabled: bool) -> SharedNotifydState {
        Arc::new(RwLock::new(Self::new(server_name, dnd_enabled)))
    }

    pub fn new(server_name: String, dnd_enabled: bool) -> Self {
        Self {
            snapshot: NotifydSnapshot {
                server_name,
                dnd_enabled,
                ..NotifydSnapshot::default()
            },
        }
    }

    pub fn upsert_notification(
        &mut self,
        record: NotificationRecord,
        max_notifications: usize,
    ) -> Result<Vec<GraphChange>> {
        let mut snapshot = self.snapshot.clone();
        snapshot
            .notifications
            .insert(record.local_id.clone(), record);
        trim_notifications(&mut snapshot, max_notifications);
        self.apply_snapshot(snapshot)
    }

    pub fn discard_notification(&mut self, local_id: &str) -> Result<Vec<GraphChange>> {
        let mut snapshot = self.snapshot.clone();
        snapshot.notifications.remove(local_id);
        self.apply_snapshot(snapshot)
    }

    pub fn set_dnd_enabled(&mut self, enabled: bool) -> Result<Vec<GraphChange>> {
        let mut snapshot = self.snapshot.clone();
        snapshot.dnd_enabled = enabled;
        self.apply_snapshot(snapshot)
    }

    pub fn increment_suppressed(&mut self) -> Result<Vec<GraphChange>> {
        let mut snapshot = self.snapshot.clone();
        snapshot.suppressed_count = snapshot.suppressed_count.saturating_add(1);
        self.apply_snapshot(snapshot)
    }

    pub(crate) fn dnd_enabled(&self) -> bool {
        self.snapshot.dnd_enabled
    }

    pub(crate) fn notification(&self, local_id: &str) -> Option<NotificationRecord> {
        self.snapshot.notifications.get(local_id).cloned()
    }

    pub(crate) fn notification_ids(&self) -> Vec<String> {
        self.snapshot.notifications.keys().cloned().collect()
    }

    pub(crate) fn contains_dbus_id(&self, dbus_id: u32) -> bool {
        self.snapshot
            .notifications
            .values()
            .any(|record| record.dbus_id == dbus_id)
    }

    pub(crate) fn action_for_key(
        &self,
        notification_id: &str,
        action_key: &str,
    ) -> Option<(NotificationRecord, NotificationAction)> {
        let notification = self.snapshot.notifications.get(notification_id)?;
        let action = notification
            .actions
            .iter()
            .find(|action| action.key == action_key)?;
        Some((notification.clone(), action.clone()))
    }

    fn apply_snapshot(&mut self, snapshot: NotifydSnapshot) -> Result<Vec<GraphChange>> {
        let changes = snapshot_changes(&self.snapshot, &snapshot)?;
        self.snapshot = snapshot;
        Ok(changes)
    }

    pub fn contains_node(&self, node: &NodeId) -> Result<bool> {
        Ok(match node.kind().as_str() {
            NOTIFYD_KIND => matches!(
                node.local(),
                NOTIFICATIONS_NODE | STATE_NODE | COMMANDS_NODE
            ),
            NOTIFICATION_KIND => self.notification_record(node.local()).is_some(),
            NOTIFICATION_ACTION_KIND => self.action_by_node(node).is_some(),
            _ => false,
        })
    }

    pub fn nodes(&self, kind: &NodeKind) -> Result<Vec<NodeId>> {
        let mut nodes = match kind.as_str() {
            NOTIFYD_KIND => [NOTIFICATIONS_NODE, STATE_NODE, COMMANDS_NODE]
                .into_iter()
                .map(notifyd_node)
                .collect::<Result<Vec<_>>>()?,
            NOTIFICATION_KIND => all_notification_ids(&self.snapshot)
                .into_iter()
                .map(notification_node)
                .collect::<Result<Vec<_>>>()?,
            NOTIFICATION_ACTION_KIND => self
                .snapshot
                .notifications
                .values()
                .flat_map(|record| record.actions.iter().map(|action| action.local_id.as_str()))
                .map(action_node)
                .collect::<Result<Vec<_>>>()?,
            _ => Vec::new(),
        };
        nodes.sort();
        Ok(nodes)
    }

    pub fn property_spec(&self, subject: &NodeId, key: &PropertyKey) -> Result<PropertySpec> {
        let value = self
            .node_properties(subject)?
            .remove(key)
            .ok_or_else(|| property_not_found(subject, key))?;
        if is_write_only_property(subject, key) {
            Ok(PropertySpec::write_only(key.clone(), value.kind()))
        } else if is_read_write_property(subject, key) {
            Ok(PropertySpec::read_write(key.clone(), value.kind()))
        } else {
            Ok(PropertySpec::new(key.clone(), value.kind()))
        }
    }

    pub fn properties(&self, subject: &NodeId) -> Result<Vec<PropertySpec>> {
        self.node_properties(subject)?
            .into_iter()
            .map(|(key, value)| {
                if is_write_only_property(subject, &key) {
                    PropertySpec::write_only(key, value.kind())
                } else if is_read_write_property(subject, &key) {
                    PropertySpec::read_write(key, value.kind())
                } else {
                    PropertySpec::new(key, value.kind())
                }
            })
            .collect::<Vec<_>>()
            .pipe(Ok)
    }

    pub fn property(&self, subject: &NodeId, key: &PropertyKey) -> Result<LocusValue> {
        self.node_properties(subject)?
            .remove(key)
            .ok_or_else(|| property_not_found(subject, key))
    }

    pub fn relations(&self, source: &NodeId) -> Result<Vec<RelationName>> {
        self.node_relations(source)
            .map(|relations| relations.into_keys().collect())
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
                if node.kind().as_str() == NOTIFYD_KIND && node.local() == NOTIFICATIONS_NODE =>
            {
                if self.snapshot.notifications.contains_key(name.as_str()) {
                    Ok(Some(GraphPathEntry::Directory(GraphPathDirectory::Node(
                        notification_node(name.as_str())?,
                    ))))
                } else {
                    Ok(None)
                }
            }
            GraphPathDirectory::Node(node) if node.kind().as_str() == NOTIFICATION_KIND => {
                self.notification_path_lookup(node, name)
            }
            GraphPathDirectory::Node(node) if node.kind().as_str() == NOTIFICATION_ACTION_KIND => {
                self.property_path_lookup(node, name)
            }
            GraphPathDirectory::Virtual { owner, local } if owner.as_str() == NOTIFYD_KIND => {
                let Some(notification_id) = actions_virtual_notification_id(local) else {
                    return Ok(None);
                };
                let Some(record) = self.snapshot.notifications.get(notification_id) else {
                    return Ok(None);
                };
                record
                    .actions
                    .iter()
                    .find(|action| action.path_name == name.as_str())
                    .map(|action| {
                        Ok(GraphPathEntry::Directory(GraphPathDirectory::Node(
                            action_node(&action.local_id)?,
                        )))
                    })
                    .transpose()
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
                if node.kind().as_str() == NOTIFYD_KIND && node.local() == NOTIFICATIONS_NODE =>
            {
                Ok(Some(
                    self.snapshot
                        .notifications
                        .keys()
                        .map(|id| {
                            Ok(GraphPathChild {
                                name: PathName::new(id)?,
                                entry: GraphPathEntry::Directory(GraphPathDirectory::Node(
                                    notification_node(id)?,
                                )),
                            })
                        })
                        .collect::<Result<Vec<_>>>()?,
                ))
            }
            GraphPathDirectory::Node(node) if node.kind().as_str() == NOTIFICATION_KIND => {
                if self.notification_record(node.local()).is_none() {
                    return Ok(None);
                }
                Ok(Some(self.notification_path_children(node)?))
            }
            GraphPathDirectory::Node(node) if node.kind().as_str() == NOTIFICATION_ACTION_KIND => {
                if self.action_by_node(node).is_none() {
                    return Ok(None);
                }
                Ok(Some(self.property_path_children(node)?))
            }
            GraphPathDirectory::Virtual { owner, local } if owner.as_str() == NOTIFYD_KIND => {
                let Some(notification_id) = actions_virtual_notification_id(local) else {
                    return Ok(None);
                };
                let Some(record) = self.snapshot.notifications.get(notification_id) else {
                    return Ok(None);
                };
                Ok(Some(
                    record
                        .actions
                        .iter()
                        .map(|action| {
                            Ok(GraphPathChild {
                                name: PathName::new(&action.path_name)?,
                                entry: GraphPathEntry::Directory(GraphPathDirectory::Node(
                                    action_node(&action.local_id)?,
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
                if matches!(
                    node.kind().as_str(),
                    NOTIFYD_KIND | NOTIFICATION_KIND | NOTIFICATION_ACTION_KIND
                ) =>
            {
                Ok(Some(GraphWatchTarget::Node(node.clone())))
            }
            GraphPathDirectory::Virtual { owner, local } if owner.as_str() == NOTIFYD_KIND => {
                let Some(notification_id) = actions_virtual_notification_id(local) else {
                    return Ok(None);
                };
                Ok(Some(GraphWatchTarget::Node(notification_node(
                    notification_id,
                )?)))
            }
            _ => Ok(None),
        }
    }

    pub(crate) fn command_target(
        &self,
        subject: &NodeId,
        key: &PropertyKey,
        value: &LocusValue,
    ) -> Result<NotifydCommandTarget> {
        match subject.kind().as_str() {
            NOTIFICATION_KIND if key.as_str() == DISCARD_PROPERTY => {
                if !matches!(value, LocusValue::String(_) | LocusValue::Bool(true)) {
                    return invalid_command_value(subject, key, value, "write a string or true");
                }
                if self.notification_record(subject.local()).is_none() {
                    return Err(node_not_found(subject));
                }
                Ok(NotifydCommandTarget::Discard {
                    notification_id: subject.local().to_owned(),
                })
            }
            NOTIFICATION_ACTION_KIND if key.as_str() == INVOKE_PROPERTY => {
                if !matches!(value, LocusValue::String(_)) {
                    return invalid_command_value(subject, key, value, "expected string payload");
                }
                let Some((notification, action)) = self.action_by_node(subject) else {
                    return Err(node_not_found(subject));
                };
                Ok(NotifydCommandTarget::InvokeAction {
                    notification_id: notification.local_id.clone(),
                    action_key: action.key.clone(),
                })
            }
            NOTIFYD_KIND
                if subject.local() == COMMANDS_NODE && key.as_str() == DISCARD_ALL_PROPERTY =>
            {
                if !matches!(value, LocusValue::String(_) | LocusValue::Bool(true)) {
                    return invalid_command_value(subject, key, value, "write a string or true");
                }
                Ok(NotifydCommandTarget::DiscardAll)
            }
            NOTIFYD_KIND
                if subject.local() == COMMANDS_NODE && key.as_str() == DND_ENABLED_PROPERTY =>
            {
                let LocusValue::Bool(enabled) = value else {
                    return invalid_command_value(subject, key, value, "expected bool");
                };
                Ok(NotifydCommandTarget::SetDnd(*enabled))
            }
            _ => Err(GraphError::InvalidValue {
                kind: "notifyd command property",
                value: format!("{subject}/{key}"),
                reason: "property is not writable",
            }),
        }
    }

    fn notification_path_lookup(
        &self,
        node: &NodeId,
        name: &PathName,
    ) -> Result<Option<GraphPathEntry>> {
        if self.notification_record(node.local()).is_none() {
            return Ok(None);
        }
        if name.as_str() == ACTIONS_DIR && self.snapshot.notifications.contains_key(node.local()) {
            return Ok(Some(GraphPathEntry::Directory(
                GraphPathDirectory::Virtual {
                    owner: NodeKind::new(NOTIFYD_KIND)?,
                    local: actions_virtual_local(node.local()),
                },
            )));
        }
        self.property_path_lookup(node, name)
    }

    fn property_path_lookup(
        &self,
        node: &NodeId,
        name: &PathName,
    ) -> Result<Option<GraphPathEntry>> {
        let key = PropertyKey::new(name.as_str())?;
        if self.property_spec(node, &key).is_ok() {
            Ok(Some(GraphPathEntry::Property {
                node: node.clone(),
                key,
            }))
        } else {
            Ok(None)
        }
    }

    fn notification_path_children(&self, node: &NodeId) -> Result<Vec<GraphPathChild>> {
        let mut children = self.property_path_children(node)?;
        if self.snapshot.notifications.contains_key(node.local()) {
            children.push(GraphPathChild {
                name: PathName::new(ACTIONS_DIR)?,
                entry: GraphPathEntry::Directory(GraphPathDirectory::Virtual {
                    owner: NodeKind::new(NOTIFYD_KIND)?,
                    local: actions_virtual_local(node.local()),
                }),
            });
        }
        Ok(children)
    }

    fn property_path_children(&self, node: &NodeId) -> Result<Vec<GraphPathChild>> {
        self.properties(node)?
            .into_iter()
            .map(|spec| {
                let key = spec.into_key();
                Ok(GraphPathChild {
                    name: PathName::new(key.as_str())?,
                    entry: GraphPathEntry::Property {
                        node: node.clone(),
                        key,
                    },
                })
            })
            .collect()
    }

    fn node_properties(&self, node: &NodeId) -> Result<BTreeMap<PropertyKey, LocusValue>> {
        match node.kind().as_str() {
            NOTIFYD_KIND if node.local() == NOTIFICATIONS_NODE => Ok(BTreeMap::new()),
            NOTIFYD_KIND if node.local() == STATE_NODE => self.state_properties(),
            NOTIFYD_KIND if node.local() == COMMANDS_NODE => self.command_properties(),
            NOTIFICATION_KIND => self.notification_properties_by_id(node.local()),
            NOTIFICATION_ACTION_KIND => self
                .action_by_node(node)
                .map(|(_, action)| action_properties(action))
                .transpose()?
                .ok_or_else(|| node_not_found(node)),
            _ => Err(node_not_found(node)),
        }
    }

    fn state_properties(&self) -> Result<BTreeMap<PropertyKey, LocusValue>> {
        let mut properties = BTreeMap::new();
        insert(
            &mut properties,
            DND_ENABLED_PROPERTY,
            LocusValue::Bool(self.snapshot.dnd_enabled),
        )?;
        insert(
            &mut properties,
            "notification-count",
            LocusValue::U32(
                self.snapshot
                    .notifications
                    .len()
                    .try_into()
                    .unwrap_or(u32::MAX),
            ),
        )?;
        insert(
            &mut properties,
            "suppressed-count",
            LocusValue::U32(self.snapshot.suppressed_count),
        )?;
        insert(
            &mut properties,
            "server-name",
            string(&self.snapshot.server_name),
        )?;
        Ok(properties)
    }

    fn command_properties(&self) -> Result<BTreeMap<PropertyKey, LocusValue>> {
        let mut properties = BTreeMap::new();
        insert(&mut properties, DISCARD_ALL_PROPERTY, string(String::new()))?;
        insert(
            &mut properties,
            DND_ENABLED_PROPERTY,
            LocusValue::Bool(self.snapshot.dnd_enabled),
        )?;
        Ok(properties)
    }

    fn node_relations(&self, node: &NodeId) -> Result<BTreeMap<RelationName, Vec<NodeId>>> {
        match node.kind().as_str() {
            NOTIFYD_KIND
                if matches!(
                    node.local(),
                    NOTIFICATIONS_NODE | STATE_NODE | COMMANDS_NODE
                ) =>
            {
                Ok(BTreeMap::new())
            }
            NOTIFICATION_KIND if self.notification_record(node.local()).is_some() => {
                Ok(BTreeMap::new())
            }
            NOTIFICATION_ACTION_KIND if self.action_by_node(node).is_some() => Ok(BTreeMap::new()),
            _ => Err(node_not_found(node)),
        }
    }

    fn notification_record(&self, local_id: &str) -> Option<&NotificationRecord> {
        self.snapshot.notifications.get(local_id)
    }

    fn notification_properties_by_id(
        &self,
        local_id: &str,
    ) -> Result<BTreeMap<PropertyKey, LocusValue>> {
        if let Some(record) = self.snapshot.notifications.get(local_id) {
            return notification_properties(record);
        }
        Err(GraphError::NotFound {
            kind: "node",
            name: format!("{NOTIFICATION_KIND}:{local_id}"),
        })
    }

    fn action_by_node(&self, node: &NodeId) -> Option<(&NotificationRecord, &NotificationAction)> {
        if node.kind().as_str() != NOTIFICATION_ACTION_KIND {
            return None;
        }
        self.snapshot
            .notifications
            .values()
            .find_map(|notification| {
                notification
                    .actions
                    .iter()
                    .find(|action| action.local_id == node.local())
                    .map(|action| (notification, action))
            })
    }
}

fn snapshot_changes(old: &NotifydSnapshot, new: &NotifydSnapshot) -> Result<Vec<GraphChange>> {
    let mut changes = Vec::new();
    let old_ids = all_notification_ids(old);
    let new_ids = all_notification_ids(new);
    let notifications = notifyd_node(NOTIFICATIONS_NODE)?;
    let state = notifyd_node(STATE_NODE)?;

    if old_ids != new_ids {
        changes.push(GraphChange::NodeChanged {
            node: notifications,
        });
    }
    if old_ids != new_ids {
        changes.push(GraphChange::NodeKindChanged {
            kind: NodeKind::new(NOTIFICATION_KIND)?,
        });
    }

    for id in old_ids.difference(&new_ids) {
        changes.push(GraphChange::NodeRemoved {
            node: notification_node(id)?,
        });
    }
    for id in new_ids.difference(&old_ids) {
        changes.push(GraphChange::NodeAdded {
            node: notification_node(id)?,
        });
    }

    for id in old_ids.intersection(&new_ids) {
        let old_notification = snapshot_notification(old, id).expect("old id exists");
        let new_notification = snapshot_notification(new, id).expect("new id exists");
        if old_notification != new_notification {
            let node = notification_node(id)?;
            changes.push(GraphChange::NodeChanged { node: node.clone() });
            for key in changed_property_keys(old_notification, new_notification)? {
                changes.push(GraphChange::PropertyChanged {
                    node: node.clone(),
                    key,
                });
            }
        }

        let old_actions = action_map(old_notification);
        let new_actions = action_map(new_notification);
        if old_actions.keys().collect::<BTreeSet<_>>()
            != new_actions.keys().collect::<BTreeSet<_>>()
        {
            changes.push(GraphChange::NodeKindChanged {
                kind: NodeKind::new(NOTIFICATION_ACTION_KIND)?,
            });
        }
        for action_id in old_actions
            .keys()
            .filter(|id| !new_actions.contains_key(*id))
        {
            changes.push(GraphChange::NodeRemoved {
                node: action_node(action_id)?,
            });
        }
        for action_id in new_actions
            .keys()
            .filter(|id| !old_actions.contains_key(*id))
        {
            changes.push(GraphChange::NodeAdded {
                node: action_node(action_id)?,
            });
        }
        for action_id in old_actions
            .keys()
            .filter(|id| new_actions.contains_key(*id))
        {
            let old_action = old_actions.get(action_id).expect("old action exists");
            let new_action = new_actions.get(action_id).expect("new action exists");
            if old_action != new_action {
                changes.push(GraphChange::NodeChanged {
                    node: action_node(action_id)?,
                });
            }
        }
    }

    if old.dnd_enabled != new.dnd_enabled {
        changes.push(GraphChange::PropertyChanged {
            node: state.clone(),
            key: PropertyKey::new(DND_ENABLED_PROPERTY)?,
        });
        changes.push(GraphChange::PropertyChanged {
            node: notifyd_node(COMMANDS_NODE)?,
            key: PropertyKey::new(DND_ENABLED_PROPERTY)?,
        });
    }
    if old.notifications.len() != new.notifications.len() {
        changes.push(GraphChange::PropertyChanged {
            node: state.clone(),
            key: PropertyKey::new("notification-count")?,
        });
    }
    if old.suppressed_count != new.suppressed_count {
        changes.push(GraphChange::PropertyChanged {
            node: state.clone(),
            key: PropertyKey::new("suppressed-count")?,
        });
    }
    if old.server_name != new.server_name {
        changes.push(GraphChange::PropertyChanged {
            node: state,
            key: PropertyKey::new("server-name")?,
        });
    }

    Ok(changes)
}

fn changed_property_keys(
    old: &NotificationRecord,
    new: &NotificationRecord,
) -> Result<Vec<PropertyKey>> {
    let old = notification_properties(old)?;
    let new = notification_properties(new)?;
    Ok(old
        .keys()
        .chain(new.keys())
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|key| old.get(key) != new.get(key))
        .collect())
}

fn all_notification_ids(snapshot: &NotifydSnapshot) -> BTreeSet<String> {
    snapshot.notifications.keys().cloned().collect()
}

fn snapshot_notification<'a>(
    snapshot: &'a NotifydSnapshot,
    id: &str,
) -> Option<&'a NotificationRecord> {
    snapshot.notifications.get(id)
}

fn action_map(notification: &NotificationRecord) -> BTreeMap<String, &NotificationAction> {
    notification
        .actions
        .iter()
        .map(|action| (action.local_id.clone(), action))
        .collect()
}

fn notification_properties(
    record: &NotificationRecord,
) -> Result<BTreeMap<PropertyKey, LocusValue>> {
    let mut properties = BTreeMap::new();
    insert(&mut properties, "id", string(&record.local_id))?;
    insert(&mut properties, "dbus-id", LocusValue::U32(record.dbus_id))?;
    insert(&mut properties, "source", string(SOURCE))?;
    insert(
        &mut properties,
        "created-at-unix-ms",
        LocusValue::U32(record.created_at_unix_ms.try_into().unwrap_or(u32::MAX)),
    )?;
    insert(
        &mut properties,
        "updated-at-unix-ms",
        LocusValue::U32(record.updated_at_unix_ms.try_into().unwrap_or(u32::MAX)),
    )?;
    insert(
        &mut properties,
        "expire-timeout-ms",
        LocusValue::I32(record.expire_timeout_ms),
    )?;
    insert(&mut properties, "app-name", string(&record.app_name))?;
    insert(
        &mut properties,
        "desktop-entry",
        string(&record.desktop_entry),
    )?;
    insert(&mut properties, "app-icon", string(&record.app_icon))?;
    insert(&mut properties, "summary", string(&record.summary))?;
    insert(&mut properties, "body", string(&record.body))?;
    if let Some(body_markup) = &record.body_markup {
        insert(&mut properties, "body-markup", string(body_markup))?;
    }
    insert(&mut properties, "category", string(&record.category))?;
    insert(&mut properties, "urgency", string(record.urgency.as_str()))?;
    insert(
        &mut properties,
        "urgency-level",
        LocusValue::U32(record.urgency.level()),
    )?;
    if let Some(progress) = record.progress {
        insert(
            &mut properties,
            "progress",
            LocusValue::U32(progress.min(100)),
        )?;
    }
    insert(
        &mut properties,
        "resident",
        LocusValue::Bool(record.resident),
    )?;
    insert(
        &mut properties,
        "transient",
        LocusValue::Bool(record.transient),
    )?;
    insert(
        &mut properties,
        "suppress-sound",
        LocusValue::Bool(record.suppress_sound),
    )?;
    insert(&mut properties, "icon-name", string(&record.icon_name))?;
    if let Some(image_path) = &record.image_path {
        insert(&mut properties, "image-path", string(image_path))?;
    }
    insert(
        &mut properties,
        "image-source",
        string(&record.image_source),
    )?;
    if let Some(width) = record.image_width {
        insert(&mut properties, "image-width", LocusValue::U32(width))?;
    }
    if let Some(height) = record.image_height {
        insert(&mut properties, "image-height", LocusValue::U32(height))?;
    }
    if let Some(stack_key) = &record.stack_key {
        insert(&mut properties, "stack-key", string(stack_key))?;
    }
    insert(&mut properties, DISCARD_PROPERTY, string(String::new()))?;
    Ok(properties)
}

fn action_properties(action: &NotificationAction) -> Result<BTreeMap<PropertyKey, LocusValue>> {
    let mut properties = BTreeMap::new();
    insert(&mut properties, "key", string(&action.key))?;
    insert(&mut properties, "label", string(&action.label))?;
    insert(
        &mut properties,
        "default",
        LocusValue::Bool(action.is_default),
    )?;
    if let Some(icon_name) = &action.icon_name {
        insert(&mut properties, "icon-name", string(icon_name))?;
    }
    insert(&mut properties, INVOKE_PROPERTY, string(String::new()))?;
    Ok(properties)
}

fn trim_notifications(snapshot: &mut NotifydSnapshot, max_notifications: usize) {
    while snapshot.notifications.len() > max_notifications {
        let Some(oldest) = snapshot
            .notifications
            .values()
            .min_by_key(|record| {
                (
                    record.updated_at_unix_ms.max(record.created_at_unix_ms),
                    record.dbus_id,
                )
            })
            .map(|record| record.local_id.clone())
        else {
            break;
        };
        snapshot.notifications.remove(&oldest);
    }
}

pub fn make_action(notification_id: &str, key: String, label: String) -> NotificationAction {
    let is_default = key == "default";
    let path_name = safe_segment(&key);
    NotificationAction {
        local_id: format!("{notification_id}-{path_name}"),
        path_name,
        key,
        label,
        is_default,
        icon_name: None,
    }
}

pub fn notifyd_node(local: &str) -> Result<NodeId> {
    node_id(NOTIFYD_KIND, local)
}

pub fn notification_node(local: impl AsRef<str>) -> Result<NodeId> {
    node_id(NOTIFICATION_KIND, local)
}

pub fn action_node(local: impl AsRef<str>) -> Result<NodeId> {
    node_id(NOTIFICATION_ACTION_KIND, local)
}

pub fn safe_segment(value: &str) -> String {
    let mut safe = String::new();
    for char in value.chars() {
        if char.is_ascii_alphanumeric() || matches!(char, '.' | '_' | '-') {
            safe.push(char);
        } else {
            safe.push('_');
        }
    }
    if safe.is_empty() || safe == "." || safe == ".." {
        "default".to_owned()
    } else {
        safe
    }
}

fn node_id(kind: &str, local: impl AsRef<str>) -> Result<NodeId> {
    NodeId::new(NodeKind::new(kind)?, local.as_ref())
}

fn actions_virtual_local(notification_id: &str) -> String {
    format!("{VIRTUAL_ACTIONS}{VIRTUAL_SEPARATOR}{notification_id}")
}

fn actions_virtual_notification_id(local: &str) -> Option<&str> {
    let (kind, notification_id) = local.split_once(VIRTUAL_SEPARATOR)?;
    (kind == VIRTUAL_ACTIONS).then_some(notification_id)
}

fn is_write_only_property(subject: &NodeId, key: &PropertyKey) -> bool {
    matches!(
        (subject.kind().as_str(), key.as_str()),
        (NOTIFICATION_KIND, DISCARD_PROPERTY)
            | (NOTIFICATION_ACTION_KIND, INVOKE_PROPERTY)
            | (NOTIFYD_KIND, DISCARD_ALL_PROPERTY)
    )
}

fn is_read_write_property(subject: &NodeId, key: &PropertyKey) -> bool {
    subject.kind().as_str() == NOTIFYD_KIND
        && subject.local() == COMMANDS_NODE
        && key.as_str() == DND_ENABLED_PROPERTY
}

fn invalid_command_value<T>(
    subject: &NodeId,
    key: &PropertyKey,
    value: &LocusValue,
    reason: &'static str,
) -> Result<T> {
    Err(GraphError::InvalidValue {
        kind: "notifyd command value",
        value: format!("{subject}/{key}={value}"),
        reason,
    })
}

fn property_not_found(subject: &NodeId, key: &PropertyKey) -> GraphError {
    GraphError::NotFound {
        kind: "property",
        name: format!("{subject}/{key}"),
    }
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

trait Pipe: Sized {
    fn pipe<T>(self, f: impl FnOnce(Self) -> T) -> T {
        f(self)
    }
}

impl<T> Pipe for T {}
