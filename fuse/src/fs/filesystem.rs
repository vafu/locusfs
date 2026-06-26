use std::ffi::OsStr;
use std::future::Future;
use std::num::NonZeroU32;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::sync::Arc;
use std::vec::IntoIter;

use bytes::Bytes;
use fuse3::notify::Notify;
use fuse3::raw::Filesystem;
use fuse3::raw::Request;
use fuse3::raw::reply::{
    DirectoryEntry, DirectoryEntryPlus, FileAttr, ReplyAttr, ReplyCreated, ReplyData,
    ReplyDirectory, ReplyDirectoryPlus, ReplyEntry, ReplyInit, ReplyOpen, ReplyPoll, ReplyWrite,
};
use fuse3::{Errno, FileType, Inode, SetAttr};
use futures_util::stream::{self, Iter};
use locusfs_graph::{
    DynamicGraph, GraphError, GraphPathDirectory, GraphPathEntry, GraphWatch, InMemoryProvider,
    NodeId, NodeKind, PathName, PropertyKey, RelationName,
};
use tokio::sync::Mutex;

use super::attr::{EntryTimes, TTL, file_attr};
use super::entry::{
    FsEntry, WATCH_FILE_NAME, direct_relation_link_target, nested_relation_link_target,
};
use super::inode::{InodeTable, SharedInodeTable};
use super::name::{
    node_id_from_kind_and_segment, node_id_from_relation_link_target_path, node_kind_from_segment,
    os_str_to_str, property_key_from_segment, relation_name_from_segment,
    relation_target_from_name,
};
use super::resolve::{parse_watch_subscription, resolve_watch_path};
use super::value::{
    node_dir_perm, parse_property_write, property_file_string, property_perm,
    property_spec_or_new_string, slice_for_read,
};
use super::watch::{FileHandle, SharedWatchRegistry, WatchMode, WatchRegistry};
use crate::layout::decode_segment;
use crate::{errno, graph_error_to_errno};

const FOPEN_DIRECT_IO: u32 = 1;

type VecDirEntryStream = Iter<IntoIter<fuse3::Result<DirectoryEntry>>>;
type VecDirEntryPlusStream = Iter<IntoIter<fuse3::Result<DirectoryEntryPlus>>>;
pub(crate) type SharedKernelNotify = Arc<Mutex<Option<Notify>>>;

/// FUSE request adapter over the generic graph filesystem.
#[derive(Clone, Debug)]
pub struct LocusFs {
    pub(super) graph: DynamicGraph,
    inodes: SharedInodeTable,
    watch: SharedWatchRegistry,
    notify: SharedKernelNotify,
}

impl LocusFs {
    pub fn new(graph: DynamicGraph) -> Self {
        Self::new_with_state(
            graph,
            InodeTable::shared(),
            WatchRegistry::shared(),
            Arc::new(Mutex::new(None)),
        )
    }

    pub(crate) fn new_with_state(
        graph: DynamicGraph,
        inodes: SharedInodeTable,
        watch: SharedWatchRegistry,
        notify: SharedKernelNotify,
    ) -> Self {
        Self {
            graph,
            inodes,
            watch,
            notify,
        }
    }

