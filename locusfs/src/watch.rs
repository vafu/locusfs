use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

pub fn watch_path(path: &Path) -> io::Result<()> {
    let path = absolute_path(path)?;
    let mount_root = find_mount_root(&path)?;
    let logical_path = logical_watch_path(&mount_root, &path)?;
    let mut watcher = WatchClient::open(path, mount_root, logical_path)?;

    watcher.print_current_value()?;
    loop {
        watcher.wait_for_change()?;
        watcher.print_current_value()?;
    }
}

fn absolute_path(path: &Path) -> io::Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

#[derive(Debug)]
struct WatchClient {
    data_path: PathBuf,
    watch_file: File,
}

impl WatchClient {
    fn open(data_path: PathBuf, mount_root: PathBuf, logical_path: String) -> io::Result<Self> {
        let watch_file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(mount_root.join("watch"))?;
        let subscription = format!("{logical_path}\n");
        watch_file.write_all_at(subscription.as_bytes(), 0)?;

        Ok(Self {
            data_path,
            watch_file,
        })
    }

    fn wait_for_change(&mut self) -> io::Result<()> {
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

    fn print_current_value(&self) -> io::Result<()> {
        let mut file = open_retrying(&self.data_path)?;
        let mut value = Vec::new();
        file.read_to_end(&mut value)?;

        let mut stdout = io::stdout().lock();
        stdout.write_all(&value)?;
        if !value.ends_with(b"\n") {
            stdout.write_all(b"\n")?;
        }
        stdout.flush()
    }
}

fn find_mount_root(path: &Path) -> io::Result<PathBuf> {
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

fn logical_watch_path(mount_root: &Path, path: &Path) -> io::Result<String> {
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

fn open_retrying(path: &Path) -> io::Result<File> {
    loop {
        match File::open(path) {
            Ok(file) => return Ok(file),
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
