use buglogin_domain::ids::{ProxyId, ResourceId, WorkspaceId};
use buglogin_storage::blob::checksum;
use buglogin_sync::{
    conflict::{
        ApplyOutcome, ConflictKind, ConflictResolution, ConflictStatus, ManifestCoordinator,
    },
    manifest::{
        MANIFEST_SCHEMA_VERSION, MAX_PROFILE_SYNC_OBJECT_BYTES, ManifestDependency,
        ManifestFileChunk, ManifestFileChunks, ManifestObject, ManifestObjectReference,
        ManifestPayloadEncryption, ManifestTombstone, PayloadEncryptionAlgorithm, SyncManifest,
    },
};
use serde_json::json;

fn resource() -> ResourceId {
    ResourceId::new("resource-1").unwrap()
}

fn workspace() -> WorkspaceId {
    WorkspaceId::new("workspace-1").unwrap()
}

fn manifest(generation: u64, parent: Option<u64>, body: &[u8]) -> SyncManifest {
    SyncManifest::new(
        workspace(),
        resource(),
        1,
        generation,
        parent,
        body,
        vec![ManifestObject::new("profiles/p1", body).unwrap()],
    )
    .unwrap()
}

const KEY_A: &str = "sync-attempt-key-0001";
const KEY_B: &str = "sync-attempt-key-0002";

#[test]
fn verifies_manifest_and_object_checksums() {
    let value = manifest(1, None, b"one");
    assert!(value.verify_root(b"one").is_ok());
    assert!(value.verify_root(b"two").is_err());
    assert!(value.objects[0].verify(b"one").is_ok());
    assert!(value.objects[0].verify(b"two").is_err());
}

#[test]
fn rejects_duplicate_object_keys() {
    let result = SyncManifest::new(
        workspace(),
        resource(),
        1,
        1,
        None,
        b"one",
        vec![
            ManifestObject::new("profiles/p1", b"one").unwrap(),
            ManifestObject::new("profiles/p1", b"two").unwrap(),
        ],
    );
    assert_eq!(result.unwrap_err().code, "duplicate_manifest_object");
}

#[test]
fn object_keys_are_confined_relative_paths() {
    for key in [
        "../outside",
        "profile/../outside",
        "profile\\outside",
        "C:/outside",
        "profile//state",
        "profile/./state",
        "profile/state\u{0000}",
    ] {
        assert_eq!(
            ManifestObject::new(key, b"data").unwrap_err().code,
            "invalid_manifest_object_key",
            "key {key:?} must be rejected"
        );
    }
    assert!(ManifestObject::new("profile/state..json", b"data").is_ok());
    assert!(ManifestObject::new(".buglogin/resource-key-envelope.json", b"data").is_ok());
}

#[test]
fn rejects_objects_above_the_shared_sync_limit() {
    let mut value = manifest(1, None, b"one");
    value.objects[0].size = MAX_PROFILE_SYNC_OBJECT_BYTES + 1;
    assert_eq!(
        value.validate().unwrap_err().code,
        "manifest_object_too_large"
    );
}

#[test]
fn rejects_ephemeral_profile_paths_in_plain_and_chunked_manifests() {
    let mut legacy_plain = manifest(1, None, b"cache profile root");
    legacy_plain.objects[0].key = "Default/Cache/index".into();
    assert!(legacy_plain.validate().is_ok());
    assert_eq!(
        legacy_plain.validate_new_profile_paths().unwrap_err().code,
        "sync_ephemeral_profile_path"
    );

    let new_plain = SyncManifest::new(
        workspace(),
        resource(),
        1,
        1,
        None,
        b"cache profile root",
        vec![ManifestObject::new("Default/Cache/index", b"cache").unwrap()],
    );
    assert_eq!(new_plain.unwrap_err().code, "sync_ephemeral_profile_path");

    assert!(buglogin_sync::manifest::is_ephemeral_profile_path(
        "Default/Service Worker/ScriptCache/index"
    ));
    assert!(!buglogin_sync::manifest::is_ephemeral_profile_path(
        "Default/Cookies"
    ));
}