    async fn lookup_entry(&self, parent: u64, name: &OsStr) -> std::result::Result<FsEntry, Errno> {
        let name = os_str_to_str(name)?;
        let parent = self.entry(parent).await?;
        match parent.clone() {
            FsEntry::Root => {
                if name == WATCH_FILE_NAME {
                    return Ok(FsEntry::WatchFile);
                }
                let kind = node_kind_from_segment(name)?;
                if self
                    .graph
                    .kind_access(&kind)
                    .await
                    .map_err(graph_error_to_errno)
                    .is_ok_and(|access| !access.is_readable())
                {
                    return Err(errno(libc::ENOENT));
                }
                if self
                    .graph
                    .node_kinds()
                    .await
                    .map_err(graph_error_to_errno)?
                    .contains(&kind)
                {
                    Ok(FsEntry::KindDir(kind))
                } else {
                    Err(errno(libc::ENOENT))
                }
            }
            FsEntry::KindDir(kind) => {
                let node = node_id_from_kind_and_segment(kind, name)?;
                if self
                    .graph
                    .contains_node(&node)
                    .await
                    .map_err(graph_error_to_errno)?
                {
                    Ok(FsEntry::NodeDir(node))
                } else {
                    Err(errno(libc::ENOENT))
                }
            }
            FsEntry::NodeDir(node) => {
                self.lookup_node_child(&node, name, FsEntry::NodeDir(node.clone()))
                    .await
            }
            FsEntry::RelationDir(source, relation) => {
                let targets = self
                    .graph
                    .targets(&source, &relation)
                    .await
                    .map_err(graph_error_to_errno)?;
                let target = relation_target_from_name(name, &source, &targets)?;
                Ok(FsEntry::RelationTargetLink {
                    source,
                    relation,
                    target,
                })
            }
            FsEntry::PathDir { directory, .. } => {
                if let Some(entry) = self
                    .lookup_path_child(&directory, name, parent.clone())
                    .await?
                {
                    return Ok(entry);
                }
                match directory {
                    GraphPathDirectory::Node(node) => {
                        self.lookup_graph_node_child(&node, name).await
                    }
                    GraphPathDirectory::Virtual { .. } => Err(errno(libc::ENOENT)),
                }
            }
            FsEntry::WatchFile
            | FsEntry::PropertyFile(_, _)
            | FsEntry::RelationLink { .. }
            | FsEntry::RelationTargetLink { .. }
            | FsEntry::PathLink { .. } => Err(errno(libc::ENOTDIR)),
        }
    }

    async fn lookup_path_child(
        &self,
        directory: &GraphPathDirectory,
        name: &str,
        parent: FsEntry,
    ) -> std::result::Result<Option<FsEntry>, Errno> {
        let name = PathName::new(decode_segment(name).map_err(graph_error_to_errno)?)
            .map_err(graph_error_to_errno)?;
        let Some(entry) = self
            .graph
            .lookup_path_child(directory, &name)
            .await
            .map_err(graph_error_to_errno)?
        else {
            return Ok(None);
        };
        Ok(Some(self.path_entry(entry, parent)))
    }

    async fn lookup_node_child(
        &self,
        node: &NodeId,
        name: &str,
        parent: FsEntry,
    ) -> std::result::Result<FsEntry, Errno> {
        if let Some(entry) = self
            .lookup_path_child(&GraphPathDirectory::Node(node.clone()), name, parent)
            .await?
        {
            return Ok(entry);
        }
        self.lookup_graph_node_child(node, name).await
    }

    async fn lookup_graph_node_child(
        &self,
        node: &NodeId,
        name: &str,
    ) -> std::result::Result<FsEntry, Errno> {
        let property = property_key_from_segment(name)?;
        let has_property = self.graph.property_spec(node, &property).await.is_ok();
        let relation = relation_name_from_segment(name)?;
        let targets = self.relation_targets(node, &relation).await?;
        let has_relation = !targets.is_empty();

        if has_property && has_relation {
            return Err(errno(libc::EIO));
        }
        if has_property {
            return Ok(FsEntry::PropertyFile(node.clone(), property));
        }

        match targets.as_slice() {
            [] => Err(errno(libc::ENOENT)),
            [target] => Ok(FsEntry::RelationLink {
                source: node.clone(),
                relation,
                target: target.clone(),
            }),
            _ => Ok(FsEntry::RelationDir(node.clone(), relation)),
        }
    }

    fn path_entry(&self, entry: GraphPathEntry, parent: FsEntry) -> FsEntry {
        match entry {
            GraphPathEntry::Directory(directory) => FsEntry::PathDir {
                directory,
                parent: Box::new(parent),
            },
            GraphPathEntry::Property { node, key } => FsEntry::PropertyFile(node, key),
            GraphPathEntry::Symlink { target } => FsEntry::PathLink {
                target,
                parent: Box::new(parent),
            },
        }
    }

    async fn entry(&self, ino: u64) -> std::result::Result<FsEntry, Errno> {
        self.inodes
            .lock()
            .await
            .entry(ino)
            .ok_or(errno(libc::ENOENT))
    }

    pub(super) async fn inode(&self, entry: FsEntry) -> std::result::Result<u64, Errno> {
        self.inodes.lock().await.inode(entry)
    }

