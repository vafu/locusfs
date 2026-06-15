use std::fmt;
use std::str::FromStr;

use crate::{LocusFsError, Result};

macro_rules! identity_type {
    ($type_name:ident, $kind:literal) => {
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $type_name(String);

        impl $type_name {
            pub fn new(value: impl Into<String>) -> Result<Self> {
                let value = value.into();
                validate_identifier($kind, &value)?;
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }

            pub fn into_string(self) -> String {
                self.0
            }
        }

        impl fmt::Display for $type_name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }

        impl FromStr for $type_name {
            type Err = LocusFsError;

            fn from_str(value: &str) -> Result<Self> {
                Self::new(value)
            }
        }
    };
}

identity_type!(NodeId, "node id");
identity_type!(NodeKind, "node kind");
identity_type!(PathName, "path name");
identity_type!(ProjectName, "project name");
identity_type!(PropertyKey, "property key");
identity_type!(RelationName, "relation name");

pub(crate) fn validate_identifier(kind: &'static str, value: &str) -> Result<()> {
    if value.is_empty() {
        return Err(LocusFsError::invalid_identifier(kind, value, "empty"));
    }

    if value == "." || value == ".." {
        return Err(LocusFsError::invalid_identifier(
            kind,
            value,
            "reserved path segment",
        ));
    }

    if value.contains('\0') {
        return Err(LocusFsError::invalid_identifier(
            kind,
            value,
            "contains NUL",
        ));
    }

    Ok(())
}

#[cfg(test)]
mod test;
