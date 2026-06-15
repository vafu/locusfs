//! FUSE adapter boundary for Locus.
//!
//! This crate owns mount lifecycle and kernel filesystem request translation.
//! Graph semantics stay in `locusfs-core`.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::hash::{Hash, Hasher};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use fuser::{
    BackgroundSession, Config, Errno, FileAttr, FileHandle, FileType, Filesystem, FopenFlags,
    Generation, INodeNo, KernelConfig, LockOwner, MountOption, ReplyAttr, ReplyCreate, ReplyData,
    ReplyDirectory, ReplyEmpty, ReplyEntry, ReplyOpen, ReplyWrite, Request, TimeOrNow,
};
use locusfs_core::{
    GraphFilesystem, InMemoryGraph, LocusFsError, ProjectName, PropertyKey, decode_segment,
};
use thiserror::Error;

const ROOT_INO: u64 = 1;
const PROJECTS_INO: u64 = 2;
const NODES_INO: u64 = 3;
const PROJECT_DATA_INO: u64 = 4;
const TTL: Duration = Duration::from_millis(250);

pub type Result<T> = std::result::Result<T, FuseError>;

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

/// Errors from FUSE mount setup.
#[derive(Debug, Error)]
pub enum FuseError {
    #[error(transparent)]
    Core(#[from] LocusFsError),
    #[error("FUSE mount failed: {0}")]
    Mount(String),
}

/// A live FUSE session. Dropping this value unmounts the filesystem.
#[derive(Debug)]
pub struct FuseMount {
    _session: BackgroundSession,
}

pub fn mount(config: FuseMountConfig, graph: InMemoryGraph) -> Result<FuseMount> {
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

#[derive(Debug)]
pub struct LocusFs {
    graph: InMemoryGraph,
    inodes: Mutex<InodeTable>,
}

impl LocusFs {
    pub fn new(graph: InMemoryGraph) -> Self {
        Self {
            graph,
            inodes: Mutex::new(InodeTable::new()),
        }
    }

    fn lookup_entry(&self, parent: u64, name: &OsStr) -> std::result::Result<FsEntry, Errno> {
        let name = os_str_to_str(name)?;
        let parent = self.entry(parent)?;
        match parent {
            FsEntry::Root => match name {
                "projects" => Ok(FsEntry::ProjectsDir),
                "nodes" => Ok(FsEntry::NodesDir),
                ".projects" => Ok(FsEntry::ProjectDataDir),
                _ => Err(Errno::ENOENT),
            },
            FsEntry::ProjectsDir => {
                let project = project_name_from_segment(name)?;
                if self
                    .graph
                    .project(&project)
                    .map_err(core_error_to_errno)?
                    .is_some()
                {
                    Ok(FsEntry::ProjectLink(project))
                } else {
                    Err(Errno::ENOENT)
                }
            }
            FsEntry::ProjectDataDir => {
                let project = project_name_from_segment(name)?;
                if self
                    .graph
                    .project(&project)
                    .map_err(core_error_to_errno)?
                    .is_some()
                {
                    Ok(FsEntry::ProjectDir(project))
                } else {
                    Err(Errno::ENOENT)
                }
            }
            FsEntry::ProjectDir(project) => {
                let key = property_key_from_segment(name)?;
                if self
                    .graph
                    .project_property(&project, &key)
                    .map_err(core_error_to_errno)?
                    .is_some()
                    || project_virtual_property(&key)
                {
                    Ok(FsEntry::ProjectProperty(project, key))
                } else {
                    Err(Errno::ENOENT)
                }
            }
            FsEntry::NodesDir | FsEntry::ProjectLink(_) | FsEntry::ProjectProperty(_, _) => {
                Err(Errno::ENOTDIR)
            }
        }
    }

    fn entry(&self, ino: u64) -> std::result::Result<FsEntry, Errno> {
        self.inodes
            .lock()
            .map_err(|_| Errno::EIO)?
            .entry(ino)
            .ok_or(Errno::ENOENT)
    }

    fn inode(&self, entry: FsEntry) -> std::result::Result<u64, Errno> {
        self.inodes.lock().map_err(|_| Errno::EIO)?.inode(entry)
    }

    fn attr(&self, entry: &FsEntry, ino: u64) -> std::result::Result<FileAttr, Errno> {
        let (kind, perm, size) = match entry {
            FsEntry::Root
            | FsEntry::ProjectsDir
            | FsEntry::NodesDir
            | FsEntry::ProjectDataDir
            | FsEntry::ProjectDir(_) => (FileType::Directory, 0o755, 0),
            FsEntry::ProjectLink(project) => (
                FileType::Symlink,
                0o777,
                project_link_target(project).as_os_str().as_bytes().len() as u64,
            ),
            FsEntry::ProjectProperty(project, key) => {
                let value = self
                    .graph
                    .project_property(project, key)
                    .map_err(core_error_to_errno)?
                    .ok_or(Errno::ENOENT)?;
                (
                    FileType::RegularFile,
                    0o644,
                    value.to_file_string().len() as u64,
                )
            }
        };

        Ok(file_attr(ino, kind, perm, size))
    }

    fn read_project_property(
        &self,
        project: &ProjectName,
        key: &PropertyKey,
    ) -> std::result::Result<Vec<u8>, Errno> {
        let value = self
            .graph
            .project_property(project, key)
            .map_err(core_error_to_errno)?
            .ok_or(Errno::ENOENT)?;
        Ok(value.to_file_string().into_bytes())
    }

    fn create_project_property(
        &self,
        parent: u64,
        name: &OsStr,
    ) -> std::result::Result<FsEntry, Errno> {
        let FsEntry::ProjectDir(project) = self.entry(parent)? else {
            return Err(Errno::ENOTDIR);
        };
        let key = property_key_from_segment(os_str_to_str(name)?)?;
        if key.as_str() == "git-branch" || key.as_str() == "kind" {
            return Err(Errno::EROFS);
        }
        Ok(FsEntry::ProjectProperty(project, key))
    }
}

impl Filesystem for LocusFs {
    fn init(&mut self, _req: &Request, _config: &mut KernelConfig) -> io::Result<()> {
        Ok(())
    }

    fn lookup(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEntry) {
        match self.lookup_entry(parent.0, name).and_then(|entry| {
            let ino = self.inode(entry.clone())?;
            let attr = self.attr(&entry, ino)?;
            Ok(attr)
        }) {
            Ok(attr) => reply.entry(&TTL, &attr, Generation(0)),
            Err(error) => reply.error(error),
        }
    }

    fn getattr(&self, _req: &Request, ino: INodeNo, _fh: Option<FileHandle>, reply: ReplyAttr) {
        match self.entry(ino.0).and_then(|entry| self.attr(&entry, ino.0)) {
            Ok(attr) => reply.attr(&TTL, &attr),
            Err(error) => reply.error(error),
        }
    }

    fn opendir(&self, _req: &Request, ino: INodeNo, _flags: fuser::OpenFlags, reply: ReplyOpen) {
        match self.entry(ino.0) {
            Ok(
                FsEntry::Root
                | FsEntry::ProjectsDir
                | FsEntry::NodesDir
                | FsEntry::ProjectDataDir
                | FsEntry::ProjectDir(_),
            ) => reply.opened(FileHandle(0), FopenFlags::empty()),
            Ok(FsEntry::ProjectLink(_) | FsEntry::ProjectProperty(_, _)) => {
                reply.error(Errno::ENOTDIR)
            }
            Err(error) => reply.error(error),
        }
    }

    fn readdir(
        &self,
        _req: &Request,
        ino: INodeNo,
        _fh: FileHandle,
        offset: u64,
        mut reply: ReplyDirectory,
    ) {
        let result = self
            .entry(ino.0)
            .and_then(|entry| self.dir_entries(&entry, ino.0));
        match result {
            Ok(entries) => {
                for (index, entry) in entries.into_iter().enumerate().skip(offset as usize) {
                    if reply.add(
                        INodeNo(entry.ino),
                        (index + 1) as u64,
                        entry.kind,
                        entry.name,
                    ) {
                        break;
                    }
                }
                reply.ok();
            }
            Err(error) => reply.error(error),
        }
    }

    fn open(&self, _req: &Request, ino: INodeNo, _flags: fuser::OpenFlags, reply: ReplyOpen) {
        match self.entry(ino.0) {
            Ok(FsEntry::ProjectProperty(_, _)) => reply.opened(FileHandle(0), FopenFlags::empty()),
            Ok(FsEntry::ProjectLink(_)) => reply.error(Errno::EINVAL),
            Ok(_) => reply.error(Errno::EISDIR),
            Err(error) => reply.error(error),
        }
    }

    fn readlink(&self, _req: &Request, ino: INodeNo, reply: ReplyData) {
        match self.entry(ino.0) {
            Ok(FsEntry::ProjectLink(project)) => {
                reply.data(project_link_target(&project).as_os_str().as_bytes())
            }
            Ok(_) => reply.error(Errno::EINVAL),
            Err(error) => reply.error(error),
        }
    }

    fn read(
        &self,
        _req: &Request,
        ino: INodeNo,
        _fh: FileHandle,
        offset: u64,
        size: u32,
        _flags: fuser::OpenFlags,
        _lock_owner: Option<LockOwner>,
        reply: ReplyData,
    ) {
        let result = self.entry(ino.0).and_then(|entry| match entry {
            FsEntry::ProjectProperty(project, key) => self.read_project_property(&project, &key),
            _ => Err(Errno::EISDIR),
        });

        match result {
            Ok(data) => reply.data(slice_for_read(&data, offset, size)),
            Err(error) => reply.error(error),
        }
    }

    fn write(
        &self,
        _req: &Request,
        ino: INodeNo,
        _fh: FileHandle,
        offset: u64,
        data: &[u8],
        _write_flags: fuser::WriteFlags,
        _flags: fuser::OpenFlags,
        _lock_owner: Option<LockOwner>,
        reply: ReplyWrite,
    ) {
        if offset != 0 {
            reply.error(Errno::EINVAL);
            return;
        }

        let result = self.entry(ino.0).and_then(|entry| {
            let FsEntry::ProjectProperty(project, key) = entry else {
                return Err(Errno::EISDIR);
            };
            if key.as_str() == "git-branch" || key.as_str() == "kind" {
                return Err(Errno::EROFS);
            }
            let input = std::str::from_utf8(data).map_err(|_| Errno::EINVAL)?;
            self.graph
                .set_project_property(&project, &key, input)
                .map_err(core_error_to_errno)?;
            Ok(data.len() as u32)
        });

        match result {
            Ok(written) => reply.written(written),
            Err(error) => reply.error(error),
        }
    }

    fn create(
        &self,
        _req: &Request,
        parent: INodeNo,
        name: &OsStr,
        _mode: u32,
        _umask: u32,
        _flags: i32,
        reply: ReplyCreate,
    ) {
        match self
            .create_project_property(parent.0, name)
            .and_then(|entry| {
                let ino = self.inode(entry.clone())?;
                let attr = self
                    .attr(&entry, ino)
                    .unwrap_or_else(|_| file_attr(ino, FileType::RegularFile, 0o644, 0));
                Ok(attr)
            }) {
            Ok(attr) => reply.created(
                &TTL,
                &attr,
                Generation(0),
                FileHandle(0),
                FopenFlags::empty(),
            ),
            Err(error) => reply.error(error),
        }
    }

    fn setattr(
        &self,
        _req: &Request,
        ino: INodeNo,
        _mode: Option<u32>,
        _uid: Option<u32>,
        _gid: Option<u32>,
        size: Option<u64>,
        _atime: Option<TimeOrNow>,
        _mtime: Option<TimeOrNow>,
        _ctime: Option<SystemTime>,
        _fh: Option<FileHandle>,
        _crtime: Option<SystemTime>,
        _chgtime: Option<SystemTime>,
        _bkuptime: Option<SystemTime>,
        _flags: Option<fuser::BsdFileFlags>,
        reply: ReplyAttr,
    ) {
        if matches!(size, Some(0)) {
            match self.entry(ino.0).and_then(|entry| self.attr(&entry, ino.0)) {
                Ok(attr) => reply.attr(&TTL, &attr),
                Err(error) => reply.error(error),
            }
        } else {
            reply.error(Errno::ENOSYS);
        }
    }

    fn symlink(
        &self,
        _req: &Request,
        parent: INodeNo,
        link_name: &OsStr,
        target: &Path,
        reply: ReplyEntry,
    ) {
        let result = self.entry(parent.0).and_then(|entry| {
            if entry != FsEntry::ProjectsDir {
                return Err(Errno::ENOTDIR);
            }
            let project = project_name_from_segment(os_str_to_str(link_name)?)?;
            self.graph
                .upsert_project_link(&project, target)
                .map_err(core_error_to_errno)?;
            let entry = FsEntry::ProjectLink(project);
            let ino = self.inode(entry.clone())?;
            self.attr(&entry, ino)
        });

        match result {
            Ok(attr) => reply.entry(&TTL, &attr, Generation(0)),
            Err(error) => reply.error(error),
        }
    }

    fn unlink(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        let result = self.entry(parent.0).and_then(|entry| match entry {
            FsEntry::ProjectsDir => {
                let project = project_name_from_segment(os_str_to_str(name)?)?;
                self.graph
                    .remove_project(&project)
                    .map_err(core_error_to_errno)
            }
            FsEntry::ProjectDir(project) => {
                let key = property_key_from_segment(os_str_to_str(name)?)?;
                if key.as_str() == "git-branch" || key.as_str() == "kind" {
                    return Err(Errno::EROFS);
                }
                let node = locusfs_core::NodeId::new(format!("project:{}", project.as_str()))
                    .map_err(core_error_to_errno)?;
                self.graph
                    .remove_property(&node, &key)
                    .map_err(core_error_to_errno)
            }
            _ => Err(Errno::ENOTDIR),
        });

        match result {
            Ok(()) => reply.ok(),
            Err(error) => reply.error(error),
        }
    }
}

impl LocusFs {
    fn dir_entries(&self, entry: &FsEntry, ino: u64) -> std::result::Result<Vec<DirEntry>, Errno> {
        let mut entries = vec![
            DirEntry::new(ino, FileType::Directory, "."),
            DirEntry::new(parent_inode(entry), FileType::Directory, ".."),
        ];

        match entry {
            FsEntry::Root => {
                entries.push(DirEntry::new(PROJECTS_INO, FileType::Directory, "projects"));
                entries.push(DirEntry::new(NODES_INO, FileType::Directory, "nodes"));
            }
            FsEntry::ProjectsDir => {
                for project in self.graph.projects().map_err(core_error_to_errno)? {
                    let child = FsEntry::ProjectLink(project.name.clone());
                    let child_ino = self.inode(child)?;
                    entries.push(DirEntry::new(
                        child_ino,
                        FileType::Symlink,
                        project.name.into_string(),
                    ));
                }
            }
            FsEntry::ProjectDir(project) => {
                for name in ["kind", "path", "name", "git-branch"] {
                    let key = PropertyKey::new(name).map_err(core_error_to_errno)?;
                    let child = FsEntry::ProjectProperty(project.clone(), key);
                    let child_ino = self.inode(child)?;
                    entries.push(DirEntry::new(child_ino, FileType::RegularFile, name));
                }
            }
            FsEntry::ProjectDataDir
            | FsEntry::NodesDir
            | FsEntry::ProjectLink(_)
            | FsEntry::ProjectProperty(_, _) => {}
        }

        Ok(entries)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum FsEntry {
    Root,
    ProjectsDir,
    NodesDir,
    ProjectDataDir,
    ProjectLink(ProjectName),
    ProjectDir(ProjectName),
    ProjectProperty(ProjectName, PropertyKey),
}

impl Hash for FsEntry {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            Self::Root => 0_u8.hash(state),
            Self::ProjectsDir => 1_u8.hash(state),
            Self::NodesDir => 2_u8.hash(state),
            Self::ProjectDataDir => 3_u8.hash(state),
            Self::ProjectLink(project) => {
                4_u8.hash(state);
                project.hash(state);
            }
            Self::ProjectDir(project) => {
                5_u8.hash(state);
                project.hash(state);
            }
            Self::ProjectProperty(project, key) => {
                6_u8.hash(state);
                project.hash(state);
                key.hash(state);
            }
        }
    }
}

#[derive(Debug)]
struct InodeTable {
    next: u64,
    by_ino: HashMap<u64, FsEntry>,
    by_entry: HashMap<FsEntry, u64>,
}

impl InodeTable {
    fn new() -> Self {
        let mut table = Self {
            next: 5,
            by_ino: HashMap::new(),
            by_entry: HashMap::new(),
        };
        table.insert(ROOT_INO, FsEntry::Root);
        table.insert(PROJECTS_INO, FsEntry::ProjectsDir);
        table.insert(NODES_INO, FsEntry::NodesDir);
        table.insert(PROJECT_DATA_INO, FsEntry::ProjectDataDir);
        table
    }

    fn entry(&self, ino: u64) -> Option<FsEntry> {
        self.by_ino.get(&ino).cloned()
    }

    fn inode(&mut self, entry: FsEntry) -> std::result::Result<u64, Errno> {
        if let Some(ino) = self.by_entry.get(&entry) {
            return Ok(*ino);
        }
        let ino = self.next;
        self.next = self.next.checked_add(1).ok_or(Errno::EOVERFLOW)?;
        self.insert(ino, entry);
        Ok(ino)
    }

    fn insert(&mut self, ino: u64, entry: FsEntry) {
        self.by_entry.insert(entry.clone(), ino);
        self.by_ino.insert(ino, entry);
    }
}

#[derive(Debug)]
struct DirEntry {
    ino: u64,
    kind: FileType,
    name: String,
}

impl DirEntry {
    fn new(ino: u64, kind: FileType, name: impl Into<String>) -> Self {
        Self {
            ino,
            kind,
            name: name.into(),
        }
    }
}

fn file_attr(ino: u64, kind: FileType, perm: u16, size: u64) -> FileAttr {
    let now = SystemTime::now();
    FileAttr {
        ino: INodeNo(ino),
        size,
        blocks: size.div_ceil(512),
        atime: now,
        mtime: now,
        ctime: now,
        crtime: now,
        kind,
        perm,
        nlink: 1,
        uid: unsafe { libc::getuid() },
        gid: unsafe { libc::getgid() },
        rdev: 0,
        blksize: 4096,
        flags: 0,
    }
}

fn parent_inode(entry: &FsEntry) -> u64 {
    match entry {
        FsEntry::Root => ROOT_INO,
        FsEntry::ProjectsDir | FsEntry::NodesDir | FsEntry::ProjectDataDir => ROOT_INO,
        FsEntry::ProjectLink(_) => PROJECTS_INO,
        FsEntry::ProjectDir(_) => PROJECT_DATA_INO,
        FsEntry::ProjectProperty(_, _) => PROJECT_DATA_INO,
    }
}

fn project_link_target(project: &ProjectName) -> PathBuf {
    PathBuf::from("../.projects").join(project.as_str())
}

fn os_str_to_str(value: &OsStr) -> std::result::Result<&str, Errno> {
    value.to_str().ok_or(Errno::EINVAL)
}

fn project_name_from_segment(segment: &str) -> std::result::Result<ProjectName, Errno> {
    ProjectName::new(decode_segment(segment).map_err(core_error_to_errno)?)
        .map_err(core_error_to_errno)
}

fn property_key_from_segment(segment: &str) -> std::result::Result<PropertyKey, Errno> {
    PropertyKey::new(decode_segment(segment).map_err(core_error_to_errno)?)
        .map_err(core_error_to_errno)
}

fn project_virtual_property(key: &PropertyKey) -> bool {
    matches!(key.as_str(), "kind" | "path" | "name" | "git-branch")
}

fn slice_for_read(data: &[u8], offset: u64, size: u32) -> &[u8] {
    let offset = offset as usize;
    if offset >= data.len() {
        return &[];
    }
    let end = data.len().min(offset + size as usize);
    &data[offset..end]
}

fn core_error_to_errno(error: LocusFsError) -> Errno {
    match error {
        LocusFsError::NotFound { .. } => Errno::ENOENT,
        LocusFsError::InvalidIdentifier { .. }
        | LocusFsError::InvalidPathSegment { .. }
        | LocusFsError::InvalidEncoding { .. }
        | LocusFsError::InvalidValue { .. } => Errno::EINVAL,
        LocusFsError::Unsupported { .. } => Errno::ENOSYS,
        LocusFsError::Io(_) => Errno::EIO,
    }
}

#[cfg(test)]
mod test;
