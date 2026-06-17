//! Client helpers for reading and watching values exposed by a locusfs mount.

use std::io;
use std::os::fd::{AsRawFd, OwnedFd};
use std::path::{Path, PathBuf};

use tokio::io::unix::AsyncFd;
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use tokio::time::{Duration, sleep};
use tracing::info;

/// Reads a locusfs path into memory.
pub async fn read(path: impl AsRef<Path>) -> io::Result<Vec<u8>> {
    tokio::fs::read(path).await
}

/// Reads a UTF-8 locusfs path into a string.
pub async fn read_to_string(path: impl AsRef<Path>) -> io::Result<String> {
    bytes_to_string(read(path).await?)
}

fn bytes_to_string(value: Vec<u8>) -> io::Result<String> {
    String::from_utf8(value).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("locusfs value is not valid UTF-8: {error}"),
        )
    })
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
    let ancestors = path.ancestors().map(Path::to_path_buf).collect::<Vec<_>>();
    for ancestor in ancestors {
        if tokio::fs::metadata(ancestor.join("watch"))
            .await
            .is_ok_and(|metadata| metadata.is_file())
        {
            return Ok(ancestor);
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

    /// Waits until this subscription receives a watch event and returns its raw payload.
    pub async fn wait_event(&mut self) -> io::Result<Vec<u8>> {
        loop {
            let mut guard = self.watch_file.readable().await?;
            match guard.try_io(|watch_file| drain_watch_events(watch_file.get_ref())) {
                Ok(result) => return result,
                Err(_would_block) => continue,
            }
        }
    }

    /// Waits until this subscription receives a UTF-8 watch event.
    pub async fn wait_event_to_string(&mut self) -> io::Result<String> {
        bytes_to_string(self.wait_event().await?)
    }

    /// Reads the current value from the watched data path.
    pub async fn read(&self) -> io::Result<Vec<u8>> {
        let value = read_retrying(&self.data_path).await?;
        info!("{} >>> {}", self.logical_path, format_watch_value(&value));
        Ok(value)
    }

    /// Reads the current UTF-8 value from the watched data path.
    pub async fn read_to_string(&self) -> io::Result<String> {
        bytes_to_string(self.read().await?)
    }

    /// Waits for a change and then reads the current value.
    pub async fn wait_and_read(&mut self) -> io::Result<Vec<u8>> {
        self.wait().await?;
        self.read().await
    }
}

fn drain_watch_events(fd: &OwnedFd) -> io::Result<Vec<u8>> {
    let revents = poll_revents(fd)?;
    check_poll_errors(revents, "watch file")?;
    if revents & libc::POLLIN == 0 {
        return Err(io::ErrorKind::WouldBlock.into());
    }

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
