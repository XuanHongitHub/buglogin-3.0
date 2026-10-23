use axum::{body::Body, http::Request, http::StatusCode};
use buglogin_api::http::{ApiState, FirstOwnerBootstrapInput, router};
use buglogin_browser::profile::ProfileService;
use buglogin_storage::control::MemoryControlRecordStore;
use serde_json::{Value, json};
use tower::ServiceExt;
use zeroize::Zeroizing;

fn input(email: &str) -> FirstOwnerBootstrapInput {
    FirstOwnerBootstrapInput {
        account_id: None,
        username: "First.Owner".into(),
        email: email.into(),
        operator_attested_email: true,
        password: Zeroizing::new("initial owner password 2026".into()),
        organization_name: "BugLogin Organization".into(),
        workspace_name: "Default Workspace".into(),
        max_workspaces: 10,
        max_profiles: 1_000,
        max_members: 100,
        max_storage_bytes: 10 * 1024 * 1024 * 1024,
    }
}

#[tokio::test]
async fn first_owner_bootstrap_persists_login_admin_and_a_default_workspace() {
    let store = MemoryControlRecordStore::default();
    let state =
        ApiState::hosted_with_control_store(ProfileService::disabled(), store.clone()).unwrap();
    let provisioned = state
        .bootstrap_first_owner(input("owner@example.com"))
        .unwrap();
    assert!(provisioned.created_account);
    assert!(provisioned.created_organization);
    assert!(provisioned.created_workspace);

    let reopened =
        ApiState::hosted_with_control_store(ProfileService::disabled(), store.reopen()).unwrap();
    let app = router(reopened.clone());
    let login = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/auth/login")
                .header("content-type", "application/json")
                .header("origin", "http://127.0.0.1:3000")
                .header("x-correlation-id", "first-owner-login-test")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "identifier": "first.owner",
                        "password": "initial owner password 2026"
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(login.status(), StatusCode::OK);

    let token = reopened
        .issue_token(provisioned.account_id.as_str())
        .unwrap();
    let organizations = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/organizations")
                .header("authorization", format!("Bearer {token}"))
                .header("x-correlation-id", "first-owner-org-list-test")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(organizations.status(), StatusCode::OK);
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(organizations.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(body.as_array().unwrap().len(), 1);
    assert_eq!(body[0]["id"], provisioned.organization_id.as_str());
}

#[tokio::test]
async fn platform_admin_can_manage_orgs_and_workspaces_outside_membership_scope() {
    let store = MemoryControlRecordStore::default();
    let state = ApiState::hosted_with_control_store(ProfileService::disabled(), store).unwrap();
    let provisioned = state
        .bootstrap_first_owner(input("platform-admin-scope@example.com"))
        .unwrap();
    let login = router(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/auth/desktop/login")
                .header("content-type", "application/json")
                .header("x-correlation-id", "platform-admin-scope-login")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "identifier": "platform-admin-scope@example.com",
                        "password": "initial owner password 2026",
                        "deviceId": "platform-admin-scope-device"
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    if login.status() != StatusCode::OK {
        panic!("desktop login failed: {}", json_body(login).await);
    }
    let token_body = json_body(login).await;
    let token = token_body["accessToken"].as_str().unwrap().to_owned();
    let app = router(state);

    let organization = json!({
        "organization": {
            "id": "org_platform_admin_scope",
            "accountId": provisioned.account_id.as_str(),
            "name": "Platform Admin Managed Organization",
            "createdAt": "2026-10-21T00:00:00Z"
        },
        "policy": { "maxWorkspaces": 2, "offlineEntitled": false }
    });
    let created_organization = app
        .clone()
        .oneshot(admin_request(
            "POST",
            "/v1/organizations",
            &token,
            "platform-admin-org-create-1",
            "platform-admin-org-create",
            Some(organization),
        ))
        .await
        .unwrap();
    assert_eq!(created_organization.status(), StatusCode::OK);

    let workspace = json!({
        "id": "ws_platform_admin_scope",
        "organizationId": "org_platform_admin_scope",
        "name": "Platform Admin Managed Workspace",
        "policy": {
            "maxProfiles": 20,
            "maxMembers": 5,
            "maxStorageBytes": 1048576,
            "offlineEntitled": false
        },
        "createdAt": "2026-10-21T00:00:00Z"
    });
    let created_workspace = app
        .clone()
        .oneshot(admin_request(
            "POST",
            "/v1/organizations/org_platform_admin_scope/workspaces",
            &token,
            "platform-admin-ws-create-1",
            "platform-admin-ws-create",
            Some(workspace.clone()),
        ))
        .await
        .unwrap();
    assert_eq!(created_workspace.status(), StatusCode::OK);

    let organization_record = app
        .clone()
        .oneshot(admin_request(
            "GET",
            "/v1/organizations/org_platform_admin_scope/record",
            &token,
            "",
            "platform-admin-org-read",
            None,
        ))
        .await
        .unwrap();
    assert_eq!(organization_record.status(), StatusCode::OK);
    assert_eq!(json_body(organization_record).await["revision"], 1);

    let organization_update = app
        .clone()
        .oneshot(admin_request(
            "PATCH",
            "/v1/organizations/org_platform_admin_scope",
            &token,
            "platform-admin-org-update-1",
            "platform-admin-org-update",
            Some(json!({
                "organization": {
                    "id": "org_platform_admin_scope",
                    "accountId": provisioned.account_id.as_str(),
                    "name": "Updated Platform Admin Organization",
                    "createdAt": "2026-10-21T00:00:00Z"
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
        "Updated Platform Admin Organization"
    );

    let workspace_update = app
        .clone()
        .oneshot(admin_request(
            "PATCH",
            "/v1/organizations/org_platform_admin_scope/workspaces/ws_platform_admin_scope",
            &token,
            "platform-admin-ws-update-1",
            "platform-admin-ws-update",
            Some(json!({
                "workspace": {
                    "id": "ws_platform_admin_scope",
                    "organizationId": "org_platform_admin_scope",
                    "name": "Updated Platform Admin Workspace",
                    "policy": {
                        "maxProfiles": 30,
                        "maxMembers": 8,
                        "maxStorageBytes": 2097152,
                        "offlineEntitled": false
                    },
                    "createdAt": "2026-10-21T00:00:00Z"
                },
                "expectedRevision": 1
            })),
        ))
        .await
        .unwrap();
    assert_eq!(workspace_update.status(), StatusCode::OK);
    assert_eq!(
        json_body(workspace_update).await["policy"]["maxProfiles"],
        30
    );
}

#[tokio::test]
async fn owner_can_manage_workspace_member_lifecycle_over_authenticated_http() {
    let store = MemoryControlRecordStore::default();
    let state = ApiState::hosted_with_control_store(ProfileService::disabled(), store).unwrap();
    let provisioned = state
        .bootstrap_first_owner(input("owner-member-lifecycle@example.com"))
        .unwrap();
    let login = router(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/auth/desktop/login")
                .header("content-type", "application/json")
                .header("x-correlation-id", "owner-member-lifecycle-login")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "identifier": "owner-member-lifecycle@example.com",
                        "password": "initial owner password 2026",
                        "deviceId": "owner-member-lifecycle-device"
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(login.status(), StatusCode::OK);
    let token = json_body(login).await["accessToken"]
        .as_str()
        .unwrap()
        .to_owned();
    let app = router(state);
    let workspace_id = provisioned.workspace_id.as_str();
    let member_id = "mem_owner_lifecycle";
    let member_path = format!("/v1/workspaces/{workspace_id}/members/{member_id}");

    let invited = app
        .clone()
        .oneshot(admin_request(
            "POST",
            &format!("/v1/workspaces/{workspace_id}/members"),
            &token,
            "owner-member-lifecycle-create",
            "owner-member-lifecycle-create",
            Some(json!({
                "id": member_id,
                "accountId": "acct_member_lifecycle",
                "workspaceId": workspace_id,
                "teamId": null,
                "role": "member",
                "status": "invited",
                "inviteExpiresAt": 4102444800_u64
            })),
        ))
        .await
        .unwrap();
    assert_eq!(
        invited.status(),
        StatusCode::OK,
        "{}",
        json_body(invited).await
    );

    let activated = app
        .clone()
        .oneshot(admin_request(
            "PUT",
            &format!("{member_path}/activate"),
            &token,
            "owner-member-lifecycle-activate",
            "owner-member-lifecycle-activate",
            None,
        ))
        .await
        .unwrap();
    assert_eq!(activated.status(), StatusCode::OK);
    assert_eq!(json_body(activated).await["status"], "active");

    let role_updated = app
        .clone()
        .oneshot(admin_request(
            "PUT",
            &format!("{member_path}/role"),
            &token,
            "owner-member-lifecycle-role",
            "owner-member-lifecycle-role",
            Some(json!({ "role": "admin" })),
        ))
        .await
        .unwrap();
    assert_eq!(role_updated.status(), StatusCode::OK);
    assert_eq!(json_body(role_updated).await["role"], "admin");

    let suspended = app
        .clone()
        .oneshot(admin_request(
            "PUT",
            &format!("{member_path}/suspend"),
            &token,
            "owner-member-lifecycle-suspend",
            "owner-member-lifecycle-suspend",
            None,
        ))
        .await
        .unwrap();
    assert_eq!(suspended.status(), StatusCode::OK);

    let member = app
        .oneshot(admin_request(
            "GET",
            &member_path,
            &token,
            "",
            "owner-member-lifecycle-read",
            None,
        ))
        .await
        .unwrap();
    assert_eq!(member.status(), StatusCode::OK);
    let member = json_body(member).await;
    assert_eq!(member["role"], "admin");
    assert_eq!(member["status"], "suspended");
}

fn admin_request(
    method: &str,
    uri: &str,
    token: &str,
    idempotency_key: &str,
    correlation_id: &str,
    body: Option<Value>,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .header("x-correlation-id", correlation_id);
    if !idempotency_key.is_empty() {
        builder = builder.header("idempotency-key", idempotency_key);
    }
    match body {
        Some(body) => builder
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    }
}

async fn json_body(response: axum::response::Response) -> Value {
    serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap()
}

#[tokio::test]
async fn operator_password_reset_updates_active_admin_credentials_without_dropping_data() {
    let store = MemoryControlRecordStore::default();
    let state =
        ApiState::hosted_with_control_store(ProfileService::disabled(), store.clone()).unwrap();
    let owner = state
        .bootstrap_first_owner(input("owner-reset@example.com"))
        .unwrap();

    let reset = state
        .operator_reset_platform_admin_password(
            &owner.account_id,
            "replacement owner password 2026",
        )
        .unwrap();

    assert_eq!(reset.account_id, owner.account_id);
    let reopened =
        ApiState::hosted_with_control_store(ProfileService::disabled(), store.reopen()).unwrap();
    let app = router(reopened);
    let old_password = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/auth/login")
                .header("content-type", "application/json")
                .header("origin", "http://127.0.0.1:3000")
                .header("x-correlation-id", "operator-reset-old-password")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "identifier": "first.owner",
                        "password": "initial owner password 2026"
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(old_password.status(), StatusCode::UNAUTHORIZED);
    let new_password = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/auth/login")
                .header("content-type", "application/json")
                .header("origin", "http://127.0.0.1:3000")
                .header("x-correlation-id", "operator-reset-new-password")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "identifier": "first.owner",
                        "password": "replacement owner password 2026"
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(new_password.status(), StatusCode::OK);
}

#[test]
fn first_owner_bootstrap_is_one_time_and_rejects_existing_identities() {
    let store = MemoryControlRecordStore::default();
    let state = ApiState::hosted_with_control_store(ProfileService::disabled(), store).unwrap();
    state
        .bootstrap_first_owner(input("owner@example.com"))
        .unwrap();
    let error = state
        .bootstrap_first_owner(input("second@example.com"))
        .unwrap_err();
    assert_eq!(error.code, "initial_owner_already_bootstrapped");
}