    async fn acquire_inode(&self, entry: FsEntry) -> std::result::Result<u64, Errno> {
        self.inodes.lock().await.acquire(entry)
    }

    async fn forget_inode(&self, ino: u64, nlookup: u64) {
        self.inodes.lock().await.forget(ino, nlookup);
    }

    async fn forget_entry(&self, entry: &FsEntry) {
        self.inodes.lock().await.forget_entry(entry);
    }

    async fn attr(&self, entry: &FsEntry, ino: u64) -> std::result::Result<FileAttr, Errno> {
        let (kind, perm, size) = match entry {
            FsEntry::Root => (FileType::Directory, 0o755, 0),
            FsEntry::WatchFile => (FileType::RegularFile, 0o600, 4096),
            FsEntry::KindDir(kind) => {
                if !self
                    .graph
                    .node_kinds()
                    .await
                    .map_err(graph_error_to_errno)?
                    .contains(kind)
                {
                    return Err(errno(libc::ENOENT));
                }
                let access = self
                    .graph
                    .kind_access(kind)
                    .await
                    .map_err(graph_error_to_errno)?;
                (FileType::Directory, node_dir_perm(access), 0)
            }
            FsEntry::NodeDir(node) => {
                self.ensure_node_exists(node).await?;
                let access = self
                    .graph
                    .node_access(node)
                    .await
                    .map_err(graph_error_to_errno)?;
                (FileType::Directory, node_dir_perm(access), 0)
            }
            FsEntry::RelationDir(node, _) => {
                self.ensure_node_exists(node).await?;
                let access = self
                    .graph
                    .node_access(node)
                    .await
                    .map_err(graph_error_to_errno)?;
                (FileType::Directory, node_dir_perm(access), 0)
            }
            FsEntry::PathDir { .. } => (FileType::Directory, 0o555, 0),
            FsEntry::PropertyFile(node, key) => {
                let spec = self
                    .graph
                    .property_spec(node, key)
                    .await
                    .map_err(graph_error_to_errno)?;
                let size = if spec.is_readable() {
                    let value = self
                        .graph
                        .property(node, key)
                        .await
                        .map_err(graph_error_to_errno)?;
                    property_file_string(&value).len() as u64
                } else {
                    0
                };
                (FileType::RegularFile, property_perm(&spec), size)
            }
            FsEntry::RelationLink {
                source,
                relation,
                target,
            } => {
                self.ensure_relation_link_exists(source, relation, target)
                    .await?;
                (
                    FileType::Symlink,
                    0o777,
                    direct_relation_link_target(target)
                        .as_os_str()
                        .as_bytes()
                        .len() as u64,
                )
            }
            FsEntry::RelationTargetLink {
                source,
                relation,
                target,
            } => {
                self.ensure_relation_link_exists(source, relation, target)
                    .await?;
                (
                    FileType::Symlink,
                    0o777,
                    nested_relation_link_target(target)
                        .as_os_str()
                        .as_bytes()
                        .len() as u64,
                )
            }
            FsEntry::PathLink { target, .. } => (
                FileType::Symlink,
                0o777,
                direct_relation_link_target(target)
                    .as_os_str()
                    .as_bytes()
                    .len() as u64,
            ),
        };

        Ok(file_attr(
            ino,
            kind,
            perm,
            size,
            self.entry_times(entry).await?,
        ))
    }

    async fn entry_times(&self, entry: &FsEntry) -> std::result::Result<EntryTimes, Errno> {
        Ok(self.inodes.lock().await.times(entry))
    }

    async fn touch_entry(&self, entry: &FsEntry) {
        self.inodes.lock().await.touch(entry);
    }

    async fn create_property_file(
        &self,
        parent: u64,
        name: &OsStr,
    ) -> std::result::Result<FsEntry, Errno> {
        let FsEntry::NodeDir(node) = self.entry(parent).await? else {
            return Err(errno(libc::ENOTDIR));
        };
        self.ensure_node_writable(&node).await?;
        let key = property_key_from_segment(os_str_to_str(name)?)?;
        Ok(FsEntry::PropertyFile(node, key))
    }

