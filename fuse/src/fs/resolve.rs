use fuse3::Errno;
use locusfs_graph::{
    DynamicGraph, GraphError, GraphPathDirectory, GraphPathEntry, GraphWatchTarget, NodeId,
    NodeKind, PathName, RelationName,
};

use super::name::{
    node_id_from_kind_and_segment, node_kind_from_segment, property_key_from_segment,
    relation_name_from_segment, relation_target_from_name,
};
use super::watch::{WatchKey, WatchMode, WatchState, WatchTarget, WatchValue, watch_subject_path};
use crate::layout::decode_segment;
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
    let directory_watch = path.ends_with('/');
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
            subject: GraphWatchTarget::Kind(kind),
            dependencies: Vec::new(),
            ready: true,
            mode: WatchMode::Changes,
        });
    };

    let mut node = node_id_from_kind_and_segment(kind, local_segment)?;
    ensure_node_exists(graph, &node).await?;

    let mut dependencies = Vec::new();
    while let Some(segment) = next_segment(&segments, &mut index) {
        if let Some(target) = resolve_path_provider_child(
            graph,
            GraphPathDirectory::Node(node.clone()),
            segment,
            &segments[index..],
            directory_watch,
        )
        .await?
        {
            return Ok(target);
        }
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
                subject: GraphWatchTarget::Property(node, key),
                dependencies,
                ready: true,
                mode: WatchMode::State,
            });
        }

        if !has_relation {
            push_unique(
                &mut dependencies,
                WatchKey::Relation(node.clone(), relation.clone()),
            );
            return Ok(WatchTarget {
                subject: GraphWatchTarget::NodeChild(
                    node,
                    decode_segment(segment).map_err(graph_error_to_errno)?,
                ),
                dependencies,
                ready: false,
                mode: if directory_watch {
                    WatchMode::Changes
                } else {
                    WatchMode::State
                },
            });
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
                    subject: GraphWatchTarget::Relation(node, relation),
                    dependencies,
                    ready: true,
                    mode: WatchMode::Changes,
                });
            };
            relation_target_from_name(target_segment, &node, &targets)?
        };
        node = target;
    }

    Ok(WatchTarget {
        subject: GraphWatchTarget::Node(node),
        dependencies,
        ready: true,
        mode: if directory_watch {
            WatchMode::Changes
        } else {
            WatchMode::State
        },
    })
}

async fn resolve_path_provider_child(
    graph: &DynamicGraph,
    directory: GraphPathDirectory,
    segment: &str,
    remaining: &[&str],
    directory_watch: bool,
) -> std::result::Result<Option<WatchTarget>, Errno> {
    let name = PathName::new(decode_segment(segment).map_err(graph_error_to_errno)?)
        .map_err(graph_error_to_errno)?;
    let Some(entry) = graph
        .lookup_path_child(&directory, &name)
        .await
        .map_err(graph_error_to_errno)?
    else {
        return Ok(None);
    };
    Ok(Some(
        resolve_path_provider_entry(graph, entry, remaining, directory_watch).await?,
    ))
}

async fn resolve_path_provider_entry(
    graph: &DynamicGraph,
    mut entry: GraphPathEntry,
    mut remaining: &[&str],
    directory_watch: bool,
) -> std::result::Result<WatchTarget, Errno> {
    loop {
        match entry {
            GraphPathEntry::Directory(GraphPathDirectory::Node(node)) => {
                return resolve_graph_node_path(graph, node, remaining, directory_watch).await;
            }
            GraphPathEntry::Directory(directory) => {
                if let Some((segment, rest)) = remaining.split_first() {
                    let name =
                        PathName::new(decode_segment(segment).map_err(graph_error_to_errno)?)
                            .map_err(graph_error_to_errno)?;
                    entry = graph
                        .lookup_path_child(&directory, &name)
                        .await
                        .map_err(graph_error_to_errno)?
                        .ok_or(errno(libc::ENOENT))?;
                    remaining = rest;
                    continue;
                }

                let subject = graph
                    .path_watch_target(&directory)
                    .await
                    .map_err(graph_error_to_errno)?
                    .ok_or(errno(libc::ENOENT))?;
                return Ok(WatchTarget {
                    subject,
                    dependencies: Vec::new(),
                    ready: true,
                    mode: if directory_watch {
                        WatchMode::Changes
                    } else {
                        WatchMode::State
                    },
                });
            }
            GraphPathEntry::Property { node, key } => {
                if !remaining.is_empty() {
                    return Err(errno(libc::ENOTDIR));
                }
                return Ok(WatchTarget {
                    subject: GraphWatchTarget::Property(node, key),
                    dependencies: Vec::new(),
                    ready: true,
                    mode: WatchMode::State,
                });
            }
            GraphPathEntry::Symlink { target } => {
                if !remaining.is_empty() {
                    return Err(errno(libc::ENOTDIR));
                }
                return Ok(WatchTarget {
                    subject: GraphWatchTarget::Node(target),
                    dependencies: Vec::new(),
                    ready: true,
                    mode: if directory_watch {
                        WatchMode::Changes
                    } else {
                        WatchMode::State
                    },
                });
            }
        }
    }
}

