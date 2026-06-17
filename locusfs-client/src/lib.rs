//! Client helpers for reading and watching values exposed by a locusfs mount.

use std::fs::{File, OpenOptions};
use std::io::{self, Read};
use std::os::fd::AsRawFd;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

/// Reads a locusfs path into memory.
pub fn read(path: impl AsRef<Path>) -> io::Result<Vec<u8>> {
    let mut file = File::open(path)?;
    let mut value = Vec::new();
    file.read_to_end(&mut value)?;
    Ok(value)
}

/// Reads a UTF-8 locusfs path into a string.
pub fn read_to_string(path: impl AsRef<Path>) -> io::Result<String> {
    bytes_to_string(read(path)?)
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
pub fn find_mount_root(path: impl AsRef<Path>) -> io::Result<PathBuf> {
    let path = path.as_ref();
    for ancestor in path.ancestors() {
        if ancestor.join("watch").is_file() {
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
    watch_file: File,
}

impl Watch {
    /// Opens `/watch` for `path` and subscribes this handle to that path.
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let data_path = absolute_path(path)?;
        let mount_root = find_mount_root(&data_path)?;
        let logical_path = logical_watch_path(&mount_root, &data_path)?;
        Self::open_with_parts(data_path, mount_root, logical_path)
    }

    /// Opens a watch from already resolved path parts.
    pub fn open_with_parts(
        data_path: PathBuf,
        mount_root: PathBuf,
        logical_path: String,
    ) -> io::Result<Self> {
        let watch_file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(mount_root.join("watch"))?;
        let subscription = format!("{logical_path}\n");
        watch_file.write_all_at(subscription.as_bytes(), 0)?;

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

    /// Blocks until this subscription receives a change notification.
    pub fn wait(&mut self) -> io::Result<()> {
        let mut pollfd = libc::pollfd {
            fd: self.watch_file.as_raw_fd(),
            events: libc::POLLIN | libc::POLLERR | libc::POLLHUP,
            revents: 0,
        };

        loop {
            let result = unsafe { libc::poll(&mut pollfd, 1, -1) };
            if result >= 0 {
                break;
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }

        check_poll_errors(pollfd.revents, "watch file")?;
        if pollfd.revents & libc::POLLIN != 0 {
            drain_watch_events(&mut self.watch_file)?;
        }
        Ok(())
    }

    /// Reads the current value from the watched data path.
    pub fn read(&self) -> io::Result<Vec<u8>> {
        read_retrying(&self.data_path)
    }

    /// Reads the current UTF-8 value from the watched data path.
    pub fn read_to_string(&self) -> io::Result<String> {
        bytes_to_string(self.read()?)
    }

    /// Waits for a change and then reads the current value.
    pub fn wait_and_read(&mut self) -> io::Result<Vec<u8>> {
        self.wait()?;
        self.read()
    }
}

fn drain_watch_events(file: &mut File) -> io::Result<()> {
    let mut buffer = [0_u8; 4096];
    loop {
        match file.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(_) => continue,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
            Err(error) => return Err(error),
        }
    }
}

fn read_retrying(path: &Path) -> io::Result<Vec<u8>> {
    loop {
        match File::open(path) {
            Ok(mut file) => {
                let mut value = Vec::new();
                file.read_to_end(&mut value)?;
                return Ok(value);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                thread::sleep(Duration::from_millis(25));
            }
            Err(error) => return Err(error),
        }
    }
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
