use std::path::Path;

use super::logical_watch_path;

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
