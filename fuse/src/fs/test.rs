use async_trait::async_trait;
use locusfs_graph::{
    DynamicGraph, GraphError, GraphWatchEvent, InMemoryProvider, LocusValue, NodeId, NodeKind,
    NodeProvider, PropertyKey, PropertyProvider, PropertySpec, RelationName, Result,
};

use super::resolve::resolve_watch_path;
use super::watch;
use super::*;

#[test]
fn stable_inodes_are_allocated_for_same_entry() {
    let mut table = InodeTable::new();
    let node = test_node("57");
    let first = table.inode(FsEntry::NodeDir(node.clone())).unwrap();
    let second = table.inode(FsEntry::NodeDir(node)).unwrap();
    assert_eq!(first, second);
}

#[test]
fn forgotten_inodes_are_removed_from_cache() {
    let mut table = InodeTable::new();
    let entry = FsEntry::NodeDir(test_node("57"));
    let ino = table.acquire(entry).unwrap();

    table.forget(ino, 1);

    assert!(table.entry(ino).is_none());
}

#[test]
fn forgotten_inodes_drop_cached_timestamps() {
    let mut table = InodeTable::new();
    let entry = FsEntry::NodeDir(test_node("57"));
    let ino = table.acquire(entry.clone()).unwrap();
    table.times(&entry);
    assert_eq!(table.times_len(), 1);

    table.forget(ino, 1);

    assert_eq!(table.times_len(), 0);
}

#[test]
fn entry_timestamps_are_stable_until_touched() {
    let mut table = InodeTable::new();
    let entry = FsEntry::NodeDir(test_node("57"));

    let first = table.times(&entry);
    let second = table.times(&entry);

    assert_eq!(first, second);
}

#[test]
fn touching_entry_updates_mtime_and_ctime() {
    let mut table = InodeTable::new();
    let entry = FsEntry::NodeDir(test_node("57"));
    let before = table.times(&entry);

    wait_for_clock_tick();
    table.touch(&entry);
    let after = table.times(&entry);

    assert_eq!(after.created, before.created);
    assert_eq!(after.accessed, before.accessed);
    assert!(after.modified > before.modified);
    assert!(after.changed > before.changed);
}

fn wait_for_clock_tick() {
    unsafe {
        libc::poll(std::ptr::null_mut(), 0, 1);
    }
}

#[test]
fn watch_registry_tracks_unread_property_changes() {
    let node = test_node("57");
    let key = PropertyKey::new("title").unwrap();
    let entry = FsEntry::PropertyFile(node.clone(), key.clone());
    let mut registry = WatchRegistry::new();
    let handle = registry.open(&entry).unwrap();

    assert!(!registry.has_unread_change(handle));
    assert!(registry.notify_property_change(&node, &key).is_empty());
    assert!(registry.has_unread_change(handle));

    registry.mark_read(handle);

    assert!(!registry.has_unread_change(handle));
}

#[test]
fn watch_registry_replaces_stale_property_poll_handles() {
    let node = test_node("57");
    let key = PropertyKey::new("title").unwrap();
    let entry = FsEntry::PropertyFile(node.clone(), key.clone());
    let mut registry = WatchRegistry::new();
    let handle = registry.open(&entry).unwrap();

    assert_eq!(
        registry
            .poll(
                handle,
                Some(10),
                fuse3::raw::flags::FUSE_POLL_SCHEDULE_NOTIFY
            )
            .unwrap(),
        0
    );
    assert_eq!(
        registry
            .poll(
                handle,
                Some(11),
                fuse3::raw::flags::FUSE_POLL_SCHEDULE_NOTIFY
            )
            .unwrap(),
        0
    );

    assert_eq!(registry.notify_property_change(&node, &key), vec![11]);
}

#[test]
fn watch_registry_tracks_unread_node_changes_for_open_properties() {
    let node = test_node("57");
    let key = PropertyKey::new("title").unwrap();
    let entry = FsEntry::PropertyFile(node.clone(), key);
    let mut registry = WatchRegistry::new();
    let handle = registry.open(&entry).unwrap();

    assert!(!registry.has_unread_change(handle));
    assert!(
        registry
            .notify_node_change(&node, WatchChange::NodeChanged(node.clone()))
            .is_empty()
    );
    assert!(registry.has_unread_change(handle));

    registry.mark_read(handle);

    assert!(!registry.has_unread_change(handle));
}

#[test]
fn watch_registry_marks_configured_watch_pending_for_subject_change() {
    let node = test_node("57");
    let key = PropertyKey::new("title").unwrap();
    let mut registry = WatchRegistry::new();
    let handle = registry.open(&FsEntry::WatchFile).unwrap();

    registry
        .configure_watch(
            handle,
            "/node/57/title".to_string(),
            watch::WatchTarget {
                subject: watch::WatchSubjectKey::Property(node.clone(), key.clone()),
                dependencies: Vec::new(),
                ready: true,
                mode: watch::WatchMode::Changes,
            },
            false,
        )
        .unwrap();

    assert!(!registry.has_unread_change(handle));
    assert!(registry.notify_property_change(&node, &key).is_empty());
    assert!(registry.has_unread_change(handle));

    let event = String::from_utf8(registry.read_watch(handle).unwrap()).unwrap();
    assert_eq!(event, "property changed node:57 title\n");
    assert!(!registry.has_unread_change(handle));
}

