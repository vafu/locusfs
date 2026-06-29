//! Client helpers for reading and watching values exposed by a locusfs mount.

use std::io;
use std::os::fd::{AsRawFd, OwnedFd};
use std::path::{Component, Path, PathBuf};

use tokio::io::unix::AsyncFd;
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use tokio::time::{Duration, sleep, timeout};
use tracing::info;

use crate::{WatchEvent, WatchState};

const DEFAULT_READ_TIMEOUT: Duration = Duration::from_secs(5);

/// Reads a locusfs path into memory.
pub async fn read(path: impl AsRef<Path>) -> io::Result<Vec<u8>> {
    tokio::fs::read(path).await
}

/// Reads a UTF-8 locusfs path into a string.
pub async fn read_to_string(path: impl AsRef<Path>) -> io::Result<String> {
    bytes_to_string(read(path).await?)
}

/// Lists the UTF-8 file names in a locusfs directory.
pub async fn read_dir_names(path: impl AsRef<Path>) -> io::Result<Vec<String>> {
    let mut entries = tokio::fs::read_dir(path).await?;
    let mut names = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        let name = entry.file_name().into_string().map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "locusfs directory entry name is not valid UTF-8",
            )
        })?;
        names.push(name);
    }
    Ok(names)
}

/// Resolves a locusfs symlink target to an absolute data path.
pub async fn read_link(path: impl AsRef<Path>) -> io::Result<PathBuf> {
    let path = path.as_ref();
    let target = tokio::fs::read_link(path).await?;
    Ok(resolve_link_target(path, target))
}

/// Returns whether a locusfs path currently exists.
pub async fn exists(path: impl AsRef<Path>) -> bool {
    tokio::fs::metadata(path).await.is_ok()
}

fn bytes_to_string(value: Vec<u8>) -> io::Result<String> {
    String::from_utf8(value).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("locusfs value is not valid UTF-8: {error}"),
        )
    })
}

fn resolve_link_target(path: &Path, target: PathBuf) -> PathBuf {
    if target.is_absolute() {
        target
    } else {
        path.parent().unwrap_or_else(|| Path::new("/")).join(target)
    }
}

/// Converts a path into an absolute path without resolving symlinks.
pub fn absolute_path(path: impl AsRef<Path>) -> io::Result<PathBuf> {
    let path = path.as_ref();
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

/// Finds the nearest locusfs mount root by walking upward until `/watch` exists.
pub async fn find_mount_root(path: impl AsRef<Path>) -> io::Result<PathBuf> {
    let path = path.as_ref();
    for ancestor in path.ancestors() {
        let watch = ancestor.join("watch");
        if tokio::fs::metadata(watch)
            .await
            .is_ok_and(|metadata| metadata.is_file())
        {
            return Ok(ancestor.to_path_buf());
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!("could not find locusfs watch file above {}", path.display()),
    ))
}

/// Converts an absolute mount path into the logical path accepted by `/watch`.
pub fn logical_watch_path(
    mount_root: impl AsRef<Path>,
    path: impl AsRef<Path>,
) -> io::Result<String> {
    let mount_root = mount_root.as_ref();
    let path = path.as_ref();
    let relative = path.strip_prefix(mount_root).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} is not under {}", path.display(), mount_root.display()),
        )
    })?;
    if relative.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::Prefix(_) | Component::RootDir
        )
    }) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("path escapes mount root: {}", path.display()),
        ));
    }
    let relative = relative.to_str().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("path is not valid UTF-8: {}", relative.display()),
        )
    })?;
    Ok(format!("/{relative}"))
}

/// A single `/watch` file handle subscribed to one locusfs data path.
#[derive(Debug)]
pub struct Watch {
    data_path: PathBuf,
    mount_root: PathBuf,
    logical_path: String,
    watch_file: AsyncFd<OwnedFd>,
    raw_event_buffer: Vec<u8>,
}

