use std::{
    collections::BTreeMap,
    fs,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
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
    errors::Problem,
    ids::{AccountId, GrantId, ProfileId, ResourceId, WorkspaceId},
    permissions::{Action, Capability, Effect, Grant, ResourceScope},
    profile::{ProfileArchiveState, ProfileResource},
    resource::{Resource, ResourceKind, ResourceState},
};
use buglogin_storage::{
    local::LocalRepository,
    remote::{MemoryRemoteChunkStore, RemoteChunkStore},
};
use buglogin_sync::{
    coordination::{
        CoordinationBackend, CoordinationEvent, CoordinationScope, LeaseGrant, Presence,
        PublishedEvent, StreamEvent,
    },
    export::{SyncExport, SyncExportBlob, SyncRevision, canonical_manifest_checksum},
    import::{plan_sync_import, sync_object_storage_key},
    manifest::{ManifestObject, SyncManifest},
};
use serde_json::{Value, json};
use tower::ServiceExt;

#[derive(Default)]
struct FakeCoordination {
    lease: Mutex<Option<LeaseGrant>>,
    replays: Mutex<BTreeMap<String, LeaseGrant>>,
    lease_acquires: AtomicUsize,
    manifest_commits: AtomicUsize,
    fail_validation: AtomicBool,
}

#[async_trait]
impl CoordinationBackend for FakeCoordination {
    async fn acquire_lease(
        &self,
        scope: CoordinationScope<'_>,
        resource_id: &ResourceId,
        ttl: Duration,
    ) -> Result<LeaseGrant, Problem> {
        self.acquire_lease_idempotent(scope, resource_id, ttl, "fallback-lease-key-0001")
            .await
    }

    async fn acquire_lease_idempotent(
        &self,
        scope: CoordinationScope<'_>,
        resource_id: &ResourceId,
        ttl: Duration,
        idempotency_key: &str,
    ) -> Result<LeaseGrant, Problem> {
        if let Some(lease) = self.replays.lock().unwrap().get(idempotency_key).cloned() {
            return Ok(lease);
        }
        if self.lease.lock().unwrap().is_some() {
            return Err(Problem::new("lease_held", "Lease held", 409, "test"));
        }
        self.lease_acquires.fetch_add(1, Ordering::SeqCst);
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
        Ok(lease)
    }

    async fn renew_lease(
        &self,
        _scope: CoordinationScope<'_>,
        resource_id: &ResourceId,
        owner_token: &str,
        ttl: Duration,
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
        resource_id: &ResourceId,
        owner_token: &str,
        fencing_token: u64,
    ) -> Result<(), Problem> {
        if self.fail_validation.swap(false, Ordering::SeqCst) {
            return Err(Problem::new(
                "test_lease_lost",
                "Test lease was lost before activation",
                503,
                "test",
            )
            .retryable(true));
        }
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
async fn sync_import_resumes_verified_blob_and_activates_authoritative_manifest() {
    let root = std::env::temp_dir().join(format!(
        "buglogin-http-sync-migration-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    let state = ApiState::new(ProfileService::new(LocalRepository::open(&root).unwrap()));
    let token = state.issue_token("acct_sync_migration").unwrap();
    let account = AccountId::new("acct_sync_migration").unwrap();
    let workspace = WorkspaceId::new("ws_sync_migration").unwrap();
    let resource = ResourceId::new("res_sync_migration").unwrap();
    let profile = ProfileId::new("prf_sync_migration").unwrap();
    let mut grants = state.grants.write().unwrap();
    grants.extend([
        grant(
            &account,
            "grant_sync_migration",
            "migration.write",
            Action::Execute,
            ResourceScope::resource(workspace.clone(), resource.clone()),
        ),
        grant(
            &account,
            "grant_sync_resource_create",
            "CAP-RES-001",
            Action::Create,
            ResourceScope::workspace(workspace.clone()),
        ),
        grant(
            &account,
            "grant_sync_read",
            "profile.read",
            Action::Read,
            ResourceScope::resource(workspace.clone(), resource.clone()),
        ),
    ]);
    let grant_snapshot = grants.clone();
    drop(grants);
    let principal = state.authenticate_token(&token, unix_seconds()).unwrap();
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
                display_name: "Migrated profile".into(),
            },
        )
        .unwrap();
    let coordination = Arc::new(FakeCoordination::default());
    let remote = MemoryRemoteChunkStore::default();
    let export = genesis_export(workspace.clone(), resource.clone());
    let plan = plan_sync_import(&export, None, 41).unwrap();
    let staged_blob = &plan.blobs[0];
    let staged_key = sync_object_storage_key(&plan.manifest, staged_blob);
    let mut interrupted_store = remote.clone();
    RemoteChunkStore::put(
        &mut interrupted_store,
        &staged_key,
        &staged_blob.data,
        &staged_blob.checksum,
    )
    .unwrap();
    let app = router(
        state
            .with_profile_resource(ProfileResource {
                profile_id: profile,
                resource_id: resource.clone(),
                workspace_id: workspace.clone(),
                archive_state: ProfileArchiveState::Active,
            })
            .with_remote_chunks(Box::new(remote.clone()))
            .with_coordination(coordination.clone()),
    );
    assert_eq!(export.manifest.fencing_token, 8);
    coordination.fail_validation.store(true, Ordering::SeqCst);
    let rejected = app
        .clone()
        .oneshot(request(
            &token,
            "sync-migration-import-0001",
            json!({"export": export.clone(), "dryRun": false}),
        ))
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        RemoteChunkStore::checksum(&remote, &staged_key).unwrap(),
        Some(staged_blob.checksum.clone())
    );

    let first = app
        .clone()
        .oneshot(request(
            &token,
            "sync-migration-import-0001",
            json!({"export": export.clone(), "dryRun": false}),
        ))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let first_body = body(first).await;
    assert_eq!(first_body["state"], "active");
    assert_eq!(first_body["createdBlobs"], 0);
    assert_eq!(first_body["resumedBlobs"], 1);
    assert_eq!(
        RemoteChunkStore::checksum(&remote, &staged_key).unwrap(),
        Some(staged_blob.checksum.clone())
    );

    let replay = app
        .clone()
        .oneshot(request(
            &token,
            "sync-migration-import-0001",
            json!({"export": genesis_export(workspace.clone(), resource.clone()), "dryRun": false}),
        ))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::OK);
    let replay_body = body(replay).await;
    assert_eq!(replay_body["state"], "active");
    assert_eq!(replay_body["generation"], 1);
    assert_eq!(coordination.lease_acquires.load(Ordering::SeqCst), 2);