    pub(super) async fn create_kind_dir(
        &self,
        name: &OsStr,
    ) -> std::result::Result<FsEntry, Errno> {
        let kind = node_kind_from_segment(os_str_to_str(name)?)?;
        if self
            .graph
            .node_kinds()
            .await
            .map_err(graph_error_to_errno)?
            .contains(&kind)
        {
            return Err(errno(libc::EEXIST));
        }

        let provider = InMemoryProvider::new(kind.clone());
        self.graph
            .register_read_write_provider(provider)
            .await
            .map_err(graph_error_to_errno)?;
        Ok(FsEntry::KindDir(kind))
    }

    async fn read_property(
        &self,
        node: &NodeId,
        key: &PropertyKey,
    ) -> std::result::Result<Vec<u8>, Errno> {
        let spec = self
            .graph
            .property_spec(node, key)
            .await
            .map_err(graph_error_to_errno)?;
        if spec.is_readable() {
            let value = self
                .graph
                .property(node, key)
                .await
                .map_err(graph_error_to_errno)?;
            Ok(property_file_string(&value).into_bytes())
        } else {
            Err(errno(libc::EACCES))
        }
    }

    async fn entry_reply(&self, entry: FsEntry) -> std::result::Result<ReplyEntry, Errno> {
        let ino = self.acquire_inode(entry.clone()).await?;
        let attr = self.attr(&entry, ino).await?;
        Ok(ReplyEntry {
            ttl: TTL,
            attr,
            generation: 0,
        })
    }

    async fn attr_reply(&self, ino: u64) -> std::result::Result<ReplyAttr, Errno> {
        let entry = self.entry(ino).await?;
        Ok(ReplyAttr {
            ttl: TTL,
            attr: self.attr(&entry, ino).await?,
        })
    }
}

async fn wake_poll_handles(notify: SharedKernelNotify, handles: Vec<u64>) {
    for handle in handles {
        if let Some(notifier) = notify.lock().await.clone() {
            notifier.wakeup(handle).await;
        }
    }
}

impl Filesystem for LocusFs {
    async fn init(&self, _req: Request) -> fuse3::Result<ReplyInit> {
        Ok(ReplyInit {
            max_write: NonZeroU32::new(128 * 1024).expect("nonzero max write"),
        })
    }

    async fn destroy(&self, _req: Request) {}

    async fn lookup(
        &self,
        _req: Request,
        parent: Inode,
        name: &OsStr,
    ) -> fuse3::Result<ReplyEntry> {
        let entry = self.lookup_entry(parent, name).await?;
        self.entry_reply(entry).await
    }

    async fn forget(&self, _req: Request, inode: Inode, nlookup: u64) {
        self.forget_inode(inode, nlookup).await;
    }

    async fn getattr(
        &self,
        _req: Request,
        inode: Inode,
        _fh: Option<u64>,
        _flags: u32,
    ) -> fuse3::Result<ReplyAttr> {
        self.attr_reply(inode).await
    }

    async fn opendir(&self, _req: Request, inode: Inode, _flags: u32) -> fuse3::Result<ReplyOpen> {
        match self.entry(inode).await? {
            FsEntry::Root
            | FsEntry::KindDir(_)
            | FsEntry::NodeDir(_)
            | FsEntry::RelationDir(_, _)
            | FsEntry::PathDir { .. } => Ok(ReplyOpen { fh: 0, flags: 0 }),
            FsEntry::WatchFile
            | FsEntry::PropertyFile(_, _)
            | FsEntry::RelationLink { .. }
            | FsEntry::RelationTargetLink { .. }
            | FsEntry::PathLink { .. } => Err(errno(libc::ENOTDIR)),
        }
    }

    type DirEntryStream<'a>
        = VecDirEntryStream
    where
        Self: 'a;

