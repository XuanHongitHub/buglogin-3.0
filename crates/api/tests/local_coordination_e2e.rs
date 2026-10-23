use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use buglogin_api::http::{ApiState, router};
use buglogin_browser::profile::ProfileService;
use buglogin_domain::{
    entities::{Profile, ProfileState, Runtime, SyncMode},
    ids::{AccountId, GrantId, ProfileId, ResourceId, WorkspaceId},
    permissions::{Action, Capability, Effect, Grant, ResourceScope},
    profile::{ProfileArchiveState, ProfileResource},
    resource::{Resource, ResourceKind, ResourceState},
};
use buglogin_storage::{
    control::SqliteControlRecordStore, local::LocalRepository, remote::LocalRemoteChunkStore,
};
use buglogin_sync::{coordination::LocalCoordination, manifest::SyncManifest};
use serde_json::{Value, json};
use tower::ServiceExt;

struct TestRoot(PathBuf);

impl TestRoot {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock must be after the Unix epoch")
            .as_nanos();
        Self(std::env::temp_dir().join(format!(
            "buglogin-local-coordination-e2e-{}-{nonce}",
            std::process::id()
        )))
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn grant(
    account_id: &AccountId,
    id: &str,
    capability: &str,
    action: Action,
    scope: ResourceScope,
) -> Grant {
    Grant {
        id: GrantId::new(id).unwrap(),
        account_id: Some(account_id.clone()),
        member_id: None,
        capability: Capability::new(capability).unwrap(),
        action,
        scope,
        effect: Effect::Allow,
    }
}

fn request(
    method: &str,
    path: &str,
    token: &str,
    idempotency_key: Option<&str>,
    body: Option<String>,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", format!("Bearer {token}"))
        .header("x-correlation-id", "local-coordination-e2e")
        .header("content-type", "application/json");
    if let Some(key) = idempotency_key {
        builder = builder.header("idempotency-key", key);
    }
    builder
        .body(body.map_or_else(Body::empty, Body::from))
        .unwrap()
}

async fn response_json(response: axum::response::Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
}

#[tokio::test]
async fn local_sqlite_api_uses_local_coordination_for_lease_and_manifest_routes() {
    let root = TestRoot::new();
    let repository = LocalRepository::open(root.0.join("repository")).unwrap();
    let control_store = SqliteControlRecordStore::open(root.0.join("control.sqlite")).unwrap();
    let chunks = LocalRemoteChunkStore::new(root.0.join("chunks")).unwrap();
    let state = ApiState::local_with_control_store(ProfileService::new(repository), control_store)
        .unwrap()
        .with_concurrent_remote_chunks(Box::new(chunks))
        .with_coordination(std::sync::Arc::new(LocalCoordination::new()));

    let account_id = AccountId::new("acct_local_sync_e2e").unwrap();
    let workspace_id = WorkspaceId::new("ws_local_sync_e2e").unwrap();
    let resource_id = ResourceId::new("res_local_sync_e2e").unwrap();
    let profile_id = ProfileId::new("prf_local_sync_e2e").unwrap();
    let token = state.issue_token(account_id.as_str()).unwrap();
    let principal = state
        .authenticate_token(&token, current_time_seconds())
        .unwrap();

    let grants = vec![
        grant(
            &account_id,
            "grant_local_resource_create",
            "CAP-RES-001",
            Action::Create,
            ResourceScope::workspace(workspace_id.clone()),
        ),
        grant(
            &account_id,
            "grant_local_profile_launch",
            "profile.launch",
            Action::Execute,
            ResourceScope::resource(workspace_id.clone(), resource_id.clone()),
        ),
        grant(
            &account_id,
            "grant_local_profile_stop",
            "profile.stop",
            Action::Execute,
            ResourceScope::resource(workspace_id.clone(), resource_id.clone()),
        ),
        grant(
            &account_id,
            "grant_local_profile_read",
            "profile.read",
            Action::Read,
            ResourceScope::resource(workspace_id.clone(), resource_id.clone()),
        ),
        grant(
            &account_id,
            "grant_local_profile_update",
            "profile.update",
            Action::Update,
            ResourceScope::resource(workspace_id.clone(), resource_id.clone()),
        ),
    ];
    state.grants.write().unwrap().extend(grants.clone());
    state
        .resources
        .write()
        .unwrap()
        .register(
            &principal,
            &grants,
            Resource {
                id: resource_id.clone(),
                workspace_id: workspace_id.clone(),
                kind: ResourceKind::Profile,
                state: ResourceState::Active,
                display_name: "Local E2E profile".into(),
            },
        )
        .unwrap();
    state
        .profiles
        .create(Profile {
            id: profile_id.clone(),
            name: "Local E2E profile".into(),
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
            created_at: "2026-10-19T00:00:00Z".into(),
            updated_at: "2026-10-19T00:00:00Z".into(),
        })
        .unwrap();
    let state = state.with_profile_resource(ProfileResource {
        profile_id,
        resource_id: resource_id.clone(),
        workspace_id: workspace_id.clone(),
        archive_state: ProfileArchiveState::Active,
    });
    let app = router(state);

    let ready = app
        .clone()
        .oneshot(request("GET", "/v1/readyz", &token, None, None))
        .await
        .unwrap();
    assert_eq!(ready.status(), StatusCode::OK);
    assert_eq!(
        ready
            .headers()
            .get("x-buglogin-persistence-mode")
            .and_then(|value| value.to_str().ok()),
        Some("sqlite")
    );

    let lease_path = format!(
        "/v1/workspaces/{}/resources/{}/lease",
        workspace_id.as_str(),
        resource_id.as_str()
    );
    let lease_response = app
        .clone()
        .oneshot(request(
            "POST",
            &lease_path,
            &token,
            Some("local-lease-acquire-e2e-0001"),
            Some(json!({"ownerId": account_id, "ttlSeconds": 30}).to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(lease_response.status(), StatusCode::OK);
    let lease = response_json(lease_response).await;
    let owner_token = lease["ownerToken"].as_str().unwrap();
    let fencing_token = lease["fencingToken"].as_u64().unwrap();

    let manifest = SyncManifest::new(
        workspace_id.clone(),
        resource_id.clone(),
        fencing_token,
        1,
        None,
        b"local-e2e-empty-profile",
        Vec::new(),
    )
    .unwrap();
    let manifest_path = format!(
        "/v1/workspaces/{}/resources/{}/manifest",
        workspace_id.as_str(),
        resource_id.as_str()
    );
    let mut commit = request(
        "PUT",
        &manifest_path,
        &token,
        Some("local-manifest-commit-e2e-0001"),
        Some(serde_json::to_string(&manifest).unwrap()),
    );
    commit
        .headers_mut()
        .insert("x-buglogin-owner-id", account_id.as_str().parse().unwrap());
    commit
        .headers_mut()
        .insert("x-buglogin-owner-token", owner_token.parse().unwrap());
    commit.headers_mut().insert(
        "x-buglogin-fencing-token",
        fencing_token.to_string().parse().unwrap(),
    );
    let committed = app.clone().oneshot(commit).await.unwrap();
    assert_eq!(committed.status(), StatusCode::OK);

    let mut read_manifest = request("GET", &manifest_path, &token, None, None);
    read_manifest
        .headers_mut()
        .insert("x-buglogin-actor-id", account_id.as_str().parse().unwrap());
    let read = app.oneshot(read_manifest).await.unwrap();
    assert_eq!(read.status(), StatusCode::OK);
    let persisted: Value = response_json(read).await;
    assert_eq!(persisted["generation"], 1);
    assert_eq!(persisted["fencingToken"], fencing_token);
    assert_eq!(persisted["resourceId"], resource_id.as_str());
}

fn current_time_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock must be after the Unix epoch")
        .as_secs()
}