impl Watch {
    /// Opens `/watch` for `path` and subscribes this handle to that path.
    pub async fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let data_path = absolute_path(path)?;
        let mount_root = find_mount_root(&data_path).await?;
        let logical_path = logical_watch_path(&mount_root, &data_path)?;
        Self::open_with_parts(data_path, mount_root, logical_path).await
    }

    /// Opens a watch from already resolved path parts.
    pub async fn open_with_parts(
        data_path: PathBuf,
        mount_root: PathBuf,
        logical_path: String,
    ) -> io::Result<Self> {
        let mut watch_file = tokio::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(mount_root.join("watch"))
            .await?;
        let subscription = format!("{logical_path}\n");
        watch_file.write_all(subscription.as_bytes()).await?;
        info!("<<<< {}", logical_path);
        watch_file.seek(std::io::SeekFrom::Start(0)).await?;
        let watch_file = OwnedFd::from(watch_file.into_std().await);
        set_nonblocking(&watch_file)?;
        let watch_file = AsyncFd::new(watch_file)?;

        Ok(Self {
            data_path,
            mount_root,
            logical_path,
            watch_file,
            raw_event_buffer: Vec::new(),
        })
    }

    /// Returns the data path this handle reads after wakeups.
    pub fn data_path(&self) -> &Path {
        &self.data_path
    }

    /// Returns the detected locusfs mount root.
    pub fn mount_root(&self) -> &Path {
        &self.mount_root
    }

    /// Returns the logical path registered with `/watch`.
    pub fn logical_path(&self) -> &str {
        &self.logical_path
    }

    /// Waits until this subscription receives a change notification.
    pub async fn wait(&mut self) -> io::Result<()> {
        self.wait_event().await.map(|_| ())
    }

    /// Waits until this subscription receives a typed watch event.
    pub async fn wait_event(&mut self) -> io::Result<WatchEvent> {
        self.next_event().await
    }

    /// Waits until this subscription receives a typed watch event.
    pub async fn next_event(&mut self) -> io::Result<WatchEvent> {
        WatchEvent::decode_text(&self.next_raw_event().await?)
    }

    /// Waits until this subscription receives a state event.
    pub async fn next_state(&mut self) -> io::Result<WatchState> {
        match self.next_event().await? {
            WatchEvent::State(state) => Ok(state),
            event => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected watch state event, got {event:?}"),
            )),
        }
    }

    /// Waits until this subscription receives a raw watch event payload.
    pub async fn wait_raw_event(&mut self) -> io::Result<Vec<u8>> {
        self.next_raw_event().await
    }

    /// Waits until this subscription receives a raw watch event payload.
    pub async fn next_raw_event(&mut self) -> io::Result<Vec<u8>> {
        if let Some(event) = self.pop_raw_event() {
            return Ok(event);
        }
        loop {
            let revents = poll_revents(self.watch_file.get_ref())?;
            check_poll_errors(revents, "watch file")?;
            if revents & libc::POLLIN != 0 {
                self.raw_event_buffer
                    .extend(drain_watch_events(self.watch_file.get_ref())?);
                if let Some(event) = self.pop_raw_event() {
                    return Ok(event);
                }
                continue;
            }

            let mut guard = self.watch_file.readable().await?;
            let mut drained_events = Vec::new();
            match guard.try_io(
                |watch_file| match drain_watch_events(watch_file.get_ref()) {
                    Ok(events) => {
                        drained_events = events;
                        Err(io::ErrorKind::WouldBlock.into())
                    }
                    Err(error) => Err(error),
                },
            ) {
                Ok(result) => {
                    result?;
                }
                Err(_would_block) => {
                    if drained_events.is_empty() {
                        continue;
                    }
                    self.raw_event_buffer.extend(drained_events);
                    if let Some(event) = self.pop_raw_event() {
                        return Ok(event);
                    }
                }
            }
        }
    }

    /// Waits until this subscription receives a raw UTF-8 watch event.
    pub async fn wait_raw_event_to_string(&mut self) -> io::Result<String> {
        bytes_to_string(self.next_raw_event().await?)
    }

    /// Reads the current value from the watched data path.
    pub async fn read(&self) -> io::Result<Vec<u8>> {
        let value = timeout(DEFAULT_READ_TIMEOUT, read_retrying(&self.data_path))
            .await
            .map_err(|_| timed_out("read watched path"))??;
        info!("{} >>> {}", self.logical_path, format_watch_value(&value));
        Ok(value)
    }

    /// Reads the current value, retrying missing paths until `duration` elapses.
    pub async fn read_timeout(&self, duration: Duration) -> io::Result<Vec<u8>> {
        let value = timeout(duration, read_retrying(&self.data_path))
            .await
            .map_err(|_| timed_out("read watched path"))??;
        info!("{} >>> {}", self.logical_path, format_watch_value(&value));
        Ok(value)
    }

    /// Reads the current UTF-8 value from the watched data path.
    pub async fn read_to_string(&self) -> io::Result<String> {
        bytes_to_string(self.read().await?)
    }

    /// Reads the current UTF-8 value, retrying missing paths until `duration` elapses.
    pub async fn read_to_string_timeout(&self, duration: Duration) -> io::Result<String> {
        bytes_to_string(self.read_timeout(duration).await?)
    }

    /// Waits for a change and then reads the current value.
    pub async fn wait_and_read(&mut self) -> io::Result<Vec<u8>> {
        self.wait().await?;
        self.read().await
    }

    /// Waits for a change and then reads the current value within `duration`.
    pub async fn wait_and_read_timeout(&mut self, duration: Duration) -> io::Result<Vec<u8>> {
        timeout(duration, async {
            self.wait().await?;
            let value = read_retrying(&self.data_path).await?;
            info!("{} >>> {}", self.logical_path, format_watch_value(&value));
            Ok(value)
        })
        .await
        .map_err(|_| timed_out("wait for watch event and read watched path"))?
    }

    fn pop_raw_event(&mut self) -> Option<Vec<u8>> {
        let newline = self
            .raw_event_buffer
            .iter()
            .position(|byte| *byte == b'\n')?;
        let rest = self.raw_event_buffer.split_off(newline + 1);
        let event = std::mem::replace(&mut self.raw_event_buffer, rest);
        Some(event)
    }
}

