use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::panic::{AssertUnwindSafe, catch_unwind};

use locusfs_graph::{
    GraphChange, GraphError, LocusValue, NodeId, NodeKind, PropertyKey, PropertySpec, RelationName,
    Result,
};
use niri_ipc::state::{EventStreamState, EventStreamStatePart};
use niri_ipc::{Event, Output, Window, Workspace};

use crate::{CONTEXT_KIND, OUTPUT_KIND, WINDOW_KIND, WORKSPACE_KIND};

const SELECTED_CONTEXT: &str = "selected";
const SOURCE: &str = "niri";
const WINDOW_RELATION: &str = "window";
const WORKSPACE_RELATION: &str = "workspace";
const OUTPUT_RELATION: &str = "output";

#[derive(Debug, Default)]
pub struct NiriState {
    outputs: HashMap<String, Output>,
    stream: EventStreamState,
    reported_selected_window_id: Option<u64>,
}

impl NiriState {
    pub fn new(outputs: HashMap<String, Output>) -> Self {
        Self {
            outputs,
            stream: EventStreamState::default(),
            reported_selected_window_id: None,
        }
    }

    pub fn apply_event(&mut self, event: Event) -> Result<Vec<GraphChange>> {
        let (changes, reported_selected_window_id) = self.changes_for_event(&event)?;
        catch_unwind(AssertUnwindSafe(|| {
            self.stream.apply(event);
        }))
        .map_err(|payload| GraphError::Io(panic_message(payload)))?;
        self.reported_selected_window_id = reported_selected_window_id;
        Ok(changes)
    }

    pub fn contains_node(&self, node: &NodeId) -> Result<bool> {
        Ok(match node.kind().as_str() {
            WINDOW_KIND => self.window(node).is_some(),
            WORKSPACE_KIND => self.workspace(node).is_some(),
            OUTPUT_KIND => self.outputs.contains_key(node.local()),
            CONTEXT_KIND => node.local() == SELECTED_CONTEXT,
            _ => false,
        })
    }

    pub fn nodes(&self, kind: &NodeKind) -> Result<Vec<NodeId>> {
        let mut nodes = match kind.as_str() {
            WINDOW_KIND => self
                .stream
                .windows
                .windows
                .keys()
                .map(|id| node_id(WINDOW_KIND, id.to_string()))
                .collect::<Result<Vec<_>>>()?,
            WORKSPACE_KIND => self
                .stream
                .workspaces
                .workspaces
                .keys()
                .map(|id| node_id(WORKSPACE_KIND, id.to_string()))
                .collect::<Result<Vec<_>>>()?,
            OUTPUT_KIND => self
                .outputs
                .keys()
                .map(|name| node_id(OUTPUT_KIND, name))
                .collect::<Result<Vec<_>>>()?,
            CONTEXT_KIND => vec![node_id(CONTEXT_KIND, SELECTED_CONTEXT)?],
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
            WINDOW_KIND => self
                .window(node)
                .map(|window| window_properties(window, self.focused_window_id()))
                .transpose()?
                .ok_or_else(|| node_not_found(node)),
            WORKSPACE_KIND => self
                .workspace(node)
                .map(|workspace| workspace_properties(workspace, self.focused_workspace_id()))
                .transpose()?
                .ok_or_else(|| node_not_found(node)),
            OUTPUT_KIND => self
                .outputs
                .get(node.local())
                .map(output_properties)
                .transpose()?
                .ok_or_else(|| node_not_found(node)),
            CONTEXT_KIND if node.local() == SELECTED_CONTEXT => context_properties(),
            _ => Err(node_not_found(node)),
        }
    }