    async fn readdir<'a>(
        &'a self,
        _req: Request,
        parent: Inode,
        _fh: u64,
        offset: i64,
    ) -> fuse3::Result<ReplyDirectory<Self::DirEntryStream<'a>>> {
        let entry = self.entry(parent).await?;
        let entries = self.dir_entries(&entry, parent).await?;
        let entries = entries
            .into_iter()
            .enumerate()
            .skip(offset.max(0) as usize)
            .map(|(index, entry)| {
                Ok(DirectoryEntry {
                    inode: entry.ino,
                    kind: entry.kind,
                    name: entry.name.into(),
                    offset: (index + 1) as i64,
                })
            })
            .collect::<Vec<_>>();
        Ok(ReplyDirectory {
            entries: stream::iter(entries),
        })
    }

    async fn mkdir(
        &self,
        _req: Request,
        parent: Inode,
        name: &OsStr,
        _mode: u32,
        _umask: u32,
    ) -> fuse3::Result<ReplyEntry> {
        let entry = match self.entry(parent).await? {
            FsEntry::Root => self.create_kind_dir(name).await?,
            FsEntry::KindDir(kind) => {
                self.ensure_kind_writable(&kind).await?;
                let node = node_id_from_kind_and_segment(kind, os_str_to_str(name)?)?;
                self.graph
                    .create_node(&node)
                    .await
                    .map_err(graph_error_to_errno)?;
                FsEntry::NodeDir(node)
            }
            FsEntry::NodeDir(node) => {
                let relation = relation_name_from_segment(os_str_to_str(name)?)?;
                FsEntry::RelationDir(node, relation)
            }
            _ => return Err(errno(libc::ENOTDIR)),
        };
        self.entry_reply(entry).await
    }

    async fn open(&self, _req: Request, inode: Inode, _flags: u32) -> fuse3::Result<ReplyOpen> {
        match self.entry(inode).await? {
            entry @ (FsEntry::PropertyFile(_, _) | FsEntry::WatchFile) => {
                let handle = self.watch.lock().await.open(&entry)?;
                Ok(ReplyOpen {
                    fh: handle.0,
                    flags: FOPEN_DIRECT_IO,
                })
            }
            FsEntry::RelationLink { .. } | FsEntry::RelationTargetLink { .. } => {
                Err(errno(libc::EINVAL))
            }
            _ => Err(errno(libc::EISDIR)),
        }
    }

    async fn readlink(&self, _req: Request, inode: Inode) -> fuse3::Result<ReplyData> {
        let data = match self.entry(inode).await? {
            FsEntry::RelationLink {
                source,
                relation,
                target,
            } => {
                self.ensure_relation_link_exists(&source, &relation, &target)
                    .await?;
                direct_relation_link_target(&target)
                    .as_os_str()
                    .as_bytes()
                    .to_vec()
            }
            FsEntry::RelationTargetLink {
                source,
                relation,
                target,
            } => {
                self.ensure_relation_link_exists(&source, &relation, &target)
                    .await?;
                nested_relation_link_target(&target)
                    .as_os_str()
                    .as_bytes()
                    .to_vec()
            }
            FsEntry::PathLink { target, .. } => direct_relation_link_target(&target)
                .as_os_str()
                .as_bytes()
                .to_vec(),
            _ => return Err(errno(libc::EINVAL)),
        };
        Ok(ReplyData {
            data: Bytes::from(data),
        })
    }

    async fn read(
        &self,
        _req: Request,
        inode: Inode,
        fh: u64,
        offset: u64,
        size: u32,
    ) -> fuse3::Result<ReplyData> {
        let data = match self.entry(inode).await? {
            FsEntry::PropertyFile(node, key) => {
                let data = self.read_property(&node, &key).await?;
                slice_for_read(&data, offset, size).to_vec()
            }
            FsEntry::WatchFile => {
                self.watch
                    .lock()
                    .await
                    .read_watch_chunk(FileHandle(fh), offset, size)?
            }
            FsEntry::RelationLink { .. }
            | FsEntry::RelationTargetLink { .. }
            | FsEntry::PathLink { .. } => {
                return Err(errno(libc::EINVAL));
            }
            _ => return Err(errno(libc::EISDIR)),
        };

        self.watch.lock().await.mark_read(FileHandle(fh));
        Ok(ReplyData {
            data: Bytes::from(data),
        })
    }

    fn write(
        &self,
        _req: Request,
        inode: Inode,
        fh: u64,
        offset: u64,
        data: &[u8],
        _write_flags: u32,
        _flags: u32,
    ) -> impl Future<Output = fuse3::Result<ReplyWrite>> + Send {
        let fs = self.clone();
        let data = data.to_vec();
        async move { fs.write_owned(inode, fh, offset, data).await }
    }

    async fn release(
        &self,
        _req: Request,
        _inode: Inode,
        fh: u64,
        _flags: u32,
        _lock_owner: u64,
        _flush: bool,
    ) -> fuse3::Result<()> {
        self.watch.lock().await.release(FileHandle(fh));
        Ok(())
    }

    async fn create(
        &self,
        _req: Request,
        parent: Inode,
        name: &OsStr,
        _mode: u32,
        _flags: u32,
    ) -> fuse3::Result<ReplyCreated> {
        let entry = self.create_property_file(parent, name).await?;
        let ino = self.acquire_inode(entry.clone()).await?;
        let handle = self.watch.lock().await.open(&entry)?;
        Ok(ReplyCreated {
            ttl: TTL,
            attr: file_attr(
                ino,
                FileType::RegularFile,
                0o644,
                0,
                self.entry_times(&entry).await?,
            ),
            generation: 0,
            fh: handle.0,
            flags: FOPEN_DIRECT_IO,
        })
    }

    async fn setattr(
        &self,
        _req: Request,
        inode: Inode,
        _fh: Option<u64>,
        set_attr: SetAttr,
    ) -> fuse3::Result<ReplyAttr> {
        if matches!(set_attr.size, Some(0)) {
            if let FsEntry::PropertyFile(node, key) = self.entry(inode).await? {
                self.truncate_property(&node, &key).await?;
            }
            self.attr_reply(inode).await
        } else {
            Err(errno(libc::ENOSYS))
        }
    }

    async fn poll(
        &self,
        _req: Request,
        inode: Inode,
        fh: u64,
        kh: Option<u64>,
        flags: u32,
        _events: u32,
        notify: &Notify,
    ) -> fuse3::Result<ReplyPoll> {
        *self.notify.lock().await = Some(notify.clone());
        let revents = match self.entry(inode).await? {
            FsEntry::PropertyFile(_, _) | FsEntry::WatchFile => {
                self.watch.lock().await.poll(FileHandle(fh), kh, flags)?
            }
            _ => libc::POLLNVAL as u32,
        };
        Ok(ReplyPoll { revents })
    }

    async fn symlink(
        &self,
        _req: Request,
        parent: Inode,
        link_name: &OsStr,
        target: &OsStr,
    ) -> fuse3::Result<ReplyEntry> {
        let symlink_target = node_id_from_relation_link_target_path(Path::new(target))?;
        let entry = match self.entry(parent).await? {
            FsEntry::NodeDir(source) => {
                let relation = relation_name_from_segment(os_str_to_str(link_name)?)?;
                self.graph
                    .set_link(&source, &relation, &symlink_target)
                    .await
                    .map_err(graph_error_to_errno)?;
                FsEntry::RelationLink {
                    source,
                    relation,
                    target: symlink_target,
                }
            }
            FsEntry::RelationDir(source, relation) => {
                let targets = self.relation_targets(&source, &relation).await?;
                let target =
                    match relation_target_from_name(os_str_to_str(link_name)?, &source, &targets) {
                        Ok(target) => target,
                        Err(_) => NodeId::parse(
                            &decode_segment(os_str_to_str(link_name)?)
                                .map_err(graph_error_to_errno)?,
                        )
                        .map_err(graph_error_to_errno)?,
                    };
                if symlink_target != target {
                    return Err(errno(libc::EINVAL));
                }
                self.graph
                    .set_link(&source, &relation, &target)
                    .await
                    .map_err(graph_error_to_errno)?;
                FsEntry::RelationTargetLink {
                    source,
                    relation,
                    target,
                }
            }
            _ => return Err(errno(libc::ENOTDIR)),
        };

        self.entry_reply(entry).await
    }

    async fn unlink(&self, _req: Request, parent: Inode, name: &OsStr) -> fuse3::Result<()> {
        match self.entry(parent).await? {
            FsEntry::NodeDir(node) => {
                let name = os_str_to_str(name)?;
                let key = property_key_from_segment(name)?;
                let has_property = self.graph.property_spec(&node, &key).await.is_ok();
                let relation = relation_name_from_segment(name)?;
                let targets = self.relation_targets(&node, &relation).await?;
                let has_relation = !targets.is_empty();

                if has_property && has_relation {
                    return Err(errno(libc::EIO));
                }
                if has_property {
                    self.ensure_node_writable(&node).await?;
                    return self
                        .graph
                        .remove_property(&node, &key)
                        .await
                        .map_err(graph_error_to_errno);
                }

                let [target] = targets.as_slice() else {
                    return Err(errno(libc::ENOENT));
                };
                self.graph
                    .remove_link(&node, &relation, target)
                    .await
                    .map_err(graph_error_to_errno)
            }
            FsEntry::RelationDir(source, relation) => {
                let targets = self.relation_targets(&source, &relation).await?;
                let target = relation_target_from_name(os_str_to_str(name)?, &source, &targets)?;
                self.graph
                    .remove_link(&source, &relation, &target)
                    .await
                    .map_err(graph_error_to_errno)
            }
            _ => Err(errno(libc::ENOTDIR)),
        }
    }

    async fn rmdir(&self, _req: Request, parent: Inode, name: &OsStr) -> fuse3::Result<()> {
        match self.entry(parent).await? {
            FsEntry::KindDir(kind) => {
                self.ensure_kind_writable(&kind).await?;
                let node = node_id_from_kind_and_segment(kind, os_str_to_str(name)?)?;
                self.graph
                    .remove_node(&node)
                    .await
                    .map_err(graph_error_to_errno)?;
                self.forget_entry(&FsEntry::NodeDir(node)).await;
                Ok(())
            }
            FsEntry::NodeDir(source) => {
                let relation = relation_name_from_segment(os_str_to_str(name)?)?;
                let entry = FsEntry::RelationDir(source.clone(), relation.clone());
                if self.relation_targets(&source, &relation).await?.is_empty() {
                    self.forget_entry(&entry).await;
                    Ok(())
                } else {
                    Err(errno(libc::ENOTEMPTY))
                }
            }
            _ => Err(errno(libc::ENOTDIR)),
        }
    }

    type DirEntryPlusStream<'a>
        = VecDirEntryPlusStream
    where
        Self: 'a;

    async fn readdirplus<'a>(
        &'a self,
        _req: Request,
        parent: Inode,
        _fh: u64,
        offset: u64,
        _lock_owner: u64,
    ) -> fuse3::Result<ReplyDirectoryPlus<Self::DirEntryPlusStream<'a>>> {
        let entry = self.entry(parent).await?;
        let entries = self.dir_entries(&entry, parent).await?;
        let mut plus_entries = Vec::new();
        for (index, entry) in entries.into_iter().enumerate().skip(offset as usize) {
            let fs_entry = self.entry(entry.ino).await?;
            let ino = self.acquire_inode(fs_entry.clone()).await?;
            let attr = self.attr(&fs_entry, ino).await?;
            plus_entries.push(Ok(DirectoryEntryPlus {
                inode: ino,
                generation: 0,
                kind: entry.kind,
                name: entry.name.into(),
                offset: (index + 1) as i64,
                attr,
                entry_ttl: TTL,
                attr_ttl: TTL,
            }));
        }
        Ok(ReplyDirectoryPlus {
            entries: stream::iter(plus_entries),
        })
    }
}

