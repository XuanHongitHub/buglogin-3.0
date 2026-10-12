use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use buglogin_api::http::{ApiState, router};
use buglogin_browser::profile::ProfileService;
use buglogin_domain::{
    account::Account,
    ids::{AccountId, GrantId, WorkspaceId},
    permissions::{Action, Capability, Effect, Grant, ResourceScope},
};
use buglogin_storage::control::{ControlRecordStore, MemoryControlRecordStore, PutControlRecord};
use serde_json::{Value, json};
use tower::ServiceExt;

fn request(token: &str, body: impl Into<Body>) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/v1/proxies/parse")
        .header("authorization", format!("Bearer {token}"))
        .header("x-correlation-id", "http-proxy-test")
        .body(body.into())
        .unwrap()
}

fn app() -> (Router, String) {
    let state = ApiState::new(ProfileService::disabled());
    let token = state.issue_token("acct_http_proxy").unwrap();
    state.grants.write().unwrap().push(Grant {
        id: GrantId::from_str("grant_http_proxy_parse").unwrap(),
        account_id: Some(AccountId::from_str("acct_http_proxy").unwrap()),
        member_id: None,
        capability: Capability::new("CAP-NET-001").unwrap(),
        action: Action::Read,
        scope: ResourceScope::default(),
        effect: Effect::Allow,
    });
    (router(state), token)
}

