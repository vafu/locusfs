use fuser::Errno;
use locusfs_graph::{
    DynamicGraph, GraphError, LocusValue, NodeId, PropertyKey, PropertySpec, ValueKind,
};

use crate::graph_error_to_errno;

pub(super) fn slice_for_read(data: &[u8], offset: u64, size: u32) -> &[u8] {
    let offset = offset as usize;
    if offset >= data.len() {
        return &[];
    }
    let end = data.len().min(offset + size as usize);
    &data[offset..end]
}

pub(super) fn property_spec_or_new_string(
    graph: &DynamicGraph,
    node: &NodeId,
    key: &PropertyKey,
) -> std::result::Result<PropertySpec, Errno> {
    match graph.property_spec(node, key) {
        Ok(spec) => Ok(spec),
        Err(GraphError::NotFound { .. }) => {
            Ok(PropertySpec::read_write(key.clone(), ValueKind::String))
        }
        Err(error) => Err(graph_error_to_errno(error)),
    }
}

pub(super) fn property_perm(spec: &PropertySpec) -> u16 {
    match (spec.is_readable(), spec.is_writable()) {
        (true, true) => 0o644,
        (true, false) => 0o444,
        (false, true) => 0o222,
        (false, false) => 0o000,
    }
}

pub(super) fn parse_property_write(
    kind: ValueKind,
    input: &str,
) -> std::result::Result<LocusValue, GraphError> {
    let value = strip_single_trailing_newline(input);
    match kind {
        ValueKind::String => {
            if value.contains('\0') {
                return Err(GraphError::InvalidValue {
                    kind: "string",
                    value: value.to_string(),
                    reason: "contains NUL",
                });
            }
            Ok(LocusValue::String(value.to_string()))
        }
        ValueKind::Bool => parse_bool(value).map(LocusValue::Bool),
        ValueKind::U32 => {
            value
                .parse::<u32>()
                .map(LocusValue::U32)
                .map_err(|_| GraphError::InvalidValue {
                    kind: "u32",
                    value: value.to_string(),
                    reason: "expected unsigned integer",
                })
        }
        ValueKind::I32 => {
            value
                .parse::<i32>()
                .map(LocusValue::I32)
                .map_err(|_| GraphError::InvalidValue {
                    kind: "i32",
                    value: value.to_string(),
                    reason: "expected signed integer",
                })
        }
        ValueKind::F64 => {
            let number = value.parse::<f64>().map_err(|_| GraphError::InvalidValue {
                kind: "f64",
                value: value.to_string(),
                reason: "expected float",
            })?;
            if !number.is_finite() {
                return Err(GraphError::InvalidValue {
                    kind: "f64",
                    value: value.to_string(),
                    reason: "expected finite float",
                });
            }
            Ok(LocusValue::F64(number))
        }
    }
}

pub(super) fn property_file_string(value: &LocusValue) -> String {
    format!("{}\n", value.display_string())
}

fn parse_bool(value: &str) -> std::result::Result<bool, GraphError> {
    match value {
        "true" | "1" => Ok(true),
        "false" | "0" => Ok(false),
        _ => Err(GraphError::InvalidValue {
            kind: "bool",
            value: value.to_string(),
            reason: "expected true, false, 1, or 0",
        }),
    }
}

fn strip_single_trailing_newline(input: &str) -> &str {
    input
        .strip_suffix("\r\n")
        .or_else(|| input.strip_suffix('\n'))
        .unwrap_or(input)
}