#[test]
fn schema_v3_requires_a_valid_chunk_mapping_bound_to_uploaded_objects() {
    let path = "Default/Cookies";
    let first_plaintext_checksum = "c".repeat(64);
    let last_plaintext_checksum = "d".repeat(64);
    let first = ManifestObject::new(
        format!(
            ".buglogin/file-chunks/{}/{:016x}-{}-{}",
            checksum(path.as_bytes()),
            0,
            first_plaintext_checksum,
            checksum(b"encrypted first chunk")
        ),
        b"encrypted first chunk",
    )
    .unwrap();
    let last = ManifestObject::new(
        format!(
            ".buglogin/file-chunks/{}/{:016x}-{}-{}",
            checksum(path.as_bytes()),
            1,
            last_plaintext_checksum,
            checksum(b"encrypted last chunk")
        ),
        b"encrypted last chunk",
    )
    .unwrap();
    let envelope =
        ManifestObject::new(".buglogin/resource-key-envelope.json", b"opaque envelope").unwrap();
    let mut value = SyncManifest::new_chunked_with_tombstones(
        workspace(),
        resource(),
        1,
        1,
        None,
        b"chunked root",
        vec![first.clone(), last.clone(), envelope.clone()],
        vec![ManifestTombstone::new_object(path, 1, 100, 200).unwrap()],
        ManifestPayloadEncryption {
            schema_version: 1,
            algorithm: PayloadEncryptionAlgorithm::Aes256GcmChunkV2,
            key_version: Some("a".repeat(64)),
            resource_key_envelope: ManifestObjectReference::from(&envelope),
        },
        vec![ManifestFileChunks {
            path: path.into(),
            plaintext_size: 4 * 1024 * 1024 + 2,
            plaintext_checksum: "b".repeat(64),
            chunks: vec![
                ManifestFileChunk {
                    index: 0,
                    plaintext_size: 4 * 1024 * 1024,
                    plaintext_checksum: first_plaintext_checksum,
                    object: ManifestObjectReference::from(&first),
                },
                ManifestFileChunk {
                    index: 1,
                    plaintext_size: 2,
                    plaintext_checksum: last_plaintext_checksum,
                    object: ManifestObjectReference::from(&last),
                },
            ],
        }],
    )
    .unwrap();
    assert!(value.validate().is_ok());
    let mut colliding_object_path = value.clone();
    colliding_object_path
        .objects
        .push(ManifestObject::new(path, b"conflicting logical file path").unwrap());
    assert_eq!(
        colliding_object_path.validate().unwrap_err().code,
        "invalid_manifest_file_chunks"
    );
    let serialized = serde_json::to_value(&value).unwrap();
    assert_eq!(serialized["schemaVersion"], 3);
    assert_eq!(
        serde_json::from_value::<SyncManifest>(serialized).unwrap(),
        value
    );

    let mut unsafe_path = value.clone();
    unsafe_path.file_chunks[0].path = "../escape".into();
    assert_eq!(
        unsafe_path.validate().unwrap_err().code,
        "invalid_manifest_file_chunks"
    );

    let mut cache_path = value.clone();
    let cache_path_name = "Default/Cache/Cookies";
    cache_path.file_chunks[0].path = cache_path_name.into();
    for chunk in &mut cache_path.file_chunks[0].chunks {
        let previous_key = chunk.object.key.clone();
        let new_key = format!(
            ".buglogin/file-chunks/{}/{:016x}-{}-{}",
            checksum(cache_path_name.as_bytes()),
            chunk.index,
            chunk.plaintext_checksum,
            chunk.object.checksum
        );
        cache_path
            .objects
            .iter_mut()
            .find(|object| object.key == previous_key)
            .unwrap()
            .key = new_key.clone();
        chunk.object.key = new_key;
    }
    assert!(cache_path.validate().is_ok());
    assert_eq!(
        cache_path.validate_new_profile_paths().unwrap_err().code,
        "sync_ephemeral_profile_path"
    );

    let nested_path = "Default/Cookies/child";
    let nested_plaintext_checksum = "e".repeat(64);
    let nested_object = ManifestObject::new(
        format!(
            ".buglogin/file-chunks/{}/{:016x}-{}-{}",
            checksum(nested_path.as_bytes()),
            0,
            nested_plaintext_checksum,
            checksum(b"encrypted nested chunk")
        ),
        b"encrypted nested chunk",
    )
    .unwrap();
    let mut colliding_file_paths = value.clone();
    colliding_file_paths.objects.push(nested_object.clone());
    colliding_file_paths.file_chunks.push(ManifestFileChunks {
        path: nested_path.into(),
        plaintext_size: 5,
        plaintext_checksum: "f".repeat(64),
        chunks: vec![ManifestFileChunk {
            index: 0,
            plaintext_size: 5,
            plaintext_checksum: nested_plaintext_checksum,
            object: ManifestObjectReference::from(&nested_object),
        }],
    });
    assert_eq!(
        colliding_file_paths.validate().unwrap_err().code,
        "invalid_manifest_file_chunks"
    );

    let mut missing_object = value.clone();
    let missing_key = missing_object.file_chunks[0].chunks[0].object.key.clone();
    missing_object
        .objects
        .retain(|object| object.key != missing_key);
    assert_eq!(
        missing_object.validate().unwrap_err().code,
        "manifest_dependency_object_missing"
    );

    let mut orphan_object = value.clone();
    orphan_object
        .objects
        .push(ManifestObject::new(".buglogin/file-chunks/orphan/object", b"orphan").unwrap());
    assert_eq!(
        orphan_object.validate().unwrap_err().code,
        "invalid_manifest_file_chunks"
    );

    value.file_chunks[0].chunks[1].index = 3;
    assert_eq!(
        value.validate().unwrap_err().code,
        "invalid_manifest_file_chunks"
    );
}