#[test]
fn watch_registry_reports_node_child_property_lifecycle_for_node_subject() {
    let node = test_node("57");
    let key = PropertyKey::new("title").unwrap();
    let mut registry = WatchRegistry::new();
    let handle = registry.open(&FsEntry::WatchFile).unwrap();

    registry
        .configure_watch(
            handle,
            "/node/57".to_string(),
            watch::WatchTarget {
                subject: watch::WatchSubjectKey::Node(node.clone()),
                dependencies: Vec::new(),
                ready: true,
                mode: watch::WatchMode::Changes,
            },
            false,
        )
        .unwrap();

    assert!(
        registry
            .notify_property_event(
                &node,
                &key,
                WatchChange::PropertyAdded(node.clone(), key.clone())
            )
            .is_empty()
    );

    let event = String::from_utf8(registry.read_watch(handle).unwrap()).unwrap();
    assert_eq!(event, "property added title\n");
}

#[test]
fn dependent_watch_suppresses_relation_lifecycle_events() {
    let node = test_node("57");
    let relation = RelationName::new("project").unwrap();
    let mut registry = WatchRegistry::new();
    let handle = registry.open(&FsEntry::WatchFile).unwrap();

    registry
        .configure_watch(
            handle,
            "/node/57/project".to_string(),
            watch::WatchTarget {
                subject: watch::WatchSubjectKey::NodeChild(
                    node.clone(),
                    relation.as_str().to_string(),
                ),
                dependencies: vec![watch::WatchKey::Relation(node.clone(), relation.clone())],
                ready: false,
                mode: watch::WatchMode::State,
            },
            false,
        )
        .unwrap();

    assert!(
        registry
            .notify_relation_event(
                &node,
                &relation,
                WatchChange::RelationAdded(node.clone(), relation.clone())
            )
            .is_empty()
    );
    assert!(!registry.has_unread_change(handle));
}

#[test]
fn watch_registry_can_suppress_relation_fanout_for_retargeted_watchers() {
    let node = test_node("57");
    let relation = RelationName::new("project").unwrap();
    let mut registry = WatchRegistry::new();
    let node_handle = registry.open(&FsEntry::WatchFile).unwrap();
    let child_handle = registry.open(&FsEntry::WatchFile).unwrap();

    registry
        .configure_watch(
            node_handle,
            "/node/57".to_string(),
            watch::WatchTarget {
                subject: watch::WatchSubjectKey::Node(node.clone()),
                dependencies: Vec::new(),
                ready: true,
                mode: watch::WatchMode::Changes,
            },
            false,
        )
        .unwrap();
    registry
        .configure_watch(
            child_handle,
            "/node/57/project".to_string(),
            watch::WatchTarget {
                subject: watch::WatchSubjectKey::NodeChild(
                    node.clone(),
                    relation.as_str().to_string(),
                ),
                dependencies: vec![watch::WatchKey::Relation(node.clone(), relation.clone())],
                ready: false,
                mode: watch::WatchMode::State,
            },
            false,
        )
        .unwrap();

    let excluded = [child_handle].into_iter().collect();
    registry.notify_relation_event_excluding(
        &node,
        &relation,
        WatchChange::RelationAdded(node.clone(), relation.clone()),
        &excluded,
    );

    assert!(registry.has_unread_change(node_handle));
    assert!(!registry.has_unread_change(child_handle));
    let event = String::from_utf8(registry.read_watch(node_handle).unwrap()).unwrap();
    assert_eq!(event, "relation added project\n");
}

#[test]
fn retargeted_symlink_watch_recovers_after_transient_missing_target() {
    let source = test_node("selected");
    let relation = RelationName::new("workspace").unwrap();
    let old_target = test_node("3");
    let new_target = test_node("5");
    let selected = PropertyKey::new("selected").unwrap();
    let dependency = watch::WatchKey::Relation(source.clone(), relation.clone());
    let path = "/context/selected/workspace".to_string();
    let mut registry = WatchRegistry::new();
    let handle = registry.open(&FsEntry::WatchFile).unwrap();

    registry
        .configure_watch(
            handle,
            path.clone(),
            watch::WatchTarget {
                subject: watch::WatchSubjectKey::Node(old_target),
                dependencies: vec![dependency.clone()],
                ready: true,
                mode: watch::WatchMode::State,
            },
            false,
        )
        .unwrap();

    assert_eq!(
        registry.dependent_watch_paths(&dependency),
        vec![(handle, path.clone())]
    );
    registry.apply_retarget_result(handle, path.clone(), Err(crate::errno(libc::ENOENT)), None);
    assert!(registry.has_unread_change(handle));
    assert_eq!(registry.read_watch(handle).unwrap(), b"unset\n");
    assert_eq!(
        registry.dependent_watch_paths(&dependency),
        vec![(handle, path.clone())]
    );

    registry
        .apply_retarget_result(
            handle,
            path,
            Ok(watch::WatchTarget {
                subject: watch::WatchSubjectKey::Node(new_target.clone()),
                dependencies: vec![dependency],
                ready: true,
                mode: watch::WatchMode::State,
            }),
            Some(watch::WatchState::Set(watch::WatchValue::Path(
                "/node/5".to_string(),
            ))),
        )
        .is_empty();
    assert_eq!(registry.read_watch(handle).unwrap(), b"set /node/5\n");

    assert!(
        registry
            .notify_property_event(
                &new_target,
                &selected,
                WatchChange::PropertyChanged(new_target.clone(), selected.clone()),
            )
            .is_empty()
    );
    assert!(!registry.has_unread_change(handle));
}

