use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use buglogin_api::{
    http::{ApiState, router},
    mcp::{McpDispatcher, McpRequest, ProductionMcpExecutor, ProductionMcpPolicy},
};
use buglogin_browser::profile::ProfileService;
use buglogin_domain::{
    account::Account,
    errors::Problem,
    ids::{AccountId, GrantId, ProfileId, ResourceId, WorkspaceId},
    permissions::{Action, Capability, Effect, Grant, ResourceScope},
};
use buglogin_migration::{
    MigrationKey,
    inspect::inspect_source,
    legacy::map_legacy_control_plane_json,
    package::{
        NeutralPackage, NeutralPackageManifest, PackageMetadata, build_encrypted_profile_package,
    },
    signature::{SignatureMetadata, sign_package},
};
use buglogin_storage::{
    blob::checksum,
    control::{ControlRecordStore, MemoryControlRecordStore, PutControlRecord},
    local::LocalRepository,
    secrets::SecretValue,
};
use buglogin_sync::{
    coordination::{
        CoordinationBackend, CoordinationEvent, CoordinationScope, LeaseGrant, Presence,
        PublishedEvent, StreamEvent,
    },
    export::{SyncExport, SyncExportBlob, SyncRevision, canonical_manifest_checksum},
    manifest::{ManifestObject, SyncManifest},
};
use serde_json::{Value, json};
use tokio::sync::Notify;
use tower::ServiceExt;