    fn node_relations(&self, node: &NodeId) -> Result<BTreeMap<RelationName, Vec<NodeId>>> {
        let mut relations = BTreeMap::new();

        match node.kind().as_str() {
            WINDOW_KIND => {
                let window = self.window(node).ok_or_else(|| node_not_found(node))?;
                if let Some(workspace_id) = window.workspace_id {
                    let target = workspace_id_node(workspace_id)?;
                    if self.contains_node(&target)? {
                        relations.insert(relation(WORKSPACE_RELATION)?, vec![target]);
                    }
                }
            }
            WORKSPACE_KIND => {
                let workspace = self.workspace(node).ok_or_else(|| node_not_found(node))?;
                if let Some(output) = workspace.output.as_deref() {
                    let target = node_id(OUTPUT_KIND, output)?;
                    if self.contains_node(&target)? {
                        relations.insert(relation(OUTPUT_RELATION)?, vec![target]);
                    }
                }
            }
            OUTPUT_KIND => {
                if !self.outputs.contains_key(node.local()) {
                    return Err(node_not_found(node));
                }
            }
            CONTEXT_KIND if node.local() == SELECTED_CONTEXT => {
                if let Some(workspace) = self.focused_workspace() {
                    relations.insert(
                        relation(WORKSPACE_RELATION)?,
                        vec![workspace_id_node(workspace.id)?],
                    );
                }
                if let Some(window_id) = self.focused_window_id() {
                    let target = window_id_node(window_id)?;
                    if self.contains_node(&target)? {
                        relations.insert(relation(WINDOW_RELATION)?, vec![target]);
                    }
                }
            }
            _ => return Err(node_not_found(node)),
        }

        Ok(relations)
    }

    fn window(&self, node: &NodeId) -> Option<&Window> {
        node.local()
            .parse::<u64>()
            .ok()
            .and_then(|id| self.stream.windows.windows.get(&id))
    }

    fn workspace(&self, node: &NodeId) -> Option<&Workspace> {
        node.local()
            .parse::<u64>()
            .ok()
            .and_then(|id| self.stream.workspaces.workspaces.get(&id))
    }

    fn focused_workspace(&self) -> Option<&Workspace> {
        self.stream
            .workspaces
            .workspaces
            .values()
            .find(|workspace| workspace.is_focused)
    }

    fn focused_workspace_id(&self) -> Option<u64> {
        self.focused_workspace().map(|workspace| workspace.id)
    }

    fn focused_window_id(&self) -> Option<u64> {
        self.stream
            .windows
            .windows
            .values()
            .find(|window| window.is_focused)
            .map(|window| window.id)
            .or_else(|| self.focused_workspace()?.active_window_id)
    }

