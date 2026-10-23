use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use buglogin_api::http::{ApiState, router};
use buglogin_browser::profile::ProfileService;
use buglogin_domain::{
    entities::{Profile, ProfileState, Runtime, SyncMode},
    errors::Problem,
    ids::{AccountId, GrantId, ProfileId, ResourceId, WorkspaceId},
    member::{Member, MemberRole, MemberStatus},
    permissions::{Action, Capability, Effect, Grant, ResourceScope},
    profile::{ProfileArchiveState, ProfileResource},
};
use buglogin_storage::local::LocalRepository;
use buglogin_sync::{
    coordination::{
        CoordinationBackend, CoordinationEvent, CoordinationScope, LeaseGrant, Presence,
        PublishedEvent, StreamEvent,
    },
    manifest::SyncManifest,
};
use serde_json::{Value, json};
use tower::ServiceExt;

const DEST_PROFILE: &str = "prf_cookie_destination";
const DEST_WORKSPACE: &str = "ws_cookie_destination";
const DEST_RESOURCE: &str = "res_cookie_destination";
static NEXT_TEST_STATE: AtomicUsize = AtomicUsize::new(1);

fn cookie_state() -> ApiState {
    let root = std::env::temp_dir().join(format!(
        "buglogin-cookie-jobs-{}-{}",
        std::process::id(),
        NEXT_TEST_STATE.fetch_add(1, Ordering::Relaxed)
    ));
    ApiState::new(ProfileService::new(LocalRepository::open(root).unwrap()))
}

#[derive(Default)]
struct CookieTestCoordination;

#[async_trait]
impl CoordinationBackend for CookieTestCoordination {
    async fn readiness(&self) -> Result<(), Problem> {
        Ok(())
    }

    async fn acquire_lease_idempotent(
        &self,
        _scope: CoordinationScope<'_>,
        _resource_id: &ResourceId,
        _ttl: Duration,
        _idempotency_key: &str,
    ) -> Result<LeaseGrant, Problem> {
        Err(coordination_unsupported())
    }

    async fn renew_lease_idempotent(
        &self,
        _scope: CoordinationScope<'_>,
        _resource_id: &ResourceId,
        _owner_token: &str,
        _ttl: Duration,
        _idempotency_key: &str,
    ) -> Result<LeaseGrant, Problem> {
        Err(coordination_unsupported())
    }

    async fn release_lease_idempotent(
        &self,
        _scope: CoordinationScope<'_>,
        _resource_id: &ResourceId,
        _owner_token: &str,
        _idempotency_key: &str,
    ) -> Result<(), Problem> {
        Err(coordination_unsupported())
    }

    async fn lease_status(
        &self,
        _scope: CoordinationScope<'_>,
        _resource_id: &ResourceId,
    ) -> Result<Option<buglogin_sync::coordination::LeaseStatus>, Problem> {
        Err(coordination_unsupported())
    }

    async fn validate_write_authority(
        &self,
        scope: CoordinationScope<'_>,
        resource_id: &ResourceId,
        owner_token: &str,
        fencing_token: u64,
    ) -> Result<(), Problem> {
        if scope.workspace_id.as_str() == DEST_WORKSPACE
            && resource_id.as_str() == DEST_RESOURCE
            && scope.auth.actor_id() == "cookie-runtime-owner"
            && owner_token == "cookie-runtime-token"
            && fencing_token == 17
        {
            Ok(())
        } else {
            Err(Problem::new(
                "lease_not_owner",
                "Lease mismatch",
                409,
                "test",
            ))
        }
    }

    async fn heartbeat_presence(
        &self,
        _scope: CoordinationScope<'_>,
        _session_id: &str,
        _metadata: BTreeMap<String, String>,
        _ttl: Duration,
    ) -> Result<Presence, Problem> {
        Err(coordination_unsupported())
    }

    async fn list_presence(&self, _scope: CoordinationScope<'_>) -> Result<Vec<Presence>, Problem> {
        Err(coordination_unsupported())
    }

    async fn publish_event(
        &self,
        _scope: CoordinationScope<'_>,
        _idempotency_key: &str,
        _event: CoordinationEvent,
    ) -> Result<PublishedEvent, Problem> {
        Err(coordination_unsupported())
    }

    async fn read_events(
        &self,
        _scope: CoordinationScope<'_>,
        _after_stream_id: Option<&str>,
        _limit: usize,
    ) -> Result<Vec<StreamEvent>, Problem> {
        Err(coordination_unsupported())
    }

    async fn current_manifest(
        &self,
        _scope: CoordinationScope<'_>,
        _resource_id: &ResourceId,
    ) -> Result<Option<SyncManifest>, Problem> {
        Err(coordination_unsupported())
    }

