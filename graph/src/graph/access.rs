#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NodeAccess {
    readable: bool,
    writable: bool,
}

impl NodeAccess {
    pub const fn new(readable: bool, writable: bool) -> Self {
        Self { readable, writable }
    }

    pub const fn read_only() -> Self {
        Self::new(true, false)
    }

    pub const fn read_write() -> Self {
        Self::new(true, true)
    }

    pub const fn hidden() -> Self {
        Self::new(false, false)
    }

    pub const fn is_readable(self) -> bool {
        self.readable
    }

    pub const fn is_writable(self) -> bool {
        self.writable
    }
}