    fn changes_for_event(&self, event: &Event) -> Result<(Vec<GraphChange>, Option<u64>)> {
        let mut changes = Vec::new();
        let mut reported_selected_window_id = self.reported_selected_window_id;
        match event {
            Event::WorkspacesChanged { workspaces } => {
                let old_selected_workspace_id = self.focused_workspace_id();
                let old_workspace_ids = self
                    .stream
                    .workspaces
                    .workspaces
                    .keys()
                    .copied()
                    .collect::<BTreeSet<_>>();
                let new_workspace_ids = workspaces
                    .iter()
                    .map(|workspace| workspace.id)
                    .collect::<BTreeSet<_>>();
                changes.push(GraphChange::NodeKindChanged {
                    kind: NodeKind::new(WORKSPACE_KIND)?,
                });
                for id in old_workspace_ids.difference(&new_workspace_ids) {
                    changes.push(GraphChange::NodeRemoved {
                        node: workspace_id_node(*id)?,
                    });
                }
                for workspace in workspaces {
                    let node = workspace_id_node(workspace.id)?;
                    changes.push(if old_workspace_ids.contains(&workspace.id) {
                        GraphChange::NodeChanged { node: node.clone() }
                    } else {
                        GraphChange::NodeAdded { node: node.clone() }
                    });
                    changes.push(GraphChange::RelationChanged {
                        source: node,
                        relation: relation(OUTPUT_RELATION)?,
                    });
                }
                let new_selected_workspace_id = workspaces
                    .iter()
                    .find(|workspace| workspace.is_focused)
                    .map(|workspace| workspace.id);
                push_selected_workspace_property_changes(
                    &mut changes,
                    old_selected_workspace_id,
                    new_selected_workspace_id,
                )?;
                changes.push(GraphChange::RelationChanged {
                    source: selected_context_node()?,
                    relation: relation(WORKSPACE_RELATION)?,
                });
                changes.push(GraphChange::RelationChanged {
                    source: selected_context_node()?,
                    relation: relation(WINDOW_RELATION)?,
                });
            }
            Event::WorkspaceUrgencyChanged { id, .. } => {
                changes.push(property_change(workspace_id_node(*id)?, "urgent")?);
            }
            Event::WorkspaceActivated { id, focused } => {
                let old_selected_workspace_id = self.focused_workspace_id();
                changes.push(GraphChange::NodeKindChanged {
                    kind: NodeKind::new(WORKSPACE_KIND)?,
                });
                changes.push(property_change(workspace_id_node(*id)?, "active")?);
                changes.push(property_change(workspace_id_node(*id)?, "focused")?);
                if *focused {
                    push_selected_workspace_property_changes(
                        &mut changes,
                        old_selected_workspace_id,
                        Some(*id),
                    )?;
                    changes.push(GraphChange::RelationChanged {
                        source: selected_context_node()?,
                        relation: relation(WORKSPACE_RELATION)?,
                    });
                    changes.push(GraphChange::RelationChanged {
                        source: selected_context_node()?,
                        relation: relation(WINDOW_RELATION)?,
                    });
                }
            }
            Event::WorkspaceActiveWindowChanged { workspace_id, .. } => {
                let old_selected_window_id = self.focused_window_id();
                changes.push(property_change(
                    workspace_id_node(*workspace_id)?,
                    "active-window-id",
                )?);
                if self
                    .focused_workspace()
                    .is_some_and(|workspace| workspace.id == *workspace_id)
                {
                    push_selected_window_property_changes(
                        &mut changes,
                        &mut reported_selected_window_id,
                        old_selected_window_id,
                        self.workspace_event_active_window_id(event),
                    )?;
                }
                changes.push(GraphChange::RelationChanged {
                    source: selected_context_node()?,
                    relation: relation(WINDOW_RELATION)?,
                });
            }
            Event::WindowsChanged { windows } => {
                let old_window_ids = self
                    .stream
                    .windows
                    .windows
                    .keys()
                    .copied()
                    .collect::<BTreeSet<_>>();
                let new_window_ids = windows
                    .iter()
                    .map(|window| window.id)
                    .collect::<BTreeSet<_>>();
                changes.push(GraphChange::NodeKindChanged {
                    kind: NodeKind::new(WINDOW_KIND)?,
                });
                for id in old_window_ids.difference(&new_window_ids) {
                    changes.push(GraphChange::NodeRemoved {
                        node: window_id_node(*id)?,
                    });
                }
                for window in windows {
                    let node = window_id_node(window.id)?;
                    changes.push(if old_window_ids.contains(&window.id) {
                        GraphChange::NodeChanged { node: node.clone() }
                    } else {
                        GraphChange::NodeAdded { node: node.clone() }
                    });
                    changes.push(GraphChange::RelationChanged {
                        source: node,
                        relation: relation(WORKSPACE_RELATION)?,
                    });
                }
                changes.push(GraphChange::RelationChanged {
                    source: selected_context_node()?,
                    relation: relation(WINDOW_RELATION)?,
                });
            }
            Event::WindowOpenedOrChanged { window } => {
                let old_selected_window_id = self.focused_window_id();
                let node = window_id_node(window.id)?;
                changes.push(GraphChange::NodeKindChanged {
                    kind: NodeKind::new(WINDOW_KIND)?,
                });
                changes.push(if self.window(&node).is_some() {
                    GraphChange::NodeChanged { node: node.clone() }
                } else {
                    GraphChange::NodeAdded { node: node.clone() }
                });
                changes.push(GraphChange::RelationChanged {
                    source: node,
                    relation: relation(WORKSPACE_RELATION)?,
                });
                if window.is_focused {
                    push_selected_window_property_changes(
                        &mut changes,
                        &mut reported_selected_window_id,
                        old_selected_window_id,
                        Some(window.id),
                    )?;
                    changes.push(GraphChange::RelationChanged {
                        source: selected_context_node()?,
                        relation: relation(WINDOW_RELATION)?,
                    });
                }
            }
            Event::WindowClosed { id } => {
                let old_selected_window_id = self.focused_window_id();
                changes.push(GraphChange::NodeRemoved {
                    node: window_id_node(*id)?,
                });
                changes.push(GraphChange::NodeKindChanged {
                    kind: NodeKind::new(WINDOW_KIND)?,
                });
                if old_selected_window_id == Some(*id) {
                    changes.push(property_change(window_id_node(*id)?, "selected")?);
                }
                changes.push(GraphChange::RelationChanged {
                    source: selected_context_node()?,
                    relation: relation(WINDOW_RELATION)?,
                });
            }
            Event::WindowFocusChanged { id } => {
                let old_selected_window_id = if id.is_some()
                    && self
                        .focused_workspace()
                        .and_then(|workspace| workspace.active_window_id)
                        == *id
                {
                    *id
                } else {
                    self.focused_window_id()
                };
                changes.push(GraphChange::NodeKindChanged {
                    kind: NodeKind::new(WINDOW_KIND)?,
                });
                if let Some(id) = id {
                    changes.push(property_change(window_id_node(*id)?, "focused")?);
                }
                push_selected_window_property_changes(
                    &mut changes,
                    &mut reported_selected_window_id,
                    old_selected_window_id,
                    *id,
                )?;
                changes.push(GraphChange::RelationChanged {
                    source: selected_context_node()?,
                    relation: relation(WINDOW_RELATION)?,
                });
            }
            Event::WindowUrgencyChanged { id, .. } => {
                changes.push(property_change(window_id_node(*id)?, "urgent")?);
            }
            Event::WindowLayoutsChanged { changes: layouts } => {
                for (id, _) in layouts {
                    changes.push(GraphChange::NodeChanged {
                        node: window_id_node(*id)?,
                    });
                }
            }
            Event::WindowFocusTimestampChanged { .. }
            | Event::KeyboardLayoutsChanged { .. }
            | Event::KeyboardLayoutSwitched { .. }
            | Event::OverviewOpenedOrClosed { .. }
            | Event::ConfigLoaded { .. }
            | Event::ScreenshotCaptured { .. }
            | Event::CastsChanged { .. }
            | Event::CastStartedOrChanged { .. }
            | Event::CastStopped { .. } => {}
        }
        Ok((changes, reported_selected_window_id))
    }

