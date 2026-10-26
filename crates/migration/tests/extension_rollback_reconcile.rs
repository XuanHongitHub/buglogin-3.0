use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use buglogin_domain::{
    entities::{BrowserCompatibility, Extension, ExtensionGroup, ExtensionPackageType, SyncMode},
    ids::{ArtifactId, ExtensionGroupId, ExtensionId},
};
use buglogin_migration::{
    activate::{
        ActivationJournal, ActivationStatus, ArtifactActivationJournalEntry,
        EntityActivationJournalEntry,
    },
    inspect::{CutoverEligibility, InspectReport, SecretScan, SourceIdentity},
    reconcile::{ReconciliationStatus, reconcile_activation},
    rollback::{confirm_rollback, rollback_activation},
};
use buglogin_storage::local::LocalRepository;

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct Temp(PathBuf);

impl Temp {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "buglogin-extension-rollback-{label}-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn rollback_quarantines_owned_extensions_and_removes_only_owned_artifacts() {
    let destination = Temp::new("owned");
    let artifact_bytes = b"owned-extension";
    let artifact_sha256 = buglogin_storage::blob::checksum(artifact_bytes);
    let reused_bytes = b"reused-extension";
    let reused_sha256 = buglogin_storage::blob::checksum(reused_bytes);
    let owned_path = artifact_path("artifact_owned", &artifact_sha256);
    let reused_path = artifact_path("artifact_reused", &reused_sha256);
    write_artifact(destination.path(), &owned_path, artifact_bytes);
    write_artifact(destination.path(), &reused_path, reused_bytes);

    let repository = LocalRepository::open(destination.path()).unwrap();
    let extension = extension("ext_owned", "artifact_owned", &artifact_sha256);
    let group = extension_group("ext_group_owned", &extension);
    let extension_version = repository.create(extension.clone()).unwrap();
    let group_version = repository.create(group.clone()).unwrap();
    write_journal(
        destination.path(),
        "extension-run",
        vec![
            artifact_entry("artifact_owned", &owned_path, &artifact_sha256, true),
            artifact_entry("artifact_reused", &reused_path, &reused_sha256, false),
        ],
        vec![entity_entry(&extension, extension_version.revision, true)],
        vec![entity_entry(&group, group_version.revision, true)],
    );

    let receipt = rollback_activation(
        destination.path(),
        "extension-run",
        confirm_rollback("ROLLBACK_MIGRATION_RUN").unwrap(),
    )
    .unwrap();

    assert_eq!(receipt.quarantined_extensions.len(), 1);
    assert_eq!(receipt.quarantined_extension_groups.len(), 1);
    assert_eq!(
        receipt.removed_extension_artifacts,
        vec![owned_path.clone()]
    );
    assert!(
        repository
            .get::<Extension>(extension.id.as_str())
            .unwrap()
            .is_none()
    );
    assert!(
        repository
            .get::<ExtensionGroup>(group.id.as_str())
            .unwrap()
            .is_none()
    );
    assert!(!destination.path().join(owned_path).exists());
    assert_eq!(
        fs::read(destination.path().join(reused_path)).unwrap(),
        reused_bytes
    );
}

#[test]
fn rollback_refuses_entity_or_artifact_drift_before_mutation() {
    let destination = Temp::new("drift");
    let artifact_bytes = b"original-extension";
    let artifact_sha256 = buglogin_storage::blob::checksum(artifact_bytes);
    let path = artifact_path("artifact_drift", &artifact_sha256);
    write_artifact(destination.path(), &path, artifact_bytes);
    let repository = LocalRepository::open(destination.path()).unwrap();
    let extension = extension("ext_drift", "artifact_drift", &artifact_sha256);
    let version = repository.create(extension.clone()).unwrap();
    write_journal(
        destination.path(),
        "drift-run",
        vec![artifact_entry(
            "artifact_drift",
            &path,
            &artifact_sha256,
            true,
        )],
        vec![entity_entry(&extension, version.revision, true)],
        Vec::new(),
    );

    fs::write(destination.path().join(&path), b"tampered-extension").unwrap();
    let error = rollback_activation(
        destination.path(),
        "drift-run",
        confirm_rollback("ROLLBACK_MIGRATION_RUN").unwrap(),
    )
    .unwrap_err();
    assert_eq!(error.code, "migration_activation_entity_changed");
    assert!(
        repository
            .get::<Extension>(extension.id.as_str())
            .unwrap()
            .is_some()
    );
}

