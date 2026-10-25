use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

mod blob {
    pub use buglogin_storage::blob::checksum;
}
mod paths {
    pub use buglogin_storage::paths::confined_path;
}
#[path = "../src/backup/mod.rs"]
mod backup;

use backup::{
    RestoreBoundary, confirm_overwrite, create_snapshot, restore_snapshot, verify_snapshot,
};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "buglogin-backup-test-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        make_tree_writable(&self.0);
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn immutable_snapshot_round_trips_with_manifest_checksum() {
    let root = TestDirectory::new();
    let source = root.path().join("source");
    let backups = root.path().join("backups");
    fs::create_dir_all(source.join("profiles/nested")).unwrap();
    fs::write(source.join("profiles/a.json"), b"alpha").unwrap();
    fs::write(source.join("profiles/nested/b.bin"), b"beta").unwrap();

    let snapshot = create_snapshot(&source, &backups, "snap-001").unwrap();
    assert_eq!(snapshot.file_count(), 2);
    let verified = verify_snapshot(&backups, "snap-001", snapshot.manifest_sha256()).unwrap();
    let restored = root.path().join("restored");
    restore_snapshot(&verified, &restored, RestoreBoundary::CreateOnly).unwrap();

    assert_eq!(
        fs::read(restored.join("profiles/a.json")).unwrap(),
        b"alpha"
    );
    assert_eq!(
        fs::read(restored.join("profiles/nested/b.bin")).unwrap(),
        b"beta"
    );
}

#[test]
fn existing_snapshot_id_is_never_overwritten() {
    let root = TestDirectory::new();
    let source = root.path().join("source");
    let backups = root.path().join("backups");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("value"), b"first").unwrap();
    let snapshot = create_snapshot(&source, &backups, "immutable").unwrap();

    fs::write(source.join("value"), b"second").unwrap();
    let error = create_snapshot(&source, &backups, "immutable").unwrap_err();
    assert_eq!(error.code, "backup_snapshot_exists");
    assert!(verify_snapshot(&backups, "immutable", snapshot.manifest_sha256()).is_ok());
}

#[test]
fn tampered_snapshot_blocks_restore_before_destination_write() {
    let root = TestDirectory::new();
    let source = root.path().join("source");
    let backups = root.path().join("backups");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("value"), b"trusted").unwrap();
    let snapshot = create_snapshot(&source, &backups, "tamper").unwrap();

    let stored = backups.join("tamper/files/value");
    make_writable(&stored);
    fs::write(&stored, b"tampered").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&stored).unwrap().permissions().mode();
        assert_ne!(mode & 0o200, 0, "owner must be able to modify cleanup copy");
        assert_eq!(mode & 0o022, 0, "cleanup must not grant group/world write");
    }
    let destination = root.path().join("destination");
    let error = restore_snapshot(&snapshot, &destination, RestoreBoundary::CreateOnly).unwrap_err();

    assert_eq!(error.code, "backup_checksum_mismatch");
    assert!(!destination.exists());
}

#[test]
fn replacement_requires_confirmation_and_preserves_safety_snapshot() {
    let root = TestDirectory::new();
    let source = root.path().join("source");
    let destination = root.path().join("destination");
    let backups = root.path().join("backups");
    let safety = root.path().join("safety");
    fs::create_dir_all(&source).unwrap();
    fs::create_dir_all(&destination).unwrap();
    fs::write(source.join("value"), b"new").unwrap();
    fs::write(destination.join("value"), b"old").unwrap();
    fs::write(destination.join("old-only"), b"preserve").unwrap();
    let snapshot = create_snapshot(&source, &backups, "restore-source").unwrap();

    assert_eq!(
        confirm_overwrite("yes").unwrap_err().code,
        "backup_overwrite_not_confirmed"
    );
    assert_eq!(
        restore_snapshot(&snapshot, &destination, RestoreBoundary::CreateOnly)
            .unwrap_err()
            .code,
        "backup_restore_destination_exists"
    );

    restore_snapshot(
        &snapshot,
        &destination,
        RestoreBoundary::ReplaceExisting {
            confirmation: confirm_overwrite("RESTORE_AND_OVERWRITE").unwrap(),
            safety_backups_root: &safety,
            safety_snapshot_id: "before-restore",
        },
    )
    .unwrap();

    assert_eq!(fs::read(destination.join("value")).unwrap(), b"new");
    assert!(!destination.join("old-only").exists());
    let manifest = fs::read(safety.join("before-restore/manifest.json")).unwrap();
    let safety_snapshot = verify_snapshot(
        &safety,
        "before-restore",
        &buglogin_storage::blob::checksum(&manifest),
    )
    .unwrap();
    let recovered = root.path().join("recovered-old");
    restore_snapshot(&safety_snapshot, &recovered, RestoreBoundary::CreateOnly).unwrap();
    assert_eq!(fs::read(recovered.join("value")).unwrap(), b"old");
    assert_eq!(fs::read(recovered.join("old-only")).unwrap(), b"preserve");
}

fn make_tree_writable(root: &Path) {
    if !root.exists() {
        return;
    }
    if root.is_file() {
        make_writable(root);
        return;
    }
    if let Ok(entries) = fs::read_dir(root) {
        for entry in entries.flatten() {
            make_tree_writable(&entry.path());
        }
    }
}

fn make_writable(path: &Path) {
    if let Ok(metadata) = fs::metadata(path) {
        let mut permissions = metadata.permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            permissions.set_mode(permissions.mode() | 0o200);
        }
        #[cfg(windows)]
        {
            #[allow(clippy::permissions_set_readonly_false)]
            permissions.set_readonly(false);
        }
        #[cfg(not(any(unix, windows)))]
        permissions.set_readonly(false);
        let _ = fs::set_permissions(path, permissions);
    }
}
