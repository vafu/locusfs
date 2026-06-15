use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use crate::{
    LocusFsError, LocusValue, NodeId, ProjectName, PropertyKey, RelationName, Result, ValueKind,
};

/// FUSE-independent contract implemented by graph-backed filesystem engines.
pub trait GraphFilesystem: Clone + Send + Sync + 'static {
    fn set_property(&self, subject: &NodeId, key: &PropertyKey, value: LocusValue) -> Result<()>;
    fn remove_property(&self, subject: &NodeId, key: &PropertyKey) -> Result<()>;
    fn property(&self, subject: &NodeId, key: &PropertyKey) -> Result<Option<LocusValue>>;
    fn set_link(&self, source: &NodeId, relation: &RelationName, target: &NodeId) -> Result<()>;
    fn remove_link(&self, source: &NodeId, relation: &RelationName, target: &NodeId) -> Result<()>;
    fn targets(&self, source: &NodeId, relation: &RelationName) -> Result<Vec<NodeId>>;
}

/// Current project-domain entry materialized by a symlink operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectEntry {
    pub name: ProjectName,
    pub node: NodeId,
    pub root: PathBuf,
}

/// Small in-memory graph used by the initial FUSE adapter.
#[derive(Clone, Debug, Default)]
pub struct InMemoryGraph {
    inner: Arc<RwLock<GraphState>>,
}

#[derive(Clone, Debug, Default)]
struct GraphState {
    nodes: BTreeMap<NodeId, Node>,
}

#[derive(Clone, Debug, Default)]
struct Node {
    properties: BTreeMap<PropertyKey, LocusValue>,
    links: BTreeMap<RelationName, BTreeSet<NodeId>>,
}

impl InMemoryGraph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn ensure_node(&self, node: &NodeId) -> Result<()> {
        let mut state = self.write_state()?;
        state.nodes.entry(node.clone()).or_default();
        Ok(())
    }

    pub fn upsert_project_link(
        &self,
        name: &ProjectName,
        target: impl Into<PathBuf>,
    ) -> Result<ProjectEntry> {
        let root = target.into();
        if root.as_os_str().is_empty() {
            return Err(LocusFsError::invalid_value(
                "project root",
                "",
                "empty symlink target",
            ));
        }

        let node_id = project_node_id(name)?;
        let kind = PropertyKey::new("kind")?;
        let path = PropertyKey::new("path")?;
        let display_name = PropertyKey::new("name")?;

        let mut state = self.write_state()?;
        let node = state.nodes.entry(node_id.clone()).or_default();
        node.properties
            .insert(kind, LocusValue::String("project".to_string()));
        node.properties.insert(
            path,
            LocusValue::String(root.to_string_lossy().into_owned()),
        );
        node.properties
            .entry(display_name)
            .or_insert_with(|| LocusValue::String(name.as_str().to_string()));

        Ok(ProjectEntry {
            name: name.clone(),
            node: node_id,
            root,
        })
    }

    pub fn remove_project(&self, name: &ProjectName) -> Result<()> {
        let node_id = project_node_id(name)?;
        let mut state = self.write_state()?;
        state.nodes.remove(&node_id);
        Ok(())
    }

    pub fn project(&self, name: &ProjectName) -> Result<Option<ProjectEntry>> {
        let node_id = project_node_id(name)?;
        let path = PropertyKey::new("path")?;
        let state = self.read_state()?;
        let Some(node) = state.nodes.get(&node_id) else {
            return Ok(None);
        };
        let Some(LocusValue::String(root)) = node.properties.get(&path) else {
            return Ok(None);
        };
        Ok(Some(ProjectEntry {
            name: name.clone(),
            node: node_id,
            root: PathBuf::from(root),
        }))
    }

    pub fn projects(&self) -> Result<Vec<ProjectEntry>> {
        let state = self.read_state()?;
        let path = PropertyKey::new("path")?;
        let mut projects = Vec::new();
        for (node_id, node) in &state.nodes {
            let Some(project_name) = node_id.as_str().strip_prefix("project:") else {
                continue;
            };
            let Some(LocusValue::String(root)) = node.properties.get(&path) else {
                continue;
            };
            projects.push(ProjectEntry {
                name: ProjectName::new(project_name)?,
                node: node_id.clone(),
                root: PathBuf::from(root),
            });
        }
        Ok(projects)
    }

    pub fn set_project_property(
        &self,
        project: &ProjectName,
        key: &PropertyKey,
        input: &str,
    ) -> Result<()> {
        let node_id = project_node_id(project)?;
        let value = LocusValue::parse_shell(project_property_kind(key), input)?;
        self.set_property(&node_id, key, value)
    }

    pub fn project_property(
        &self,
        project: &ProjectName,
        key: &PropertyKey,
    ) -> Result<Option<LocusValue>> {
        if key.as_str() == "git-branch" {
            return self
                .project_git_branch(project)
                .map(|branch| branch.map(|value| LocusValue::String(value.unwrap_or_default())));
        }

        let node_id = project_node_id(project)?;
        self.property(&node_id, key)
    }

    pub fn project_git_branch(&self, project: &ProjectName) -> Result<Option<Option<String>>> {
        let Some(entry) = self.project(project)? else {
            return Ok(None);
        };
        Ok(Some(current_git_branch(&entry.root)?))
    }

    fn read_state(&self) -> Result<std::sync::RwLockReadGuard<'_, GraphState>> {
        self.inner.read().map_err(|_| LocusFsError::Unsupported {
            operation: "graph lock poisoned",
        })
    }

    fn write_state(&self) -> Result<std::sync::RwLockWriteGuard<'_, GraphState>> {
        self.inner.write().map_err(|_| LocusFsError::Unsupported {
            operation: "graph lock poisoned",
        })
    }
}

