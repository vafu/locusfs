use std::collections::BTreeSet;

use fuser::{Errno, FileType};

use super::entry::{DirEntry, FsEntry, WATCH_FILE_NAME, parent_entry};
use super::filesystem::LocusFs;
use super::name::encode_relation_target_name;
use crate::graph_error_to_errno;
use crate::layout::encode_segment;

impl LocusFs {
    pub(super) fn dir_entries(
        &self,
        entry: &FsEntry,
        ino: u64,
    ) -> std::result::Result<Vec<DirEntry>, Errno> {
        let mut entries = vec![
            DirEntry::new(ino, FileType::Directory, "."),
            DirEntry::new(self.inode(parent_entry(entry))?, FileType::Directory, ".."),
        ];

        match entry {
            FsEntry::Root => {
                let child = FsEntry::WatchFile;
                let child_ino = self.inode(child)?;
                entries.push(DirEntry::new(
                    child_ino,
                    FileType::RegularFile,
                    WATCH_FILE_NAME,
                ));

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
                let mut names = BTreeSet::new();
                for spec in self.graph.properties(node).map_err(graph_error_to_errno)? {
                    let key = spec.key();
                    let name = encode_segment(key.as_str()).map_err(graph_error_to_errno)?;
                    if !names.insert(name.clone()) {
                        return Err(Errno::EIO);
                    }
                    let child = FsEntry::PropertyFile(node.clone(), key.clone());
                    let child_ino = self.inode(child)?;
                    entries.push(DirEntry::new(child_ino, FileType::RegularFile, name));
                }

                for relation in self.graph.relations(node).map_err(graph_error_to_errno)? {
                    let name = encode_segment(relation.as_str()).map_err(graph_error_to_errno)?;
                    if !names.insert(name.clone()) {
                        return Err(Errno::EIO);
                    }
                    let targets = self.relation_targets(node, &relation)?;
                    match targets.as_slice() {
                        [] => {}
                        [target] => {
                            let child = FsEntry::RelationLink {
                                source: node.clone(),
                                relation: relation.clone(),
                                target: target.clone(),
                            };
                            let child_ino = self.inode(child)?;
                            entries.push(DirEntry::new(child_ino, FileType::Symlink, name.clone()));
                        }
                        _ => {
                            let child = FsEntry::RelationDir(node.clone(), relation.clone());
                            let child_ino = self.inode(child)?;
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
                for target in self.relation_targets(source, relation)? {
                    let child = FsEntry::RelationTargetLink {
                        source: source.clone(),
                        relation: relation.clone(),
                        target: target.clone(),
                    };
                    let child_ino = self.inode(child)?;
                    entries.push(DirEntry::new(
                        child_ino,
                        FileType::Symlink,
                        encode_relation_target_name(&target).map_err(graph_error_to_errno)?,
                    ));
                }
            }
            FsEntry::WatchFile
            | FsEntry::PropertyFile(_, _)
            | FsEntry::RelationLink { .. }
            | FsEntry::RelationTargetLink { .. } => {}
        }

        Ok(entries)
    }
}
