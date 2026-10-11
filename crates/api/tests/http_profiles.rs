use std::{fs, path::PathBuf};

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use buglogin_api::http::{ApiState, router};
use buglogin_browser::profile::ProfileService;
use buglogin_domain::{
    account::Account,
    entities::{Profile, ProfileState, Runtime, SyncMode},
    ids::{AccountId, GrantId, ProfileId, ProxyId, WorkspaceId},
    permissions::{Action, Capability, Effect, Grant, ResourceScope},
    profile::ProfileResource,
};
use buglogin_storage::{
    control::{ControlRecordStore, MemoryControlRecordStore, PutControlRecord},
    local::LocalRepository,
};
use serde_json::json;
use tower::ServiceExt;

fn root(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!("buglogin-http-{label}-{}", std::process::id()))
}

fn profile() -> Profile {
    Profile {
        id: ProfileId::from_str("prf_http_1").unwrap(),
        name: "HTTP profile".into(),
        runtime: Runtime::Wayfern,
        runtime_version: Some("1.0.0".into()),
        runtime_config: None,
        state: ProfileState::Created,
        archived: false,
        fingerprint: None,
        note: None,
        proxy_id: None,
        vpn_id: None,
        group_ids: vec![],
        tags: vec!["http".into()],
        extension_group_id: None,
        sync_mode: Some(SyncMode::Local),
        created_at: "2026-07-20T00:00:00Z".into(),
        updated_at: "2026-07-20T00:00:00Z".into(),
    }
}

fn request(method: &str, uri: &str, token: &str, body: Option<String>) -> Request<Body> {
    let builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .header("x-correlation-id", "http-test-1");
    match body {
        Some(body) => builder
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    }
}

fn lifecycle_request(
    method: &str,
    uri: &str,
    token: &str,
    key: &str,
    body: Option<String>,
) -> Request<Body> {
    let mut request = request(method, uri, token, body);
    request
        .headers_mut()
        .insert("idempotency-key", key.parse().unwrap());
    request
}

