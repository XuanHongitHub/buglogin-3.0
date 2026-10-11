use buglogin_domain::{
    errors::Problem,
    ids::{ResourceId, WorkspaceId},
};
use buglogin_storage::remote::{MemoryRemoteChunkStore, RemoteChunk, RemoteChunkStore};
use buglogin_sync::{
    export::{SyncExport, SyncExportBlob, SyncRevision, canonical_manifest_checksum},
    import::{
        plan_sync_import, rollback_staged_sync_import, stage_sync_import, sync_object_storage_key,
    },
    manifest::{ManifestObject, ManifestTombstone, SyncManifest},
};

fn manifest() -> SyncManifest {
    SyncManifest::new_with_tombstones(
        WorkspaceId::new("ws-sync-export").unwrap(),
        ResourceId::new("res-sync-export").unwrap(),
        17,
        2,
        Some(1),
        b"canonical-root",
        vec![
            ManifestObject::new("profile/b", b"two").unwrap(),
            ManifestObject::new("profile/a", b"one").unwrap(),
        ],
        vec![ManifestTombstone::new("profile/deleted", 1, 1_000, 9_000).unwrap()],
    )
    .unwrap()
}

fn blobs() -> Vec<SyncExportBlob> {
    vec![
        SyncExportBlob {
            key: "profile/b".into(),
            size: 3,
            checksum: buglogin_storage::blob::checksum(b"two"),
            data: b"two".to_vec(),
        },
        SyncExportBlob {
            key: "profile/a".into(),
            size: 3,
            checksum: buglogin_storage::blob::checksum(b"one"),
            data: b"one".to_vec(),
        },
    ]
}

fn export() -> SyncExport {
    let manifest = manifest();
    let current_checksum = canonical_manifest_checksum(&manifest).unwrap();
    SyncExport::new(
        "sync-export-0001",
        "legacy-production-1",
        manifest,
        None,
        vec![
            SyncRevision {
                generation: 1,
                parent_generation: None,
                manifest_sha256: "1".repeat(64),
            },
            SyncRevision {
                generation: 2,
                parent_generation: Some(1),
                manifest_sha256: current_checksum,
            },
        ],
        blobs(),
        1_753_142_400_000,
        "0.1.0",
    )
    .unwrap()
}

#[test]
fn export_round_trip_is_deterministic_and_remaps_fencing() {
    let first = export();
    let encoded = serde_json::to_vec(&first).unwrap();
    let decoded: SyncExport = serde_json::from_slice(&encoded).unwrap();
    decoded.validate().unwrap();
    assert_eq!(first, decoded);

    let mut reversed_blobs = blobs();
    reversed_blobs.reverse();
    let manifest = manifest();
    let current_checksum = canonical_manifest_checksum(&manifest).unwrap();
    let second = SyncExport::new(
        "sync-export-0001",
        "legacy-production-1",
        manifest,
        None,
        vec![
            SyncRevision {
                generation: 1,
                parent_generation: None,
                manifest_sha256: "1".repeat(64),
            },
            SyncRevision {
                generation: 2,
                parent_generation: Some(1),
                manifest_sha256: current_checksum,
            },
        ],
        reversed_blobs,
        1_753_142_400_000,
        "0.1.0",
    )
    .unwrap();
    assert_eq!(first, second);

    let imported = first.import_manifest(99).unwrap();
    assert_eq!(imported.fencing_token, 99);
    assert_eq!(imported.generation, 2);
    assert_eq!(first.manifest.fencing_token, 17);
}

