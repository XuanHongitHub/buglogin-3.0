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
    permissions::{Action, Capability, Effect, Grant, Principal, ResourceScope},
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
    let admin = AccountId::from_str("acct_control_admin").unwrap();
    let workspace = WorkspaceId::from_str("ws_account_team").unwrap();
    state.grants.write().unwrap().extend([
        allow(
            &admin,
            "grant_account_list",
            "CAP-ID-001",
            Action::Read,
            ResourceScope::global(),
        ),
        allow(
            &admin,
            "grant_account_manage",
            "CAP-ID-001",
            Action::Manage,
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
            ResourceScope::workspace(workspace),
        ),
        allow(
            &admin,
            "grant_member_create_for_hydration",
            "CAP-MEM-001",
            Action::Create,
            ResourceScope::workspace(WorkspaceId::from_str("ws_account_team").unwrap()),
        ),
    ]);
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
    assert_eq!(json_body(accounts).await.as_array().unwrap().len(), 3);

    let suspended = app
        .clone()
        .oneshot(request(
            "PUT",
            "/v1/accounts/acct_control_target/status",
            Some(&admin_token),
            Some("account-status-update-0001"),
            Some("account-suspend"),
            Some(json!({"status": "suspended", "expectedRevision": 1})),
        ))
        .await
        .unwrap();
    assert_eq!(suspended.status(), StatusCode::OK);
    let suspended_body = json_body(suspended).await;
    assert_eq!(suspended_body["status"], "suspended");
    assert_eq!(suspended_body["revision"], 2);

    let stale = app
        .clone()
        .oneshot(request(
            "PUT",
            "/v1/accounts/acct_control_target/status",
            Some(&admin_token),
            Some("account-status-update-0002"),
            Some("account-stale"),
            Some(json!({"status": "active", "expectedRevision": 1})),
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
    assert_eq!(inactive.status(), StatusCode::FORBIDDEN);

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
    let missing_auth_body = json_body(missing_auth).await;
    assert_eq!(missing_auth_body["code"], "authentication_required");
    assert!(!missing_auth_body.to_string().contains(SECRET_CANARY));

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
            "grant_ws_create",
            "CAP-WS-001",
            Action::Create,
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