#[test]
fn retargeted_symlink_watch_coalesces_transient_unset_before_read() {
    let source = test_node("selected");
    let relation = RelationName::new("workspace").unwrap();
    let old_target = test_node("3");
    let new_target = test_node("5");
    let dependency = watch::WatchKey::Relation(source.clone(), relation.clone());
    let path = "/context/selected/workspace".to_string();
    let mut registry = WatchRegistry::new();
    let handle = registry.open(&FsEntry::WatchFile).unwrap();

    registry
        .configure_watch(
            handle,
            path.clone(),
            watch::WatchTarget {
                subject: watch::WatchSubjectKey::Node(old_target),
                dependencies: vec![dependency.clone()],
                ready: true,
                mode: watch::WatchMode::State,
            },
            false,
        )
        .unwrap();

    registry.apply_retarget_result(handle, path.clone(), Err(crate::errno(libc::ENOENT)), None);
    registry.apply_retarget_result(
        handle,
        path,
        Ok(watch::WatchTarget {
            subject: watch::WatchSubjectKey::Node(new_target),
            dependencies: vec![dependency],
            ready: true,
            mode: watch::WatchMode::State,
        }),
        Some(watch::WatchState::Set(watch::WatchValue::Path(
            "/node/5".to_string(),
        ))),
    );

    assert_eq!(registry.read_watch(handle).unwrap(), b"set /node/5\n");
    assert!(!registry.has_unread_change(handle));
}

#[test]
fn node_watch_reports_relation_lifecycle_for_child_subject() {
    let node = test_node("57");
    let relation = RelationName::new("project").unwrap();
    let mut registry = WatchRegistry::new();
    let handle = registry.open(&FsEntry::WatchFile).unwrap();

    registry
        .configure_watch(
            handle,
            "/node/57".to_string(),
            watch::WatchTarget {
                subject: watch::WatchSubjectKey::Node(node.clone()),
                dependencies: Vec::new(),
                ready: true,
                mode: watch::WatchMode::Changes,
            },
            false,
        )
        .unwrap();

    registry.notify_relation_event(
        &node,
        &relation,
        WatchChange::RelationAdded(node.clone(), relation.clone()),
    );

    let event = String::from_utf8(registry.read_watch(handle).unwrap()).unwrap();
    assert_eq!(event, "relation added project\n");
}

#[test]
fn concrete_property_watch_still_reports_property_changes() {
    let new_target = test_node("5");
    let selected = PropertyKey::new("selected").unwrap();
    let mut registry = WatchRegistry::new();
    let handle = registry.open(&FsEntry::WatchFile).unwrap();

    registry
        .configure_watch(
            handle,
            "/workspace/5/selected".to_string(),
            watch::WatchTarget {
                subject: watch::WatchSubjectKey::Property(new_target.clone(), selected.clone()),
                dependencies: Vec::new(),
                ready: true,
                mode: watch::WatchMode::Changes,
            },
            false,
        )
        .unwrap();

    registry.notify_property_event(
        &new_target,
        &selected,
        WatchChange::PropertyChanged(new_target.clone(), selected.clone()),
    );

    assert_eq!(
        registry.read_watch(handle).unwrap(),
        b"property changed node:5 selected\n"
    );
}

#[test]
fn watch_registry_buffers_partial_event_reads() {
    let node = test_node("57");
    let mut registry = WatchRegistry::new();
    let handle = registry.open(&FsEntry::WatchFile).unwrap();

    registry
        .configure_watch(
            handle,
            "/node/57".to_string(),
            watch::WatchTarget {
                subject: watch::WatchSubjectKey::Node(node.clone()),
                dependencies: Vec::new(),
                ready: true,
                mode: watch::WatchMode::Changes,
            },
            false,
        )
        .unwrap();
    registry.notify_node_change(&node, WatchChange::NodeChanged(node.clone()));

    assert_eq!(registry.read_watch_chunk(handle, 0, 5).unwrap(), b"node ");
    assert!(registry.has_unread_change(handle));
    assert_eq!(
        registry.read_watch_chunk(handle, 5, 8).unwrap(),
        b"changed "
    );
    assert_eq!(
        registry.read_watch_chunk(handle, 13, 64).unwrap(),
        b"node:57\n"
    );
    assert!(!registry.has_unread_change(handle));
    assert!(registry.read_watch(handle).unwrap().is_empty());
}