#[tokio::test]
async fn profile_http_flow_enforces_auth_scope_and_revision() {
    let root = root("profiles");
    let _ = fs::remove_dir_all(&root);
    let state = ApiState::new(ProfileService::new(LocalRepository::open(&root).unwrap()));
    let token = state.issue_token("acct_http").unwrap();
    let account = AccountId::from_str("acct_http").unwrap();
    let profile_id = ProfileId::from_str("prf_http_1").unwrap();
    let mut grants = state.grants.write().unwrap();
    for (capability, action) in [
        ("profile.create", Action::Create),
        ("profile.read", Action::Read),
        ("profile.archive", Action::Update),
        ("profile.restore", Action::Update),
        ("profile.delete", Action::Delete),
    ] {
        grants.push(Grant {
            id: GrantId::from_str(format!("grant_{}", capability.replace('.', "_")).as_str())
                .unwrap(),
            account_id: Some(account.clone()),
            member_id: None,
            capability: Capability::new(capability).unwrap(),
            action,
            scope: ResourceScope {
                profile_id: Some(profile_id.clone()),
                ..ResourceScope::default()
            },
            effect: Effect::Allow,
        });
    }
    drop(grants);
    let app: Router = router(state);

    let denied = app
        .clone()
        .oneshot(request("GET", "/v1/profiles/prf_http_1", &token, None))
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::NOT_FOUND);

    let created = app
        .clone()
        .oneshot(request(
            "POST",
            "/v1/profiles",
            &token,
            Some(serde_json::to_string(&profile()).unwrap()),
        ))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);

    let listed = app
        .clone()
        .oneshot(request("GET", "/v1/profiles", &token, None))
        .await
        .unwrap();
    assert_eq!(listed.status(), StatusCode::OK);

    let archived = app
        .clone()
        .oneshot(request(
            "PUT",
            "/v1/profiles/prf_http_1/archive",
            &token,
            Some(json!({"revision": 1}).to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(archived.status(), StatusCode::OK);

    let stale = app
        .clone()
        .oneshot(request(
            "PUT",
            "/v1/profiles/prf_http_1/restore",
            &token,
            Some(json!({"revision": 1}).to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(stale.status(), StatusCode::CONFLICT);

    let deleted = app
        .clone()
        .oneshot(request(
            "DELETE",
            "/v1/profiles/prf_http_1",
            &token,
            Some(json!({"revision": 2, "confirmation": "QUARANTINE_PROFILE_DATA"}).to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(deleted.status(), StatusCode::OK);
    let missing = app
        .clone()
        .oneshot(request("GET", "/v1/profiles/prf_http_1", &token, None))
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn profile_clone_and_share_routes_validate_path_identity() {
    let root = root("profile-route-identity");
    let _ = fs::remove_dir_all(&root);
    let state = ApiState::new(ProfileService::new(LocalRepository::open(&root).unwrap()));
    let token = state.issue_token("acct_route_identity").unwrap();
    let app = router(state);

    let clone = app
        .clone()
        .oneshot(request(
            "POST",
            "/v1/profiles/prf_source/clone",
            &token,
            Some(
                json!({
                    "sourceProfileId": "prf_other",
                    "targetProfileId": "prf_target",
                    "name": "Target",
                    "expectedRevision": 1
                })
                .to_string(),
            ),
        ))
        .await
        .unwrap();
    assert_eq!(clone.status(), StatusCode::BAD_REQUEST);

    let share = app
        .clone()
        .oneshot(request(
            "POST",
            "/v1/profiles/prf_source/shares",
            &token,
            Some(
                json!({
                    "profileId": "prf_other",
                    "memberId": "mem_one",
                    "permission": "read",
                    "expectedRevision": 1
                })
                .to_string(),
            ),
        ))
        .await
        .unwrap();
    assert_eq!(share.status(), StatusCode::BAD_REQUEST);

    let revoke = app
        .oneshot(request(
            "DELETE",
            "/v1/profiles/prf_source/shares/mem_one",
            &token,
            Some(
                json!({
                    "profileId": "prf_source",
                    "memberId": "mem_two",
                    "permission": "read",
                    "expectedRevision": 1
                })
                .to_string(),
            ),
        ))
        .await
        .unwrap();
    assert_eq!(revoke.status(), StatusCode::BAD_REQUEST);
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn managed_profile_create_provisions_binding_and_initial_manifest() {
    let account = AccountId::from_str("acct_managed_provision").unwrap();
    let mut store = MemoryControlRecordStore::default();
    let account_value =
        serde_json::to_value(Account::new(account.clone(), "2026-07-22T00:00:00Z")).unwrap();
    store
        .put(PutControlRecord {
            tenant_id: "control",
            record_type: "account",
            record_id: account.as_str(),
            expected_revision: Some(0),
            idempotency_key: "seed-managed-provision-account",
            value: &account_value,
        })
        .unwrap();
    let state = ApiState::hosted_with_control_store(ProfileService::disabled(), store).unwrap();
    let token = state.issue_token(account.as_str()).unwrap();
    let workspace = buglogin_domain::ids::WorkspaceId::from_str("ws_managed_provision").unwrap();
    let profile_id = ProfileId::from_str("prf_managed_provision").unwrap();
    let mut grants = state.grants.write().unwrap();
    grants.extend([
        Grant {
            id: GrantId::from_str("grant_managed_profile_create").unwrap(),
            account_id: Some(account.clone()),
            member_id: None,
            capability: Capability::new("profile.create").unwrap(),
            action: Action::Create,
            scope: ResourceScope {
                profile_id: Some(profile_id.clone()),
                ..ResourceScope::default()
            },
            effect: Effect::Allow,
        },
        Grant {
            id: GrantId::from_str("grant_managed_resource_create").unwrap(),
            account_id: Some(account.clone()),
            member_id: None,
            capability: Capability::new("CAP-RES-001").unwrap(),
            action: Action::Create,
            scope: ResourceScope::workspace(workspace.clone()),
            effect: Effect::Allow,
        },
        Grant {
            id: GrantId::from_str("grant_managed_profile_read").unwrap(),
            account_id: Some(account.clone()),
            member_id: None,
            capability: Capability::new("profile.read").unwrap(),
            action: Action::Read,
            scope: ResourceScope {
                profile_id: Some(profile_id.clone()),
                ..ResourceScope::default()
            },
            effect: Effect::Allow,
        },
        Grant {
            id: GrantId::from_str("grant_managed_manifest_read").unwrap(),
            account_id: Some(account.clone()),
            member_id: None,
            capability: Capability::new("profile.read").unwrap(),
            action: Action::Read,
            scope: ResourceScope::workspace(workspace.clone()),
            effect: Effect::Allow,
        },
        Grant {
            id: GrantId::from_str("grant_managed_proxy_create").unwrap(),
            account_id: Some(account.clone()),
            member_id: None,
            capability: Capability::new("CAP-NET-001").unwrap(),
            action: Action::Create,
            scope: ResourceScope::workspace(workspace.clone()),
            effect: Effect::Allow,
        },
        Grant {
            id: GrantId::from_str("grant_managed_proxy_read").unwrap(),
            account_id: Some(account.clone()),
            member_id: None,
            capability: Capability::new("CAP-NET-001").unwrap(),
            action: Action::Read,
            scope: ResourceScope::workspace(workspace.clone()),
            effect: Effect::Allow,
        },
    ]);
    drop(grants);
    let app = router(state.clone());
    let mut managed = profile();
    managed.id = profile_id.clone();
    managed.sync_mode = Some(SyncMode::Managed);
    managed.proxy_id = Some(ProxyId::from_str("proxy_missing").unwrap());
    let mut missing_create = lifecycle_request(
        "POST",
        "/v1/profiles",
        &token,
        "managed-provision-missing-proxy-0001",
        Some(serde_json::to_string(&managed).unwrap()),
    );
    missing_create.headers_mut().insert(
        "x-buglogin-workspace-id",
        workspace.as_str().parse().unwrap(),
    );
    assert_eq!(
        app.clone().oneshot(missing_create).await.unwrap().status(),
        StatusCode::NOT_FOUND
    );
    let proxy_create = lifecycle_request(
        "POST",
        "/v1/workspaces/ws_managed_provision/proxies",
        &token,
        "managed-provision-proxy-0001",
        Some(
            json!({
                "workspaceId": workspace,
                "proxy": {
                    "id": "proxy_managed_1",
                    "name": "Managed proxy",
                    "protocol": "http",
                    "host": "proxy.example",
                    "port": 8080,
                    "bypassRules": [],
                    "createdAt": "2026-07-22T00:00:00Z",
                    "updatedAt": "2026-07-22T00:00:00Z"
                }
            })
            .to_string(),
        ),
    );
    assert_eq!(
        app.clone().oneshot(proxy_create).await.unwrap().status(),
        StatusCode::OK
    );
    managed.proxy_id = Some(ProxyId::from_str("proxy_managed_1").unwrap());
    let mut create = lifecycle_request(
        "POST",
        "/v1/profiles",
        &token,
        "managed-provision-key-0001",
        Some(serde_json::to_string(&managed).unwrap()),
    );
    create.headers_mut().insert(
        "x-buglogin-workspace-id",
        workspace.as_str().parse().unwrap(),
    );
    let created = app.clone().oneshot(create).await.unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    let created_body: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(created.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(created_body["value"]["state"], "ready");
    assert_eq!(created_body["value"]["proxyId"], "proxy_managed_1");

    let mut retry_profile = managed.clone();
    retry_profile.created_at = "2026-10-12T01:00:00Z".to_owned();
    retry_profile.updated_at = retry_profile.created_at.clone();
    let mut retry = lifecycle_request(
        "POST",
        "/v1/profiles",
        &token,
        "managed-provision-key-0001",
        Some(serde_json::to_string(&retry_profile).unwrap()),
    );
    retry.headers_mut().insert(
        "x-buglogin-workspace-id",
        workspace.as_str().parse().unwrap(),
    );
    let replayed = app.clone().oneshot(retry).await.unwrap();
    assert_eq!(replayed.status(), StatusCode::OK);
    let replayed_body: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(replayed.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(replayed_body, created_body);

    let binding_response = app
        .clone()
        .oneshot(request(
            "GET",
            "/v1/profiles/prf_managed_provision/binding",
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(binding_response.status(), StatusCode::OK);
    let binding: ProfileResource = serde_json::from_slice(
        &axum::body::to_bytes(binding_response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(binding.workspace_id, workspace);

    let manifest_path = format!(
        "/v1/workspaces/{}/resources/{}/manifest",
        binding.workspace_id.as_str(),
        binding.resource_id.as_str()
    );
    let mut manifest_request = request("GET", &manifest_path, &token, None);
    manifest_request
        .headers_mut()
        .insert("x-buglogin-actor-id", "desktop-provision".parse().unwrap());
    let manifest_response = app.oneshot(manifest_request).await.unwrap();
    assert_eq!(manifest_response.status(), StatusCode::OK);
    let manifest: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(manifest_response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["generation"], 1);
    assert_eq!(manifest["objects"], serde_json::json!([]));
}

#[tokio::test]
async fn profile_lifecycle_keeps_operation_and_state_fail_closed_without_runtime() {
    let root = root("lifecycle");
    let _ = fs::remove_dir_all(&root);
    let state = ApiState::new(ProfileService::new(LocalRepository::open(&root).unwrap()));
    let token = state.issue_token("acct_lifecycle").unwrap();
    let account = AccountId::from_str("acct_lifecycle").unwrap();
    let profile_id = ProfileId::from_str("prf_http_1").unwrap();
    state.profiles.create(profile()).unwrap();
    for capability in ["profile.launch", "profile.stop"] {
        state.grants.write().unwrap().push(Grant {
            id: GrantId::from_str(format!("grant_{}", capability.replace('.', "_")).as_str())
                .unwrap(),
            account_id: Some(account.clone()),
            member_id: None,
            capability: Capability::new(capability).unwrap(),
            action: Action::Execute,
            scope: ResourceScope {
                profile_id: Some(profile_id.clone()),
                ..ResourceScope::default()
            },
            effect: Effect::Allow,
        });
    }
    let app: Router = router(state.clone());

    let launch = app
        .clone()
        .oneshot(lifecycle_request(
            "POST",
            "/v1/profiles/prf_http_1/launch",
            &token,
            "lifecycle-launch-0001",
            Some(
                json!({
                    "profilesRoot": root.join("profiles"),
                    "extraArgs": []
                })
                .to_string(),
            ),
        ))
        .await
        .unwrap();
    assert_eq!(launch.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = axum::body::to_bytes(launch.into_body(), usize::MAX)
        .await
        .unwrap();
    let operation: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(operation["status"], "failed");
    assert_eq!(operation["error"]["code"], "runtime_not_approved");
    assert!(state.lifecycles.read().unwrap().is_empty());
    assert_eq!(
        state
            .profiles
            .get(&profile_id)
            .unwrap()
            .unwrap()
            .value
            .state,
        ProfileState::Created
    );

    let stop = app
        .oneshot(lifecycle_request(
            "POST",
            "/v1/profiles/prf_http_1/stop",
            &token,
            "lifecycle-stop-0001",
            None,
        ))
        .await
        .unwrap();
    assert_eq!(stop.status(), StatusCode::CONFLICT);
    assert!(state.lifecycles.read().unwrap().is_empty());
}

#[tokio::test]
async fn hosted_profile_archive_restore_replays_and_survives_restart() {
    let root = root("hosted-archive-restore");
    let _ = fs::remove_dir_all(&root);
    let account = AccountId::from_str("acct_hosted_profile_mutation").unwrap();
    let profile_id = ProfileId::from_str("prf_hosted_profile_mutation").unwrap();
    let workspace_id = WorkspaceId::from_str("ws_hosted_profile_mutation").unwrap();
    let mut store = MemoryControlRecordStore::default();
    let account_value =
        serde_json::to_value(Account::new(account.clone(), "2026-07-22T00:00:00Z")).unwrap();
    store
        .put(PutControlRecord {
            tenant_id: "control",
            record_type: "account",
            record_id: account.as_str(),
            expected_revision: Some(0),
            idempotency_key: "seed-hosted-profile-account",
            value: &account_value,
        })
        .unwrap();
    let reopen = store.reopen();
    let state = ApiState::hosted_with_control_store(ProfileService::disabled(), store).unwrap();
    let token = state.issue_token(account.as_str()).unwrap();
    state.grants.write().unwrap().extend([
        Grant {
            id: GrantId::from_str("grant_hosted_profile_create").unwrap(),
            account_id: Some(account.clone()),
            member_id: None,
            capability: Capability::new("profile.create").unwrap(),
            action: Action::Create,
            scope: ResourceScope {
                profile_id: Some(profile_id.clone()),
                ..ResourceScope::default()
            },
            effect: Effect::Allow,
        },
        Grant {
            id: GrantId::from_str("grant_hosted_profile_resource").unwrap(),
            account_id: Some(account.clone()),
            member_id: None,
            capability: Capability::new("CAP-RES-001").unwrap(),
            action: Action::Create,
            scope: ResourceScope::workspace(workspace_id.clone()),
            effect: Effect::Allow,
        },
        Grant {
            id: GrantId::from_str("grant_hosted_profile_read").unwrap(),
            account_id: Some(account.clone()),
            member_id: None,
            capability: Capability::new("profile.read").unwrap(),
            action: Action::Read,
            scope: ResourceScope {
                profile_id: Some(profile_id.clone()),
                ..ResourceScope::default()
            },
            effect: Effect::Allow,
        },
        Grant {
            id: GrantId::from_str("grant_hosted_profile_archive").unwrap(),
            account_id: Some(account.clone()),
            member_id: None,
            capability: Capability::new("profile.archive").unwrap(),
            action: Action::Update,
            scope: ResourceScope {
                profile_id: Some(profile_id.clone()),
                ..ResourceScope::default()
            },
            effect: Effect::Allow,
        },
        Grant {
            id: GrantId::from_str("grant_hosted_profile_restore").unwrap(),
            account_id: Some(account.clone()),
            member_id: None,
            capability: Capability::new("profile.restore").unwrap(),
            action: Action::Update,
            scope: ResourceScope {
                profile_id: Some(profile_id.clone()),
                ..ResourceScope::default()
            },
            effect: Effect::Allow,
        },
    ]);
    let app = router(state.clone());
    let mut managed = profile();
    managed.id = profile_id.clone();
    managed.state = ProfileState::Created;
    managed.sync_mode = Some(SyncMode::Managed);
    let mut create = lifecycle_request(
        "POST",
        "/v1/profiles",
        &token,
        "hosted-profile-create-0001",
        Some(serde_json::to_string(&managed).unwrap()),
    );
    create.headers_mut().insert(
        "x-buglogin-workspace-id",
        workspace_id.as_str().parse().unwrap(),
    );
    assert_eq!(
        app.clone().oneshot(create).await.unwrap().status(),
        StatusCode::OK
    );

    let archive_body = json!({"revision": 1}).to_string();
    let archived = app
        .clone()
        .oneshot(lifecycle_request(
            "PUT",
            "/v1/profiles/prf_hosted_profile_mutation/archive",
            &token,
            "hosted-profile-archive-0001",
            Some(archive_body.clone()),
        ))
        .await
        .unwrap();
    assert_eq!(archived.status(), StatusCode::OK);
    let archived_bytes = axum::body::to_bytes(archived.into_body(), usize::MAX)
        .await
        .unwrap();
    let archived_value: serde_json::Value = serde_json::from_slice(&archived_bytes).unwrap();
    assert_eq!(archived_value["value"]["archived"], true);
    assert_eq!(archived_value["revision"], 2);

    let replay = app
        .clone()
        .oneshot(lifecycle_request(
            "PUT",
            "/v1/profiles/prf_hosted_profile_mutation/archive",
            &token,
            "hosted-profile-archive-0001",
            Some(archive_body),
        ))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(
        axum::body::to_bytes(replay.into_body(), usize::MAX)
            .await
            .unwrap(),
        archived_bytes
    );

    let conflict = app
        .clone()
        .oneshot(lifecycle_request(
            "PUT",
            "/v1/profiles/prf_hosted_profile_mutation/archive",
            &token,
            "hosted-profile-archive-0001",
            Some(json!({"revision": 2}).to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(conflict.status(), StatusCode::CONFLICT);

    let stale = app
        .clone()
        .oneshot(lifecycle_request(
            "PUT",
            "/v1/profiles/prf_hosted_profile_mutation/restore",
            &token,
            "hosted-profile-restore-stale-0001",
            Some(json!({"revision": 1}).to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(stale.status(), StatusCode::CONFLICT);

    let restored = app
        .clone()
        .oneshot(lifecycle_request(
            "PUT",
            "/v1/profiles/prf_hosted_profile_mutation/restore",
            &token,
            "hosted-profile-restore-0001",
            Some(json!({"revision": 2}).to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(restored.status(), StatusCode::OK);

    drop(app);
    drop(state);
    let restarted =
        ApiState::hosted_with_control_store(ProfileService::disabled(), reopen).unwrap();
    let restarted_token = restarted.issue_token(account.as_str()).unwrap();
    let restarted_app = router(restarted);
    let loaded = restarted_app
        .clone()
        .oneshot(request(
            "GET",
            "/v1/profiles/prf_hosted_profile_mutation",
            &restarted_token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(loaded.status(), StatusCode::OK);
    let loaded: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(loaded.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(loaded["revision"], 3);
    assert_eq!(loaded["value"]["archived"], false);
    let binding = restarted_app
        .oneshot(request(
            "GET",
            "/v1/profiles/prf_hosted_profile_mutation/binding",
            &restarted_token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(binding.status(), StatusCode::OK);
    let binding: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(binding.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(binding["archiveState"], "active");
    let _ = fs::remove_dir_all(root);
}