#[test]
fn rejects_zero_fencing_token() {
    let result = SyncManifest::new(
        workspace(),
        resource(),
        0,
        1,
        None,
        b"one",
        vec![ManifestObject::new("profiles/p1", b"one").unwrap()],
    );

    assert_eq!(result.unwrap_err().code, "invalid_manifest_fencing_token");
}

#[test]
fn manifest_v2_requires_tombstones_and_rejects_schema_v1() {
    let value = manifest(1, None, b"one");
    let serialized = serde_json::to_value(&value).unwrap();
    assert_eq!(serialized["schemaVersion"], MANIFEST_SCHEMA_VERSION);
    assert_eq!(serialized["tombstones"], json!([]));

    let mut legacy = serialized.clone();
    legacy["schemaVersion"] = json!(1);
    legacy.as_object_mut().unwrap().remove("tombstones");
    assert!(serde_json::from_value::<SyncManifest>(legacy).is_err());

    let mut wrong_version = serialized;
    wrong_version["schemaVersion"] = json!(1);
    let error = serde_json::from_value::<SyncManifest>(wrong_version)
        .unwrap()
        .validate()
        .unwrap_err();
    assert_eq!(error.code, "unsupported_manifest_schema");

    let mut legacy_shape = manifest(1, None, b"one");
    let legacy_shape = serde_json::to_value(&mut legacy_shape).unwrap();
    assert!(legacy_shape.get("dependencies").is_none());
    assert!(
        serde_json::from_value::<SyncManifest>(legacy_shape)
            .unwrap()
            .dependencies
            .is_empty()
    );
}

#[test]
fn proxy_dependency_must_reference_verified_manifest_objects() {
    let metadata = ManifestObject::new("proxies/p1/metadata", b"safe metadata").unwrap();
    let envelope =
        ManifestObject::new("proxies/p1/credential-envelope", b"opaque envelope").unwrap();
    let mut value = SyncManifest::new(
        workspace(),
        resource(),
        1,
        1,
        None,
        b"profile root",
        vec![metadata.clone(), envelope.clone()],
    )
    .unwrap();
    value.dependencies.push(ManifestDependency::Proxy {
        proxy_id: ProxyId::new("proxy_manifest_1").unwrap(),
        revision: 1,
        credential_version: None,
        metadata_object: Some(ManifestObjectReference::from(&metadata)),
        credential_envelope_object: Some(ManifestObjectReference::from(&envelope)),
    });
    assert!(value.validate().is_ok());

    let serialized = serde_json::to_value(&value).unwrap();
    assert_eq!(serialized["dependencies"][0]["kind"], "proxy");
    assert_eq!(serialized["dependencies"][0]["proxyId"], "proxy_manifest_1");

    value.dependencies[0] = ManifestDependency::Proxy {
        proxy_id: ProxyId::new("proxy_manifest_1").unwrap(),
        revision: 1,
        credential_version: None,
        metadata_object: Some(ManifestObjectReference {
            key: "proxies/p1/not-uploaded".into(),
            size: 1,
            checksum: "a".repeat(64),
        }),
        credential_envelope_object: None,
    };
    assert_eq!(
        value.validate().unwrap_err().code,
        "manifest_dependency_object_missing"
    );
}