#[test]
fn watch_registry_drains_next_event_after_completed_offset_read() {
    let node = test_node("57");
    let mut registry = WatchRegistry::new();
    let handle = registry.open(&FsEntry::WatchFile).unwrap();

    registry
        .configure_watch(
            handle,
            "/node/57".to_string(),
            watch::WatchTarget {
                subject: watch::WatchSubjectKey::Node(node.clone()),
                dependencies: Vec::new(),
                ready: true,
                mode: watch::WatchMode::Changes,
            },
            false,
        )
        .unwrap();
    registry.notify_node_change(&node, WatchChange::NodeChanged(node.clone()));
    registry.notify_node_change(&node, WatchChange::NodeRemoved(node.clone()));

    let first = registry.read_watch_chunk(handle, 0, 4096).unwrap();
    assert_eq!(first, b"node changed node:57\n");
    let second = registry
        .read_watch_chunk(handle, first.len() as u64, 4096)
        .unwrap();
    assert_eq!(second, b"node removed node:57\n");
    assert!(!registry.has_unread_change(handle));
}

#[test]
fn watch_registry_reports_node_change_event_for_node_subject() {
    let node = test_node("57");
    let mut registry = WatchRegistry::new();
    let handle = registry.open(&FsEntry::WatchFile).unwrap();

    registry
        .configure_watch(
            handle,
            "/node/57".to_string(),
            watch::WatchTarget {
                subject: watch::WatchSubjectKey::Node(node.clone()),
                dependencies: Vec::new(),
                ready: true,
                mode: watch::WatchMode::Changes,
            },
            false,
        )
        .unwrap();

    assert!(
        registry
            .notify_node_change(&node, WatchChange::NodeChanged(node.clone()))
            .is_empty()
    );

    let event = String::from_utf8(registry.read_watch(handle).unwrap()).unwrap();
    assert_eq!(event, "node changed node:57\n");
}

#[test]
fn watch_registry_reports_node_add_event_for_kind_subject() {
    let node = test_node("57");
    let mut registry = WatchRegistry::new();
    let handle = registry.open(&FsEntry::WatchFile).unwrap();

    registry
        .configure_watch(
            handle,
            "/node".to_string(),
            watch::WatchTarget {
                subject: watch::WatchSubjectKey::Kind(test_kind()),
                dependencies: Vec::new(),
                ready: true,
                mode: watch::WatchMode::Changes,
            },
            false,
        )
        .unwrap();

    assert!(
        registry
            .notify_node_change(&node, WatchChange::NodeAdded(node.clone()))
            .is_empty()
    );

    let event = String::from_utf8(registry.read_watch(handle).unwrap()).unwrap();
    assert_eq!(event, "node added node:57\n");
}

#[test]
fn watch_registry_reports_node_remove_event_for_kind_subject() {
    let node = test_node("57");
    let mut registry = WatchRegistry::new();
    let handle = registry.open(&FsEntry::WatchFile).unwrap();

    registry
        .configure_watch(
            handle,
            "/node".to_string(),
            watch::WatchTarget {
                subject: watch::WatchSubjectKey::Kind(test_kind()),
                dependencies: Vec::new(),
                ready: true,
                mode: watch::WatchMode::Changes,
            },
            false,
        )
        .unwrap();

    assert!(
        registry
            .notify_node_change(&node, WatchChange::NodeRemoved(node.clone()))
            .is_empty()
    );

    let event = String::from_utf8(registry.read_watch(handle).unwrap()).unwrap();
    assert_eq!(event, "node removed node:57\n");
}

#[test]
fn watch_registry_reports_node_removed_event_for_node_subject() {
    let node = test_node("57");
    let mut registry = WatchRegistry::new();
    let handle = registry.open(&FsEntry::WatchFile).unwrap();

    registry
        .configure_watch(
            handle,
            "/node/57".to_string(),
            watch::WatchTarget {
                subject: watch::WatchSubjectKey::Node(node.clone()),
                dependencies: Vec::new(),
                ready: true,
                mode: watch::WatchMode::Changes,
            },
            false,
        )
        .unwrap();

    assert!(
        registry
            .notify_node_change(&node, WatchChange::NodeRemoved(node.clone()))
            .is_empty()
    );

    let event = String::from_utf8(registry.read_watch(handle).unwrap()).unwrap();
    assert_eq!(event, "node removed node:57\n");
}

#[test]
fn watch_registry_bounds_pending_events() {
    let node = test_node("57");
    let mut registry = WatchRegistry::new();
    let handle = registry.open(&FsEntry::WatchFile).unwrap();

    registry
        .configure_watch(
            handle,
            "/node/57".to_string(),
            watch::WatchTarget {
                subject: watch::WatchSubjectKey::Node(node.clone()),
                dependencies: Vec::new(),
                ready: true,
                mode: watch::WatchMode::Changes,
            },
            false,
        )
        .unwrap();

    for _ in 0..300 {
        registry.notify_node_change(&node, WatchChange::NodeChanged(node.clone()));
    }

    assert_eq!(registry.pending_event_count(handle), Some(256));
}

