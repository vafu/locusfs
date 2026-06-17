use async_trait::async_trait;
use locusfs_graph::{
    DynamicGraph, GraphError, LocusValue, NodeId, NodeKind, NodeProvider, PropertyKey,
    PropertyProvider, PropertySpec, RelationName, Result,
};

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
            .notify_node_change(&node, WatchEvent::NodeChanged(node.clone()))
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
            },
        )
        .unwrap();

    assert!(!registry.has_unread_change(handle));
    assert!(registry.notify_property_change(&node, &key).is_empty());
    assert!(registry.has_unread_change(handle));

    let event = String::from_utf8(registry.read_watch(handle).unwrap()).unwrap();
    assert_eq!(event, "change\n");
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
            },
        )
        .unwrap();

    assert!(
        registry
            .notify_node_change(&node, WatchEvent::NodeChanged(node.clone()))
            .is_empty()
    );

    let event = String::from_utf8(registry.read_watch(handle).unwrap()).unwrap();
    assert_eq!(event, "node changed node:57\n");
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
            },
        )
        .unwrap();

    assert!(
        registry
            .notify_node_change(&node, WatchEvent::NodeRemoved(node.clone()))
            .is_empty()
    );

    let event = String::from_utf8(registry.read_watch(handle).unwrap()).unwrap();
    assert_eq!(event, "node removed node:57\n");
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
    };
    registry
        .configure_watch(
            first,
            "/context/selected/window/title".to_string(),
            target.clone(),
        )
        .unwrap();
    registry
        .configure_watch(second, "/node/57/title".to_string(), target)
        .unwrap();

    registry.notify_property_change(&node, &key);

    assert!(registry.has_unread_change(first));
    assert!(registry.has_unread_change(second));
    assert_eq!(registry.read_watch(first).unwrap(), b"change\n");
    assert!(!registry.has_unread_change(first));
    assert!(registry.has_unread_change(second));
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
            },
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

#[test]
fn read_slicing_respects_offset_and_size() {
    assert_eq!(slice_for_read(b"abcdef", 2, 3), b"cde");
    assert_eq!(slice_for_read(b"abcdef", 9, 3), b"");
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
