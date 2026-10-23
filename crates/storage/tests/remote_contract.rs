use buglogin_storage::blob::checksum;
use buglogin_storage::remote::{MemoryRemoteChunkStore, RemoteChunkStore};

#[test]
fn remote_contract_enforces_checksum_idempotency_and_conflict() {
    let mut store = MemoryRemoteChunkStore::default();
    let bytes = b"encrypted chunk";
    let digest = checksum(bytes);

    store.put("tenant/workspace/chunk", bytes, &digest).unwrap();
    store.put("tenant/workspace/chunk", bytes, &digest).unwrap();
    assert_eq!(
        store.checksum("tenant/workspace/chunk").unwrap(),
        Some(digest.clone())
    );
    assert_eq!(
        store.get("tenant/workspace/chunk", &digest).unwrap().bytes,
        bytes
    );

    let conflict = store
        .put(
            "tenant/workspace/chunk",
            b"different",
            &checksum(b"different"),
        )
        .unwrap_err();
    assert_eq!(conflict.code, "remote_chunk_conflict");
}

#[test]
fn remote_contract_rejects_untrusted_keys() {
    let mut store = MemoryRemoteChunkStore::default();
    let error = store.put("../escape", b"x", &checksum(b"x")).unwrap_err();
    assert_eq!(error.code, "invalid_remote_chunk_key");
}

#[test]
fn remote_delete_is_checksum_cas_and_idempotent() {
    let mut store = MemoryRemoteChunkStore::default();
    let bytes = b"migration-owned";
    let digest = checksum(bytes);
    store.put("migration/object", bytes, &digest).unwrap();

    assert_eq!(
        store
            .delete_if_checksum("migration/object", &checksum(b"other"))
            .unwrap_err()
            .code,
        "remote_chunk_conflict"
    );
    assert_eq!(
        store.checksum("migration/object").unwrap(),
        Some(digest.clone())
    );
    assert!(
        store
            .delete_if_checksum("migration/object", &digest)
            .unwrap()
    );
    assert!(
        !store
            .delete_if_checksum("migration/object", &digest)
            .unwrap()
    );
}