#[tokio::test]
async fn watch_path_can_target_kind_directory() {
    let kind = test_kind();
    let graph = DynamicGraph::new();
    graph
        .register_node_provider(InMemoryProvider::new(kind.clone()))
        .await
        .unwrap();

    let target = resolve_watch_path(&graph, "/node").await.unwrap();

    assert_eq!(target.subject, watch::WatchSubjectKey::Kind(kind));
    assert!(target.dependencies.is_empty());
    assert_eq!(target.mode, watch::WatchMode::Changes);
}

#[tokio::test]
async fn exact_property_watch_resolves_to_state_value() {
    let kind = test_kind();
    let provider = InMemoryProvider::new(kind.clone());
    let graph = DynamicGraph::new();
    graph
        .register_node_provider(provider.clone())
        .await
        .unwrap();
    graph
        .register_node_mutation_provider(kind.clone(), provider.clone())
        .await
        .unwrap();
    graph
        .register_property_provider(kind.clone(), provider.clone())
        .await
        .unwrap();
    graph
        .register_property_mutation_provider(kind, provider)
        .await
        .unwrap();

    let node = test_node("57");
    let key = PropertyKey::new("selected").unwrap();
    graph.create_node(&node).await.unwrap();
    graph
        .set_property(&node, &key, LocusValue::Bool(true))
        .await
        .unwrap();

    let (target, state) = resolve_watch_state(&graph, "/node/57/selected")
        .await
        .unwrap();

    assert_eq!(target.subject, watch::WatchSubjectKey::Property(node, key));
    assert_eq!(target.mode, watch::WatchMode::State);
    assert_eq!(
        state,
        watch::WatchState::Set(watch::WatchValue::Property("true".to_string()))
    );
}

#[tokio::test]
async fn exact_relation_watch_resolves_to_state_path() {
    let context_kind = NodeKind::new("context").unwrap();
    let workspace_kind = NodeKind::new("workspace").unwrap();
    let context_provider = InMemoryProvider::new(context_kind.clone());
    let workspace_provider = InMemoryProvider::new(workspace_kind.clone());
    let graph = DynamicGraph::new();
    graph
        .register_node_provider(context_provider.clone())
        .await
        .unwrap();
    graph
        .register_node_mutation_provider(context_kind.clone(), context_provider.clone())
        .await
        .unwrap();
    graph
        .register_relation_provider(context_kind.clone(), context_provider.clone())
        .await
        .unwrap();
    graph
        .register_relation_mutation_provider(context_kind.clone(), context_provider)
        .await
        .unwrap();
    graph
        .register_node_provider(workspace_provider.clone())
        .await
        .unwrap();
    graph
        .register_node_mutation_provider(workspace_kind.clone(), workspace_provider)
        .await
        .unwrap();

    let context = NodeId::new(context_kind, "selected").unwrap();
    let workspace = NodeId::new(workspace_kind, "3").unwrap();
    let relation = RelationName::new("workspace").unwrap();
    graph.create_node(&context).await.unwrap();
    graph.create_node(&workspace).await.unwrap();
    graph
        .set_link(&context, &relation, &workspace)
        .await
        .unwrap();

    let (target, state) = resolve_watch_state(&graph, "/context/selected/workspace")
        .await
        .unwrap();

    assert_eq!(target.subject, watch::WatchSubjectKey::Node(workspace));
    assert_eq!(
        target.dependencies,
        vec![watch::WatchKey::Relation(context, relation)]
    );
    assert_eq!(target.mode, watch::WatchMode::State);
    assert_eq!(
        state,
        watch::WatchState::Set(watch::WatchValue::Path("/workspace/3".to_string()))
    );
}

#[tokio::test]
async fn trailing_node_path_watch_resolves_to_changes() {
    let kind = test_kind();
    let provider = InMemoryProvider::new(kind.clone());
    let graph = DynamicGraph::new();
    graph
        .register_node_provider(provider.clone())
        .await
        .unwrap();
    graph
        .register_node_mutation_provider(kind, provider)
        .await
        .unwrap();

    let node = test_node("57");
    graph.create_node(&node).await.unwrap();

    let target = resolve_watch_path(&graph, "/node/57/").await.unwrap();

    assert_eq!(target.subject, watch::WatchSubjectKey::Node(node));
    assert_eq!(target.mode, watch::WatchMode::Changes);
}

