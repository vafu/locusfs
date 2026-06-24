use std::collections::BTreeSet;

use fuse3::{Errno, FileType};
use locusfs_graph::{GraphError, GraphPathDirectory, GraphPathEntry, NodeId, RelationName};

use super::entry::{DirEntry, FsEntry, WATCH_FILE_NAME, parent_entry};
use super::filesystem::LocusFs;
use super::name::encode_relation_target_name;
use crate::layout::encode_segment;
use crate::{errno, graph_error_to_errno};

impl LocusFs {
    pub(super) async fn dir_entries(
        &self,
        entry: &FsEntry,
        ino: u64,
    ) -> std::result::Result<Vec<DirEntry>, Errno> {
        let mut entries = vec![
            DirEntry::new(ino, FileType::Directory, "."),
            DirEntry::new(
                self.inode(parent_entry(entry)).await?,
                FileType::Directory,
                "..",
            ),
        ];

        match entry {
            FsEntry::Root => {
                let child = FsEntry::WatchFile;
                let child_ino = self.inode(child).await?;
                entries.push(DirEntry::new(
                    child_ino,
                    FileType::RegularFile,
                    WATCH_FILE_NAME,
                ));

                for kind in self
                    .graph
                    .node_kinds()
                    .await
                    .map_err(graph_error_to_errno)?
                {
                    if self
                        .graph
                        .kind_access(&kind)
                        .await
                        .map_err(graph_error_to_errno)?
                        .is_readable()
                        == false
                    {
                        continue;
                    }
                    let child = FsEntry::KindDir(kind.clone());
                    let child_ino = self.inode(child).await?;
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
                    .await
                    .map_err(graph_error_to_errno)?
                {
                    let child = FsEntry::NodeDir(node.clone());
                    let child_ino = self.inode(child).await?;
                    entries.push(DirEntry::new(
                        child_ino,
                        FileType::Directory,
                        encode_segment(node.local()).map_err(graph_error_to_errno)?,
                    ));
                }
            }
            FsEntry::NodeDir(node) => {
                let mut names = BTreeSet::new();
                let mut provider_names = BTreeSet::new();
                if let Some(children) = self
                    .graph
                    .path_children(&GraphPathDirectory::Node(node.clone()))
                    .await
                    .map_err(graph_error_to_errno)?
                {
                    for child in children {
                        let child_name = child.name.into_string();
                        provider_names.insert(child_name.clone());
                        names.insert(child_name.clone());
                        let kind = path_entry_file_type(&child.entry);
                        let child_entry = path_entry(child.entry, FsEntry::NodeDir(node.clone()));
                        let child_ino = self.inode(child_entry).await?;
                        entries.push(DirEntry::new(child_ino, kind, child_name));
                    }
                }
                for spec in self
                    .graph
                    .properties(node)
                    .await
                    .map_err(graph_error_to_errno)?
                {
                    let key = spec.key();
                    let name = encode_segment(key.as_str()).map_err(graph_error_to_errno)?;
                    if !names.insert(name.clone()) {
                        if provider_names.contains(&name) {
                            continue;
                        }
                        return Err(errno(libc::EIO));
                    }
                    let child = FsEntry::PropertyFile(node.clone(), key.clone());
                    let child_ino = self.inode(child).await?;
                    entries.push(DirEntry::new(child_ino, FileType::RegularFile, name));
                }

                for relation in self.relations(node).await? {
                    let name = encode_segment(relation.as_str()).map_err(graph_error_to_errno)?;
                    if !names.insert(name.clone()) {
                        if provider_names.contains(&name) {
                            continue;
                        }
                        return Err(errno(libc::EIO));
                    }
                    let targets = self.relation_targets(node, &relation).await?;
                    match targets.as_slice() {
                        [] => {}
                        [target] => {
                            let child = FsEntry::RelationLink {
                                source: node.clone(),
                                relation: relation.clone(),
                                target: target.clone(),
                            };
                            let child_ino = self.inode(child).await?;
                            entries.push(DirEntry::new(child_ino, FileType::Symlink, name.clone()));
                        }
                        _ => {
                            let child = FsEntry::RelationDir(node.clone(), relation.clone());
                            let child_ino = self.inode(child).await?;
                            entries.push(DirEntry::new(
                                child_ino,
                                FileType::Directory,
                                name.clone(),
                            ));
                        }
                    };
                }
            }
            FsEntry::RelationDir(source, relation) => {
                let targets = self.relation_targets(source, relation).await?;
                for target in &targets {
                    let child = FsEntry::RelationTargetLink {
                        source: source.clone(),
                        relation: relation.clone(),
                        target: target.clone(),
                    };
                    let child_ino = self.inode(child).await?;
                    entries.push(DirEntry::new(
                        child_ino,
                        FileType::Symlink,
                        encode_relation_target_name(source, &targets, target)
                            .map_err(graph_error_to_errno)?,
                    ));
                }
            }
            FsEntry::PathDir { directory, .. } => {
                if let Some(children) = self
                    .graph
                    .path_children(directory)
                    .await
                    .map_err(graph_error_to_errno)?
                {
                    for child in children {
                        let kind = path_entry_file_type(&child.entry);
                        let child_entry = path_entry(child.entry, entry.clone());
                        let child_ino = self.inode(child_entry).await?;
                        entries.push(DirEntry::new(child_ino, kind, child.name.into_string()));
                    }
                }
            }
            FsEntry::WatchFile
            | FsEntry::PropertyFile(_, _)
            | FsEntry::RelationLink { .. }
            | FsEntry::RelationTargetLink { .. }
            | FsEntry::PathLink { .. } => {}
        }

        Ok(entries)
    }

    async fn relations(&self, node: &NodeId) -> std::result::Result<Vec<RelationName>, Errno> {
        match self.graph.relations(node).await {
            Ok(relations) => Ok(relations),
            Err(GraphError::NotFound { .. }) => Ok(Vec::new()),
            Err(error) => Err(graph_error_to_errno(error)),
        }
    }
}

fn path_entry(entry: GraphPathEntry, parent: FsEntry) -> FsEntry {
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

fn path_entry_file_type(entry: &GraphPathEntry) -> FileType {
    match entry {
        GraphPathEntry::Directory(_) => FileType::Directory,
        GraphPathEntry::Property { .. } => FileType::RegularFile,
        GraphPathEntry::Symlink { .. } => FileType::Symlink,
    }
}
