use std::fs;
use std::os::unix::fs::symlink;
use std::path::Path;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use locusfs_core::InMemoryGraph;
use locusfs_fuse::{FuseMountConfig, mount};

#[test]
#[ignore = "requires host /dev/fuse access"]
fn project_symlink_flow_works_through_real_fuse_mount() {
    let mountpoint = Path::new("/tmp/locusfs-test");
    cleanup_mount(mountpoint);
    fs::create_dir_all(mountpoint).unwrap();

    let project = tempfile::tempdir().unwrap();
    init_git_repo(project.path());

    let _mount = mount(FuseMountConfig::new(mountpoint), InMemoryGraph::new()).unwrap();
    wait_for_mount(mountpoint);

    let project_path = mountpoint.join("projects/my-project");
    symlink(project.path(), &project_path).unwrap();

    let projects = fs::read_dir(mountpoint.join("projects"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert_eq!(projects, vec!["my-project"]);

    assert_eq!(
        fs::read_to_string(project_path.join("git-branch")).unwrap(),
        format!("{}\n", expected_git_branch(project.path()))
    );

    fs::write(project_path.join("name"), "Display Name\n").unwrap();
    assert_eq!(
        fs::read_to_string(project_path.join("name")).unwrap(),
        "Display Name\n"
    );

    assert_eq!(
        fs::read_to_string(project_path.join("path")).unwrap(),
        format!("{}\n", project.path().display())
    );

    drop(_mount);
    cleanup_mount(mountpoint);
}

fn init_git_repo(path: &Path) {
    run(Command::new("git").arg("-C").arg(path).arg("init"));
}

fn expected_git_branch(path: &Path) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .arg("branch")
        .arg("--show-current")
        .output()
        .unwrap();
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

fn run(command: &mut Command) {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "command failed: status={:?} stdout={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn wait_for_mount(mountpoint: &Path) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if mountpoint.join("projects").is_dir() {
            return;
        }
        thread::sleep(Duration::from_millis(25));
    }
    panic!("mount did not become ready at {}", mountpoint.display());
}

fn cleanup_mount(mountpoint: &Path) {
    let _ = Command::new("fusermount3")
        .arg("-u")
        .arg(mountpoint)
        .status();
    let _ = fs::remove_dir_all(mountpoint);
}
