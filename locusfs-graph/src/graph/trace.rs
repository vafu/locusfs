use std::time::Instant;

use async_trait::async_trait;
use tracing::trace;

use crate::{
    LocusValue, NodeAccess, NodeId, NodeKind, NodeMutationProvider, NodeProvider, PropertyKey,
    PropertyMutationProvider, PropertyProvider, PropertySpec, RelationMutationProvider,
    RelationName, RelationProvider, Result,
};

#[derive(Clone, Debug)]
pub struct TracedProvider<P> {
    label: &'static str,
    inner: P,
}

impl<P> TracedProvider<P> {
    pub fn new(label: &'static str, inner: P) -> Self {
        Self { label, inner }
    }

    pub fn into_inner(self) -> P {
        self.inner
    }
}

#[async_trait]
impl<P> NodeProvider for TracedProvider<P>
where
    P: NodeProvider,
{
    fn kind(&self) -> &NodeKind {
        self.inner.kind()
    }

    fn access(&self) -> NodeAccess {
        self.inner.access()
    }

    async fn contains_node(&self, node: &NodeId) -> Result<bool> {
        let started = Instant::now();
        let result = self.inner.contains_node(node).await;
        trace!(
            target: "locusfs_graph::provider",
            provider = self.label,
            operation = "contains_node",
            ?node,
            elapsed_us = started.elapsed().as_micros(),
            ok = result.is_ok(),
        );
        result
    }

    async fn nodes(&self) -> Result<Vec<NodeId>> {
        let started = Instant::now();
        let result = self.inner.nodes().await;
        trace!(
            target: "locusfs_graph::provider",
            provider = self.label,
            operation = "nodes",
            kind = %self.kind(),
            elapsed_us = started.elapsed().as_micros(),
            ok = result.is_ok(),
        );
        result
    }
}

#[async_trait]
impl<P> NodeMutationProvider for TracedProvider<P>
where
    P: NodeMutationProvider,
{
    async fn create_node(&self, node: &NodeId) -> Result<()> {
        let started = Instant::now();
        let result = self.inner.create_node(node).await;
        trace_provider_node_result(self.label, "create_node", node, started, &result);
        result
    }

    async fn remove_node(&self, node: &NodeId) -> Result<()> {
        let started = Instant::now();
        let result = self.inner.remove_node(node).await;
        trace_provider_node_result(self.label, "remove_node", node, started, &result);
        result
    }
}

#[async_trait]
impl<P> PropertyProvider for TracedProvider<P>
where
    P: PropertyProvider,
{
    async fn property_spec(&self, subject: &NodeId, key: &PropertyKey) -> Result<PropertySpec> {
        let started = Instant::now();
        let result = self.inner.property_spec(subject, key).await;
        trace_property_result(self.label, "property_spec", subject, key, started, &result);
        result
    }

    async fn properties(&self, subject: &NodeId) -> Result<Vec<PropertySpec>> {
        let started = Instant::now();
        let result = self.inner.properties(subject).await;
        trace!(
            target: "locusfs_graph::provider",
            provider = self.label,
            operation = "properties",
            node = ?subject,
            elapsed_us = started.elapsed().as_micros(),
            ok = result.is_ok(),
        );
        result
    }

    async fn property(&self, subject: &NodeId, key: &PropertyKey) -> Result<LocusValue> {
        let started = Instant::now();
        let result = self.inner.property(subject, key).await;
        trace_property_result(self.label, "property", subject, key, started, &result);
        result
    }
}

#[async_trait]
impl<P> PropertyMutationProvider for TracedProvider<P>
where
    P: PropertyMutationProvider,
{
    async fn set_property(
        &self,
        subject: &NodeId,
        key: &PropertyKey,
        value: LocusValue,
    ) -> Result<()> {
        let started = Instant::now();
        let result = self.inner.set_property(subject, key, value).await;
        trace_property_result(self.label, "set_property", subject, key, started, &result);
        result
    }

    async fn remove_property(&self, subject: &NodeId, key: &PropertyKey) -> Result<()> {
        let started = Instant::now();
        let result = self.inner.remove_property(subject, key).await;
        trace_property_result(
            self.label,
            "remove_property",
            subject,
            key,
            started,
            &result,
        );
        result
    }
}

#[async_trait]
impl<P> RelationProvider for TracedProvider<P>
where
    P: RelationProvider,
{
    async fn relations(&self, source: &NodeId) -> Result<Vec<RelationName>> {
        let started = Instant::now();
        let result = self.inner.relations(source).await;
        trace!(
            target: "locusfs_graph::provider",
            provider = self.label,
            operation = "relations",
            node = ?source,
            elapsed_us = started.elapsed().as_micros(),
            ok = result.is_ok(),
        );
        result
    }

    async fn targets(&self, source: &NodeId, relation: &RelationName) -> Result<Vec<NodeId>> {
        let started = Instant::now();
        let result = self.inner.targets(source, relation).await;
        trace_relation_result(self.label, "targets", source, relation, started, &result);
        result
    }
}

#[async_trait]
impl<P> RelationMutationProvider for TracedProvider<P>
where
    P: RelationMutationProvider,
{
    async fn set_link(
        &self,
        source: &NodeId,
        relation: &RelationName,
        target: &NodeId,
    ) -> Result<()> {
        let started = Instant::now();
        let result = self.inner.set_link(source, relation, target).await;
        trace_relation_result(self.label, "set_link", source, relation, started, &result);
        result
    }

    async fn remove_link(
        &self,
        source: &NodeId,
        relation: &RelationName,
        target: &NodeId,
    ) -> Result<()> {
        let started = Instant::now();
        let result = self.inner.remove_link(source, relation, target).await;
        trace_relation_result(
            self.label,
            "remove_link",
            source,
            relation,
            started,
            &result,
        );
        result
    }
}

fn trace_provider_node_result<T>(
    provider: &'static str,
    operation: &'static str,
    node: &NodeId,
    started: Instant,
    result: &Result<T>,
) {
    trace!(
        target: "locusfs_graph::provider",
        provider,
        operation,
        ?node,
        elapsed_us = started.elapsed().as_micros(),
        ok = result.is_ok(),
    );
}

fn trace_property_result<T>(
    provider: &'static str,
    operation: &'static str,
    node: &NodeId,
    key: &PropertyKey,
    started: Instant,
    result: &Result<T>,
) {
    trace!(
        target: "locusfs_graph::provider",
        provider,
        operation,
        ?node,
        ?key,
        elapsed_us = started.elapsed().as_micros(),
        ok = result.is_ok(),
    );
}

fn trace_relation_result<T>(
    provider: &'static str,
    operation: &'static str,
    source: &NodeId,
    relation: &RelationName,
    started: Instant,
    result: &Result<T>,
) {
    trace!(
        target: "locusfs_graph::provider",
        provider,
        operation,
        ?source,
        ?relation,
        elapsed_us = started.elapsed().as_micros(),
        ok = result.is_ok(),
    );
}
