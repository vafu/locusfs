/// Scalar value kinds supported by graph properties.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValueKind {
    String,
    Bool,
    U32,
    I32,
    F64,
}
