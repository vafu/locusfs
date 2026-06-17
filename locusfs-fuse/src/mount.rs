use std::path::{Path, PathBuf};
use std::thread::JoinHandle;

use fuser::{BackgroundSession, Config, MountOption};
use locusfs_graph::DynamicGraph;

use crate::fs::{InodeTable, LocusFs, WatchRegistry};
use crate::invalidation::spawn_change_invalidator;
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
    _session: BackgroundSession,
    _change_worker: JoinHandle<()>,
}

pub fn mount(config: FuseMountConfig, graph: DynamicGraph) -> Result<FuseMount> {
    let changes = graph.subscribe_changes()?;
    let invalidation_graph = graph.clone();
    let inodes = InodeTable::shared();
    let watch = WatchRegistry::shared();

    let mut options = Config::default();
    options.mount_options = vec![
        MountOption::FSName("locusfs".to_string()),
        MountOption::Subtype("locusfs".to_string()),
        MountOption::RW,
        MountOption::NoSuid,
        MountOption::NoDev,
    ];

    let session = fuser::spawn_mount2(
        LocusFs::new_with_state(graph, inodes.clone(), watch.clone()),
        config.mountpoint(),
        &options,
    )
    .map_err(|error| FuseError::Mount(error.to_string()))?;
    let change_worker = spawn_change_invalidator(
        changes,
        session.notifier(),
        invalidation_graph,
        inodes,
        watch,
    );

    Ok(FuseMount {
        _session: session,
        _change_worker: change_worker,
    })
}