    fn workspace_event_active_window_id(&self, event: &Event) -> Option<u64> {
        match event {
            Event::WorkspaceActiveWindowChanged {
                active_window_id, ..
            } => *active_window_id,
            _ => None,
        }
    }
}

fn output_properties(output: &Output) -> Result<BTreeMap<PropertyKey, LocusValue>> {
    let mut properties = BTreeMap::new();
    insert(&mut properties, "kind", string(OUTPUT_KIND))?;
    insert(&mut properties, "source", string(SOURCE))?;
    insert(&mut properties, "connector", string(&output.name))?;
    insert(&mut properties, "name", string(&output.name))?;
    insert(&mut properties, "make", string(&output.make))?;
    insert(&mut properties, "model", string(&output.model))?;
    insert_optional_string(&mut properties, "serial", output.serial.as_deref())?;
    insert(
        &mut properties,
        "is-custom-mode",
        LocusValue::Bool(output.is_custom_mode),
    )?;
    insert(
        &mut properties,
        "vrr-supported",
        LocusValue::Bool(output.vrr_supported),
    )?;
    insert(
        &mut properties,
        "vrr-enabled",
        LocusValue::Bool(output.vrr_enabled),
    )?;

    if let Some((width, height)) = output.physical_size {
        insert(&mut properties, "physical-width", LocusValue::U32(width))?;
        insert(&mut properties, "physical-height", LocusValue::U32(height))?;
    }
    if let Some(mode_index) = output.current_mode {
        insert_usize(&mut properties, "current-mode", mode_index)?;
    }
    if let Some(logical) = output.logical {
        insert(&mut properties, "x", LocusValue::I32(logical.x))?;
        insert(&mut properties, "y", LocusValue::I32(logical.y))?;
        insert(&mut properties, "width", LocusValue::U32(logical.width))?;
        insert(&mut properties, "height", LocusValue::U32(logical.height))?;
        insert(&mut properties, "scale", LocusValue::F64(logical.scale))?;
        insert(
            &mut properties,
            "transform",
            string(format!("{:?}", logical.transform)),
        )?;
    }

    Ok(properties)
}

