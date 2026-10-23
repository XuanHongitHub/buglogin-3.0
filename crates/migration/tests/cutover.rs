use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use buglogin_migration::cutover::{confirm_cutover, promote_staged};
use buglogin_migration::inspect::inspect_source;
use buglogin_migration::package::{PackageMetadata, build_neutral_package};
use buglogin_migration::rollback::{confirm_rollback, rollback_cutover};
use buglogin_migration::snapshot::create_snapshot;
use buglogin_migration::transform::stage_package;

static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new(label: &str) -> Self {
        let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "buglogin-migration-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create test directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn write(&self, relative: &str, contents: &str) {
        let path = self.0.join(relative);
        fs::create_dir_all(path.parent().unwrap()).expect("create fixture parent");
        fs::write(path, contents).expect("write fixture");
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn source_package() -> (TestDirectory, buglogin_migration::package::NeutralPackage) {
    let source = TestDirectory::new("source");
    source.write(
        "version.json",
        r#"{"system":"buglogin-legacy","version":"1.1.84","instanceId":"cutover-fixture","os":"windows","architecture":"x86_64"}"#,
    );
    source.write(
        "profiles/direct/metadata.json",
        r#"{"id":"direct","name":"Direct","runtime":"wayfern"}"#,
    );
    source.write("profiles/direct/profile/Preferences", "immutable source");
    let (report, snapshot) = inspect_source(source.path()).expect("inspect source");
    let package = build_neutral_package(
        &report,
        &snapshot,
        &PackageMetadata {
            created_at: "2026-07-20T00:00:00Z".to_owned(),
            tool_version: "0.1.0".to_owned(),
            tool_commit: "0000000000000000000000000000000000000000".to_owned(),
        },
    )
    .expect("build package");
    (source, package)
}

#[test]
fn stages_promotes_and_rolls_back_without_mutating_source() {
    let (source, package) = source_package();
    let source_before = create_snapshot(source.path()).expect("source before");
    let staging = TestDirectory::new("staging");
    let destination = TestDirectory::new("destination");

    let staged = stage_package(&package, staging.path(), "cutover-001").expect("stage package");
    assert_eq!(
        confirm_cutover("yes").unwrap_err().code,
        "migration_cutover_not_confirmed"
    );
    let receipt = promote_staged(
        &staged,
        destination.path(),
        confirm_cutover("CUTOVER_AND_PROMOTE").unwrap(),
    )
    .expect("promote staging");
    assert!(receipt.run_root.join("manifest.json").is_file());

    fs::write(
        receipt.run_root.join("foreign.txt"),
        "not owned by migration",
    )
    .expect("write foreign artifact");
    let rollback = rollback_cutover(
        destination.path(),
        "cutover-001",
        confirm_rollback("ROLLBACK_MIGRATION_RUN").unwrap(),
    )
    .expect("rollback cutover");
    assert_eq!(
        rollback.retained_unowned_artifacts,
        vec![PathBuf::from("foreign.txt")]
    );
    assert!(receipt.run_root.join("foreign.txt").is_file());
    assert!(!receipt.run_root.join("manifest.json").exists());
    assert_eq!(
        source_before,
        create_snapshot(source.path()).expect("source after")
    );
}

#[test]
fn cutover_rejects_tampered_staging_before_destination_write() {
    let (_source, package) = source_package();
    let staging = TestDirectory::new("staging-tamper");
    let destination = TestDirectory::new("destination-tamper");
    let staged = stage_package(&package, staging.path(), "cutover-002").expect("stage package");
    fs::write(staged.staging_root.join("manifest.json"), "tampered").expect("tamper staging");

    let error = promote_staged(
        &staged,
        destination.path(),
        confirm_cutover("CUTOVER_AND_PROMOTE").unwrap(),
    )
    .unwrap_err();
    assert_eq!(error.code, "migration_staging_checksum_mismatch");
    assert!(
        !destination
            .path()
            .join("migration-imports/cutover-002")
            .exists()
    );
}

#[test]
fn completed_pending_cutover_resumes_before_rename() {
    let (_source, package) = source_package();
    let staging = TestDirectory::new("staging-pending-resume");
    let destination = TestDirectory::new("destination-pending-resume");
    let staged =
        stage_package(&package, staging.path(), "cutover-pending-001").expect("stage package");
    let first = promote_staged(
        &staged,
        destination.path(),
        confirm_cutover("CUTOVER_AND_PROMOTE").unwrap(),
    )
    .expect("first promote");
    let pending_root = destination
        .path()
        .join("migration-imports/.cutover-pending-001.pending");
    fs::rename(&first.run_root, &pending_root).expect("simulate crash before rename");

    let resumed = promote_staged(
        &staged,
        destination.path(),
        confirm_cutover("CUTOVER_AND_PROMOTE").unwrap(),
    )
    .expect("resume pending promote");
    assert_eq!(
        resumed.run_root,
        destination
            .path()
            .join("migration-imports/cutover-pending-001")
    );
    assert!(!pending_root.exists());
    assert!(resumed.run_root.join("manifest.json").is_file());
}
#[test]
fn completed_cutover_replays_idempotently_and_rejects_tamper() {
    let (_source, package) = source_package();
    let staging = TestDirectory::new("staging-replay");
    let destination = TestDirectory::new("destination-replay");
    let staged =
        stage_package(&package, staging.path(), "cutover-replay-001").expect("stage package");
    let first = promote_staged(
        &staged,
        destination.path(),
        confirm_cutover("CUTOVER_AND_PROMOTE").unwrap(),
    )
    .expect("first promote");
    let replay = promote_staged(
        &staged,
        destination.path(),
        confirm_cutover("CUTOVER_AND_PROMOTE").unwrap(),
    )
    .expect("replay promote");
    assert_eq!(replay.run_root, first.run_root);
    assert_eq!(replay.artifacts, first.artifacts);

    fs::write(first.run_root.join("manifest.json"), "tampered").expect("tamper promoted file");
    let error = promote_staged(
        &staged,
        destination.path(),
        confirm_cutover("CUTOVER_AND_PROMOTE").unwrap(),
    )
    .unwrap_err();
    assert_eq!(error.code, "migration_cutover_replay_mismatch");
}

#[test]
fn rollback_rejects_duplicate_journal_artifacts_before_delete() {
    let (_source, package) = source_package();
    let staging = TestDirectory::new("staging-rollback-journal");
    let destination = TestDirectory::new("destination-rollback-journal");
    let staged = stage_package(&package, staging.path(), "cutover-rollback-journal-001")
        .expect("stage package");
    let receipt = promote_staged(
        &staged,
        destination.path(),
        confirm_cutover("CUTOVER_AND_PROMOTE").unwrap(),
    )
    .expect("promote staging");

    let journal_path = receipt.run_root.join(".buglogin-cutover.json");
    let checksum_path = receipt.run_root.join(".buglogin-cutover.sha256");
    let mut journal: serde_json::Value =
        serde_json::from_slice(&fs::read(&journal_path).expect("read journal")).unwrap();
    let artifacts = journal["artifacts"].as_array_mut().unwrap();
    artifacts.push(artifacts[0].clone());
    let mut journal_bytes = serde_json::to_vec(&journal).unwrap();
    journal_bytes.push(b'\n');
    fs::write(&journal_path, &journal_bytes).expect("tamper journal");
    fs::write(
        &checksum_path,
        buglogin_storage::blob::checksum(&journal_bytes),
    )
    .expect("rewrite journal checksum");

    let error = rollback_cutover(
        destination.path(),
        "cutover-rollback-journal-001",
        confirm_rollback("ROLLBACK_MIGRATION_RUN").unwrap(),
    )
    .unwrap_err();
    assert_eq!(error.code, "migration_cutover_journal_invalid");
    assert!(receipt.run_root.join("manifest.json").is_file());
    assert!(journal_path.is_file());
}
