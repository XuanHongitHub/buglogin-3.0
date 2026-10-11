use buglogin_storage::paths::confined_path;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn root() -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "buglogin-paths-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn accepts_relative_path_inside_root() {
    let root = root();
    assert_eq!(
        confined_path(&root, Path::new("profiles/a.json")).unwrap(),
        root.canonicalize().unwrap().join("profiles/a.json")
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn rejects_absolute_and_parent_traversal() {
    let root = root();
    assert!(confined_path(&root, Path::new("../escape")).is_err());
    assert!(confined_path(&root, &root.join("absolute")).is_err());
    let _ = fs::remove_dir_all(root);
}
