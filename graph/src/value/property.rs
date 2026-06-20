use crate::PropertyKey;

use super::ValueKind;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PropertySpec {
    key: PropertyKey,
    kind: ValueKind,
    readable: bool,
    writable: bool,
}

impl PropertySpec {
    pub fn new(key: PropertyKey, kind: ValueKind) -> Self {
        Self {
            key,
            kind,
            readable: true,
            writable: false,
        }
    }

    pub fn readable(mut self, readable: bool) -> Self {
        self.readable = readable;
        self
    }

    pub fn writable(mut self, writable: bool) -> Self {
        self.writable = writable;
        self
    }

    pub fn read_write(key: PropertyKey, kind: ValueKind) -> Self {
        Self::new(key, kind).writable(true)
    }

    pub fn write_only(key: PropertyKey, kind: ValueKind) -> Self {
        Self::new(key, kind).readable(false).writable(true)
    }

    pub fn key(&self) -> &PropertyKey {
        &self.key
    }

    pub fn kind(&self) -> ValueKind {
        self.kind
    }

    pub fn is_readable(&self) -> bool {
        self.readable
    }

    pub fn is_writable(&self) -> bool {
        self.writable
    }

    pub fn into_key(self) -> PropertyKey {
        self.key
    }
}
