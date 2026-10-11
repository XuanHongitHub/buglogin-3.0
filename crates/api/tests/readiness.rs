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
async fn liveness_stays_up_while_hosted_readiness_fails_closed() {
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
}

#[tokio::test]
async fn local_readiness_accepts_local_memory_adapters() {
    let app = router(ApiState::local(ProfileService::disabled()));
    let readiness = app.oneshot(get("/v1/readyz")).await.unwrap();
    assert_eq!(readiness.status(), StatusCode::OK);
}