fn workspace_properties(
    workspace: &Workspace,
    selected_workspace_id: Option<u64>,
) -> Result<BTreeMap<PropertyKey, LocusValue>> {
    let mut properties = BTreeMap::new();
    insert(&mut properties, "kind", string(WORKSPACE_KIND))?;
    insert(&mut properties, "source", string(SOURCE))?;
    insert(&mut properties, "id", string(workspace.id.to_string()))?;
    insert(
        &mut properties,
        "index",
        LocusValue::U32(workspace.idx.into()),
    )?;
    insert(
        &mut properties,
        "idx",
        LocusValue::U32(workspace.idx.into()),
    )?;
    insert(
        &mut properties,
        "name",
        string(
            workspace
                .name
                .clone()
                .unwrap_or_else(|| workspace.idx.to_string()),
        ),
    )?;
    insert(
        &mut properties,
        "urgent",
        LocusValue::Bool(workspace.is_urgent),
    )?;
    insert(
        &mut properties,
        "active",
        LocusValue::Bool(workspace.is_active),
    )?;
    insert(
        &mut properties,
        "focused",
        LocusValue::Bool(workspace.is_focused),
    )?;
    insert(
        &mut properties,
        "selected",
        LocusValue::Bool(selected_workspace_id == Some(workspace.id)),
    )?;
    if let Some(active_window_id) = workspace.active_window_id {
        insert(
            &mut properties,
            "active-window-id",
            string(active_window_id.to_string()),
        )?;
    }
    Ok(properties)
}

fn window_properties(
    window: &Window,
    selected_window_id: Option<u64>,
) -> Result<BTreeMap<PropertyKey, LocusValue>> {
    let mut properties = BTreeMap::new();
    insert(&mut properties, "kind", string(WINDOW_KIND))?;
    insert(&mut properties, "source", string(SOURCE))?;
    insert(&mut properties, "id", string(window.id.to_string()))?;
    insert_optional_string(&mut properties, "title", window.title.as_deref())?;
    insert_optional_string(&mut properties, "app-id", window.app_id.as_deref())?;
    if let Some(pid) = window.pid {
        insert(&mut properties, "pid", LocusValue::I32(pid))?;
    }
    if let Some(workspace_id) = window.workspace_id {
        insert(
            &mut properties,
            "workspace-id",
            string(workspace_id.to_string()),
        )?;
    }
    insert(
        &mut properties,
        "focused",
        LocusValue::Bool(window.is_focused),
    )?;
    insert(
        &mut properties,
        "selected",
        LocusValue::Bool(selected_window_id == Some(window.id)),
    )?;
    insert(
        &mut properties,
        "floating",
        LocusValue::Bool(window.is_floating),
    )?;
    insert(
        &mut properties,
        "urgent",
        LocusValue::Bool(window.is_urgent),
    )?;
    if let Some((column, row)) = window.layout.pos_in_scrolling_layout {
        insert_usize(&mut properties, "column", column)?;
        insert_usize(&mut properties, "row", row)?;
    }
    insert(
        &mut properties,
        "tile-width",
        LocusValue::F64(window.layout.tile_size.0),
    )?;
    insert(
        &mut properties,
        "tile-height",
        LocusValue::F64(window.layout.tile_size.1),
    )?;
    insert(
        &mut properties,
        "window-width",
        LocusValue::I32(window.layout.window_size.0),
    )?;
    insert(
        &mut properties,
        "window-height",
        LocusValue::I32(window.layout.window_size.1),
    )?;
    Ok(properties)
}