    let mut conflicting_export = genesis_export(workspace.clone(), resource.clone());
    conflicting_export.export_id = "hosted-sync-export-conflict".into();
    let conflict = app
        .clone()
        .oneshot(request(
            &token,
            "sync-migration-import-0001",
            json!({"export": conflicting_export, "dryRun": false}),
        ))
        .await
        .unwrap();
    assert_eq!(conflict.status(), StatusCode::CONFLICT);
    assert_eq!(body(conflict).await["code"], "idempotency_conflict");

    let wrong_base = app
        .clone()
        .oneshot(request(
            &token,
            "sync-migration-wrong-base-0001",
            json!({
                "export": successor_with_wrong_base(workspace.clone(), resource.clone()),
                "dryRun": false
            }),
        ))
        .await
        .unwrap();
    assert_eq!(wrong_base.status(), StatusCode::CONFLICT);
    assert_eq!(body(wrong_base).await["code"], "sync_import_base_conflict");
    assert_eq!(coordination.lease_acquires.load(Ordering::SeqCst), 3);

    let manifest = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/v1/workspaces/{}/resources/{}/manifest",
                    workspace.as_str(),
                    resource.as_str()
                ))
                .header("authorization", format!("Bearer {token}"))
                .header("x-correlation-id", "sync-migration-manifest-read")
                .header("x-buglogin-actor-id", account.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(manifest.status(), StatusCode::OK);
    let manifest = body(manifest).await;
    assert_eq!(manifest["generation"], 1);
    assert_eq!(manifest["fencingToken"], 41);
    assert_ne!(manifest["fencingToken"], 8);
    assert_eq!(coordination.manifest_commits.load(Ordering::SeqCst), 0);
    let _ = fs::remove_dir_all(root);
}

fn genesis_export(workspace: WorkspaceId, resource: ResourceId) -> SyncExport {
    let manifest = SyncManifest::new(
        workspace,
        resource,
        8,
        1,
        None,
        b"migration-root",
        vec![ManifestObject::new("profile/state", b"encrypted-profile").unwrap()],
    )
    .unwrap();
    SyncExport::new(
        "hosted-sync-export-0001",
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
            checksum: buglogin_storage::blob::checksum(b"encrypted-profile"),
            data: b"encrypted-profile".to_vec(),
        }],
        1_753_142_400_000,
        "0.1.0",
    )
    .unwrap()
}

fn successor_with_wrong_base(workspace: WorkspaceId, resource: ResourceId) -> SyncExport {
    let manifest = SyncManifest::new(
        workspace,
        resource,
        8,
        2,
        Some(1),
        b"migration-root-successor",
        vec![ManifestObject::new("profile/state", b"encrypted-successor").unwrap()],
    )
    .unwrap();
    SyncExport::new(
        "hosted-sync-export-wrong-base",
        "legacy-production",
        manifest.clone(),
        Some(9),
        vec![SyncRevision {
            generation: 2,
            parent_generation: Some(9),
            manifest_sha256: canonical_manifest_checksum(&manifest).unwrap(),
        }],
        vec![SyncExportBlob {
            key: "profile/state".into(),
            size: 19,
            checksum: buglogin_storage::blob::checksum(b"encrypted-successor"),
            data: b"encrypted-successor".to_vec(),
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

fn request(token: &str, idempotency: &str, value: Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/v1/migrations/sync/import")
        .header("authorization", format!("Bearer {token}"))
        .header("x-correlation-id", "sync-migration-import")
        .header("idempotency-key", idempotency)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&value).unwrap()))
        .unwrap()
}

async fn body(response: axum::response::Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn unsupported() -> Problem {
    Problem::new("unsupported", "Unsupported", 501, "test")
}

fn lease_missing() -> Problem {
    Problem::new("lease_not_found", "Lease missing", 404, "test")
}
