use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use buglogin_api::http::{ApiState, router};
use buglogin_browser::profile::ProfileService;
use buglogin_storage::control::MemoryControlRecordStore;
use tower::ServiceExt;

fn get(path: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(path)
        .header("x-correlation-id", "readiness-test")
        .body(Body::empty())
        .unwrap()
}

#[tokio::test]
async fn missing_runtime_catalog_does_not_mask_missing_hosted_storage() {
    let state = ApiState::hosted_with_control_store(
        ProfileService::disabled(),
        MemoryControlRecordStore::default(),
    )
    .unwrap();
    let app = router(state);

    let liveness = app.clone().oneshot(get("/v1/health")).await.unwrap();
    assert_eq!(liveness.status(), StatusCode::OK);

    let readiness = app.oneshot(get("/v1/readyz")).await.unwrap();
    assert_eq!(readiness.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = axum::body::to_bytes(readiness.into_body(), usize::MAX)
        .await
        .unwrap();
    let problem: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(problem["code"], "object_storage_not_configured");
}

#[tokio::test]
async fn local_readiness_accepts_local_memory_adapters() {
    let app = router(ApiState::local(ProfileService::disabled()));
    let readiness = app.oneshot(get("/v1/readyz")).await.unwrap();
    assert_eq!(readiness.status(), StatusCode::OK);
    assert_eq!(
        readiness
            .headers()
            .get("x-buglogin-persistence-mode")
            .and_then(|value| value.to_str().ok()),
        Some("memory")
    );
    assert_eq!(
        readiness
            .headers()
            .get("x-buglogin-runtime-catalog")
            .and_then(|value| value.to_str().ok()),
        Some("unavailable")
    );
}
