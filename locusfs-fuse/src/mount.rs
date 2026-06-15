use std::path::{Path, PathBuf};

use fuser::{BackgroundSession, Config, MountOption};
use locusfs_graph::DynamicGraph;

use crate::fs::LocusFs;
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
}

pub fn mount(config: FuseMountConfig, graph: DynamicGraph) -> Result<FuseMount> {
    let mut options = Config::default();
    options.mount_options = vec![
        MountOption::FSName("locusfs".to_string()),
        MountOption::Subtype("locusfs".to_string()),
        MountOption::RW,
        MountOption::NoSuid,
        MountOption::NoDev,
    ];

    let session = fuser::spawn_mount2(LocusFs::new(graph), config.mountpoint(), &options)
        .map_err(|error| FuseError::Mount(error.to_string()))?;

    Ok(FuseMount { _session: session })
}