#[test]
fn export_rejects_missing_extra_duplicate_and_tampered_blobs() {
    let mut missing = export();
    missing.blobs.pop();
    assert_eq!(
        missing.validate().unwrap_err().code,
        "sync_export_blob_missing"
    );

    let mut extra = export();
    extra.blobs.push(SyncExportBlob {
        key: "profile/extra".into(),
        size: 1,
        checksum: buglogin_storage::blob::checksum(b"x"),
        data: b"x".to_vec(),
    });
    assert_eq!(
        extra.validate().unwrap_err().code,
        "sync_export_blob_not_in_manifest"
    );

    let mut duplicate = export();
    duplicate.blobs.push(duplicate.blobs[0].clone());
    assert_eq!(
        duplicate.validate().unwrap_err().code,
        "sync_export_duplicate_blob"
    );

    let mut tampered = export();
    tampered.blobs[0].data[0] ^= 1;
    assert_eq!(
        tampered.validate().unwrap_err().code,
        "sync_export_blob_checksum_mismatch"
    );
}

#[test]
fn export_rejects_tampered_manifest_and_revision_chain() {
    let mut manifest = export();
    manifest.manifest.root_checksum = "0".repeat(64);
    assert_eq!(
        manifest.validate().unwrap_err().code,
        "sync_export_manifest_checksum_mismatch"
    );

    let mut revisions = export();
    revisions.revisions[1].parent_generation = None;
    assert_eq!(
        revisions.validate().unwrap_err().code,
        "sync_export_revision_invalid"
    );

    assert_eq!(
        export().import_manifest(0).unwrap_err().code,
        "sync_import_fencing_token_invalid"
    );
}

fn genesis() -> SyncManifest {
    SyncManifest::new(
        WorkspaceId::new("ws-sync-export").unwrap(),
        ResourceId::new("res-sync-export").unwrap(),
        3,
        1,
        None,
        b"genesis",
        vec![ManifestObject::new("profile/a", b"one").unwrap()],
    )
    .unwrap()
}

fn successor_export() -> (SyncManifest, SyncExport) {
    let current = genesis();
    let next = SyncManifest::new(
        current.workspace_id.clone(),
        current.resource_id.clone(),
        44,
        2,
        Some(1),
        b"successor",
        vec![ManifestObject::new("profile/b", b"two").unwrap()],
    )
    .unwrap();
    let export = SyncExport::new(
        "successor-export",
        "source-instance",
        next.clone(),
        Some(1),
        vec![SyncRevision {
            generation: 2,
            parent_generation: Some(1),
            manifest_sha256: canonical_manifest_checksum(&next).unwrap(),
        }],
        vec![SyncExportBlob {
            key: "profile/b".into(),
            size: 3,
            checksum: buglogin_storage::blob::checksum(b"two"),
            data: b"two".to_vec(),
        }],
        1_753_142_400_000,
        "0.1.0",
    )
    .unwrap();
    (current, export)
}

#[test]
fn import_planner_requires_base_and_remaps_fencing() {
    let (current, export) = successor_export();
    let plan = plan_sync_import(&export, Some(&current), 101).unwrap();
    assert!(!plan.replayed);
    assert_eq!(plan.manifest.fencing_token, 101);
    assert_eq!(plan.manifest.generation, 2);
    assert_eq!(plan.blobs.len(), 1);

    assert_eq!(
        plan_sync_import(&export, None, 101).unwrap_err().code,
        "sync_import_base_missing"
    );
    assert_eq!(
        plan_sync_import(&export, Some(&genesis()), 0)
            .unwrap_err()
            .code,
        "sync_import_fencing_token_invalid"
    );
}

#[test]
fn import_planner_is_idempotent_and_rejects_wrong_base() {
    let (current, export) = successor_export();
    let mut same = export.manifest.clone();
    same.fencing_token = 999;
    let replay_export = SyncExport::new(
        "successor-export",
        "source-instance",
        same,
        Some(1),
        export.revisions.clone(),
        export.blobs.clone(),
        export.created_at_unix_ms,
        export.tool_version.clone(),
    )
    .unwrap();
    let replay = plan_sync_import(&replay_export, Some(&export.manifest), 101).unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.manifest.fencing_token, export.manifest.fencing_token);

    let mut wrong = current.clone();
    wrong.generation = 9;
    assert_eq!(
        plan_sync_import(&export, Some(&wrong), 101)
            .unwrap_err()
            .code,
        "sync_import_base_conflict"
    );
}