impl LocusFs {
    pub(super) async fn truncate_property(
        &self,
        node: &NodeId,
        key: &PropertyKey,
    ) -> std::result::Result<(), Errno> {
        let spec = property_spec_or_new_string(&self.graph, node, key).await?;
        if !spec.is_writable() {
            return Err(errno(libc::EACCES));
        }
        let value =
            parse_property_write(spec.kind(), String::new()).map_err(graph_error_to_errno)?;
        self.graph
            .set_property(node, key, value)
            .await
            .map_err(graph_error_to_errno)?;
        self.touch_entry(&FsEntry::PropertyFile(node.clone(), key.clone()))
            .await;
        self.touch_entry(&FsEntry::NodeDir(node.clone())).await;
        Ok(())
    }

    async fn write_owned(
        &self,
        inode: Inode,
        fh: u64,
        offset: u64,
        data: Vec<u8>,
    ) -> fuse3::Result<ReplyWrite> {
        if offset != 0 {
            return Err(errno(libc::EINVAL));
        }

        let written = match self.entry(inode).await? {
            FsEntry::PropertyFile(node, key) => {
                let written = data.len() as u32;
                let input = String::from_utf8(data).map_err(|_| errno(libc::EINVAL))?;
                let spec = property_spec_or_new_string(&self.graph, &node, &key).await?;
                if !spec.is_writable() {
                    return Err(errno(libc::EACCES));
                }
                let value =
                    parse_property_write(spec.kind(), input).map_err(graph_error_to_errno)?;
                self.graph
                    .set_property(&node, &key, value)
                    .await
                    .map_err(graph_error_to_errno)?;
                self.touch_entry(&FsEntry::PropertyFile(node.clone(), key.clone()))
                    .await;
                self.touch_entry(&FsEntry::NodeDir(node)).await;
                written
            }
            FsEntry::WatchFile => {
                let path = parse_watch_subscription(&data)?;
                let target = resolve_watch_path(&self.graph, &path).await?;
                let graph_watch = if target.dependencies.is_empty()
                    && matches!(target.mode, WatchMode::Changes)
                {
                    Some(
                        self.graph
                            .watch(target.subject.clone())
                            .await
                            .map_err(graph_error_to_errno)?,
                    )
                } else {
                    None
                };
                self.watch.lock().await.configure_watch(
                    FileHandle(fh),
                    path,
                    target,
                    graph_watch.is_some(),
                )?;
                if let Some(graph_watch) = graph_watch {
                    let task = self.spawn_watch_forwarder(FileHandle(fh), graph_watch);
                    self.watch
                        .lock()
                        .await
                        .set_watch_task(FileHandle(fh), task)?;
                }
                data.len() as u32
            }
            _ => return Err(errno(libc::EISDIR)),
        };

        Ok(ReplyWrite { written })
    }