#[test]
fn proxy_dependency_can_reference_canonical_api_record_without_profile_blob_copy() {
    let mut value = SyncManifest::new(
        workspace(),
        resource(),
        1,
        1,
        None,
        b"profile root",
        Vec::new(),
    )
    .unwrap();
    value.dependencies.push(ManifestDependency::Proxy {
        proxy_id: ProxyId::new("proxy_manifest_2").unwrap(),
        revision: 3,
        credential_version: None,
        metadata_object: None,
        credential_envelope_object: None,
    });

    assert!(value.validate().is_ok());
    let serialized = serde_json::to_value(&value).unwrap();
    assert!(
        serialized["dependencies"][0]
            .get("metadataObject")
            .is_none()
    );
    assert_eq!(
        serde_json::from_value::<SyncManifest>(serialized).unwrap(),
        value
    );
}

#[test]
fn payload_encryption_envelope_must_reference_an_uploaded_object() {
    let envelope =
        ManifestObject::new(".buglogin/resource-key-envelope.json", b"opaque envelope").unwrap();
    let profile_data = ManifestObject::new("profiles/p1/chunk-0001", b"encrypted chunk").unwrap();
    let mut value = SyncManifest::new(
        workspace(),
        resource(),
        1,
        1,
        None,
        b"encrypted root",
        vec![envelope.clone(), profile_data],
    )
    .unwrap();
    value.payload_encryption = Some(ManifestPayloadEncryption {
        schema_version: 1,
        algorithm: PayloadEncryptionAlgorithm::Aes256GcmChunkV1,
        key_version: None,
        resource_key_envelope: ManifestObjectReference::from(&envelope),
    });
    assert!(value.validate().is_ok());
    value.payload_encryption.as_mut().unwrap().algorithm =
        PayloadEncryptionAlgorithm::Aes256GcmChunkV2;
    assert_eq!(
        value.validate().unwrap_err().code,
        "invalid_payload_key_version"
    );
    value.payload_encryption.as_mut().unwrap().key_version = Some("a".repeat(64));
    assert!(value.validate().is_ok());

    value
        .payload_encryption
        .as_mut()
        .unwrap()
        .resource_key_envelope
        .size += 1;
    assert_eq!(
        value.validate().unwrap_err().code,
        "manifest_dependency_object_missing"
    );
}

#[test]
fn rejects_live_object_with_same_key_as_tombstone() {
    let result = SyncManifest::new_with_tombstones(
        workspace(),
        resource(),
        1,
        2,
        Some(1),
        b"two",
        vec![ManifestObject::new("profiles/p1", b"two").unwrap()],
        vec![ManifestTombstone::new("profiles/p1", 1, 1_000, 2_000).unwrap()],
    );

    assert_eq!(result.unwrap_err().code, "stale_manifest_resurrection");
}

#[test]
fn validates_tombstone_generation_and_retention() {
    let future_generation = SyncManifest::new_with_tombstones(
        workspace(),
        resource(),
        1,
        2,
        Some(1),
        b"two",
        vec![],
        vec![ManifestTombstone::new("profiles/p1", 3, 1_000, 2_000).unwrap()],
    );
    assert_eq!(
        future_generation.unwrap_err().code,
        "invalid_tombstone_generation"
    );

    let retention = ManifestTombstone::new("profiles/p1", 1, 2_000, 2_000);
    assert_eq!(retention.unwrap_err().code, "invalid_tombstone_retention");
}

#[test]
fn retained_tombstone_blocks_stale_resurrection() {
    let tombstone = ManifestTombstone::new("profiles/p1", 1, 1_000, 3_000).unwrap();
    let previous = SyncManifest::new_with_tombstones(
        workspace(),
        resource(),
        1,
        1,
        None,
        b"one",
        vec![],
        vec![tombstone],
    )
    .unwrap();
    let resurrected = SyncManifest::new(
        workspace(),
        resource(),
        1,
        2,
        Some(1),
        b"two",
        vec![ManifestObject::new("profiles/p1", b"stale").unwrap()],
    )
    .unwrap();

    let error = resurrected
        .validate_successor(&previous, 2_000)
        .unwrap_err();
    assert_eq!(error.code, "stale_manifest_resurrection");
}

