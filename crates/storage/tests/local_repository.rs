use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use buglogin_domain::entities::{Profile, ProfileState, Runtime, SyncMode};
use buglogin_domain::ids::ProfileId;
use buglogin_storage::local::LocalRepository;

static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "buglogin-storage-test-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create test directory");
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

fn profile(name: &str) -> Profile {
    Profile {
        id: ProfileId::new("prf_local_repository").expect("valid profile ID"),
        name: name.to_owned(),
        runtime: Runtime::Wayfern,
        runtime_version: Some("1.0.0".to_owned()),
        runtime_config: None,
        state: ProfileState::Ready,
        archived: false,
        fingerprint: None,
        note: None,
        proxy_id: None,
        vpn_id: None,
        group_ids: Vec::new(),
        tags: vec!["bootstrap".to_owned()],
        extension_group_id: None,
        sync_mode: Some(SyncMode::Local),
        created_at: "2026-07-20T00:00:00Z".to_owned(),
        updated_at: "2026-07-20T00:00:00Z".to_owned(),
    }
}

#[test]
fn profile_round_trips_across_revisions() {
    let directory = TestDirectory::new();
    let repository = LocalRepository::open(directory.path()).expect("open repository");

    let created = repository
        .create(profile("Original"))
        .expect("create profile");
    assert_eq!(created.schema_version, 1);
    assert_eq!(created.revision, 1);

    let mut changed = created.value;
    changed.name = "Updated".to_owned();
    changed.updated_at = "2026-07-20T00:01:00Z".to_owned();
    let updated = repository.update(1, changed).expect("update profile");
    assert_eq!(updated.revision, 2);

    let restored = repository
        .get::<Profile>("prf_local_repository")
        .expect("read profile")
        .expect("stored profile");
    assert_eq!(restored, updated);
}

#[test]
fn stale_revision_returns_conflict_without_overwrite() {
    let directory = TestDirectory::new();
    let repository = LocalRepository::open(directory.path()).expect("open repository");
    repository
        .create(profile("Original"))
        .expect("create profile");

    let mut first = profile("First writer");
    first.updated_at = "2026-07-20T00:01:00Z".to_owned();
    repository.update(1, first).expect("first update");

    let error = repository
        .update(1, profile("Stale writer"))
        .expect_err("stale update must conflict");
    assert_eq!(error.status, 409);
    assert_eq!(error.code, "storage_revision_conflict");
    assert_eq!(error.fields["expectedRevision"], 1);
    assert_eq!(error.fields["actualRevision"], 2);

    let restored = repository
        .get::<Profile>("prf_local_repository")
        .expect("read profile")
        .expect("stored profile");
    assert_eq!(restored.value.name, "First writer");
    assert_eq!(restored.revision, 2);
}

#[test]
fn interrupted_temporary_residue_is_ignored() {
    let directory = TestDirectory::new();
    let repository = LocalRepository::open(directory.path()).expect("open repository");
    repository
        .create(profile("Original"))
        .expect("create profile");

    let revisions = directory
        .path()
        .join("profiles/prf_local_repository/revisions");
    fs::write(revisions.join(".tmp-interrupted"), b"{partial").expect("write interrupted residue");

    let restored = repository
        .get::<Profile>("prf_local_repository")
        .expect("read profile")
        .expect("stored profile");
    assert_eq!(restored.value.name, "Original");

    let updated = repository
        .update(1, profile("Recovered"))
        .expect("update despite residue");
    assert_eq!(updated.revision, 2);
    assert!(revisions.join(".tmp-interrupted").exists());
}
