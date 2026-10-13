use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use buglogin_api::http::{ApiState, router};
use buglogin_browser::profile::ProfileService;
use buglogin_domain::{
    ids::{AccountId, GrantId, ProfileId, ResourceId, WorkspaceId},
    permissions::{Action, Capability, Effect, Grant, ResourceScope},
    profile::{ProfileArchiveState, ProfileResource},
};
use serde_json::{Value, json};
use tower::ServiceExt;

const DEST_PROFILE: &str = "prf_cookie_destination";
const DEST_WORKSPACE: &str = "ws_cookie_destination";
const DEST_RESOURCE: &str = "res_cookie_destination";

fn binding(profile_id: &str, workspace_id: &str, resource_id: &str) -> ProfileResource {
    ProfileResource {
        profile_id: ProfileId::from_str(profile_id).unwrap(),
        resource_id: ResourceId::from_str(resource_id).unwrap(),
        workspace_id: WorkspaceId::from_str(workspace_id).unwrap(),
        archive_state: ProfileArchiveState::Active,
    }
}

fn allow(account: &str, id: &str, capability: &str, action: Action, scope: ResourceScope) -> Grant {
    Grant {
        id: GrantId::from_str(id).unwrap(),
        account_id: Some(AccountId::from_str(account).unwrap()),
        member_id: None,
        capability: Capability::new(capability).unwrap(),
        action,
        scope,
        effect: Effect::Allow,
    }
}

fn request(token: &str, key: &str, body: Value) -> Request<Body> {
    api_request(
        "POST",
        "/v1/cookie-jobs",
        Some(token),
        Some(key),
        Some(body),
    )
}

fn api_request(
    method: &str,
    uri: &str,
    token: Option<&str>,
    key: Option<&str>,
    body: Option<Value>,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("x-correlation-id", "cookie-job-http-test");
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
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

async fn body(response: axum::response::Response) -> Value {
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn import_input() -> Value {
    json!({
        "commandId": "cookie.import",
        "workspaceId": DEST_WORKSPACE,
        "deviceId": "device_cookie_owner",
        "profileId": DEST_PROFILE,
        "expectedGeneration": 4,
        "stagedImportRef": "opaque_stage_1"
    })
}

#[tokio::test]
async fn enqueue_derives_actor_key_and_expiry_and_replays_one_authorized_job() {
    let account = "acct_cookie_owner";
    let state = ApiState::new(ProfileService::disabled()).with_profile_resource(binding(
        DEST_PROFILE,
        DEST_WORKSPACE,
        DEST_RESOURCE,
    ));
    let owner_token = state.issue_token(account).unwrap();
    let denied_token = state.issue_token("acct_cookie_denied").unwrap();
    state.grants.write().unwrap().push(allow(
        account,
        "grant_cookie_import_create",
        "cookie.import",
        Action::Create,
        ResourceScope::resource(
            WorkspaceId::from_str(DEST_WORKSPACE).unwrap(),
            ResourceId::from_str(DEST_RESOURCE).unwrap(),
        ),
    ));
    let app = router(state);
    let denied = app
        .clone()
        .oneshot(request(
            &denied_token,
            "cookie-job-denied-request-0001",
            import_input(),
        ))
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);

    let key = "cookie-job-owner-request-000001";
    let accepted = app
        .clone()
        .oneshot(request(&owner_token, key, import_input()))
        .await
        .unwrap();
    assert_eq!(accepted.status(), StatusCode::ACCEPTED);
    let job = body(accepted).await;
    assert_eq!(job["requestedBy"], account);
    assert_ne!(job["idempotencyKey"], key);
    assert_eq!(job["state"], "accepted");
    assert_eq!(
        job["expiresAt"].as_u64().unwrap() - job["createdAt"].as_u64().unwrap(),
        600
    );

    let replay = app
        .clone()
        .oneshot(request(&owner_token, key, import_input()))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::ACCEPTED);
    assert_eq!(body(replay).await["jobId"], job["jobId"]);

    let mut injected_server_fields = import_input();
    injected_server_fields["requestedBy"] = json!("acct_cookie_denied");
    injected_server_fields["createdAt"] = json!(1);
    injected_server_fields["expiresAt"] = json!(u64::MAX);
    injected_server_fields["idempotencyKey"] = json!("attacker-controlled-key");
    let rejected = app
        .oneshot(request(
            &owner_token,
            "cookie-job-forged-request-00001",
            injected_server_fields,
        ))
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        rejected.headers()[axum::http::header::CONTENT_TYPE],
        "application/problem+json"
    );
    assert_eq!(body(rejected).await["code"], "invalid_input");
}

