use std::ffi::OsStr;
use std::path::{Component, Path};

use fuse3::Errno;
use locusfs_graph::{NodeId, NodeKind, PropertyKey, RelationName};

use crate::layout::{decode_segment, encode_segment};
use crate::{errno, graph_error_to_errno};

pub(super) fn os_str_to_str(value: &OsStr) -> std::result::Result<&str, Errno> {
    value.to_str().ok_or(errno(libc::EINVAL))
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

pub(super) fn encode_relation_target_name(
    source: &NodeId,
    targets: &[NodeId],
    target: &NodeId,
) -> locusfs_graph::Result<String> {
    encode_segment(&relation_target_display_name(source, targets, target))
}

pub(super) fn decode_relation_target_name(name: &str) -> std::result::Result<String, Errno> {
    decode_segment(name).map_err(graph_error_to_errno)
}

pub(super) fn relation_target_from_name(
    name: &str,
    source: &NodeId,
    targets: &[NodeId],
) -> std::result::Result<NodeId, Errno> {
    let decoded = decode_relation_target_name(name)?;
    if let Some(target) = targets
        .iter()
        .find(|target| relation_target_display_name(source, targets, target) == decoded)
    {
        return Ok(target.clone());
    }

    let target = NodeId::parse(&decoded).map_err(graph_error_to_errno)?;
    if targets.contains(&target) {
        Ok(target)
    } else {
        Err(errno(libc::ENOENT))
    }
}

pub(super) fn node_id_from_relation_link_target_path(
    path: &Path,
) -> std::result::Result<NodeId, Errno> {
    let mut components = path.components();
    while matches!(components.clone().next(), Some(Component::ParentDir)) {
        components.next();
    }

    let Some(Component::Normal(kind)) = components.next() else {
        return Err(errno(libc::EINVAL));
    };
    let kind = node_kind_from_segment(os_str_to_str(kind)?)?;

    let Some(Component::Normal(local)) = components.next() else {
        return Err(errno(libc::EINVAL));
    };
    let target = node_id_from_kind_and_segment(kind, os_str_to_str(local)?)?;

    if components.next().is_some() {
        return Err(errno(libc::EINVAL));
    }

    Ok(target)
}

fn relation_target_display_name(source: &NodeId, targets: &[NodeId], target: &NodeId) -> String {
    let stripped = service_stripped_local(source, target);
    let basename = stripped.rsplit('/').next().unwrap_or(stripped);
    if basename != stripped && is_unique_display(source, targets, basename, target) {
        return basename.to_string();
    }
    if is_unique_display(source, targets, stripped, target) {
        return stripped.to_string();
    }
    if is_unique_display(source, targets, target.local(), target) {
        return target.local().to_string();
    }
    target.to_string()
}

fn service_stripped_local<'a>(source: &NodeId, target: &'a NodeId) -> &'a str {
    target
        .local()
        .strip_prefix(source.local())
        .and_then(|local| local.strip_prefix(':'))
        .unwrap_or(target.local())
}

fn is_unique_display(source: &NodeId, targets: &[NodeId], name: &str, target: &NodeId) -> bool {
    targets
        .iter()
        .filter(|candidate| {
            let stripped = service_stripped_local(source, candidate);
            candidate.local() == name
                || stripped == name
                || stripped.rsplit('/').next().unwrap_or(stripped) == name
        })
        .all(|candidate| candidate == target)
}
