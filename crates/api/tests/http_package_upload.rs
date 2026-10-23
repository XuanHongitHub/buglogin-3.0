use std::{
    fs,
    path::{Path, PathBuf},
};

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use buglogin_api::http::{ApiState, router};
use buglogin_browser::profile::ProfileService;
use buglogin_domain::{
    errors::Problem,
    ids::{AccountId, GrantId},
    permissions::{Action, Capability, Effect, Grant, ResourceScope},
};
use buglogin_migration::{
    MigrationKey,
    inspect::inspect_source,
    package::{
        NeutralPackage, NeutralPackageManifest, PackageMetadata, build_encrypted_profile_package,
    },
    signature::{SignatureMetadata, sign_package},
};
use buglogin_storage::{
    blob::checksum,
    local::LocalRepository,
    remote::{MemoryRemoteChunkStore, RemoteChunk, RemoteChunkStore},
    secrets::SecretValue,
};
use serde_json::Value;
use tower::ServiceExt;

static PACKAGE_UPLOAD_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct FailOnceRemoteChunkStore {
    inner: MemoryRemoteChunkStore,
    fail_next_put: bool,
}

impl RemoteChunkStore for FailOnceRemoteChunkStore {
    fn readiness(&self) -> Result<(), Problem> {
        self.inner.readiness()
    }

    fn checksum(&self, object_key: &str) -> Result<Option<String>, Problem> {
        self.inner.checksum(object_key)
    }

