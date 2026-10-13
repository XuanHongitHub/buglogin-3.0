use axum::{body::to_bytes, http::Request};
use buglogin_api::http::{ApiState, router};
use buglogin_api::{
    commands::{Adapter, CommandRegistry},
    http::rest_operation_ids,
};
use buglogin_browser::profile::ProfileService;
use tower::ServiceExt;

#[test]
fn rest_registry_has_stable_operation_ids() {
    let expected: Vec<_> = CommandRegistry::all()
        .iter()
        .filter(|command| command.adapters.contains(&Adapter::Rest))
        .map(|command| command.id.as_str())
        .collect();
    assert_eq!(rest_operation_ids(), expected);
    assert!(rest_operation_ids().contains(&"proxy.parse"));
}

#[test]
fn profile_create_openapi_includes_the_shared_command_dto() {
    let openapi = include_str!("../openapi/openapi-v1.yaml");
    let dto = openapi
        .split("    ProfileCreateCommandInput:")
        .nth(1)
        .expect("shared profile create DTO schema");
    let dto = dto
        .split("\n    ProfileCreateInput:")
        .next()
        .expect("DTO schema boundary");

    for field in [
        "id",
        "name",
        "runtime",
        "runtimeVersion",
        "workspaceId",
        "proxyId",
        "note",
    ] {
        assert!(
            dto.contains(&format!("{field}:")),
            "missing {field} in OpenAPI DTO"
        );
    }
    assert!(
        !dto.contains("state:"),
        "profile state must be server-owned"
    );
    assert!(openapi.contains("ProfileCreateInput: { oneOf:"));
}

#[tokio::test]
async fn runtime_openapi_endpoint_serves_the_canonical_contract_without_auth() {
    let response = router(ApiState::local(ProfileService::disabled()))
        .oneshot(
            Request::builder()
                .uri("/v1/openapi.yaml")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), axum::http::StatusCode::OK);
    assert_eq!(
        response.headers()[axum::http::header::CONTENT_TYPE],
        "application/yaml; charset=utf-8"
    );
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(body.as_ref(), include_bytes!("../openapi/openapi-v1.yaml"));
}
