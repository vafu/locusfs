use std::fmt;

use crate::{LocusFsError, Result};

/// Scalar value kinds supported by the first filesystem contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValueKind {
    String,
    Bool,
    U32,
    I32,
    F64,
}

/// Typed graph value with shell-oriented file serialization.
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

    pub fn parse_shell(kind: ValueKind, input: &str) -> Result<Self> {
        let value = strip_single_trailing_newline(input);
        match kind {
            ValueKind::String => {
                if value.contains('\0') {
                    return Err(LocusFsError::invalid_value("string", value, "contains NUL"));
                }
                Ok(Self::String(value.to_string()))
            }
            ValueKind::Bool => parse_bool(value).map(Self::Bool),
            ValueKind::U32 => value.parse::<u32>().map(Self::U32).map_err(|_| {
                LocusFsError::invalid_value("u32", value, "expected unsigned integer")
            }),
            ValueKind::I32 => value
                .parse::<i32>()
                .map(Self::I32)
                .map_err(|_| LocusFsError::invalid_value("i32", value, "expected signed integer")),
            ValueKind::F64 => {
                let number = value
                    .parse::<f64>()
                    .map_err(|_| LocusFsError::invalid_value("f64", value, "expected float"))?;
                if !number.is_finite() {
                    return Err(LocusFsError::invalid_value(
                        "f64",
                        value,
                        "expected finite float",
                    ));
                }
                Ok(Self::F64(number))
            }
        }
    }

    pub fn to_shell_string(&self) -> String {
        match self {
            Self::String(value) => value.clone(),
            Self::Bool(value) => value.to_string(),
            Self::U32(value) => value.to_string(),
            Self::I32(value) => value.to_string(),
            Self::F64(value) => value.to_string(),
        }
    }

    pub fn to_file_string(&self) -> String {
        format!("{}\n", self.to_shell_string())
    }
}

impl fmt::Display for LocusValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.to_shell_string())
    }
}

fn parse_bool(value: &str) -> Result<bool> {
    match value {
        "true" | "1" => Ok(true),
        "false" | "0" => Ok(false),
        _ => Err(LocusFsError::invalid_value(
            "bool",
            value,
            "expected true, false, 1, or 0",
        )),
    }
}

fn strip_single_trailing_newline(input: &str) -> &str {
    input
        .strip_suffix("\r\n")
        .or_else(|| input.strip_suffix('\n'))
        .unwrap_or(input)
}

#[cfg(test)]
mod test;
