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

const SOURCE_VERSION: &[u8] = br#"{"system":"buglogin-legacy","version":"1.1.84","instanceId":"runtime-artifacts","os":"macos","architecture":"aarch64"}"#;
const PROFILE_ID: &str = "2f1e6c3a-9d4b-4e8a-b1c2-7a5d9e0f1a2b";

#[test]
fn snapshot_accepts_source_under_root_owned_system_links() {
    // std::env::temp_dir() is under /var -> /private/var on macOS, a root-owned system link.
    let root = TestDirectory::new();
    let source = root.path().join("source");
    fs::create_dir_all(&source).expect("create source");
    fs::write(source.join("version.json"), SOURCE_VERSION).expect("write source metadata");

    let snapshot = create_snapshot(&source).expect("system-managed ancestor links are allowed");

    assert_eq!(snapshot.files.len(), 1);
}

#[cfg(unix)]
#[test]
fn snapshot_skips_chromium_singleton_links_and_runtime_binaries() {
    let root = TestDirectory::new();
    let source = root.path().join("source");
    let profile = source.join("profiles").join(PROFILE_ID).join("profile");
    fs::create_dir_all(&profile).expect("create profile");
    fs::write(source.join("version.json"), SOURCE_VERSION).expect("write source metadata");
    fs::write(profile.join("Preferences"), b"{}").expect("write preferences");
    for name in [
        "SingletonLock",
        "SingletonSocket",
        "SingletonCookie",
        "RunningChromeVersion",
    ] {
        std::os::unix::fs::symlink("MacBook-Pro-12345", profile.join(name))
            .expect("create chromium singleton link");
    }
    let bundle = source.join("binaries/wayfern/154.0/Wayfern.app/Contents");
    fs::create_dir_all(&bundle).expect("create runtime bundle");
    fs::write(bundle.join("Info.plist"), b"plist").expect("write bundle file");
    std::os::unix::fs::symlink("Versions/Current/Frameworks", bundle.join("Frameworks"))
        .expect("create bundle link");

    let snapshot = create_snapshot(&source).expect("known runtime artifacts are skipped");

    let paths: Vec<&str> = snapshot
        .files
        .iter()
        .map(|file| file.path.as_str())
        .collect();
    assert_eq!(
        paths,
        vec![
            format!("profiles/{PROFILE_ID}/profile/Preferences").as_str(),
            "version.json",
        ]
    );
}

#[cfg(unix)]
#[test]
fn snapshot_rejects_unexpected_symlink_inside_source() {
    let root = TestDirectory::new();
    let source = root.path().join("source");
    let profile = source.join("profiles").join(PROFILE_ID);
    fs::create_dir_all(&profile).expect("create profile");
    fs::write(source.join("version.json"), SOURCE_VERSION).expect("write source metadata");
    std::os::unix::fs::symlink("/etc/hosts", profile.join("notes.txt"))
        .expect("create unexpected link");

    let error = create_snapshot(&source).expect_err("unexpected links must be rejected");

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
