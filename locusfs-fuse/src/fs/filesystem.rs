use std::ffi::OsStr;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::time::SystemTime;

use fuser::{
    Errno, FileHandle, FileType, Filesystem, FopenFlags, Generation, INodeNo, KernelConfig,
    LockOwner, OpenFlags, PollEvents, PollFlags, PollNotifier, ReplyAttr, ReplyCreate, ReplyData,
    ReplyDirectory, ReplyEmpty, ReplyEntry, ReplyOpen, ReplyPoll, ReplyWrite, Request, TimeOrNow,
};
use locusfs_graph::{DynamicGraph, GraphError, NodeId, PropertyKey, RelationName};

use super::attr::{EntryTimes, TTL, file_attr};
use super::entry::{
    FsEntry, WATCH_FILE_NAME, direct_relation_link_target, nested_relation_link_target,
};
use super::inode::{InodeTable, SharedInodeTable};
use super::name::{
    decode_relation_target_name, node_id_from_kind_and_segment,
    node_id_from_relation_link_target_path, node_kind_from_segment, os_str_to_str,
    property_key_from_segment, relation_name_from_segment,
};
use super::resolve::{parse_watch_subscription, resolve_watch_path};
use super::value::{
    parse_property_write, property_file_string, property_perm, property_spec_or_new_string,
    slice_for_read,
};
use super::watch::{SharedWatchRegistry, WatchRegistry};
use crate::graph_error_to_errno;

/// FUSE request adapter over the generic graph filesystem.
#[derive(Debug)]
pub struct LocusFs {
    pub(super) graph: DynamicGraph,
    inodes: SharedInodeTable,
    watch: SharedWatchRegistry,
}

impl LocusFs {
    pub fn new(graph: DynamicGraph) -> Self {
        Self::new_with_state(graph, InodeTable::shared(), WatchRegistry::shared())
    }

    pub(crate) fn new_with_state(
        graph: DynamicGraph,
        inodes: SharedInodeTable,
        watch: SharedWatchRegistry,
    ) -> Self {
        Self {
            graph,
            inodes,
            watch,
        }
    }

    fn lookup_entry(&self, parent: u64, name: &OsStr) -> std::result::Result<FsEntry, Errno> {
        let name = os_str_to_str(name)?;
        let parent = self.entry(parent)?;
        match parent {
            FsEntry::Root => {
                if name == WATCH_FILE_NAME {
                    return Ok(FsEntry::WatchFile);
                }
                let kind = node_kind_from_segment(name)?;
                if self
                    .graph
                    .node_kinds()
                    .map_err(graph_error_to_errno)?
                    .contains(&kind)
                {
                    Ok(FsEntry::KindDir(kind))
                } else {
                    Err(Errno::ENOENT)
                }
            }
            FsEntry::KindDir(kind) => {
                let node = node_id_from_kind_and_segment(kind, name)?;
                if self
                    .graph
                    .contains_node(&node)
                    .map_err(graph_error_to_errno)?
                {
                    Ok(FsEntry::NodeDir(node))
                } else {
                    Err(Errno::ENOENT)
                }
            }
            FsEntry::NodeDir(node) => {
                let property = property_key_from_segment(name)?;
                let has_property = self.graph.property_spec(&node, &property).is_ok();
                let relation = relation_name_from_segment(name)?;
                let targets = self.relation_targets(&node, &relation)?;
                let has_relation = !targets.is_empty();

                if has_property && has_relation {
                    return Err(Errno::EIO);
                }
                if has_property {
                    return Ok(FsEntry::PropertyFile(node, property));
                }

                match targets.as_slice() {
                    [] => Err(Errno::ENOENT),
                    [target] => Ok(FsEntry::RelationLink {
                        source: node,
                        relation,
                        target: target.clone(),
                    }),
                    _ => Ok(FsEntry::RelationDir(node, relation)),
                }
            }
            FsEntry::RelationDir(source, relation) => {
                let target = NodeId::parse(&decode_relation_target_name(name)?)
                    .map_err(graph_error_to_errno)?;
                if self
                    .graph
                    .targets(&source, &relation)
                    .map_err(graph_error_to_errno)?
                    .contains(&target)
                {
                    Ok(FsEntry::RelationTargetLink {
                        source,
                        relation,
                        target,
                    })
                } else {
                    Err(Errno::ENOENT)
                }
            }
            FsEntry::WatchFile
            | FsEntry::PropertyFile(_, _)
            | FsEntry::RelationLink { .. }
            | FsEntry::RelationTargetLink { .. } => Err(Errno::ENOTDIR),
        }
    }

