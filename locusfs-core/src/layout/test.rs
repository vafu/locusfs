use super::*;

#[test]
fn encodes_shell_hostile_segments() {
    assert_eq!(
        encode_segment("window:57/title active").unwrap(),
        "window%3A57%2Ftitle%20active"
    );
}

#[test]
fn decodes_segments() {
    assert_eq!(decode_segment("window%3A57").unwrap(), "window:57");
}

#[test]
fn rejects_bad_percent_encoding() {
    assert!(decode_segment("window%").is_err());
    assert!(decode_segment("window%XX").is_err());
}

#[test]
fn builds_generic_node_paths() {
    let node = NodeId::new("window:57").unwrap();
    let key = PropertyKey::new("title").unwrap();
    assert_eq!(
        Layout::node_property(&node, &key).unwrap(),
        PathBuf::from("nodes/window%3A57/props/title")
    );
}

#[test]
fn builds_project_domain_paths() {
    let project = ProjectName::new("my-project").unwrap();
    let key = PropertyKey::new("git-branch").unwrap();
    assert_eq!(
        Layout::project_property(&project, &key).unwrap(),
        PathBuf::from("projects/my-project/git-branch")
    );
}