#[tokio::test]
async fn proxy_parse_requires_capability_grant() {
    let state = ApiState::new(ProfileService::disabled());
    let token = state.issue_token("acct_http_proxy_denied").unwrap();
    let response = router(state)
        .oneshot(request(&token, "http://127.0.0.1:8080"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn proxy_parse_normalizes_and_redacts_credentials() {
    let (app, token) = app();
    let response = app
        .oneshot(request(
            &token,
            "http://user:secret@127.0.0.1:8080\ninvalid\nhttp://user:secret@127.0.0.1:8080",
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["validCount"], 1);
    assert_eq!(value["invalidCount"], 1);
    assert_eq!(value["duplicateCount"], 1);
    assert_eq!(value["rows"][0]["protocol"], "http");
    assert_eq!(value["rows"][0]["endpoint"], "127.0.0.1:8080");
    assert_eq!(value["rows"][1]["line"], 2);
    assert_eq!(value["rows"][1]["error"], "invalid_port");
    let serialized = String::from_utf8(body.to_vec()).unwrap();
    assert!(!serialized.contains("user"));
    assert!(!serialized.contains("secret"));
}

#[tokio::test]
async fn proxy_parse_rejects_invalid_utf8_and_oversized_input() {
    let (app, token) = app();
    let invalid = app
        .clone()
        .oneshot(request(&token, vec![0xff]))
        .await
        .unwrap();
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);

    let oversized = app
        .oneshot(request(&token, vec![b'a'; 1024 * 1024 + 1]))
        .await
        .unwrap();
    assert_eq!(oversized.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

fn api_request(
    method: &str,
    uri: &str,
    token: &str,
    idempotency_key: Option<&str>,
    body: Option<Value>,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .header("x-correlation-id", "http-proxy-inventory-test");
    if let Some(key) = idempotency_key {
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

async fn json_body(response: axum::response::Response) -> Value {
    serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap()
}

#[tokio::test]
async fn hosted_proxy_inventory_replays_persists_and_enforces_acl_and_revision() {
    let account = AccountId::from_str("acct_proxy_inventory").unwrap();
    let workspace = WorkspaceId::from_str("ws_proxy_inventory").unwrap();
    let mut store = MemoryControlRecordStore::default();
    let account_value =
        serde_json::to_value(Account::new(account.clone(), "2026-07-22T00:00:00Z")).unwrap();
    store
        .put(PutControlRecord {
            tenant_id: "control",
            record_type: "account",
            record_id: account.as_str(),
            expected_revision: Some(0),
            idempotency_key: "seed-proxy-inventory-account",
            value: &account_value,
        })
        .unwrap();
    let reopen = store.reopen();
    let state = ApiState::hosted_with_control_store(ProfileService::disabled(), store).unwrap();
    let token = state.issue_token(account.as_str()).unwrap();
    let stale_reader =
        ApiState::hosted_with_control_store(ProfileService::disabled(), reopen.reopen()).unwrap();
    for action in [Action::Read, Action::Create, Action::Update, Action::Delete] {
        let grant_id = format!("grant_proxy_inventory_{action:?}");
        state.grants.write().unwrap().push(Grant {
            id: GrantId::from_str(&grant_id).unwrap(),
            account_id: Some(account.clone()),
            member_id: None,
            capability: Capability::new("CAP-NET-001").unwrap(),
            action,
            scope: ResourceScope::workspace(workspace.clone()),
            effect: Effect::Allow,
        });
    }
    let app = router(state.clone());
    let create_input = json!({
        "workspaceId": workspace,
        "proxy": {
            "id": "proxy_inventory_1",
            "name": "Primary",
            "protocol": "http",
            "host": "proxy.example",
            "port": 8080,
            "hasCredential": true,
            "bypassRules": ["localhost"],
            "createdAt": "2026-07-22T00:00:00Z",
            "updatedAt": "2026-07-22T00:00:00Z"
        }
    });
    let created = app
        .clone()
        .oneshot(api_request(
            "POST",
            "/v1/workspaces/ws_proxy_inventory/proxies",
            &token,
            Some("proxy-create-idem-0001"),
            Some(create_input.clone()),
        ))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    let created_body = json_body(created).await;
    assert_eq!(created_body["record"]["revision"], 1);
    assert_eq!(created_body["deduplicated"], false);
    assert_eq!(created_body["record"]["proxy"]["hasCredential"], true);
    assert!(
        created_body["record"]["proxy"]
            .get("credentialRef")
            .is_none()
    );

    let cross_instance_list = router(stale_reader)
        .oneshot(api_request(
            "GET",
            "/v1/workspaces/ws_proxy_inventory/proxies",
            &token,
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(cross_instance_list.status(), StatusCode::OK);
    assert_eq!(
        json_body(cross_instance_list).await["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    let replay = app
        .clone()
        .oneshot(api_request(
            "POST",
            "/v1/workspaces/ws_proxy_inventory/proxies",
            &token,
            Some("proxy-create-idem-0001"),
            Some(create_input),
        ))
        .await
        .unwrap();
    assert_eq!(json_body(replay).await, created_body);

    let listed = app
        .clone()
        .oneshot(api_request(
            "GET",
            "/v1/workspaces/ws_proxy_inventory/proxies",
            &token,
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(listed.status(), StatusCode::OK);
    assert_eq!(
        json_body(listed).await["items"].as_array().unwrap().len(),
        1
    );

    let denied = app
        .clone()
        .oneshot(api_request(
            "GET",
            "/v1/workspaces/ws_proxy_other/proxies",
            &token,
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);

    let update_input = json!({
        "workspaceId": "ws_proxy_inventory",
        "proxyId": "proxy_inventory_1",
        "expectedRevision": 1,
        "proxy": {
            "id": "proxy_inventory_1",
            "name": "Updated",
            "protocol": "https",
            "host": "updated.example",
            "port": 8443,
            "bypassRules": [],
            "createdAt": "2026-07-22T00:00:00Z",
            "updatedAt": "2026-07-22T01:00:00Z"
        }
    });
    let updated = app
        .clone()
        .oneshot(api_request(
            "PATCH",
            "/v1/workspaces/ws_proxy_inventory/proxies/proxy_inventory_1",
            &token,
            Some("proxy-update-idem-0001"),
            Some(update_input),
        ))
        .await
        .unwrap();
    assert_eq!(updated.status(), StatusCode::OK);
    assert_eq!(json_body(updated).await["revision"], 2);

    let stale = app
        .clone()
        .oneshot(api_request(
            "DELETE",
            "/v1/workspaces/ws_proxy_inventory/proxies/proxy_inventory_1",
            &token,
            Some("proxy-delete-stale-0001"),
            Some(json!({
                "workspaceId": "ws_proxy_inventory",
                "proxyId": "proxy_inventory_1",
                "expectedRevision": 1
            })),
        ))
        .await
        .unwrap();
    assert_eq!(stale.status(), StatusCode::CONFLICT);

    drop(app);
    drop(state);
    let restarted =
        ApiState::hosted_with_control_store(ProfileService::disabled(), reopen).unwrap();
    let restarted_token = restarted.issue_token(account.as_str()).unwrap();
    let restarted_app = router(restarted);
    let persisted = restarted_app
        .clone()
        .oneshot(api_request(
            "GET",
            "/v1/workspaces/ws_proxy_inventory/proxies/proxy_inventory_1",
            &restarted_token,
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(persisted.status(), StatusCode::OK);
    let persisted_body = json_body(persisted).await;
    assert_eq!(persisted_body["revision"], 2);
    assert_eq!(persisted_body["proxy"]["host"], "updated.example");

    let deleted = restarted_app
        .oneshot(api_request(
            "DELETE",
            "/v1/workspaces/ws_proxy_inventory/proxies/proxy_inventory_1",
            &restarted_token,
            Some("proxy-delete-idem-0002"),
            Some(json!({
                "workspaceId": "ws_proxy_inventory",
                "proxyId": "proxy_inventory_1",
                "expectedRevision": 2
            })),
        ))
        .await
        .unwrap();
    assert_eq!(deleted.status(), StatusCode::OK);
    assert_eq!(json_body(deleted).await["revision"], 2);
}
