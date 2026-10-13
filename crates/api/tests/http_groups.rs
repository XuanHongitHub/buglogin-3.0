use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
    response::Response,
};
use buglogin_api::http::{ApiState, router};
use buglogin_browser::profile::ProfileService;
use buglogin_domain::{
    account::Account,
    entities::{Profile, ProfileState, Runtime, SyncMode},
    ids::{AccountId, GrantId, ProfileId, ResourceId, WorkspaceId},
    permissions::{Action, Capability, Effect, Grant, ResourceScope},
    profile::{ProfileArchiveState, ProfileResource},
};
use buglogin_storage::{
    control::{ControlRecordStore, MemoryControlRecordStore, PutControlRecord},
    local::LocalRepository,
};
use serde_json::{Value, json};
use tower::ServiceExt;

fn root() -> PathBuf {
    static ROOT_SEQUENCE: AtomicU64 = AtomicU64::new(1);
    let sequence = ROOT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "buglogin-http-groups-{}-{sequence}",
        std::process::id()
    ))
}

fn request(
    method: &str,
    uri: &str,
    token: &str,
    key: Option<&str>,
    body: Option<Value>,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .header("x-correlation-id", "http-groups-test");
    if let Some(key) = key {
        builder = builder.header("idempotency-key", key);
    }
    match body {
        Some(body) => builder
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    }
}

async fn body(response: Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 64 * 1024).await.unwrap()).unwrap()
}

fn grant(account: &AccountId, workspace: &WorkspaceId, action: Action, suffix: &str) -> Grant {
    Grant {
        id: GrantId::from_str(&format!("grant_group_{suffix}")).unwrap(),
        account_id: Some(account.clone()),
        member_id: None,
        capability: Capability::new("CAP-PROF-003").unwrap(),
        action,
        scope: ResourceScope::workspace(workspace.clone()),
        effect: Effect::Allow,
    }
}