fn genesis_export() -> SyncExport {
    let manifest = SyncManifest::new(
        WorkspaceId::new("ws-sync-stage").unwrap(),
        ResourceId::new("res-sync-stage").unwrap(),
        8,
        1,
        None,
        b"stage-root",
        vec![
            ManifestObject::new("profile/a", b"one").unwrap(),
            ManifestObject::new("profile/b", b"two").unwrap(),
        ],
    )
    .unwrap();
    SyncExport::new(
        "stage-export",
        "stage-source",
        manifest.clone(),
        None,
        vec![SyncRevision {
            generation: 1,
            parent_generation: None,
            manifest_sha256: canonical_manifest_checksum(&manifest).unwrap(),
        }],
        vec![
            SyncExportBlob {
                key: "profile/a".into(),
                size: 3,
                checksum: buglogin_storage::blob::checksum(b"one"),
                data: b"one".to_vec(),
            },
            SyncExportBlob {
                key: "profile/b".into(),
                size: 3,
                checksum: buglogin_storage::blob::checksum(b"two"),
                data: b"two".to_vec(),
            },
        ],
        1_753_142_400_000,
        "0.1.0",
    )
    .unwrap()
}

#[test]
fn staging_resumes_existing_blobs_and_rolls_back_only_created_objects() {
    let export = genesis_export();
    let plan = plan_sync_import(&export, None, 55).unwrap();
    let mut remote = MemoryRemoteChunkStore::default();
    let existing_key = sync_object_storage_key(&plan.manifest, &plan.blobs[0]);
    remote
        .put(&existing_key, &plan.blobs[0].data, &plan.blobs[0].checksum)
        .unwrap();

    let receipt = stage_sync_import(&mut remote, &plan).unwrap();
    assert_eq!(
        receipt.objects.iter().filter(|item| item.created).count(),
        1
    );
    assert_eq!(
        receipt.objects.iter().filter(|item| !item.created).count(),
        1
    );
    rollback_staged_sync_import(&mut remote, &receipt.objects).unwrap();
    assert_eq!(
        remote.checksum(&existing_key).unwrap(),
        Some(plan.blobs[0].checksum.clone())
    );
    let created_key = sync_object_storage_key(&plan.manifest, &plan.blobs[1]);
    assert_eq!(remote.checksum(&created_key).unwrap(), None);
}

struct FailingStore {
    inner: MemoryRemoteChunkStore,
    fail_key_suffix: String,
}

impl RemoteChunkStore for FailingStore {
    fn checksum(&self, object_key: &str) -> Result<Option<String>, Problem> {
        self.inner.checksum(object_key)
    }

    fn put(
        &mut self,
        object_key: &str,
        bytes: &[u8],
        expected_checksum: &str,
    ) -> Result<(), Problem> {
        if object_key.ends_with(&self.fail_key_suffix) {
            return Err(Problem::new(
                "forced_stage_failure",
                "Forced stage failure",
                503,
                "test",
            ));
        }
        self.inner.put(object_key, bytes, expected_checksum)
    }

    fn get(&self, object_key: &str, expected_checksum: &str) -> Result<RemoteChunk, Problem> {
        self.inner.get(object_key, expected_checksum)
    }

    fn delete_if_checksum(
        &mut self,
        object_key: &str,
        expected_checksum: &str,
    ) -> Result<bool, Problem> {
        self.inner.delete_if_checksum(object_key, expected_checksum)
    }
}

#[test]
fn staging_failure_removes_prior_migration_owned_blobs() {
    let export = genesis_export();
    let plan = plan_sync_import(&export, None, 55).unwrap();
    let mut remote = FailingStore {
        inner: MemoryRemoteChunkStore::default(),
        fail_key_suffix: plan.blobs[1].key.clone(),
    };
    assert_eq!(
        stage_sync_import(&mut remote, &plan).unwrap_err().code,
        "forced_stage_failure"
    );
    for blob in &plan.blobs {
        let key = sync_object_storage_key(&plan.manifest, blob);
        assert_eq!(remote.checksum(&key).unwrap(), None);
    }
}