    async fn commit_manifest(
        &self,
        _scope: CoordinationScope<'_>,
        _resource_id: &ResourceId,
        _owner_token: &str,
        _manifest: SyncManifest,
    ) -> Result<(), Problem> {
        Err(coordination_unsupported())
    }
}

fn coordination_unsupported() -> Problem {
    Problem::new(
        "coordination_unsupported",
        "Unsupported in this test",
        501,
        "test",
    )
}

fn with_cookie_lease(mut request: Request<Body>) -> Request<Body> {
    let headers = request.headers_mut();
    headers.insert(
        "x-buglogin-owner-id",
        "cookie-runtime-owner".parse().unwrap(),
    );
    headers.insert(
        "x-buglogin-owner-token",
        "cookie-runtime-token".parse().unwrap(),
    );
    headers.insert("x-buglogin-fencing-token", "17".parse().unwrap());
    request
}

fn with_invalid_cookie_lease(mut request: Request<Body>) -> Request<Body> {
    request.headers_mut().insert(
        "x-buglogin-owner-token",
        "stale-cookie-runtime-token".parse().unwrap(),
    );
    request
}

fn active_profile(id: &str) -> Profile {
    Profile {
        id: ProfileId::from_str(id).unwrap(),
        name: "Cookie job profile".into(),
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
        tags: vec![],
        extension_group_id: None,
        sync_mode: Some(SyncMode::Managed),
        created_at: "2026-10-15T00:00:00Z".into(),
        updated_at: "2026-10-15T00:00:00Z".into(),
    }
}

fn seed_active_profile(state: ApiState, id: &str) -> ApiState {
    state.with_profile_record(active_profile(id))
}

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
        "stagedImportRef": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    })
}