#[test]
fn state_watch_messages_coalesce_to_latest_value() {
    let node = test_node("57");
    let key = PropertyKey::new("selected").unwrap();
    let mut registry = WatchRegistry::new();
    let handle = registry.open(&FsEntry::WatchFile).unwrap();

    registry
        .configure_watch(
            handle,
            "/node/57/selected".to_string(),
            watch::WatchTarget {
                subject: watch::WatchSubjectKey::Property(node, key),
                dependencies: Vec::new(),
                ready: true,
                mode: watch::WatchMode::State,
            },
            false,
        )
        .unwrap();

    registry.queue_watch_state(
        handle,
        watch::WatchState::Set(watch::WatchValue::Property("false".to_string())),
    );
    registry.queue_watch_state(
        handle,
        watch::WatchState::Set(watch::WatchValue::Property("true".to_string())),
    );

    assert_eq!(registry.read_watch(handle).unwrap(), b"set true\n");
    assert!(!registry.has_unread_change(handle));
}

#[test]
fn watch_registry_fans_out_shared_subjects_to_multiple_open_files() {
    let node = test_node("57");
    let key = PropertyKey::new("title").unwrap();
    let mut registry = WatchRegistry::new();
    let first = registry.open(&FsEntry::WatchFile).unwrap();
    let second = registry.open(&FsEntry::WatchFile).unwrap();

    let target = watch::WatchTarget {
        subject: watch::WatchSubjectKey::Property(node.clone(), key.clone()),
        dependencies: Vec::new(),
        ready: true,
        mode: watch::WatchMode::Changes,
    };
    registry
        .configure_watch(
            first,
            "/context/selected/window/title".to_string(),
            target.clone(),
            false,
        )
        .unwrap();
    registry
        .configure_watch(second, "/node/57/title".to_string(), target, false)
        .unwrap();

    registry.notify_property_change(&node, &key);

    assert!(registry.has_unread_change(first));
    assert!(registry.has_unread_change(second));
    assert_eq!(
        registry.read_watch(first).unwrap(),
        b"property changed node:57 title\n"
    );
    assert!(!registry.has_unread_change(first));
    assert!(registry.has_unread_change(second));
}

#[tokio::test]
async fn watch_path_can_target_missing_node_child_under_existing_node() {
    let kind = test_kind();
    let provider = InMemoryProvider::new(kind.clone());
    let graph = DynamicGraph::new();
    graph
        .register_node_provider(provider.clone())
        .await
        .unwrap();
    graph
        .register_node_mutation_provider(kind.clone(), provider.clone())
        .await
        .unwrap();
    graph
        .register_property_provider(kind.clone(), provider.clone())
        .await
        .unwrap();
    graph
        .register_relation_provider(kind.clone(), provider)
        .await
        .unwrap();
    let node = test_node("57");
    let relation = RelationName::new("project").unwrap();
    graph.create_node(&node).await.unwrap();

    let target = resolve_watch_path(&graph, "/node/57/project")
        .await
        .unwrap();

    assert_eq!(
        target.subject,
        watch::WatchSubjectKey::NodeChild(node.clone(), "project".to_string())
    );
    assert_eq!(
        target.dependencies,
        vec![watch::WatchKey::Relation(node, relation)]
    );
    assert!(!target.ready);
}

#[test]
fn watch_registry_replaces_stale_meta_watch_poll_handles() {
    let node = test_node("57");
    let key = PropertyKey::new("title").unwrap();
    let mut registry = WatchRegistry::new();
    let handle = registry.open(&FsEntry::WatchFile).unwrap();

    registry
        .configure_watch(
            handle,
            "/node/57/title".to_string(),
            watch::WatchTarget {
                subject: watch::WatchSubjectKey::Property(node.clone(), key.clone()),
                dependencies: Vec::new(),
                ready: true,
                mode: watch::WatchMode::Changes,
            },
            false,
        )
        .unwrap();
    assert_eq!(
        registry
            .poll(
                handle,
                Some(20),
                fuse3::raw::flags::FUSE_POLL_SCHEDULE_NOTIFY
            )
            .unwrap(),
        0
    );
    assert_eq!(
        registry
            .poll(
                handle,
                Some(21),
                fuse3::raw::flags::FUSE_POLL_SCHEDULE_NOTIFY
            )
            .unwrap(),
        0
    );

    assert_eq!(registry.notify_property_change(&node, &key), vec![21]);
}

#[tokio::test]
async fn graph_watch_forwarding_suppresses_duplicate_global_watch_events() {
    let node = test_node("57");
    let key = PropertyKey::new("title").unwrap();
    let mut registry = WatchRegistry::new();
    let handle = registry.open(&FsEntry::WatchFile).unwrap();

    registry
        .configure_watch(
            handle,
            "/node/57/title".to_string(),
            watch::WatchTarget {
                subject: watch::WatchSubjectKey::Property(node.clone(), key.clone()),
                dependencies: Vec::new(),
                ready: true,
                mode: watch::WatchMode::Changes,
            },
            true,
        )
        .unwrap();
    registry
        .set_watch_task(handle, tokio::spawn(async {}))
        .unwrap();

    assert!(registry.notify_property_change(&node, &key).is_empty());
    assert!(!registry.has_unread_change(handle));
    assert!(
        registry
            .dependent_watch_paths(&watch::WatchKey::Property(node.clone(), key.clone()))
            .is_empty()
    );

    assert_eq!(
        registry
            .poll(
                handle,
                Some(22),
                fuse3::raw::flags::FUSE_POLL_SCHEDULE_NOTIFY
            )
            .unwrap(),
        0
    );
    assert_eq!(
        registry.queue_graph_watch_event(handle, GraphWatchEvent::Change),
        vec![22]
    );
    assert_eq!(registry.read_watch(handle).unwrap(), b"change\n");
    assert!(!registry.has_unread_change(handle));
}