    fn entry(&self, ino: u64) -> std::result::Result<FsEntry, Errno> {
        self.inodes
            .lock()
            .map_err(|_| Errno::EIO)?
            .entry(ino)
            .ok_or(Errno::ENOENT)
    }

    pub(super) fn inode(&self, entry: FsEntry) -> std::result::Result<u64, Errno> {
        self.inodes.lock().map_err(|_| Errno::EIO)?.inode(entry)
    }

    fn acquire_inode(&self, entry: FsEntry) -> std::result::Result<u64, Errno> {
        self.inodes.lock().map_err(|_| Errno::EIO)?.acquire(entry)
    }

    fn forget_inode(&self, ino: u64, nlookup: u64) {
        if let Ok(mut inodes) = self.inodes.lock() {
            inodes.forget(ino, nlookup);
        }
    }

    fn forget_entry(&self, entry: &FsEntry) {
        if let Ok(mut inodes) = self.inodes.lock() {
            inodes.forget_entry(entry);
        }
    }

    fn attr(&self, entry: &FsEntry, ino: u64) -> std::result::Result<fuser::FileAttr, Errno> {
        let (kind, perm, size) = match entry {
            FsEntry::Root => (FileType::Directory, 0o755, 0),
            FsEntry::WatchFile => (FileType::RegularFile, 0o600, 0),
            FsEntry::KindDir(kind) => {
                if !self
                    .graph
                    .node_kinds()
                    .map_err(graph_error_to_errno)?
                    .contains(kind)
                {
                    return Err(Errno::ENOENT);
                }
                (FileType::Directory, 0o755, 0)
            }
            FsEntry::NodeDir(node) => {
                self.ensure_node_exists(node)?;
                (FileType::Directory, 0o755, 0)
            }
            FsEntry::RelationDir(node, _) => {
                self.ensure_node_exists(node)?;
                (FileType::Directory, 0o755, 0)
            }
            FsEntry::PropertyFile(node, key) => {
                let spec = self
                    .graph
                    .property_spec(node, key)
                    .map_err(graph_error_to_errno)?;
                let size = if spec.is_readable() {
                    let value = self
                        .graph
                        .property(node, key)
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
                self.ensure_relation_link_exists(source, relation, target)?;
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
                self.ensure_relation_link_exists(source, relation, target)?;
                (
                    FileType::Symlink,
                    0o777,
                    nested_relation_link_target(target)
                        .as_os_str()
                        .as_bytes()
                        .len() as u64,
                )
            }
        };

        Ok(file_attr(ino, kind, perm, size, self.entry_times(entry)?))
    }

    fn entry_times(&self, entry: &FsEntry) -> std::result::Result<EntryTimes, Errno> {
        Ok(self.inodes.lock().map_err(|_| Errno::EIO)?.times(entry))
    }

    fn touch_entry(&self, entry: &FsEntry) {
        if let Ok(mut inodes) = self.inodes.lock() {
            inodes.touch(entry);
        }
    }

    fn create_property_file(
        &self,
        parent: u64,
        name: &OsStr,
    ) -> std::result::Result<FsEntry, Errno> {
        let FsEntry::NodeDir(node) = self.entry(parent)? else {
            return Err(Errno::ENOTDIR);
        };
        let key = property_key_from_segment(os_str_to_str(name)?)?;
        Ok(FsEntry::PropertyFile(node, key))
    }

    fn read_property(
        &self,
        node: &NodeId,
        key: &PropertyKey,
    ) -> std::result::Result<Vec<u8>, Errno> {
        let spec = self
            .graph
            .property_spec(node, key)
            .map_err(graph_error_to_errno)?;
        if spec.is_readable() {
            let value = self
                .graph
                .property(node, key)
                .map_err(graph_error_to_errno)?;
            Ok(property_file_string(&value).into_bytes())
        } else {
            Err(Errno::EACCES)
        }
    }
}

impl Filesystem for LocusFs {
    fn init(&mut self, _req: &Request, _config: &mut KernelConfig) -> io::Result<()> {
        Ok(())
    }

