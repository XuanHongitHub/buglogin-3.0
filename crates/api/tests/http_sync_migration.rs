use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use buglogin_api::http::{ApiState, router};
use buglogin_browser::profile::ProfileService;
use tower::ServiceExt;

#[tokio::test]
async fn retired_sync_migration_endpoint_is_not_registered() {
    let state = ApiState::new(ProfileService::disabled());
    let token = state.issue_token("acct_retired_sync_migration").unwrap();
    let response = router(state)
        .oneshot(
            Request::post("/v1/migrations/sync/import")
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {token}"))
                .header("x-correlation-id", "retired-sync-migration")
                .header("idempotency-key", "retired-sync-migration-key")
                .body(Body::from(r#"{"export":null}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