#[derive(Default)]
struct FakeCoordination {
    fail_acquire: AtomicBool,
    pause_first_acquire: AtomicBool,
    acquire_calls: AtomicUsize,
    first_acquire_paused: AtomicBool,
    first_acquire_waiting: Notify,
    resume_first_acquire: Notify,
    second_acquire_wins: AtomicBool,
    second_acquire_waiting: Notify,
    resume_second_acquire: Notify,
    lease: Mutex<Option<LeaseGrant>>,
    replays: Mutex<BTreeMap<String, LeaseGrant>>,
    manifest_commits: AtomicUsize,
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
        if self.fail_acquire.load(Ordering::SeqCst) {
            return Err(Problem::new(
                "test_coordination_unavailable",
                "Test coordination unavailable",
                503,
                "test",
            ));
        }
        let acquire_index = if self.pause_first_acquire.load(Ordering::SeqCst) {
            self.acquire_calls.fetch_add(1, Ordering::SeqCst)
        } else {
            usize::MAX
        };
        if acquire_index == 0 {
            self.first_acquire_paused.store(true, Ordering::SeqCst);
            self.first_acquire_waiting.notify_one();
            self.resume_first_acquire.notified().await;
            self.first_acquire_paused.store(false, Ordering::SeqCst);
        } else if self.first_acquire_paused.load(Ordering::SeqCst)
            && !(acquire_index == 1 && self.second_acquire_wins.load(Ordering::SeqCst))
        {
            return Err(Problem::new("lease_held", "Lease held", 409, "test"));
        }
        if let Some(lease) = self.replays.lock().unwrap().get(idempotency_key).cloned() {
            return Ok(lease);
        }
        if self.lease.lock().unwrap().is_some() {
            return Err(Problem::new("lease_held", "Lease held", 409, "test"));
        }
        let lease = LeaseGrant {
            resource_id: resource_id.clone(),
            workspace_id: scope.workspace_id.clone(),
            owner_id: scope.auth.actor_id().into(),
            owner_token: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
            fencing_token: 41,
            issued_at_ms: 1,
            expires_at_ms: 1 + ttl.as_millis() as u64,
        };
        *self.lease.lock().unwrap() = Some(lease.clone());
        self.replays
            .lock()
            .unwrap()
            .insert(idempotency_key.into(), lease.clone());
        if acquire_index == 1 && self.second_acquire_wins.load(Ordering::SeqCst) {
            self.second_acquire_waiting.notify_one();
            self.resume_second_acquire.notified().await;
        }
        Ok(lease)
    }

    async fn renew_lease_idempotent(
        &self,
        _scope: CoordinationScope<'_>,
        resource_id: &ResourceId,
        owner_token: &str,
        ttl: Duration,
        _idempotency_key: &str,
    ) -> Result<LeaseGrant, Problem> {
        let mut current = self.lease.lock().unwrap();
        let lease = current.as_mut().ok_or_else(lease_missing)?;
        if lease.resource_id != *resource_id || lease.owner_token != owner_token {
            return Err(Problem::new(
                "lease_not_owner",
                "Lease owner mismatch",
                409,
                "test",
            ));
        }
        lease.expires_at_ms = lease.expires_at_ms.saturating_add(ttl.as_millis() as u64);
        Ok(lease.clone())
    }

    async fn release_lease_idempotent(
        &self,
        _scope: CoordinationScope<'_>,
        _resource_id: &ResourceId,
        owner_token: &str,
        _idempotency_key: &str,
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

    async fn lease_status(
        &self,
        _scope: CoordinationScope<'_>,
        _resource_id: &ResourceId,
    ) -> Result<Option<buglogin_sync::coordination::LeaseStatus>, Problem> {
        Ok(None)
    }

    async fn validate_write_authority(
        &self,
        _scope: CoordinationScope<'_>,
        resource_id: &ResourceId,
        owner_token: &str,
        fencing_token: u64,
    ) -> Result<(), Problem> {
        let lease = self.lease.lock().unwrap();
        let current = lease.as_ref().ok_or_else(lease_missing)?;
        if current.resource_id != *resource_id
            || current.owner_token != owner_token
            || current.fencing_token != fencing_token
        {
            return Err(Problem::new(
                "lease_not_owner",
                "Lease owner mismatch",
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
        Ok(None)
    }

    async fn commit_manifest(
        &self,
        _scope: CoordinationScope<'_>,
        _resource_id: &ResourceId,
        _owner_token: &str,
        _manifest: SyncManifest,
    ) -> Result<(), Problem> {
        self.manifest_commits.fetch_add(1, Ordering::SeqCst);
        Err(Problem::new(
            "redis_manifest_forbidden",
            "Redis manifest commit forbidden",
            500,
            "test",
        ))
    }
}

#[tokio::test]
async fn signed_sync_package_activation_compensates_and_replays() {
    let root = std::env::temp_dir().join(format!(
        "buglogin-http-signed-sync-package-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();

    let signing_key = [77_u8; 32];
    let account = AccountId::new("acct_signed_sync_package").unwrap();
    let (package_id, package, profile, workspace, resource) =
        signed_package(&root.join("legacy-source"), &signing_key);
    let mut control_store = MemoryControlRecordStore::default();
    let account_value =
        serde_json::to_value(Account::new(account.clone(), "2026-07-22T00:00:00Z")).unwrap();
    control_store
        .put(PutControlRecord {
            tenant_id: "control",
            record_type: "account",
            record_id: account.as_str(),
            expected_revision: Some(0),
            idempotency_key: "seed-signed-sync-account",
            value: &account_value,
        })
        .unwrap();
    let mut durable_store = control_store.reopen();
    let state = ApiState::hosted_with_control_store(
        ProfileService::new(LocalRepository::open(root.join("repository")).unwrap()),
        control_store,
    )
    .unwrap()
    .with_migration_signing_key(SecretValue::new(signing_key.to_vec()).unwrap());
    let token = state.issue_token(account.as_str()).unwrap();
    let mut grants = state.grants.write().unwrap();
    grants.extend([
        grant(
            &account,
            "grant_signed_sync_migration",
            "migration.write",
            Action::Execute,
            ResourceScope::global(),
        ),
        grant(
            &account,
            "grant_signed_sync_resource_create",
            "CAP-RES-001",
            Action::Create,
            ResourceScope::workspace(workspace.clone()),
        ),
        grant(
            &account,
            "grant_signed_sync_read",
            "profile.read",
            Action::Read,
            ResourceScope::resource(workspace.clone(), resource.clone()),
        ),
    ]);
    drop(grants);
    let coordination = Arc::new(FakeCoordination::default());
    let hosted_state = state.clone();
    let activation_state = state.with_coordination(coordination.clone());
    let app = router(activation_state.clone());

    for (index, (path, bytes)) in package.files().iter().enumerate() {
        let response = app
            .clone()
            .oneshot(upload_request(
                &package_id,
                path,
                &token,
                &format!("signed-sync-file-{index:04}"),
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

    let verified = app
        .clone()
        .oneshot(verify_request(
            &package_id,
            &token,
            "signed-sync-verify-0001",
        ))
        .await
        .unwrap();
    assert_eq!(verified.status(), StatusCode::OK);
    let verified_body = body(verified).await;
    assert_eq!(verified_body["state"], "verified");
    assert_eq!(verified_body["hasControl"], true);
    assert_eq!(verified_body["hasSync"], true);

    coordination.fail_acquire.store(true, Ordering::SeqCst);
    let failed = app
        .clone()
        .oneshot(activate_request(
            &package_id,
            &token,
            "signed-sync-activate-0001",
        ))
        .await
        .unwrap();
    let failed_status = failed.status();
    let failed_body = body(failed).await;
    assert_eq!(
        failed_status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{failed_body}"
    );
    assert_eq!(failed_body["code"], "test_coordination_unavailable");

    // Control records are committed before sync activation. If sync cannot acquire
    // its lease, package activation compensates by rolling the control graph back.
    let profiles_after_failure = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/profiles?limit=50")
                .header("authorization", format!("Bearer {token}"))
                .header("x-correlation-id", "signed-sync-after-failed-activate")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(profiles_after_failure.status(), StatusCode::OK);
    assert!(
        body(profiles_after_failure).await["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    coordination.fail_acquire.store(false, Ordering::SeqCst);
    coordination
        .pause_first_acquire
        .store(true, Ordering::SeqCst);
    coordination
        .second_acquire_wins
        .store(true, Ordering::SeqCst);
    let first_app = app.clone();
    let first_package_id = package_id.clone();
    let first_token = token.clone();
    let first_activation = tokio::spawn(async move {
        first_app
            .oneshot(activate_request(
                &first_package_id,
                &first_token,
                "signed-sync-activate-0001",
            ))
            .await
            .unwrap()
    });
    tokio::time::timeout(
        Duration::from_secs(5),
        coordination.first_acquire_waiting.notified(),
    )
    .await
    .expect("first activation should reach sync lease acquisition");

    let second_app = app.clone();
    let second_package_id = package_id.clone();
    let second_token = token.clone();
    let second_activation = tokio::spawn(async move {
        second_app
            .oneshot(activate_request(
                &second_package_id,
                &second_token,
                "signed-sync-activate-concurrent-0001",
            ))
            .await
            .unwrap()
    });
    let concurrent = second_activation.await.unwrap();
    let concurrent_status = concurrent.status();
    let concurrent_body = body(concurrent).await;
    assert_eq!(concurrent_status, StatusCode::CONFLICT, "{concurrent_body}");
    assert_eq!(
        concurrent_body["code"],
        "migration_control_import_in_progress"
    );

    coordination.resume_first_acquire.notify_one();
    let first = first_activation.await.unwrap();
    let first_status = first.status();
    let activated_body = body(first).await;
    assert_eq!(first_status, StatusCode::OK, "{activated_body}");
    assert_eq!(activated_body["state"], "active");

    let profiles_after_activation = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/profiles?limit=50")
                .header("authorization", format!("Bearer {token}"))
                .header("x-correlation-id", "signed-sync-during-activate-retry")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(profiles_after_activation.status(), StatusCode::OK);
    assert_eq!(
        body(profiles_after_activation).await["items"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "the serialized activation keeps its control graph available to sync commit"
    );

    let mut dispatcher = McpDispatcher::new(
        ProductionMcpPolicy::new(activation_state.clone()),
        ProductionMcpExecutor::new(activation_state),
        true,
    );
    let mcp_activation = dispatcher
        .dispatch_async(McpRequest {
            tool_name: "migration_package_activate".into(),
            correlation_id: "cor_signed_sync_mcp_activate".into(),
            bearer_token: Some(token.clone()),
            idempotency_key: Some("signed-sync-mcp-activate-0001".into()),
            input: json!({"packageId": package_id.clone()}),
        })
        .await
        .unwrap();
    let mcp_replay = dispatcher
        .dispatch_async(McpRequest {
            tool_name: "migration_package_activate".into(),
            correlation_id: "cor_signed_sync_mcp_activate_replay".into(),
            bearer_token: Some(token.clone()),
            idempotency_key: Some("signed-sync-mcp-activate-0001".into()),
            input: json!({"packageId": package_id.clone()}),
        })
        .await
        .unwrap();
    assert_eq!(mcp_replay, mcp_activation);
    assert_eq!(mcp_activation["state"], "active");
    assert_eq!(mcp_activation["replayed"], true);
    assert_eq!(activated_body["replayed"], false);
    assert!(activated_body["controlImport"].is_object());
    assert_eq!(activated_body["controlImport"]["state"], "active");
    assert_eq!(activated_body["controlImport"]["replayed"], false);
    assert!(activated_body["syncImport"].is_object());
    assert_eq!(activated_body["syncImport"]["state"], "active");
    assert_eq!(activated_body["syncImport"]["generation"], 1);
    assert!(hosted_state.profiles.list().unwrap().is_empty());
    let durable_control = durable_store
        .get("control", "control_state", "primary")
        .unwrap()
        .unwrap();
    assert!(durable_control.value["profiles"][profile.as_str()].is_object());
    assert_eq!(
        durable_control.value["profiles"][profile.as_str()]["value"]["syncMode"],
        "managed"
    );

    let replay = app
        .clone()
        .oneshot(activate_request(
            &package_id,
            &token,
            "signed-sync-activate-replay",
        ))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::OK);
    let replay_body = body(replay).await;
    assert_eq!(replay_body["replayed"], true);
    let mut expected_replay = activated_body;
    expected_replay["replayed"] = Value::Bool(true);
    assert_eq!(replay_body, expected_replay);
    assert_eq!(coordination.manifest_commits.load(Ordering::SeqCst), 0);

    let manifest = app
        .oneshot(manifest_request(&token, &workspace, &resource))
        .await
        .unwrap();
    assert_eq!(manifest.status(), StatusCode::OK);
    let manifest_body = body(manifest).await;
    assert_eq!(manifest_body["generation"], 1);
    assert_eq!(manifest_body["fencingToken"], 41);
    let _ = fs::remove_dir_all(root);
}

fn signed_package(
    source: &Path,
    key: &[u8],
) -> (String, NeutralPackage, ProfileId, WorkspaceId, ResourceId) {
    fs::create_dir_all(source).unwrap();
    fs::write(
        source.join("version.json"),
        br#"{"system":"buglogin-legacy","version":"1.1.84","instanceId":"signed-sync-package","os":"windows","architecture":"x86_64"}"#,
    )
    .unwrap();
    let profile_root = source.join("profiles/01234567-89ab-cdef-0123-456789abcdef");
    fs::create_dir_all(profile_root.join("profile")).unwrap();
    fs::write(
        profile_root.join("metadata.json"),
        br#"{"id":"01234567-89ab-cdef-0123-456789abcdef","name":"Signed sync profile","runtime":"wayfern"}"#,
    )
    .unwrap();
    fs::write(profile_root.join("profile/Preferences"), b"profile-data").unwrap();
    let control_export = control_plane_export();
    fs::write(
        source.join("control-plane-export.json"),
        serde_json::to_vec(&control_export).unwrap(),
    )
    .unwrap();
    let (initial_report, _) = inspect_source(source).unwrap();
    let mapping = map_legacy_control_plane_json(
        &serde_json::to_vec(&control_export).unwrap(),
        &initial_report.source.instance_id,
    )
    .unwrap();
    let binding = &mapping.profile_resources[0].data;
    let profile = ProfileId::new(binding.profile_id.clone()).unwrap();
    let workspace = WorkspaceId::new(binding.workspace_id.clone()).unwrap();
    let resource = ResourceId::new(binding.resource_id.clone()).unwrap();
    fs::write(
        source.join("sync-export.json"),
        serde_json::to_vec(&sync_export(workspace.clone(), resource.clone())).unwrap(),
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
    let binding_record: Value = serde_json::from_slice(
        package
            .file("entities/profile-resources.jsonl")
            .unwrap()
            .split(|byte| *byte == b'\n')
            .find(|line| !line.is_empty())
            .unwrap(),
    )
    .unwrap();
    assert_eq!(binding_record["data"]["workspace_id"], workspace.as_str());
    assert_eq!(binding_record["data"]["resource_id"], resource.as_str());
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
    (manifest.package_id, package, profile, workspace, resource)
}

fn control_plane_export() -> Value {
    json!({
        "organizations": [{
            "id": "organization-signed-sync",
            "name": "Signed Sync Migration",
            "slug": "signed-sync-migration",
            "status": "active",
            "createdBy": "owner-signed-sync",
            "createdAt": "2025-01-01T00:00:00Z",
            "updatedAt": "2025-01-01T00:00:00Z"
        }],
        "workspaces": [{
            "id": "workspace-signed-sync",
            "organizationId": "organization-signed-sync",
            "name": "Signed Sync Workspace",
            "mode": "team",
            "createdAt": "2025-01-01T00:00:00Z",
            "createdBy": "owner-signed-sync",
            "profileLimit": 10,
            "memberLimit": 10,
            "maxStorageBytes": 1048576,
            "entitlementState": "active",
            "localEntitlementState": "active"
        }],
        "memberships": [{
            "workspaceId": "workspace-signed-sync",
            "userId": "owner-signed-sync",
            "email": "owner-signed-sync@example.test",
            "role": "owner",
            "resourceAccessMode": "full",
            "customCapabilities": null,
            "accountState": "active",
            "platformRole": null,
            "accessSource": "direct",
            "createdAt": "2025-01-01T00:00:00Z"
        }],
        "profiles": [{
            "id": "prf_0123456789abcdef0123456789abcdef",
            "workspaceId": "workspace-signed-sync", "name": "Signed sync profile",
            "runtime": "wayfern", "runtimeVersion": "146.0.7680.177.4",
            "releaseType": "stable", "proxyId": null, "vpnId": null,
            "groupId": null, "extensionGroupId": null, "ephemeral": false,
            "createdAt": "2025-01-01T00:00:00Z", "updatedAt": "2025-01-01T00:00:00Z",
            "tags": [], "note": null
        }],
        "shareGrants": [{
            "id": "share-signed-sync",
            "workspaceId": "workspace-signed-sync",
            "resourceType": "profile",
            "resourceId": "prf_0123456789abcdef0123456789abcdef",
            "resourceName": "Signed sync profile",
            "recipientEmail": "owner-signed-sync@example.test",
            "accessMode": "run_sync_limited",
            "createdAt": "2025-01-01T00:00:00Z",
            "createdBy": "owner-signed-sync",
            "revokedAt": null
        }]
    })
}

fn sync_export(workspace: WorkspaceId, resource: ResourceId) -> SyncExport {
    let manifest = SyncManifest::new(
        workspace,
        resource,
        8,
        1,
        None,
        b"signed-sync-root",
        vec![ManifestObject::new("profile/state", b"encrypted-profile").unwrap()],
    )
    .unwrap();
    SyncExport::new(
        "signed-sync-export-0001",
        "legacy-production",
        manifest.clone(),
        None,
        vec![SyncRevision {
            generation: 1,
            parent_generation: None,
            manifest_sha256: canonical_manifest_checksum(&manifest).unwrap(),
        }],
        vec![SyncExportBlob {
            key: "profile/state".into(),
            size: 17,
            checksum: checksum(b"encrypted-profile"),
            data: b"encrypted-profile".to_vec(),
        }],
        1_753_142_400_000,
        "0.1.0",
    )
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
        .header("x-correlation-id", "signed-sync-package-test")
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
        .header("x-correlation-id", "signed-sync-package-verify")
        .header("idempotency-key", key)
        .body(Body::empty())
        .unwrap()
}

fn activate_request(package_id: &str, token: &str, key: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(format!("/v1/migrations/packages/{package_id}/activate"))
        .header("authorization", format!("Bearer {token}"))
        .header("x-correlation-id", "signed-sync-package-activate")
        .header("idempotency-key", key)
        .body(Body::empty())
        .unwrap()
}

fn manifest_request(token: &str, workspace: &WorkspaceId, resource: &ResourceId) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(format!(
            "/v1/workspaces/{}/resources/{}/manifest",
            workspace.as_str(),
            resource.as_str()
        ))
        .header("authorization", format!("Bearer {token}"))
        .header("x-correlation-id", "signed-sync-package-manifest")
        .header("x-buglogin-actor-id", "acct_signed_sync_package")
        .body(Body::empty())
        .unwrap()
}

async fn body(response: axum::response::Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
}

fn unsupported() -> Problem {
    Problem::new("unsupported", "Unsupported", 501, "test")
}

fn lease_missing() -> Problem {
    Problem::new("lease_not_found", "Lease missing", 404, "test")
}
