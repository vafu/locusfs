use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use locusfs_graph::{
    GraphChange, GraphError, LocusValue, NodeId, NodeKind, PropertyKey, PropertySpec, RelationName,
    Result,
};
use serde::Deserialize;
use tokio::sync::RwLock;

use crate::{PIPEWIRE_KIND, PIPEWIRE_SINK_KIND, PIPEWIRE_SOURCE_KIND};

pub type SharedPipeWireState = Arc<RwLock<PipeWireState>>;

pub const DEFAULT_NODE: &str = "default";
pub const SINK_NODE: &str = "sink";
pub const SOURCE_NODE: &str = "source";
pub const SINK_RELATION: &str = "sink";
pub const SOURCE_RELATION: &str = "source";

const SOURCE: &str = "pipewire";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PipeWireSnapshot {
    pub default_sink: Option<String>,
    pub default_source: Option<String>,
    pub sinks: BTreeMap<String, AudioEndpoint>,
    pub sources: BTreeMap<String, AudioEndpoint>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AudioEndpoint {
    pub id: u32,
    pub name: String,
    pub description: String,
    pub nick: String,
    pub state: EndpointState,
    pub muted: bool,
    pub volume: f64,
    pub media_class: String,
    pub form_factor: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EndpointState {
    Running,
    Idle,
    Suspended,
    Error,
    Unknown,
}

#[derive(Debug, Default)]
pub struct PipeWireState {
    snapshot: PipeWireSnapshot,
}

impl PipeWireState {
    pub fn shared() -> SharedPipeWireState {
        Arc::new(RwLock::new(Self::default()))
    }

    pub fn apply_snapshot(&mut self, snapshot: PipeWireSnapshot) -> Result<Vec<GraphChange>> {
        let changes = snapshot_changes(&self.snapshot, &snapshot)?;
        self.snapshot = snapshot;
        Ok(changes)
    }

    pub fn contains_node(&self, node: &NodeId) -> Result<bool> {
        Ok(match node.kind().as_str() {
            PIPEWIRE_KIND => matches!(node.local(), DEFAULT_NODE | SINK_NODE | SOURCE_NODE),
            PIPEWIRE_SINK_KIND => self.snapshot.sinks.contains_key(node.local()),
            PIPEWIRE_SOURCE_KIND => self.snapshot.sources.contains_key(node.local()),
            _ => false,
        })
    }

    pub fn nodes(&self, kind: &NodeKind) -> Result<Vec<NodeId>> {
        let mut nodes = match kind.as_str() {
            PIPEWIRE_KIND => [DEFAULT_NODE, SINK_NODE, SOURCE_NODE]
                .into_iter()
                .map(|local| node_id(PIPEWIRE_KIND, local))
                .collect::<Result<Vec<_>>>()?,
            PIPEWIRE_SINK_KIND => self
                .snapshot
                .sinks
                .keys()
                .map(|id| node_id(PIPEWIRE_SINK_KIND, id))
                .collect::<Result<Vec<_>>>()?,
            PIPEWIRE_SOURCE_KIND => self
                .snapshot
                .sources
                .keys()
                .map(|id| node_id(PIPEWIRE_SOURCE_KIND, id))
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
            PIPEWIRE_KIND if matches!(node.local(), DEFAULT_NODE | SINK_NODE | SOURCE_NODE) => {
                facade_properties(node.local())
            }
            PIPEWIRE_SINK_KIND => self
                .snapshot
                .sinks
                .get(node.local())
                .map(endpoint_properties)
                .transpose()?
                .ok_or_else(|| node_not_found(node)),
            PIPEWIRE_SOURCE_KIND => self
                .snapshot
                .sources
                .get(node.local())
                .map(endpoint_properties)
                .transpose()?
                .ok_or_else(|| node_not_found(node)),
            _ => Err(node_not_found(node)),
        }
    }

    fn node_relations(&self, node: &NodeId) -> Result<BTreeMap<RelationName, Vec<NodeId>>> {
        let mut relations = BTreeMap::new();
        match node.kind().as_str() {
            PIPEWIRE_KIND if node.local() == DEFAULT_NODE => {
                if let Some(id) = &self.snapshot.default_sink {
                    relations.insert(relation(SINK_RELATION)?, vec![sink_node(id)?]);
                }
                if let Some(id) = &self.snapshot.default_source {
                    relations.insert(relation(SOURCE_RELATION)?, vec![source_node(id)?]);
                }
            }
            PIPEWIRE_KIND if node.local() == SINK_NODE => {
                for id in self.snapshot.sinks.keys() {
                    relations.insert(relation(id)?, vec![sink_node(id)?]);
                }
            }
            PIPEWIRE_KIND if node.local() == SOURCE_NODE => {
                for id in self.snapshot.sources.keys() {
                    relations.insert(relation(id)?, vec![source_node(id)?]);
                }
            }
            PIPEWIRE_SINK_KIND if self.snapshot.sinks.contains_key(node.local()) => {}
            PIPEWIRE_SOURCE_KIND if self.snapshot.sources.contains_key(node.local()) => {}
            _ => return Err(node_not_found(node)),
        }
        Ok(relations)
    }
}

pub fn snapshot_from_pactl(
    info: PactlInfo,
    sinks: Vec<PactlEndpoint>,
    sources: Vec<PactlEndpoint>,
) -> PipeWireSnapshot {
    let sinks = sinks
        .into_iter()
        .map(AudioEndpoint::from_pactl)
        .map(|endpoint| (endpoint.id.to_string(), endpoint))
        .collect::<BTreeMap<_, _>>();
    let sources = sources
        .into_iter()
        .filter(PactlEndpoint::is_real_source)
        .map(AudioEndpoint::from_pactl)
        .map(|endpoint| (endpoint.id.to_string(), endpoint))
        .collect::<BTreeMap<_, _>>();
    let default_sink = endpoint_id_by_name(&sinks, info.default_sink_name.as_deref());
    let default_source = endpoint_id_by_name(&sources, info.default_source_name.as_deref());

    PipeWireSnapshot {
        default_sink,
        default_source,
        sinks,
        sources,
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct PactlInfo {
    pub default_sink_name: Option<String>,
    pub default_source_name: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct PactlEndpoint {
    pub index: u32,
    pub state: Option<String>,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub mute: bool,
    #[serde(default)]
    pub volume: BTreeMap<String, PactlVolume>,
    #[serde(default)]
    pub properties: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct PactlVolume {
    pub value: u32,
}

impl PactlEndpoint {
    fn is_real_source(&self) -> bool {
        self.properties
            .get("media.class")
            .is_some_and(|value| value == "Audio/Source")
            || !self
                .properties
                .get("device.class")
                .is_some_and(|value| value == "monitor")
    }
}

impl AudioEndpoint {
    fn from_pactl(endpoint: PactlEndpoint) -> Self {
        let description = endpoint
            .description
            .clone()
            .or_else(|| endpoint.properties.get("device.description").cloned())
            .unwrap_or_else(|| endpoint.name.clone());
        let nick = endpoint
            .properties
            .get("node.nick")
            .or_else(|| endpoint.properties.get("device.nick"))
            .cloned()
            .unwrap_or_else(|| description.clone());
        let media_class = endpoint
            .properties
            .get("media.class")
            .cloned()
            .unwrap_or_else(|| "unknown".to_string());
        let form_factor = endpoint
            .properties
            .get("device.form_factor")
            .cloned()
            .unwrap_or_else(|| "unknown".to_string());

        Self {
            id: endpoint.index,
            name: endpoint.name,
            description,
            nick,
            state: EndpointState::from_pactl(endpoint.state.as_deref()),
            muted: endpoint.mute,
            volume: normalized_volume(&endpoint.volume),
            media_class,
            form_factor,
        }
    }
}

impl EndpointState {
    fn from_pactl(state: Option<&str>) -> Self {
        match state.unwrap_or_default().to_ascii_lowercase().as_str() {
            "running" => Self::Running,
            "idle" => Self::Idle,
            "suspended" => Self::Suspended,
            "error" => Self::Error,
            _ => Self::Unknown,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Idle => "idle",
            Self::Suspended => "suspended",
            Self::Error => "error",
            Self::Unknown => "unknown",
        }
    }
}

fn snapshot_changes(old: &PipeWireSnapshot, new: &PipeWireSnapshot) -> Result<Vec<GraphChange>> {
    let mut changes = Vec::new();
    changes.extend(endpoint_set_changes(
        SINK_NODE,
        PIPEWIRE_SINK_KIND,
        &old.sinks,
        &new.sinks,
    )?);
    changes.extend(endpoint_set_changes(
        SOURCE_NODE,
        PIPEWIRE_SOURCE_KIND,
        &old.sources,
        &new.sources,
    )?);
    changes.extend(default_relation_change(
        old.default_sink.as_deref(),
        new.default_sink.as_deref(),
        SINK_RELATION,
    )?);
    changes.extend(default_relation_change(
        old.default_source.as_deref(),
        new.default_source.as_deref(),
        SOURCE_RELATION,
    )?);
    Ok(changes)
}

fn endpoint_set_changes(
    facade_node: &str,
    endpoint_kind: &str,
    old: &BTreeMap<String, AudioEndpoint>,
    new: &BTreeMap<String, AudioEndpoint>,
) -> Result<Vec<GraphChange>> {
    let mut changes = Vec::new();
    let old_ids = old.keys().cloned().collect::<BTreeSet<_>>();
    let new_ids = new.keys().cloned().collect::<BTreeSet<_>>();
    if old_ids != new_ids {
        changes.push(GraphChange::NodeKindChanged {
            kind: NodeKind::new(endpoint_kind)?,
        });
    }
    let facade = pipewire_node(facade_node)?;

    for id in old_ids.difference(&new_ids) {
        changes.push(GraphChange::NodeRemoved {
            node: node_id(endpoint_kind, id)?,
        });
        changes.push(GraphChange::RelationRemoved {
            source: facade.clone(),
            relation: relation(id)?,
        });
    }

    for id in new_ids.difference(&old_ids) {
        changes.push(GraphChange::NodeAdded {
            node: node_id(endpoint_kind, id)?,
        });
        changes.push(GraphChange::RelationAdded {
            source: facade.clone(),
            relation: relation(id)?,
        });
    }

    for id in old_ids.intersection(&new_ids) {
        let Some(old_endpoint) = old.get(id) else {
            continue;
        };
        let Some(new_endpoint) = new.get(id) else {
            continue;
        };
        if old_endpoint != new_endpoint {
            let node = node_id(endpoint_kind, id)?;
            changes.push(GraphChange::NodeChanged { node: node.clone() });
            for key in changed_property_keys(old_endpoint, new_endpoint)? {
                changes.push(GraphChange::PropertyChanged {
                    node: node.clone(),
                    key,
                });
            }
        }
    }

    Ok(changes)
}

fn default_relation_change(
    old: Option<&str>,
    new: Option<&str>,
    relation_name: &str,
) -> Result<Vec<GraphChange>> {
    if old == new {
        return Ok(Vec::new());
    }
    let source = pipewire_node(DEFAULT_NODE)?;
    let relation = relation(relation_name)?;
    let change = match (old, new) {
        (None, Some(_)) => GraphChange::RelationAdded { source, relation },
        (Some(_), None) => GraphChange::RelationRemoved { source, relation },
        (Some(_), Some(_)) => GraphChange::RelationChanged { source, relation },
        (None, None) => return Ok(Vec::new()),
    };
    Ok(vec![change])
}

fn changed_property_keys(old: &AudioEndpoint, new: &AudioEndpoint) -> Result<Vec<PropertyKey>> {
    let old = endpoint_properties(old)?;
    let new = endpoint_properties(new)?;
    Ok(old
        .keys()
        .chain(new.keys())
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|key| old.get(key) != new.get(key))
        .collect())
}

fn facade_properties(local: &str) -> Result<BTreeMap<PropertyKey, LocusValue>> {
    let mut properties = BTreeMap::new();
    insert(&mut properties, "kind", string(PIPEWIRE_KIND))?;
    insert(&mut properties, "source", string(SOURCE))?;
    insert(&mut properties, "name", string(local))?;
    Ok(properties)
}

fn endpoint_properties(endpoint: &AudioEndpoint) -> Result<BTreeMap<PropertyKey, LocusValue>> {
    let mut properties = BTreeMap::new();
    insert(&mut properties, "id", LocusValue::U32(endpoint.id))?;
    insert(&mut properties, "name", string(&endpoint.name))?;
    insert(
        &mut properties,
        "description",
        string(&endpoint.description),
    )?;
    insert(&mut properties, "nick", string(&endpoint.nick))?;
    insert(&mut properties, "state", string(endpoint.state.as_str()))?;
    insert(&mut properties, "muted", LocusValue::Bool(endpoint.muted))?;
    insert(&mut properties, "volume", LocusValue::F64(endpoint.volume))?;
    insert(
        &mut properties,
        "volume-percent",
        LocusValue::U32(volume_percent(endpoint.volume)),
    )?;
    insert(
        &mut properties,
        "icon-name",
        string(volume_icon(endpoint.muted, endpoint.volume)),
    )?;
    insert(
        &mut properties,
        "form-factor",
        string(&endpoint.form_factor),
    )?;
    insert(
        &mut properties,
        "media-class",
        string(&endpoint.media_class),
    )?;
    Ok(properties)
}

fn normalized_volume(volume: &BTreeMap<String, PactlVolume>) -> f64 {
    if volume.is_empty() {
        return 0.0;
    }
    let sum = volume
        .values()
        .map(|channel| channel.value as f64)
        .sum::<f64>();
    sum / volume.len() as f64 / 65536.0
}

fn volume_percent(volume: f64) -> u32 {
    (volume * 100.0).round().max(0.0) as u32
}

fn volume_icon(muted: bool, volume: f64) -> &'static str {
    let percent = volume_percent(volume);
    if muted || percent == 0 {
        "audio-volume-muted-symbolic"
    } else if percent <= 33 {
        "audio-volume-low-symbolic"
    } else if percent <= 66 {
        "audio-volume-medium-symbolic"
    } else {
        "audio-volume-high-symbolic"
    }
}

fn endpoint_id_by_name(
    endpoints: &BTreeMap<String, AudioEndpoint>,
    name: Option<&str>,
) -> Option<String> {
    let name = name?;
    endpoints
        .iter()
        .find_map(|(id, endpoint)| (endpoint.name == name).then(|| id.clone()))
}

fn pipewire_node(local: &str) -> Result<NodeId> {
    node_id(PIPEWIRE_KIND, local)
}

fn sink_node(local: &str) -> Result<NodeId> {
    node_id(PIPEWIRE_SINK_KIND, local)
}

fn source_node(local: &str) -> Result<NodeId> {
    node_id(PIPEWIRE_SOURCE_KIND, local)
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

    use super::{
        AudioEndpoint, EndpointState, PactlEndpoint, PactlInfo, PactlVolume, PipeWireSnapshot,
        PipeWireState, endpoint_properties, pipewire_node, snapshot_from_pactl, source_node,
        volume_icon,
    };
    use crate::{PIPEWIRE_SINK_KIND, PIPEWIRE_SOURCE_KIND};

    #[test]
    fn maps_pactl_endpoint_to_bar_properties() {
        let endpoint = AudioEndpoint::from_pactl(PactlEndpoint {
            index: 42,
            state: Some("RUNNING".to_string()),
            name: "alsa_output.test".to_string(),
            description: Some("Speakers".to_string()),
            mute: false,
            volume: BTreeMap::from([
                ("front-left".to_string(), PactlVolume { value: 32768 }),
                ("front-right".to_string(), PactlVolume { value: 65536 }),
            ]),
            properties: BTreeMap::from([
                ("node.nick".to_string(), "Built-in".to_string()),
                ("device.form_factor".to_string(), "speaker".to_string()),
                ("media.class".to_string(), "Audio/Sink".to_string()),
            ]),
        });

        let properties = endpoint_properties(&endpoint).unwrap();

        assert_eq!(
            properties[&PropertyKey::new("id").unwrap()],
            LocusValue::U32(42)
        );
        assert_eq!(
            properties[&PropertyKey::new("description").unwrap()],
            LocusValue::String("Speakers".to_string())
        );
        assert_eq!(
            properties[&PropertyKey::new("volume-percent").unwrap()],
            LocusValue::U32(75)
        );
        assert_eq!(
            properties[&PropertyKey::new("icon-name").unwrap()],
            LocusValue::String("audio-volume-high-symbolic".to_string())
        );
    }

    #[test]
    fn snapshot_resolves_defaults_by_name_and_filters_monitor_sources() {
        let snapshot = snapshot_from_pactl(
            PactlInfo {
                default_sink_name: Some("sink-name".to_string()),
                default_source_name: Some("source-name".to_string()),
            },
            vec![endpoint(1, "sink-name", "Audio/Sink", "sound")],
            vec![
                endpoint(2, "sink-name.monitor", "Audio/Sink", "monitor"),
                endpoint(3, "source-name", "Audio/Source", "sound"),
            ],
        );

        assert_eq!(snapshot.default_sink.as_deref(), Some("1"));
        assert_eq!(snapshot.default_source.as_deref(), Some("3"));
        assert!(snapshot.sources.contains_key("3"));
        assert!(!snapshot.sources.contains_key("2"));
    }

    #[test]
    fn state_emits_relation_lifecycle_for_default_sink_and_sink_set() {
        let mut state = PipeWireState::default();
        let changes = state
            .apply_snapshot(PipeWireSnapshot {
                default_sink: Some("1".to_string()),
                default_source: None,
                sinks: BTreeMap::from([("1".to_string(), endpoint_value(1, 0.5))]),
                sources: BTreeMap::new(),
            })
            .unwrap();

        assert!(changes.contains(&GraphChange::NodeAdded {
            node: super::sink_node("1").unwrap()
        }));
        assert!(changes.contains(&GraphChange::RelationAdded {
            source: pipewire_node("sink").unwrap(),
            relation: RelationName::new("1").unwrap()
        }));
        assert!(changes.contains(&GraphChange::RelationAdded {
            source: pipewire_node("default").unwrap(),
            relation: RelationName::new("sink").unwrap()
        }));
    }

    #[test]
    fn state_exposes_expected_paths_as_relations() {
        let mut state = PipeWireState::default();
        state
            .apply_snapshot(PipeWireSnapshot {
                default_sink: Some("1".to_string()),
                default_source: Some("2".to_string()),
                sinks: BTreeMap::from([("1".to_string(), endpoint_value(1, 0.5))]),
                sources: BTreeMap::from([("2".to_string(), endpoint_value(2, 1.0))]),
            })
            .unwrap();

        assert_eq!(
            state
                .targets(
                    &pipewire_node("default").unwrap(),
                    &RelationName::new("sink").unwrap()
                )
                .unwrap(),
            vec![super::sink_node("1").unwrap()]
        );
        assert_eq!(
            state
                .targets(
                    &pipewire_node("source").unwrap(),
                    &RelationName::new("2").unwrap()
                )
                .unwrap(),
            vec![source_node("2").unwrap()]
        );
        assert_eq!(
            state
                .nodes(&NodeKind::new(PIPEWIRE_SINK_KIND).unwrap())
                .unwrap(),
            vec![super::sink_node("1").unwrap()]
        );
        assert_eq!(
            state
                .nodes(&NodeKind::new(PIPEWIRE_SOURCE_KIND).unwrap())
                .unwrap(),
            vec![source_node("2").unwrap()]
        );
    }

    #[test]
    fn volume_icon_tracks_mute_and_ranges() {
        assert_eq!(volume_icon(true, 1.0), "audio-volume-muted-symbolic");
        assert_eq!(volume_icon(false, 0.10), "audio-volume-low-symbolic");
        assert_eq!(volume_icon(false, 0.50), "audio-volume-medium-symbolic");
        assert_eq!(volume_icon(false, 0.90), "audio-volume-high-symbolic");
    }

    fn endpoint(index: u32, name: &str, media_class: &str, device_class: &str) -> PactlEndpoint {
        PactlEndpoint {
            index,
            state: Some("SUSPENDED".to_string()),
            name: name.to_string(),
            description: None,
            mute: false,
            volume: BTreeMap::from([("front-left".to_string(), PactlVolume { value: 65536 })]),
            properties: BTreeMap::from([
                ("media.class".to_string(), media_class.to_string()),
                ("device.class".to_string(), device_class.to_string()),
            ]),
        }
    }

    fn endpoint_value(id: u32, volume: f64) -> AudioEndpoint {
        AudioEndpoint {
            id,
            name: format!("node-{id}"),
            description: format!("Node {id}"),
            nick: format!("Node {id}"),
            state: EndpointState::Suspended,
            muted: false,
            volume,
            media_class: "Audio/Sink".to_string(),
            form_factor: "unknown".to_string(),
        }
    }
}
