//! Shared plugin ABI surface for `locusfs`.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use async_trait::async_trait;
use locusfs_graph::{DynamicGraph, Result};
use tokio::runtime::Handle;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PluginManifest {
    pub id: &'static str,
    pub name: &'static str,
    pub version: &'static str,
}

#[derive(Clone, Debug)]
pub struct PluginContext {
    pub graph: DynamicGraph,
    pub runtime: Handle,
}

impl PluginContext {
    pub fn new(graph: DynamicGraph) -> Self {
        Self {
            graph,
            runtime: Handle::current(),
        }
    }
}

#[async_trait]
pub trait LocusFsPlugin: Send + Sync {
    fn manifest(&self) -> PluginManifest;

    fn default_config(&self) -> toml::Value {
        toml::Value::Table(toml::map::Map::new())
    }

    async fn register(
        &self,
        context: PluginContext,
        config: toml::Value,
    ) -> Result<Box<dyn PluginHandle>>;
}

#[async_trait]
pub trait PluginHandle: Send + Sync {
    async fn shutdown(self: Box<Self>) {}
}

pub fn enter_runtime<F>(runtime: Handle, future: F) -> RuntimeEntered<F> {
    RuntimeEntered {
        runtime,
        future: Some(future),
    }
}

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
