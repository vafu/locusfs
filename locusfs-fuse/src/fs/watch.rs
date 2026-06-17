use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use fuser::{Errno, FileHandle, PollEvents, PollFlags, PollNotifier};
use locusfs_graph::{NodeId, PropertyKey, RelationName};

use super::entry::FsEntry;

pub type SharedWatchRegistry = Arc<Mutex<WatchRegistry>>;

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
    poll_handles: Vec<fuser::PollHandle>,
}

#[derive(Debug, Default)]
struct WatchHandle {
    original_path: Option<String>,
    resolved_subject: Option<WatchSubjectKey>,
    dependencies: Vec<WatchKey>,
    pending: bool,
    poll_handles: Vec<fuser::PollHandle>,
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
            None => return Err(Errno::EINVAL),
        };
        let handle = self.next_handle;
        self.next_handle = self.next_handle.checked_add(1).ok_or(Errno::EOVERFLOW)?;
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
            return Err(Errno::EBADF);
        }

        self.detach_watch(handle);
        let Some(OpenFileState::Watch(watch)) = self.open_files.get_mut(&handle.0) else {
            return Err(Errno::EBADF);
        };

        watch.original_path = Some(path);
        watch.resolved_subject = Some(target.subject.clone());
        watch.dependencies = target.dependencies.clone();
        watch.pending = false;
        self.attach_watch(handle, &target.subject, &target.dependencies);
        Ok(())
    }

    pub fn read_watch(&mut self, handle: FileHandle) -> std::result::Result<Vec<u8>, Errno> {
        let Some(OpenFileState::Watch(watch)) = self.open_files.get_mut(&handle.0) else {
            return Err(Errno::EINVAL);
        };
        if watch.pending {
            watch.pending = false;
            Ok(b"change\n".to_vec())
        } else {
            Ok(Vec::new())
        }
    }

    pub fn poll(
        &mut self,
        handle: FileHandle,
        notifier: PollNotifier,
        flags: PollFlags,
    ) -> std::result::Result<PollEvents, Errno> {
        let Some(open_file) = self.open_files.get(&handle.0) else {
            return Err(Errno::EBADF);
        };

        match open_file {
            OpenFileState::PropertyPoll {
                key,
                seen_generation,
            } => {
                let state = self.property_watches.entry(key.clone()).or_default();
                if state.generation > *seen_generation {
                    return Ok(PollEvents::POLLIN | PollEvents::POLLRDNORM);
                }

                if flags.contains(PollFlags::FUSE_POLL_SCHEDULE_NOTIFY) {
                    state.poll_handles.push(notifier.handle());
                }
                Ok(PollEvents::empty())
            }
            OpenFileState::Watch(watch) if watch.pending => {
                Ok(PollEvents::POLLIN | PollEvents::POLLRDNORM)
            }
            OpenFileState::Watch(_) => {
                let Some(OpenFileState::Watch(watch)) = self.open_files.get_mut(&handle.0) else {
                    return Err(Errno::EBADF);
                };
                if flags.contains(PollFlags::FUSE_POLL_SCHEDULE_NOTIFY) {
                    watch.poll_handles.push(notifier.handle());
                }
                Ok(PollEvents::empty())
            }
        }
    }

    pub fn release(&mut self, handle: FileHandle) {
        self.detach_watch(handle);
        self.open_files.remove(&handle.0);
    }

    pub fn notify_property_change(
        &mut self,
        node: &NodeId,
        key: &PropertyKey,
    ) -> Vec<fuser::PollHandle> {
        let watch_key = WatchKey::Property(node.clone(), key.clone());
        let mut handles = self.notify_property_poll_change(&watch_key);
        handles.extend(
            self.notify_subject_change(&WatchSubjectKey::Property(node.clone(), key.clone())),
        );
        handles
    }

    pub fn notify_node_change(&mut self, node: &NodeId) -> Vec<fuser::PollHandle> {
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

        let subjects = self
            .subjects
            .keys()
            .filter(|subject| match subject {
                WatchSubjectKey::Node(watched_node) => watched_node == node,
                WatchSubjectKey::Property(watched_node, _) => watched_node == node,
            })
            .cloned()
            .collect::<Vec<_>>();
        for subject in subjects {
            handles.extend(self.notify_subject_change(&subject));
        }

        handles
    }

    pub fn retarget_dependents(
        &mut self,
        dependency: &WatchKey,
        mut resolve: impl FnMut(&str) -> std::result::Result<WatchTarget, Errno>,
    ) -> Vec<fuser::PollHandle> {
        let handles = self
            .dependency_index
            .get(dependency)
            .cloned()
            .unwrap_or_default();
        let mut poll_handles = Vec::new();

        for handle in handles {
            let Some(path) = self.watch_original_path(handle) else {
                continue;
            };
            match resolve(&path) {
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
                }
            }
            poll_handles.extend(self.mark_watch_pending(handle));
        }

        poll_handles
    }

    fn notify_property_poll_change(&mut self, key: &WatchKey) -> Vec<fuser::PollHandle> {
        let state = self.property_watches.entry(key.clone()).or_default();
        state.generation = state.generation.saturating_add(1);
        std::mem::take(&mut state.poll_handles)
    }

    fn notify_subject_change(&mut self, subject: &WatchSubjectKey) -> Vec<fuser::PollHandle> {
        let watchers = self
            .subjects
            .get(subject)
            .map(|subject| subject.watchers.clone())
            .unwrap_or_default();
        let mut handles = Vec::new();
        for watcher in watchers {
            handles.extend(self.mark_watch_pending(watcher));
        }
        handles
    }

    fn mark_watch_pending(&mut self, handle: FileHandle) -> Vec<fuser::PollHandle> {
        let Some(OpenFileState::Watch(watch)) = self.open_files.get_mut(&handle.0) else {
            return Vec::new();
        };
        watch.pending = true;
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
            OpenFileState::Watch(watch) => watch.pending,
        }
    }
}

fn property_poll_key(entry: &FsEntry) -> Option<WatchKey> {
    match entry {
        FsEntry::PropertyFile(node, key) => Some(WatchKey::Property(node.clone(), key.clone())),
        _ => None,
    }
}