    fn spawn_watch_forwarder(
        &self,
        handle: FileHandle,
        mut graph_watch: GraphWatch,
    ) -> tokio::task::JoinHandle<()> {
        let watch = self.watch.clone();
        let notify = self.notify.clone();
        tokio::spawn(async move {
            while let Some(event) = graph_watch.recv().await {
                let handles = {
                    let mut watch = watch.lock().await;
                    watch.queue_graph_watch_event(handle, event)
                };
                wake_poll_handles(notify.clone(), handles).await;
            }
        })
    }

    async fn ensure_node_exists(&self, node: &NodeId) -> std::result::Result<(), Errno> {
        if self
            .graph
            .contains_node(node)
            .await
            .map_err(graph_error_to_errno)?
        {
            Ok(())
        } else {
            Err(errno(libc::ENOENT))
        }
    }

    async fn ensure_kind_writable(&self, kind: &NodeKind) -> std::result::Result<(), Errno> {
        let access = self
            .graph
            .kind_access(kind)
            .await
            .map_err(graph_error_to_errno)?;
        if access.is_writable() {
            Ok(())
        } else {
            Err(errno(libc::EACCES))
        }
    }

    async fn ensure_node_writable(&self, node: &NodeId) -> std::result::Result<(), Errno> {
        let access = self
            .graph
            .node_access(node)
            .await
            .map_err(graph_error_to_errno)?;
        if access.is_writable() {
            Ok(())
        } else {
            Err(errno(libc::EACCES))
        }
    }

    async fn ensure_relation_link_exists(
        &self,
        source: &NodeId,
        relation: &RelationName,
        target: &NodeId,
    ) -> std::result::Result<(), Errno> {
        if self
            .graph
            .targets(source, relation)
            .await
            .map_err(graph_error_to_errno)?
            .contains(target)
        {
            Ok(())
        } else {
            Err(errno(libc::ENOENT))
        }
    }

    pub(super) async fn relation_targets(
        &self,
        source: &NodeId,
        relation: &RelationName,
    ) -> std::result::Result<Vec<NodeId>, Errno> {
        match self.graph.targets(source, relation).await {
            Ok(targets) => Ok(targets),
            Err(GraphError::NotFound { .. }) => Ok(Vec::new()),
            Err(error) => Err(graph_error_to_errno(error)),
        }
    }
}