#[tokio::test]
async fn group_http_contract_enforces_acl_revisions_scope_and_idempotency() {
    let root = root();
    let _ = fs::remove_dir_all(&root);
    let state = ApiState::new(ProfileService::new(LocalRepository::open(&root).unwrap()));
    let token = state.issue_token("acct_group_http").unwrap();
    let account = AccountId::from_str("acct_group_http").unwrap();
    let workspace = WorkspaceId::from_str("ws_group_http").unwrap();
    let other_workspace = WorkspaceId::from_str("ws_group_other").unwrap();
    state.grants.write().unwrap().extend([
        grant(&account, &workspace, Action::Read, "read"),
        grant(&account, &workspace, Action::Create, "create"),
        grant(&account, &workspace, Action::Update, "update"),
        grant(&account, &workspace, Action::Delete, "delete"),
        grant(&account, &other_workspace, Action::Read, "other_read"),
    ]);
    let app = router(state.clone());
    let path = "/v1/workspaces/ws_group_http/groups";
    let create = json!({
        "workspaceId": "ws_group_http",
        "groupId": "grp_http_group",
        "name": "  Accounts  ",
        "appearance": {"color": "blue"}
    });

    let created = app
        .clone()
        .oneshot(request(
            "POST",
            path,
            &token,
            Some("group-create-0001"),
            Some(create.clone()),
        ))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    let created_body = body(created).await;
    assert_eq!(created_body["group"]["name"], "Accounts");
    assert_eq!(created_body["revision"], 1);

    let replay = app
        .clone()
        .oneshot(request(
            "POST",
            path,
            &token,
            Some("group-create-0001"),
            Some(create),
        ))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(body(replay).await, created_body);

    let update_path = "/v1/workspaces/ws_group_http/groups/grp_http_group";
    let updated = app
        .clone()
        .oneshot(request(
            "PUT",
            update_path,
            &token,
            Some("group-update-0001"),
            Some(json!({
                "workspaceId": "ws_group_http", "groupId": "grp_http_group", "expectedRevision": 1,
                "name": "Social", "appearance": null
            })),
        ))
        .await
        .unwrap();
    assert_eq!(updated.status(), StatusCode::OK);
    assert_eq!(body(updated).await["revision"], 2);

    let stale = app
        .clone()
        .oneshot(request(
            "PUT",
            update_path,
            &token,
            Some("group-update-stale-0001"),
            Some(json!({
                "workspaceId": "ws_group_http", "groupId": "grp_http_group", "expectedRevision": 1,
                "name": "Stale", "appearance": null
            })),
        ))
        .await
        .unwrap();
    assert_eq!(stale.status(), StatusCode::CONFLICT);

    let cross_workspace = app
        .clone()
        .oneshot(request(
            "GET",
            "/v1/workspaces/ws_group_other/groups/grp_http_group",
            &token,
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(cross_workspace.status(), StatusCode::NOT_FOUND);

    let list = app
        .clone()
        .oneshot(request("GET", path, &token, None, None))
        .await
        .unwrap();
    assert_eq!(list.status(), StatusCode::OK);
    assert_eq!(body(list).await.as_array().unwrap().len(), 1);

    let delete = app
        .oneshot(request(
            "DELETE",
            update_path,
            &token,
            Some("group-delete-0001"),
            Some(json!({
                "workspaceId": "ws_group_http", "groupId": "grp_http_group", "expectedRevision": 2
            })),
        ))
        .await
        .unwrap();
    assert_eq!(delete.status(), StatusCode::OK);
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn hosted_group_state_survives_api_state_reopen() {
    let mut store = MemoryControlRecordStore::default();
    let account_id = AccountId::from_str("acct_group_restart").unwrap();
    let account =
        serde_json::to_value(Account::new(account_id.clone(), "2026-10-13T00:00:00Z")).unwrap();
    store
        .put(PutControlRecord {
            tenant_id: "control",
            record_type: "account",
            record_id: account_id.as_str(),
            expected_revision: Some(0),
            idempotency_key: "seed-group-restart-account",
            value: &account,
        })
        .unwrap();

    let workspace = WorkspaceId::from_str("ws_group_restart").unwrap();
    let first =
        ApiState::hosted_with_control_store(ProfileService::disabled(), store.clone()).unwrap();
    let token = first.issue_token(account_id.as_str()).unwrap();
    first.grants.write().unwrap().push(grant(
        &account_id,
        &workspace,
        Action::Create,
        "restart_create",
    ));
    let create = router(first)
        .oneshot(request(
            "POST",
            "/v1/workspaces/ws_group_restart/groups",
            &token,
            Some("group-create-restart-0001"),
            Some(json!({
                "workspaceId": "ws_group_restart",
                "groupId": "grp_group_restart",
                "name": "Persistent"
            })),
        ))
        .await
        .unwrap();
    assert_eq!(create.status(), StatusCode::OK);

    let reopened = ApiState::hosted_with_control_store(ProfileService::disabled(), store).unwrap();
    reopened.grants.write().unwrap().push(grant(
        &account_id,
        &workspace,
        Action::Read,
        "restart_read",
    ));
    let listed = router(reopened)
        .oneshot(request(
            "GET",
            "/v1/workspaces/ws_group_restart/groups",
            &token,
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(listed.status(), StatusCode::OK);
    let records = body(listed).await;
    assert_eq!(records.as_array().unwrap().len(), 1);
    assert_eq!(records[0]["group"]["name"], "Persistent");
}

#[tokio::test]
async fn profile_group_replacement_is_workspace_scoped_and_revision_safe() {
    let root = root();
    let _ = fs::remove_dir_all(&root);
    let profile_id = ProfileId::from_str("prf_group_membership").unwrap();
    let workspace_id = WorkspaceId::from_str("ws_group_membership").unwrap();
    let account_id = AccountId::from_str("acct_group_membership").unwrap();
    let profile = Profile {
        id: profile_id.clone(),
        name: "Group membership profile".to_owned(),
        runtime: Runtime::Wayfern,
        runtime_version: Some("1.0.0".to_owned()),
        runtime_config: None,
        state: ProfileState::Created,
        archived: false,
        fingerprint: None,
        note: None,
        proxy_id: None,
        vpn_id: None,
        group_ids: Vec::new(),
        tags: Vec::new(),
        extension_group_id: None,
        sync_mode: Some(SyncMode::Managed),
        created_at: "2026-10-13T00:00:00Z".to_owned(),
        updated_at: "2026-10-13T00:00:00Z".to_owned(),
    };
    let state = ApiState::new(ProfileService::new(LocalRepository::open(&root).unwrap()));
    state.profiles.create(profile).unwrap();
    state.grants.write().unwrap().extend([
        grant(
            &account_id,
            &workspace_id,
            Action::Create,
            "membership_create",
        ),
        grant(
            &account_id,
            &workspace_id,
            Action::Update,
            "membership_update",
        ),
    ]);
    let token = state.issue_token(account_id.as_str()).unwrap();
    let state = state.with_profile_resource(ProfileResource {
        profile_id: profile_id.clone(),
        resource_id: ResourceId::from_str("res_group_membership").unwrap(),
        workspace_id: workspace_id.clone(),
        archive_state: ProfileArchiveState::Active,
    });
    let group_path = "/v1/workspaces/ws_group_membership/groups";
    let create = router(state.clone())
        .oneshot(request(
            "POST",
            group_path,
            &token,
            Some("group-membership-create-0001"),
            Some(json!({
                "workspaceId": "ws_group_membership",
                "groupId": "grp_membership_local",
                "name": "Local group"
            })),
        ))
        .await
        .unwrap();
    assert_eq!(create.status(), StatusCode::OK);

    let replace_path = "/v1/profiles/prf_group_membership/groups";
    let replace = |workspace: &str, expected_revision: u64, group_ids: Value| {
        request(
            "PUT",
            replace_path,
            &token,
            Some("profile-groups-replace-0001"),
            Some(json!({
                "workspaceId": workspace,
                "profileId": "prf_group_membership",
                "expectedRevision": expected_revision,
                "groupIds": group_ids
            })),
        )
    };
    let updated = router(state.clone())
        .oneshot(replace(
            "ws_group_membership",
            1,
            json!(["grp_membership_local"]),
        ))
        .await
        .unwrap();
    assert_eq!(updated.status(), StatusCode::OK);
    let updated_body = body(updated).await;
    assert_eq!(updated_body["revision"], 2);
    assert_eq!(updated_body["groupIds"], json!(["grp_membership_local"]));

    let replay = router(state.clone())
        .oneshot(replace(
            "ws_group_membership",
            1,
            json!(["grp_membership_local"]),
        ))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(body(replay).await, updated_body);

    let stale = router(state.clone())
        .oneshot(request(
            "PUT",
            replace_path,
            &token,
            Some("profile-groups-stale-0001"),
            Some(json!({
                "workspaceId": "ws_group_membership",
                "profileId": "prf_group_membership",
                "expectedRevision": 1,
                "groupIds": []
            })),
        ))
        .await
        .unwrap();
    assert_eq!(stale.status(), StatusCode::CONFLICT);

    let foreign = router(state.clone())
        .oneshot(replace(
            "ws_group_foreign",
            2,
            json!(["grp_membership_local"]),
        ))
        .await
        .unwrap();
    assert_eq!(foreign.status(), StatusCode::FORBIDDEN);

    let _ = fs::remove_dir_all(root);
}
