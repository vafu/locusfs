mod kind;
mod property;
mod scalar;

pub use kind::ValueKind;
pub use property::PropertySpec;
pub use scalar::LocusValue;

#[cfg(test)]
mod test;