#[test]
fn newer_fencing_token_can_legitimately_recreate_tombstoned_path() {
    let tombstone = ManifestTombstone::new("profiles/p1", 1, 1_000, 3_000).unwrap();
    let previous = SyncManifest::new_with_tombstones(
        workspace(),
        resource(),
        4,
        1,
        None,
        b"one",
        vec![],
        vec![tombstone],
    )
    .unwrap();
    let recreated = SyncManifest::new(
        workspace(),
        resource(),
        5,
        2,
        Some(1),
        b"two",
        vec![ManifestObject::new("profiles/p1", b"new data").unwrap()],
    )
    .unwrap();

    assert!(recreated.validate_successor(&previous, 2_000).is_ok());
}

#[test]
fn same_fence_cannot_drop_tombstone_and_recreate_path() {
    let tombstone = ManifestTombstone::new("profiles/p1", 1, 1_000, 3_000).unwrap();
    let previous = SyncManifest::new_with_tombstones(
        workspace(),
        resource(),
        4,
        1,
        None,
        b"one",
        vec![],
        vec![tombstone],
    )
    .unwrap();
    let recreated = SyncManifest::new(
        workspace(),
        resource(),
        4,
        2,
        Some(1),
        b"two",
        vec![ManifestObject::new("profiles/p1", b"stale data").unwrap()],
    )
    .unwrap();

    assert_eq!(
        recreated
            .validate_successor(&previous, 2_000)
            .unwrap_err()
            .code,
        "stale_manifest_resurrection"
    );
}

#[test]
fn coordinator_rejects_resurrection_during_tombstone_retention() {
    let tombstone = ManifestTombstone::new("profiles/p1", 1, 1_000, 3_000).unwrap();
    let previous = SyncManifest::new_with_tombstones(
        workspace(),
        resource(),
        1,
        1,
        None,
        b"one",
        vec![],
        vec![tombstone],
    )
    .unwrap();
    let resurrected = SyncManifest::new(
        workspace(),
        resource(),
        1,
        2,
        Some(1),
        b"two",
        vec![ManifestObject::new("profiles/p1", b"stale").unwrap()],
    )
    .unwrap();
    let mut coordinator = ManifestCoordinator::default();
    coordinator.submit_at(KEY_A, previous, 1_000).unwrap();

    let error = coordinator
        .submit_at(KEY_B, resurrected, 2_000)
        .unwrap_err();

    assert_eq!(error.code, "stale_manifest_resurrection");
    assert_eq!(coordinator.current().unwrap().generation, 1);
}

#[test]
fn retained_tombstone_is_required_until_deadline() {
    let tombstone = ManifestTombstone::new("profiles/p1", 1, 1_000, 3_000).unwrap();
    let previous = SyncManifest::new_with_tombstones(
        workspace(),
        resource(),
        1,
        1,
        None,
        b"one",
        vec![],
        vec![tombstone.clone()],
    )
    .unwrap();
    let retained = SyncManifest::new_with_tombstones(
        workspace(),
        resource(),
        1,
        2,
        Some(1),
        b"two",
        vec![],
        vec![tombstone],
    )
    .unwrap();
    assert!(retained.validate_successor(&previous, 2_000).is_ok());

    let missing =
        SyncManifest::new(workspace(), resource(), 1, 2, Some(1), b"two", vec![]).unwrap();
    assert_eq!(
        missing
            .validate_successor(&previous, 2_000)
            .unwrap_err()
            .code,
        "missing_retained_tombstone"
    );
    assert!(missing.validate_successor(&previous, 3_000).is_ok());
}

