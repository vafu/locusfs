use std::collections::HashSet;

use locusfs_graph::{DynamicGraph, GraphChange, GraphChangeReceiver, GraphWatchTarget};
use tokio::sync::{broadcast, oneshot};
use tokio::task::JoinHandle;

use crate::fs::{
    FsEntry, SharedInodeTable, SharedKernelNotify, SharedWatchRegistry, WatchChange, WatchKey,
    resolve_watch_state,
};
use crate::layout::encode_segment;

#[derive(Debug)]
pub(crate) struct InvalidationWorker {
    shutdown: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
}

impl InvalidationWorker {
    pub fn shutdown(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

impl Drop for InvalidationWorker {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub(crate) fn spawn_change_invalidator(
    mut changes: GraphChangeReceiver,
    notifier: SharedKernelNotify,
    graph: DynamicGraph,
    inodes: SharedInodeTable,
    watch: SharedWatchRegistry,
) -> InvalidationWorker {
    let (shutdown, mut shutdown_receiver) = oneshot::channel();
    let task = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = &mut shutdown_receiver => break,
                received = changes.recv() => {
                    match received {
                        Ok(change) => {
                            invalidate_change(
                                notifier.clone(),
                                graph.clone(),
                                inodes.clone(),
                                watch.clone(),
                                change,
                            )
                            .await;
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => {
                            resync_known_state(notifier.clone(), inodes.clone(), watch.clone())
                                .await;
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
        }
    });

    InvalidationWorker {
        shutdown: Some(shutdown),
        task: Some(task),
    }
}

async fn invalidate_change(
    notifier: SharedKernelNotify,
    graph: DynamicGraph,
    inodes: SharedInodeTable,
    watch: SharedWatchRegistry,
    change: GraphChange,
) {
    match change {
        GraphChange::NodeKindChanged { kind } => {
            let name = match encode_segment(kind.as_str()) {
                Ok(name) => name,
                Err(_) => return,
            };
            invalidate_known_child(notifier.clone(), inodes.clone(), FsEntry::Root, name).await;
            invalidate_known_inode(notifier.clone(), inodes.clone(), FsEntry::KindDir(kind)).await;
        }
        GraphChange::NodeAdded { node } => {
            notify_node_watchers(
                notifier.clone(),
                watch.clone(),
                node.clone(),
                WatchChange::NodeAdded(node.clone()),
            )
            .await;
            refresh_node_state_watchers(
                notifier.clone(),
                graph.clone(),
                watch.clone(),
                node.clone(),
            )
            .await;
            let parent = FsEntry::KindDir(node.kind().clone());
            let name = match encode_segment(node.local()) {
                Ok(name) => name,
                Err(_) => return,
            };
            invalidate_known_child(notifier.clone(), inodes.clone(), parent, name).await;
            invalidate_known_inode(
                notifier.clone(),
                inodes.clone(),
                FsEntry::NodeDir(node.clone()),
            )
            .await;
        }
        GraphChange::NodeChanged { node } => {
            notify_node_watchers(
                notifier.clone(),
                watch.clone(),
                node.clone(),
                WatchChange::NodeChanged(node.clone()),
            )
            .await;
            refresh_node_state_watchers(
                notifier.clone(),
                graph.clone(),
                watch.clone(),
                node.clone(),
            )
            .await;
            invalidate_known_inode(
                notifier.clone(),
                inodes.clone(),
                FsEntry::NodeDir(node.clone()),
            )
            .await;
        }
        GraphChange::NodeRemoved { node } => {
            notify_node_watchers(
                notifier.clone(),
                watch.clone(),
                node.clone(),
                WatchChange::NodeRemoved(node.clone()),
            )
            .await;
            refresh_node_state_watchers(
                notifier.clone(),
                graph.clone(),
                watch.clone(),
                node.clone(),
            )
            .await;
            let parent = FsEntry::KindDir(node.kind().clone());
            let name = match encode_segment(node.local()) {
                Ok(name) => name,
                Err(_) => return,
            };
            invalidate_known_child(notifier.clone(), inodes.clone(), parent, name).await;
            invalidate_known_inode(
                notifier.clone(),
                inodes.clone(),
                FsEntry::NodeDir(node.clone()),
            )
            .await;
        }
        GraphChange::PropertyChanged { node, key } => {
            invalidate_property_change(
                notifier,
                graph,
                inodes,
                watch,
                node,
                key,
                WatchChange::PropertyChanged,
                false,
            )
            .await;
        }
        GraphChange::PropertyAdded { node, key } => {
            invalidate_property_change(
                notifier,
                graph,
                inodes,
                watch,
                node,
                key,
                WatchChange::PropertyAdded,
                true,
            )
            .await;
        }
        GraphChange::PropertyRemoved { node, key } => {
            invalidate_property_change(
                notifier,
                graph,
                inodes,
                watch,
                node,
                key,
                WatchChange::PropertyRemoved,
                true,
            )
            .await;
        }
        GraphChange::RelationChanged { source, relation } => {
            invalidate_relation_change(
                notifier,
                graph,
                inodes,
                watch,
                source,
                relation,
                WatchChange::RelationChanged,
                false,
            )
            .await;
        }
        GraphChange::RelationAdded { source, relation } => {
            invalidate_relation_change(
                notifier,
                graph,
                inodes,
                watch,
                source,
                relation,
                WatchChange::RelationAdded,
                true,
            )
            .await;
        }
        GraphChange::RelationRemoved { source, relation } => {
            invalidate_relation_change(
                notifier,
                graph,
                inodes,
                watch,
                source,
                relation,
                WatchChange::RelationRemoved,
                true,
            )
            .await;
        }
    }
}

async fn invalidate_property_change(
    notifier: SharedKernelNotify,
    graph: DynamicGraph,
    inodes: SharedInodeTable,
    watch: SharedWatchRegistry,
    node: locusfs_graph::NodeId,
    key: locusfs_graph::PropertyKey,
    event: fn(locusfs_graph::NodeId, locusfs_graph::PropertyKey) -> WatchChange,
    invalidate_parent: bool,
) {
    let parent = FsEntry::NodeDir(node.clone());
    notify_property_watchers(
        notifier.clone(),
        watch.clone(),
        node.clone(),
        key.clone(),
        event(node.clone(), key.clone()),
    )
    .await;
    refresh_state_watchers_for_subject(
        notifier.clone(),
        graph.clone(),
        watch.clone(),
        GraphWatchTarget::Property(node.clone(), key.clone()),
    )
    .await;
    refresh_state_watchers_for_subject(
        notifier.clone(),
        graph.clone(),
        watch.clone(),
        GraphWatchTarget::NodeChild(node.clone(), key.as_str().to_string()),
    )
    .await;

    if invalidate_parent {
        let name = match encode_segment(key.as_str()) {
            Ok(name) => name,
            Err(_) => return,
        };
        invalidate_known_child(notifier.clone(), inodes.clone(), parent.clone(), name).await;
        invalidate_known_inode(notifier.clone(), inodes.clone(), parent).await;
    }

    invalidate_known_inode(
        notifier.clone(),
        inodes.clone(),
        FsEntry::PropertyFile(node.clone(), key.clone()),
    )
    .await;
}

async fn invalidate_relation_change(
    notifier: SharedKernelNotify,
    graph: DynamicGraph,
    inodes: SharedInodeTable,
    watch: SharedWatchRegistry,
    source: locusfs_graph::NodeId,
    relation: locusfs_graph::RelationName,
    event: fn(locusfs_graph::NodeId, locusfs_graph::RelationName) -> WatchChange,
    invalidate_parent: bool,
) {
    let watch_event = event(source.clone(), relation.clone());

    retarget_relation_watchers(
        notifier.clone(),
        graph,
        watch,
        source.clone(),
        relation.clone(),
        watch_event,
    )
    .await;

    if invalidate_parent {
        let parent = FsEntry::NodeDir(source.clone());
        let name = match encode_segment(relation.as_str()) {
            Ok(name) => name,
            Err(_) => return,
        };
        invalidate_known_child(notifier.clone(), inodes.clone(), parent.clone(), name).await;
        invalidate_known_inode(notifier.clone(), inodes.clone(), parent).await;
    }

    // For plain target changes, relation-dependent watches receive the new
    // state directly and the short entry TTL covers non-watch readlink users.
    // Invalidating the symlink inode here can block in FUSE reverse
    // invalidation while clients are resolving that same symlink.
    if invalidate_parent {
        invalidate_matching_relation_inodes(notifier, inodes, source, relation).await;
    }
}

async fn resync_known_state(
    notifier: SharedKernelNotify,
    inodes: SharedInodeTable,
    watch: SharedWatchRegistry,
) {
    let entries = {
        let mut inodes = inodes.lock().await;
        let entries = inodes.entries();
        for (entry, _) in &entries {
            inodes.touch(entry);
        }
        entries
    };

    for (_, ino) in entries {
        if let Some(notifier) = current_notifier(notifier.clone()).await {
            notifier.invalid_inode(ino, 0, 0).await;
        }
    }

    let handles = {
        let mut watch = watch.lock().await;
        watch.notify_all()
    };
    notify_poll_handles(notifier, handles).await;
}

async fn invalidate_known_child(
    notifier: SharedKernelNotify,
    inodes: SharedInodeTable,
    parent: FsEntry,
    _name: String,
) {
    let parent_ino = {
        let mut inodes = inodes.lock().await;
        inodes.touch(&parent);
        let Some(parent_ino) = inodes.known_inode(&parent) else {
            return;
        };
        parent_ino
    };
    if let Some(notifier) = current_notifier(notifier).await {
        notifier.invalid_inode(parent_ino, 0, 0).await;
    }
}

async fn notify_property_watchers(
    notifier: SharedKernelNotify,
    watch: SharedWatchRegistry,
    node: locusfs_graph::NodeId,
    key: locusfs_graph::PropertyKey,
    event: WatchChange,
) -> bool {
    let handles = {
        let mut watch = watch.lock().await;
        watch.notify_property_event(&node, &key, event)
    };

    notify_poll_handles(notifier, handles).await
}

async fn notify_node_watchers(
    notifier: SharedKernelNotify,
    watch: SharedWatchRegistry,
    node: locusfs_graph::NodeId,
    event: WatchChange,
) -> bool {
    let handles = {
        let mut watch = watch.lock().await;
        watch.notify_node_change(&node, event)
    };

    notify_poll_handles(notifier, handles).await
}

async fn retarget_relation_watchers(
    notifier: SharedKernelNotify,
    graph: DynamicGraph,
    watch: SharedWatchRegistry,
    source: locusfs_graph::NodeId,
    relation: locusfs_graph::RelationName,
    event: WatchChange,
) -> bool {
    let key = WatchKey::Relation(source.clone(), relation.clone());
    let paths = {
        let watch = watch.lock().await;
        watch.dependent_watch_paths(&key)
    };
    let retargeted_handles = paths
        .iter()
        .map(|(handle, _)| *handle)
        .collect::<HashSet<_>>();

    let handles = {
        let mut watch = watch.lock().await;
        watch.notify_relation_event_excluding(
            &source,
            &relation,
            event.clone(),
            &retargeted_handles,
        )
    };
    let mut had_poll_waiters = notify_poll_handles(notifier.clone(), handles).await;
    had_poll_waiters |= refresh_state_watchers_for_subject(
        notifier.clone(),
        graph.clone(),
        watch.clone(),
        GraphWatchTarget::Relation(source.clone(), relation.clone()),
    )
    .await;
    had_poll_waiters |= refresh_state_watchers_for_subject(
        notifier.clone(),
        graph.clone(),
        watch.clone(),
        GraphWatchTarget::NodeChild(source.clone(), relation.as_str().to_string()),
    )
    .await;

    had_poll_waiters |= !paths.is_empty();
    for (handle, path) in paths {
        let result = resolve_watch_state(&graph, &path).await;
        let (target, state) = match result {
            Ok((target, state)) => (Ok(target), Some(state)),
            Err(error) => (Err(error), None),
        };
        let handles = {
            let mut watch = watch.lock().await;
            watch.apply_retarget_result(handle, path, target, state)
        };
        had_poll_waiters |= notify_poll_handles(notifier.clone(), handles).await;
    }
    had_poll_waiters
}

async fn refresh_state_watchers_for_subject(
    notifier: SharedKernelNotify,
    graph: DynamicGraph,
    watch: SharedWatchRegistry,
    subject: GraphWatchTarget,
) -> bool {
    let paths = {
        let watch = watch.lock().await;
        watch.state_watch_paths_for_subject(&subject)
    };

    let mut had_poll_waiters = false;
    for (handle, path) in paths {
        let result = resolve_watch_state(&graph, &path).await;
        let (target, state) = match result {
            Ok((target, state)) => (Ok(target), Some(state)),
            Err(error) => (Err(error), None),
        };
        let handles = {
            let mut watch = watch.lock().await;
            watch.apply_retarget_result(handle, path, target, state)
        };
        had_poll_waiters |= notify_poll_handles(notifier.clone(), handles).await;
    }
    had_poll_waiters
}

async fn refresh_node_state_watchers(
    notifier: SharedKernelNotify,
    graph: DynamicGraph,
    watch: SharedWatchRegistry,
    node: locusfs_graph::NodeId,
) -> bool {
    let node_subject = GraphWatchTarget::Node(node.clone());
    let kind_subject = GraphWatchTarget::Kind(node.kind().clone());

    let mut had_poll_waiters = refresh_state_watchers_for_subject(
        notifier.clone(),
        graph.clone(),
        watch.clone(),
        node_subject,
    )
    .await;
    had_poll_waiters |=
        refresh_state_watchers_for_subject(notifier, graph, watch, kind_subject).await;
    had_poll_waiters
}

async fn notify_poll_handles(notifier: SharedKernelNotify, handles: Vec<u64>) -> bool {
    let had_handles = !handles.is_empty();
    for handle in handles {
        if let Some(notifier) = current_notifier(notifier.clone()).await {
            notifier.wakeup(handle).await;
        }
    }
    had_handles
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

async fn invalidate_known_inode(
    notifier: SharedKernelNotify,
    inodes: SharedInodeTable,
    entry: FsEntry,
) {
    let ino = {
        let mut inodes = inodes.lock().await;
        let Some(ino) = inodes.touch(&entry) else {
            return;
        };
        ino
    };
    if let Some(notifier) = current_notifier(notifier).await {
        notifier.invalid_inode(ino, 0, 0).await;
    }
}

async fn invalidate_matching_relation_inodes(
    notifier: SharedKernelNotify,
    inodes: SharedInodeTable,
    source: locusfs_graph::NodeId,
    relation: locusfs_graph::RelationName,
) {
    let entries = {
        let mut inodes = inodes.lock().await;
        let entries = inodes.entries();
        for (entry, _) in &entries {
            if relation_entry_matches(entry, &source, &relation) {
                inodes.touch(entry);
            }
        }
        entries
    };

    for (entry, ino) in entries {
        if relation_entry_matches(&entry, &source, &relation) {
            if let Some(notifier) = current_notifier(notifier.clone()).await {
                notifier.invalid_inode(ino, 0, 0).await;
            }
        }
    }
}

async fn current_notifier(notifier: SharedKernelNotify) -> Option<fuse3::notify::Notify> {
    notifier.lock().await.clone()
}
