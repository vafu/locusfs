use std::fmt;
use std::str::FromStr;

use crate::{GraphError, Result};

use super::{NodeKind, validate_identifier};

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct NodeId {
    kind: NodeKind,
    local: String,
}

impl NodeId {
    pub fn new(kind: NodeKind, local: impl Into<String>) -> Result<Self> {
        let local = local.into();
        validate_identifier("node local id", &local)?;
        Ok(Self { kind, local })
    }

    pub fn parse(value: &str) -> Result<Self> {
        let Some((kind, local)) = value.split_once(':') else {
            return Err(GraphError::invalid_identifier(
                "node id",
                value,
                "expected <kind>:<local>",
            ));
        };
        Self::new(NodeKind::new(kind)?, local)
    }

    pub fn kind(&self) -> &NodeKind {
        &self.kind
    }

    pub fn local(&self) -> &str {
        &self.local
    }

    pub fn into_parts(self) -> (NodeKind, String) {
        (self.kind, self.local)
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}:{}", self.kind, self.local)
    }
}

impl FromStr for NodeId {
    type Err = GraphError;

    fn from_str(value: &str) -> Result<Self> {
        Self::parse(value)
    }
}
