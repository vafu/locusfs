use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use fuse3::Errno;
use fuse3::raw::flags::FUSE_POLL_SCHEDULE_NOTIFY;
use locusfs_graph::{GraphWatchEvent, GraphWatchTarget, NodeId, PropertyKey, RelationName};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tracing::info;

use super::entry::FsEntry;
use crate::errno;
use crate::layout::encode_segment;

pub type SharedWatchRegistry = Arc<Mutex<WatchRegistry>>;
pub type PollHandle = u64;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct FileHandle(pub u64);

const READABLE_EVENTS: u32 = libc::POLLIN as u32 | libc::POLLRDNORM as u32;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum WatchKey {
    Property(NodeId, PropertyKey),
    Relation(NodeId, RelationName),
}

pub type WatchSubjectKey = GraphWatchTarget;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WatchChange {
    Change,
    NodeAdded(NodeId),
    NodeChanged(NodeId),
    NodeRemoved(NodeId),
    PropertyAdded(NodeId, PropertyKey),
    PropertyChanged(NodeId, PropertyKey),
    PropertyRemoved(NodeId, PropertyKey),
    RelationAdded(NodeId, RelationName),
    RelationChanged(NodeId, RelationName),
    RelationRemoved(NodeId, RelationName),
}

impl From<GraphWatchEvent> for WatchChange {
    fn from(event: GraphWatchEvent) -> Self {
        match event {
            GraphWatchEvent::Change => Self::Change,
            GraphWatchEvent::NodeAdded(node) => Self::NodeAdded(node),
            GraphWatchEvent::NodeChanged(node) => Self::NodeChanged(node),
            GraphWatchEvent::NodeRemoved(node) => Self::NodeRemoved(node),
            GraphWatchEvent::PropertyAdded(node, key) => Self::PropertyAdded(node, key),
            GraphWatchEvent::PropertyChanged(node, key) => Self::PropertyChanged(node, key),
            GraphWatchEvent::PropertyRemoved(node, key) => Self::PropertyRemoved(node, key),
            GraphWatchEvent::RelationAdded(node, relation) => Self::RelationAdded(node, relation),
            GraphWatchEvent::RelationChanged(node, relation) => {
                Self::RelationChanged(node, relation)
            }
            GraphWatchEvent::RelationRemoved(node, relation) => {
                Self::RelationRemoved(node, relation)
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WatchMessage {
    State(WatchState),
    Change(WatchChange),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WatchState {
    Unset,
    Set(WatchValue),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WatchValue {
    Path(String),
    Property(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WatchMode {
    State,
    Changes,
}

const MAX_PENDING_WATCH_EVENTS: usize = 256;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WatchTarget {
    pub subject: WatchSubjectKey,
    pub dependencies: Vec<WatchKey>,
    pub ready: bool,
    pub mode: WatchMode,
}

#[derive(Debug)]
pub struct WatchRegistry {
    next_handle: u64,
    open_files: HashMap<u64, OpenFileState>,
    property_watches: HashMap<WatchKey, PropertyWatchState>,
    subjects: HashMap<WatchSubjectKey, WatchSubject>,
    dependency_index: HashMap<WatchKey, Vec<FileHandle>>,
}

#[derive(Debug)]
enum OpenFileState {
    PropertyPoll { key: WatchKey, seen_generation: u64 },
    Watch(WatchHandle),
}

#[derive(Debug, Default)]
struct PropertyWatchState {
    generation: u64,
    poll_handles: Vec<(FileHandle, PollHandle)>,
}

#[derive(Debug, Default)]
struct WatchHandle {
    original_path: Option<String>,
    resolved_subject: Option<WatchSubjectKey>,
    dependencies: Vec<WatchKey>,
    mode: WatchMode,
    uses_graph_watch: bool,
    watch_task: Option<JoinHandle<()>>,
    pending_events: VecDeque<WatchMessage>,
    poll_handles: Vec<PollHandle>,
}

impl Default for WatchMode {
    fn default() -> Self {
        Self::Changes
    }
}

#[derive(Debug, Default)]
struct WatchSubject {
    watchers: Vec<FileHandle>,
}

impl WatchRegistry {
    pub fn shared() -> SharedWatchRegistry {
        Arc::new(Mutex::new(Self::new()))
    }

    pub fn new() -> Self {
        Self {
            next_handle: 1,
            open_files: HashMap::new(),
            property_watches: HashMap::new(),
            subjects: HashMap::new(),
            dependency_index: HashMap::new(),
        }
    }

    pub fn open(&mut self, entry: &FsEntry) -> std::result::Result<FileHandle, Errno> {
        let state = match property_poll_key(entry) {
            Some(key) => {
                let seen_generation = self
                    .property_watches
                    .entry(key.clone())
                    .or_default()
                    .generation;
                OpenFileState::PropertyPoll {
                    key,
                    seen_generation,
                }
            }
            None if matches!(entry, FsEntry::WatchFile) => {
                OpenFileState::Watch(WatchHandle::default())
            }
            None => return Err(errno(libc::EINVAL)),
        };
        let handle = self.next_handle;
        self.next_handle = self
            .next_handle
            .checked_add(1)
            .ok_or(errno(libc::EOVERFLOW))?;
        self.open_files.insert(handle, state);
        Ok(FileHandle(handle))
    }

    pub fn mark_read(&mut self, handle: FileHandle) {
        let Some(OpenFileState::PropertyPoll {
            key,
            seen_generation,
        }) = self.open_files.get_mut(&handle.0)
        else {
            return;
        };
        *seen_generation = self
            .property_watches
            .get(key)
            .map(|state| state.generation)
            .unwrap_or(0);
    }

    pub fn configure_watch(
        &mut self,
        handle: FileHandle,
        path: String,
        target: WatchTarget,
        uses_graph_watch: bool,
    ) -> std::result::Result<(), Errno> {
        if !matches!(
            self.open_files.get(&handle.0),
            Some(OpenFileState::Watch(_))
        ) {
            return Err(errno(libc::EBADF));
        }

        self.detach_watch(handle);
        let Some(OpenFileState::Watch(watch)) = self.open_files.get_mut(&handle.0) else {
            return Err(errno(libc::EBADF));
        };

        watch.original_path = Some(path);
        watch.resolved_subject = Some(target.subject.clone());
        watch.dependencies = target.dependencies.clone();
        watch.mode = target.mode;
        watch.uses_graph_watch = uses_graph_watch;
        watch.pending_events.clear();
        self.attach_watch(handle, &target.subject, &target.dependencies);
        Ok(())
    }

    pub fn set_watch_task(
        &mut self,
        handle: FileHandle,
        task: JoinHandle<()>,
    ) -> std::result::Result<(), Errno> {
        let Some(OpenFileState::Watch(watch)) = self.open_files.get_mut(&handle.0) else {
            task.abort();
            return Err(errno(libc::EBADF));
        };
        if let Some(previous) = watch.watch_task.replace(task) {
            previous.abort();
        }
        Ok(())
    }

    pub fn read_watch(&mut self, handle: FileHandle) -> std::result::Result<Vec<u8>, Errno> {
        let Some(OpenFileState::Watch(watch)) = self.open_files.get_mut(&handle.0) else {
            return Err(errno(libc::EINVAL));
        };
        if let Some(message) = watch.pending_events.pop_front() {
            let value = watch_message_bytes(message, watch.resolved_subject.as_ref());
            let path = watch.original_path.as_deref().unwrap_or("<unconfigured>");
            info!("{} >>> {}", path, format_watch_value(&value));
            Ok(value)
        } else {
            Ok(Vec::new())
        }
    }

    pub fn poll(
        &mut self,
        handle: FileHandle,
        kh: Option<PollHandle>,
        flags: u32,
    ) -> std::result::Result<u32, Errno> {
        let Some(open_file) = self.open_files.get(&handle.0) else {
            return Err(errno(libc::EBADF));
        };

        match open_file {
            OpenFileState::PropertyPoll {
                key,
                seen_generation,
            } => {
                let state = self.property_watches.entry(key.clone()).or_default();
                if state.generation > *seen_generation {
                    return Ok(READABLE_EVENTS);
                }

                if flags & FUSE_POLL_SCHEDULE_NOTIFY != 0 {
                    if let Some(kh) = kh {
                        state
                            .poll_handles
                            .retain(|(poll_file, _)| *poll_file != handle);
                        state.poll_handles.push((handle, kh));
                    }
                }
                Ok(0)
            }
            OpenFileState::Watch(_) => {
                let Some(OpenFileState::Watch(watch)) = self.open_files.get_mut(&handle.0) else {
                    return Err(errno(libc::EBADF));
                };
                if !watch.pending_events.is_empty() {
                    return Ok(READABLE_EVENTS);
                }
                if flags & FUSE_POLL_SCHEDULE_NOTIFY != 0 {
                    if let Some(kh) = kh {
                        watch.poll_handles.clear();
                        watch.poll_handles.push(kh);
                    }
                }
                Ok(0)
            }
        }
    }

    pub fn release(&mut self, handle: FileHandle) {
        self.detach_watch(handle);
        self.remove_property_poll_handles(handle);
        self.open_files.remove(&handle.0);
    }

    #[cfg(test)]
    pub fn notify_property_change(&mut self, node: &NodeId, key: &PropertyKey) -> Vec<PollHandle> {
        self.notify_property_event(
            node,
            key,
            WatchChange::PropertyChanged(node.clone(), key.clone()),
        )
    }

    pub fn notify_property_event(
        &mut self,
        node: &NodeId,
        key: &PropertyKey,
        event: WatchChange,
    ) -> Vec<PollHandle> {
        let watch_key = WatchKey::Property(node.clone(), key.clone());
        let mut handles = self.notify_property_poll_change(&watch_key);
        handles.extend(self.notify_subject_change(
            &GraphWatchTarget::Property(node.clone(), key.clone()),
            event.clone(),
        ));
        handles.extend(self.notify_subject_change(
            &GraphWatchTarget::NodeChild(node.clone(), key.as_str().to_string()),
            event.clone(),
        ));
        handles.extend(self.notify_subject_change(&GraphWatchTarget::Node(node.clone()), event));
        handles
    }

    #[cfg(test)]
    pub fn notify_relation_event(
        &mut self,
        source: &NodeId,
        relation: &RelationName,
        event: WatchChange,
    ) -> Vec<PollHandle> {
        self.notify_relation_event_excluding(source, relation, event, &HashSet::new())
    }

    pub fn notify_relation_event_excluding(
        &mut self,
        source: &NodeId,
        relation: &RelationName,
        event: WatchChange,
        excluded_watchers: &HashSet<FileHandle>,
    ) -> Vec<PollHandle> {
        let watch_key = WatchKey::Relation(source.clone(), relation.clone());
        let mut handles = self.notify_property_poll_change(&watch_key);
        handles.extend(self.notify_subject_change_excluding(
            &GraphWatchTarget::Relation(source.clone(), relation.clone()),
            event.clone(),
            excluded_watchers,
        ));
        handles.extend(self.notify_subject_change_excluding(
            &GraphWatchTarget::NodeChild(source.clone(), relation.as_str().to_string()),
            event.clone(),
            excluded_watchers,
        ));
        handles.extend(self.notify_subject_change_excluding(
            &GraphWatchTarget::Node(source.clone()),
            event,
            excluded_watchers,
        ));
        handles
    }

    pub fn notify_node_change(&mut self, node: &NodeId, event: WatchChange) -> Vec<PollHandle> {
        let mut handles = Vec::new();
        let keys = self
            .property_watches
            .keys()
            .filter(|key| match key {
                WatchKey::Property(watched_node, _) => watched_node == node,
                WatchKey::Relation(watched_node, _) => watched_node == node,
            })
            .cloned()
            .collect::<Vec<_>>();
        for key in keys {
            handles.extend(self.notify_property_poll_change(&key));
        }

        let node_subjects = self
            .subjects
            .keys()
            .filter(|subject| match subject {
                GraphWatchTarget::Kind(_) => false,
                GraphWatchTarget::Node(watched_node) => watched_node == node,
                GraphWatchTarget::Property(_, _) => false,
                GraphWatchTarget::NodeChild(_, _) | GraphWatchTarget::Relation(_, _) => false,
            })
            .cloned()
            .collect::<Vec<_>>();
        for subject in node_subjects {
            handles.extend(self.notify_subject_change(&subject, event.clone()));
        }

        if matches!(event, WatchChange::NodeRemoved(_)) {
            let child_subjects = self
                .subjects
                .keys()
                .filter(|subject| match subject {
                    GraphWatchTarget::Kind(_) | GraphWatchTarget::Node(_) => false,
                    GraphWatchTarget::Property(watched_node, _)
                    | GraphWatchTarget::NodeChild(watched_node, _)
                    | GraphWatchTarget::Relation(watched_node, _) => watched_node == node,
                })
                .cloned()
                .collect::<Vec<_>>();
            for subject in child_subjects {
                handles.extend(self.notify_subject_change(&subject, WatchChange::Change));
            }
        }

        let kind_subject = GraphWatchTarget::Kind(node.kind().clone());
        handles.extend(self.notify_subject_change(&kind_subject, event));

        handles
    }

    pub fn notify_all(&mut self) -> Vec<PollHandle> {
        let mut handles = Vec::new();
        let keys = self.property_watches.keys().cloned().collect::<Vec<_>>();
        for key in keys {
            handles.extend(self.notify_property_poll_change(&key));
        }

        let watchers = self
            .open_files
            .iter()
            .filter_map(|(handle, state)| match state {
                OpenFileState::Watch(_) => Some(FileHandle(*handle)),
                OpenFileState::PropertyPoll { .. } => None,
            })
            .collect::<Vec<_>>();
        for watcher in watchers {
            handles.extend(self.queue_watch_change(watcher, WatchChange::Change, false));
        }
        handles
    }

    pub fn queue_graph_watch_event(
        &mut self,
        handle: FileHandle,
        event: GraphWatchEvent,
    ) -> Vec<PollHandle> {
        self.queue_watch_change(handle, event.into(), true)
    }

    pub fn dependent_watch_paths(&self, dependency: &WatchKey) -> Vec<(FileHandle, String)> {
        self.dependency_index
            .get(dependency)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|handle| self.watch_original_path(handle).map(|path| (handle, path)))
            .collect()
    }

    pub fn apply_retarget_result(
        &mut self,
        handle: FileHandle,
        path: String,
        result: std::result::Result<WatchTarget, Errno>,
        state: Option<WatchState>,
    ) -> Vec<PollHandle> {
        let previous_mode = self.watch_mode(handle).unwrap_or_default();
        match result {
            Ok(target) => {
                self.detach_watch(handle);
                let ready = target.ready;
                let mode = target.mode;
                if let Some(OpenFileState::Watch(watch)) = self.open_files.get_mut(&handle.0) {
                    watch.original_path = Some(path);
                    watch.resolved_subject = Some(target.subject.clone());
                    watch.dependencies = target.dependencies.clone();
                    watch.mode = mode;
                }
                self.attach_watch(handle, &target.subject, &target.dependencies);
                return match mode {
                    WatchMode::State => self.queue_watch_state(
                        handle,
                        state.unwrap_or_else(|| fallback_state_for_target(&target, ready)),
                    ),
                    WatchMode::Changes => {
                        self.queue_watch_change(handle, WatchChange::Change, false)
                    }
                };
            }
            Err(_) => {
                self.detach_subject(handle);
                if matches!(previous_mode, WatchMode::State) {
                    return self.queue_watch_state(handle, WatchState::Unset);
                }
            }
        }
        self.queue_watch_change(handle, WatchChange::Change, false)
    }

    fn notify_property_poll_change(&mut self, key: &WatchKey) -> Vec<PollHandle> {
        let state = self.property_watches.entry(key.clone()).or_default();
        state.generation = state.generation.saturating_add(1);
        std::mem::take(&mut state.poll_handles)
            .into_iter()
            .map(|(_, kh)| kh)
            .collect()
    }

    fn notify_subject_change(
        &mut self,
        subject: &WatchSubjectKey,
        event: WatchChange,
    ) -> Vec<PollHandle> {
        self.notify_subject_change_excluding(subject, event, &HashSet::new())
    }

    fn notify_subject_change_excluding(
        &mut self,
        subject: &WatchSubjectKey,
        event: WatchChange,
        excluded_watchers: &HashSet<FileHandle>,
    ) -> Vec<PollHandle> {
        let watchers = self
            .subjects
            .get(subject)
            .map(|subject| subject.watchers.clone())
            .unwrap_or_default();
        let mut handles = Vec::new();
        for watcher in watchers {
            if excluded_watchers.contains(&watcher) {
                continue;
            }
            handles.extend(self.queue_watch_change(watcher, event.clone(), false));
        }
        handles
    }

    pub fn queue_watch_state(&mut self, handle: FileHandle, state: WatchState) -> Vec<PollHandle> {
        let Some(OpenFileState::Watch(watch)) = self.open_files.get_mut(&handle.0) else {
            return Vec::new();
        };
        if !matches!(watch.mode, WatchMode::State) {
            return Vec::new();
        }
        watch
            .pending_events
            .retain(|pending| !matches!(pending, WatchMessage::State(_)));
        if watch.pending_events.len() >= MAX_PENDING_WATCH_EVENTS {
            watch.pending_events.pop_front();
        }
        watch.pending_events.push_back(WatchMessage::State(state));
        std::mem::take(&mut watch.poll_handles)
    }

    fn queue_watch_change(
        &mut self,
        handle: FileHandle,
        event: WatchChange,
        from_graph_watch: bool,
    ) -> Vec<PollHandle> {
        let Some(OpenFileState::Watch(watch)) = self.open_files.get_mut(&handle.0) else {
            return Vec::new();
        };
        if !matches!(watch.mode, WatchMode::Changes) {
            return Vec::new();
        }
        if watch.uses_graph_watch && !from_graph_watch {
            return Vec::new();
        }
        if watch.pending_events.len() >= MAX_PENDING_WATCH_EVENTS {
            watch.pending_events.pop_front();
        }
        watch.pending_events.push_back(WatchMessage::Change(event));
        std::mem::take(&mut watch.poll_handles)
    }

    fn watch_original_path(&self, handle: FileHandle) -> Option<String> {
        let Some(OpenFileState::Watch(watch)) = self.open_files.get(&handle.0) else {
            return None;
        };
        watch.original_path.clone()
    }

    pub fn state_watch_paths_for_subject(
        &self,
        subject: &WatchSubjectKey,
    ) -> Vec<(FileHandle, String)> {
        self.subjects
            .get(subject)
            .map(|subject| subject.watchers.clone())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|handle| {
                let Some(OpenFileState::Watch(watch)) = self.open_files.get(&handle.0) else {
                    return None;
                };
                if !matches!(watch.mode, WatchMode::State) {
                    return None;
                }
                watch.original_path.clone().map(|path| (handle, path))
            })
            .collect()
    }

    fn watch_mode(&self, handle: FileHandle) -> Option<WatchMode> {
        match self.open_files.get(&handle.0)? {
            OpenFileState::Watch(watch) => Some(watch.mode),
            OpenFileState::PropertyPoll { .. } => None,
        }
    }

    fn attach_watch(
        &mut self,
        handle: FileHandle,
        subject: &WatchSubjectKey,
        dependencies: &[WatchKey],
    ) {
        self.subjects
            .entry(subject.clone())
            .or_default()
            .watchers
            .push(handle);
        for dependency in dependencies {
            self.dependency_index
                .entry(dependency.clone())
                .or_default()
                .push(handle);
        }
    }

    fn detach_watch(&mut self, handle: FileHandle) {
        self.detach_subject(handle);
        self.detach_dependencies(handle);
    }

    fn remove_property_poll_handles(&mut self, handle: FileHandle) {
        for state in self.property_watches.values_mut() {
            state
                .poll_handles
                .retain(|(poll_file, _)| *poll_file != handle);
        }
    }

    fn detach_subject(&mut self, handle: FileHandle) {
        let subject = match self.open_files.get(&handle.0) {
            Some(OpenFileState::Watch(watch)) => watch.resolved_subject.clone(),
            _ => None,
        };
        let Some(subject) = subject else {
            return;
        };
        if let Some(entry) = self.subjects.get_mut(&subject) {
            entry.watchers.retain(|watcher| *watcher != handle);
            if entry.watchers.is_empty() {
                self.subjects.remove(&subject);
            }
        }
        if let Some(OpenFileState::Watch(watch)) = self.open_files.get_mut(&handle.0) {
            watch.resolved_subject = None;
            watch.uses_graph_watch = false;
            if let Some(task) = watch.watch_task.take() {
                task.abort();
            }
        }
    }

    fn detach_dependencies(&mut self, handle: FileHandle) {
        let dependencies = match self.open_files.get(&handle.0) {
            Some(OpenFileState::Watch(watch)) => watch.dependencies.clone(),
            _ => Vec::new(),
        };
        for dependency in dependencies {
            if let Some(handles) = self.dependency_index.get_mut(&dependency) {
                handles.retain(|candidate| *candidate != handle);
                if handles.is_empty() {
                    self.dependency_index.remove(&dependency);
                }
            }
        }
        if let Some(OpenFileState::Watch(watch)) = self.open_files.get_mut(&handle.0) {
            watch.dependencies.clear();
        }
    }

    #[cfg(test)]
    pub fn has_unread_change(&self, handle: FileHandle) -> bool {
        let Some(open_file) = self.open_files.get(&handle.0) else {
            return false;
        };
        match open_file {
            OpenFileState::PropertyPoll {
                key,
                seen_generation,
            } => self
                .property_watches
                .get(key)
                .is_some_and(|state| state.generation > *seen_generation),
            OpenFileState::Watch(watch) => !watch.pending_events.is_empty(),
        }
    }

    #[cfg(test)]
    pub fn pending_event_count(&self, handle: FileHandle) -> Option<usize> {
        match self.open_files.get(&handle.0)? {
            OpenFileState::Watch(watch) => Some(watch.pending_events.len()),
            OpenFileState::PropertyPoll { .. } => None,
        }
    }
}

fn watch_message_bytes(message: WatchMessage, subject: Option<&WatchSubjectKey>) -> Vec<u8> {
    match message {
        WatchMessage::State(state) => watch_state_bytes(state),
        WatchMessage::Change(change) => watch_change_bytes(change, subject),
    }
}

fn watch_state_bytes(state: WatchState) -> Vec<u8> {
    match state {
        WatchState::Unset => b"unset\n".to_vec(),
        WatchState::Set(value) => format!("set {}\n", watch_value_payload(value)).into_bytes(),
    }
}

fn watch_value_payload(value: WatchValue) -> String {
    match value {
        WatchValue::Path(path) | WatchValue::Property(path) => path,
    }
}

fn watch_change_bytes(change: WatchChange, subject: Option<&WatchSubjectKey>) -> Vec<u8> {
    match change {
        WatchChange::Change => b"change\n".to_vec(),
        WatchChange::NodeAdded(node) => format!("node added {node}\n").into_bytes(),
        WatchChange::NodeChanged(node) => format!("node changed {node}\n").into_bytes(),
        WatchChange::NodeRemoved(node) => format!("node removed {node}\n").into_bytes(),
        WatchChange::PropertyAdded(node, key) => property_event_bytes("added", node, key, subject),
        WatchChange::PropertyChanged(node, key) => {
            property_event_bytes("changed", node, key, subject)
        }
        WatchChange::PropertyRemoved(node, key) => {
            property_event_bytes("removed", node, key, subject)
        }
        WatchChange::RelationAdded(source, relation) => {
            relation_event_bytes("added", source, relation, subject)
        }
        WatchChange::RelationChanged(source, relation) => {
            relation_event_bytes("changed", source, relation, subject)
        }
        WatchChange::RelationRemoved(source, relation) => {
            relation_event_bytes("removed", source, relation, subject)
        }
    }
}

pub(crate) fn watch_subject_path(subject: &WatchSubjectKey) -> String {
    match subject {
        GraphWatchTarget::Kind(kind) => {
            format!(
                "/{}",
                encode_segment(kind.as_str()).expect("validated node kind should encode")
            )
        }
        GraphWatchTarget::Node(node) => node_path(node),
        GraphWatchTarget::NodeChild(node, name) => {
            format!(
                "{}/{}",
                node_path(node),
                encode_segment(name).expect("validated child name should encode")
            )
        }
        GraphWatchTarget::Property(node, key) => {
            format!(
                "{}/{}",
                node_path(node),
                encode_segment(key.as_str()).expect("validated property key should encode")
            )
        }
        GraphWatchTarget::Relation(node, relation) => {
            format!(
                "{}/{}",
                node_path(node),
                encode_segment(relation.as_str()).expect("validated relation name should encode")
            )
        }
    }
}

fn node_path(node: &NodeId) -> String {
    format!(
        "/{}/{}",
        encode_segment(node.kind().as_str()).expect("validated node kind should encode"),
        encode_segment(node.local()).expect("validated node local id should encode")
    )
}

fn fallback_state_for_target(target: &WatchTarget, ready: bool) -> WatchState {
    if ready {
        WatchState::Set(WatchValue::Path(watch_subject_path(&target.subject)))
    } else {
        WatchState::Unset
    }
}

fn property_event_bytes(
    action: &str,
    node: NodeId,
    key: PropertyKey,
    subject: Option<&WatchSubjectKey>,
) -> Vec<u8> {
    match subject {
        Some(GraphWatchTarget::Node(watched)) if watched == &node => {
            format!("property {action} {key}\n").into_bytes()
        }
        _ => format!("property {action} {node} {key}\n").into_bytes(),
    }
}

fn relation_event_bytes(
    action: &str,
    source: NodeId,
    relation: RelationName,
    subject: Option<&WatchSubjectKey>,
) -> Vec<u8> {
    match subject {
        Some(GraphWatchTarget::Node(watched)) if watched == &source => {
            format!("relation {action} {relation}\n").into_bytes()
        }
        _ => format!("relation {action} {source} {relation}\n").into_bytes(),
    }
}

fn property_poll_key(entry: &FsEntry) -> Option<WatchKey> {
    match entry {
        FsEntry::PropertyFile(node, key) => Some(WatchKey::Property(node.clone(), key.clone())),
        _ => None,
    }
}

fn format_watch_value(value: &[u8]) -> String {
    String::from_utf8_lossy(value).escape_debug().to_string()
}
