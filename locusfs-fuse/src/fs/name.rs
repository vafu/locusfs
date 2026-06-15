use std::ffi::OsStr;
use std::path::{Component, Path};

use fuser::Errno;
use locusfs_graph::{NodeId, NodeKind, PropertyKey, RelationName};

use crate::graph_error_to_errno;
use crate::layout::decode_segment;

pub(super) fn os_str_to_str(value: &OsStr) -> std::result::Result<&str, Errno> {
    value.to_str().ok_or(Errno::EINVAL)
}

pub(super) fn node_kind_from_segment(segment: &str) -> std::result::Result<NodeKind, Errno> {
    NodeKind::new(decode_segment(segment).map_err(graph_error_to_errno)?)
        .map_err(graph_error_to_errno)
}

pub(super) fn node_id_from_kind_and_segment(
    kind: NodeKind,
    segment: &str,
) -> std::result::Result<NodeId, Errno> {
    NodeId::new(kind, decode_segment(segment).map_err(graph_error_to_errno)?)
        .map_err(graph_error_to_errno)
}

pub(super) fn property_key_from_segment(segment: &str) -> std::result::Result<PropertyKey, Errno> {
    PropertyKey::new(decode_segment(segment).map_err(graph_error_to_errno)?)
        .map_err(graph_error_to_errno)
}

pub(super) fn relation_name_from_segment(
    segment: &str,
) -> std::result::Result<RelationName, Errno> {
    RelationName::new(decode_segment(segment).map_err(graph_error_to_errno)?)
        .map_err(graph_error_to_errno)
}

pub(super) fn node_id_from_relation_link_target_path(
    path: &Path,
) -> std::result::Result<NodeId, Errno> {
    let mut components = path.components();
    for _ in 0..5 {
        match components.next() {
            Some(Component::ParentDir) => {}
            _ => return Err(Errno::EINVAL),
        }
    }

    let Some(Component::Normal(kind)) = components.next() else {
        return Err(Errno::EINVAL);
    };
    let kind = node_kind_from_segment(os_str_to_str(kind)?)?;

    let Some(Component::Normal(local)) = components.next() else {
        return Err(Errno::EINVAL);
    };
    let target = node_id_from_kind_and_segment(kind, os_str_to_str(local)?)?;

    if components.next().is_some() {
        return Err(Errno::EINVAL);
    }

    Ok(target)
}
