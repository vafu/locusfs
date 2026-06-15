use super::*;

#[test]
fn stable_inodes_are_allocated_for_same_entry() {
    let table = Mutex::new(InodeTable::new());
    let fs = LocusFs {
        graph: InMemoryGraph::new(),
        inodes: table,
    };
    let project = ProjectName::new("my-project").unwrap();
    let first = fs.inode(FsEntry::ProjectDir(project.clone())).unwrap();
    let second = fs.inode(FsEntry::ProjectDir(project)).unwrap();
    assert_eq!(first, second);
}

#[test]
fn read_slicing_respects_offset_and_size() {
    assert_eq!(slice_for_read(b"abcdef", 2, 3), b"cde");
    assert_eq!(slice_for_read(b"abcdef", 9, 3), b"");
}