#[test]
fn reconciliation_proves_extension_entities_and_artifact_checksum() {
    let destination = Temp::new("reconcile");
    let artifact_bytes = b"reconciled-extension";
    let artifact_sha256 = buglogin_storage::blob::checksum(artifact_bytes);
    let path = artifact_path("artifact_reconcile", &artifact_sha256);
    write_artifact(destination.path(), &path, artifact_bytes);
    let repository = LocalRepository::open(destination.path()).unwrap();
    let extension = extension("ext_reconcile", "artifact_reconcile", &artifact_sha256);
    let group = extension_group("ext_group_reconcile", &extension);
    let extension_version = repository.create(extension.clone()).unwrap();
    let group_version = repository.create(group.clone()).unwrap();
    write_journal(
        destination.path(),
        "reconcile-run",
        vec![artifact_entry(
            "artifact_reconcile",
            &path,
            &artifact_sha256,
            false,
        )],
        vec![entity_entry(&extension, extension_version.revision, false)],
        vec![entity_entry(&group, group_version.revision, false)],
    );

    let report = extension_report();
    let passed = reconcile_activation(&report, destination.path(), "reconcile-run").unwrap();
    assert_eq!(passed.status, ReconciliationStatus::Passed);
    assert_eq!(passed.activated_extension_artifacts, 1);
    assert_eq!(passed.activated_extensions, 1);
    assert_eq!(passed.activated_extension_groups, 1);

    fs::write(destination.path().join(path), b"tampered-extension").unwrap();
    let blocked = reconcile_activation(&report, destination.path(), "reconcile-run").unwrap();
    assert_eq!(blocked.status, ReconciliationStatus::Blocked);
    assert_eq!(
        blocked.mismatched_extension_artifact_ids,
        vec!["artifact_reconcile"]
    );
}

fn extension(id: &str, artifact_ref: &str, artifact_sha256: &str) -> Extension {
    Extension {
        id: ExtensionId::new(id).unwrap(),
        name: "Extension".into(),
        file_name: Some("extension.xpi".into()),
        package_type: ExtensionPackageType::Xpi,
        artifact_ref: ArtifactId::new(artifact_ref).unwrap(),
        artifact_sha256: artifact_sha256.into(),
        compatibility: vec![BrowserCompatibility::Firefox],
        sync_mode: Some(SyncMode::Managed),
        created_at: "2026-07-21T00:00:00Z".into(),
        updated_at: "2026-07-21T00:00:00Z".into(),
    }
}

fn extension_group(id: &str, extension: &Extension) -> ExtensionGroup {
    ExtensionGroup {
        id: ExtensionGroupId::new(id).unwrap(),
        name: "Extension Group".into(),
        extension_ids: vec![extension.id.clone()],
        extension_sources: Default::default(),
        sync_mode: Some(SyncMode::Managed),
        created_at: "2026-07-21T00:00:00Z".into(),
        updated_at: "2026-07-21T00:00:00Z".into(),
    }
}

fn artifact_path(artifact_ref: &str, sha256: &str) -> PathBuf {
    PathBuf::from("extension-artifacts")
        .join(artifact_ref)
        .join(sha256)
}

fn write_artifact(root: &Path, relative: &Path, bytes: &[u8]) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

fn artifact_entry(
    artifact_ref: &str,
    path: &Path,
    sha256: &str,
    created: bool,
) -> ArtifactActivationJournalEntry {
    ArtifactActivationJournalEntry {
        artifact_ref: artifact_ref.into(),
        path: path.to_string_lossy().replace('\\', "/"),
        sha256: sha256.into(),
        created,
    }
}

fn entity_entry<T: serde::Serialize>(
    entity: &T,
    revision: u64,
    created: bool,
) -> EntityActivationJournalEntry {
    EntityActivationJournalEntry {
        entity_id: match serde_json::to_value(entity).unwrap()["id"].as_str() {
            Some(id) => id.into(),
            None => panic!("entity ID missing"),
        },
        revision,
        sha256: buglogin_storage::blob::checksum(&serde_json::to_vec(entity).unwrap()),
        created,
    }
}

fn write_journal(
    root: &Path,
    run_id: &str,
    extension_artifacts: Vec<ArtifactActivationJournalEntry>,
    extensions: Vec<EntityActivationJournalEntry>,
    extension_groups: Vec<EntityActivationJournalEntry>,
) {
    let directory = root.join("migration-activations");
    fs::create_dir_all(&directory).unwrap();
    let journal = ActivationJournal {
        schema: "buglogin.activation/1".into(),
        run_id: run_id.into(),
        status: ActivationStatus::Active,
        input_sha256: "test-input".into(),
        credentials: Vec::new(),
        extension_artifacts,
        extensions,
        extension_groups,
        groups: Vec::new(),
        proxies: Vec::new(),
        profiles: Vec::new(),
    };
    fs::write(
        directory.join(format!("{run_id}.json")),
        serde_json::to_vec_pretty(&journal).unwrap(),
    )
    .unwrap();
}

fn extension_report() -> InspectReport {
    InspectReport {
        schema: "buglogin.inspect/1".into(),
        source: SourceIdentity {
            system: "buglogin-legacy".into(),
            version: "1.1.84".into(),
            instance_id: "sha256:test".into(),
            os: "windows".into(),
            architecture: "x86_64".into(),
            root: ".".into(),
        },
        entity_counts: BTreeMap::from([("extensions".into(), 1), ("extensionGroups".into(), 1)]),
        total_bytes: 0,
        runtime_findings: Vec::new(),
        secret_scan: SecretScan::default(),
        invalid_records: Vec::new(),
        warnings: Vec::new(),
        dangling_references: Vec::new(),
        source_tree_sha256: "test-source".into(),
        cutover_eligibility: CutoverEligibility::Eligible,
    }
}
