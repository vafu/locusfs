use std::ffi::OsString;
use std::sync::mpsc::Receiver;
use std::thread::{self, JoinHandle};

use fuser::{INodeNo, Notifier};
use locusfs_graph::{DynamicGraph, GraphChange};

use crate::fs::{FsEntry, SharedInodeTable, SharedWatchRegistry, WatchKey, resolve_watch_path};
use crate::layout::encode_segment;

pub(crate) fn spawn_change_invalidator(
    changes: Receiver<GraphChange>,
    notifier: Notifier,
    graph: DynamicGraph,
    inodes: SharedInodeTable,
    watch: SharedWatchRegistry,
) -> JoinHandle<()> {
    thread::Builder::new()
        .name("locusfs-fuse-invalidator".to_string())
        .spawn(move || {
            for change in changes {
                invalidate_change(&notifier, &graph, &inodes, &watch, change);
            }
        })
        .expect("spawn FUSE invalidation worker")
}

fn invalidate_change(
    notifier: &Notifier,
    graph: &DynamicGraph,
    inodes: &SharedInodeTable,
    watch: &SharedWatchRegistry,
    change: GraphChange,
) {
    match change {
        GraphChange::NodeKindChanged { kind } => {
            let name = match encode_segment(kind.as_str()) {
                Ok(name) => name,
                Err(_) => return,
            };
            invalidate_known_child(notifier, inodes, &FsEntry::Root, name);
            invalidate_known_inode(notifier, inodes, &FsEntry::KindDir(kind));
        }
        GraphChange::NodeChanged { node } | GraphChange::NodeRemoved { node } => {
            let parent = FsEntry::KindDir(node.kind().clone());
            let name = match encode_segment(node.local()) {
                Ok(name) => name,
                Err(_) => return,
            };
            invalidate_known_child(notifier, inodes, &parent, name);
            invalidate_known_inode(notifier, inodes, &FsEntry::NodeDir(node.clone()));
            notify_node_watchers(notifier, watch, &node);
        }
        GraphChange::PropertyChanged { node, key } => {
            let name = match encode_segment(key.as_str()) {
                Ok(name) => name,
                Err(_) => return,
            };
            let parent = FsEntry::NodeDir(node.clone());
            invalidate_known_child(notifier, inodes, &parent, name.clone());
            invalidate_known_inode(notifier, inodes, &parent);
            invalidate_known_inode(
                notifier,
                inodes,
                &FsEntry::PropertyFile(node.clone(), key.clone()),
            );
            notify_property_watchers(notifier, watch, &node, &key);
        }
        GraphChange::RelationChanged { source, relation } => {
            let parent = FsEntry::NodeDir(source.clone());
            let name = match encode_segment(relation.as_str()) {
                Ok(name) => name,
                Err(_) => return,
            };
            notify_relation_entries_deleted(notifier, inodes, &source, &relation);
            invalidate_known_child(notifier, inodes, &parent, name);
            invalidate_known_inode(notifier, inodes, &parent);
            invalidate_matching_relation_inodes(notifier, inodes, &source, &relation);
            retarget_relation_watchers(notifier, graph, watch, &source, &relation);
        }
    }
}

fn invalidate_known_child(
    notifier: &Notifier,
    inodes: &SharedInodeTable,
    parent: &FsEntry,
    name: String,
) {
    let Ok(mut inodes) = inodes.lock() else {
        return;
    };
    inodes.touch(parent);
    let Some(parent_ino) = inodes.known_inode(parent) else {
        return;
    };
    drop(inodes);
    invalidate_entry(notifier, INodeNo(parent_ino), name);
}

fn invalidate_entry(notifier: &Notifier, parent: INodeNo, name: String) {
    let name = OsString::from(name);
    let _ = notifier.inval_entry(parent, &name);
}

