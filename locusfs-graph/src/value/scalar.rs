use std::fmt;

use super::ValueKind;

/// Typed graph value.
#[derive(Clone, Debug, PartialEq)]
pub enum LocusValue {
    String(String),
    Bool(bool),
    U32(u32),
    I32(i32),
    F64(f64),
}

impl LocusValue {
    pub fn kind(&self) -> ValueKind {
        match self {
            Self::String(_) => ValueKind::String,
            Self::Bool(_) => ValueKind::Bool,
            Self::U32(_) => ValueKind::U32,
            Self::I32(_) => ValueKind::I32,
            Self::F64(_) => ValueKind::F64,
        }
    }

    pub fn display_string(&self) -> String {
        match self {
            Self::String(value) => value.clone(),
            Self::Bool(value) => value.to_string(),
            Self::U32(value) => value.to_string(),
            Self::I32(value) => value.to_string(),
            Self::F64(value) => value.to_string(),
        }
    }
}

impl fmt::Display for LocusValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.display_string())
    }
}
