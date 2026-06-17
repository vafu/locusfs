//! Reusable graph contract for Locus.
//!
//! `locusfs-graph` owns graph identity, typed values, property metadata,
//! provider contracts, routing, and typed errors.

mod error;
pub mod graph;
pub mod identity;
pub mod value;

pub use error::{GraphError, Result};
pub use graph::{
    DynamicGraph, GraphChange, GraphChangeReceiver, GraphChangeStreamError,
    GraphChangeSubscription, InMemoryProvider, NodeMutationProvider, NodeProvider,
    PropertyMutationProvider, PropertyProvider, RelationMutationProvider, RelationProvider,
    TracedProvider,
};
pub use identity::{NodeId, NodeKind, PathName, PropertyKey, RelationName};
pub use value::{LocusValue, PropertySpec, ValueKind};