impl GraphFilesystem for InMemoryGraph {
    fn set_property(&self, subject: &NodeId, key: &PropertyKey, value: LocusValue) -> Result<()> {
        let mut state = self.write_state()?;
        let node = state.nodes.entry(subject.clone()).or_default();
        node.properties.insert(key.clone(), value);
        Ok(())
    }

    fn remove_property(&self, subject: &NodeId, key: &PropertyKey) -> Result<()> {
        let mut state = self.write_state()?;
        if let Some(node) = state.nodes.get_mut(subject) {
            node.properties.remove(key);
        }
        Ok(())
    }

    fn property(&self, subject: &NodeId, key: &PropertyKey) -> Result<Option<LocusValue>> {
        let state = self.read_state()?;
        Ok(state
            .nodes
            .get(subject)
            .and_then(|node| node.properties.get(key))
            .cloned())
    }

    fn set_link(&self, source: &NodeId, relation: &RelationName, target: &NodeId) -> Result<()> {
        let mut state = self.write_state()?;
        state.nodes.entry(target.clone()).or_default();
        let source = state.nodes.entry(source.clone()).or_default();
        source
            .links
            .entry(relation.clone())
            .or_default()
            .insert(target.clone());
        Ok(())
    }

    fn remove_link(&self, source: &NodeId, relation: &RelationName, target: &NodeId) -> Result<()> {
        let mut state = self.write_state()?;
        if let Some(node) = state.nodes.get_mut(source)
            && let Some(targets) = node.links.get_mut(relation)
        {
            targets.remove(target);
        }
        Ok(())
    }

    fn targets(&self, source: &NodeId, relation: &RelationName) -> Result<Vec<NodeId>> {
        let state = self.read_state()?;
        Ok(state
            .nodes
            .get(source)
            .and_then(|node| node.links.get(relation))
            .map(|targets| targets.iter().cloned().collect())
            .unwrap_or_default())
    }
}

fn project_node_id(name: &ProjectName) -> Result<NodeId> {
    NodeId::new(format!("project:{}", name.as_str()))
}

fn project_property_kind(key: &PropertyKey) -> ValueKind {
    match key.as_str() {
        "active" => ValueKind::Bool,
        _ => ValueKind::String,
    }
}

fn current_git_branch(root: &Path) -> Result<Option<String>> {
    let head = match fs::read_to_string(root.join(".git/HEAD")) {
        Ok(head) => head,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };

    if let Some(branch) = head.trim().strip_prefix("ref: refs/heads/") {
        Ok(Some(branch.to_string()))
    } else {
        Ok(None)
    }
}

#[cfg(test)]
mod test;