#[test]
fn workspace_mismatch_fails_closed_for_same_resource_id() {
    let mut coordinator = ManifestCoordinator::default();
    coordinator
        .submit(KEY_A, manifest(1, None, b"one"))
        .unwrap();

    let incoming = SyncManifest::new(
        WorkspaceId::new("workspace-2").unwrap(),
        resource(),
        1,
        2,
        Some(1),
        b"two",
        vec![ManifestObject::new("profiles/p1", b"two").unwrap()],
    )
    .unwrap();
    let error = coordinator.submit(KEY_B, incoming).unwrap_err();

    assert_eq!(error.code, "manifest_workspace_mismatch");
    assert_eq!(coordinator.current().unwrap().generation, 1);
}

#[test]
fn stale_fence_rejected_after_reacquire() {
    let mut coordinator = ManifestCoordinator::default();
    coordinator
        .submit(KEY_A, manifest(1, None, b"one"))
        .unwrap();
    let reacquired = SyncManifest::new(
        workspace(),
        resource(),
        2,
        2,
        Some(1),
        b"two",
        vec![ManifestObject::new("profiles/p1", b"two").unwrap()],
    )
    .unwrap();
    coordinator.submit(KEY_B, reacquired).unwrap();

    let stale = SyncManifest::new(
        workspace(),
        resource(),
        1,
        3,
        Some(2),
        b"three",
        vec![ManifestObject::new("profiles/p1", b"three").unwrap()],
    )
    .unwrap();
    let error = coordinator
        .submit("sync-attempt-key-0003", stale)
        .unwrap_err();

    assert_eq!(error.code, "stale_manifest_fencing_token");
    assert_eq!(coordinator.current().unwrap().generation, 2);
    assert_eq!(coordinator.current().unwrap().fencing_token, 2);
}

#[test]
fn accepts_only_contiguous_parented_versions() {
    let mut coordinator = ManifestCoordinator::default();
    assert!(matches!(
        coordinator
            .submit(KEY_A, manifest(1, None, b"one"))
            .unwrap(),
        ApplyOutcome::Applied(_)
    ));
    assert!(matches!(
        coordinator.submit(KEY_B, manifest(3, Some(2), b"three")).unwrap(),
        ApplyOutcome::Conflict(conflict) if conflict.kind == ConflictKind::VersionGap
    ));
    assert_eq!(coordinator.current().unwrap().generation, 1);
}

#[test]
fn gap_retry_applies_after_parent_arrives() {
    let mut coordinator = ManifestCoordinator::default();
    coordinator
        .submit(KEY_A, manifest(1, None, b"one"))
        .unwrap();

    let gap_key = "sync-attempt-key-0003";
    assert!(matches!(
        coordinator
            .submit(gap_key, manifest(3, Some(2), b"three"))
            .unwrap(),
        ApplyOutcome::Conflict(conflict) if conflict.kind == ConflictKind::VersionGap
    ));

    coordinator
        .submit(KEY_B, manifest(2, Some(1), b"two"))
        .unwrap();

    assert!(matches!(
        coordinator
            .submit(gap_key, manifest(3, Some(2), b"three"))
            .unwrap(),
        ApplyOutcome::Applied(_)
    ));
    assert_eq!(coordinator.current().unwrap().generation, 3);
}

#[test]
fn detects_same_generation_different_checksum_without_overwrite() {
    let mut coordinator = ManifestCoordinator::default();
    coordinator
        .submit(KEY_A, manifest(1, None, b"one"))
        .unwrap();
    let outcome = coordinator
        .submit(KEY_B, manifest(1, None, b"two"))
        .unwrap();
    assert!(
        matches!(outcome, ApplyOutcome::Conflict(conflict) if conflict.kind == ConflictKind::ConcurrentWrite)
    );
    assert_eq!(
        coordinator.current().unwrap().root_checksum,
        manifest(1, None, b"one").root_checksum
    );
}

#[test]
fn retries_are_idempotent_and_reuse_with_new_input_fails() {
    let mut coordinator = ManifestCoordinator::default();
    let first = coordinator
        .submit(KEY_A, manifest(1, None, b"one"))
        .unwrap();
    let retry = coordinator
        .submit(KEY_A, manifest(1, None, b"one"))
        .unwrap();
    assert_eq!(first, retry);
    let error = coordinator
        .submit(KEY_A, manifest(2, Some(1), b"two"))
        .unwrap_err();
    assert_eq!(error.code, "idempotency_conflict");
}

