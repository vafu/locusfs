use std::ffi::OsStr;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::sync::Mutex;
use std::time::SystemTime;

use fuser::{
    Errno, FileHandle, FileType, Filesystem, FopenFlags, Generation, INodeNo, KernelConfig,
    LockOwner, ReplyAttr, ReplyCreate, ReplyData, ReplyDirectory, ReplyEmpty, ReplyEntry,
    ReplyOpen, ReplyWrite, Request, TimeOrNow,
};
use locusfs_graph::{DynamicGraph, GraphError, NodeId, PropertyKey, RelationName};

use super::attr::{TTL, file_attr};
use super::entry::{self, DirEntry, FsEntry, parent_entry, relation_link_target};
use super::inode::InodeTable;
use super::name::{
    node_id_from_kind_and_segment, node_id_from_relation_link_target_path, node_kind_from_segment,
    os_str_to_str, property_key_from_segment, relation_name_from_segment,
};
use super::value::{
    parse_property_write, property_file_string, property_perm, property_spec_or_new_string,
    slice_for_read,
};
use crate::graph_error_to_errno;
use crate::layout::encode_segment;

/// FUSE request adapter over the generic graph filesystem.
#[derive(Debug)]
pub struct LocusFs {
    graph: DynamicGraph,
    inodes: Mutex<InodeTable>,
}

