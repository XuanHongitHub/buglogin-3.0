use buglogin_domain::ids::{ResourceId, WorkspaceId};
use buglogin_sync::{
    manifest::{ManifestObject, SyncManifest},
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
