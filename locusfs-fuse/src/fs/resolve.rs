use fuser::Errno;
use locusfs_graph::{DynamicGraph, GraphError, NodeId, RelationName};

use super::name::{
    decode_relation_target_name, node_id_from_kind_and_segment, node_kind_from_segment,
    property_key_from_segment, relation_name_from_segment,
};
use super::watch::{WatchKey, WatchSubjectKey, WatchTarget};
use crate::graph_error_to_errno;

pub(super) fn parse_watch_subscription(data: &[u8]) -> std::result::Result<String, Errno> {
    let path = std::str::from_utf8(data).map_err(|_| Errno::EINVAL)?.trim();
    if path.is_empty() || !path.starts_with('/') {
        return Err(Errno::EINVAL);
    }
    Ok(path.to_string())
}

pub(crate) fn resolve_watch_path(
    graph: &DynamicGraph,
    path: &str,
) -> std::result::Result<WatchTarget, Errno> {
    let mut segments = path
        .split('/')
        .filter(|segment| !segment.is_empty())
        .peekable();

    let Some(kind_segment) = segments.next() else {
        return Err(Errno::EINVAL);
    };
    let Some(local_segment) = segments.next() else {
        return Err(Errno::EINVAL);
    };

    let kind = node_kind_from_segment(kind_segment)?;
    let mut node = node_id_from_kind_and_segment(kind, local_segment)?;
    ensure_node_exists(graph, &node)?;

    let mut dependencies = Vec::new();
    while let Some(segment) = segments.next() {
        let relation = relation_name_from_segment(segment)?;
        let targets = relation_targets(graph, &node, &relation)?;
        let has_relation = !targets.is_empty();
        let property = property_key_from_segment(segment)?;
        let has_property = graph.property_spec(&node, &property).is_ok();

        if has_property && has_relation {
            return Err(Errno::EIO);
        }

        if has_property {
            if segments.next().is_some() {
                return Err(Errno::ENOTDIR);
            }
            let key = property_key_from_segment(segment)?;
            return Ok(WatchTarget {
                subject: WatchSubjectKey::Property(node, key),
                dependencies,
            });
        }

        if !has_relation {
            return Err(Errno::ENOENT);
        }

        push_unique(
            &mut dependencies,
            WatchKey::Relation(node.clone(), relation.clone()),
        );

        let target = if targets.len() == 1 {
            targets[0].clone()
        } else {
            let Some(target_segment) = segments.next() else {
                return Ok(WatchTarget {
                    subject: WatchSubjectKey::Node(node),
                    dependencies,
                });
            };
            let target = NodeId::parse(&decode_relation_target_name(target_segment)?)
                .map_err(graph_error_to_errno)?;
            if !targets.contains(&target) {
                return Err(Errno::ENOENT);
            }
            target
        };
        node = target;
    }

    Ok(WatchTarget {
        subject: WatchSubjectKey::Node(node),
        dependencies,
    })
}

fn ensure_node_exists(graph: &DynamicGraph, node: &NodeId) -> std::result::Result<(), Errno> {
    if graph.contains_node(node).map_err(graph_error_to_errno)? {
        Ok(())
    } else {
        Err(Errno::ENOENT)
    }
}

fn relation_targets(
    graph: &DynamicGraph,
    source: &NodeId,
    relation: &RelationName,
) -> std::result::Result<Vec<NodeId>, Errno> {
    match graph.targets(source, relation) {
        Ok(targets) => Ok(targets),
        Err(GraphError::NotFound { .. }) => Ok(Vec::new()),
        Err(error) => Err(graph_error_to_errno(error)),
    }
}

fn push_unique<T: Eq>(values: &mut Vec<T>, value: T) {
    if !values.contains(&value) {
        values.push(value);
    }
}