async fn resolve_graph_node_path(
    graph: &DynamicGraph,
    mut node: NodeId,
    mut remaining: &[&str],
    directory_watch: bool,
) -> std::result::Result<WatchTarget, Errno> {
    let mut dependencies = Vec::new();
    while let Some((segment, rest)) = remaining.split_first() {
        remaining = rest;
        let relation = relation_name_from_segment(segment)?;
        let targets = relation_targets(graph, &node, &relation).await?;
        let has_relation = !targets.is_empty();
        let property = property_key_from_segment(segment)?;
        let has_property = graph.property_spec(&node, &property).await.is_ok();

        if has_property && has_relation {
            return Err(errno(libc::EIO));
        }

        if has_property {
            if !remaining.is_empty() {
                return Err(errno(libc::ENOTDIR));
            }
            return Ok(WatchTarget {
                subject: GraphWatchTarget::Property(node, property),
                dependencies,
                ready: true,
                mode: WatchMode::State,
            });
        }

        if !has_relation {
            push_unique(
                &mut dependencies,
                WatchKey::Relation(node.clone(), relation.clone()),
            );
            return Ok(WatchTarget {
                subject: GraphWatchTarget::NodeChild(
                    node,
                    decode_segment(segment).map_err(graph_error_to_errno)?,
                ),
                dependencies,
                ready: false,
                mode: if directory_watch {
                    WatchMode::Changes
                } else {
                    WatchMode::State
                },
            });
        }

        push_unique(
            &mut dependencies,
            WatchKey::Relation(node.clone(), relation.clone()),
        );

        node = if targets.len() == 1 {
            targets[0].clone()
        } else {
            let Some((target_segment, rest)) = remaining.split_first() else {
                return Ok(WatchTarget {
                    subject: GraphWatchTarget::Relation(node, relation),
                    dependencies,
                    ready: true,
                    mode: WatchMode::Changes,
                });
            };
            remaining = rest;
            relation_target_from_name(target_segment, &node, &targets)?
        };
    }

    Ok(WatchTarget {
        subject: GraphWatchTarget::Node(node),
        dependencies,
        ready: true,
        mode: if directory_watch {
            WatchMode::Changes
        } else {
            WatchMode::State
        },
    })
}

pub(crate) async fn resolve_watch_state(
    graph: &DynamicGraph,
    path: &str,
) -> std::result::Result<(WatchTarget, WatchState), Errno> {
    let target = resolve_watch_path(graph, path).await?;
    let state = watch_state_for_target(graph, &target).await?;
    Ok((target, state))
}

pub(crate) async fn watch_state_for_target(
    graph: &DynamicGraph,
    target: &WatchTarget,
) -> std::result::Result<WatchState, Errno> {
    if !target.ready {
        return Ok(WatchState::Unset);
    }

    match &target.subject {
        GraphWatchTarget::Property(node, key) => match graph.property(node, key).await {
            Ok(value) => Ok(WatchState::Set(WatchValue::Property(
                value.display_string(),
            ))),
            Err(GraphError::NotFound { .. }) => Ok(WatchState::Unset),
            Err(error) => Err(graph_error_to_errno(error)),
        },
        GraphWatchTarget::Relation(source, relation) => {
            let targets = relation_targets(graph, source, relation).await?;
            match targets.as_slice() {
                [] => Ok(WatchState::Unset),
                [target] => Ok(WatchState::Set(WatchValue::Path(watch_subject_path(
                    &GraphWatchTarget::Node(target.clone()),
                )))),
                _ => Ok(WatchState::Set(WatchValue::Path(watch_subject_path(
                    &target.subject,
                )))),
            }
        }
        _ => Ok(WatchState::Set(WatchValue::Path(watch_subject_path(
            &target.subject,
        )))),
    }
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