#[test]
fn conflict_requires_explicit_resolution() {
    let mut coordinator = ManifestCoordinator::default();
    coordinator
        .submit(KEY_A, manifest(1, None, b"one"))
        .unwrap();
    let conflict = match coordinator
        .submit(KEY_B, manifest(1, None, b"two"))
        .unwrap()
    {
        ApplyOutcome::Conflict(conflict) => conflict,
        _ => panic!("expected conflict"),
    };
    assert_eq!(conflict.status, ConflictStatus::Detected);
    let conflict = conflict.acknowledge().unwrap();
    assert_eq!(conflict.status, ConflictStatus::AwaitingResolution);
    let conflict = conflict.resolve(ConflictResolution::KeepCurrent).unwrap();
    assert_eq!(conflict.status, ConflictStatus::Resolved);
    assert_eq!(conflict.resolution, Some(ConflictResolution::KeepCurrent));
}

#[test]
fn coordinator_keep_current_resolution_preserves_authoritative_head() {
    let mut coordinator = ManifestCoordinator::default();
    coordinator
        .submit(KEY_A, manifest(1, None, b"one"))
        .unwrap();
    coordinator
        .submit(KEY_B, manifest(1, None, b"two"))
        .unwrap();

    let outcome = coordinator
        .resolve_at(KEY_B, ConflictResolution::KeepCurrent, None, 1_000)
        .unwrap();

    assert!(matches!(outcome, ApplyOutcome::Unchanged(_)));
    assert_eq!(
        coordinator.current().unwrap().root_checksum,
        manifest(1, None, b"one").root_checksum
    );
}

#[test]
fn coordinator_accept_incoming_resolution_applies_canonical_successor() {
    let mut coordinator = ManifestCoordinator::default();
    coordinator
        .submit(KEY_A, manifest(1, None, b"one"))
        .unwrap();
    coordinator
        .submit(KEY_B, manifest(1, None, b"two"))
        .unwrap();
    let replacement = manifest(2, Some(1), b"two");

    let outcome = coordinator
        .resolve_at(
            KEY_B,
            ConflictResolution::AcceptIncoming,
            Some(replacement.clone()),
            1_000,
        )
        .unwrap();

    assert_eq!(outcome, ApplyOutcome::Applied(replacement.clone()));
    assert_eq!(coordinator.current(), Some(&replacement));
    assert_eq!(
        coordinator
            .resolve_at(
                KEY_B,
                ConflictResolution::AcceptIncoming,
                Some(replacement.clone()),
                1_000,
            )
            .unwrap(),
        ApplyOutcome::Applied(replacement)
    );
}

#[test]
fn coordinator_rejects_noncanonical_accept_incoming_resolution() {
    let mut coordinator = ManifestCoordinator::default();
    coordinator
        .submit(KEY_A, manifest(1, None, b"one"))
        .unwrap();
    coordinator
        .submit(KEY_B, manifest(1, None, b"two"))
        .unwrap();

    let error = coordinator
        .resolve_at(
            KEY_B,
            ConflictResolution::AcceptIncoming,
            Some(manifest(2, Some(1), b"different")),
            1_000,
        )
        .unwrap_err();

    assert_eq!(error.code, "invalid_resolution_manifest");
    assert_eq!(
        coordinator.current().unwrap().root_checksum,
        manifest(1, None, b"one").root_checksum
    );
}

#[test]
fn refuses_non_genesis_manifest_without_a_known_base() {
    let mut coordinator = ManifestCoordinator::default();
    let error = coordinator
        .submit(KEY_A, manifest(2, Some(1), b"two"))
        .unwrap_err();
    assert_eq!(error.code, "manifest_base_missing");
    assert!(coordinator.current().is_none());
}

#[test]
fn stale_manifest_becomes_conflict_without_rollback() {
    let mut coordinator = ManifestCoordinator::default();
    coordinator
        .submit(KEY_A, manifest(1, None, b"one"))
        .unwrap();
    coordinator
        .submit(KEY_B, manifest(2, Some(1), b"two"))
        .unwrap();

    let outcome = coordinator
        .submit("sync-attempt-key-0003", manifest(1, None, b"stale"))
        .unwrap();
    assert!(
        matches!(outcome, ApplyOutcome::Conflict(conflict) if conflict.kind == ConflictKind::StaleBase)
    );
    assert_eq!(coordinator.current().unwrap().generation, 2);
}
