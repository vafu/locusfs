//! Reusable graph contract for Locus.
//!
//! `locusfs-graph` owns graph identity, typed values, property metadata,
//! provider contracts, routing, and typed errors.
//!
//! Feature flags:
//!
//! - `dynamic` enables `DynamicGraph` and graph change subscriptions.
//! - `watch-provider` enables target-scoped provider watch contracts.
//! - `in-memory` enables `InMemoryProvider`, a concrete writable provider.
//! - `provider-tracing` enables `TracedProvider`, a provider instrumentation
//!   wrapper.
//!
//! With `default-features = false`, this crate exposes only the stable graph
//! contracts, identity types, values, and errors.

mod error;
pub mod graph;
pub mod identity;
pub mod value;

pub use error::{GraphError, Result};
#[cfg(feature = "in-memory")]
pub use graph::InMemoryProvider;
#[cfg(feature = "provider-tracing")]
pub use graph::TracedProvider;
#[cfg(feature = "dynamic")]
pub use graph::{
    DynamicGraph, GraphChangeReceiver, GraphChangeStreamError, GraphChangeSubscription,
};
pub use graph::{
    GraphChange, NodeAccess, NodeMutationProvider, NodeProvider, PropertyMutationProvider,
    PropertyProvider, RelationMutationProvider, RelationProvider,
};
#[cfg(feature = "watch-provider")]
pub use graph::{GraphWatch, GraphWatchEvent, GraphWatchTarget, WatchProvider};
pub use identity::{NodeId, NodeKind, PathName, PropertyKey, RelationName};
pub use value::{LocusValue, PropertySpec, ValueKind};

/// Common graph contract imports for provider and graph consumers.
///
/// The prelude intentionally contains the everyday trait and value vocabulary,
/// not optional concrete implementations such as `DynamicGraph` or
/// `InMemoryProvider`.
pub mod prelude {
    pub use crate::{
        GraphChange, GraphError, LocusValue, NodeAccess, NodeId, NodeKind, NodeMutationProvider,
        NodeProvider, PathName, PropertyKey, PropertyMutationProvider, PropertyProvider,
        PropertySpec, RelationMutationProvider, RelationName, RelationProvider, Result, ValueKind,
    };
}
