use buglogin_domain::errors::Problem;
use buglogin_storage::remote::{MemoryRemoteChunkStore, RemoteChunk, RemoteChunkStore};
use buglogin_sync::{
    chunks::{download_and_decrypt, encrypt_chunks, upload_resumable},
    crypto::{ClientKey, ProxyCredentialEnvelope, RecoveryKey, ResourceKeyEnvelope},
};

#[test]
fn round_trip_uses_client_key_and_remote_sees_only_ciphertext() {
    let key = ClientKey::from_bytes([7; 32]);
    let plaintext = b"client-only profile secret";
    let chunks = encrypt_chunks(&key, "workspace/profile", plaintext, 8).unwrap();
    assert!(
        chunks
            .iter()
            .all(|chunk| !chunk.bytes.windows(6).any(|window| window == b"client"))
    );

    let mut remote = MemoryRemoteChunkStore::default();
    let receipt = upload_resumable(&mut remote, &chunks, 2).unwrap();
    assert_eq!(receipt.uploaded, chunks.len());
    assert_eq!(
        download_and_decrypt(&remote, &key, &chunks).unwrap(),
        plaintext
    );
}

#[test]
fn wrong_key_and_tampering_fail_authentication() {
    let key = ClientKey::from_bytes([1; 32]);
    let wrong_key = ClientKey::from_bytes([2; 32]);
    let mut chunks = encrypt_chunks(&key, "workspace/profile", b"sensitive", 64).unwrap();
    assert_eq!(
        chunks[0].decrypt(&wrong_key).unwrap_err().code,
        "chunk_authentication_failed"
    );

    let last = chunks[0].bytes.len() - 1;
    chunks[0].bytes[last] ^= 1;
    assert_eq!(
        chunks[0].decrypt(&key).unwrap_err().code,
        "encrypted_chunk_checksum_mismatch"
    );
}

#[test]
fn retry_resumes_without_duplicate_upload() {
    let key = ClientKey::from_bytes([3; 32]);
    let chunks = encrypt_chunks(&key, "workspace/profile", b"abcdefghijkl", 4).unwrap();
    let mut remote = FlakyRemote::default();

    let first = upload_resumable(&mut remote, &chunks[..2], 1).unwrap();
    assert_eq!(first.uploaded, 2);
    let resumed = upload_resumable(&mut remote, &chunks, 1).unwrap();
    assert_eq!(resumed.resumed, 2);
    assert_eq!(resumed.uploaded, 1);
    assert_eq!(resumed.retries, 1);
    assert_eq!(
        download_and_decrypt(&remote.inner, &key, &chunks).unwrap(),
        b"abcdefghijkl"
    );
}

#[test]
fn resource_key_envelope_recovers_key_only_for_its_resource() {
    let workspace = buglogin_domain::ids::WorkspaceId::from_str("ws_alpha").unwrap();
    let other_workspace = buglogin_domain::ids::WorkspaceId::from_str("ws_beta").unwrap();
    let resource = buglogin_domain::ids::ResourceId::from_str("res_alpha").unwrap();
    let other_resource = buglogin_domain::ids::ResourceId::from_str("res_beta").unwrap();
    let client_key = ClientKey::from_bytes([0x5a; 32]);
    let recovery_key = RecoveryKey::from_bytes([0xa5; 32]);
    let envelope = ResourceKeyEnvelope::wrap(
        workspace.clone(),
        resource.clone(),
        &client_key,
        &recovery_key,
    )
    .unwrap();

    let json = serde_json::to_vec(&envelope).unwrap();
    let decoded: ResourceKeyEnvelope = serde_json::from_slice(&json).unwrap();
    let recovered = decoded
        .unwrap(&workspace, &resource, &recovery_key)
        .unwrap();
    assert_eq!(recovered.as_bytes(), client_key.as_bytes());
    assert_eq!(
        decoded
            .unwrap(&other_workspace, &resource, &recovery_key)
            .unwrap_err()
            .code,
        "resource_key_envelope_scope_mismatch"
    );
    assert_eq!(
        decoded
            .unwrap(&workspace, &other_resource, &recovery_key)
            .unwrap_err()
            .code,
        "resource_key_envelope_scope_mismatch"
    );

    let wrong_recovery_key = RecoveryKey::from_bytes([0x33; 32]);
    assert_eq!(
        decoded
            .unwrap(&workspace, &resource, &wrong_recovery_key)
            .unwrap_err()
            .code,
        "chunk_authentication_failed"
    );
}

#[test]
fn proxy_credential_envelope_is_bound_to_profile_proxy_and_revision() {
    let workspace = buglogin_domain::ids::WorkspaceId::from_str("ws_alpha").unwrap();
    let resource = buglogin_domain::ids::ResourceId::from_str("res_alpha").unwrap();
    let proxy = buglogin_domain::ids::ProxyId::from_str("prx_alpha").unwrap();
    let key = ClientKey::from_bytes([0x42; 32]);
    let secret = br#"{"username":"user","password":"pass"}"#;
    let envelope = ProxyCredentialEnvelope::seal(
        workspace.clone(),
        resource.clone(),
        proxy.clone(),
        7,
        &key,
        secret,
    )
    .unwrap();

    let encoded = serde_json::to_vec(&envelope).unwrap();
    assert!(!encoded.windows(4).any(|window| window == b"pass"));
    let decoded: ProxyCredentialEnvelope = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(
        decoded
            .open(&workspace, &resource, &proxy, 7, &key)
            .unwrap(),
        secret
    );
    assert_eq!(
        decoded
            .open(&workspace, &resource, &proxy, 8, &key)
            .unwrap_err()
            .code,
        "proxy_credential_envelope_scope_mismatch"
    );
    assert_eq!(
        decoded
            .open(
                &workspace,
                &resource,
                &buglogin_domain::ids::ProxyId::from_str("prx_beta").unwrap(),
                7,
                &key,
            )
            .unwrap_err()
            .code,
        "proxy_credential_envelope_scope_mismatch"
    );
}

#[test]
fn recovery_code_round_trips_and_debug_redacts_secret() {
    let key = RecoveryKey::from_bytes([0x2f; 32]);
    let code = key.recovery_code();
    assert_eq!(code.len(), 64);
    assert_eq!(
        RecoveryKey::parse_code(&code).unwrap().recovery_code(),
        code
    );
    assert_eq!(format!("{key:?}"), "RecoveryKey(<redacted>)");
    assert!(RecoveryKey::parse_code(&"é".repeat(32)).is_err());
}

#[derive(Default)]
struct FlakyRemote {
    inner: MemoryRemoteChunkStore,
    failed_once: bool,
}

impl RemoteChunkStore for FlakyRemote {
    fn checksum(&self, object_key: &str) -> Result<Option<String>, Problem> {
        self.inner.checksum(object_key)
    }

    fn put(
        &mut self,
        object_key: &str,
        bytes: &[u8],
        expected_checksum: &str,
    ) -> Result<(), Problem> {
        if !self.failed_once && object_key.contains("0000000000000002") {
            self.failed_once = true;
            return Err(
                Problem::new("network_interrupted", "Network interrupted", 503, "test")
                    .retryable(true),
            );
        }
        self.inner.put(object_key, bytes, expected_checksum)
    }

    fn get(&self, object_key: &str, expected_checksum: &str) -> Result<RemoteChunk, Problem> {
        self.inner.get(object_key, expected_checksum)
    }
}
