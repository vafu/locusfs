use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use locusfs_graph::{
    GraphChange, GraphError, GraphPathChild, GraphPathDirectory, GraphPathEntry, GraphWatchTarget,
    LocusValue, NodeId, NodeKind, PathName, PropertyKey, PropertySpec, RelationName, Result,
};
use tokio::sync::RwLock;

use crate::{MPRIS_KIND, MPRIS_PLAYER_KIND};

pub type SharedMprisState = Arc<RwLock<MprisState>>;

pub const PLAYER_NODE: &str = "player";

const SOURCE: &str = "mpris";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MprisSnapshot {
    pub players: BTreeMap<String, MprisPlayer>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MprisPlayer {
    pub id: String,
    pub service_name: String,
    pub playerctl_name: String,
    pub identity: String,
    pub artist: String,
    pub title: String,
    pub album: String,
    pub art_url: String,
    pub playback_status: String,
    pub can_play: bool,
    pub can_pause: bool,
    pub can_go_next: bool,
    pub can_go_previous: bool,
    pub length_us: Option<i64>,
}

#[derive(Debug, Default)]
pub struct MprisState {
    snapshot: MprisSnapshot,
}

impl MprisState {
    pub fn shared() -> SharedMprisState {
        Arc::new(RwLock::new(Self::default()))
    }

    pub fn apply_snapshot(&mut self, snapshot: MprisSnapshot) -> Result<Vec<GraphChange>> {
        let changes = snapshot_changes(&self.snapshot, &snapshot)?;
        self.snapshot = snapshot;
        Ok(changes)
    }

    pub fn upsert_player(&mut self, player: MprisPlayer) -> Result<Vec<GraphChange>> {
        let mut snapshot = self.snapshot.clone();
        snapshot.players.insert(player.id.clone(), player);
        self.apply_snapshot(snapshot)
    }

    pub fn remove_player(&mut self, id: &str) -> Result<Vec<GraphChange>> {
        let mut snapshot = self.snapshot.clone();
        snapshot.players.remove(id);
        self.apply_snapshot(snapshot)
    }

    pub fn contains_node(&self, node: &NodeId) -> Result<bool> {
        Ok(match node.kind().as_str() {
            MPRIS_KIND => node.local() == PLAYER_NODE,
            MPRIS_PLAYER_KIND => self.snapshot.players.contains_key(node.local()),
            _ => false,
        })
    }

    pub fn nodes(&self, kind: &NodeKind) -> Result<Vec<NodeId>> {
        let mut nodes = match kind.as_str() {
            MPRIS_KIND => vec![mpris_node(PLAYER_NODE)?],
            MPRIS_PLAYER_KIND => self
                .snapshot
                .players
                .keys()
                .map(player_node)
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
                if node.kind().as_str() == MPRIS_KIND && node.local() == PLAYER_NODE =>
            {
                if self.snapshot.players.contains_key(name.as_str()) {
                    Ok(Some(GraphPathEntry::Directory(GraphPathDirectory::Node(
                        player_node(name.as_str())?,
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
                if node.kind().as_str() == MPRIS_KIND && node.local() == PLAYER_NODE =>
            {
                Ok(Some(
                    self.snapshot
                        .players
                        .keys()
                        .map(|id| {
                            Ok(GraphPathChild {
                                name: PathName::new(id)?,
                                entry: GraphPathEntry::Directory(GraphPathDirectory::Node(
                                    player_node(id)?,
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
                if node.kind().as_str() == MPRIS_KIND && node.local() == PLAYER_NODE =>
            {
                Ok(Some(GraphWatchTarget::Node(node.clone())))
            }
            _ => Ok(None),
        }
    }

    fn node_properties(&self, node: &NodeId) -> Result<BTreeMap<PropertyKey, LocusValue>> {
        match node.kind().as_str() {
            MPRIS_KIND if node.local() == PLAYER_NODE => Ok(BTreeMap::new()),
            MPRIS_PLAYER_KIND => self
                .snapshot
                .players
                .get(node.local())
                .map(player_properties)
                .transpose()?
                .ok_or_else(|| node_not_found(node)),
            _ => Err(node_not_found(node)),
        }
    }

    fn node_relations(&self, node: &NodeId) -> Result<BTreeMap<RelationName, Vec<NodeId>>> {
        let mut relations = BTreeMap::new();
        match node.kind().as_str() {
            MPRIS_KIND if node.local() == PLAYER_NODE => {
                for id in self.snapshot.players.keys() {
                    relations.insert(relation(id)?, vec![player_node(id)?]);
                }
            }
            MPRIS_PLAYER_KIND if self.snapshot.players.contains_key(node.local()) => {}
            _ => return Err(node_not_found(node)),
        }
        Ok(relations)
    }
}

fn snapshot_changes(old: &MprisSnapshot, new: &MprisSnapshot) -> Result<Vec<GraphChange>> {
    let mut changes = Vec::new();
    let old_ids = old.players.keys().cloned().collect::<BTreeSet<_>>();
    let new_ids = new.players.keys().cloned().collect::<BTreeSet<_>>();

    if old_ids != new_ids {
        changes.push(GraphChange::NodeKindChanged {
            kind: NodeKind::new(MPRIS_PLAYER_KIND)?,
        });
    }

    let facade = mpris_node(PLAYER_NODE)?;
    for id in old_ids.difference(&new_ids) {
        changes.push(GraphChange::NodeRemoved {
            node: player_node(id)?,
        });
        changes.push(GraphChange::RelationRemoved {
            source: facade.clone(),
            relation: relation(id)?,
        });
    }

    for id in new_ids.difference(&old_ids) {
        changes.push(GraphChange::NodeAdded {
            node: player_node(id)?,
        });
        changes.push(GraphChange::RelationAdded {
            source: facade.clone(),
            relation: relation(id)?,
        });
    }

    for id in old_ids.intersection(&new_ids) {
        let Some(old_player) = old.players.get(id) else {
            continue;
        };
        let Some(new_player) = new.players.get(id) else {
            continue;
        };
        if old_player != new_player {
            let node = player_node(id)?;
            changes.push(GraphChange::NodeChanged { node: node.clone() });
            for key in changed_property_keys(old_player, new_player)? {
                changes.push(GraphChange::PropertyChanged {
                    node: node.clone(),
                    key,
                });
            }
        }
    }

    Ok(changes)
}

fn changed_property_keys(old: &MprisPlayer, new: &MprisPlayer) -> Result<Vec<PropertyKey>> {
    let old = player_properties(old)?;
    let new = player_properties(new)?;
    Ok(old
        .keys()
        .chain(new.keys())
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|key| old.get(key) != new.get(key))
        .collect())
}

fn player_properties(player: &MprisPlayer) -> Result<BTreeMap<PropertyKey, LocusValue>> {
    let mut properties = BTreeMap::new();
    insert(&mut properties, "id", string(&player.id))?;
    insert(&mut properties, "source", string(SOURCE))?;
    insert(
        &mut properties,
        "service-name",
        string(&player.service_name),
    )?;
    insert(
        &mut properties,
        "playerctl-name",
        string(&player.playerctl_name),
    )?;
    insert(&mut properties, "identity", string(&player.identity))?;
    insert(&mut properties, "artist", string(&player.artist))?;
    insert(&mut properties, "title", string(&player.title))?;
    insert(&mut properties, "album", string(&player.album))?;
    insert(&mut properties, "art-url", string(&player.art_url))?;
    insert(
        &mut properties,
        "playback-status",
        string(&player.playback_status),
    )?;
    insert(
        &mut properties,
        "can-play",
        LocusValue::Bool(player.can_play),
    )?;
    insert(
        &mut properties,
        "can-pause",
        LocusValue::Bool(player.can_pause),
    )?;
    insert(
        &mut properties,
        "can-go-next",
        LocusValue::Bool(player.can_go_next),
    )?;
    insert(
        &mut properties,
        "can-go-previous",
        LocusValue::Bool(player.can_go_previous),
    )?;
    if let Some(length_us) = player.length_us {
        insert(&mut properties, "length-us", string(length_us.to_string()))?;
    }
    Ok(properties)
}

pub fn player_id_from_service(service_name: &str) -> String {
    let id = service_name
        .strip_prefix("org.mpris.MediaPlayer2.")
        .unwrap_or(service_name);
    id.chars()
        .map(|char| match char {
            '\0' | '/' => '_',
            char => char,
        })
        .collect()
}

pub fn playerctl_name_from_service(service_name: &str) -> String {
    service_name
        .strip_prefix("org.mpris.MediaPlayer2.")
        .unwrap_or(service_name)
        .to_owned()
}

fn mpris_node(local: &str) -> Result<NodeId> {
    node_id(MPRIS_KIND, local)
}

fn player_node(local: impl AsRef<str>) -> Result<NodeId> {
    node_id(MPRIS_PLAYER_KIND, local)
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

#[cfg(test)]
mod test {
    use std::collections::BTreeMap;

    use locusfs_graph::{GraphChange, LocusValue, NodeKind, PropertyKey, RelationName};

    use super::{MprisPlayer, MprisSnapshot, MprisState, player_properties};
    use crate::{MPRIS_KIND, MPRIS_PLAYER_KIND};

    #[test]
    fn state_exposes_player_properties_and_relation() {
        let mut state = MprisState::default();
        state
            .apply_snapshot(MprisSnapshot {
                players: BTreeMap::from([("firefox".to_string(), player("firefox", "Playing"))]),
            })
            .unwrap();

        let source = super::mpris_node("player").unwrap();
        let target = super::player_node("firefox").unwrap();

        assert_eq!(
            state
                .targets(&source, &RelationName::new("firefox").unwrap())
                .unwrap(),
            vec![target.clone()]
        );
        assert_eq!(
            state
                .nodes(&NodeKind::new(MPRIS_KIND).unwrap())
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            state
                .nodes(&NodeKind::new(MPRIS_PLAYER_KIND).unwrap())
                .unwrap(),
            vec![target]
        );
    }

    #[test]
    fn player_properties_include_bar_fields() {
        let properties = player_properties(&player("firefox", "Paused")).unwrap();

        assert_eq!(
            properties[&PropertyKey::new("title").unwrap()],
            LocusValue::String("Track".to_string())
        );
        assert_eq!(
            properties[&PropertyKey::new("can-go-next").unwrap()],
            LocusValue::Bool(true)
        );
    }

    #[test]
    fn state_emits_relation_and_property_changes() {
        let mut state = MprisState::default();
        let added = state
            .apply_snapshot(MprisSnapshot {
                players: BTreeMap::from([("firefox".to_string(), player("firefox", "Paused"))]),
            })
            .unwrap();

        assert!(added.contains(&GraphChange::NodeAdded {
            node: super::player_node("firefox").unwrap()
        }));
        assert!(added.contains(&GraphChange::RelationAdded {
            source: super::mpris_node("player").unwrap(),
            relation: RelationName::new("firefox").unwrap()
        }));

        let changed = state
            .apply_snapshot(MprisSnapshot {
                players: BTreeMap::from([("firefox".to_string(), player("firefox", "Playing"))]),
            })
            .unwrap();

        assert!(changed.contains(&GraphChange::PropertyChanged {
            node: super::player_node("firefox").unwrap(),
            key: PropertyKey::new("playback-status").unwrap()
        }));
    }

    fn player(id: &str, status: &str) -> MprisPlayer {
        MprisPlayer {
            id: id.to_string(),
            service_name: format!("org.mpris.MediaPlayer2.{id}"),
            playerctl_name: id.to_string(),
            identity: "Firefox".to_string(),
            artist: "Artist".to_string(),
            title: "Track".to_string(),
            album: "Album".to_string(),
            art_url: "file:///tmp/art.png".to_string(),
            playback_status: status.to_string(),
            can_play: true,
            can_pause: true,
            can_go_next: true,
            can_go_previous: true,
            length_us: Some(1_000_000),
        }
    }
}
