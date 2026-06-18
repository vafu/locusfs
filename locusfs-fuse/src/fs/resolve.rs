use fuse3::Errno;
use locusfs_graph::{DynamicGraph, GraphError, NodeId, NodeKind, RelationName};

use super::name::{
    node_id_from_kind_and_segment, node_kind_from_segment, property_key_from_segment,
    relation_name_from_segment, relation_target_from_name,
};
use super::watch::{WatchKey, WatchSubjectKey, WatchTarget};
use crate::{errno, graph_error_to_errno};

pub(super) fn parse_watch_subscription(data: &[u8]) -> std::result::Result<String, Errno> {
    let path = std::str::from_utf8(data)
        .map_err(|_| errno(libc::EINVAL))?
        .trim();
    if path.is_empty() || !path.starts_with('/') {
        return Err(errno(libc::EINVAL));
    }
    Ok(path.to_string())
}

pub(crate) async fn resolve_watch_path(
    graph: &DynamicGraph,
    path: &str,
) -> std::result::Result<WatchTarget, Errno> {
    let segments = path
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    let mut index = 0;

    let Some(kind_segment) = next_segment(&segments, &mut index) else {
        return Err(errno(libc::EINVAL));
    };
    let kind = node_kind_from_segment(kind_segment)?;
    let Some(local_segment) = next_segment(&segments, &mut index) else {
        ensure_kind_exists(graph, &kind).await?;
        return Ok(WatchTarget {
            subject: WatchSubjectKey::Kind(kind),
            dependencies: Vec::new(),
        });
    };

    let mut node = node_id_from_kind_and_segment(kind, local_segment)?;
    ensure_node_exists(graph, &node).await?;

    let mut dependencies = Vec::new();
    while let Some(segment) = next_segment(&segments, &mut index) {
        let relation = relation_name_from_segment(segment)?;
        let targets = relation_targets(graph, &node, &relation).await?;
        let has_relation = !targets.is_empty();
        let property = property_key_from_segment(segment)?;
        let has_property = graph.property_spec(&node, &property).await.is_ok();

        if has_property && has_relation {
            return Err(errno(libc::EIO));
        }

        if has_property {
            if next_segment(&segments, &mut index).is_some() {
                return Err(errno(libc::ENOTDIR));
            }
            let key = property_key_from_segment(segment)?;
            return Ok(WatchTarget {
                subject: WatchSubjectKey::Property(node, key),
                dependencies,
            });
        }

        if !has_relation {
            return Err(errno(libc::ENOENT));
        }

        push_unique(
            &mut dependencies,
            WatchKey::Relation(node.clone(), relation.clone()),
        );

        let target = if targets.len() == 1 {
            targets[0].clone()
        } else {
            let Some(target_segment) = next_segment(&segments, &mut index) else {
                return Ok(WatchTarget {
                    subject: WatchSubjectKey::Node(node),
                    dependencies,
                });
            };
            relation_target_from_name(target_segment, &node, &targets)?
        };
        node = target;
    }

    Ok(WatchTarget {
        subject: WatchSubjectKey::Node(node),
        dependencies,
    })
}

fn next_segment<'a>(segments: &'a [&'a str], index: &mut usize) -> Option<&'a str> {
    let segment = segments.get(*index).copied();
    *index += usize::from(segment.is_some());
    segment
}

async fn ensure_kind_exists(
    graph: &DynamicGraph,
    kind: &NodeKind,
) -> std::result::Result<(), Errno> {
    if graph
        .node_kinds()
        .await
        .map_err(graph_error_to_errno)?
        .contains(kind)
    {
        Ok(())
    } else {
        Err(errno(libc::ENOENT))
    }
}

async fn ensure_node_exists(graph: &DynamicGraph, node: &NodeId) -> std::result::Result<(), Errno> {
    if graph
        .contains_node(node)
        .await
        .map_err(graph_error_to_errno)?
    {
        Ok(())
    } else {
        Err(errno(libc::ENOENT))
    }
}

async fn relation_targets(
    graph: &DynamicGraph,
    source: &NodeId,
    relation: &RelationName,
) -> std::result::Result<Vec<NodeId>, Errno> {
    match graph.targets(source, relation).await {
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
