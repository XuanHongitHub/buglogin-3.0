use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use buglogin_migration::snapshot::create_snapshot;

static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "buglogin-migration-source-path-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create test root");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn snapshot_rejects_source_reached_through_symlink_ancestor() {
    let root = TestDirectory::new();
    let outside = root.path().join("outside");
    let linked_parent = root.path().join("linked-parent");
    let source = outside.join("source");

    fs::create_dir_all(&source).expect("create source");
    fs::write(
        source.join("version.json"),
        br#"{"system":"buglogin-legacy","version":"1.1.84","instanceId":"ancestor-link","os":"windows","architecture":"x86_64"}"#,
    )
    .expect("write source metadata");
    create_directory_link(&outside, &linked_parent);

    let error = create_snapshot(&linked_parent.join("source"))
        .expect_err("source ancestor link must be rejected");

    assert_eq!(error.code, "migration_source_symlink_unsupported");
}

#[cfg(unix)]
fn create_directory_link(target: &Path, link: &Path) {
    std::os::unix::fs::symlink(target, link).expect("create source directory symlink");
}

#[cfg(windows)]
fn create_directory_link(target: &Path, link: &Path) {
    let status = std::process::Command::new("cmd")
        .args(["/c", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .status()
        .expect("run junction command");
    assert!(status.success(), "create source directory junction");
}