#[test]
fn read_slicing_respects_offset_and_size() {
    assert_eq!(slice_for_read(b"abcdef", 2, 3), b"cde");
    assert_eq!(slice_for_read(b"abcdef", 9, 3), b"");
}

#[tokio::test]
async fn truncate_property_updates_graph_value() {
    let kind = test_kind();
    let provider = InMemoryProvider::new(kind.clone());
    let graph = DynamicGraph::new();
    graph
        .register_node_provider(provider.clone())
        .await
        .unwrap();
    graph
        .register_node_mutation_provider(kind.clone(), provider.clone())
        .await
        .unwrap();
    graph
        .register_property_provider(kind.clone(), provider.clone())
        .await
        .unwrap();
    graph
        .register_property_mutation_provider(kind, provider)
        .await
        .unwrap();

    let node = test_node("57");
    let key = PropertyKey::new("title").unwrap();
    graph.create_node(&node).await.unwrap();
    graph
        .set_property(&node, &key, LocusValue::String("value".to_string()))
        .await
        .unwrap();

    let fs = LocusFs::new(graph.clone());
    fs.truncate_property(&node, &key).await.unwrap();

    assert_eq!(
        graph.property(&node, &key).await.unwrap(),
        LocusValue::String(String::new())
    );
}

#[test]
fn node_directory_permissions_follow_provider_access() {
    assert_eq!(node_dir_perm(locusfs_graph::NodeAccess::read_only()), 0o555);
    assert_eq!(
        node_dir_perm(locusfs_graph::NodeAccess::read_write()),
        0o755
    );
}

#[test]
fn relation_symlink_targets_point_back_to_node_dir() {
    let target = test_node("6");
    assert_eq!(
        direct_relation_link_target(&target),
        std::path::PathBuf::from("../../node/6")
    );
    assert_eq!(
        nested_relation_link_target(&target),
        std::path::PathBuf::from("../../../node/6")
    );
}

#[test]
fn relation_entries_are_hashable_and_stable() {
    let source = test_node("57");
    let relation = RelationName::new("linked-to").unwrap();
    let target = test_node("6");
    let mut table = InodeTable::new();
    let first = table
        .inode(FsEntry::RelationLink {
            source: source.clone(),
            relation: relation.clone(),
            target: target.clone(),
        })
        .unwrap();
    let second = table
        .inode(FsEntry::RelationLink {
            source,
            relation,
            target,
        })
        .unwrap();
    assert_eq!(first, second);
}

#[tokio::test]
async fn node_directory_lists_properties_without_relation_provider() {
    let kind = NodeKind::new("property-only").unwrap();
    let node = NodeId::new(kind.clone(), "upower").unwrap();
    let key = PropertyKey::new("active").unwrap();
    let provider = PropertyOnlyProvider {
        kind,
        node: node.clone(),
        key: key.clone(),
        value: LocusValue::Bool(true),
    };
    let graph = DynamicGraph::new();
    graph
        .register_node_provider(provider.clone())
        .await
        .unwrap();
    graph
        .register_property_provider(node.kind().clone(), provider)
        .await
        .unwrap();
    let fs = LocusFs::new(graph);

    let entries = fs.dir_entries(&FsEntry::NodeDir(node), 7).await.unwrap();
    let names = entries
        .into_iter()
        .map(|entry| entry.name)
        .collect::<Vec<_>>();

    assert!(names.contains(&".".to_string()));
    assert!(names.contains(&"..".to_string()));
    assert!(names.contains(&key.to_string()));
}

#[tokio::test]
async fn root_mkdir_creates_writable_in_memory_kind() {
    let graph = DynamicGraph::new();
    let fs = LocusFs::new(graph.clone());
    let entry = fs
        .create_kind_dir(std::ffi::OsStr::new("project"))
        .await
        .unwrap();
    let kind = NodeKind::new("project").unwrap();
    let node = NodeId::new(kind.clone(), "locusfs").unwrap();
    let key = PropertyKey::new("name").unwrap();

    assert_eq!(entry, FsEntry::KindDir(kind.clone()));
    assert!(graph.node_kinds().await.unwrap().contains(&kind));
    graph.create_node(&node).await.unwrap();
    graph
        .set_property(&node, &key, LocusValue::String("locusfs".to_string()))
        .await
        .unwrap();
    assert_eq!(
        graph.property(&node, &key).await.unwrap(),
        LocusValue::String("locusfs".to_string())
    );
}