    fn lookup(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEntry) {
        match self.lookup_entry(parent.0, name).and_then(|entry| {
            let ino = self.acquire_inode(entry.clone())?;
            let attr = self.attr(&entry, ino)?;
            Ok(attr)
        }) {
            Ok(attr) => reply.entry(&TTL, &attr, Generation(0)),
            Err(error) => reply.error(error),
        }
    }

    fn forget(&self, _req: &Request, ino: INodeNo, nlookup: u64) {
        self.forget_inode(ino.0, nlookup);
    }

    fn getattr(&self, _req: &Request, ino: INodeNo, _fh: Option<FileHandle>, reply: ReplyAttr) {
        match self.entry(ino.0).and_then(|entry| self.attr(&entry, ino.0)) {
            Ok(attr) => reply.attr(&TTL, &attr),
            Err(error) => reply.error(error),
        }
    }

    fn opendir(&self, _req: &Request, ino: INodeNo, _flags: OpenFlags, reply: ReplyOpen) {
        match self.entry(ino.0) {
            Ok(
                FsEntry::Root
                | FsEntry::KindDir(_)
                | FsEntry::NodeDir(_)
                | FsEntry::RelationDir(_, _),
            ) => reply.opened(FileHandle(0), FopenFlags::empty()),
            Ok(
                FsEntry::WatchFile
                | FsEntry::PropertyFile(_, _)
                | FsEntry::RelationLink { .. }
                | FsEntry::RelationTargetLink { .. },
            ) => reply.error(Errno::ENOTDIR),
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

    fn mkdir(
        &self,
        _req: &Request,
        parent: INodeNo,
        name: &OsStr,
        _mode: u32,
        _umask: u32,
        reply: ReplyEntry,
    ) {
        let result = self.entry(parent.0).and_then(|entry| match entry {
            FsEntry::KindDir(kind) => {
                let node = node_id_from_kind_and_segment(kind, os_str_to_str(name)?)?;
                self.graph
                    .create_node(&node)
                    .map_err(graph_error_to_errno)?;
                Ok(FsEntry::NodeDir(node))
            }
            FsEntry::NodeDir(node) => {
                let relation = relation_name_from_segment(os_str_to_str(name)?)?;
                Ok(FsEntry::RelationDir(node, relation))
            }
            _ => Err(Errno::ENOTDIR),
        });

        match result.and_then(|entry| {
            let ino = self.acquire_inode(entry.clone())?;
            self.attr(&entry, ino)
        }) {
            Ok(attr) => reply.entry(&TTL, &attr, Generation(0)),
            Err(error) => reply.error(error),
        }
    }

    fn open(&self, _req: &Request, ino: INodeNo, _flags: OpenFlags, reply: ReplyOpen) {
        match self.entry(ino.0) {
            Ok(entry @ (FsEntry::PropertyFile(_, _) | FsEntry::WatchFile)) => {
                let handle = self
                    .watch
                    .lock()
                    .map_err(|_| Errno::EIO)
                    .and_then(|mut watch| watch.open(&entry));
                match handle {
                    Ok(handle) => {
                        reply.opened(handle, FopenFlags::FOPEN_DIRECT_IO);
                    }
                    Err(error) => reply.error(error),
                }
            }
            Ok(FsEntry::RelationLink { .. } | FsEntry::RelationTargetLink { .. }) => {
                reply.error(Errno::EINVAL)
            }
            Ok(_) => reply.error(Errno::EISDIR),
            Err(error) => reply.error(error),
        }
    }

    fn readlink(&self, _req: &Request, ino: INodeNo, reply: ReplyData) {
        match self.entry(ino.0) {
            Ok(FsEntry::RelationLink {
                source,
                relation,
                target,
            }) => match self.ensure_relation_link_exists(&source, &relation, &target) {
                Ok(()) => reply.data(direct_relation_link_target(&target).as_os_str().as_bytes()),
                Err(error) => reply.error(error),
            },
            Ok(FsEntry::RelationTargetLink {
                source,
                relation,
                target,
            }) => match self.ensure_relation_link_exists(&source, &relation, &target) {
                Ok(()) => reply.data(nested_relation_link_target(&target).as_os_str().as_bytes()),
                Err(error) => reply.error(error),
            },
            Ok(_) => reply.error(Errno::EINVAL),
            Err(error) => reply.error(error),
        }
    }

    fn read(
        &self,
        _req: &Request,
        ino: INodeNo,
        fh: FileHandle,
        offset: u64,
        size: u32,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        reply: ReplyData,
    ) {
        let result = self.entry(ino.0).and_then(|entry| match entry {
            FsEntry::PropertyFile(node, key) => self
                .read_property(&node, &key)
                .map(|data| slice_for_read(&data, offset, size).to_vec()),
            FsEntry::WatchFile => {
                self.watch
                    .lock()
                    .map_err(|_| Errno::EIO)
                    .and_then(|mut watch| {
                        let data = watch.read_watch(fh)?;
                        Ok(slice_for_read(&data, 0, size).to_vec())
                    })
            }
            FsEntry::RelationLink { .. } | FsEntry::RelationTargetLink { .. } => Err(Errno::EINVAL),
            _ => Err(Errno::EISDIR),
        });

        match result {
            Ok(data) => {
                if let Ok(mut watch) = self.watch.lock() {
                    watch.mark_read(fh);
                }
                reply.data(&data)
            }
            Err(error) => reply.error(error),
        }
    }

    fn write(
        &self,
        _req: &Request,
        ino: INodeNo,
        fh: FileHandle,
        offset: u64,
        data: &[u8],
        _write_flags: fuser::WriteFlags,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        reply: ReplyWrite,
    ) {
        if offset != 0 {
            reply.error(Errno::EINVAL);
            return;
        }

        let result = self.entry(ino.0).and_then(|entry| match entry {
            FsEntry::PropertyFile(node, key) => {
                let input = std::str::from_utf8(data).map_err(|_| Errno::EINVAL)?;
                let spec = property_spec_or_new_string(&self.graph, &node, &key)?;
                if !spec.is_writable() {
                    return Err(Errno::EACCES);
                }
                let value =
                    parse_property_write(spec.kind(), input).map_err(graph_error_to_errno)?;
                self.graph
                    .set_property(&node, &key, value)
                    .map_err(graph_error_to_errno)?;
                self.touch_entry(&FsEntry::PropertyFile(node.clone(), key.clone()));
                self.touch_entry(&FsEntry::NodeDir(node));
                Ok(data.len() as u32)
            }
            FsEntry::WatchFile => {
                let path = parse_watch_subscription(data)?;
                let target = resolve_watch_path(&self.graph, &path)?;
                self.watch
                    .lock()
                    .map_err(|_| Errno::EIO)?
                    .configure_watch(fh, path, target)?;
                Ok(data.len() as u32)
            }
            _ => Err(Errno::EISDIR),
        });

        match result {
            Ok(written) => reply.written(written),
            Err(error) => reply.error(error),
        }
    }

    fn release(
        &self,
        _req: &Request,
        _ino: INodeNo,
        fh: FileHandle,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        _flush: bool,
        reply: ReplyEmpty,
    ) {
        if let Ok(mut watch) = self.watch.lock() {
            watch.release(fh);
        }
        reply.ok();
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
        match self.create_property_file(parent.0, name).and_then(|entry| {
            let ino = self.acquire_inode(entry.clone())?;
            Ok(file_attr(
                ino,
                FileType::RegularFile,
                0o644,
                0,
                self.entry_times(&entry)?,
            ))
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

    fn poll(
        &self,
        _req: &Request,
        ino: INodeNo,
        fh: FileHandle,
        ph: PollNotifier,
        _events: PollEvents,
        flags: PollFlags,
        reply: ReplyPoll,
    ) {
        match self.entry(ino.0) {
            Ok(FsEntry::PropertyFile(_, _) | FsEntry::WatchFile) => {
                let result = self
                    .watch
                    .lock()
                    .map_err(|_| Errno::EIO)
                    .and_then(|mut watch| watch.poll(fh, ph, flags));
                match result {
                    Ok(events) => reply.poll(events),
                    Err(error) => reply.error(error),
                }
            }
            Ok(_) => reply.poll(PollEvents::POLLNVAL),
            Err(error) => reply.error(error),
        }
    }

    fn symlink(
        &self,
        _req: &Request,
        parent: INodeNo,
        link_name: &OsStr,
        _target: &Path,
        reply: ReplyEntry,
    ) {
        let result = self.entry(parent.0).and_then(|entry| {
            let symlink_target = node_id_from_relation_link_target_path(_target)?;
            match entry {
                FsEntry::NodeDir(source) => {
                    let relation = relation_name_from_segment(os_str_to_str(link_name)?)?;
                    self.graph
                        .set_link(&source, &relation, &symlink_target)
                        .map_err(graph_error_to_errno)?;
                    Ok(FsEntry::RelationLink {
                        source,
                        relation,
                        target: symlink_target,
                    })
                }
                FsEntry::RelationDir(source, relation) => {
                    let target =
                        NodeId::parse(&decode_relation_target_name(os_str_to_str(link_name)?)?)
                            .map_err(graph_error_to_errno)?;
                    if symlink_target != target {
                        return Err(Errno::EINVAL);
                    }
                    self.graph
                        .set_link(&source, &relation, &target)
                        .map_err(graph_error_to_errno)?;
                    Ok(FsEntry::RelationTargetLink {
                        source,
                        relation,
                        target,
                    })
                }
                _ => Err(Errno::ENOTDIR),
            }
        });

        match result.and_then(|entry| {
            let ino = self.acquire_inode(entry.clone())?;
            self.attr(&entry, ino)
        }) {
            Ok(attr) => reply.entry(&TTL, &attr, Generation(0)),
            Err(error) => reply.error(error),
        }
    }

    fn unlink(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        let result = self.entry(parent.0).and_then(|entry| match entry {
            FsEntry::NodeDir(node) => {
                let name = os_str_to_str(name)?;
                let key = property_key_from_segment(name)?;
                let has_property = self.graph.property_spec(&node, &key).is_ok();
                let relation = relation_name_from_segment(name)?;
                let targets = self.relation_targets(&node, &relation)?;
                let has_relation = !targets.is_empty();

                if has_property && has_relation {
                    return Err(Errno::EIO);
                }
                if has_property {
                    return self
                        .graph
                        .remove_property(&node, &key)
                        .map_err(graph_error_to_errno);
                }

                let [target] = targets.as_slice() else {
                    return Err(Errno::ENOENT);
                };
                self.graph
                    .remove_link(&node, &relation, target)
                    .map_err(graph_error_to_errno)
            }
            FsEntry::RelationDir(source, relation) => {
                let target = NodeId::parse(&decode_relation_target_name(os_str_to_str(name)?)?)
                    .map_err(graph_error_to_errno)?;
                self.graph
                    .remove_link(&source, &relation, &target)
                    .map_err(graph_error_to_errno)
            }
            _ => Err(Errno::ENOTDIR),
        });

        match result {
            Ok(()) => reply.ok(),
            Err(error) => reply.error(error),
        }
    }

    fn rmdir(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        let result = self.entry(parent.0).and_then(|entry| match entry {
            FsEntry::KindDir(kind) => {
                let node = node_id_from_kind_and_segment(kind, os_str_to_str(name)?)?;
                self.graph
                    .remove_node(&node)
                    .map_err(graph_error_to_errno)?;
                self.forget_entry(&FsEntry::NodeDir(node));
                Ok(())
            }
            FsEntry::NodeDir(source) => {
                let relation = relation_name_from_segment(os_str_to_str(name)?)?;
                let entry = FsEntry::RelationDir(source.clone(), relation.clone());
                if self.relation_targets(&source, &relation)?.is_empty() {
                    self.forget_entry(&entry);
                    Ok(())
                } else {
                    Err(Errno::ENOTEMPTY)
                }
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
    fn ensure_node_exists(&self, node: &NodeId) -> std::result::Result<(), Errno> {
        if self
            .graph
            .contains_node(node)
            .map_err(graph_error_to_errno)?
        {
            Ok(())
        } else {
            Err(Errno::ENOENT)
        }
    }

    fn ensure_relation_link_exists(
        &self,
        source: &NodeId,
        relation: &RelationName,
        target: &NodeId,
    ) -> std::result::Result<(), Errno> {
        if self
            .graph
            .targets(source, relation)
            .map_err(graph_error_to_errno)?
            .contains(target)
        {
            Ok(())
        } else {
            Err(Errno::ENOENT)
        }
    }

    pub(super) fn relation_targets(
        &self,
        source: &NodeId,
        relation: &RelationName,
    ) -> std::result::Result<Vec<NodeId>, Errno> {
        match self.graph.targets(source, relation) {
            Ok(targets) => Ok(targets),
            Err(GraphError::NotFound { .. }) => Ok(Vec::new()),
            Err(error) => Err(graph_error_to_errno(error)),
        }
    }
}