#[tokio::test]
async fn copy_requires_source_binding_and_read_acl_independently() {
    let account = "acct_cookie_copy";
    let source_profile = "prf_cookie_source";
    let source_workspace = "ws_cookie_source";
    let source_resource = "res_cookie_source";
    let state = ApiState::new(ProfileService::disabled())
        .with_profile_resource(binding(DEST_PROFILE, DEST_WORKSPACE, DEST_RESOURCE))
        .with_profile_resource(binding(source_profile, source_workspace, source_resource));
    let token = state.issue_token(account).unwrap();
    state.grants.write().unwrap().push(allow(
        account,
        "grant_cookie_copy_create",
        "cookie.copy",
        Action::Create,
        ResourceScope::resource(
            WorkspaceId::from_str(DEST_WORKSPACE).unwrap(),
            ResourceId::from_str(DEST_RESOURCE).unwrap(),
        ),
    ));
    let app = router(state);
    let rejected = app
        .oneshot(request(
            &token,
            "cookie-copy-source-denied-0001",
            json!({
                "commandId": "cookie.copy",
                "workspaceId": DEST_WORKSPACE,
                "deviceId": "device_cookie_owner",
                "profileId": DEST_PROFILE,
                "expectedGeneration": 4,
                "sourceWorkspaceId": source_workspace,
                "sourceProfileId": source_profile,
                "sourceDeviceId": "device_cookie_source"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn device_bearer_is_required_for_cookie_job_claim_ack_and_receipt() {
    let requester = "acct_cookie_requester";
    let executor = "acct_cookie_executor";
    let state = ApiState::new(ProfileService::disabled()).with_profile_resource(binding(
        DEST_PROFILE,
        DEST_WORKSPACE,
        DEST_RESOURCE,
    ));
    let requester_token = state.issue_token(requester).unwrap();
    let executor_session_token = state.issue_token(executor).unwrap();
    state.grants.write().unwrap().push(allow(
        requester,
        "grant_cookie_requester_import",
        "cookie.import",
        Action::Create,
        ResourceScope::resource(
            WorkspaceId::from_str(DEST_WORKSPACE).unwrap(),
            ResourceId::from_str(DEST_RESOURCE).unwrap(),
        ),
    ));
    let app = router(state.clone());

    for (token, expected_status) in [
        (None, StatusCode::UNAUTHORIZED),
        (Some(executor_session_token.as_str()), StatusCode::FORBIDDEN),
    ] {
        let inbox = app
            .clone()
            .oneshot(api_request(
                "GET",
                &format!("/v1/workspaces/{DEST_WORKSPACE}/cookie-jobs/inbox"),
                token,
                None,
                None,
            ))
            .await
            .unwrap();
        assert_eq!(inbox.status(), expected_status);
    }

    let started = app
        .clone()
        .oneshot(api_request(
            "POST",
            "/v1/auth/device/start",
            None,
            None,
            Some(json!({"deviceId":"device_cookie_owner","clientName":"BugLogin Desktop"})),
        ))
        .await
        .unwrap();
    assert_eq!(started.status(), StatusCode::OK);
    let started_body = body(started).await;
    let device_code = started_body["deviceCode"].as_str().unwrap();
    let approved = app
        .clone()
        .oneshot(api_request(
            "POST",
            "/v1/auth/device/approve",
            Some(&executor_session_token),
            None,
            Some(json!({"userCode":started_body["userCode"]})),
        ))
        .await
        .unwrap();
    assert_eq!(approved.status(), StatusCode::OK);
    let device_token_response = app
        .clone()
        .oneshot(api_request(
            "POST",
            "/v1/auth/device/token",
            None,
            None,
            Some(json!({"deviceCode":device_code})),
        ))
        .await
        .unwrap();
    assert_eq!(device_token_response.status(), StatusCode::OK);
    let device_token = body(device_token_response).await["accessToken"]
        .as_str()
        .unwrap()
        .to_owned();

    let created = app
        .clone()
        .oneshot(request(
            &requester_token,
            "cookie-device-job-idempotency-001",
            import_input(),
        ))
        .await
        .unwrap();
    let job = body(created).await;
    let job_id = job["jobId"].as_str().unwrap();
    let query = format!("?workspaceId={DEST_WORKSPACE}");
    let inbox = app
        .clone()
        .oneshot(api_request(
            "GET",
            &format!("/v1/workspaces/{DEST_WORKSPACE}/cookie-jobs/inbox"),
            Some(&device_token),
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(inbox.status(), StatusCode::OK);
    assert_eq!(body(inbox).await["items"], json!([]));

    let claim = app
        .clone()
        .oneshot(api_request(
            "POST",
            &format!("/v1/cookie-jobs/{job_id}/claim{query}"),
            Some(&device_token),
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(claim.status(), StatusCode::FORBIDDEN);

    // A device belonging to another team member may execute the shared job
    // only after that executor independently receives current profile rights.
    state.grants.write().unwrap().push(allow(
        executor,
        "grant_cookie_executor_import",
        "cookie.import",
        Action::Create,
        ResourceScope::resource(
            WorkspaceId::from_str(DEST_WORKSPACE).unwrap(),
            ResourceId::from_str(DEST_RESOURCE).unwrap(),
        ),
    ));
    let inbox = app
        .clone()
        .oneshot(api_request(
            "GET",
            &format!("/v1/workspaces/{DEST_WORKSPACE}/cookie-jobs/inbox"),
            Some(&device_token),
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(inbox.status(), StatusCode::OK);
    assert_eq!(body(inbox).await["items"][0]["jobId"], job_id);
    let claim = app
        .clone()
        .oneshot(api_request(
            "POST",
            &format!("/v1/cookie-jobs/{job_id}/claim{query}"),
            Some(&device_token),
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(claim.status(), StatusCode::OK);
    assert_eq!(body(claim).await["state"], "dispatched");
    let ack = app
        .clone()
        .oneshot(api_request(
            "POST",
            &format!("/v1/cookie-jobs/{job_id}/ack{query}"),
            Some(&device_token),
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(ack.status(), StatusCode::OK);
    assert_eq!(body(ack).await["state"], "running");

    let complete = app
        .clone()
        .oneshot(api_request(
            "POST",
            &format!("/v1/cookie-jobs/{job_id}/complete{query}"),
            Some(&device_token),
            None,
            Some(json!({
                "operationId":job["operationId"],
                "jobId":job["jobId"],
                "deviceId":"device_cookie_owner",
                "profileId":DEST_PROFILE,
                "previousGeneration":4,
                "newGeneration":5,
                "createdCount":2,
                "replacedCount":0,
                "deletedCount":0,
                "metadataChecksum":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "completedAt":1
            })),
        ))
        .await
        .unwrap();
    assert_eq!(complete.status(), StatusCode::OK);
    let completed = body(complete).await;
    assert_eq!(completed["state"], "completed");
    assert_eq!(completed["receipt"]["createdCount"], 2);

    let requester_read = app
        .oneshot(api_request(
            "GET",
            &format!("/v1/cookie-jobs/{job_id}{query}"),
            Some(&requester_token),
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(requester_read.status(), StatusCode::OK);
    assert_eq!(body(requester_read).await["state"], "completed");
}
