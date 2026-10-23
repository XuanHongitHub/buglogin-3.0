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
    ids::{AccountId, GrantId, MemberId, ProfileId, ResourceId, WorkspaceId},
    member::{Member, MemberRole, MemberStatus},
    permissions::{Action, Capability, Effect, Grant, Principal, ResourceScope, Role},
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
        grant(&account, &workspace, Action::Share, "share"),
        grant(&account, &other_workspace, Action::Read, "other_read"),
    ]);
    state.grants.write().unwrap().push(Grant {
        id: GrantId::from_str("grant_group_member_create").unwrap(),
        account_id: Some(account.clone()),
        member_id: None,
        capability: Capability::new("CAP-MEM-001").unwrap(),
        action: Action::Create,
        scope: ResourceScope::workspace(workspace.clone()),
        effect: Effect::Allow,
    });
    let owner_member_id = MemberId::from_str("mem_group_http_owner").unwrap();
    state
        .members
        .write()
        .unwrap()
        .invite(
            &Principal {
                roles: [Role::PlatformAdmin].into_iter().collect(),
                ..Principal::default()
            },
            &[],
            Member {
                id: owner_member_id.clone(),
                account_id: account.clone(),
                workspace_id: workspace.clone(),
                team_id: None,
                role: MemberRole::Owner,
                status: MemberStatus::Invited,
                invite_expires_at: Some(u64::MAX),
            },
            1,
        )
        .unwrap();
    state
        .members
        .write()
        .unwrap()
        .activate(
            &Principal {
                account_id: Some(account.clone()),
                ..Principal::default()
            },
            &[],
            &workspace,
            &owner_member_id,
            2,
        )
        .unwrap();
    let member_id = buglogin_domain::ids::MemberId::from_str("mem_group_share_target").unwrap();
    let target_account = AccountId::from_str("acct_group_share_target").unwrap();
    state
        .members
        .write()
        .unwrap()
        .invite(
            &buglogin_domain::permissions::Principal {
                account_id: Some(account.clone()),
                ..Default::default()
            },
            &state.grants.read().unwrap(),
            Member {
                id: member_id.clone(),
                account_id: target_account.clone(),
                workspace_id: workspace.clone(),
                team_id: None,
                role: MemberRole::Member,
                status: MemberStatus::Invited,
                invite_expires_at: Some(100),
            },
            1,
        )
        .unwrap();
    state
        .members
        .write()
        .unwrap()
        .activate(
            &buglogin_domain::permissions::Principal {
                account_id: Some(target_account),
                ..Default::default()
            },
            &[],
            &workspace,
            &member_id,
            2,
        )
        .unwrap();
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

    let shares_path = format!("{path}/grp_http_group/shares");
    let unauthorized_token = state.issue_token("acct_group_http_unauthorized").unwrap();
    for (target_member_id, key) in [
        ("mem_group_share_target", "group-share-denied-active-01"),
        ("mem_group_share_missing", "group-share-denied-missing-1"),
    ] {
        let denied = app
            .clone()
            .oneshot(request(
                "POST",
                &shares_path,
                &unauthorized_token,
                Some(key),
                Some(json!({
                    "workspaceId": "ws_group_http",
                    "groupId": "grp_http_group",
                    "memberId": target_member_id,
                    "action": "read",
                    "effect": "allow"
                })),
            ))
            .await
            .unwrap();
        assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    }
    let created_share = app
        .clone()
        .oneshot(request(
            "POST",
            &shares_path,
            &token,
            Some("group-share-create-0001"),
            Some(json!({
                "workspaceId": "ws_group_http",
                "groupId": "grp_http_group",
                "memberId": member_id,
                "action": "read",
                "effect": "allow"
            })),
        ))
        .await
        .unwrap();
    assert_eq!(created_share.status(), StatusCode::OK);
    let created_share_body = body(created_share).await;
    assert_eq!(created_share_body["revision"], 1);
    let grant_id = created_share_body["id"].as_str().unwrap();
    let listed_shares = app
        .clone()
        .oneshot(request("GET", &shares_path, &token, None, None))
        .await
        .unwrap();
    assert_eq!(listed_shares.status(), StatusCode::OK);
    assert_eq!(body(listed_shares).await.as_array().unwrap().len(), 1);
    let revoked_share = app
        .clone()
        .oneshot(request(
            "DELETE",
            &format!("{shares_path}/{grant_id}"),
            &token,
            Some("group-share-revoke-0001"),
            Some(json!({
                "workspaceId": "ws_group_http",
                "groupId": "grp_http_group",
                "grantId": grant_id,
                "expectedRevision": 1
            })),
        ))
        .await
        .unwrap();
    assert_eq!(revoked_share.status(), StatusCode::OK);
    assert_eq!(body(revoked_share).await["revision"], 2);

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
    assert_eq!(delete.status(), StatusCode::CONFLICT);
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
    state.grants.write().unwrap().push(Grant {
        id: GrantId::from_str("grant_profile_metadata_update").unwrap(),
        account_id: Some(account_id.clone()),
        member_id: None,
        capability: Capability::new("profile.update").unwrap(),
        action: Action::Update,
        scope: ResourceScope::workspace(workspace_id.clone()),
        effect: Effect::Allow,
    });
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

    let metadata_path = "/v1/workspaces/ws_group_membership/profiles/prf_group_membership";
    let metadata_input = json!({
        "workspaceId": "ws_group_membership",
        "profileId": "prf_group_membership",
        "expectedRevision": 2,
        "name": "  Updated profile  ",
        "note": "Managed profile note",
        "tags": [" social ", "account"]
    });
    let metadata_updated = router(state.clone())
        .oneshot(request(
            "PUT",
            metadata_path,
            &token,
            Some("profile-update-metadata-0001"),
            Some(metadata_input.clone()),
        ))
        .await
        .unwrap();
    assert_eq!(metadata_updated.status(), StatusCode::OK);
    let metadata_body = body(metadata_updated).await;
    assert_eq!(metadata_body["revision"], 3);
    assert_eq!(metadata_body["name"], "Updated profile");
    assert_eq!(metadata_body["note"], "Managed profile note");
    assert_eq!(metadata_body["tags"], json!(["account", "social"]));

    let metadata_replay = router(state.clone())
        .oneshot(request(
            "PUT",
            metadata_path,
            &token,
            Some("profile-update-metadata-0001"),
            Some(metadata_input),
        ))
        .await
        .unwrap();
    assert_eq!(metadata_replay.status(), StatusCode::OK);
    assert_eq!(body(metadata_replay).await, metadata_body);

    let stale_metadata = router(state.clone())
        .oneshot(request(
            "PUT",
            metadata_path,
            &token,
            Some("profile-update-metadata-stale-0001"),
            Some(json!({
                "workspaceId": "ws_group_membership",
                "profileId": "prf_group_membership",
                "expectedRevision": 2,
                "name": "Stale profile name",
                "note": null,
                "tags": []
            })),
        ))
        .await
        .unwrap();
    assert_eq!(stale_metadata.status(), StatusCode::CONFLICT);

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

    state
        .profile_proxy_assignments
        .write()
        .unwrap()
        .set(
            &workspace_id,
            buglogin_api::services::profile_proxy_assignment::ProfileProxyAssignmentSetInput {
                workspace_id: workspace_id.clone(),
                profile_id: profile_id.clone(),
                expected_profile_revision: 3,
                proxy_id: Some(
                    buglogin_domain::ids::ProxyId::from_str("prx_pending_group").unwrap(),
                ),
            },
        )
        .unwrap();
    let pending_metadata = router(state.clone())
        .oneshot(request(
            "PUT",
            metadata_path,
            &token,
            Some("profile-update-pending-proxy-0001"),
            Some(json!({
                "workspaceId": "ws_group_membership",
                "profileId": "prf_group_membership",
                "expectedRevision": 3,
                "name": "Blocked metadata edit",
                "note": null,
                "tags": []
            })),
        ))
        .await
        .unwrap();
    assert_eq!(pending_metadata.status(), StatusCode::CONFLICT);
    assert_eq!(
        body(pending_metadata).await["code"],
        "profile_proxy_assignment_pending"
    );

    let pending_groups = router(state.clone())
        .oneshot(request(
            "PUT",
            replace_path,
            &token,
            Some("profile-groups-pending-proxy-0001"),
            Some(json!({
                "workspaceId": "ws_group_membership",
                "profileId": "prf_group_membership",
                "expectedRevision": 3,
                "groupIds": []
            })),
        ))
        .await
        .unwrap();
    assert_eq!(pending_groups.status(), StatusCode::CONFLICT);
    assert_eq!(
        body(pending_groups).await["code"],
        "profile_proxy_assignment_pending"
    );

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
