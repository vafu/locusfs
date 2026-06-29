use std::{
    fs::File,
    io::Write,
    os::fd::{FromRawFd, OwnedFd},
    path::Path,
    time::Duration,
};

use tokio::io::unix::AsyncFd;
use tokio::time::timeout;

use super::{Watch, logical_watch_path};

#[test]
fn logical_path_is_mount_relative_with_leading_slash() {
    let path = logical_watch_path(
        Path::new("/tmp/locusfs-run"),
        Path::new("/tmp/locusfs-run/window/10/title"),
    )
    .unwrap();

    assert_eq!(path, "/window/10/title");
}

#[test]
fn logical_path_rejects_paths_outside_mount() {
    let error = logical_watch_path(
        Path::new("/tmp/locusfs-run"),
        Path::new("/tmp/other/window/10/title"),
    )
    .unwrap_err();

    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
}

#[test]
fn logical_path_rejects_parent_dir_escape() {
    let error = logical_watch_path(
        Path::new("/tmp/locusfs-run"),
        Path::new("/tmp/locusfs-run/../outside"),
    )
    .unwrap_err();

    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
}

#[tokio::test]
async fn read_timeout_bounds_missing_path_retry() {
    let (watch_file, _peer) = std::os::unix::net::UnixStream::pair().unwrap();
    watch_file.set_nonblocking(true).unwrap();
    let watch = Watch {
        data_path: Path::new("/tmp/locusfs-client-missing-value").to_path_buf(),
        mount_root: Path::new("/tmp").to_path_buf(),
        logical_path: "/missing".to_string(),
        watch_file: AsyncFd::new(watch_file.into()).unwrap(),
        raw_event_buffer: Vec::new(),
    };

    let error = watch
        .read_timeout(Duration::from_millis(1))
        .await
        .unwrap_err();

    assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
}

#[tokio::test]
async fn next_event_returns_one_frame_when_read_drains_multiple_frames() {
    let (watch_file, _peer) = std::os::unix::net::UnixStream::pair().unwrap();
    watch_file.set_nonblocking(true).unwrap();
    let mut watch = Watch {
        data_path: Path::new("/tmp/locusfs-client-value").to_path_buf(),
        mount_root: Path::new("/tmp").to_path_buf(),
        logical_path: "/node/57".to_string(),
        watch_file: AsyncFd::new(watch_file.into()).unwrap(),
        raw_event_buffer: b"change\nnode removed node:57\n".to_vec(),
    };

    assert_eq!(
        watch.next_event().await.unwrap(),
        crate::WatchEvent::Change(crate::WatchChange::Change)
    );
    assert_eq!(
        watch.next_event().await.unwrap(),
        crate::WatchEvent::Change(crate::WatchChange::Node {
            action: crate::WatchAction::Removed,
            node: "node:57".to_string(),
        })
    );
}

#[tokio::test]
async fn next_event_clears_readiness_after_drain() {
    let (watch_file, mut peer) = pipe_pair();
    peer.write_all(b"change\n").unwrap();

    let mut watch = Watch {
        data_path: Path::new("/tmp/locusfs-client-value").to_path_buf(),
        mount_root: Path::new("/tmp").to_path_buf(),
        logical_path: "/node/57".to_string(),
        watch_file: AsyncFd::new(watch_file.into()).unwrap(),
        raw_event_buffer: Vec::new(),
    };

    assert_eq!(
        watch.next_event().await.unwrap(),
        crate::WatchEvent::Change(crate::WatchChange::Change)
    );

    let readable = timeout(Duration::from_millis(10), watch.watch_file.readable()).await;
    assert!(readable.is_err());
}

fn pipe_pair() -> (OwnedFd, File) {
    let mut fds = [0; 2];
    let result = unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) };
    assert_eq!(result, 0);
    unsafe { (OwnedFd::from_raw_fd(fds[0]), File::from_raw_fd(fds[1])) }
}
