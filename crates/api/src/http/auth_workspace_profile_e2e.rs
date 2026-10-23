use super::{ApiState, FirstOwnerBootstrapInput, router};
use crate::services::profile_hosted::HostedProfileService;
use async_trait::async_trait;
use axum::{body::Body, http::Request, http::StatusCode};
use buglogin_browser::profile::ProfileService;
use buglogin_domain::{errors::Problem, ids::ProfileId};
use buglogin_storage::control::SqliteControlRecordStore;
use buglogin_storage::remote::LocalRemoteChunkStore;
use buglogin_sync::{
    coordination::{
        CoordinationBackend, CoordinationScope, LeaseGrant, Presence, PublishedEvent, StreamEvent,
    },
    crypto::{AuthenticatedChunk, ClientKey, RecoveryKey, ResourceKeyEnvelope},
    manifest::{
        ManifestDependency, ManifestObject, ManifestObjectReference, ManifestPayloadEncryption,
        PayloadEncryptionAlgorithm, RESOURCE_KEY_ENVELOPE_KEY, SyncManifest,
    },
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tower::ServiceExt;
use zeroize::Zeroizing;

const TEST_PASSWORD: &str = "isolated-e2e-owner-password-2026";

#[derive(Default)]
struct E2eCoordination {
    state: Mutex<E2eCoordinationState>,
}

#[derive(Default)]
struct E2eCoordinationState {
    lease: Option<LeaseGrant>,
    next_fencing_token: u64,
    acquire_replays: BTreeMap<String, LeaseGrant>,
}

#[async_trait]
impl CoordinationBackend for E2eCoordination {
    async fn readiness(&self) -> Result<(), Problem> {
        Ok(())
    }

    async fn acquire_lease_idempotent(
        &self,
        scope: CoordinationScope<'_>,
        resource_id: &buglogin_domain::ids::ResourceId,
        ttl: Duration,
        idempotency_key: &str,
    ) -> Result<LeaseGrant, Problem> {
        let mut state = self.state.lock().unwrap();
        if let Some(lease) = state.acquire_replays.get(idempotency_key) {
            return Ok(lease.clone());
        }
        if state.lease.is_some() {
            return Err(Problem::new(
                "lease_held",
                "Another device currently owns the profile lease",
                409,
                "e2e",
            ));
        }
        state.next_fencing_token = state.next_fencing_token.saturating_add(1).max(1);
        let fencing_token = state.next_fencing_token;
        let lease = LeaseGrant {
            resource_id: resource_id.clone(),
            workspace_id: scope.workspace_id.clone(),
            owner_id: scope.auth.actor_id().to_owned(),
            owner_token: format!("e2e-coordination-owner-token-{fencing_token:03}"),
            fencing_token,
            issued_at_ms: 1,
            expires_at_ms: 1 + ttl.as_millis() as u64,
        };
        state.lease = Some(lease.clone());
        state
            .acquire_replays
            .insert(idempotency_key.to_owned(), lease.clone());
        Ok(lease)
    }

    async fn renew_lease_idempotent(
        &self,
        _scope: CoordinationScope<'_>,
        _resource_id: &buglogin_domain::ids::ResourceId,
        _owner_token: &str,
        _ttl: Duration,
        _idempotency_key: &str,
    ) -> Result<LeaseGrant, Problem> {
        Err(unsupported_coordination_operation())
    }

    async fn release_lease_idempotent(
        &self,
        scope: CoordinationScope<'_>,
        resource_id: &buglogin_domain::ids::ResourceId,
        owner_token: &str,
        _idempotency_key: &str,
    ) -> Result<(), Problem> {
        let mut state = self.state.lock().unwrap();
        let current = state.lease.as_ref().ok_or_else(|| {
            Problem::new(
                "lease_not_found",
                "Profile lease was already released",
                404,
                "e2e",
            )
        })?;
        if &current.resource_id != resource_id
            || current.owner_id != scope.auth.actor_id()
            || current.owner_token != owner_token
        {
            return Err(Problem::new(
                "lease_not_owner",
                "The device does not own this profile lease",
                409,
                "e2e",
            ));
        }
        state.lease = None;
        Ok(())
    }

    async fn lease_status(
        &self,
        _scope: CoordinationScope<'_>,
        _resource_id: &buglogin_domain::ids::ResourceId,
    ) -> Result<Option<buglogin_sync::coordination::LeaseStatus>, Problem> {
        Ok(None)
    }

    async fn validate_write_authority(
        &self,
        _scope: CoordinationScope<'_>,
        resource_id: &buglogin_domain::ids::ResourceId,
        owner_token: &str,
        fencing_token: u64,
    ) -> Result<(), Problem> {
        let state = self.state.lock().unwrap();
        if state.lease.as_ref().is_some_and(|lease| {
            &lease.resource_id == resource_id
                && lease.owner_token == owner_token
                && lease.fencing_token == fencing_token
        }) {
            return Ok(());
        }
        Err(Problem::new(
            "stale_fencing_token",
            "Lease authority is invalid",
            409,
            "e2e",
        ))
    }

    async fn heartbeat_presence(
        &self,
        _scope: CoordinationScope<'_>,
        _session_id: &str,
        _metadata: BTreeMap<String, String>,
        _ttl: Duration,
    ) -> Result<Presence, Problem> {
        Err(unsupported_coordination_operation())
    }

    async fn list_presence(&self, _scope: CoordinationScope<'_>) -> Result<Vec<Presence>, Problem> {
        Err(unsupported_coordination_operation())
    }

    async fn publish_event(
        &self,
        _scope: CoordinationScope<'_>,
        _idempotency_key: &str,
        _event: buglogin_sync::coordination::CoordinationEvent,
    ) -> Result<PublishedEvent, Problem> {
        Err(unsupported_coordination_operation())
    }

    async fn read_events(
        &self,
        _scope: CoordinationScope<'_>,
        _after_stream_id: Option<&str>,
        _limit: usize,
    ) -> Result<Vec<StreamEvent>, Problem> {
        Err(unsupported_coordination_operation())
    }

    async fn current_manifest(
        &self,
        _scope: CoordinationScope<'_>,
        _resource_id: &buglogin_domain::ids::ResourceId,
    ) -> Result<Option<SyncManifest>, Problem> {
        Err(unsupported_coordination_operation())
    }

    async fn commit_manifest(
        &self,
        _scope: CoordinationScope<'_>,
        _resource_id: &buglogin_domain::ids::ResourceId,
        _owner_token: &str,
        _manifest: SyncManifest,
    ) -> Result<(), Problem> {
        Err(unsupported_coordination_operation())
    }
}

fn unsupported_coordination_operation() -> Problem {
    Problem::new(
        "unsupported_e2e_coordination_operation",
        "This E2E coordination operation is not used",
        500,
        "e2e",
    )
}

fn owner_input() -> FirstOwnerBootstrapInput {
    FirstOwnerBootstrapInput {
        account_id: None,
        username: "E2E.Owner".into(),
        email: "e2e.owner@example.test".into(),
        operator_attested_email: true,
        password: Zeroizing::new(TEST_PASSWORD.into()),
        organization_name: "Isolated E2E Organization".into(),
        workspace_name: "Isolated E2E Workspace".into(),
        max_workspaces: 2,
        max_profiles: 10,
        max_members: 2,
        max_storage_bytes: 1_048_576,
    }
}

fn json_request(
    method: &str,
    uri: &str,
    body: Value,
    token: Option<&str>,
    idempotency_key: Option<&str>,
    correlation_id: &str,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .header("x-correlation-id", correlation_id);
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    if let Some(key) = idempotency_key {
        builder = builder.header("idempotency-key", key);
    }
    builder
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

fn get_request(uri: &str, token: &str, correlation_id: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .header("x-correlation-id", correlation_id)
        .body(Body::empty())
        .unwrap()
}

async fn response_json(response: axum::response::Response) -> Value {
    serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap()
}

fn authenticated_request(
    method: &str,
    uri: &str,
    token: &str,
    correlation_id: &str,
) -> axum::http::request::Builder {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .header("x-correlation-id", correlation_id)
}

#[tokio::test]
async fn desktop_login_workspace_list_and_profile_crud_use_isolated_api_state() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let data_root =
        std::env::temp_dir().join(format!("buglogin-api-e2e-{}-{nonce}", std::process::id()));
    fs::create_dir_all(&data_root).unwrap();
    let control_path = data_root.join("control.sqlite");
    let chunks_path = data_root.join("chunks");
    let state = ApiState::hosted_with_control_store(
        ProfileService::disabled(),
        SqliteControlRecordStore::open(&control_path).unwrap(),
    )
    .unwrap()
    .with_concurrent_remote_chunks(Box::new(LocalRemoteChunkStore::new(&chunks_path).unwrap()))
    .with_coordination(Arc::new(E2eCoordination::default()));
    let owner = state.bootstrap_first_owner(owner_input()).unwrap();
    let app = router(state);

    let login = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/v1/auth/desktop/login",
            json!({
                "identifier": "e2e.owner@example.test",
                "password": TEST_PASSWORD,
                "deviceId": "e2e-device-local-001"
            }),
            None,
            None,
            "e2e-login-001",
        ))
        .await
        .unwrap();
    assert_eq!(login.status(), StatusCode::OK);
    let login_body = response_json(login).await;
    let token = login_body["accessToken"].as_str().unwrap();
    assert_eq!(login_body["tokenType"], "Bearer");

    let workspaces = app
        .clone()
        .oneshot(get_request(
            &format!(
                "/v1/organizations/{}/workspaces",
                owner.organization_id.as_str()
            ),
            token,
            "e2e-workspaces-001",
        ))
        .await
        .unwrap();
    assert_eq!(workspaces.status(), StatusCode::OK);
    let workspace_body = response_json(workspaces).await;
    let workspace = workspace_body
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == owner.workspace_id.as_str())
        .expect("bootstrapped workspace is visible to the signed-in owner");
    assert_eq!(workspace["name"], "Isolated E2E Workspace");

    let proxy_create = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/v1/workspaces/{}/proxies", owner.workspace_id.as_str()),
            json!({
                "workspaceId": owner.workspace_id.as_str(),
                "proxy": {
                    "id": "proxy_e2e_primary",
                    "name": "E2E primary",
                    "protocol": "http",
                    "host": "127.0.0.1",
                    "port": 3128,
                    "bypassRules": [],
                    "createdAt": "2026-10-18T00:00:00Z",
                    "updatedAt": "2026-10-18T00:00:00Z"
                }
            }),
            Some(token),
            Some("e2e-proxy-create-001"),
            "e2e-proxy-create-correlation",
        ))
        .await
        .unwrap();
    assert_eq!(proxy_create.status(), StatusCode::OK);
    let proxy_body = response_json(proxy_create).await;
    assert_eq!(proxy_body["record"]["proxy"]["id"], "proxy_e2e_primary");

    let mut implicit_group_create = json_request(
        "POST",
        "/v1/profiles",
        json!({
            "id": "prf_e2e_implicit_group",
            "name": "Implicit group assignment",
            "runtime": "wayfern",
            "runtimeVersion": null,
            "runtimeConfig": null,
            "state": "created",
            "archived": false,
            "fingerprint": null,
            "note": null,
            "proxyId": null,
            "vpnId": null,
            "groupIds": ["grp_e2e_implicit_acl"],
            "tags": [],
            "extensionGroupId": null,
            "syncMode": "managed",
            "createdAt": "2026-10-19T00:00:00Z",
            "updatedAt": "2026-10-19T00:00:00Z"
        }),
        Some(token),
        Some("e2e-profile-implicit-group-001"),
        "e2e-profile-implicit-group-correlation",
    );
    implicit_group_create.headers_mut().insert(
        "x-buglogin-workspace-id",
        owner.workspace_id.as_str().parse().unwrap(),
    );
    let implicit_group_response = app.clone().oneshot(implicit_group_create).await.unwrap();
    assert_eq!(implicit_group_response.status(), StatusCode::BAD_REQUEST);
    let implicit_group_problem = response_json(implicit_group_response).await;
    assert_eq!(
        implicit_group_problem["code"],
        "profile_groups_mutation_required"
    );
    let profile_after_rejection = app
        .clone()
        .oneshot(get_request(
            "/v1/profiles/prf_e2e_implicit_group",
            token,
            "e2e-profile-implicit-group-readback",
        ))
        .await
        .unwrap();
    assert_eq!(profile_after_rejection.status(), StatusCode::NOT_FOUND);

    let create = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/v1/profiles",
            json!({
                "name": "Isolated E2E Profile",
                "runtime": "wayfern",
                "workspaceId": owner.workspace_id.as_str(),
                "proxyId": "proxy_e2e_primary"
            }),
            Some(token),
            Some("e2e-profile-create-001"),
            "e2e-profile-create-correlation",
        ))
        .await
        .unwrap();
    assert_eq!(create.status(), StatusCode::CREATED);
    let created = response_json(create).await;
    let profile_id = created["value"]["id"].as_str().unwrap().to_owned();
    assert_eq!(created["value"]["name"], "Isolated E2E Profile");
    assert_eq!(created["value"]["proxyId"], "proxy_e2e_primary");
    assert_eq!(created["revision"], 1);
    let resource_id =
        HostedProfileService::managed_resource_id(&ProfileId::from_str(&profile_id).unwrap())
            .unwrap();
    let manifest_path = format!(
        "/v1/workspaces/{}/resources/{}/manifest",
        owner.workspace_id.as_str(),
        resource_id.as_str()
    );
    let manifest = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(&manifest_path)
                .header("authorization", format!("Bearer {token}"))
                .header("x-buglogin-actor-id", owner.account_id.as_str())
                .header("x-correlation-id", "e2e-initial-manifest")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(manifest.status(), StatusCode::OK);
    let initial_manifest = response_json(manifest).await;
    assert_eq!(initial_manifest["generation"], 1);
    assert_eq!(initial_manifest["objects"], json!([]));

    let proxy_update = app
        .clone()
        .oneshot(json_request(
            "PATCH",
            &format!(
                "/v1/workspaces/{}/proxies/proxy_e2e_primary",
                owner.workspace_id.as_str()
            ),
            json!({
                "workspaceId": owner.workspace_id.as_str(),
                "proxyId": "proxy_e2e_primary",
                "expectedRevision": 1,
                "proxy": {
                    "id": "proxy_e2e_primary",
                    "name": "E2E edited",
                    "protocol": "http",
                    "host": "127.0.0.1",
                    "port": 3128,
                    "bypassRules": [],
                    "createdAt": "2026-10-18T00:00:00Z",
                    "updatedAt": "2026-10-18T00:01:00Z"
                }
            }),
            Some(token),
            Some("e2e-proxy-update-001"),
            "e2e-proxy-update-correlation",
        ))
        .await
        .unwrap();
    assert_eq!(proxy_update.status(), StatusCode::OK);
    let updated_proxy = response_json(proxy_update).await;
    assert_eq!(updated_proxy["revision"], 2);

    let profile_update = app
        .clone()
        .oneshot(json_request(
            "PUT",
            &format!(
                "/v1/workspaces/{}/profiles/{profile_id}",
                owner.workspace_id.as_str()
            ),
            json!({
                "workspaceId": owner.workspace_id.as_str(),
                "profileId": profile_id,
                "expectedRevision": 1,
                "name": "Isolated E2E Profile edited",
                "note": "Updated through hosted API",
                "tags": ["e2e", "profile"]
            }),
            Some(token),
            Some("e2e-profile-update-001"),
            "e2e-profile-update-correlation",
        ))
        .await
        .unwrap();
    assert_eq!(profile_update.status(), StatusCode::OK);
    let updated_profile = response_json(profile_update).await;
    assert_eq!(updated_profile["revision"], 2);

    let owner_id = "e2e-device-local-001";
    let lease = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!(
                "/v1/workspaces/{}/resources/{}/lease",
                owner.workspace_id.as_str(),
                resource_id.as_str()
            ),
            json!({"ownerId": owner_id, "ttlSeconds": 120}),
            Some(token),
            Some("e2e-profile-lease-001"),
            "e2e-profile-lease-correlation",
        ))
        .await
        .unwrap();
    assert_eq!(lease.status(), StatusCode::OK);
    let lease_body = response_json(lease).await;
    assert_eq!(lease_body["ownerId"], owner_id);
    let owner_token = lease_body["ownerToken"].as_str().unwrap();
    let fencing_token = lease_body["fencingToken"].as_u64().unwrap();

    let profile_key = ClientKey::from_bytes([7; 32]);
    let recovery_key = RecoveryKey::from_bytes([8; 32]);
    let resource_key_envelope = ResourceKeyEnvelope::wrap(
        owner.workspace_id.clone(),
        resource_id.clone(),
        &profile_key,
        &recovery_key,
    )
    .unwrap();
    let envelope_bytes = serde_json::to_vec(&resource_key_envelope).unwrap();
    let payload_bytes = AuthenticatedChunk::encrypt(
        &profile_key,
        b"e2e encrypted cookie database payload",
        b"buglogin-e2e-cookie-database-v1",
    )
    .unwrap()
    .encoded();
    let envelope_object = ManifestObject::new(RESOURCE_KEY_ENVELOPE_KEY, &envelope_bytes).unwrap();
    let payload_object = ManifestObject::new("Default/Cookies", &payload_bytes).unwrap();

    for (object, bytes, key) in [
        (
            &envelope_object,
            envelope_bytes.as_slice(),
            "e2e-profile-envelope-upload-001",
        ),
        (
            &payload_object,
            payload_bytes.as_slice(),
            "e2e-profile-payload-upload-001",
        ),
    ] {
        let upload = authenticated_request(
            "PUT",
            &format!(
                "/v1/workspaces/{}/resources/{}/blobs?key={}",
                owner.workspace_id.as_str(),
                resource_id.as_str(),
                object.key
            ),
            token,
            &format!("{key}-correlation"),
        )
        .header("content-type", "application/octet-stream")
        .header("idempotency-key", key)
        .header("x-buglogin-owner-id", owner_id)
        .header("x-buglogin-owner-token", owner_token)
        .header("x-buglogin-fencing-token", fencing_token.to_string())
        .header("x-buglogin-generation", "2")
        .header("x-content-sha256", &object.checksum)
        .body(Body::from(bytes.to_vec()))
        .unwrap();
        let uploaded = app.clone().oneshot(upload).await.unwrap();
        assert_eq!(uploaded.status(), StatusCode::OK);
        let receipt = response_json(uploaded).await;
        assert_eq!(receipt["checksum"], object.checksum);
        assert_eq!(receipt["size"], bytes.len() as u64);
    }

    let mut synced_manifest = SyncManifest::new(
        owner.workspace_id.clone(),
        resource_id.clone(),
        fencing_token,
        2,
        Some(1),
        b"e2e profile root at generation two",
        vec![envelope_object.clone(), payload_object.clone()],
    )
    .unwrap();
    synced_manifest.payload_encryption = Some(ManifestPayloadEncryption {
        schema_version: 1,
        algorithm: PayloadEncryptionAlgorithm::Aes256GcmChunkV1,
        key_version: None,
        resource_key_envelope: ManifestObjectReference::from(&envelope_object),
    });
    synced_manifest
        .dependencies
        .push(ManifestDependency::Proxy {
            proxy_id: buglogin_domain::ids::ProxyId::from_str("proxy_e2e_primary").unwrap(),
            revision: 2,
            credential_version: None,
            metadata_object: None,
            credential_envelope_object: None,
        });
    let commit = authenticated_request("PUT", &manifest_path, token, "e2e-profile-manifest-commit")
        .header("content-type", "application/json")
        .header("idempotency-key", "e2e-profile-manifest-commit-001")
        .header("x-buglogin-owner-id", owner_id)
        .header("x-buglogin-owner-token", owner_token)
        .header("x-buglogin-fencing-token", fencing_token.to_string())
        .header("x-buglogin-profile-revision", "2")
        .body(Body::from(serde_json::to_vec(&synced_manifest).unwrap()))
        .unwrap();
    let committed = app.clone().oneshot(commit).await.unwrap();
    assert_eq!(committed.status(), StatusCode::OK);
    let committed_manifest: SyncManifest = serde_json::from_slice(
        &axum::body::to_bytes(committed.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(committed_manifest.generation, 2);
    assert_eq!(committed_manifest.fencing_token, fencing_token);
    assert_eq!(committed_manifest.objects.len(), 2);

    let second_login = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/v1/auth/desktop/login",
            json!({
                "identifier": "e2e.owner@example.test",
                "password": TEST_PASSWORD,
                "deviceId": "e2e-device-remote-002"
            }),
            None,
            None,
            "e2e-login-device-two",
        ))
        .await
        .unwrap();
    assert_eq!(second_login.status(), StatusCode::OK);
    let second_login_body = response_json(second_login).await;
    let second_token = second_login_body["accessToken"].as_str().unwrap();
    assert_ne!(
        second_token, token,
        "each desktop receives its own session token"
    );

    let second_device_manifest = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(&manifest_path)
                .header("authorization", format!("Bearer {second_token}"))
                .header("x-buglogin-actor-id", owner.account_id.as_str())
                .header("x-correlation-id", "e2e-device-two-read-generation-two")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(second_device_manifest.status(), StatusCode::OK);
    let second_device_manifest: SyncManifest = serde_json::from_slice(
        &axum::body::to_bytes(second_device_manifest.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(second_device_manifest, committed_manifest);

    let second_device_download = authenticated_request(
        "GET",
        &format!(
            "/v1/workspaces/{}/resources/{}/blobs?key=Default%2FCookies",
            owner.workspace_id.as_str(),
            resource_id.as_str()
        ),
        second_token,
        "e2e-device-two-download-generation-two",
    )
    .header("x-buglogin-actor-id", owner.account_id.as_str())
    .header("x-buglogin-generation", "2")
    .header("x-content-sha256", &payload_object.checksum)
    .body(Body::empty())
    .unwrap();
    let second_device_download = app.clone().oneshot(second_device_download).await.unwrap();
    assert_eq!(second_device_download.status(), StatusCode::OK);
    let second_device_download =
        axum::body::to_bytes(second_device_download.into_body(), usize::MAX)
            .await
            .unwrap();
    assert_eq!(second_device_download.as_ref(), payload_bytes.as_slice());

    let release_first_lease = app
        .clone()
        .oneshot(json_request(
            "DELETE",
            &format!(
                "/v1/workspaces/{}/resources/{}/lease",
                owner.workspace_id.as_str(),
                resource_id.as_str()
            ),
            json!({"ownerId": owner_id, "ownerToken": owner_token}),
            Some(token),
            Some("e2e-profile-lease-release-device-one"),
            "e2e-profile-lease-release-device-one",
        ))
        .await
        .unwrap();
    assert_eq!(release_first_lease.status(), StatusCode::OK);

    let second_lease = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!(
                "/v1/workspaces/{}/resources/{}/lease",
                owner.workspace_id.as_str(),
                resource_id.as_str()
            ),
            json!({"ownerId": "e2e-device-remote-002", "ttlSeconds": 120}),
            Some(second_token),
            Some("e2e-profile-lease-device-two"),
            "e2e-profile-lease-device-two",
        ))
        .await
        .unwrap();
    assert_eq!(second_lease.status(), StatusCode::OK);
    let second_lease_body = response_json(second_lease).await;
    let second_owner_token = second_lease_body["ownerToken"].as_str().unwrap();
    let second_fencing_token = second_lease_body["fencingToken"].as_u64().unwrap();
    assert_eq!(second_lease_body["ownerId"], "e2e-device-remote-002");
    assert_eq!(second_fencing_token, fencing_token + 1);

    let generation_three_plaintext = b"updated encrypted cookie database from device two";
    let generation_three_payload_bytes = AuthenticatedChunk::encrypt(
        &profile_key,
        generation_three_plaintext,
        b"buglogin-e2e-cookie-database-v1",
    )
    .unwrap()
    .encoded();
    let generation_three_payload_object =
        ManifestObject::new("Default/Cookies", &generation_three_payload_bytes).unwrap();
    let stale_device_upload = authenticated_request(
        "PUT",
        &format!(
            "/v1/workspaces/{}/resources/{}/blobs?key=Default%2FCookies",
            owner.workspace_id.as_str(),
            resource_id.as_str()
        ),
        token,
        "e2e-device-one-stale-generation-three-upload",
    )
    .header("content-type", "application/octet-stream")
    .header(
        "idempotency-key",
        "e2e-device-one-stale-generation-three-upload",
    )
    .header("x-buglogin-owner-id", owner_id)
    .header("x-buglogin-owner-token", owner_token)
    .header("x-buglogin-fencing-token", fencing_token.to_string())
    .header("x-buglogin-generation", "3")
    .header(
        "x-content-sha256",
        &generation_three_payload_object.checksum,
    )
    .body(Body::from(generation_three_payload_bytes.clone()))
    .unwrap();
    let stale_device_upload = app.clone().oneshot(stale_device_upload).await.unwrap();
    assert_eq!(stale_device_upload.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json(stale_device_upload).await["code"],
        "stale_fencing_token"
    );

    let generation_three_envelope_object =
        ManifestObject::new(RESOURCE_KEY_ENVELOPE_KEY, &envelope_bytes).unwrap();
    let mut generation_three_manifest = SyncManifest::new(
        owner.workspace_id.clone(),
        resource_id.clone(),
        second_fencing_token,
        3,
        Some(2),
        b"e2e profile root at generation three",
        vec![
            generation_three_envelope_object.clone(),
            generation_three_payload_object.clone(),
        ],
    )
    .unwrap();
    generation_three_manifest.payload_encryption = Some(ManifestPayloadEncryption {
        schema_version: 1,
        algorithm: PayloadEncryptionAlgorithm::Aes256GcmChunkV1,
        key_version: None,
        resource_key_envelope: ManifestObjectReference::from(&generation_three_envelope_object),
    });
    generation_three_manifest
        .dependencies
        .push(ManifestDependency::Proxy {
            proxy_id: buglogin_domain::ids::ProxyId::from_str("proxy_e2e_primary").unwrap(),
            revision: 2,
            credential_version: None,
            metadata_object: None,
            credential_envelope_object: None,
        });

    let mut stale_device_manifest = generation_three_manifest.clone();
    stale_device_manifest.fencing_token = fencing_token;
    let stale_device_commit = authenticated_request(
        "PUT",
        &manifest_path,
        token,
        "e2e-device-one-stale-generation-three-commit",
    )
    .header("content-type", "application/json")
    .header(
        "idempotency-key",
        "e2e-device-one-stale-generation-three-commit",
    )
    .header("x-buglogin-owner-id", owner_id)
    .header("x-buglogin-owner-token", owner_token)
    .header("x-buglogin-fencing-token", fencing_token.to_string())
    .header("x-buglogin-profile-revision", "2")
    .body(Body::from(
        serde_json::to_vec(&stale_device_manifest).unwrap(),
    ))
    .unwrap();
    let stale_device_commit = app.clone().oneshot(stale_device_commit).await.unwrap();
    assert_eq!(stale_device_commit.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json(stale_device_commit).await["code"],
        "stale_fencing_token"
    );

    let upload_generation_three_envelope = authenticated_request(
        "PUT",
        &format!(
            "/v1/workspaces/{}/resources/{}/blobs?key={}",
            owner.workspace_id.as_str(),
            resource_id.as_str(),
            ".buglogin%2Fresource-key-envelope.json"
        ),
        second_token,
        "e2e-device-two-generation-three-envelope-upload",
    )
    .header("content-type", "application/octet-stream")
    .header(
        "idempotency-key",
        "e2e-device-two-generation-three-envelope-upload",
    )
    .header("x-buglogin-owner-id", "e2e-device-remote-002")
    .header("x-buglogin-owner-token", second_owner_token)
    .header("x-buglogin-fencing-token", second_fencing_token.to_string())
    .header("x-buglogin-generation", "3")
    .header(
        "x-content-sha256",
        &generation_three_envelope_object.checksum,
    )
    .body(Body::from(envelope_bytes.clone()))
    .unwrap();
    let upload_generation_three_envelope = app
        .clone()
        .oneshot(upload_generation_three_envelope)
        .await
        .unwrap();
    assert_eq!(upload_generation_three_envelope.status(), StatusCode::OK);

    let upload_generation_three = authenticated_request(
        "PUT",
        &format!(
            "/v1/workspaces/{}/resources/{}/blobs?key=Default%2FCookies",
            owner.workspace_id.as_str(),
            resource_id.as_str()
        ),
        second_token,
        "e2e-device-two-generation-three-upload",
    )
    .header("content-type", "application/octet-stream")
    .header("idempotency-key", "e2e-device-two-generation-three-upload")
    .header("x-buglogin-owner-id", "e2e-device-remote-002")
    .header("x-buglogin-owner-token", second_owner_token)
    .header("x-buglogin-fencing-token", second_fencing_token.to_string())
    .header("x-buglogin-generation", "3")
    .header(
        "x-content-sha256",
        &generation_three_payload_object.checksum,
    )
    .body(Body::from(generation_three_payload_bytes.clone()))
    .unwrap();
    let upload_generation_three = app.clone().oneshot(upload_generation_three).await.unwrap();
    assert_eq!(upload_generation_three.status(), StatusCode::OK);

    let commit_generation_three = authenticated_request(
        "PUT",
        &manifest_path,
        second_token,
        "e2e-device-two-generation-three-commit",
    )
    .header("content-type", "application/json")
    .header("idempotency-key", "e2e-device-two-generation-three-commit")
    .header("x-buglogin-owner-id", "e2e-device-remote-002")
    .header("x-buglogin-owner-token", second_owner_token)
    .header("x-buglogin-fencing-token", second_fencing_token.to_string())
    .header("x-buglogin-profile-revision", "2")
    .body(Body::from(
        serde_json::to_vec(&generation_three_manifest).unwrap(),
    ))
    .unwrap();
    let commit_generation_three = app.clone().oneshot(commit_generation_three).await.unwrap();
    assert_eq!(commit_generation_three.status(), StatusCode::OK);
    let generation_three_manifest: SyncManifest = serde_json::from_slice(
        &axum::body::to_bytes(commit_generation_three.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(generation_three_manifest.generation, 3);
    assert_eq!(
        generation_three_manifest.fencing_token,
        second_fencing_token
    );

    let listed = app
        .clone()
        .oneshot(get_request(
            &format!(
                "/v1/profiles?workspaceId={}&limit=10",
                owner.workspace_id.as_str()
            ),
            token,
            "e2e-profile-list-001",
        ))
        .await
        .unwrap();
    assert_eq!(listed.status(), StatusCode::OK);
    let listed_body = response_json(listed).await;
    let listed_profile = listed_body["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["value"]["id"] == profile_id)
        .expect("updated profile is visible in its workspace list");
    assert_eq!(
        listed_profile["value"]["name"],
        "Isolated E2E Profile edited"
    );

    drop(app);
    let reopened = ApiState::hosted_with_control_store(
        ProfileService::disabled(),
        SqliteControlRecordStore::open(&control_path).unwrap(),
    )
    .unwrap()
    .with_concurrent_remote_chunks(Box::new(LocalRemoteChunkStore::new(&chunks_path).unwrap()))
    .with_coordination(Arc::new(E2eCoordination::default()));
    let reopened_app = router(reopened);
    let reopened_profiles = reopened_app
        .clone()
        .oneshot(get_request(
            &format!(
                "/v1/profiles?workspaceId={}&limit=10",
                owner.workspace_id.as_str()
            ),
            token,
            "e2e-profile-list-after-api-reopen",
        ))
        .await
        .unwrap();
    assert_eq!(reopened_profiles.status(), StatusCode::OK);
    let reopened_body = response_json(reopened_profiles).await;
    let reopened_profile = reopened_body["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["value"]["id"] == profile_id)
        .expect("hosted profile is still visible after rebuilding the API state");
    assert_eq!(reopened_profile["value"]["proxyId"], "proxy_e2e_primary");
    assert_eq!(
        reopened_profile["value"]["name"],
        "Isolated E2E Profile edited"
    );

    let reopened_proxies = reopened_app
        .clone()
        .oneshot(get_request(
            &format!("/v1/workspaces/{}/proxies", owner.workspace_id.as_str()),
            token,
            "e2e-proxy-list-after-api-reopen",
        ))
        .await
        .unwrap();
    assert_eq!(reopened_proxies.status(), StatusCode::OK);
    let reopened_proxy_body = response_json(reopened_proxies).await;
    assert_eq!(reopened_proxy_body["items"].as_array().unwrap().len(), 1);
    assert_eq!(
        reopened_proxy_body["items"][0]["proxy"]["name"],
        "E2E edited"
    );
    assert_eq!(reopened_proxy_body["items"][0]["revision"], 2);

    let reopened_manifest = reopened_app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(&manifest_path)
                .header("authorization", format!("Bearer {second_token}"))
                .header("x-buglogin-actor-id", owner.account_id.as_str())
                .header("x-correlation-id", "e2e-manifest-after-api-reopen")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(reopened_manifest.status(), StatusCode::OK);
    let reopened_manifest_body: SyncManifest = serde_json::from_slice(
        &axum::body::to_bytes(reopened_manifest.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(reopened_manifest_body.generation, 3);
    assert_eq!(reopened_manifest_body, generation_three_manifest);

    let downloaded = authenticated_request(
        "GET",
        &format!(
            "/v1/workspaces/{}/resources/{}/blobs?key=Default%2FCookies",
            owner.workspace_id.as_str(),
            resource_id.as_str()
        ),
        second_token,
        "e2e-profile-payload-download-after-api-reopen",
    )
    .header("x-buglogin-actor-id", owner.account_id.as_str())
    .header("x-buglogin-generation", "3")
    .header(
        "x-content-sha256",
        &generation_three_payload_object.checksum,
    )
    .body(Body::empty())
    .unwrap();
    let downloaded = reopened_app.clone().oneshot(downloaded).await.unwrap();
    assert_eq!(downloaded.status(), StatusCode::OK);
    let downloaded_bytes = axum::body::to_bytes(downloaded.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        downloaded_bytes.as_ref(),
        generation_three_payload_bytes.as_slice()
    );
    assert_eq!(
        buglogin_storage::blob::checksum(&downloaded_bytes),
        generation_three_payload_object.checksum
    );

    let downloaded_envelope = authenticated_request(
        "GET",
        &format!(
            "/v1/workspaces/{}/resources/{}/blobs?key={}",
            owner.workspace_id.as_str(),
            resource_id.as_str(),
            ".buglogin%2Fresource-key-envelope.json"
        ),
        token,
        "e2e-profile-envelope-download-after-api-reopen",
    )
    .header("x-buglogin-actor-id", owner.account_id.as_str())
    .header("x-buglogin-generation", "3")
    .header(
        "x-content-sha256",
        &generation_three_envelope_object.checksum,
    )
    .body(Body::empty())
    .unwrap();
    let downloaded_envelope = reopened_app
        .clone()
        .oneshot(downloaded_envelope)
        .await
        .unwrap();
    assert_eq!(downloaded_envelope.status(), StatusCode::OK);
    let downloaded_envelope_bytes =
        axum::body::to_bytes(downloaded_envelope.into_body(), usize::MAX)
            .await
            .unwrap();
    assert_eq!(
        downloaded_envelope_bytes.as_ref(),
        envelope_bytes.as_slice()
    );
    assert_eq!(
        buglogin_storage::blob::checksum(&downloaded_envelope_bytes),
        generation_three_envelope_object.checksum
    );

    let deleted = reopened_app
        .clone()
        .oneshot(json_request(
            "DELETE",
            &format!("/v1/profiles/{profile_id}"),
            json!({
                "revision": 2,
                "confirmation": "QUARANTINE_PROFILE_DATA"
            }),
            Some(token),
            Some("e2e-profile-delete-001"),
            "e2e-profile-delete-correlation",
        ))
        .await
        .unwrap();
    assert_eq!(deleted.status(), StatusCode::OK);
    let delete_body = response_json(deleted).await;
    assert_eq!(delete_body["deleted"], true);

    let after_delete = reopened_app
        .oneshot(get_request(
            &format!(
                "/v1/profiles?workspaceId={}&limit=10",
                owner.workspace_id.as_str()
            ),
            token,
            "e2e-profile-list-after-delete",
        ))
        .await
        .unwrap();
    assert_eq!(after_delete.status(), StatusCode::OK);
    let after_delete_body = response_json(after_delete).await;
    assert!(
        !after_delete_body["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["value"]["id"] == profile_id)
    );
    let _ = fs::remove_dir_all(data_root);
}
