use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use buglogin_api::http::{ApiState, router};
use buglogin_browser::profile::ProfileService;
use tower::ServiceExt;

fn preflight(origin: &str) -> Request<Body> {
    Request::builder()
        .method("OPTIONS")
        .uri("/v1/auth/login")
        .header("origin", origin)
        .header("access-control-request-method", "POST")
        .header(
            "access-control-request-headers",
            "content-type,x-correlation-id,x-csrf-token",
        )
        .body(Body::empty())
        .unwrap()
}

#[tokio::test]
async fn configured_web_origin_can_preflight_credentialed_auth_requests() {
    let state = ApiState::local(ProfileService::disabled())
        .with_web_origin("https://app.buglogin.com")
        .unwrap();
    let response = router(state)
        .oneshot(preflight("https://app.buglogin.com"))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["access-control-allow-origin"],
        "https://app.buglogin.com"
    );
    assert_eq!(
        response.headers()["access-control-allow-credentials"],
        "true"
    );
    assert!(
        response.headers()["access-control-allow-methods"]
            .to_str()
            .unwrap()
            .contains("POST")
    );
    assert!(
        response.headers()["access-control-allow-headers"]
            .to_str()
            .unwrap()
            .contains("x-csrf-token")
    );
}

#[tokio::test]
async fn unconfigured_origins_receive_no_cors_permission() {
    let state = ApiState::local(ProfileService::disabled())
        .with_web_origin("https://app.buglogin.com")
        .unwrap();
    let response = router(state)
        .oneshot(preflight("https://attacker.example"))
        .await
        .unwrap();

    assert!(
        !response
            .headers()
            .contains_key("access-control-allow-origin")
    );
}

#[test]
fn web_origin_configuration_accepts_only_serialized_origins() {
    let state = ApiState::local(ProfileService::disabled());
    assert!(
        state
            .clone()
            .with_web_origin("https://app.buglogin.com")
            .is_ok()
    );
    assert!(
        state
            .clone()
            .with_web_origin("http://127.0.0.1:3000")
            .is_ok()
    );
    assert!(
        state
            .clone()
            .with_web_origin("https://app.buglogin.com/path")
            .is_err()
    );
    assert!(
        state
            .with_web_origin("http://127.0.0.1.attacker.example")
            .is_err()
    );
}
