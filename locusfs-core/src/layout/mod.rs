use std::path::PathBuf;

use crate::{
    LocusFsError, NodeId, ProjectName, PropertyKey, RelationName, Result,
    identity::validate_identifier,
};

/// FUSE-independent path builder for the public filesystem contract.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Layout;

impl Layout {
    pub fn nodes_dir() -> PathBuf {
        PathBuf::from("nodes")
    }

    pub fn node_dir(node: &NodeId) -> Result<PathBuf> {
        Ok(Self::nodes_dir().join(encode_segment(node.as_str())?))
    }

    pub fn node_kind(node: &NodeId) -> Result<PathBuf> {
        Ok(Self::node_dir(node)?.join("kind"))
    }

    pub fn node_props_dir(node: &NodeId) -> Result<PathBuf> {
        Ok(Self::node_dir(node)?.join("props"))
    }

    pub fn node_property(node: &NodeId, key: &PropertyKey) -> Result<PathBuf> {
        Ok(Self::node_props_dir(node)?.join(encode_segment(key.as_str())?))
    }

    pub fn node_out_dir(node: &NodeId) -> Result<PathBuf> {
        Ok(Self::node_dir(node)?.join("out"))
    }

    pub fn node_relation_dir(node: &NodeId, relation: &RelationName) -> Result<PathBuf> {
        Ok(Self::node_out_dir(node)?.join(encode_segment(relation.as_str())?))
    }

    pub fn node_relation_link(
        node: &NodeId,
        relation: &RelationName,
        target: &NodeId,
    ) -> Result<PathBuf> {
        Ok(Self::node_relation_dir(node, relation)?.join(encode_segment(target.as_str())?))
    }

    pub fn projects_dir() -> PathBuf {
        PathBuf::from("projects")
    }

    pub fn project_entry(project: &ProjectName) -> Result<PathBuf> {
        Ok(Self::projects_dir().join(encode_segment(project.as_str())?))
    }

    pub fn project_property(project: &ProjectName, key: &PropertyKey) -> Result<PathBuf> {
        Ok(Self::project_entry(project)?.join(encode_segment(key.as_str())?))
    }
}

pub fn encode_segment(raw: &str) -> Result<String> {
    validate_identifier("path segment", raw)?;
    let mut encoded = String::new();
    for byte in raw.bytes() {
        if is_plain_segment_byte(byte) {
            encoded.push(byte as char);
        } else {
            encoded.push('%');
            encoded.push(hex_digit(byte >> 4));
            encoded.push(hex_digit(byte & 0x0f));
        }
    }
    Ok(encoded)
}

pub fn decode_segment(encoded: &str) -> Result<String> {
    validate_identifier("path segment", encoded)?;
    let bytes = encoded.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                return Err(LocusFsError::InvalidEncoding {
                    segment: encoded.to_string(),
                });
            }
            let high =
                hex_value(bytes[index + 1]).ok_or_else(|| LocusFsError::InvalidEncoding {
                    segment: encoded.to_string(),
                })?;
            let low = hex_value(bytes[index + 2]).ok_or_else(|| LocusFsError::InvalidEncoding {
                segment: encoded.to_string(),
            })?;
            decoded.push((high << 4) | low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }

    let value = String::from_utf8(decoded).map_err(|_| LocusFsError::InvalidEncoding {
        segment: encoded.to_string(),
    })?;
    validate_identifier("path segment", &value)?;
    Ok(value)
}

fn is_plain_segment_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-')
}

fn hex_digit(value: u8) -> char {
    match value {
        0..=9 => (b'0' + value) as char,
        10..=15 => (b'A' + value - 10) as char,
        _ => unreachable!("nibble is always <= 15"),
    }
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod test;
