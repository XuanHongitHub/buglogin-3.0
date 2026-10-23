use buglogin_domain::ids::{ResourceId, WorkspaceId};
use buglogin_storage::blob::checksum;
use buglogin_sync::{
    manifest::{
        ManifestFileChunk, ManifestFileChunks, ManifestObject, ManifestObjectReference,
        SyncManifest,
    },
    payload::{PayloadDescriptor, PayloadObject},
};

fn ids() -> (WorkspaceId, ResourceId) {
    (
        WorkspaceId::new("ws_payload").unwrap(),
        ResourceId::new("res_payload").unwrap(),
    )
}

#[test]
fn canonical_root_is_order_independent_at_construction() {
    let (workspace, resource) = ids();
    let first = PayloadDescriptor::new(
        workspace.clone(),
        resource.clone(),
        1,
        vec![
            PayloadObject::new("b/data", b"two").unwrap(),
            PayloadObject::new("a/data", b"one").unwrap(),
        ],
    )
    .unwrap();
    let second = PayloadDescriptor::new(
        workspace,
        resource,
        1,
        vec![
            PayloadObject::new("a/data", b"one").unwrap(),
            PayloadObject::new("b/data", b"two").unwrap(),
        ],
    )
    .unwrap();
    assert_eq!(first, second);
}

#[test]
fn rejects_duplicate_unsafe_and_tampered_objects() {
    let (workspace, resource) = ids();
    assert!(PayloadObject::new("../escape", b"x").is_err());
    assert!(PayloadObject::new("windows\\escape", b"x").is_err());
    let object = PayloadObject::new("profile/data", b"one").unwrap();
    let descriptor =
        PayloadDescriptor::new(workspace, resource, 1, vec![object.clone(), object]).unwrap_err();
    assert_eq!(descriptor.code, "payload_object_order_invalid");
}

#[test]
fn verifies_manifest_object_set_and_canonical_root() {
    let (workspace, resource) = ids();
    let descriptor = PayloadDescriptor::new(
        workspace.clone(),
        resource.clone(),
        1,
        vec![PayloadObject::new("profile/data", b"one").unwrap()],
    )
    .unwrap();
    let manifest = SyncManifest::new(
        workspace,
        resource,
        1,
        1,
        None,
        &descriptor.canonical_root_bytes(),
        vec![ManifestObject::new("profile/data", b"one").unwrap()],
    )
    .unwrap();
    descriptor.verify_manifest(&manifest).unwrap();
}

#[test]
fn detects_root_and_object_checksum_tampering() {
    let (workspace, resource) = ids();
    let mut descriptor = PayloadDescriptor::new(
        workspace,
        resource,
        1,
        vec![PayloadObject::new("profile/data", b"one").unwrap()],
    )
    .unwrap();
    descriptor.root_checksum = "0".repeat(64);
    assert_eq!(
        descriptor.validate().unwrap_err().code,
        "payload_root_checksum_mismatch"
    );
    assert_eq!(
        descriptor.objects[0].verify(b"two").unwrap_err().code,
        "payload_object_checksum_mismatch"
    );
}

#[test]
fn accepts_legacy_empty_genesis_root_for_existing_profiles() {
    let (workspace, resource) = ids();
    let mut descriptor =
        PayloadDescriptor::new(workspace.clone(), resource.clone(), 1, vec![]).unwrap();
    descriptor.root_checksum = checksum(&[]);
    descriptor.validate().unwrap();

    let manifest = SyncManifest::new(workspace, resource, 1, 1, None, b"", vec![]).unwrap();
    descriptor.verify_manifest(&manifest).unwrap();

    descriptor.generation = 2;
    assert_eq!(
        descriptor.validate().unwrap_err().code,
        "payload_root_checksum_mismatch"
    );
}

#[test]
fn canonical_root_binds_file_chunk_mapping() {
    let (workspace, resource) = ids();
    let encrypted_chunk =
        PayloadObject::new(".buglogin/file-chunks/pathhash/chunk-object", b"ciphertext").unwrap();
    let file = ManifestFileChunks {
        path: "Default/Cookies".into(),
        plaintext_size: 3,
        plaintext_checksum: "a".repeat(64),
        chunks: vec![ManifestFileChunk {
            index: 0,
            plaintext_size: 3,
            plaintext_checksum: "b".repeat(64),
            object: ManifestObjectReference {
                key: encrypted_chunk.path.clone(),
                size: encrypted_chunk.size,
                checksum: encrypted_chunk.checksum.clone(),
            },
        }],
    };
    let with_mapping = PayloadDescriptor::new_with_file_chunks(
        workspace.clone(),
        resource.clone(),
        1,
        vec![encrypted_chunk.clone()],
        vec![file.clone()],
    )
    .unwrap();
    let without_mapping =
        PayloadDescriptor::new(workspace, resource, 1, vec![encrypted_chunk]).unwrap();
    assert_ne!(with_mapping.root_checksum, without_mapping.root_checksum);

    let mut tampered = with_mapping;
    tampered.file_chunks[0].path = "Default/Preferences".into();
    assert_eq!(
        tampered.validate().unwrap_err().code,
        "payload_root_checksum_mismatch"
    );
}
