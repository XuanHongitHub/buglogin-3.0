use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use buglogin_api::{
    http::{ApiState, FirstOwnerBootstrapInput, router},
    mcp::tools,
    mcp::transport::{McpTransportConfig, production_router},
};
use buglogin_browser::profile::ProfileService;
use buglogin_storage::{control::MemoryControlRecordStore, local::LocalRepository};
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
    request_at("/mcp", token, body)
}

fn request_at(path: &str, token: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(path)
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

async fn json_body(response: axum::response::Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap()).unwrap()
}

async fn scoped_token(label: &str) -> (ApiState, String) {
    scoped_token_with_scopes(label, vec!["system.read:read"]).await
}

async fn scoped_token_with_scopes(label: &str, scopes: Vec<&str>) -> (ApiState, String) {
    let api_state = ApiState::hosted_with_control_store(
        ProfileService::disabled(),
        MemoryControlRecordStore::default(),
    )
    .unwrap();
    api_state
        .bootstrap_first_owner(FirstOwnerBootstrapInput {
            account_id: Some(format!("acct_mcp_{label}")),
            username: format!("mcp-{label}"),
            email: format!("mcp-{label}@example.test"),
            operator_attested_email: true,
            password: zeroize::Zeroizing::new("test-password-123".into()),
            organization_name: "MCP integration test org".into(),
            workspace_name: "MCP integration test workspace".into(),
            max_workspaces: 5,
            max_profiles: 20,
            max_members: 5,
            max_storage_bytes: 1024 * 1024,
        })
        .unwrap();

    let app = router(api_state.clone());
    let login = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/auth/login")
                .header("origin", "http://127.0.0.1:3000")
                .header("content-type", "application/json")
                .header("x-correlation-id", format!("cor_mcp_{label}_login"))
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "identifier": format!("mcp-{label}@example.test"),
                        "password": "test-password-123"
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(login.status(), StatusCode::OK);
    let cookies: Vec<_> = login
        .headers()
        .get_all("set-cookie")
        .iter()
        .map(|value| {
            value
                .to_str()
                .unwrap()
                .split(';')
                .next()
                .unwrap()
                .to_owned()
        })
        .collect();
    let session = cookies
        .iter()
        .find(|value| value.starts_with("__Host-buglogin_session="))
        .unwrap();
    let csrf_cookie = cookies
        .iter()
        .find(|value| value.starts_with("__Host-buglogin_csrf="))
        .unwrap();
    let csrf = csrf_cookie.split_once('=').unwrap().1;
    let expires_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 3600;
    let create = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/auth/api-tokens")
                .header("origin", "http://127.0.0.1:3000")
                .header("cookie", format!("{session}; {csrf_cookie}"))
                .header("x-csrf-token", csrf)
                .header("content-type", "application/json")
                .header("x-correlation-id", format!("cor_mcp_{label}_token"))
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "name": "mcp-test",
                        "scopes": scopes,
                        "expiresAt": expires_at
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(create.status(), StatusCode::CREATED);
    let token = json_body(create).await["token"]
        .as_str()
        .unwrap()
        .to_owned();
    (api_state, token)
}

