use crate::{GraphError, Result};

pub(crate) fn validate_identifier(kind: &'static str, value: &str) -> Result<()> {
    if value.is_empty() {
        return Err(GraphError::invalid_identifier(kind, value, "empty"));
    }

    if value == "." || value == ".." {
        return Err(GraphError::invalid_identifier(
            kind,
            value,
            "reserved path segment",
        ));
    }

    if value.contains('\0') {
        return Err(GraphError::invalid_identifier(kind, value, "contains NUL"));
    }

    if kind == "node kind" && value.contains(':') {
        return Err(GraphError::invalid_identifier(
            kind,
            value,
            "contains node id separator",
        ));
    }

    Ok(())
}
