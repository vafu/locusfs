//! Shared plugin ABI surface for `locusfs`.
//!
//! This crate is the Rust extension contract between `locusfs-bin` and plugins
//! built with the same workspace/toolchain. It is not a stable cross-compiler
//! binary ABI: plugin dynamic libraries currently exchange Rust trait objects.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use async_trait::async_trait;
use locusfs_graph::{DynamicGraph, GraphError, Result};
use tokio::runtime::Handle;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Static metadata used by the host to identify and validate a plugin.
pub struct PluginManifest {
    /// Stable configuration and loading identifier.
    pub id: &'static str,
    /// Human-readable plugin name.
    pub name: &'static str,
    /// Plugin version reported for diagnostics.
    pub version: &'static str,
}

#[derive(Clone, Debug)]
/// Host capabilities passed to a plugin during registration.
///
/// Plugins receive the dynamic graph they can register providers with and the
/// Tokio runtime handle they should use for long-lived async work.
pub struct PluginContext {
    /// Host graph registry and mutation surface.
    pub graph: DynamicGraph,
    /// Tokio runtime handle owned by the host.
    pub runtime: Handle,
}

impl PluginContext {
    /// Creates a plugin context from the current Tokio runtime.
    ///
    /// Panics when called outside a Tokio runtime. Prefer [`Self::try_new`] at
    /// host/plugin boundaries where runtime availability is not guaranteed.
    pub fn new(graph: DynamicGraph) -> Self {
        Self::from_runtime(graph, Handle::current())
    }

    /// Creates a plugin context from the current Tokio runtime, returning an
    /// explicit graph error when no runtime is active.
    pub fn try_new(graph: DynamicGraph) -> Result<Self> {
        let runtime = Handle::try_current().map_err(|_| GraphError::Internal {
            reason: "plugin context requires a Tokio runtime",
        })?;
        Ok(Self::from_runtime(graph, runtime))
    }

    /// Creates a plugin context from an explicit runtime handle.
    pub fn from_runtime(graph: DynamicGraph, runtime: Handle) -> Self {
        Self { graph, runtime }
    }
}

#[async_trait]
/// Plugin implementation loaded by the host.
///
/// The host calls [`Self::manifest`] before registration, merges
/// [`Self::default_config`] with user config, then calls [`Self::register`] and
/// retains the returned handle until shutdown.
pub trait LocusFsPlugin: Send + Sync {
    /// Returns static plugin metadata.
    fn manifest(&self) -> PluginManifest;

    /// Returns plugin defaults as raw TOML for host-side config merging.
    fn default_config(&self) -> toml::Value {
        toml::Value::Table(toml::map::Map::new())
    }

    /// Registers plugin providers and starts any plugin-owned runtime work.
    async fn register(
        &self,
        context: PluginContext,
        config: toml::Value,
    ) -> Result<Box<dyn PluginHandle>>;
}

#[async_trait]
/// Host-retained plugin lifetime handle.
///
/// The handle keeps plugin runtime state alive until the host calls
/// [`Self::shutdown`] during unmount.
pub trait PluginHandle: Send + Sync {
    /// Stops plugin runtime work and releases resources.
    async fn shutdown(self: Box<Self>) {}
}

/// Wraps a future so every poll and drop happens inside the provided runtime.
pub fn enter_runtime<F>(runtime: Handle, future: F) -> RuntimeEntered<F> {
    RuntimeEntered {
        runtime,
        future: Some(future),
    }
}

/// Future returned by [`enter_runtime`].
pub struct RuntimeEntered<F> {
    runtime: Handle,
    future: Option<F>,
}

impl<F> Future for RuntimeEntered<F>
where
    F: Future,
{
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = unsafe { self.get_unchecked_mut() };
        let _guard = this.runtime.enter();
        let future = this
            .future
            .as_mut()
            .expect("runtime-entered future polled after completion");
        let future = unsafe { Pin::new_unchecked(future) };
        match future.poll(cx) {
            Poll::Ready(output) => {
                this.future = None;
                Poll::Ready(output)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<F> Drop for RuntimeEntered<F> {
    fn drop(&mut self) {
        if self.future.is_none() {
            return;
        }
        let _guard = self.runtime.enter();
        let future = self.future.take();
        drop(future);
    }
}
