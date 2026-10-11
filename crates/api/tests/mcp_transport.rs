use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use buglogin_api::{
    http::ApiState,
    mcp::tools,
    mcp::transport::{McpTransportConfig, McpTransportControl, production_router},
};
use buglogin_browser::profile::ProfileService;
use buglogin_storage::local::LocalRepository;
use serde_json::{Value, json};
use tower::ServiceExt;

fn state(label: &str) -> ApiState {
    let root = std::env::temp_dir().join(format!(
        "buglogin-mcp-transport-{label}-{}",
        std::process::id()
    ));
    let repository = LocalRepository::open(root).unwrap();
    ApiState::local(ProfileService::new(repository))
}

fn request(token: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

async fn json_body(response: axum::response::Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap()).unwrap()
}

#[tokio::test]
async fn transport_authenticates_and_serves_json_rpc_tools() {
    let control = McpTransportControl::default();
    let token = control.issue().unwrap().expose_once();
    let app = production_router(
        state("rpc"),
        McpTransportConfig {
            enabled: true,
            ..Default::default()
        },
        control,
    );

    let initialize = app
        .clone()
        .oneshot(request(
            &token,
            json!({"jsonrpc":"2.0","id":1,"method":"initialize"}),
        ))
        .await
        .unwrap();
    assert_eq!(initialize.status(), StatusCode::OK);
    assert_eq!(
        json_body(initialize).await["result"]["serverInfo"]["name"],
        "buglogin"
    );

    let listed = app
        .clone()
        .oneshot(request(
            &token,
            json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
        ))
        .await
        .unwrap();
    let listed = json_body(listed).await;
    assert_eq!(
        listed["result"]["tools"].as_array().unwrap().len(),
        tools().len()
    );

    let called = app
        .oneshot(request(
            &token,
            json!({
                "jsonrpc":"2.0",
                "id":3,
                "method":"tools/call",
                "params":{
                    "name":"system_health_get",
                    "arguments":{},
                    "correlationId":"cor_transport_health"
                }
            }),
        ))
        .await
        .unwrap();
    assert_eq!(called.status(), StatusCode::OK);
    assert_eq!(json_body(called).await["result"]["status"], "ok");
}

#[tokio::test]
async fn transport_rejects_bad_bearer_and_non_loopback_origin() {
    let control = McpTransportControl::default();
    let token = control.issue().unwrap().expose_once();
    let app = production_router(
        state("deny"),
        McpTransportConfig {
            enabled: true,
            ..Default::default()
        },
        control,
    );
    let denied = app
        .clone()
        .oneshot(request(
            "invalid-transport-bearer-value-0000",
            json!({"jsonrpc":"2.0","id":1,"method":"initialize"}),
        ))
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);

    let mut forbidden = request(
        &token,
        json!({"jsonrpc":"2.0","id":2,"method":"initialize"}),
    );
    forbidden
        .headers_mut()
        .insert("origin", "https://remote.example".parse().unwrap());
    let forbidden = app.oneshot(forbidden).await.unwrap();
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);
}