#[tokio::test]
async fn relation_directory_uses_compact_unique_target_names() {
    let service_kind = NodeKind::new("dbus").unwrap();
    let object_kind = NodeKind::new("dbus-object").unwrap();
    let service_provider = InMemoryProvider::new(service_kind.clone());
    let object_provider = InMemoryProvider::new(object_kind.clone());
    let graph = DynamicGraph::new();
    graph
        .register_node_provider(service_provider.clone())
        .await
        .unwrap();
    graph
        .register_node_mutation_provider(service_kind.clone(), service_provider.clone())
        .await
        .unwrap();
    graph
        .register_relation_provider(service_kind.clone(), service_provider.clone())
        .await
        .unwrap();
    graph
        .register_relation_mutation_provider(service_kind, service_provider)
        .await
        .unwrap();
    graph
        .register_node_provider(object_provider.clone())
        .await
        .unwrap();
    graph
        .register_node_mutation_provider(object_kind, object_provider)
        .await
        .unwrap();

    let service = NodeId::new(NodeKind::new("dbus").unwrap(), "upower").unwrap();
    let battery = NodeId::new(
        NodeKind::new("dbus-object").unwrap(),
        "upower:devices/battery_BAT1",
    )
    .unwrap();
    let relation = RelationName::new("object").unwrap();
    graph.create_node(&service).await.unwrap();
    graph.create_node(&battery).await.unwrap();
    graph.set_link(&service, &relation, &battery).await.unwrap();
    let fs = LocusFs::new(graph);

    let entries = fs
        .dir_entries(&FsEntry::RelationDir(service.clone(), relation.clone()), 7)
        .await
        .unwrap();
    let names = entries
        .into_iter()
        .map(|entry| entry.name)
        .collect::<Vec<_>>();

    assert!(names.contains(&"battery_BAT1".to_string()));
}

#[tokio::test]
async fn relation_directory_keeps_path_when_target_basenames_collide() {
    let kind = test_kind();
    let provider = InMemoryProvider::new(kind.clone());
    let graph = DynamicGraph::new();
    graph
        .register_node_provider(provider.clone())
        .await
        .unwrap();
    graph
        .register_node_mutation_provider(kind.clone(), provider.clone())
        .await
        .unwrap();
    graph
        .register_relation_provider(kind.clone(), provider.clone())
        .await
        .unwrap();
    graph
        .register_relation_mutation_provider(kind, provider)
        .await
        .unwrap();
    let source = test_node("source");
    let first = test_node("devices/battery");
    let second = test_node("other/battery");
    let relation = RelationName::new("object").unwrap();
    graph.create_node(&source).await.unwrap();
    graph.create_node(&first).await.unwrap();
    graph.create_node(&second).await.unwrap();
    graph.set_link(&source, &relation, &first).await.unwrap();
    graph.set_link(&source, &relation, &second).await.unwrap();
    let fs = LocusFs::new(graph);

    let entries = fs
        .dir_entries(&FsEntry::RelationDir(source.clone(), relation.clone()), 7)
        .await
        .unwrap();
    let names = entries
        .into_iter()
        .map(|entry| entry.name)
        .collect::<Vec<_>>();

    assert!(names.contains(&"devices%2Fbattery".to_string()));
    assert!(names.contains(&"other%2Fbattery".to_string()));
}

fn test_kind() -> NodeKind {
    NodeKind::new("node").unwrap()
}

fn test_node(local: &str) -> NodeId {
    NodeId::new(test_kind(), local).unwrap()
}

#[derive(Clone, Debug)]
struct PropertyOnlyProvider {
    kind: NodeKind,
    node: NodeId,
    key: PropertyKey,
    value: LocusValue,
}

#[async_trait]
impl NodeProvider for PropertyOnlyProvider {
    fn kind(&self) -> &NodeKind {
        &self.kind
    }

    async fn contains_node(&self, node: &NodeId) -> Result<bool> {
        Ok(node == &self.node)
    }

    async fn nodes(&self) -> Result<Vec<NodeId>> {
        Ok(vec![self.node.clone()])
    }
}

#[async_trait]
impl PropertyProvider for PropertyOnlyProvider {
    async fn property_spec(&self, subject: &NodeId, key: &PropertyKey) -> Result<PropertySpec> {
        if subject == &self.node && key == &self.key {
            Ok(PropertySpec::new(key.clone(), self.value.kind()))
        } else {
            Err(GraphError::NotFound {
                kind: "property",
                name: format!("{subject}/{key}"),
            })
        }
    }

    async fn properties(&self, subject: &NodeId) -> Result<Vec<PropertySpec>> {
        if subject == &self.node {
            Ok(vec![PropertySpec::new(self.key.clone(), self.value.kind())])
        } else {
            Err(GraphError::NotFound {
                kind: "node",
                name: subject.to_string(),
            })
        }
    }

    async fn property(&self, subject: &NodeId, key: &PropertyKey) -> Result<LocusValue> {
        if subject == &self.node && key == &self.key {
            Ok(self.value.clone())
        } else {
            Err(GraphError::NotFound {
                kind: "property",
                name: format!("{subject}/{key}"),
            })
        }
    }
}