fn notify_relation_entries_deleted(
    notifier: &Notifier,
    inodes: &SharedInodeTable,
    source: &locusfs_graph::NodeId,
    relation: &locusfs_graph::RelationName,
) {
    let Ok(inodes) = inodes.lock() else {
        return;
    };
    let entries = inodes.entries();
    let mut deletes = Vec::new();

    for (entry, child_ino) in &entries {
        let Some((parent, name)) = relation_entry_parent_and_name(entry) else {
            continue;
        };
        if !relation_entry_matches(entry, source, relation) {
            continue;
        }
        let Some(parent_ino) = inodes.known_inode(&parent) else {
            continue;
        };
        deletes.push((parent_ino, *child_ino, name));
    }
    drop(inodes);

    for (parent_ino, child_ino, name) in deletes {
        let name = OsString::from(name);
        let _ = notifier.delete(INodeNo(parent_ino), INodeNo(child_ino), &name);
    }
}

fn relation_entry_parent_and_name(entry: &FsEntry) -> Option<(FsEntry, String)> {
    match entry {
        FsEntry::RelationDir(source, relation)
        | FsEntry::RelationLink {
            source, relation, ..
        } => encode_segment(relation.as_str())
            .ok()
            .map(|name| (FsEntry::NodeDir(source.clone()), name)),
        FsEntry::RelationTargetLink {
            source,
            relation,
            target,
        } => encode_segment(&target.to_string())
            .ok()
            .map(|name| (FsEntry::RelationDir(source.clone(), relation.clone()), name)),
        _ => None,
    }
}

fn relation_entry_matches(
    entry: &FsEntry,
    source: &locusfs_graph::NodeId,
    relation: &locusfs_graph::RelationName,
) -> bool {
    match entry {
        FsEntry::RelationDir(entry_source, entry_relation)
        | FsEntry::RelationLink {
            source: entry_source,
            relation: entry_relation,
            ..
        }
        | FsEntry::RelationTargetLink {
            source: entry_source,
            relation: entry_relation,
            ..
        } => entry_source == source && entry_relation == relation,
        _ => false,
    }
}

fn notify_property_watchers(
    notifier: &Notifier,
    watch: &SharedWatchRegistry,
    node: &locusfs_graph::NodeId,
    key: &locusfs_graph::PropertyKey,
) {
    let Ok(mut watch) = watch.lock() else {
        return;
    };
    let handles = watch.notify_property_change(node, key);
    drop(watch);

    notify_poll_handles(notifier, handles);
}

fn notify_node_watchers(
    notifier: &Notifier,
    watch: &SharedWatchRegistry,
    node: &locusfs_graph::NodeId,
) {
    let Ok(mut watch) = watch.lock() else {
        return;
    };
    let handles = watch.notify_node_change(node);
    drop(watch);

    notify_poll_handles(notifier, handles);
}

fn retarget_relation_watchers(
    notifier: &Notifier,
    graph: &DynamicGraph,
    watch: &SharedWatchRegistry,
    source: &locusfs_graph::NodeId,
    relation: &locusfs_graph::RelationName,
) {
    let key = WatchKey::Relation(source.clone(), relation.clone());
    let Ok(mut watch) = watch.lock() else {
        return;
    };
    let handles = watch.retarget_dependents(&key, |path| resolve_watch_path(graph, path));
    drop(watch);

    notify_poll_handles(notifier, handles);
}

fn notify_poll_handles(notifier: &Notifier, handles: Vec<fuser::PollHandle>) {
    for handle in handles {
        let _ = notifier.poll(handle);
    }
}

fn invalidate_known_inode(notifier: &Notifier, inodes: &SharedInodeTable, entry: &FsEntry) {
    let Ok(mut inodes) = inodes.lock() else {
        return;
    };
    let Some(ino) = inodes.touch(entry) else {
        return;
    };
    drop(inodes);
    let _ = notifier.inval_inode(INodeNo(ino), 0, 0);
}

fn invalidate_matching_relation_inodes(
    notifier: &Notifier,
    inodes: &SharedInodeTable,
    source: &locusfs_graph::NodeId,
    relation: &locusfs_graph::RelationName,
) {
    let Ok(mut inodes) = inodes.lock() else {
        return;
    };
    let entries = inodes.entries();
    for (entry, _) in &entries {
        if relation_entry_matches(entry, source, relation) {
            inodes.touch(entry);
        }
    }
    drop(inodes);

    for (entry, ino) in entries {
        if relation_entry_matches(&entry, source, relation) {
            let _ = notifier.inval_inode(INodeNo(ino), 0, 0);
        }
    }
}
