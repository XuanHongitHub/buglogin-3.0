use std::{fs, path::PathBuf};

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
    response::Response,
};
use buglogin_api::http::{ApiState, router};
use buglogin_browser::profile::ProfileService;
use buglogin_domain::{
    ids::{AccountId, GrantId, MemberId, OrganizationId, ResourceId, WorkspaceId},
    member::{Member, MemberRole, MemberStatus},
    organization::{Organization, OrganizationPolicy},
    permissions::{Action, Capability, Effect, Grant, Principal, ResourceScope, Role},
    workspace::{Workspace, WorkspacePolicy},
};
use buglogin_storage::local::LocalRepository;
use serde_json::{Value, json};
use tower::ServiceExt;

const CORRELATION: &str = "control-http-correlation";
const SECRET_CANARY: &str = "live-control-secret-canary";

fn root(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "buglogin-http-control-{label}-{}",
        std::process::id()
    ))
}

#[tokio::test]
async fn account_admin_and_team_crud_are_acl_and_revision_checked() {
    let base = root("account-team");
    let _ = fs::remove_dir_all(&base);
    let state = ApiState::new(ProfileService::new(LocalRepository::open(&base).unwrap()));
    let admin_token = state.issue_token("acct_control_admin").unwrap();
    let target_token = state.issue_token("acct_control_target").unwrap();
    let denied_token = state.issue_token("acct_control_team_denied").unwrap();
    let target_account = AccountId::from_str("acct_control_target").unwrap();
    let target_session = state
        .auth
        .write()
        .unwrap()
        .issue_browser(target_account, u64::MAX, 1)
        .unwrap();
    let auth = std::sync::Arc::clone(&state.auth);
    let admin = AccountId::from_str("acct_control_admin").unwrap();
    let workspace = WorkspaceId::from_str("ws_account_team").unwrap();
    state.grants.write().unwrap().extend([
        allow(
            &admin,
            "grant_account_list",
            "CAP-ADM-001",
            Action::Read,
            ResourceScope::global(),
        ),
        allow(
            &admin,
            "grant_account_manage",
            "CAP-ADM-001",
            Action::Manage,
            ResourceScope::global(),
        ),
        allow(
            &admin,
            "grant_account_create",
            "CAP-ADM-001",
            Action::Create,
            ResourceScope::global(),
        ),
        allow(
            &admin,
            "grant_team_create",
            "CAP-TEAM-001",
            Action::Create,
            ResourceScope::workspace(workspace.clone()),
        ),
        allow(
            &admin,
            "grant_team_read",
            "CAP-TEAM-001",
            Action::Read,
            ResourceScope::workspace(workspace.clone()),
        ),
        allow(
            &admin,
            "grant_team_update",
            "CAP-TEAM-001",
            Action::Update,
            ResourceScope::workspace(workspace.clone()),
        ),
        allow(
            &admin,
            "grant_team_delete",
            "CAP-TEAM-001",
            Action::Delete,
            ResourceScope::workspace(workspace.clone()),
        ),
        allow(
            &admin,
            "grant_member_create_for_hydration",
            "CAP-MEM-001",
            Action::Create,
            ResourceScope::workspace(WorkspaceId::from_str("ws_account_team").unwrap()),
        ),
    ]);
    let admin_member_id = MemberId::from_str("mem_control_admin").unwrap();
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
                id: admin_member_id.clone(),
                account_id: admin.clone(),
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
                account_id: Some(admin.clone()),
                ..Principal::default()
            },
            &[],
            &workspace,
            &admin_member_id,
            2,
        )
        .unwrap();
    let member_id = MemberId::from_str("mem_control_hydrated").unwrap();
    state
        .members
        .write()
        .unwrap()
        .invite(
            &Principal {
                account_id: Some(admin.clone()),
                ..Principal::default()
            },
            &state.grants.read().unwrap(),
            Member {
                id: member_id.clone(),
                account_id: AccountId::from_str("acct_control_team_denied").unwrap(),
                workspace_id: WorkspaceId::from_str("ws_account_team").unwrap(),
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
            &Principal {
                account_id: Some(AccountId::from_str("acct_control_team_denied").unwrap()),
                ..Principal::default()
            },
            &[],
            &WorkspaceId::from_str("ws_account_team").unwrap(),
            &member_id,
            2,
        )
        .unwrap();
    let app = router(state);

    let denied_accounts = app
        .clone()
        .oneshot(request(
            "GET",
            "/v1/accounts",
            Some(&denied_token),
            None,
            Some("account-list-denied"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(denied_accounts.status(), StatusCode::FORBIDDEN);

    let accounts = app
        .clone()
        .oneshot(request(
            "GET",
            "/v1/accounts",
            Some(&admin_token),
            None,
            Some("account-list"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(accounts.status(), StatusCode::OK);
    let accounts = json_body(accounts).await;
    assert_eq!(accounts["items"].as_array().unwrap().len(), 3);
    let target_revision = accounts["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|record| record["account"]["id"] == "acct_control_target")
        .and_then(|record| record["account"]["revision"].as_u64())
        .expect("target account revision");

    let create_input = json!({
        "accountId": "acct_control_created",
        "username": "created-user",
        "email": "created@example.test"
    });
    let denied_create = app
        .clone()
        .oneshot(request(
            "POST",
            "/v1/accounts",
            Some(&denied_token),
            Some("account-create-denied-0001"),
            Some("account-create-denied-idem"),
            Some(create_input.clone()),
        ))
        .await
        .unwrap();
    assert_eq!(denied_create.status(), StatusCode::FORBIDDEN);

    let created = app
        .clone()
        .oneshot(request(
            "POST",
            "/v1/accounts",
            Some(&admin_token),
            Some("account-create-allowed-idem"),
            Some("account-create-allowed-0001"),
            Some(create_input.clone()),
        ))
        .await
        .unwrap();
    let created_status = created.status();
    let created_body = json_body(created).await;
    assert_eq!(created_status, StatusCode::CREATED, "{created_body}");
    assert_eq!(created_body["identity"]["username"], "created-user");
    assert_eq!(created_body["identity"]["verification"], "pending");
    assert_eq!(created_body["loginEnabled"], false);

    let replayed = app
        .clone()
        .oneshot(request(
            "POST",
            "/v1/accounts",
            Some(&admin_token),
            Some("account-create-allowed-idem"),
            Some("account-create-allowed-0002"),
            Some(create_input),
        ))
        .await
        .unwrap();
    assert_eq!(replayed.status(), StatusCode::CREATED);
    assert_eq!(json_body(replayed).await["replayed"], true);

    let updated_accounts = app
        .clone()
        .oneshot(request(
            "GET",
            "/v1/accounts",
            Some(&admin_token),
            None,
            Some("account-list-after-create"),
            None,
        ))
        .await
        .unwrap();
    let updated_accounts = json_body(updated_accounts).await;
    assert_eq!(updated_accounts["items"].as_array().unwrap().len(), 4);
    assert_eq!(
        updated_accounts["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|record| record["account"]["id"] == "acct_control_created")
            .unwrap()["identity"]["email"],
        "created@example.test"
    );

    let suspended = app
        .clone()
        .oneshot(request(
            "PUT",
            "/v1/accounts/acct_control_target/status",
            Some(&admin_token),
            Some("account-status-update-0001"),
            Some("account-suspend"),
            Some(json!({"status": "suspended", "expectedRevision": target_revision})),
        ))
        .await
        .unwrap();
    let suspended_status = suspended.status();
    let suspended_body = json_body(suspended).await;
    assert_eq!(suspended_status, StatusCode::OK, "{suspended_body}");
    assert_eq!(suspended_body["status"], "suspended");
    assert_eq!(suspended_body["revision"], target_revision + 1);
    let revoked_session = auth
        .read()
        .unwrap()
        .authenticate_browser(&target_session.raw_session, &target_session.raw_csrf, 2)
        .unwrap_err();
    assert_eq!(revoked_session.code, "session_revoked");

    let stale = app
        .clone()
        .oneshot(request(
            "PUT",
            "/v1/accounts/acct_control_target/status",
            Some(&admin_token),
            Some("account-status-update-0002"),
            Some("account-stale"),
            Some(json!({"status": "active", "expectedRevision": target_revision})),
        ))
        .await
        .unwrap();
    assert_eq!(stale.status(), StatusCode::CONFLICT);

    let inactive = app
        .clone()
        .oneshot(request(
            "GET",
            "/v1/account",
            Some(&target_token),
            None,
            Some("account-inactive"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(inactive.status(), StatusCode::UNAUTHORIZED);

    let denied_team = app
        .clone()
        .oneshot(request(
            "POST",
            "/v1/workspaces/ws_account_team/teams",
            Some(&denied_token),
            Some("team-create-denied-0001"),
            Some("team-create-denied"),
            Some(json!({
                "id": "team_control",
                "workspaceId": "ws_account_team",
                "name": "Control Team",
                "memberIds": []
            })),
        ))
        .await
        .unwrap();
    assert_eq!(denied_team.status(), StatusCode::FORBIDDEN);

    let created = app
        .clone()
        .oneshot(request(
            "POST",
            "/v1/workspaces/ws_account_team/teams",
            Some(&admin_token),
            Some("team-create-control-0001"),
            Some("team-create"),
            Some(json!({
                "id": "team_control",
                "workspaceId": "ws_account_team",
                "name": "Control Team",
                "memberIds": []
            })),
        ))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    assert_eq!(json_body(created).await["revision"], 1);

    let listed = app
        .clone()
        .oneshot(request(
            "GET",
            "/v1/workspaces/ws_account_team/teams",
            Some(&admin_token),
            None,
            Some("team-list"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(listed.status(), StatusCode::OK);
    assert_eq!(json_body(listed).await.as_array().unwrap().len(), 1);

    let member_list = app
        .clone()
        .oneshot(request(
            "GET",
            "/v1/workspaces/ws_account_team/teams",
            Some(&denied_token),
            None,
            Some("team-list-member-hydrated"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(member_list.status(), StatusCode::OK);

    let updated = app
        .clone()
        .oneshot(request(
            "PUT",
            "/v1/workspaces/ws_account_team/teams/team_control",
            Some(&admin_token),
            Some("team-update-control-0001"),
            Some("team-update"),
            Some(json!({"name": "Renamed Team", "expectedRevision": 1})),
        ))
        .await
        .unwrap();
    assert_eq!(updated.status(), StatusCode::OK);
    let updated_body = json_body(updated).await;
    assert_eq!(updated_body["team"]["name"], "Renamed Team");
    assert_eq!(updated_body["revision"], 2);

    let deleted = app
        .clone()
        .oneshot(request(
            "DELETE",
            "/v1/workspaces/ws_account_team/teams/team_control",
            Some(&admin_token),
            Some("team-delete-control-0001"),
            Some("team-delete"),
            Some(json!({"expectedRevision": 2})),
        ))
        .await
        .unwrap();
    assert_eq!(deleted.status(), StatusCode::OK);
    let deleted_body = json_body(deleted).await;
    assert_eq!(deleted_body["team"]["id"], "team_control");
    assert_eq!(deleted_body["revision"], 2);

    let missing = app
        .oneshot(request(
            "GET",
            "/v1/workspaces/ws_account_team/teams/team_control",
            Some(&admin_token),
            None,
            Some("team-missing"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    let _ = fs::remove_dir_all(base);
}

fn request(
    method: &str,
    uri: &str,
    token: Option<&str>,
    idempotency_key: Option<&str>,
    correlation: Option<&str>,
    body: Option<Value>,
) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    if let Some(key) = idempotency_key {
        builder = builder.header("idempotency-key", key);
    }
    if let Some(correlation) = correlation {
        builder = builder.header("x-correlation-id", correlation);
    }
    match body {
        Some(body) => builder
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    }
}

async fn json_body(response: Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 64 * 1024).await.unwrap()).unwrap()
}

fn allow(
    account_id: &AccountId,
    id: &str,
    capability: &str,
    action: Action,
    scope: ResourceScope,
) -> Grant {
    Grant {
        id: GrantId::from_str(id).unwrap(),
        account_id: Some(account_id.clone()),
        member_id: None,
        capability: Capability::new(capability).unwrap(),
        action,
        scope,
        effect: Effect::Allow,
    }
}

#[tokio::test]
async fn control_routes_require_correlation_and_authentication() {
    let base = root("auth");
    let _ = fs::remove_dir_all(&base);
    let state = ApiState::new(ProfileService::new(LocalRepository::open(&base).unwrap()));
    let token = state.issue_token("acct_control_auth").unwrap();
    let app = router(state);

    let missing_correlation = app
        .clone()
        .oneshot(request(
            "GET",
            "/v1/account",
            Some(&token),
            None,
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(missing_correlation.status(), StatusCode::BAD_REQUEST);
    let missing_correlation_id = missing_correlation
        .headers()
        .get("x-correlation-id")
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    assert!(missing_correlation_id.starts_with("cor_"));
    let missing_correlation_body = json_body(missing_correlation).await;
    assert_eq!(missing_correlation_body["code"], "correlation_required");
    assert_eq!(
        missing_correlation_body["correlationId"],
        missing_correlation_id
    );

    let empty_correlation = app
        .clone()
        .oneshot(request(
            "GET",
            "/v1/account",
            Some(&token),
            None,
            Some("   "),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(empty_correlation.status(), StatusCode::BAD_REQUEST);
    let empty_correlation_body = json_body(empty_correlation).await;
    assert_eq!(empty_correlation_body["code"], "correlation_required");

    let missing_auth = app
        .clone()
        .oneshot(request(
            "GET",
            "/v1/account",
            None,
            None,
            Some(CORRELATION),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(missing_auth.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        missing_auth.headers().get("x-correlation-id").unwrap(),
        CORRELATION
    );
    let missing_auth_body = json_body(missing_auth).await;
    assert_eq!(missing_auth_body["code"], "authentication_required");
    assert_eq!(missing_auth_body["correlationId"], CORRELATION);
    assert!(!missing_auth_body.to_string().contains(SECRET_CANARY));

    let malformed_json = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/auth/device/start")
                .header("content-type", "application/json")
                .header("x-correlation-id", CORRELATION)
                .body(Body::from("{"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(malformed_json.status(), StatusCode::BAD_REQUEST);
    let malformed_body = json_body(malformed_json).await;
    assert_eq!(
        malformed_body["type"],
        "https://buglogin.local/problems/request_invalid"
    );
    assert_eq!(malformed_body["code"], "request_invalid");
    assert_eq!(malformed_body["correlationId"], CORRELATION);
    assert_eq!(malformed_body["retryable"], false);
    assert!(malformed_body["fields"].is_object());

    let account = app
        .oneshot(request(
            "GET",
            "/v1/account",
            Some(&token),
            None,
            Some(CORRELATION),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(account.status(), StatusCode::OK);
    assert_eq!(json_body(account).await["accountId"], "acct_control_auth");
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn extension_group_rest_crud_is_workspace_scoped_versioned_and_authorized() {
    let base = root("extension-groups");
    let _ = fs::remove_dir_all(&base);
    let state = ApiState::new(ProfileService::new(LocalRepository::open(&base).unwrap()));
    let token = state.issue_token("acct_extension_groups").unwrap();
    let account_id = AccountId::from_str("acct_extension_groups").unwrap();
    let workspace_id = WorkspaceId::from_str("wsp_extension_groups").unwrap();
    let owner_member_id = MemberId::from_str("mem_extension_groups_owner").unwrap();
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
                account_id: account_id.clone(),
                workspace_id: workspace_id.clone(),
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
                account_id: Some(account_id.clone()),
                ..Principal::default()
            },
            &[],
            &workspace_id,
            &owner_member_id,
            2,
        )
        .unwrap();
    let app = router(state);
    let created = app
        .clone()
        .oneshot(request(
            "POST",
            "/v1/workspaces/wsp_extension_groups/extension-groups",
            Some(&token),
            Some("extension-group-create-1"),
            Some("extension-group-create-correlation"),
            Some(json!({
                "workspaceId": "wsp_extension_groups",
                "extensionGroupId": "extgrp_desktop_test",
                "name": "Desktop test",
                "extensionIds": [],
                "syncMode": "local"
            })),
        ))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    let created = json_body(created).await;
    assert_eq!(created["group"]["name"], "Desktop test");
    assert_eq!(created["revision"], 1);

    let listed = app
        .clone()
        .oneshot(request(
            "GET",
            "/v1/workspaces/wsp_extension_groups/extension-groups",
            Some(&token),
            None,
            Some("extension-group-list-correlation"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(listed.status(), StatusCode::OK);
    let listed = json_body(listed).await;
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert_eq!(listed[0]["group"]["id"], "extgrp_desktop_test");

    let group_path = "/v1/workspaces/wsp_extension_groups/extension-groups/extgrp_desktop_test";
    let loaded = app
        .clone()
        .oneshot(request(
            "GET",
            group_path,
            Some(&token),
            None,
            Some("extension-group-get-correlation"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(loaded.status(), StatusCode::OK);
    assert_eq!(json_body(loaded).await["revision"], 1);

    let updated = app
        .clone()
        .oneshot(request(
            "PUT",
            group_path,
            Some(&token),
            Some("extension-group-update-1"),
            Some("extension-group-update-correlation"),
            Some(json!({
                "workspaceId": "wsp_extension_groups",
                "extensionGroupId": "extgrp_desktop_test",
                "expectedRevision": 1,
                "name": "Renamed desktop group",
                "extensionIds": [],
                "syncMode": "managed"
            })),
        ))
        .await
        .unwrap();
    assert_eq!(updated.status(), StatusCode::OK);
    let updated = json_body(updated).await;
    assert_eq!(updated["group"]["name"], "Renamed desktop group");
    assert_eq!(updated["revision"], 2);

    let stale_update = app
        .clone()
        .oneshot(request(
            "PUT",
            group_path,
            Some(&token),
            Some("extension-group-update-stale-1"),
            Some("extension-group-update-stale-correlation"),
            Some(json!({
                "workspaceId": "wsp_extension_groups",
                "extensionGroupId": "extgrp_desktop_test",
                "expectedRevision": 1,
                "name": "Stale update",
                "extensionIds": [],
                "syncMode": "managed"
            })),
        ))
        .await
        .unwrap();
    assert_eq!(stale_update.status(), StatusCode::CONFLICT);

    let deleted = app
        .clone()
        .oneshot(request(
            "DELETE",
            group_path,
            Some(&token),
            Some("extension-group-delete-1"),
            Some("extension-group-delete-correlation"),
            Some(json!({
                "workspaceId": "wsp_extension_groups",
                "extensionGroupId": "extgrp_desktop_test",
                "expectedRevision": 2
            })),
        ))
        .await
        .unwrap();
    assert_eq!(deleted.status(), StatusCode::OK);

    let deleted_group = app
        .clone()
        .oneshot(request(
            "GET",
            group_path,
            Some(&token),
            None,
            Some("extension-group-get-deleted-correlation"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(deleted_group.status(), StatusCode::NOT_FOUND);

    let cross_workspace = app
        .oneshot(request(
            "GET",
            "/v1/workspaces/wsp_other_workspace/extension-groups",
            Some(&token),
            None,
            Some("extension-group-cross-workspace"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(cross_workspace.status(), StatusCode::FORBIDDEN);
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn control_http_flow_enforces_acl_replay_audit_and_redaction() {
    let base = root("flow");
    let _ = fs::remove_dir_all(&base);
    let state = ApiState::new(ProfileService::new(LocalRepository::open(&base).unwrap()));
    let owner_token = state.issue_token("acct_control_owner").unwrap();
    let denied_token = state.issue_token("acct_control_denied").unwrap();
    let owner = AccountId::from_str("acct_control_owner").unwrap();
    let organization = OrganizationId::from_str("org_control").unwrap();
    let workspace = WorkspaceId::from_str("ws_control").unwrap();
    let resource = ResourceId::from_str("res_control").unwrap();
    let workspace_scope = ResourceScope::workspace(workspace.clone());
    let resource_scope = ResourceScope::resource(workspace.clone(), resource.clone());
    state.grants.write().unwrap().extend([
        allow(
            &owner,
            "grant_org_create",
            "CAP-ORG-001",
            Action::Create,
            ResourceScope {
                account_id: Some(owner.clone()),
                organization_id: Some(organization.clone()),
                ..ResourceScope::default()
            },
        ),
        allow(
            &owner,
            "grant_org_read",
            "CAP-ORG-001",
            Action::Read,
            ResourceScope {
                account_id: Some(owner.clone()),
                organization_id: Some(organization.clone()),
                ..ResourceScope::default()
            },
        ),
        allow(
            &owner,
            "grant_org_update",
            "CAP-ORG-001",
            Action::Update,
            ResourceScope {
                account_id: Some(owner.clone()),
                organization_id: Some(organization.clone()),
                ..ResourceScope::default()
            },
        ),
        allow(
            &owner,
            "grant_ws_create",
            "CAP-WS-001",
            Action::Create,
            workspace_scope.clone(),
        ),
        allow(
            &owner,
            "grant_ws_update",
            "CAP-WS-001",
            Action::Update,
            workspace_scope.clone(),
        ),
        allow(
            &owner,
            "grant_ws_read",
            "CAP-WS-001",
            Action::Read,
            workspace_scope.clone(),
        ),
        allow(
            &owner,
            "grant_mem_create",
            "CAP-MEM-001",
            Action::Create,
            workspace_scope.clone(),
        ),
        allow(
            &owner,
            "grant_mem_read",
            "CAP-MEM-001",
            Action::Read,
            workspace_scope.clone(),
        ),
        allow(
            &owner,
            "grant_res_create",
            "CAP-RES-001",
            Action::Create,
            workspace_scope.clone(),
        ),
        allow(
            &owner,
            "grant_res_read",
            "CAP-RES-001",
            Action::Read,
            resource_scope.clone(),
        ),
        allow(
            &owner,
            "grant_res_manage",
            "CAP-RES-001",
            Action::Manage,
            resource_scope,
        ),
        allow(
            &owner,
            "grant_share_read",
            "CAP-SHARE-001",
            Action::Read,
            ResourceScope::workspace(workspace.clone()),
        ),
        allow(
            &owner,
            "grant_share_manage",
            "CAP-SHARE-001",
            Action::Manage,
            ResourceScope::workspace(workspace.clone()),
        ),
        allow(
            &owner,
            "grant_audit_read",
            "CAP-AUD-001",
            Action::Read,
            workspace_scope,
        ),
    ]);
    let principal = Principal {
        account_id: Some(owner.clone()),
        ..Principal::default()
    };
    let grants = state.grants.read().unwrap().clone();
    state
        .workspaces
        .write()
        .unwrap()
        .add_organization(
            &principal,
            &grants,
            Organization {
                id: organization.clone(),
                account_id: owner.clone(),
                name: "Control organization".into(),
                created_at: "2026-07-20T00:00:00Z".into(),
            },
            OrganizationPolicy {
                max_workspaces: 2,
                offline_entitled: true,
            },
        )
        .unwrap();
    state
        .workspaces
        .write()
        .unwrap()
        .create_workspace(
            &principal,
            &grants,
            &organization,
            Workspace {
                id: workspace.clone(),
                organization_id: organization.clone(),
                name: "Control workspace".into(),
                policy: WorkspacePolicy {
                    max_profiles: 10,
                    max_members: 10,
                    max_storage_bytes: 1_048_576,
                    offline_entitled: true,
                },
                created_at: "2026-07-20T00:00:00Z".into(),
            },
        )
        .unwrap();
    let owner_member_id = MemberId::from_str("mem_control_owner").unwrap();
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
                account_id: owner.clone(),
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
                account_id: Some(owner.clone()),
                ..Principal::default()
            },
            &[],
            &workspace,
            &owner_member_id,
            2,
        )
        .unwrap();
    let resources = std::sync::Arc::clone(&state.resources);
    let app: Router = router(state);

    let workspace_response = app
        .clone()
        .oneshot(request(
            "GET",
            "/v1/organizations/org_control/workspaces/ws_control",
            Some(&owner_token),
            None,
            Some(CORRELATION),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(workspace_response.status(), StatusCode::OK);
    assert_eq!(json_body(workspace_response).await["id"], "ws_control");

    let organization_record = app
        .clone()
        .oneshot(request(
            "GET",
            "/v1/organizations/org_control/record",
            Some(&owner_token),
            None,
            Some("control-http-organization-record"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(organization_record.status(), StatusCode::OK);
    let organization_record = json_body(organization_record).await;
    assert_eq!(organization_record["organization"]["id"], "org_control");
    assert_eq!(organization_record["revision"], 1);

    let workspace_record = app
        .clone()
        .oneshot(request(
            "GET",
            "/v1/organizations/org_control/workspaces/ws_control/record",
            Some(&owner_token),
            None,
            Some("control-http-workspace-record"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(workspace_record.status(), StatusCode::OK);
    let workspace_record = json_body(workspace_record).await;
    assert_eq!(workspace_record["workspace"]["id"], "ws_control");
    assert_eq!(workspace_record["revision"], 1);

    let organization_update = app
        .clone()
        .oneshot(request(
            "PATCH",
            "/v1/organizations/org_control",
            Some(&owner_token),
            Some("control-organization-update-0001"),
            Some("control-http-organization-update"),
            Some(json!({
                "organization": {
                    "id": "org_control",
                    "accountId": "acct_control_owner",
                    "name": "Updated control organization",
                    "createdAt": "2026-07-20T00:00:00Z"
                },
                "policy": { "maxWorkspaces": 3, "offlineEntitled": true },
                "expectedRevision": 1
            })),
        ))
        .await
        .unwrap();
    assert_eq!(organization_update.status(), StatusCode::OK);
    assert_eq!(
        json_body(organization_update).await["name"],
        "Updated control organization"
    );

    let workspace_update = app
        .clone()
        .oneshot(request(
            "PATCH",
            "/v1/organizations/org_control/workspaces/ws_control",
            Some(&owner_token),
            Some("control-workspace-update-0001"),
            Some("control-http-workspace-update"),
            Some(json!({
                "workspace": {
                    "id": "ws_control",
                    "organizationId": "org_control",
                    "name": "Updated control workspace",
                    "policy": {
                        "maxProfiles": 25,
                        "maxMembers": 12,
                        "maxStorageBytes": 2097152,
                        "offlineEntitled": true
                    },
                    "createdAt": "2026-07-20T00:00:00Z"
                },
                "expectedRevision": 1
            })),
        ))
        .await
        .unwrap();
    assert_eq!(workspace_update.status(), StatusCode::OK);
    assert_eq!(
        json_body(workspace_update).await["name"],
        "Updated control workspace"
    );

    let workspace_record = app
        .clone()
        .oneshot(request(
            "GET",
            "/v1/organizations/org_control/workspaces/ws_control/record",
            Some(&owner_token),
            None,
            Some("control-http-workspace-updated-record"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(workspace_record.status(), StatusCode::OK);
    let workspace_record = json_body(workspace_record).await;
    assert_eq!(workspace_record["workspace"]["policy"]["maxProfiles"], 25);
    assert_eq!(workspace_record["revision"], 2);

    let stale_workspace_update = app
        .clone()
        .oneshot(request(
            "PATCH",
            "/v1/organizations/org_control/workspaces/ws_control",
            Some(&owner_token),
            Some("control-workspace-stale-update-01"),
            Some("control-http-workspace-stale-update"),
            Some(json!({
                "workspace": {
                    "id": "ws_control",
                    "organizationId": "org_control",
                    "name": "Stale update",
                    "policy": {
                        "maxProfiles": 10,
                        "maxMembers": 10,
                        "maxStorageBytes": 1048576,
                        "offlineEntitled": true
                    },
                    "createdAt": "2026-07-20T00:00:00Z"
                },
                "expectedRevision": 1
            })),
        ))
        .await
        .unwrap();
    assert_eq!(stale_workspace_update.status(), StatusCode::CONFLICT);

    let denied_workspace_update = app
        .clone()
        .oneshot(request(
            "PATCH",
            "/v1/organizations/org_control/workspaces/ws_control",
            Some(&denied_token),
            Some("control-workspace-denied-update-01"),
            Some("control-http-workspace-denied-update"),
            Some(json!({
                "workspace": {
                    "id": "ws_control",
                    "organizationId": "org_control",
                    "name": "Unauthorized update",
                    "policy": {
                        "maxProfiles": 99,
                        "maxMembers": 99,
                        "maxStorageBytes": 9999999,
                        "offlineEntitled": true
                    },
                    "createdAt": "2026-07-20T00:00:00Z"
                },
                "expectedRevision": 2
            })),
        ))
        .await
        .unwrap();
    assert_eq!(denied_workspace_update.status(), StatusCode::FORBIDDEN);

    let denied_workspace = app
        .clone()
        .oneshot(request(
            "GET",
            "/v1/organizations/org_control/workspaces/ws_control",
            Some(&denied_token),
            None,
            Some("control-http-workspace-denied"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(denied_workspace.status(), StatusCode::FORBIDDEN);

    let denied_workspace_record = app
        .clone()
        .oneshot(request(
            "GET",
            "/v1/organizations/org_control/workspaces/ws_control/record",
            Some(&denied_token),
            None,
            Some("control-http-workspace-record-denied"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(denied_workspace_record.status(), StatusCode::FORBIDDEN);

    let denied_member = app
        .clone()
        .oneshot(request(
            "POST",
            "/v1/workspaces/ws_control/members",
            Some(&denied_token),
            Some("control-member-denied-0001"),
            Some("control-http-denied"),
            Some(json!({
                "id": "mem_denied",
                "accountId": "acct_member_denied",
                "workspaceId": "ws_control",
                "teamId": null,
                "role": "member",
                "status": "invited",
                "inviteExpiresAt": 4102444800_u64
            })),
        ))
        .await
        .unwrap();
    assert_eq!(denied_member.status(), StatusCode::FORBIDDEN);

    let denied_member_page = app
        .clone()
        .oneshot(request(
            "GET",
            "/v1/workspaces/ws_control/members/page?limit=1",
            Some(&denied_token),
            None,
            Some("control-http-member-page-denied"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(denied_member_page.status(), StatusCode::FORBIDDEN);

    let member_body = json!({
        "id": "mem_control",
        "accountId": "acct_member_control",
        "workspaceId": "ws_control",
        "teamId": null,
        "role": "member",
        "status": "invited",
        "inviteExpiresAt": 4102444800_u64
    });
    let member = app
        .clone()
        .oneshot(request(
            "POST",
            "/v1/workspaces/ws_control/members",
            Some(&owner_token),
            Some("control-member-create-0001"),
            Some("control-http-member"),
            Some(member_body.clone()),
        ))
        .await
        .unwrap();
    assert_eq!(member.status(), StatusCode::OK);
    let member_result = json_body(member).await;
    let member_replay = app
        .clone()
        .oneshot(request(
            "POST",
            "/v1/workspaces/ws_control/members",
            Some(&owner_token),
            Some("control-member-create-0001"),
            Some("control-http-replay"),
            Some(member_body),
        ))
        .await
        .unwrap();
    assert_eq!(member_replay.status(), StatusCode::OK);
    assert_eq!(json_body(member_replay).await, member_result);

    let member_two = app
        .clone()
        .oneshot(request(
            "POST",
            "/v1/workspaces/ws_control/members",
            Some(&owner_token),
            Some("control-member-create-0002"),
            Some("control-http-member-two"),
            Some(json!({
                "id": "mem_control2",
                "accountId": "acct_member_control2",
                "workspaceId": "ws_control",
                "teamId": null,
                "role": "member",
                "status": "invited",
                "inviteExpiresAt": 4102444800_u64
            })),
        ))
        .await
        .unwrap();
    assert_eq!(member_two.status(), StatusCode::OK);

    let registered = app
        .clone()
        .oneshot(request(
            "POST",
            "/v1/workspaces/ws_control/resources",
            Some(&owner_token),
            Some("control-resource-create-0001"),
            Some("control-http-resource"),
            Some(json!({
                "id": "res_control",
                "workspaceId": "ws_control",
                "kind": "profile",
                "state": "active",
                "displayName": "Control resource"
            })),
        ))
        .await
        .unwrap();
    assert_eq!(registered.status(), StatusCode::OK);

    let registered_two = app
        .clone()
        .oneshot(request(
            "POST",
            "/v1/workspaces/ws_control/resources",
            Some(&owner_token),
            Some("control-resource-create-0002"),
            Some("control-http-resource-two"),
            Some(json!({
                "id": "res_control2",
                "workspaceId": "ws_control",
                "kind": "profile",
                "state": "active",
                "displayName": "Control resource 2"
            })),
        ))
        .await
        .unwrap();
    assert_eq!(registered_two.status(), StatusCode::OK);

    {
        let mut service = resources.write().unwrap();
        for (id, resource_id, member_id) in [
            ("rg_control_a", "res_control", "mem_control"),
            ("rg_control_b", "res_control2", "mem_control2"),
        ] {
            service
                .grant(
                    &principal,
                    &grants,
                    buglogin_domain::grant::ResourceGrant {
                        id: GrantId::from_str(id).unwrap(),
                        workspace_id: workspace.clone(),
                        resource_id: ResourceId::from_str(resource_id).unwrap(),
                        member_id: MemberId::from_str(member_id).unwrap(),
                        action: Action::Read,
                        effect: Effect::Allow,
                    },
                )
                .unwrap();
        }
    }

    let member_page = app
        .clone()
        .oneshot(request(
            "GET",
            "/v1/workspaces/ws_control/members/page?limit=1",
            Some(&owner_token),
            None,
            Some("control-http-members-page"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(member_page.status(), StatusCode::OK);
    let member_page = json_body(member_page).await;
    assert_eq!(member_page["items"].as_array().unwrap().len(), 1);
    assert_eq!(member_page["items"][0]["id"], "mem_control");
    assert_eq!(member_page["nextCursor"], "mem_control");

    let member_page_two = app
        .clone()
        .oneshot(request(
            "GET",
            "/v1/workspaces/ws_control/members/page?cursor=mem_control&limit=1",
            Some(&owner_token),
            None,
            Some("control-http-members-page-two"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(member_page_two.status(), StatusCode::OK);
    let member_page_two = json_body(member_page_two).await;
    assert_eq!(member_page_two["items"][0]["id"], "mem_control2");
    assert_eq!(member_page_two["nextCursor"], "mem_control2");

    let member_page_three = app
        .clone()
        .oneshot(request(
            "GET",
            "/v1/workspaces/ws_control/members/page?cursor=mem_control2&limit=1",
            Some(&owner_token),
            None,
            Some("control-http-members-page-three"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(member_page_three.status(), StatusCode::OK);
    let member_page_three = json_body(member_page_three).await;
    assert_eq!(member_page_three["items"][0]["id"], "mem_control_owner");
    assert_eq!(member_page_three["nextCursor"], Value::Null);

    let resource_page = app
        .clone()
        .oneshot(request(
            "GET",
            "/v1/workspaces/ws_control/resources/page?limit=1",
            Some(&owner_token),
            None,
            Some("control-http-resources-page"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resource_page.status(), StatusCode::OK);
    let resource_page = json_body(resource_page).await;
    assert_eq!(resource_page["items"].as_array().unwrap().len(), 1);
    assert_eq!(resource_page["items"][0]["id"], "res_control");
    assert_eq!(resource_page["nextCursor"], "res_control");

    let resource_page_two = app
        .clone()
        .oneshot(request(
            "GET",
            "/v1/workspaces/ws_control/resources/page?cursor=res_control&limit=1",
            Some(&owner_token),
            None,
            Some("control-http-resources-page-two"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resource_page_two.status(), StatusCode::OK);
    let resource_page_two = json_body(resource_page_two).await;
    assert_eq!(resource_page_two["items"][0]["id"], "res_control2");
    assert_eq!(resource_page_two["nextCursor"], Value::Null);

    let grant_page = app
        .clone()
        .oneshot(request(
            "GET",
            "/v1/workspaces/ws_control/grants/page?limit=1",
            Some(&owner_token),
            None,
            Some("control-http-grants-page"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(grant_page.status(), StatusCode::OK);
    let grant_page = json_body(grant_page).await;
    assert_eq!(grant_page["items"].as_array().unwrap().len(), 1);
    assert_eq!(grant_page["items"][0]["id"], "rg_control_a");
    assert_eq!(grant_page["items"][0]["workspaceId"], "ws_control");
    assert_eq!(grant_page["nextCursor"], "rg_control_a");

    let grant_page_two = app
        .clone()
        .oneshot(request(
            "GET",
            "/v1/workspaces/ws_control/grants/page?cursor=rg_control_a&limit=1",
            Some(&owner_token),
            None,
            Some("control-http-grants-page-two"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(grant_page_two.status(), StatusCode::OK);
    let grant_page_two = json_body(grant_page_two).await;
    assert_eq!(grant_page_two["items"][0]["id"], "rg_control_b");
    assert_eq!(grant_page_two["nextCursor"], Value::Null);

    let denied_grant_page = app
        .clone()
        .oneshot(request(
            "GET",
            "/v1/workspaces/ws_control/grants/page?limit=1",
            Some(&denied_token),
            None,
            Some("control-http-grants-page-denied"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(denied_grant_page.status(), StatusCode::FORBIDDEN);

    let invalid_grant_page_limit = app
        .clone()
        .oneshot(request(
            "GET",
            "/v1/workspaces/ws_control/grants/page?limit=101",
            Some(&owner_token),
            None,
            Some("control-http-grants-invalid-limit"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(invalid_grant_page_limit.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        json_body(invalid_grant_page_limit).await["code"],
        "invalid_page_limit"
    );

    let legacy_grants = app
        .clone()
        .oneshot(request(
            "GET",
            "/v1/workspaces/ws_control/grants?resourceId=res_control",
            Some(&owner_token),
            None,
            Some("control-http-grants-legacy"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(legacy_grants.status(), StatusCode::OK);
    let legacy_grants = json_body(legacy_grants).await;
    assert!(
        legacy_grants.is_array(),
        "legacy route must retain array shape"
    );
    assert_eq!(legacy_grants.as_array().unwrap().len(), 1);
    assert_eq!(legacy_grants[0]["id"], "rg_control_a");

    let legacy_members = app
        .clone()
        .oneshot(request(
            "GET",
            "/v1/workspaces/ws_control/members",
            Some(&owner_token),
            None,
            Some("control-http-members-legacy"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(legacy_members.status(), StatusCode::OK);
    assert_eq!(json_body(legacy_members).await.as_array().unwrap().len(), 3);

    let invalid_page_limit = app
        .clone()
        .oneshot(request(
            "GET",
            "/v1/workspaces/ws_control/resources/page?limit=101",
            Some(&owner_token),
            None,
            Some("control-http-resources-invalid-limit"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(invalid_page_limit.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        json_body(invalid_page_limit).await["code"],
        "invalid_page_limit"
    );

    let denied_resource = app
        .clone()
        .oneshot(request(
            "GET",
            "/v1/workspaces/ws_control/resources/res_control",
            Some(&denied_token),
            None,
            Some("control-http-resource-denied"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(denied_resource.status(), StatusCode::FORBIDDEN);

    let denied_resource_page = app
        .clone()
        .oneshot(request(
            "GET",
            "/v1/workspaces/ws_control/resources/page?limit=1",
            Some(&denied_token),
            None,
            Some("control-http-resource-page-denied"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(denied_resource_page.status(), StatusCode::FORBIDDEN);

    let audit = app
        .clone()
        .oneshot(request(
            "GET",
            "/v1/workspaces/ws_control/audit-events",
            Some(&owner_token),
            None,
            Some("control-http-audit"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(audit.status(), StatusCode::OK);
    let audit_body = json_body(audit).await;
    let events = audit_body["items"].as_array().unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| {
                event["commandId"] == "member.invite"
                    && event["correlationId"] == "control-http-member"
                    && event["outcome"] == "succeeded"
            })
            .count(),
        1
    );
    assert!(events.iter().any(|event| {
        event["commandId"] == "member.invite"
            && event["correlationId"] == "control-http-denied"
            && event["outcome"] == "denied"
    }));
    assert!(events.iter().any(|event| {
        event["commandId"] == "resource.register"
            && event["correlationId"] == "control-http-resource"
            && event["outcome"] == "succeeded"
    }));
    let evidence = audit_body.to_string();
    for secret in [
        SECRET_CANARY,
        owner_token.as_str(),
        denied_token.as_str(),
        "authorization",
        "control-resource-create-0001",
    ] {
        assert!(!evidence.contains(secret), "audit leaked {secret}");
    }

    let denied_audit = app
        .clone()
        .oneshot(request(
            "GET",
            "/v1/workspaces/ws_control/audit-events",
            Some(&denied_token),
            None,
            Some("control-http-audit-denied"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(denied_audit.status(), StatusCode::FORBIDDEN);

    let first_page = app
        .clone()
        .oneshot(request(
            "GET",
            "/v1/workspaces/ws_control/audit-events?limit=1",
            Some(&owner_token),
            None,
            Some("control-http-audit-page-1"),
            None,
        ))
        .await
        .unwrap();
    let first_page = json_body(first_page).await;
    let first_item = first_page["items"][0]["eventId"].as_str().unwrap();
    let cursor = first_page["nextCursor"].as_str().unwrap();
    assert!(cursor.ends_with(first_item));

    let next_page = app
        .clone()
        .oneshot(request(
            "GET",
            &format!("/v1/workspaces/ws_control/audit-events?limit=1&cursor={cursor}"),
            Some(&owner_token),
            None,
            Some("control-http-audit-page-2"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(next_page.status(), StatusCode::OK);
    assert_ne!(
        json_body(next_page).await["items"][0]["eventId"],
        first_item
    );

    let invalid_page = app
        .oneshot(request(
            "GET",
            "/v1/workspaces/ws_control/audit-events?limit=101",
            Some(&owner_token),
            None,
            Some("control-http-audit-invalid-limit"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(invalid_page.status(), StatusCode::BAD_REQUEST);
    let _ = fs::remove_dir_all(base);
}
