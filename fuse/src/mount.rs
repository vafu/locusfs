use std::path::{Path, PathBuf};
use std::sync::Arc;

use fuse3::MountOptions;
use fuse3::raw::{MountHandle, Session};
use locusfs_graph::DynamicGraph;
use tokio::sync::Mutex;

use crate::fs::{
    InodeTable, LocusFs, SharedInodeTable, SharedKernelNotify, SharedWatchRegistry, WatchRegistry,
};
use crate::invalidation::{InvalidationWorker, resync_known_state, spawn_change_invalidator};
use crate::{FuseError, Result};

/// Configuration for serving a Locus graph through a FUSE mount.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FuseMountConfig {
    mountpoint: PathBuf,
}

impl FuseMountConfig {
    pub fn new(mountpoint: impl Into<PathBuf>) -> Self {
        Self {
            mountpoint: mountpoint.into(),
        }
    }

    pub fn mountpoint(&self) -> &Path {
        &self.mountpoint
    }
}

/// A live FUSE session. Dropping this value unmounts the filesystem.
#[derive(Debug)]
pub struct FuseMount {
    change_worker: InvalidationWorker,
    graph: DynamicGraph,
    inodes: SharedInodeTable,
    watch: SharedWatchRegistry,
    notify: SharedKernelNotify,
    session: Option<MountHandle>,
}

impl FuseMount {
    /// Invalidates known kernel state and wakes active `/watch` waiters.
    pub async fn resync_known_state(&self) {
        resync_known_state(
            self.notify.clone(),
            self.graph.clone(),
            self.inodes.clone(),
            self.watch.clone(),
        )
        .await;
    }

    pub async fn unmount(mut self) -> Result<()> {
        self.change_worker.shutdown();
        let Some(session) = self.session.take() else {
            return Ok(());
        };
        session
            .unmount()
            .await
            .map_err(|error| FuseError::Unmount(error.to_string()))
    }
}

pub async fn mount(config: FuseMountConfig, graph: DynamicGraph) -> Result<FuseMount> {
    let changes = graph.subscribe_global_changes();
    let invalidation_graph = graph.clone();
    let inodes = InodeTable::shared();
    let watch = WatchRegistry::shared();
    let notify: SharedKernelNotify = Arc::new(Mutex::new(None));

    let mut options = MountOptions::default();
    options.fs_name("locusfs");
    options.custom_options("subtype=locusfs");

    let session = Session::<LocusFs>::new(options)
        .mount_with_unprivileged(
            LocusFs::new_with_state(graph.clone(), inodes.clone(), watch.clone(), notify.clone()),
            config.mountpoint(),
        )
        .await
        .map_err(|error| FuseError::Mount(error.to_string()))?;
    let change_worker = spawn_change_invalidator(
        changes,
        notify.clone(),
        invalidation_graph,
        inodes.clone(),
        watch.clone(),
    );

    Ok(FuseMount {
        change_worker,
        graph,
        inodes,
        watch,
        notify,
        session: Some(session),
    })
}
