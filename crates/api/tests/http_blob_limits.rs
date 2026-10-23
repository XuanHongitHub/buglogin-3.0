use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use buglogin_api::http::{ApiState, router};
use buglogin_browser::profile::ProfileService;
use buglogin_domain::{
    ids::{AccountId, GrantId, ResourceId, WorkspaceId},
    permissions::{Action, Capability, Effect, Grant, ResourceScope},
};
use tower::ServiceExt;

#[tokio::test]
async fn blob_endpoint_accepts_objects_larger_than_axums_default_body_limit() {
    let state = ApiState::local(ProfileService::disabled());
    let account_id = AccountId::new("acct_blob_limit").unwrap();
    let workspace_id = WorkspaceId::new("ws_blob_limit").unwrap();
    let resource_id = ResourceId::new("res_blob_limit").unwrap();
    let token = state.issue_token(account_id.as_str()).unwrap();
    state.grants.write().unwrap().push(Grant {
        id: GrantId::new("grant_blob_limit").unwrap(),
        account_id: Some(account_id),
        member_id: None,
        capability: Capability::new("profile.update").unwrap(),
        action: Action::Update,
        scope: ResourceScope::resource(workspace_id, resource_id),
        effect: Effect::Allow,
    });
    let app = router(state);
    let body = vec![0_u8; 2 * 1024 * 1024 + 1];
    let request = Request::builder()
        .method("PUT")
        .uri("/v1/workspaces/ws_blob_limit/resources/res_blob_limit/blobs?key=profile%2Flarge.bin")
        .header("x-correlation-id", "blob-limit-test-0001")
        .header("content-type", "application/octet-stream")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from(body))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}