#[tokio::test]
async fn enqueue_derives_actor_key_and_expiry_and_replays_one_authorized_job() {
    let account = "acct_cookie_owner";
    let state =
        cookie_state().with_profile_resource(binding(DEST_PROFILE, DEST_WORKSPACE, DEST_RESOURCE));
    let state = seed_active_profile(state, DEST_PROFILE);
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
    let accepted_status = accepted.status();
    let job = body(accepted).await;
    assert_eq!(accepted_status, StatusCode::ACCEPTED, "{job}");
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
    let state = cookie_state()
        .with_profile_resource(binding(DEST_PROFILE, DEST_WORKSPACE, DEST_RESOURCE))
        .with_profile_resource(binding(source_profile, source_workspace, source_resource));
    let state = seed_active_profile(state, DEST_PROFILE);
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
async fn archived_destination_is_rejected_before_cookie_job_persistence() {
    let account = "acct_cookie_archived";
    let mut archived = active_profile(DEST_PROFILE);
    archived.archived = true;
    let state = cookie_state().with_profile_resource(ProfileResource {
        archive_state: ProfileArchiveState::Archived,
        ..binding(DEST_PROFILE, DEST_WORKSPACE, DEST_RESOURCE)
    });
    let state = state.with_profile_record(archived);
    let token = state.issue_token(account).unwrap();
    state.grants.write().unwrap().push(allow(
        account,
        "grant_cookie_archived_import",
        "cookie.import",
        Action::Create,
        ResourceScope::resource(
            WorkspaceId::from_str(DEST_WORKSPACE).unwrap(),
            ResourceId::from_str(DEST_RESOURCE).unwrap(),
        ),
    ));

    let response = router(state)
        .oneshot(request(
            &token,
            "cookie-archived-profile-request-0001",
            import_input(),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(body(response).await["code"], "profile_archived");
}

#[tokio::test]
async fn device_bearer_is_required_for_cookie_job_claim_ack_and_receipt() {
    let requester = "acct_cookie_requester";
    let executor = "acct_cookie_executor";
    let state = cookie_state()
        .with_coordination(Arc::new(CookieTestCoordination))
        .with_profile_resource(binding(DEST_PROFILE, DEST_WORKSPACE, DEST_RESOURCE));
    let state = seed_active_profile(state, DEST_PROFILE);
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
    state.grants.write().unwrap().push(allow(
        requester,
        "grant_cookie_requester_read",
        "profile.read",
        Action::Read,
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
    for (token, expected_status) in [
        (None, StatusCode::UNAUTHORIZED),
        (Some(executor_session_token.as_str()), StatusCode::FORBIDDEN),
    ] {
        let inbox = app
            .clone()
            .oneshot(api_request(
                "GET",
                "/v1/device/cookie-jobs/inbox",
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
    state
        .grants
        .write()
        .unwrap()
        .retain(|grant| grant.id.as_str() != "grant_cookie_executor_import");
    let revoked_before_ack = app
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
    assert_eq!(revoked_before_ack.status(), StatusCode::FORBIDDEN);
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
    let missing_lease = app
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
    assert_eq!(missing_lease.status(), StatusCode::BAD_REQUEST);

    let stale_lease = app
        .clone()
        .oneshot(with_invalid_cookie_lease(with_cookie_lease(api_request(
            "POST",
            &format!("/v1/cookie-jobs/{job_id}/ack{query}"),
            Some(&device_token),
            None,
            None,
        ))))
        .await
        .unwrap();
    assert_eq!(stale_lease.status(), StatusCode::CONFLICT);

    let ack = app
        .clone()
        .oneshot(with_cookie_lease(api_request(
            "POST",
            &format!("/v1/cookie-jobs/{job_id}/ack{query}"),
            Some(&device_token),
            None,
            None,
        )))
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
    assert_eq!(complete.status(), StatusCode::CONFLICT);
    assert_eq!(
        body(complete).await["code"],
        "cookie_profile_sync_not_committed"
    );

    let failed_job_response = app
        .clone()
        .oneshot(request(
            &requester_token,
            "cookie-job-failing-request-01",
            import_input(),
        ))
        .await
        .unwrap();
    assert_eq!(failed_job_response.status(), StatusCode::ACCEPTED);
    let failed_job = body(failed_job_response).await;
    let failed_job_id = failed_job["jobId"].as_str().unwrap();
    for transition in ["claim", "ack"] {
        let request = api_request(
            "POST",
            &format!("/v1/cookie-jobs/{failed_job_id}/{transition}{query}"),
            Some(&device_token),
            None,
            None,
        );
        let request = if transition == "ack" {
            with_cookie_lease(request)
        } else {
            request
        };
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
    let failed = app
        .clone()
        .oneshot(api_request(
            "POST",
            &format!("/v1/cookie-jobs/{failed_job_id}/fail{query}"),
            Some(&device_token),
            None,
            Some(json!({"failureCode":"materialization_failed"})),
        ))
        .await
        .unwrap();
    assert_eq!(failed.status(), StatusCode::OK);
    let failed = body(failed).await;
    assert_eq!(failed["state"], "failed");
    assert_eq!(failed["failureCode"], "materialization_failed");

    let cancelled_job_response = app
        .clone()
        .oneshot(request(
            &requester_token,
            "cookie-job-cancel-request-0001",
            import_input(),
        ))
        .await
        .unwrap();
    assert_eq!(cancelled_job_response.status(), StatusCode::ACCEPTED);
    let cancelled_job = body(cancelled_job_response).await;
    let denied_by_scope = app
        .clone()
        .oneshot(api_request(
            "POST",
            &format!(
                "/v1/cookie-jobs/{}/cancel{query}",
                cancelled_job["jobId"].as_str().unwrap()
            ),
            Some(&executor_session_token),
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(denied_by_scope.status(), StatusCode::FORBIDDEN);
    let denied_cancel = app
        .clone()
        .oneshot(api_request(
            "POST",
            &format!(
                "/v1/cookie-jobs/{}/cancel{query}",
                cancelled_job["jobId"].as_str().unwrap()
            ),
            Some(&executor_session_token),
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(denied_cancel.status(), StatusCode::FORBIDDEN);
    let cancelled = app
        .clone()
        .oneshot(api_request(
            "POST",
            &format!(
                "/v1/cookie-jobs/{}/cancel{query}",
                cancelled_job["jobId"].as_str().unwrap()
            ),
            Some(&requester_token),
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(cancelled.status(), StatusCode::OK);
    assert_eq!(body(cancelled).await["state"], "cancelled");

    let requester_read = app
        .clone()
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
    assert_eq!(body(requester_read).await["state"], "running");
}

#[tokio::test]
async fn runtime_dispatch_targets_owned_devices_and_requires_a_matching_receipt() {
    let account = "acct_runtime_dispatch";
    let workspace = "ws_runtime_dispatch";
    let profile = "prf_runtime_dispatch";
    let resource = "res_runtime_dispatch";
    let device_id = "device_runtime_dispatch";
    let state = cookie_state()
        .with_profile_resource(binding(profile, workspace, resource))
        .with_profile_record(active_profile(profile));
    let owner_id = AccountId::from_str(account).unwrap();
    let workspace_id = WorkspaceId::from_str(workspace).unwrap();
    let member_id = buglogin_domain::ids::MemberId::from_str("mem_runtime_dispatch").unwrap();
    state
        .members
        .write()
        .unwrap()
        .invite(
            &buglogin_domain::permissions::Principal {
                roles: [buglogin_domain::permissions::Role::PlatformAdmin]
                    .into_iter()
                    .collect(),
                ..Default::default()
            },
            &[],
            Member {
                id: member_id.clone(),
                account_id: owner_id.clone(),
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
            &buglogin_domain::permissions::Principal {
                account_id: Some(owner_id.clone()),
                ..Default::default()
            },
            &[],
            &workspace_id,
            &member_id,
            2,
        )
        .unwrap();
    for (grant_id, capability) in [
        ("grant_runtime_launch", "profile.launch"),
        ("grant_runtime_stop", "profile.stop"),
    ] {
        state.grants.write().unwrap().push(allow(
            account,
            grant_id,
            capability,
            Action::Execute,
            ResourceScope::resource(
                workspace_id.clone(),
                ResourceId::from_str(resource).unwrap(),
            ),
        ));
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let device_tokens = state
        .auth
        .write()
        .unwrap()
        .issue_device_session(owner_id, now + 3600, now + 7200, device_id.into(), None)
        .unwrap();
    let device_token = device_tokens.access_token.clone();
    let owner_token = state.issue_token(account).unwrap();
    let app = router(state.clone());
    let enqueue = app
        .clone()
        .oneshot(api_request(
            "POST",
            &format!("/v1/workspaces/{workspace}/profiles/{profile}/runtime-dispatch"),
            Some(&owner_token),
            Some("runtime-dispatch-test-key-0001"),
            Some(json!({
                "commandId":"profile.launch",
                "targetDeviceId":device_id,
                "input":{"headless":false}
            })),
        ))
        .await
        .unwrap();
    assert_eq!(enqueue.status(), StatusCode::ACCEPTED);
    let job = body(enqueue).await["job"].clone();
    assert_eq!(job["deviceId"], device_id);
    assert_eq!(job["state"], "accepted");
    let operation_uri = format!("/v1/operations/{}", job["operationId"].as_str().unwrap());
    let pending = app
        .clone()
        .oneshot(api_request(
            "GET",
            &operation_uri,
            Some(&owner_token),
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(pending.status(), StatusCode::OK);
    assert_eq!(body(pending).await["status"], "running");

    let inbox = app
        .clone()
        .oneshot(api_request(
            "GET",
            "/v1/device/runtime-dispatch/inbox",
            Some(&device_token),
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(inbox.status(), StatusCode::OK);
    assert_eq!(body(inbox).await["items"][0]["jobId"], job["jobId"]);

    let claim = app
        .clone()
        .oneshot(api_request(
            "POST",
            &format!(
                "/v1/workspaces/{workspace}/runtime-dispatch/{}/claim",
                job["jobId"].as_str().unwrap()
            ),
            Some(&device_token),
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(claim.status(), StatusCode::OK);
    assert_eq!(body(claim).await["job"]["state"], "running");

    let receipt = json!({
        "jobId":job["jobId"],
        "operationId":job["operationId"],
        "deviceId":device_id,
        "profileId":profile,
        "commandId":"profile.launch",
        "state":"running"
    });
    let mut mismatched_receipt = receipt.clone();
    mismatched_receipt["state"] = json!("stopped");
    let mismatch = app
        .clone()
        .oneshot(api_request(
            "POST",
            &format!(
                "/v1/workspaces/{workspace}/runtime-dispatch/{}/complete",
                job["jobId"].as_str().unwrap()
            ),
            Some(&device_token),
            None,
            Some(mismatched_receipt),
        ))
        .await
        .unwrap();
    assert_eq!(mismatch.status(), StatusCode::BAD_REQUEST);
    assert_eq!(body(mismatch).await["code"], "runtime_job_receipt_mismatch");

    let complete = app
        .clone()
        .oneshot(api_request(
            "POST",
            &format!(
                "/v1/workspaces/{workspace}/runtime-dispatch/{}/complete",
                job["jobId"].as_str().unwrap()
            ),
            Some(&device_token),
            None,
            Some(receipt.clone()),
        ))
        .await
        .unwrap();
    assert_eq!(complete.status(), StatusCode::OK);
    assert_eq!(body(complete).await["job"]["state"], "completed");
    let finished = app
        .clone()
        .oneshot(api_request(
            "GET",
            &operation_uri,
            Some(&owner_token),
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(finished.status(), StatusCode::OK);
    assert_eq!(body(finished).await["status"], "completed");

    let replay = app
        .oneshot(api_request(
            "POST",
            &format!(
                "/v1/workspaces/{workspace}/runtime-dispatch/{}/complete",
                job["jobId"].as_str().unwrap()
            ),
            Some(&device_token),
            None,
            Some(receipt),
        ))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(body(replay).await["job"]["state"], "completed");
}
