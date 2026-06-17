use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use fuse3::Errno;
use fuse3::raw::flags::FUSE_POLL_SCHEDULE_NOTIFY;
use locusfs_graph::{NodeId, PropertyKey, RelationName};
use tokio::sync::Mutex;
use tracing::info;

use super::entry::FsEntry;
use crate::errno;

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

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum WatchSubjectKey {
    Node(NodeId),
    Property(NodeId, PropertyKey),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WatchTarget {
    pub subject: WatchSubjectKey,
    pub dependencies: Vec<WatchKey>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WatchEvent {
    Change,
    NodeChanged(NodeId),
    NodeRemoved(NodeId),
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
    pending_events: VecDeque<WatchEvent>,
    poll_handles: Vec<PollHandle>,
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
        watch.pending_events.clear();
        self.attach_watch(handle, &target.subject, &target.dependencies);
        Ok(())
    }

    pub fn read_watch(&mut self, handle: FileHandle) -> std::result::Result<Vec<u8>, Errno> {
        let Some(OpenFileState::Watch(watch)) = self.open_files.get_mut(&handle.0) else {
            return Err(errno(libc::EINVAL));
        };
        if let Some(event) = watch.pending_events.pop_front() {
            let value = event.into_bytes();
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
            OpenFileState::Watch(watch) if !watch.pending_events.is_empty() => Ok(READABLE_EVENTS),
            OpenFileState::Watch(_) => {
                let Some(OpenFileState::Watch(watch)) = self.open_files.get_mut(&handle.0) else {
                    return Err(errno(libc::EBADF));
                };
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

    pub fn notify_property_change(&mut self, node: &NodeId, key: &PropertyKey) -> Vec<PollHandle> {
        let watch_key = WatchKey::Property(node.clone(), key.clone());
        let mut handles = self.notify_property_poll_change(&watch_key);
        handles.extend(self.notify_subject_change(
            &WatchSubjectKey::Property(node.clone(), key.clone()),
            WatchEvent::Change,
        ));
        handles
    }

    pub fn notify_node_change(&mut self, node: &NodeId, event: WatchEvent) -> Vec<PollHandle> {
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
                WatchSubjectKey::Node(watched_node) => watched_node == node,
                WatchSubjectKey::Property(_, _) => false,
            })
            .cloned()
            .collect::<Vec<_>>();
        for subject in node_subjects {
            handles.extend(self.notify_subject_change(&subject, event.clone()));
        }

        let property_subjects = self
            .subjects
            .keys()
            .filter(|subject| match subject {
                WatchSubjectKey::Node(_) => false,
                WatchSubjectKey::Property(watched_node, _) => watched_node == node,
            })
            .cloned()
            .collect::<Vec<_>>();
        for subject in property_subjects {
            handles.extend(self.notify_subject_change(&subject, WatchEvent::Change));
        }

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
            handles.extend(self.queue_watch_event(watcher, WatchEvent::Change));
        }
        handles
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
    ) -> Vec<PollHandle> {
        match result {
            Ok(target) => {
                self.detach_watch(handle);
                if let Some(OpenFileState::Watch(watch)) = self.open_files.get_mut(&handle.0) {
                    watch.original_path = Some(path);
                    watch.resolved_subject = Some(target.subject.clone());
                    watch.dependencies = target.dependencies.clone();
                }
                self.attach_watch(handle, &target.subject, &target.dependencies);
            }
            Err(_) => {
                self.detach_subject(handle);
                return Vec::new();
            }
        }
        self.queue_watch_event(handle, WatchEvent::Change)
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
        event: WatchEvent,
    ) -> Vec<PollHandle> {
        let watchers = self
            .subjects
            .get(subject)
            .map(|subject| subject.watchers.clone())
            .unwrap_or_default();
        let mut handles = Vec::new();
        for watcher in watchers {
            handles.extend(self.queue_watch_event(watcher, event.clone()));
        }
        handles
    }

    fn queue_watch_event(&mut self, handle: FileHandle, event: WatchEvent) -> Vec<PollHandle> {
        let Some(OpenFileState::Watch(watch)) = self.open_files.get_mut(&handle.0) else {
            return Vec::new();
        };
        watch.pending_events.push_back(event);
        std::mem::take(&mut watch.poll_handles)
    }

    fn watch_original_path(&self, handle: FileHandle) -> Option<String> {
        let Some(OpenFileState::Watch(watch)) = self.open_files.get(&handle.0) else {
            return None;
        };
        watch.original_path.clone()
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
}

impl WatchEvent {
    fn into_bytes(self) -> Vec<u8> {
        match self {
            Self::Change => b"change\n".to_vec(),
            Self::NodeChanged(node) => format!("node changed {node}\n").into_bytes(),
            Self::NodeRemoved(node) => format!("node removed {node}\n").into_bytes(),
        }
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
