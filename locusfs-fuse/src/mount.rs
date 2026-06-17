use std::path::{Path, PathBuf};
use std::sync::Arc;

use fuse3::MountOptions;
use fuse3::raw::{MountHandle, Session};
use locusfs_graph::DynamicGraph;
use tokio::sync::Mutex;

use crate::fs::{InodeTable, LocusFs, SharedKernelNotify, WatchRegistry};
use crate::invalidation::{InvalidationWorker, spawn_change_invalidator};
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
    _change_worker: InvalidationWorker,
    _session: MountHandle,
}

pub async fn mount(config: FuseMountConfig, graph: DynamicGraph) -> Result<FuseMount> {
    let changes = graph.subscribe_changes();
    let invalidation_graph = graph.clone();
    let inodes = InodeTable::shared();
    let watch = WatchRegistry::shared();
    let notify: SharedKernelNotify = Arc::new(Mutex::new(None));

    let mut options = MountOptions::default();
    options.fs_name("locusfs");
    options.custom_options("subtype=locusfs");

    let session = Session::<LocusFs>::new(options)
        .mount_with_unprivileged(
            LocusFs::new_with_state(graph, inodes.clone(), watch.clone(), notify.clone()),
            config.mountpoint(),
        )
        .await
        .map_err(|error| FuseError::Mount(error.to_string()))?;
    let change_worker =
        spawn_change_invalidator(changes, notify, invalidation_graph, inodes, watch);

    Ok(FuseMount {
        _change_worker: change_worker,
        _session: session,
    })
}
