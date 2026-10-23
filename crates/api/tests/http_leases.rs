use std::{
    collections::BTreeMap,
    fs,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
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
        ManifestDependency, ManifestFileChunk, ManifestFileChunks, ManifestObject,
        ManifestObjectReference, ManifestPayloadEncryption, PayloadEncryptionAlgorithm,
        SyncManifest,
    },
    payload::{PayloadDescriptor, PayloadObject},
};
use futures_util::StreamExt;
use serde_json::{Value, json};
use tower::ServiceExt;

#[derive(Default)]
struct FakeCoordination {
    lease: Mutex<Option<LeaseGrant>>,
    replay: Mutex<BTreeMap<String, LeaseGrant>>,
    release_replay: Mutex<BTreeMap<String, String>>,
    manifest: Mutex<Option<SyncManifest>>,
    event_reads: Arc<AtomicUsize>,
    events: Arc<Mutex<Vec<StreamEvent>>>,
}

#[async_trait]
impl CoordinationBackend for FakeCoordination {
    async fn readiness(&self) -> Result<(), Problem> {
        Ok(())
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

    async fn renew_lease_idempotent(
        &self,
        _scope: CoordinationScope<'_>,
        _resource_id: &ResourceId,
        owner_token: &str,
        ttl: Duration,
        _idempotency_key: &str,
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

    async fn release_lease_idempotent(
        &self,
        scope: CoordinationScope<'_>,
        resource_id: &ResourceId,
        owner_token: &str,
        idempotency_key: &str,
    ) -> Result<(), Problem> {
        let fingerprint = format!(
            "{}|{}|{}",
            scope.auth.actor_id(),
            resource_id.as_str(),
            owner_token
        );
        let mut receipts = self.release_replay.lock().unwrap();
        if let Some(previous) = receipts.get(idempotency_key) {
            if previous != &fingerprint {
                return Err(Problem::new(
                    "idempotency_conflict",
                    "Idempotency key was reused with different input",
                    409,
                    "test",
                ));
            }
            return Ok(());
        }
        let mut lease = self.lease.lock().unwrap();
        if let Some(current) = lease.as_ref() {
            if current.owner_token != owner_token {
                return Err(Problem::new(
                    "lease_not_owner",
                    "Lease owner mismatch",
                    409,
                    "test",
                ));
            }
        }
        *lease = None;
        receipts.insert(idempotency_key.to_owned(), fingerprint);
        Ok(())
    }

    async fn lease_status(
        &self,
        _scope: CoordinationScope<'_>,
        resource_id: &ResourceId,
    ) -> Result<Option<buglogin_sync::coordination::LeaseStatus>, Problem> {
        Ok(self.lease.lock().unwrap().as_ref().and_then(|lease| {
            (&lease.resource_id == resource_id).then(|| buglogin_sync::coordination::LeaseStatus {
                resource_id: lease.resource_id.clone(),
                workspace_id: lease.workspace_id.clone(),
                owner_id: lease.owner_id.clone(),
                fencing_token: lease.fencing_token,
                issued_at_ms: lease.issued_at_ms,
                expires_at_ms: lease.expires_at_ms,
            })
        }))
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
        after_stream_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<StreamEvent>, Problem> {
        self.event_reads.fetch_add(1, Ordering::SeqCst);
        Ok(self
            .events
            .lock()
            .unwrap()
            .iter()
            .filter(|event| after_stream_id.is_none_or(|cursor| event.stream_id.as_str() > cursor))
            .take(limit)
            .cloned()
            .collect())
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
            "grant_proxy_update",
            "CAP-NET-001",
            Action::Update,
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
    let proxy_credential_version = CredentialId::new("credential-proxy_lease_1").unwrap();
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
                credential_ref: Some(proxy_credential_version.clone()),
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
    let absent_status = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(&path)
                .header("authorization", format!("Bearer {token}"))
                .header("x-correlation-id", "lease-status-absent-test")
                .header("x-buglogin-actor-id", account.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(absent_status.status(), StatusCode::OK);
    let absent: Value = serde_json::from_slice(
        &to_bytes(absent_status.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert!(absent.is_null());

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

    let lease_status = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(&path)
                .header("authorization", format!("Bearer {token}"))
                .header("x-correlation-id", "lease-status-test")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(lease_status.status(), StatusCode::OK);
    let status: Value = serde_json::from_slice(
        &to_bytes(lease_status.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(status["ownerId"], "desktop-1");
    assert!(status.get("ownerToken").is_none());

    let unauthorized_token = state.issue_token("acct_lease_no_access").unwrap();
    let unauthorized_status = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(&path)
                .header("authorization", format!("Bearer {unauthorized_token}"))
                .header("x-correlation-id", "lease-status-forbidden-test")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauthorized_status.status(), StatusCode::FORBIDDEN);

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
    let blob_check_path = format!(
        "/v1/workspaces/{}/resources/{}/blobs/check?key=profile%2Fpayload.bin&generation=1&checksum={blob_checksum}",
        workspace.as_str(),
        resource.as_str()
    );
    let blob_check_request = |correlation: &'static str, owner_token: &str| {
        Request::builder()
            .method("GET")
            .uri(&blob_check_path)
            .header("authorization", format!("Bearer {token}"))
            .header("x-correlation-id", correlation)
            .header("x-buglogin-owner-id", "desktop-1")
            .header("x-buglogin-owner-token", owner_token)
            .header(
                "x-buglogin-fencing-token",
                first["fencingToken"].as_u64().unwrap().to_string(),
            )
            .body(Body::empty())
            .unwrap()
    };
    let missing_blob_check = app
        .clone()
        .oneshot(blob_check_request(
            "blob-check-missing",
            first["ownerToken"].as_str().unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(missing_blob_check.status(), StatusCode::OK);
    let missing_blob_check: Value = serde_json::from_slice(
        &to_bytes(missing_blob_check.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(missing_blob_check["present"], false);
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
    let uploaded_blob_check = app
        .clone()
        .oneshot(blob_check_request(
            "blob-check-present",
            first["ownerToken"].as_str().unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(uploaded_blob_check.status(), StatusCode::OK);
    let uploaded_blob_check: Value = serde_json::from_slice(
        &to_bytes(uploaded_blob_check.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(uploaded_blob_check["present"], true);
    let stale_lease_check = app
        .clone()
        .oneshot(blob_check_request(
            "blob-check-stale-lease",
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        ))
        .await
        .unwrap();
    assert_eq!(stale_lease_check.status(), StatusCode::CONFLICT);
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

    let mut manifest = SyncManifest::new(
        workspace.clone(),
        resource.clone(),
        first["fencingToken"].as_u64().unwrap(),
        1,
        None,
        &blob,
        vec![
            ManifestObject::new("profile/payload.bin", &blob).unwrap(),
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
            credential_version: None,
            metadata_object: None,
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
        credential_version: Some(proxy_credential_version),
        metadata_object: None,
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
        .header("x-buglogin-actor-id", "acct_lease")
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

    let proxy_update = app
        .clone()
        .oneshot(request(
            "PATCH",
            &format!(
                "/v1/workspaces/{}/proxies/{}",
                workspace.as_str(),
                proxy_id.as_str()
            ),
            &token,
            Some("proxy-update-stale-blob-repair-0001"),
            json!({
                "workspaceId": workspace,
                "proxyId": proxy_id,
                "expectedRevision": 1,
                "proxy": {
                    "id": proxy_id,
                    "name": "Primary",
                    "protocol": "http",
                    "host": "proxy-updated.example",
                    "port": 8080,
                    "bypassRules": [],
                    "createdAt": "2026-07-22T00:00:00Z",
                    "updatedAt": "2026-07-22T00:00:00Z"
                }
            }),
        ))
        .await
        .unwrap();
    assert_eq!(proxy_update.status(), StatusCode::OK);

    let stale_manifest_read = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/v1/workspaces/{}/resources/{}/manifest?repairStaleProxy=true",
                    workspace.as_str(),
                    resource.as_str()
                ))
                .header("authorization", format!("Bearer {token}"))
                .header("x-correlation-id", "manifest-stale-proxy-repair")
                .header("x-buglogin-actor-id", "acct_lease")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(stale_manifest_read.status(), StatusCode::OK);

    let update_grants = {
        let mut current_grants = state.grants.write().unwrap();
        let mut removed = Vec::new();
        current_grants.retain(|grant| {
            let should_remove = matches!(
                grant.id.as_str(),
                "grant_profile_update_lease" | "grant_profile_update"
            );
            if should_remove {
                removed.push(grant.clone());
            }
            !should_remove
        });
        removed
    };
    let unauthorized_manifest_repair = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/v1/workspaces/{}/resources/{}/manifest?repairStaleProxy=true",
                    workspace.as_str(),
                    resource.as_str()
                ))
                .header("authorization", format!("Bearer {token}"))
                .header("x-correlation-id", "manifest-stale-proxy-repair-denied")
                .header("x-buglogin-actor-id", "acct_lease")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauthorized_manifest_repair.status(), StatusCode::FORBIDDEN);

    let unauthorized_blob_repair = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("{blob_path}&repairStaleProxy=true"))
                .header("authorization", format!("Bearer {token}"))
                .header("x-correlation-id", "blob-stale-proxy-repair-denied")
                .header("x-buglogin-actor-id", "acct_lease")
                .header("x-buglogin-generation", "1")
                .header("x-content-sha256", &blob_checksum)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauthorized_blob_repair.status(), StatusCode::FORBIDDEN);
    state.grants.write().unwrap().extend(update_grants);

    let stale_blob_without_repair = Request::builder()
        .method("GET")
        .uri(&blob_path)
        .header("authorization", format!("Bearer {token}"))
        .header("x-correlation-id", "blob-stale-proxy-no-repair")
        .header("x-buglogin-actor-id", "acct_lease")
        .header("x-buglogin-generation", "1")
        .header("x-content-sha256", &blob_checksum)
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone()
            .oneshot(stale_blob_without_repair)
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );

    let stale_blob_repair = Request::builder()
        .method("GET")
        .uri(format!("{blob_path}&repairStaleProxy=true"))
        .header("authorization", format!("Bearer {token}"))
        .header("x-correlation-id", "blob-stale-proxy-repair")
        .header("x-buglogin-actor-id", "acct_lease")
        .header("x-buglogin-generation", "1")
        .header("x-content-sha256", &blob_checksum)
        .body(Body::empty())
        .unwrap();
    let repaired_blob = app.clone().oneshot(stale_blob_repair).await.unwrap();
    assert_eq!(repaired_blob.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(repaired_blob.into_body(), usize::MAX)
            .await
            .unwrap()
            .as_ref(),
        blob
    );

    let repaired_credential_blob = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("{credential_blob_path}&repairStaleProxy=true"))
                .header("authorization", format!("Bearer {token}"))
                .header("x-correlation-id", "proxy-credential-stale-blob-repair")
                .header("x-buglogin-actor-id", "acct_lease")
                .header("x-buglogin-generation", "1")
                .header("x-content-sha256", &credential_checksum)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(repaired_credential_blob.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(repaired_credential_blob.into_body(), usize::MAX)
            .await
            .unwrap()
            .as_ref(),
        credential_bytes
    );

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
    let denied_credential_envelope_read = Request::builder()
        .method("GET")
        .uri(format!("{credential_blob_path}&repairStaleProxy=true"))
        .header("authorization", format!("Bearer {token}"))
        .header("x-correlation-id", "proxy-credential-envelope-read-revoked")
        .header("x-buglogin-actor-id", "acct_lease")
        .header("x-buglogin-generation", "1")
        .header("x-content-sha256", &credential_checksum)
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone()
            .oneshot(denied_credential_envelope_read)
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    state.grants.write().unwrap().push(proxy_read_grant);

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
    let release_conflict = app
        .clone()
        .oneshot(request(
            "DELETE",
            &path,
            &token,
            Some("lease-release-key-0001"),
            json!({
                "ownerId":"desktop-1",
                "ownerToken":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(release_conflict.status(), StatusCode::CONFLICT);
    let release_conflict: Value = serde_json::from_slice(
        &to_bytes(release_conflict.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(release_conflict["code"], "idempotency_conflict");
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
async fn proxy_update_is_rejected_while_bound_profile_is_launching() {
    let root = std::env::temp_dir().join(format!(
        "buglogin-http-proxy-launching-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    let state = ApiState::new(ProfileService::new(LocalRepository::open(&root).unwrap()));
    let token = state.issue_token("acct_proxy_launching").unwrap();
    let account = AccountId::new("acct_proxy_launching").unwrap();
    let workspace = WorkspaceId::new("ws_proxy_launching").unwrap();
    let resource = ResourceId::new("res_proxy_launching").unwrap();
    let profile_id = ProfileId::new("prf_proxy_launching").unwrap();
    let proxy_id = ProxyId::new("proxy_launching").unwrap();
    let mut grants = state.grants.write().unwrap();
    grants.extend([
        grant(
            &account,
            "grant_proxy_launch_create",
            "CAP-NET-001",
            Action::Create,
            ResourceScope::workspace(workspace.clone()),
        ),
        grant(
            &account,
            "grant_proxy_launch_read",
            "CAP-NET-001",
            Action::Read,
            ResourceScope::workspace(workspace.clone()),
        ),
        grant(
            &account,
            "grant_proxy_launch_update",
            "CAP-NET-001",
            Action::Update,
            ResourceScope::workspace(workspace.clone()),
        ),
    ]);
    let grant_snapshot = grants.clone();
    drop(grants);
    let principal = state.authenticate_token(&token, now()).unwrap();
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
                credential_ref: None,
                rotation_url: None,
                bypass_rules: Vec::new(),
                created_at: "2026-10-18T00:00:00Z".into(),
                updated_at: "2026-10-18T00:00:00Z".into(),
            },
            &principal,
            &grant_snapshot,
        )
        .unwrap();
    state
        .profiles
        .create(Profile {
            id: profile_id.clone(),
            name: "Launching proxy consumer".into(),
            runtime: Runtime::Wayfern,
            runtime_version: Some("1.0.0".into()),
            runtime_config: None,
            state: ProfileState::Launching,
            archived: false,
            fingerprint: None,
            note: None,
            proxy_id: Some(proxy_id.clone()),
            vpn_id: None,
            group_ids: Vec::new(),
            tags: Vec::new(),
            extension_group_id: None,
            sync_mode: Some(SyncMode::Managed),
            created_at: "2026-10-18T00:00:00Z".into(),
            updated_at: "2026-10-18T00:00:00Z".into(),
        })
        .unwrap();
    let state = state.with_profile_resource(ProfileResource {
        profile_id,
        resource_id: resource,
        workspace_id: workspace.clone(),
        archive_state: ProfileArchiveState::Active,
    });
    let app = router(state);
    let response = app
        .oneshot(request(
            "PATCH",
            &format!(
                "/v1/workspaces/{}/proxies/{}",
                workspace.as_str(),
                proxy_id.as_str()
            ),
            &token,
            Some("proxy-update-during-launch-0001"),
            json!({
                "workspaceId": workspace,
                "proxyId": proxy_id,
                "expectedRevision": 1,
                "proxy": {
                    "id": proxy_id,
                    "name": "Primary",
                    "protocol": "http",
                    "host": "proxy-updated.example",
                    "port": 8080,
                    "bypassRules": [],
                    "createdAt": "2026-10-18T00:00:00Z",
                    "updatedAt": "2026-10-18T00:00:00Z"
                }
            }),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(body["code"], "proxy_in_use");
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
    let profile_id = ProfileId::new("prf_manifest").unwrap();
    state
        .profiles
        .create(Profile {
            id: profile_id.clone(),
            name: "Manifest profile".into(),
            runtime: Runtime::Wayfern,
            runtime_version: Some("1.0.0".into()),
            runtime_config: None,
            state: ProfileState::Ready,
            archived: false,
            fingerprint: None,
            note: None,
            proxy_id: None,
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
            .with_profile_resource(ProfileResource {
                profile_id,
                resource_id: resource.clone(),
                workspace_id: workspace.clone(),
                archive_state: ProfileArchiveState::Active,
            })
            .with_coordination(Arc::new(FakeCoordination::default())),
    );
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

    let client_key = ClientKey::from_bytes([0x35; 32]);
    let recovery_key = RecoveryKey::from_bytes([0x47; 32]);
    let plaintext_chunks = [vec![0x5a; 4 * 1024 * 1024], vec![0x6b; 7]];
    let mut chunk_objects = Vec::new();
    let mut chunk_files = Vec::new();
    let file_path = "Default/Cookies";
    let mut whole_plaintext = Vec::new();
    let path_hash = checksum(file_path.as_bytes());
    let mut file_chunks = Vec::new();
    for (index, plaintext) in plaintext_chunks.iter().enumerate() {
        whole_plaintext.extend_from_slice(plaintext);
        let plaintext_checksum = checksum(plaintext);
        let aad = format!(
            "buglogin/profile-file-chunk/v1/{}/{}/{}/{index:016x}/{plaintext_checksum}",
            workspace.as_str(),
            resource.as_str(),
            file_path
        );
        let encrypted = AuthenticatedChunk::encrypt(&client_key, plaintext, aad.as_bytes())
            .unwrap()
            .encoded();
        let ciphertext_checksum = checksum(&encrypted);
        let object_key = format!(
            ".buglogin/file-chunks/{path_hash}/{index:016x}-{plaintext_checksum}-{ciphertext_checksum}"
        );
        let object = ManifestObject::new(object_key.clone(), &encrypted).unwrap();
        chunk_files.push((object_key, encrypted.clone()));
        chunk_objects.push(object.clone());
        file_chunks.push(ManifestFileChunk {
            index: index as u64,
            plaintext_size: plaintext.len() as u64,
            plaintext_checksum,
            object: ManifestObjectReference::from(&object),
        });
    }
    let envelope = ResourceKeyEnvelope::wrap(
        workspace.clone(),
        resource.clone(),
        &client_key,
        &recovery_key,
    )
    .unwrap();
    let envelope_bytes = serde_json::to_vec(&envelope).unwrap();
    let envelope_object =
        ManifestObject::new(".buglogin/resource-key-envelope.json", &envelope_bytes).unwrap();
    let mut all_objects = chunk_objects.clone();
    all_objects.push(envelope_object.clone());
    let descriptor = PayloadDescriptor::new_with_file_chunks(
        workspace.clone(),
        resource.clone(),
        2,
        all_objects
            .iter()
            .map(|object| PayloadObject {
                path: object.key.clone(),
                size: object.size,
                checksum: object.checksum.clone(),
            })
            .collect(),
        vec![ManifestFileChunks {
            path: file_path.into(),
            plaintext_size: whole_plaintext.len() as u64,
            plaintext_checksum: checksum(&whole_plaintext),
            chunks: file_chunks.clone(),
        }],
    )
    .unwrap();
    let chunked_manifest = SyncManifest::new_chunked(
        workspace.clone(),
        resource.clone(),
        1,
        2,
        Some(1),
        &descriptor.canonical_root_bytes(),
        all_objects,
        ManifestPayloadEncryption {
            schema_version: 1,
            algorithm: PayloadEncryptionAlgorithm::Aes256GcmChunkV2,
            key_version: Some(checksum(client_key.as_bytes())),
            resource_key_envelope: ManifestObjectReference::from(&envelope_object),
        },
        descriptor.file_chunks.clone(),
    )
    .unwrap();

    let owner_token = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let mut upload_objects = chunk_files;
    upload_objects.push((envelope_object.key.clone(), envelope_bytes));
    for (index, (key, bytes)) in upload_objects.into_iter().enumerate() {
        let object_checksum = checksum(&bytes);
        let encoded_key = key.replace('/', "%2F");
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!(
                        "/v1/workspaces/{}/resources/{}/blobs?key={encoded_key}",
                        workspace.as_str(),
                        resource.as_str()
                    ))
                    .header("authorization", format!("Bearer {token}"))
                    .header("x-correlation-id", "chunked-profile-upload")
                    .header("content-type", "application/octet-stream")
                    .header("idempotency-key", format!("chunk-object-upload-{index:04}"))
                    .header("x-buglogin-owner-id", "desktop-manifest")
                    .header("x-buglogin-owner-token", owner_token)
                    .header("x-buglogin-fencing-token", "1")
                    .header("x-buglogin-generation", "2")
                    .header("x-content-sha256", object_checksum)
                    .body(Body::from(bytes))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
    let chunked_commit_request = Request::builder()
        .method("PUT")
        .uri(&path)
        .header("authorization", format!("Bearer {token}"))
        .header("x-correlation-id", "manifest-v3-chunked-commit")
        .header("content-type", "application/json")
        .header("idempotency-key", "manifest-v3-chunked-commit-0001")
        .header("x-buglogin-owner-id", "desktop-manifest")
        .header("x-buglogin-owner-token", owner_token)
        .header("x-buglogin-fencing-token", "1")
        .body(Body::from(
            serde_json::to_string(&chunked_manifest).unwrap(),
        ))
        .unwrap();
    let chunked_commit = app.clone().oneshot(chunked_commit_request).await.unwrap();
    assert_eq!(chunked_commit.status(), StatusCode::OK);

    let get = Request::builder()
        .method("GET")
        .uri(&path)
        .header("authorization", format!("Bearer {token}"))
        .header("x-correlation-id", "manifest-http-read")
        .header("x-buglogin-actor-id", "acct_manifest")
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(get).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: SyncManifest =
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(body, chunked_manifest);
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn events_stream_reads_workspace_feed_from_coordination_backend() {
    let root = std::env::temp_dir().join(format!("buglogin-http-events-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let state = ApiState::new(ProfileService::new(LocalRepository::open(&root).unwrap()));
    let token = state.issue_token("acct_events").unwrap();
    let account = AccountId::new("acct_events").unwrap();
    let workspace = WorkspaceId::new("ws_events").unwrap();
    state.grants.write().unwrap().push(grant(
        &account,
        "grant_events_read",
        "events.read",
        Action::Read,
        ResourceScope::workspace(workspace.clone()),
    ));
    let coordination = Arc::new(FakeCoordination {
        events: Arc::new(Mutex::new(vec![StreamEvent {
            stream_id: "1001-0".into(),
            event: CoordinationEvent {
                event_id: "evt_profile_sync_1".into(),
                event_version: "1".into(),
                name: "profile.sync.committed".into(),
                occurred_at_ms: 1_001,
                correlation_id: "evt_profile_sync_1".into(),
                causation_id: "evt_profile_sync_1".into(),
                actor_id: "acct_events".into(),
                resource: buglogin_sync::coordination::EventResource {
                    resource_type: "profile".into(),
                    id: "prf_event_test".into(),
                    revision: 1,
                },
                data: json!({"generation": 1}),
            },
        }])),
        ..FakeCoordination::default()
    });
    let app = router(state.with_coordination(coordination.clone()));
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/v1/events?workspaceId={}", workspace.as_str()))
                .header("authorization", format!("Bearer {token}"))
                .header("x-correlation-id", "events-stream-test")
                .header("last-event-id", "1000-0")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    assert_eq!(coordination.event_reads.load(Ordering::SeqCst), 1);
    let first_chunk = response
        .into_body()
        .into_data_stream()
        .next()
        .await
        .unwrap()
        .unwrap();
    let first_event = String::from_utf8(first_chunk.to_vec()).unwrap();
    assert!(first_event.contains("id: 1001-0"));
    assert!(first_event.contains("event: profile.sync.committed"));
    assert!(first_event.contains("\"generation\":1"));
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
