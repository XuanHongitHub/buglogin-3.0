use std::{
    collections::BTreeMap,
    fs,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use buglogin_api::http::{ApiState, router};
use buglogin_browser::profile::ProfileService;
use buglogin_domain::{
    entities::{Profile, ProfileState, Proxy, ProxyProtocol, Runtime, SyncMode},
    errors::Problem,
    ids::{AccountId, CredentialId, GrantId, ProfileId, ProxyId, ResourceId, WorkspaceId},
    permissions::{Action, Capability, Effect, Grant, ResourceScope},
    profile::{ProfileArchiveState, ProfileResource},
    resource::{Resource, ResourceKind, ResourceState},
};
use buglogin_storage::blob::checksum;
use buglogin_storage::local::LocalRepository;
use buglogin_sync::{
    coordination::{
        CoordinationBackend, CoordinationEvent, CoordinationScope, LeaseGrant, Presence,
        PublishedEvent, StreamEvent,
    },
    crypto::{
        AuthenticatedChunk, ClientKey, ProxyCredentialEnvelope, RecoveryKey, ResourceKeyEnvelope,
    },
    manifest::{
        ManifestDependency, ManifestObject, ManifestObjectReference, ManifestPayloadEncryption,
        PayloadEncryptionAlgorithm, SyncManifest,
    },
};
use serde_json::{Value, json};
use tower::ServiceExt;

#[derive(Default)]
struct FakeCoordination {
    lease: Mutex<Option<LeaseGrant>>,
    replay: Mutex<BTreeMap<String, LeaseGrant>>,
    manifest: Mutex<Option<SyncManifest>>,
}

#[async_trait]
impl CoordinationBackend for FakeCoordination {
    async fn acquire_lease(
        &self,
        scope: CoordinationScope<'_>,
        resource_id: &ResourceId,
        ttl: Duration,
    ) -> Result<LeaseGrant, Problem> {
        self.acquire_lease_idempotent(scope, resource_id, ttl, "fallback-idempotency-0001")
            .await
    }

    async fn acquire_lease_idempotent(
        &self,
        scope: CoordinationScope<'_>,
        resource_id: &ResourceId,
        ttl: Duration,
        idempotency_key: &str,
    ) -> Result<LeaseGrant, Problem> {
        if let Some(lease) = self.replay.lock().unwrap().get(idempotency_key).cloned() {
            return Ok(lease);
        }
        if self.lease.lock().unwrap().is_some() {
            return Err(Problem::new("lease_held", "Lease held", 409, "test"));
        }
        let lease = LeaseGrant {
            resource_id: resource_id.clone(),
            workspace_id: scope.workspace_id.clone(),
            owner_id: scope.auth.actor_id().to_owned(),
            owner_token: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
            fencing_token: 1,
            issued_at_ms: 1,
            expires_at_ms: 1 + ttl.as_millis() as u64,
        };
        *self.lease.lock().unwrap() = Some(lease.clone());
        self.replay
            .lock()
            .unwrap()
            .insert(idempotency_key.to_owned(), lease.clone());
        Ok(lease)
    }

    async fn renew_lease(
        &self,
        _scope: CoordinationScope<'_>,
        _resource_id: &ResourceId,
        owner_token: &str,
        ttl: Duration,
    ) -> Result<LeaseGrant, Problem> {
        let mut lease = self.lease.lock().unwrap();
        let current = lease.as_mut().ok_or_else(|| lease_missing())?;
        if current.owner_token != owner_token {
            return Err(Problem::new(
                "lease_not_owner",
                "Lease owner mismatch",
                409,
                "test",
            ));
        }
        current.expires_at_ms += ttl.as_millis() as u64;
        Ok(current.clone())
    }

    async fn release_lease(
        &self,
        _scope: CoordinationScope<'_>,
        _resource_id: &ResourceId,
        owner_token: &str,
    ) -> Result<(), Problem> {
        let mut lease = self.lease.lock().unwrap();
        let current = lease.as_ref().ok_or_else(lease_missing)?;
        if current.owner_token != owner_token {
            return Err(Problem::new(
                "lease_not_owner",
                "Lease owner mismatch",
                409,
                "test",
            ));
        }
        *lease = None;
        Ok(())
    }

    async fn validate_write_authority(
        &self,
        _scope: CoordinationScope<'_>,
        _resource_id: &ResourceId,
        owner_token: &str,
        fencing_token: u64,
    ) -> Result<(), Problem> {
        let lease = self.lease.lock().unwrap();
        let current = lease.as_ref().ok_or_else(lease_missing)?;
        if current.owner_token != owner_token {
            return Err(Problem::new(
                "lease_not_owner",
                "Lease owner mismatch",
                409,
                "test",
            ));
        }
        if current.fencing_token != fencing_token {
            return Err(Problem::new(
                "stale_fencing_token",
                "Stale fencing token",
                409,
                "test",
            ));
        }
        Ok(())
    }

    async fn heartbeat_presence(
        &self,
        _scope: CoordinationScope<'_>,
        _session_id: &str,
        _metadata: BTreeMap<String, String>,
        _ttl: Duration,
    ) -> Result<Presence, Problem> {
        Err(unsupported())
    }
    async fn list_presence(&self, _scope: CoordinationScope<'_>) -> Result<Vec<Presence>, Problem> {
        Err(unsupported())
    }
    async fn publish_event(
        &self,
        _scope: CoordinationScope<'_>,
        _idempotency_key: &str,
        _event: CoordinationEvent,
    ) -> Result<PublishedEvent, Problem> {
        Err(unsupported())
    }
    async fn read_events(
        &self,
        _scope: CoordinationScope<'_>,
        _after_stream_id: Option<&str>,
        _limit: usize,
    ) -> Result<Vec<StreamEvent>, Problem> {
        Err(unsupported())
    }
    async fn current_manifest(
        &self,
        _scope: CoordinationScope<'_>,
        _resource_id: &ResourceId,
    ) -> Result<Option<SyncManifest>, Problem> {
        Ok(self.manifest.lock().unwrap().clone())
    }
    async fn commit_manifest(
        &self,
        _scope: CoordinationScope<'_>,
        _resource_id: &ResourceId,
        owner_token: &str,
        manifest: SyncManifest,
    ) -> Result<(), Problem> {
        if owner_token != "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" {
            return Err(Problem::new(
                "lease_not_owner",
                "Lease owner mismatch",
                409,
                "test",
            ));
        }
        *self.manifest.lock().unwrap() = Some(manifest);
        Ok(())
    }
}

#[tokio::test]
async fn hosted_lease_http_flow_is_acl_scoped_and_retry_safe() {
    let root = std::env::temp_dir().join(format!("buglogin-http-lease-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let state = ApiState::new(ProfileService::new(LocalRepository::open(&root).unwrap()));
    let token = state.issue_token("acct_lease").unwrap();
    let account = AccountId::new("acct_lease").unwrap();
    let workspace = WorkspaceId::new("ws_lease").unwrap();
    let resource = ResourceId::new("res_lease").unwrap();
    let profile_id = ProfileId::new("prf_lease").unwrap();
    let mut grants = state.grants.write().unwrap();
    grants.extend([
        grant(
            &account,
            "grant_resource_create",
            "CAP-RES-001",
            Action::Create,
            ResourceScope::workspace(workspace.clone()),
        ),
        grant(
            &account,
            "grant_proxy_create",
            "CAP-NET-001",
            Action::Create,
            ResourceScope::workspace(workspace.clone()),
        ),
        grant(
            &account,
            "grant_proxy_read",
            "CAP-NET-001",
            Action::Read,
            ResourceScope::workspace(workspace.clone()),
        ),
        grant(
            &account,
            "grant_profile_launch",
            "profile.launch",
            Action::Execute,
            ResourceScope::resource(workspace.clone(), resource.clone()),
        ),
        grant(
            &account,
            "grant_profile_stop",
            "profile.stop",
            Action::Execute,
            ResourceScope::resource(workspace.clone(), resource.clone()),
        ),
        grant(
            &account,
            "grant_profile_read_lease",
            "profile.read",
            Action::Read,
            ResourceScope::resource(workspace.clone(), resource.clone()),
        ),
        grant(
            &account,
            "grant_profile_update_lease",
            "profile.update",
            Action::Update,
            ResourceScope::resource(workspace.clone(), resource.clone()),
        ),
        grant(
            &account,
            "grant_profile_read",
            "profile.read",
            Action::Read,
            ResourceScope::resource(workspace.clone(), resource.clone()),
        ),
        grant(
            &account,
            "grant_profile_update",
            "profile.update",
            Action::Update,
            ResourceScope::resource(workspace.clone(), resource.clone()),
        ),
        grant(
            &account,
            "grant_runtime_profile_read",
            "profile.read",
            Action::Read,
            ResourceScope {
                profile_id: Some(profile_id.clone()),
                ..ResourceScope::default()
            },
        ),
        grant(
            &account,
            "grant_runtime_profile_launch",
            "profile.launch",
            Action::Execute,
            ResourceScope {
                profile_id: Some(profile_id.clone()),
                ..ResourceScope::default()
            },
        ),
        grant(
            &account,
            "grant_runtime_profile_stop",
            "profile.stop",
            Action::Execute,
            ResourceScope {
                profile_id: Some(profile_id.clone()),
                ..ResourceScope::default()
            },
        ),
    ]);
    let grant_snapshot = grants.clone();
    drop(grants);
    let principal = state.authenticate_token(&token, now()).unwrap();
    state
        .resources
        .write()
        .unwrap()
        .register(
            &principal,
            &grant_snapshot,
            Resource {
                id: resource.clone(),
                workspace_id: workspace.clone(),
                kind: ResourceKind::Profile,
                state: ResourceState::Active,
                display_name: "Profile resource".into(),
            },
        )
        .unwrap();
    let proxy_id = ProxyId::new("proxy_lease_1").unwrap();
    state
        .proxies
        .write()
        .unwrap()
        .create(
            workspace.clone(),
            Proxy {
                id: proxy_id.clone(),
                name: Some("Primary".into()),
                protocol: ProxyProtocol::Http,
                host: "proxy.example".into(),
                port: 8080,
                credential_ref: Some(CredentialId::new("credential-proxy_lease_1").unwrap()),
                rotation_url: None,
                bypass_rules: Vec::new(),
                created_at: "2026-07-22T00:00:00Z".into(),
                updated_at: "2026-07-22T00:00:00Z".into(),
            },
            &principal,
            &grant_snapshot,
        )
        .unwrap();
    state
        .profiles
        .create(Profile {
            id: profile_id.clone(),
            name: "Managed profile".into(),
            runtime: Runtime::Wayfern,
            runtime_version: Some("1.0.0".into()),
            runtime_config: None,
            state: ProfileState::Ready,
            archived: false,
            fingerprint: None,
            note: None,
            proxy_id: Some(proxy_id.clone()),
            vpn_id: None,
            group_ids: Vec::new(),
            tags: Vec::new(),
            extension_group_id: None,
            sync_mode: Some(SyncMode::Managed),
            created_at: "2026-07-22T00:00:00Z".into(),
            updated_at: "2026-07-22T00:00:00Z".into(),
        })
        .unwrap();
    let app = router(
        state
            .clone()
            .with_profile_resource(ProfileResource {
                profile_id: profile_id.clone(),
                resource_id: resource.clone(),
                workspace_id: workspace.clone(),
                archive_state: ProfileArchiveState::Active,
            })
            .with_coordination(Arc::new(FakeCoordination::default())),
    );
    let path = format!(
        "/v1/workspaces/{}/resources/{}/lease",
        workspace.as_str(),
        resource.as_str()
    );
    let acquired = app
        .clone()
        .oneshot(request(
            "POST",
            &path,
            &token,
            Some("lease-idempotency-0001"),
            json!({"ownerId":"desktop-1","ttlSeconds":30}),
        ))
        .await
        .unwrap();
    assert_eq!(acquired.status(), StatusCode::OK);
    let first: Value =
        serde_json::from_slice(&to_bytes(acquired.into_body(), usize::MAX).await.unwrap()).unwrap();
    let replayed = app
        .clone()
        .oneshot(request(
            "POST",
            &path,
            &token,
            Some("lease-idempotency-0001"),
            json!({"ownerId":"desktop-1","ttlSeconds":30}),
        ))
        .await
        .unwrap();
    let replay: Value =
        serde_json::from_slice(&to_bytes(replayed.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(first["ownerToken"], replay["ownerToken"]);
    assert_eq!(first["fencingToken"], replay["fencingToken"]);

    let runtime_path = format!("/v1/profiles/{}/runtime/started", profile_id.as_str());
    let runtime_request = Request::builder()
        .method("PUT")
        .uri(&runtime_path)
        .header("authorization", format!("Bearer {token}"))
        .header("x-correlation-id", "runtime-started-test")
        .header("content-type", "application/json")
        .header("idempotency-key", "runtime-started-key-0001")
        .header("x-buglogin-owner-id", "desktop-1")
        .header(
            "x-buglogin-owner-token",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        )
        .header("x-buglogin-fencing-token", "1")
        .body(Body::from(
            json!({
                "deviceId": "desktop-1",
                "generation": 1,
                "rootChecksum": "a".repeat(64)
            })
            .to_string(),
        ))
        .unwrap();
    let runtime_response = app.clone().oneshot(runtime_request).await.unwrap();
    assert_eq!(runtime_response.status(), StatusCode::OK);
    let runtime_body: Value = serde_json::from_slice(
        &to_bytes(runtime_response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(runtime_body["state"], "running");
    assert_eq!(runtime_body["profileId"], profile_id.as_str());
    assert!(runtime_body.get("ownerToken").is_none());

    let runtime_get = Request::builder()
        .method("GET")
        .uri(format!("/v1/profiles/{}/runtime", profile_id.as_str()))
        .header("authorization", format!("Bearer {token}"))
        .header("x-correlation-id", "runtime-get-test")
        .body(Body::empty())
        .unwrap();
    let runtime_get_response = app.clone().oneshot(runtime_get).await.unwrap();
    assert_eq!(runtime_get_response.status(), StatusCode::OK);
    let persisted_runtime: Value = serde_json::from_slice(
        &to_bytes(runtime_get_response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(persisted_runtime["state"], "running");
    assert!(persisted_runtime.get("ownerToken").is_none());

    let payload_key = ClientKey::from_bytes([0x41; 32]);
    let recovery_key = RecoveryKey::from_bytes([0x52; 32]);
    let envelope = ResourceKeyEnvelope::wrap(
        workspace.clone(),
        resource.clone(),
        &payload_key,
        &recovery_key,
    )
    .unwrap();
    let envelope_bytes = serde_json::to_vec(&envelope).unwrap();
    let envelope_object =
        ManifestObject::new(".buglogin/resource-key-envelope.json", &envelope_bytes).unwrap();
    let payload_aad = format!(
        "buglogin/profile-object/v2/{}/{}/profile/payload.bin",
        workspace.as_str(),
        resource.as_str()
    );
    let blob =
        AuthenticatedChunk::encrypt(&payload_key, b"profile-payload", payload_aad.as_bytes())
            .unwrap()
            .encoded();
    let blob_checksum = checksum(&blob);
    let blob_path = format!(
        "/v1/workspaces/{}/resources/{}/blobs?key=profile%2Fpayload.bin",
        workspace.as_str(),
        resource.as_str()
    );
    let plaintext_blob = b"plaintext-cookie-values";
    let plaintext_checksum = checksum(plaintext_blob);
    let plaintext_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(&blob_path)
                .header("authorization", format!("Bearer {token}"))
                .header("x-correlation-id", "plaintext-blob-rejected")
                .header("content-type", "application/octet-stream")
                .header("idempotency-key", "plaintext-blob-key-0001")
                .header("x-buglogin-owner-id", "desktop-1")
                .header(
                    "x-buglogin-owner-token",
                    first["ownerToken"].as_str().unwrap(),
                )
                .header(
                    "x-buglogin-fencing-token",
                    first["fencingToken"].as_u64().unwrap().to_string(),
                )
                .header("x-buglogin-generation", "1")
                .header("x-content-sha256", &plaintext_checksum)
                .body(Body::from(plaintext_blob.to_vec()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(plaintext_response.status(), StatusCode::CONFLICT);
    let plaintext_response: Value = serde_json::from_slice(
        &to_bytes(plaintext_response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        plaintext_response["code"],
        "encrypted_payload_object_invalid"
    );
    let blob_put = Request::builder()
        .method("PUT")
        .uri(&blob_path)
        .header("authorization", format!("Bearer {token}"))
        .header("x-correlation-id", "blob-put")
        .header("content-type", "application/octet-stream")
        .header("idempotency-key", "blob-put-key-000001")
        .header("x-buglogin-owner-id", "desktop-1")
        .header(
            "x-buglogin-owner-token",
            first["ownerToken"].as_str().unwrap(),
        )
        .header(
            "x-buglogin-fencing-token",
            first["fencingToken"].as_u64().unwrap().to_string(),
        )
        .header("x-buglogin-generation", "1")
        .header("x-content-sha256", &blob_checksum)
        .body(Body::from(blob.clone()))
        .unwrap();
    assert_eq!(
        app.clone().oneshot(blob_put).await.unwrap().status(),
        StatusCode::OK
    );
    let blob_retry = Request::builder()
        .method("PUT")
        .uri(&blob_path)
        .header("authorization", format!("Bearer {token}"))
        .header("x-correlation-id", "blob-put-retry")
        .header("content-type", "application/octet-stream")
        .header("idempotency-key", "blob-put-key-000001")
        .header("x-buglogin-owner-id", "desktop-1")
        .header(
            "x-buglogin-owner-token",
            first["ownerToken"].as_str().unwrap(),
        )
        .header(
            "x-buglogin-fencing-token",
            first["fencingToken"].as_u64().unwrap().to_string(),
        )
        .header("x-buglogin-generation", "1")
        .header("x-content-sha256", &blob_checksum)
        .body(Body::from(blob.clone()))
        .unwrap();
    assert_eq!(
        app.clone().oneshot(blob_retry).await.unwrap().status(),
        StatusCode::OK
    );
    let changed_blob = b"different-payload";
    let changed_checksum = checksum(changed_blob);
    let blob_conflict = Request::builder()
        .method("PUT")
        .uri(&blob_path)
        .header("authorization", format!("Bearer {token}"))
        .header("x-correlation-id", "blob-put-conflict")
        .header("content-type", "application/octet-stream")
        .header("idempotency-key", "blob-put-key-000001")
        .header("x-buglogin-owner-id", "desktop-1")
        .header(
            "x-buglogin-owner-token",
            first["ownerToken"].as_str().unwrap(),
        )
        .header(
            "x-buglogin-fencing-token",
            first["fencingToken"].as_u64().unwrap().to_string(),
        )
        .header("x-buglogin-generation", "1")
        .header("x-content-sha256", changed_checksum)
        .body(Body::from(changed_blob.to_vec()))
        .unwrap();
    assert_eq!(
        app.clone().oneshot(blob_conflict).await.unwrap().status(),
        StatusCode::CONFLICT
    );

    let envelope_path = format!(
        "/v1/workspaces/{}/resources/{}/blobs?key=.buglogin%2Fresource-key-envelope.json",
        workspace.as_str(),
        resource.as_str()
    );
    let envelope_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(envelope_path)
                .header("authorization", format!("Bearer {token}"))
                .header("x-correlation-id", "resource-key-envelope-put")
                .header("content-type", "application/octet-stream")
                .header("idempotency-key", "resource-key-envelope-0001")
                .header("x-buglogin-owner-id", "desktop-1")
                .header(
                    "x-buglogin-owner-token",
                    first["ownerToken"].as_str().unwrap(),
                )
                .header(
                    "x-buglogin-fencing-token",
                    first["fencingToken"].as_u64().unwrap().to_string(),
                )
                .header("x-buglogin-generation", "1")
                .header("x-content-sha256", checksum(&envelope_bytes))
                .body(Body::from(envelope_bytes.clone()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(envelope_response.status(), StatusCode::OK);

    let proxy_metadata = json!({
        "workspaceId": workspace.as_str(),
        "proxy": {
            "id": proxy_id.as_str(),
            "name": "Primary",
            "protocol": "http",
            "host": "proxy.example",
            "port": 8080,
            "hasCredential": true,
            "rotationConfigured": false,
            "bypassRules": [],
            "createdAt": "2026-07-22T00:00:00Z",
            "updatedAt": "2026-07-22T00:00:00Z"
        },
        "revision": 1
    });
    let proxy_bytes = serde_json::to_vec(&proxy_metadata).unwrap();
    let proxy_checksum = checksum(&proxy_bytes);
    let proxy_key = "proxy/proxy_lease_1/metadata.json";
    let proxy_blob_path = format!(
        "/v1/workspaces/{}/resources/{}/blobs?key=proxy%2Fproxy_lease_1%2Fmetadata.json",
        workspace.as_str(),
        resource.as_str()
    );
    let proxy_blob_put = Request::builder()
        .method("PUT")
        .uri(&proxy_blob_path)
        .header("authorization", format!("Bearer {token}"))
        .header("x-correlation-id", "proxy-metadata-put")
        .header("content-type", "application/octet-stream")
        .header("idempotency-key", "proxy-meta-put-key-0001")
        .header("x-buglogin-owner-id", "desktop-1")
        .header(
            "x-buglogin-owner-token",
            first["ownerToken"].as_str().unwrap(),
        )
        .header(
            "x-buglogin-fencing-token",
            first["fencingToken"].as_u64().unwrap().to_string(),
        )
        .header("x-buglogin-generation", "1")
        .header("x-content-sha256", &proxy_checksum)
        .body(Body::from(proxy_bytes.clone()))
        .unwrap();
    assert_eq!(
        app.clone().oneshot(proxy_blob_put).await.unwrap().status(),
        StatusCode::OK
    );

    let proxy_credential = ProxyCredentialEnvelope::seal(
        workspace.clone(),
        resource.clone(),
        proxy_id.clone(),
        1,
        &payload_key,
        br#"{"username":"user","password":"secret"}"#,
    )
    .unwrap();
    let credential_bytes = serde_json::to_vec(&proxy_credential).unwrap();
    let credential_object = ManifestObject::new(
        ".buglogin/proxy-dependencies/proxy_lease_1/1/credential-envelope.json",
        &credential_bytes,
    )
    .unwrap();
    let credential_checksum = checksum(&credential_bytes);
    let credential_key = credential_object.key.clone();
    let credential_blob_path = format!(
        "/v1/workspaces/{}/resources/{}/blobs?key={}",
        workspace.as_str(),
        resource.as_str(),
        credential_key.replace('/', "%2F")
    );
    let credential_blob_put = Request::builder()
        .method("PUT")
        .uri(&credential_blob_path)
        .header("authorization", format!("Bearer {token}"))
        .header("x-correlation-id", "proxy-credential-envelope-put")
        .header("content-type", "application/octet-stream")
        .header("idempotency-key", "proxy-credential-envelope-put-0001")
        .header("x-buglogin-owner-id", "desktop-1")
        .header(
            "x-buglogin-owner-token",
            first["ownerToken"].as_str().unwrap(),
        )
        .header(
            "x-buglogin-fencing-token",
            first["fencingToken"].as_u64().unwrap().to_string(),
        )
        .header("x-buglogin-generation", "1")
        .header("x-content-sha256", &credential_checksum)
        .body(Body::from(credential_bytes.clone()))
        .unwrap();
    assert_eq!(
        app.clone()
            .oneshot(credential_blob_put)
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );

    let proxy_object = ManifestObject::new(proxy_key, &proxy_bytes).unwrap();
    let mut manifest = SyncManifest::new(
        workspace.clone(),
        resource.clone(),
        first["fencingToken"].as_u64().unwrap(),
        1,
        None,
        &blob,
        vec![
            ManifestObject::new("profile/payload.bin", &blob).unwrap(),
            proxy_object.clone(),
            credential_object.clone(),
            envelope_object.clone(),
        ],
    )
    .unwrap();
    manifest.payload_encryption = Some(ManifestPayloadEncryption {
        schema_version: 1,
        algorithm: PayloadEncryptionAlgorithm::Aes256GcmChunkV2,
        key_version: Some(checksum(payload_key.as_bytes())),
        resource_key_envelope: ManifestObjectReference::from(&envelope_object),
    });
    let rejected_missing_proxy = app
        .clone()
        .oneshot(manifest_request(
            &workspace,
            &resource,
            &token,
            first["ownerToken"].as_str().unwrap(),
            first["fencingToken"].as_u64().unwrap(),
            "manifest-missing-proxy-dep-0001",
            &manifest,
        ))
        .await
        .unwrap();
    assert_eq!(rejected_missing_proxy.status(), StatusCode::CONFLICT);
    let mut stale_dependency_manifest = manifest.clone();
    stale_dependency_manifest
        .dependencies
        .push(ManifestDependency::Proxy {
            proxy_id: proxy_id.clone(),
            revision: 2,
            metadata_object: ManifestObjectReference::from(&proxy_object),
            credential_envelope_object: None,
        });
    let rejected_stale_proxy = app
        .clone()
        .oneshot(manifest_request(
            &workspace,
            &resource,
            &token,
            first["ownerToken"].as_str().unwrap(),
            first["fencingToken"].as_u64().unwrap(),
            "manifest-stale-proxy-dep-0001",
            &stale_dependency_manifest,
        ))
        .await
        .unwrap();
    assert_eq!(rejected_stale_proxy.status(), StatusCode::CONFLICT);
    manifest.dependencies.push(ManifestDependency::Proxy {
        proxy_id: proxy_id.clone(),
        revision: 1,
        metadata_object: ManifestObjectReference::from(&proxy_object),
        credential_envelope_object: Some(ManifestObjectReference::from(&credential_object)),
    });
    let proxy_read_grant = {
        let mut current_grants = state.grants.write().unwrap();
        let grant = current_grants
            .iter()
            .find(|grant| grant.id.as_str() == "grant_proxy_read")
            .cloned()
            .unwrap();
        current_grants.retain(|grant| grant.id.as_str() != "grant_proxy_read");
        grant
    };
    let rejected_proxy_acl = app
        .clone()
        .oneshot(manifest_request(
            &workspace,
            &resource,
            &token,
            first["ownerToken"].as_str().unwrap(),
            first["fencingToken"].as_u64().unwrap(),
            "manifest-proxy-acl-deny-0001",
            &manifest,
        ))
        .await
        .unwrap();
    assert_eq!(rejected_proxy_acl.status(), StatusCode::FORBIDDEN);
    state.grants.write().unwrap().push(proxy_read_grant);
    let commit = manifest_request(
        &workspace,
        &resource,
        &token,
        first["ownerToken"].as_str().unwrap(),
        first["fencingToken"].as_u64().unwrap(),
        "manifest-blob-key-0001",
        &manifest,
    );
    assert_eq!(
        app.clone().oneshot(commit).await.unwrap().status(),
        StatusCode::OK
    );
    let mut stale_manifest = manifest.clone();
    stale_manifest.root_checksum = checksum(b"stale profile root");
    let stale_commit = app
        .clone()
        .oneshot(manifest_request(
            &workspace,
            &resource,
            &token,
            first["ownerToken"].as_str().unwrap(),
            first["fencingToken"].as_u64().unwrap(),
            "manifest-stale-generation-0001",
            &stale_manifest,
        ))
        .await
        .unwrap();
    assert_eq!(stale_commit.status(), StatusCode::CONFLICT);
    let blob_get = Request::builder()
        .method("GET")
        .uri(&blob_path)
        .header("authorization", format!("Bearer {token}"))
        .header("x-correlation-id", "blob-get")
        .header("x-buglogin-actor-id", "desktop-1")
        .header("x-buglogin-generation", "1")
        .header("x-content-sha256", &blob_checksum)
        .body(Body::empty())
        .unwrap();
    let blob_response = app.clone().oneshot(blob_get).await.unwrap();
    assert_eq!(blob_response.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(blob_response.into_body(), usize::MAX)
            .await
            .unwrap()
            .as_ref(),
        blob
    );

    let renewed = app
        .clone()
        .oneshot(request(
            "PUT",
            &path,
            &token,
            Some("lease-renew-key-0001"),
            json!({"ownerId":"desktop-1","ownerToken":first["ownerToken"],"ttlSeconds":30}),
        ))
        .await
        .unwrap();
    assert_eq!(renewed.status(), StatusCode::OK);
    let released = app
        .clone()
        .oneshot(request(
            "DELETE",
            &path,
            &token,
            Some("lease-release-key-0001"),
            json!({"ownerId":"desktop-1","ownerToken":first["ownerToken"]}),
        ))
        .await
        .unwrap();
    assert_eq!(released.status(), StatusCode::OK);
    let released_retry = app
        .clone()
        .oneshot(request(
            "DELETE",
            &path,
            &token,
            Some("lease-release-key-0001"),
            json!({"ownerId":"desktop-1","ownerToken":first["ownerToken"]}),
        ))
        .await
        .unwrap();
    assert_eq!(released_retry.status(), StatusCode::OK);
    let missing_key = app
        .oneshot(request(
            "POST",
            &path,
            &token,
            None,
            json!({"ownerId":"desktop-1","ttlSeconds":30}),
        ))
        .await
        .unwrap();
    assert_eq!(missing_key.status(), StatusCode::BAD_REQUEST);
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn hosted_manifest_http_flow_requires_fencing_and_owner_headers() {
    let root = std::env::temp_dir().join(format!("buglogin-http-manifest-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let state = ApiState::new(ProfileService::new(LocalRepository::open(&root).unwrap()));
    let token = state.issue_token("acct_manifest").unwrap();
    let account = AccountId::new("acct_manifest").unwrap();
    let workspace = WorkspaceId::new("ws_manifest").unwrap();
    let resource = ResourceId::new("res_manifest").unwrap();
    let mut grants = state.grants.write().unwrap();
    grants.push(grant(
        &account,
        "grant_resource_create_manifest",
        "CAP-RES-001",
        Action::Create,
        ResourceScope::workspace(workspace.clone()),
    ));
    grants.push(grant(
        &account,
        "grant_profile_read_manifest",
        "profile.read",
        Action::Read,
        ResourceScope::resource(workspace.clone(), resource.clone()),
    ));
    grants.push(grant(
        &account,
        "grant_profile_update_manifest",
        "profile.update",
        Action::Update,
        ResourceScope::resource(workspace.clone(), resource.clone()),
    ));
    grants.push(grant(
        &account,
        "grant_profile_launch_manifest",
        "profile.launch",
        Action::Execute,
        ResourceScope::resource(workspace.clone(), resource.clone()),
    ));
    let grant_snapshot = grants.clone();
    drop(grants);
    let principal = state.authenticate_token(&token, now()).unwrap();
    state
        .resources
        .write()
        .unwrap()
        .register(
            &principal,
            &grant_snapshot,
            Resource {
                id: resource.clone(),
                workspace_id: workspace.clone(),
                kind: ResourceKind::Profile,
                state: ResourceState::Active,
                display_name: "Manifest resource".into(),
            },
        )
        .unwrap();
    let app = router(state.with_coordination(Arc::new(FakeCoordination::default())));
    let path = format!(
        "/v1/workspaces/{}/resources/{}/manifest",
        workspace.as_str(),
        resource.as_str()
    );
    let lease_path = format!(
        "/v1/workspaces/{}/resources/{}/lease",
        workspace.as_str(),
        resource.as_str()
    );
    let lease_response = app
        .clone()
        .oneshot(request(
            "POST",
            &lease_path,
            &token,
            Some("manifest-lease-key-0001"),
            json!({"ownerId":"desktop-manifest","ttlSeconds":30}),
        ))
        .await
        .unwrap();
    assert_eq!(lease_response.status(), StatusCode::OK);
    let manifest = SyncManifest::new(
        workspace.clone(),
        resource.clone(),
        1,
        1,
        None,
        b"root",
        Vec::new(),
    )
    .unwrap();
    let request = Request::builder()
        .method("PUT")
        .uri(&path)
        .header("authorization", format!("Bearer {token}"))
        .header("x-correlation-id", "manifest-http-test")
        .header("content-type", "application/json")
        .header("idempotency-key", "manifest-key-0001")
        .header("x-buglogin-owner-id", "desktop-manifest")
        .header(
            "x-buglogin-owner-token",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        )
        .header("x-buglogin-fencing-token", "1")
        .body(Body::from(serde_json::to_string(&manifest).unwrap()))
        .unwrap();
    assert_eq!(
        app.clone().oneshot(request).await.unwrap().status(),
        StatusCode::OK
    );
    let unencrypted_object =
        ManifestObject::new("Default/Cookies", b"plaintext-cookie-data").unwrap();
    let unencrypted = SyncManifest::new(
        workspace.clone(),
        resource.clone(),
        1,
        2,
        Some(1),
        b"unencrypted profile root",
        vec![unencrypted_object],
    )
    .unwrap();
    let rejected = app
        .clone()
        .oneshot(manifest_request(
            &workspace,
            &resource,
            &token,
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            1,
            "manifest-unencrypted-key-0001",
            &unencrypted,
        ))
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::CONFLICT);
    let rejected: Value =
        serde_json::from_slice(&to_bytes(rejected.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(rejected["code"], "payload_encryption_required");

    let get = Request::builder()
        .method("GET")
        .uri(&path)
        .header("authorization", format!("Bearer {token}"))
        .header("x-correlation-id", "manifest-http-read")
        .header("x-buglogin-actor-id", "desktop-manifest")
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(get).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: SyncManifest =
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(body, manifest);
    let _ = fs::remove_dir_all(root);
}

fn request(method: &str, uri: &str, token: &str, key: Option<&str>, body: Value) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .header("x-correlation-id", "lease-http-test")
        .header("content-type", "application/json");
    if let Some(key) = key {
        builder = builder.header("idempotency-key", key);
    }
    builder.body(Body::from(body.to_string())).unwrap()
}

fn manifest_request(
    workspace: &WorkspaceId,
    resource: &ResourceId,
    token: &str,
    owner_token: &str,
    fencing_token: u64,
    idempotency_key: &str,
    manifest: &SyncManifest,
) -> Request<Body> {
    Request::builder()
        .method("PUT")
        .uri(format!(
            "/v1/workspaces/{}/resources/{}/manifest",
            workspace.as_str(),
            resource.as_str()
        ))
        .header("authorization", format!("Bearer {token}"))
        .header("x-correlation-id", "manifest-blob")
        .header("content-type", "application/json")
        .header("idempotency-key", idempotency_key)
        .header("x-buglogin-owner-id", "desktop-1")
        .header("x-buglogin-owner-token", owner_token)
        .header("x-buglogin-fencing-token", fencing_token.to_string())
        .body(Body::from(serde_json::to_string(manifest).unwrap()))
        .unwrap()
}

fn grant(
    account: &AccountId,
    id: &str,
    capability: &str,
    action: Action,
    scope: ResourceScope,
) -> Grant {
    Grant {
        id: GrantId::new(id).unwrap(),
        account_id: Some(account.clone()),
        member_id: None,
        capability: Capability::new(capability).unwrap(),
        action,
        scope,
        effect: Effect::Allow,
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}
fn lease_missing() -> Problem {
    Problem::new("lease_not_found", "Lease not found", 404, "test")
}
fn unsupported() -> Problem {
    Problem::new("unsupported", "Unsupported", 501, "test")
}