#[tokio::test]
async fn transport_authenticates_and_serves_json_rpc_tools() {
    let (api_state, token) = scoped_token("rpc").await;
    // Any durable account token is accepted by transport, then its own scopes
    // are evaluated by the command authorization policy.
    let app = production_router(
        api_state,
        McpTransportConfig {
            enabled: true,
            ..Default::default()
        },
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
    for tool in listed["result"]["tools"].as_array().unwrap() {
        let schema = &tool["inputSchema"];
        assert!(schema.is_object());
        assert!(
            schema["$ref"].is_string()
                || (schema["type"] == "object" && schema["properties"].is_object()),
            "tool {} must expose an object input schema or a schema reference",
            tool["name"]
        );
        if schema["$ref"].is_string() {
            assert!(schema["$defs"].is_object());
        }
        assert_ne!(schema["additionalProperties"], true);
    }

    let called = app
        .oneshot(request(
            &token,
            json!({
                "jsonrpc":"2.0",
                "id":3,
                "method":"tools/call",
                "params":{
                    "name":"system_health_get",
                    "arguments":{}
                }
            }),
        ))
        .await
        .unwrap();
    assert_eq!(called.status(), StatusCode::OK);
    assert_eq!(json_body(called).await["result"]["status"], "ok");
}

#[tokio::test]
async fn standard_mcp_mutation_generates_stable_correlation_and_idempotency() {
    let (api_state, token) =
        scoped_token_with_scopes("standard-mutation", vec!["profile.create:create"]).await;
    let app = production_router(
        api_state,
        McpTransportConfig {
            enabled: true,
            ..Default::default()
        },
    );
    let call = json!({
        "jsonrpc":"2.0",
        "id":"create-profile-1",
        "method":"tools/call",
        "params":{
            "name":"profile_create",
            "arguments":{"name":"standard MCP profile","runtime":"wayfern"}
        }
    });

    let created = app
        .clone()
        .oneshot(request(&token, call.clone()))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    let created = json_body(created).await;
    assert_eq!(created["result"]["name"], "standard MCP profile");
    let profile_id = created["result"]["id"].as_str().unwrap();

    let replay = app.clone().oneshot(request(&token, call)).await.unwrap();
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(json_body(replay).await["result"]["id"], profile_id);

    let conflict = app
        .oneshot(request(
            &token,
            json!({
                "jsonrpc":"2.0",
                "id":"create-profile-1",
                "method":"tools/call",
                "params":{
                    "name":"profile_create",
                    "arguments":{"name":"different profile","runtime":"wayfern"}
                }
            }),
        ))
        .await
        .unwrap();
    assert_eq!(conflict.status(), StatusCode::CONFLICT);
    assert_eq!(
        json_body(conflict).await["error"]["data"]["code"],
        "idempotency_conflict"
    );
}

#[tokio::test]
async fn transport_rejects_unknown_api_token_and_non_loopback_origin() {
    let app = production_router(
        state("deny"),
        McpTransportConfig {
            enabled: true,
            ..Default::default()
        },
    );
    let denied = app
        .clone()
        .oneshot(request(
            &format!("blapi_{}", "a".repeat(64)),
            json!({"jsonrpc":"2.0","id":1,"method":"initialize"}),
        ))
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);

    let (api_state, valid_token) = scoped_token("origin").await;
    let app = production_router(
        api_state,
        McpTransportConfig {
            enabled: true,
            ..Default::default()
        },
    );
    let mut forbidden = request(
        &valid_token,
        json!({"jsonrpc":"2.0","id":2,"method":"initialize"}),
    );
    forbidden
        .headers_mut()
        .insert("origin", "https://remote.example".parse().unwrap());
    let forbidden = app.oneshot(forbidden).await.unwrap();
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn versioned_api_mount_uses_transport_bearer_for_tool_authorization() {
    let (api_state, token) = scoped_token("mount").await;
    let app = buglogin_api::http::router(api_state.clone()).nest(
        "/v1",
        production_router(
            api_state,
            McpTransportConfig {
                enabled: true,
                ..Default::default()
            },
        ),
    );

    let initialized = app
        .clone()
        .oneshot(request_at(
            "/v1/mcp",
            &token,
            json!({"jsonrpc":"2.0","id":1,"method":"initialize"}),
        ))
        .await
        .unwrap();
    assert_eq!(initialized.status(), StatusCode::OK);

    let mismatch = app
        .oneshot(request_at(
            "/v1/mcp",
            &token,
            json!({
                "jsonrpc":"2.0",
                "id":2,
                "method":"tools/call",
                "params":{
                    "name":"system_health_get",
                    "arguments":{},
                    "bearerToken":"different-token",
                    "correlationId":"cor_mcp_bearer_mismatch"
                }
            }),
        ))
        .await
        .unwrap();
    assert_eq!(mismatch.status(), StatusCode::UNAUTHORIZED);
}
