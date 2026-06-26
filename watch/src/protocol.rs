use std::fmt;
use std::io;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WatchEvent {
    State(WatchState),
    Change(WatchChange),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WatchState {
    Unset,
    Set(WatchValue),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WatchValue {
    Path(String),
    Property(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WatchChange {
    Change,
    Node {
        action: WatchAction,
        node: String,
    },
    Property {
        action: WatchAction,
        node: Option<String>,
        key: String,
    },
    Relation {
        action: WatchAction,
        node: Option<String>,
        relation: String,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WatchAction {
    Added,
    Changed,
    Removed,
}

impl WatchEvent {
    pub fn encode_text(&self) -> Vec<u8> {
        match self {
            Self::State(state) => state.encode_text(),
            Self::Change(change) => change.encode_text(),
        }
    }

    pub fn decode_text(bytes: &[u8]) -> io::Result<Self> {
        let text = std::str::from_utf8(bytes).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("watch event is not valid UTF-8: {error}"),
            )
        })?;
        Self::parse_text(text)
    }

    pub fn parse_text(text: &str) -> io::Result<Self> {
        let text = text.trim();
        if text == "unset" {
            return Ok(Self::State(WatchState::Unset));
        }
        if text == "set" {
            return Ok(Self::State(WatchState::Set(WatchValue::Property(
                String::new(),
            ))));
        }
        if let Some(value) = text.strip_prefix("set ") {
            let value = if value.starts_with('/') {
                WatchValue::Path(value.to_string())
            } else {
                WatchValue::Property(value.to_string())
            };
            return Ok(Self::State(WatchState::Set(value)));
        }
        WatchChange::parse_text(text).map(Self::Change)
    }
}

impl WatchState {
    pub fn encode_text(&self) -> Vec<u8> {
        match self {
            Self::Unset => b"unset\n".to_vec(),
            Self::Set(value) => format!("set {}\n", value.payload()).into_bytes(),
        }
    }
}

impl WatchValue {
    pub fn payload(&self) -> &str {
        match self {
            Self::Path(value) | Self::Property(value) => value,
        }
    }
}

impl WatchChange {
    pub fn encode_text(&self) -> Vec<u8> {
        match self {
            Self::Change => b"change\n".to_vec(),
            Self::Node { action, node } => format!("node {action} {node}\n").into_bytes(),
            Self::Property { action, node, key } => match node {
                Some(node) => format!("property {action} {node} {key}\n").into_bytes(),
                None => format!("property {action} {key}\n").into_bytes(),
            },
            Self::Relation {
                action,
                node,
                relation,
            } => match node {
                Some(node) => format!("relation {action} {node} {relation}\n").into_bytes(),
                None => format!("relation {action} {relation}\n").into_bytes(),
            },
        }
    }

    pub fn parse_text(text: &str) -> io::Result<Self> {
        let parts = text.split_whitespace().collect::<Vec<_>>();
        match parts.as_slice() {
            ["change"] => Ok(Self::Change),
            ["node", action, node] => Ok(Self::Node {
                action: WatchAction::parse(action)?,
                node: (*node).to_string(),
            }),
            ["property", action, key] => Ok(Self::Property {
                action: WatchAction::parse(action)?,
                node: None,
                key: (*key).to_string(),
            }),
            ["property", action, node, key] => Ok(Self::Property {
                action: WatchAction::parse(action)?,
                node: Some((*node).to_string()),
                key: (*key).to_string(),
            }),
            ["relation", action, relation] => Ok(Self::Relation {
                action: WatchAction::parse(action)?,
                node: None,
                relation: (*relation).to_string(),
            }),
            ["relation", action, node, relation] => Ok(Self::Relation {
                action: WatchAction::parse(action)?,
                node: Some((*node).to_string()),
                relation: (*relation).to_string(),
            }),
            _ => Err(invalid_event(text)),
        }
    }
}

impl WatchAction {
    fn parse(value: &str) -> io::Result<Self> {
        match value {
            "added" => Ok(Self::Added),
            "changed" => Ok(Self::Changed),
            "removed" => Ok(Self::Removed),
            _ => Err(invalid_event(value)),
        }
    }
}

impl fmt::Display for WatchAction {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Added => formatter.write_str("added"),
            Self::Changed => formatter.write_str("changed"),
            Self::Removed => formatter.write_str("removed"),
        }
    }
}

fn invalid_event(value: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("invalid watch event: {value}"),
    )
}

#[cfg(test)]
mod test;