fn context_properties() -> Result<BTreeMap<PropertyKey, LocusValue>> {
    let mut properties = BTreeMap::new();
    insert(&mut properties, "kind", string(CONTEXT_KIND))?;
    insert(&mut properties, "source", string(SOURCE))?;
    insert(&mut properties, "name", string(SELECTED_CONTEXT))?;
    Ok(properties)
}

fn insert(
    properties: &mut BTreeMap<PropertyKey, LocusValue>,
    key: &'static str,
    value: LocusValue,
) -> Result<()> {
    properties.insert(PropertyKey::new(key)?, value);
    Ok(())
}

fn insert_optional_string(
    properties: &mut BTreeMap<PropertyKey, LocusValue>,
    key: &'static str,
    value: Option<&str>,
) -> Result<()> {
    if let Some(value) = value {
        insert(properties, key, string(value))?;
    }
    Ok(())
}

fn insert_usize(
    properties: &mut BTreeMap<PropertyKey, LocusValue>,
    key: &'static str,
    value: usize,
) -> Result<()> {
    let value = u32::try_from(value).map_err(|_| GraphError::InvalidValue {
        kind: "usize property",
        value: value.to_string(),
        reason: "value does not fit in u32",
    })?;
    insert(properties, key, LocusValue::U32(value))
}

fn relation(name: &str) -> Result<RelationName> {
    RelationName::new(name)
}

fn node_id(kind: &str, local: impl Into<String>) -> Result<NodeId> {
    NodeId::new(NodeKind::new(kind)?, local)
}

fn window_id_node(id: u64) -> Result<NodeId> {
    node_id(WINDOW_KIND, id.to_string())
}

fn workspace_id_node(id: u64) -> Result<NodeId> {
    node_id(WORKSPACE_KIND, id.to_string())
}

fn selected_context_node() -> Result<NodeId> {
    node_id(CONTEXT_KIND, SELECTED_CONTEXT)
}

fn property_change(node: NodeId, key: &'static str) -> Result<GraphChange> {
    Ok(GraphChange::PropertyChanged {
        node,
        key: PropertyKey::new(key)?,
    })
}

fn push_selected_window_property_changes(
    changes: &mut Vec<GraphChange>,
    reported_selected_window_id: &mut Option<u64>,
    old_selected_window_id: Option<u64>,
    new_selected_window_id: Option<u64>,
) -> Result<()> {
    let old_selected_window_id = (*reported_selected_window_id).or(old_selected_window_id);
    if old_selected_window_id == new_selected_window_id {
        *reported_selected_window_id = new_selected_window_id;
        return Ok(());
    }
    if let Some(id) = old_selected_window_id {
        changes.push(property_change(window_id_node(id)?, "selected")?);
    }
    if let Some(id) = new_selected_window_id {
        changes.push(property_change(window_id_node(id)?, "selected")?);
    }
    *reported_selected_window_id = new_selected_window_id;
    Ok(())
}

fn push_selected_workspace_property_changes(
    changes: &mut Vec<GraphChange>,
    old_selected_workspace_id: Option<u64>,
    new_selected_workspace_id: Option<u64>,
) -> Result<()> {
    if let Some(id) = old_selected_workspace_id {
        changes.push(property_change(workspace_id_node(id)?, "selected")?);
    }
    if let Some(id) = new_selected_workspace_id
        && Some(id) != old_selected_workspace_id
    {
        changes.push(property_change(workspace_id_node(id)?, "selected")?);
    }
    Ok(())
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

fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    let message = if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic".to_string()
    };
    format!("niri event stream state panicked: {message}")
}

#[cfg(test)]
mod test;