impl LocusFs {
    pub fn new(graph: DynamicGraph) -> Self {
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
                "nodes" => Ok(FsEntry::NodesDir),
                _ => Err(Errno::ENOENT),
            },
            FsEntry::NodesDir => {
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
            FsEntry::NodeDir(node) => match name {
                "props" => Ok(FsEntry::PropsDir(node)),
                "out" => Ok(FsEntry::OutDir(node)),
                _ => Err(Errno::ENOENT),
            },
            FsEntry::PropsDir(node) => {
                let key = property_key_from_segment(name)?;
                self.graph
                    .property_spec(&node, &key)
                    .map_err(graph_error_to_errno)?;
                Ok(FsEntry::PropertyFile(node, key))
            }
            FsEntry::OutDir(node) => {
                let relation = relation_name_from_segment(name)?;
                if self
                    .graph
                    .relations(&node)
                    .map_err(graph_error_to_errno)?
                    .contains(&relation)
                {
                    Ok(FsEntry::RelationDir(node, relation))
                } else {
                    Err(Errno::ENOENT)
                }
            }
            FsEntry::RelationDir(source, relation) => {
                let target_kind = node_kind_from_segment(name)?;
                if self
                    .graph
                    .targets(&source, &relation)
                    .map_err(graph_error_to_errno)?
                    .iter()
                    .any(|target| target.kind() == &target_kind)
                {
                    Ok(FsEntry::RelationTargetKindDir {
                        source,
                        relation,
                        target_kind,
                    })
                } else {
                    Err(Errno::ENOENT)
                }
            }
            FsEntry::RelationTargetKindDir {
                source,
                relation,
                target_kind,
            } => {
                let target = node_id_from_kind_and_segment(target_kind, name)?;
                if self
                    .graph
                    .targets(&source, &relation)
                    .map_err(graph_error_to_errno)?
                    .contains(&target)
                {
                    Ok(FsEntry::RelationLink {
                        source,
                        relation,
                        target,
                    })
                } else {
                    Err(Errno::ENOENT)
                }
            }
            FsEntry::PropertyFile(_, _) | FsEntry::RelationLink { .. } => Err(Errno::ENOTDIR),
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
            FsEntry::Root | FsEntry::NodesDir => (FileType::Directory, 0o755, 0),
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
            FsEntry::NodeDir(node) | FsEntry::PropsDir(node) | FsEntry::OutDir(node) => {
                self.ensure_node_exists(node)?;
                (FileType::Directory, 0o755, 0)
            }
            FsEntry::RelationDir(node, _) | FsEntry::RelationTargetKindDir { source: node, .. } => {
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
                    relation_link_target(target).as_os_str().as_bytes().len() as u64,
                )
            }
        };

        Ok(file_attr(ino, kind, perm, size))
    }

    fn create_property_file(
        &self,
        parent: u64,
        name: &OsStr,
    ) -> std::result::Result<FsEntry, Errno> {
        let FsEntry::PropsDir(node) = self.entry(parent)? else {
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

    fn opendir(&self, _req: &Request, ino: INodeNo, _flags: fuser::OpenFlags, reply: ReplyOpen) {
        match self.entry(ino.0) {
            Ok(
                FsEntry::Root
                | FsEntry::NodesDir
                | FsEntry::KindDir(_)
                | FsEntry::NodeDir(_)
                | FsEntry::PropsDir(_)
                | FsEntry::OutDir(_)
                | FsEntry::RelationDir(_, _)
                | FsEntry::RelationTargetKindDir { .. },
            ) => reply.opened(FileHandle(0), FopenFlags::empty()),
            Ok(FsEntry::PropertyFile(_, _) | FsEntry::RelationLink { .. }) => {
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
            FsEntry::OutDir(node) => {
                let relation = relation_name_from_segment(os_str_to_str(name)?)?;
                Ok(FsEntry::RelationDir(node, relation))
            }
            FsEntry::RelationDir(source, relation) => {
                let target_kind = node_kind_from_segment(os_str_to_str(name)?)?;
                Ok(FsEntry::RelationTargetKindDir {
                    source,
                    relation,
                    target_kind,
                })
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

    fn open(&self, _req: &Request, ino: INodeNo, _flags: fuser::OpenFlags, reply: ReplyOpen) {
        match self.entry(ino.0) {
            Ok(FsEntry::PropertyFile(_, _)) => reply.opened(FileHandle(0), FopenFlags::empty()),
            Ok(FsEntry::RelationLink { .. }) => reply.error(Errno::EINVAL),
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
                Ok(()) => reply.data(relation_link_target(&target).as_os_str().as_bytes()),
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
        _fh: FileHandle,
        offset: u64,
        size: u32,
        _flags: fuser::OpenFlags,
        _lock_owner: Option<LockOwner>,
        reply: ReplyData,
    ) {
        let result = self.entry(ino.0).and_then(|entry| match entry {
            FsEntry::PropertyFile(node, key) => self.read_property(&node, &key),
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
            let FsEntry::PropertyFile(node, key) = entry else {
                return Err(Errno::EISDIR);
            };
            let input = std::str::from_utf8(data).map_err(|_| Errno::EINVAL)?;
            let spec = property_spec_or_new_string(&self.graph, &node, &key)?;
            if !spec.is_writable() {
                return Err(Errno::EACCES);
            }
            let value = parse_property_write(spec.kind(), input).map_err(graph_error_to_errno)?;
            self.graph
                .set_property(&node, &key, value)
                .map_err(graph_error_to_errno)?;
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
        match self.create_property_file(parent.0, name).and_then(|entry| {
            let ino = self.acquire_inode(entry.clone())?;
            Ok(file_attr(ino, FileType::RegularFile, 0o644, 0))
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
        _target: &Path,
        reply: ReplyEntry,
    ) {
        let result = self.entry(parent.0).and_then(|entry| {
            let FsEntry::RelationTargetKindDir {
                source,
                relation,
                target_kind,
            } = entry
            else {
                return Err(Errno::ENOTDIR);
            };
            let target = node_id_from_kind_and_segment(target_kind, os_str_to_str(link_name)?)?;
            let symlink_target = node_id_from_relation_link_target_path(_target)?;
            if symlink_target != target {
                return Err(Errno::EINVAL);
            }
            self.graph
                .set_link(&source, &relation, &target)
                .map_err(graph_error_to_errno)?;
            Ok(FsEntry::RelationLink {
                source,
                relation,
                target,
            })
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
            FsEntry::PropsDir(node) => {
                let key = property_key_from_segment(os_str_to_str(name)?)?;
                self.graph
                    .remove_property(&node, &key)
                    .map_err(graph_error_to_errno)
            }
            FsEntry::RelationDir(_, _) => Err(Errno::EISDIR),
            FsEntry::RelationTargetKindDir {
                source,
                relation,
                target_kind,
            } => {
                let target = node_id_from_kind_and_segment(target_kind, os_str_to_str(name)?)?;
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
            FsEntry::RelationDir(source, relation) => {
                let target_kind = node_kind_from_segment(os_str_to_str(name)?)?;
                let entry = FsEntry::RelationTargetKindDir {
                    source: source.clone(),
                    relation: relation.clone(),
                    target_kind: target_kind.clone(),
                };
                if self
                    .relation_targets(&source, &relation)?
                    .iter()
                    .any(|target| target.kind() == &target_kind)
                {
                    Err(Errno::ENOTEMPTY)
                } else {
                    self.forget_entry(&entry);
                    Ok(())
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

    fn relation_targets(
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

    fn dir_entries(&self, entry: &FsEntry, ino: u64) -> std::result::Result<Vec<DirEntry>, Errno> {
        let mut entries = vec![
            DirEntry::new(ino, FileType::Directory, "."),
            DirEntry::new(self.inode(parent_entry(entry))?, FileType::Directory, ".."),
        ];

        match entry {
            FsEntry::Root => {
                entries.push(DirEntry::new(
                    entry::NODES_INO,
                    FileType::Directory,
                    "nodes",
                ));
            }
            FsEntry::NodesDir => {
                for kind in self.graph.node_kinds().map_err(graph_error_to_errno)? {
                    let child = FsEntry::KindDir(kind.clone());
                    let child_ino = self.inode(child)?;
                    entries.push(DirEntry::new(
                        child_ino,
                        FileType::Directory,
                        encode_segment(kind.as_str()).map_err(graph_error_to_errno)?,
                    ));
                }
            }
            FsEntry::KindDir(kind) => {
                for node in self
                    .graph
                    .nodes_by_kind(kind)
                    .map_err(graph_error_to_errno)?
                {
                    let child = FsEntry::NodeDir(node.clone());
                    let child_ino = self.inode(child)?;
                    entries.push(DirEntry::new(
                        child_ino,
                        FileType::Directory,
                        encode_segment(node.local()).map_err(graph_error_to_errno)?,
                    ));
                }
            }
            FsEntry::NodeDir(node) => {
                let props = FsEntry::PropsDir(node.clone());
                let out = FsEntry::OutDir(node.clone());
                entries.push(DirEntry::new(
                    self.inode(props)?,
                    FileType::Directory,
                    "props",
                ));
                entries.push(DirEntry::new(self.inode(out)?, FileType::Directory, "out"));
            }
            FsEntry::PropsDir(node) => {
                for spec in self.graph.properties(node).map_err(graph_error_to_errno)? {
                    let key = spec.key();
                    let child = FsEntry::PropertyFile(node.clone(), key.clone());
                    let child_ino = self.inode(child)?;
                    entries.push(DirEntry::new(
                        child_ino,
                        FileType::RegularFile,
                        encode_segment(key.as_str()).map_err(graph_error_to_errno)?,
                    ));
                }
            }
            FsEntry::OutDir(node) => {
                for relation in self.graph.relations(node).map_err(graph_error_to_errno)? {
                    let child = FsEntry::RelationDir(node.clone(), relation.clone());
                    let child_ino = self.inode(child)?;
                    entries.push(DirEntry::new(
                        child_ino,
                        FileType::Directory,
                        encode_segment(relation.as_str()).map_err(graph_error_to_errno)?,
                    ));
                }
            }
            FsEntry::RelationDir(source, relation) => {
                let mut target_kinds = self
                    .relation_targets(source, relation)?
                    .into_iter()
                    .map(|target| target.kind().clone())
                    .collect::<Vec<_>>();
                target_kinds.sort();
                target_kinds.dedup();
                for target_kind in target_kinds {
                    let child = FsEntry::RelationTargetKindDir {
                        source: source.clone(),
                        relation: relation.clone(),
                        target_kind: target_kind.clone(),
                    };
                    let child_ino = self.inode(child)?;
                    entries.push(DirEntry::new(
                        child_ino,
                        FileType::Directory,
                        encode_segment(target_kind.as_str()).map_err(graph_error_to_errno)?,
                    ));
                }
            }
            FsEntry::RelationTargetKindDir {
                source,
                relation,
                target_kind,
            } => {
                for target in self
                    .relation_targets(source, relation)?
                    .into_iter()
                    .filter(|target| target.kind() == target_kind)
                {
                    let child = FsEntry::RelationLink {
                        source: source.clone(),
                        relation: relation.clone(),
                        target: target.clone(),
                    };
                    let child_ino = self.inode(child)?;
                    entries.push(DirEntry::new(
                        child_ino,
                        FileType::Symlink,
                        encode_segment(target.local()).map_err(graph_error_to_errno)?,
                    ));
                }
            }
            FsEntry::PropertyFile(_, _) | FsEntry::RelationLink { .. } => {}
        }

        Ok(entries)
    }
}