fn drain_watch_events(fd: &OwnedFd) -> io::Result<Vec<u8>> {
    let mut buffer = [0_u8; 4096];
    let mut events = Vec::new();
    loop {
        let result = unsafe {
            libc::read(
                fd.as_raw_fd(),
                buffer.as_mut_ptr().cast::<libc::c_void>(),
                buffer.len(),
            )
        };
        match result {
            0 if events.is_empty() => return Err(io::ErrorKind::WouldBlock.into()),
            0 => return Ok(events),
            read if read > 0 => {
                events.extend_from_slice(&buffer[..read as usize]);
                continue;
            }
            _ => {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                if error.kind() == io::ErrorKind::WouldBlock {
                    return if events.is_empty() {
                        Err(io::ErrorKind::WouldBlock.into())
                    } else {
                        Ok(events)
                    };
                }
                return Err(error);
            }
        }
    }
}

fn format_watch_value(value: &[u8]) -> String {
    String::from_utf8_lossy(value).escape_debug().to_string()
}

fn timed_out(operation: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, format!("{operation} timed out"))
}

async fn read_retrying(path: &Path) -> io::Result<Vec<u8>> {
    loop {
        match tokio::fs::read(path).await {
            Ok(value) => return Ok(value),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                sleep(Duration::from_millis(25)).await;
            }
            Err(error) => return Err(error),
        }
    }
}

fn set_nonblocking(fd: &OwnedFd) -> io::Result<()> {
    let flags = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    let result = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn poll_revents(fd: &OwnedFd) -> io::Result<libc::c_short> {
    let mut pollfd = libc::pollfd {
        fd: fd.as_raw_fd(),
        events: libc::POLLIN | libc::POLLERR | libc::POLLHUP,
        revents: 0,
    };
    let result = unsafe { libc::poll(&mut pollfd, 1, 0) };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(pollfd.revents)
}

fn check_poll_errors(revents: libc::c_short, label: &str) -> io::Result<()> {
    if revents & libc::POLLNVAL != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{label} file descriptor is invalid"),
        ));
    }
    if revents & libc::POLLERR != 0 {
        return Err(io::Error::other(format!("{label} reported POLLERR")));
    }
    if revents & libc::POLLHUP != 0 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            format!("{label} reported POLLHUP"),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod test;