    fn put(
        &mut self,
        object_key: &str,
        bytes: &[u8],
        expected_checksum: &str,
    ) -> Result<(), Problem> {
        if self.fail_next_put {
            self.fail_next_put = false;
            return Err(Problem::new(
                "test_remote_put_failed",
                "Remote put failed",
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
fn root() -> PathBuf {
    std::env::temp_dir().join(format!(
        "buglogin-http-package-upload-{}",
        std::process::id()
    ))
}

fn signed_package(source: &Path, key: &[u8]) -> (String, NeutralPackage) {
    fs::create_dir_all(source).unwrap();
    fs::write(
        source.join("version.json"),
        r#"{"system":"buglogin-legacy","version":"1.1.84","instanceId":"package-upload-fixture","os":"windows","architecture":"x86_64"}"#,
    )
    .unwrap();
    fs::write(
        source.join("control-plane-export.json"),
        serde_json::to_vec(&control_plane_export()).unwrap(),
    )
    .unwrap();
    let (report, snapshot) = inspect_source(source).unwrap();
    let package = build_encrypted_profile_package(
        source,
        &report,
        &snapshot,
        &PackageMetadata {
            created_at: "2026-07-22T00:00:00Z".into(),
            tool_version: "0.1.0".into(),
            tool_commit: "test".into(),
        },
        &MigrationKey::from_bytes([61; 32]),
    )
    .unwrap();
    let package = sign_package(
        &package,
        key,
        SignatureMetadata {
            key_id: "test-key".into(),
            signer: "test".into(),
            signed_at: "2026-07-22T00:00:00Z".into(),
        },
    )
    .unwrap();
    let manifest: NeutralPackageManifest =
        serde_json::from_slice(package.file("manifest.json").unwrap()).unwrap();
    (manifest.package_id, package)
}

fn control_plane_export() -> Value {
    serde_json::json!({
        "organizations": [{
            "id": "organization-upload",
            "name": "Upload Migration",
            "slug": "upload-migration",
            "status": "active",
            "createdBy": "owner-upload",
            "createdAt": "2025-01-01T00:00:00Z",
            "updatedAt": "2025-01-01T00:00:00Z"
        }],
        "workspaces": [{
            "id": "workspace-upload",
            "organizationId": "organization-upload",
            "name": "Upload Workspace",
            "mode": "team",
            "createdAt": "2025-01-01T00:00:00Z",
            "createdBy": "owner-upload",
            "profileLimit": 10,
            "memberLimit": 10,
            "maxStorageBytes": 1048576,
            "entitlementState": "active",
            "localEntitlementState": "active"
        }],
        "memberships": [{
            "workspaceId": "workspace-upload",
            "userId": "owner-upload",
            "email": "owner-upload@example.test",
            "role": "owner",
            "resourceAccessMode": "full",
            "customCapabilities": null,
            "accountState": "active",
            "platformRole": null,
            "accessSource": "direct",
            "createdAt": "2025-01-01T00:00:00Z"
        }, {
            "workspaceId": "workspace-upload",
            "userId": "member-upload",
            "email": "member-upload@example.test",
            "role": "member",
            "resourceAccessMode": "restricted",
            "customCapabilities": [],
            "accountState": "active",
            "platformRole": null,
            "accessSource": "direct",
            "createdAt": "2025-01-02T00:00:00Z"
        }],
        "shareGrants": []

    })
}

fn upload_request(
    package_id: &str,
    path: &str,
    token: &str,
    idempotency_key: &str,
    bytes: &[u8],
) -> Request<Body> {
    Request::builder()
        .method("PUT")
        .uri(format!(
            "/v1/migrations/packages/{package_id}/files?path={}",
            path.replace('/', "%2F")
        ))
        .header("authorization", format!("Bearer {token}"))
        .header("x-correlation-id", "package-upload-test")
        .header("idempotency-key", idempotency_key)
        .header("x-content-sha256", checksum(bytes))
        .header("content-type", "application/octet-stream")
        .body(Body::from(bytes.to_vec()))
        .unwrap()
}

fn verify_request(package_id: &str, token: &str, key: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(format!("/v1/migrations/packages/{package_id}/verify"))
        .header("authorization", format!("Bearer {token}"))
        .header("x-correlation-id", "package-verify-test")
        .header("idempotency-key", key)
        .body(Body::empty())
        .unwrap()
}

fn activate_request(package_id: &str, token: &str, key: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(format!("/v1/migrations/packages/{package_id}/activate"))
        .header("authorization", format!("Bearer {token}"))
        .header("x-correlation-id", "package-activate-test")
        .header("idempotency-key", key)
        .body(Body::empty())
        .unwrap()
}

#[tokio::test]
async fn package_upload_retry_repairs_journaled_missing_blob() {
    let _test_guard = PACKAGE_UPLOAD_TEST_LOCK.lock().await;
    let root = root().with_extension("retry-missing-blob");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let signing_key = [77_u8; 32];
    let state = ApiState::new(ProfileService::new(LocalRepository::open(&root).unwrap()))
        .with_migration_signing_key(SecretValue::new(signing_key.to_vec()).unwrap())
        .with_remote_chunks(Box::new(FailOnceRemoteChunkStore {
            inner: MemoryRemoteChunkStore::default(),
            fail_next_put: true,
        }));
    let token = state.issue_token("acct_package_retry").unwrap();
    state.grants.write().unwrap().push(Grant {
        id: GrantId::from_str("grant_package_retry").unwrap(),
        account_id: Some(AccountId::from_str("acct_package_retry").unwrap()),
        member_id: None,
        capability: Capability::new("migration.write").unwrap(),
        action: Action::Execute,
        scope: ResourceScope::global(),
        effect: Effect::Allow,
    });
    let app = router(state);
    let (package_id, package) = signed_package(&root.join("source"), &signing_key);
    let (path, bytes) = package.files().iter().next().unwrap();

    let failed = app
        .clone()
        .oneshot(upload_request(
            &package_id,
            path,
            &token,
            "package-retry-missing-blob",
            bytes,
        ))
        .await
        .unwrap();
    assert_eq!(failed.status(), StatusCode::SERVICE_UNAVAILABLE);

    let repaired = app
        .oneshot(upload_request(
            &package_id,
            path,
            &token,
            "package-retry-missing-blob",
            bytes,
        ))
        .await
        .unwrap();
    let status = repaired.status();
    let bytes = axum::body::to_bytes(repaired.into_body(), 16 * 1024)
        .await
        .unwrap();
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["replayed"], true);
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn package_upload_accepts_files_above_axum_default_body_limit() {
    let _test_guard = PACKAGE_UPLOAD_TEST_LOCK.lock().await;
    let root = root().with_extension("large-file-limit");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let signing_key = [78_u8; 32];
    let state = ApiState::new(ProfileService::new(LocalRepository::open(&root).unwrap()))
        .with_migration_signing_key(SecretValue::new(signing_key.to_vec()).unwrap());
    let token = state.issue_token("acct_package_large").unwrap();
    state.grants.write().unwrap().push(Grant {
        id: GrantId::from_str("grant_package_large").unwrap(),
        account_id: Some(AccountId::from_str("acct_package_large").unwrap()),
        member_id: None,
        capability: Capability::new("migration.write").unwrap(),
        action: Action::Execute,
        scope: ResourceScope::global(),
        effect: Effect::Allow,
    });
    let app = router(state);
    let (package_id, _) = signed_package(&root.join("source"), &signing_key);
    let bytes = vec![7_u8; 3 * 1024 * 1024];
    let uploaded = app
        .oneshot(upload_request(
            &package_id,
            "large.bin",
            &token,
            "package-file-upload-large-body",
            &bytes,
        ))
        .await
        .unwrap();
    assert_eq!(uploaded.status(), StatusCode::OK);
    let receipt: Value = serde_json::from_slice(
        &axum::body::to_bytes(uploaded.into_body(), 16 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(receipt["size"], bytes.len());
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn signed_package_upload_is_cas_verified_replayable_and_immutable() {
    let _test_guard = PACKAGE_UPLOAD_TEST_LOCK.lock().await;
    let root = root();
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let signing_key = [77_u8; 32];
    let state = ApiState::new(ProfileService::new(LocalRepository::open(&root).unwrap()))
        .with_migration_signing_key(SecretValue::new(signing_key.to_vec()).unwrap());
    let token = state.issue_token("acct_package_upload").unwrap();
    state.grants.write().unwrap().push(Grant {
        id: GrantId::from_str("grant_package_upload").unwrap(),
        account_id: Some(AccountId::from_str("acct_package_upload").unwrap()),
        member_id: None,
        capability: Capability::new("migration.write").unwrap(),
        action: Action::Execute,
        scope: ResourceScope::global(),
        effect: Effect::Allow,
    });
    let app = router(state);
    let (package_id, package) = signed_package(&root.join("source"), &signing_key);

    for (index, (path, bytes)) in package.files().iter().enumerate() {
        let response = app
            .clone()
            .oneshot(upload_request(
                &package_id,
                path,
                &token,
                &format!("package-file-upload-{index:04}"),
                bytes,
            ))
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "upload failed for {path}"
        );
    }

    let (first_path, first_bytes) = package.files().iter().next().unwrap();
    let replay = app
        .clone()
        .oneshot(upload_request(
            &package_id,
            first_path,
            &token,
            "package-file-upload-replay",
            first_bytes,
        ))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::OK);
    let replay_body: Value = serde_json::from_slice(
        &axum::body::to_bytes(replay.into_body(), 16 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(replay_body["replayed"], true);

    let verified = app
        .clone()
        .oneshot(verify_request(&package_id, &token, "package-verify-0001"))
        .await
        .unwrap();
    assert_eq!(verified.status(), StatusCode::OK);
    let verified_body: Value = serde_json::from_slice(
        &axum::body::to_bytes(verified.into_body(), 16 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(verified_body["packageId"], package_id);
    assert_eq!(verified_body["state"], "verified");
    assert_eq!(verified_body["hasControl"], true);
    assert_eq!(verified_body["hasSync"], false);

    let verify_replay = app
        .clone()
        .oneshot(verify_request(&package_id, &token, "package-verify-replay"))
        .await
        .unwrap();
    assert_eq!(verify_replay.status(), StatusCode::OK);
    let verify_replay_body: Value = serde_json::from_slice(
        &axum::body::to_bytes(verify_replay.into_body(), 16 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(verify_replay_body["replayed"], true);

    let activated = app
        .clone()
        .oneshot(activate_request(
            &package_id,
            &token,
            "package-activate-0001",
        ))
        .await
        .unwrap();
    let activated_status = activated.status();
    let activated_bytes = axum::body::to_bytes(activated.into_body(), 16 * 1024)
        .await
        .unwrap();
    assert_eq!(
        activated_status,
        StatusCode::OK,
        "activation failed: {}",
        String::from_utf8_lossy(&activated_bytes)
    );
    let activated_body: Value = serde_json::from_slice(&activated_bytes).unwrap();
    assert_eq!(activated_body["state"], "active");
    assert!(activated_body["controlImport"].is_object());
    assert!(activated_body["syncImport"].is_null());

    let activation_replay = app
        .clone()
        .oneshot(activate_request(
            &package_id,
            &token,
            "package-activate-replay",
        ))
        .await
        .unwrap();
    assert_eq!(activation_replay.status(), StatusCode::OK);
    let activation_replay_body: Value = serde_json::from_slice(
        &axum::body::to_bytes(activation_replay.into_body(), 16 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(activation_replay_body["replayed"], true);

    let immutable = app
        .oneshot(upload_request(
            &package_id,
            "extra.json",
            &token,
            "package-file-after-verify",
            b"{}",
        ))
        .await
        .unwrap();
    assert_eq!(immutable.status(), StatusCode::CONFLICT);
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn package_verify_rejects_wrong_signing_key() {
    let _test_guard = PACKAGE_UPLOAD_TEST_LOCK.lock().await;
    let root = root().with_extension("wrong-key");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let trusted_key = [77_u8; 32];
    let state = ApiState::new(ProfileService::new(LocalRepository::open(&root).unwrap()))
        .with_migration_signing_key(SecretValue::new(trusted_key.to_vec()).unwrap());
    let token = state.issue_token("acct_package_wrong_key").unwrap();
    state.grants.write().unwrap().push(Grant {
        id: GrantId::from_str("grant_package_wrong_key").unwrap(),
        account_id: Some(AccountId::from_str("acct_package_wrong_key").unwrap()),
        member_id: None,
        capability: Capability::new("migration.write").unwrap(),
        action: Action::Execute,
        scope: ResourceScope::global(),
        effect: Effect::Allow,
    });
    let app = router(state);
    let (package_id, package) = signed_package(&root.join("source"), &[88_u8; 32]);
    for (index, (path, bytes)) in package.files().iter().enumerate() {
        let response = app
            .clone()
            .oneshot(upload_request(
                &package_id,
                path,
                &token,
                &format!("wrong-key-file-{index:04}"),
                bytes,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
    let rejected = app
        .oneshot(verify_request(
            &package_id,
            &token,
            "wrong-key-package-verify",
        ))
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::CONFLICT);
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(rejected.into_body(), 16 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(body["code"], "migration_signature_mismatch");
    let _ = fs::remove_dir_all(root);
}
