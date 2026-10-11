use std::{
    fs,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use buglogin_api::http::{ApiState, router};
use buglogin_api::services::recovery::{RecoveryDelivery, RecoveryDeliveryMessage};
use buglogin_browser::profile::ProfileService;
use buglogin_domain::errors::Problem;
use buglogin_storage::local::LocalRepository;
use serde_json::{Value, json};
use tower::ServiceExt;

fn root(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!("buglogin-http-auth-{label}-{}", std::process::id()))
}

fn request(method: &str, uri: &str, token: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .header("x-correlation-id", "http-auth-test")
        .body(Body::empty())
        .unwrap()
}

fn json_request(method: &str, uri: &str, body: Value, token: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .header("x-correlation-id", "http-device-auth-test");
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    builder
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

fn web_json_request(method: &str, uri: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .header("origin", "http://127.0.0.1:3000")
        .header("x-correlation-id", "http-web-auth-test")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

fn web_json_idempotent_request(
    method: &str,
    uri: &str,
    body: Value,
    idempotency_key: &str,
) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .header("origin", "http://127.0.0.1:3000")
        .header("idempotency-key", idempotency_key)
        .header("x-correlation-id", "http-web-auth-test")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

fn cookie_pair(value: &str) -> &str {
    value.split(';').next().unwrap()
}

fn auth_cookies(response: &axum::response::Response) -> (String, String) {
    let values: Vec<&str> = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|value| value.to_str().unwrap())
        .collect();
    let session = values
        .iter()
        .find(|value| value.starts_with("__Host-buglogin_session="))
        .unwrap();
    let csrf = values
        .iter()
        .find(|value| value.starts_with("__Host-buglogin_csrf="))
        .unwrap();
    (
        format!("{}; {}", cookie_pair(session), cookie_pair(csrf)),
        cookie_pair(csrf).split_once('=').unwrap().1.to_owned(),
    )
}

#[tokio::test]
async fn refresh_rotates_bearer_and_logout_revokes_replacement() {
    let root = root("rotate");
    let _ = fs::remove_dir_all(&root);
    let state = ApiState::new(ProfileService::new(LocalRepository::open(&root).unwrap()));
    let token = state.issue_token("acct_auth_http").unwrap();
    let app: Router = router(state);

    let response = app
        .clone()
        .oneshot(request("POST", "/v1/auth/refresh", &token))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).expect("invalid auth response");
    let replacement = body["accessToken"].as_str().unwrap();
    assert_ne!(replacement, token);
    assert_eq!(body["tokenType"], "Bearer");

    let old = app
        .clone()
        .oneshot(request("POST", "/v1/auth/refresh", &token))
        .await
        .unwrap();
    assert_eq!(old.status(), StatusCode::UNAUTHORIZED);

    let logged_out = app
        .clone()
        .oneshot(request("POST", "/v1/auth/logout", replacement))
        .await
        .unwrap();
    assert_eq!(logged_out.status(), StatusCode::NO_CONTENT);

    let revoked = app
        .oneshot(request("POST", "/v1/auth/refresh", replacement))
        .await
        .unwrap();
    assert_eq!(revoked.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn auth_routes_require_correlation_and_bearer() {
    let root = root("headers");
    let _ = fs::remove_dir_all(&root);
    let state = ApiState::new(ProfileService::new(LocalRepository::open(&root).unwrap()));
    let app: Router = router(state);
    let missing = Request::builder()
        .method("POST")
        .uri("/v1/auth/refresh")
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(missing).await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );
    let bearer_missing = Request::builder()
        .method("POST")
        .uri("/v1/auth/refresh")
        .header("x-correlation-id", "http-auth-header-test")
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.oneshot(bearer_missing).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn web_login_uses_secure_cookies_session_projection_and_csrf_logout() {
    let root = root("web-login");
    let _ = fs::remove_dir_all(&root);
    let state = ApiState::new(ProfileService::new(LocalRepository::open(&root).unwrap()));
    state
        .provision_web_identity(
            "acct_web_http",
            "Web.Owner",
            "web.owner@example.com",
            "correct horse battery staple",
        )
        .unwrap();
    let app: Router = router(state);

    let login = app
        .clone()
        .oneshot(web_json_request(
            "POST",
            "/v1/auth/login",
            json!({
                "identifier": "WEB.OWNER",
                "password": "correct horse battery staple"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(login.status(), StatusCode::OK);
    assert_eq!(login.headers()[header::CACHE_CONTROL], "no-store");
    let set_cookies: Vec<String> = login
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|value| value.to_str().unwrap().to_owned())
        .collect();
    assert_eq!(set_cookies.len(), 2);
    let session_cookie = set_cookies
        .iter()
        .find(|value| value.starts_with("__Host-buglogin_session="))
        .unwrap();
    let csrf_cookie = set_cookies
        .iter()
        .find(|value| value.starts_with("__Host-buglogin_csrf="))
        .unwrap();
    assert!(session_cookie.contains("HttpOnly"));
    assert!(session_cookie.contains("Secure"));
    assert!(session_cookie.contains("SameSite=Lax"));
    assert!(csrf_cookie.contains("Secure"));
    assert!(!csrf_cookie.contains("HttpOnly"));
    let cookie_header = format!(
        "{}; {}",
        cookie_pair(session_cookie),
        cookie_pair(csrf_cookie)
    );
    let csrf = cookie_pair(csrf_cookie).split_once('=').unwrap().1;

    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(login.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(body["accountId"], "acct_web_http");
    assert!(body.get("accessToken").is_none());
    assert!(body.get("csrfToken").is_none());

    let session = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/auth/session")
                .header("cookie", &cookie_header)
                .header("x-correlation-id", "http-web-session-test")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(session.status(), StatusCode::OK);

    let missing_csrf = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/auth/logout")
                .header("origin", "http://127.0.0.1:3000")
                .header("cookie", &cookie_header)
                .header("x-correlation-id", "http-web-logout-missing-csrf")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing_csrf.status(), StatusCode::FORBIDDEN);

    let logout = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/auth/logout")
                .header("origin", "http://127.0.0.1:3000")
                .header("cookie", &cookie_header)
                .header("x-csrf-token", csrf)
                .header("x-correlation-id", "http-web-logout-test")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(logout.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        logout.headers().get_all(header::SET_COOKIE).iter().count(),
        2
    );

    let revoked = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/auth/session")
                .header("cookie", cookie_header)
                .header("x-correlation-id", "http-web-revoked-test")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(revoked.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn web_login_rejects_cross_origin_before_credential_verification() {
    let root = root("web-origin");
    let _ = fs::remove_dir_all(&root);
    let state = ApiState::new(ProfileService::new(LocalRepository::open(&root).unwrap()));
    let app: Router = router(state);
    let request = Request::builder()
        .method("POST")
        .uri("/v1/auth/login")
        .header("content-type", "application/json")
        .header("origin", "https://attacker.example")
        .header("x-correlation-id", "http-web-origin-test")
        .body(Body::from(
            serde_json::to_vec(&json!({
                "identifier": "nobody",
                "password": "wrong password value"
            }))
            .unwrap(),
        ))
        .unwrap();
    assert_eq!(
        app.oneshot(request).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn web_session_list_and_revoke_are_owner_scoped_csrf_bound_and_durable() {
    let root = root("web-session-management");
    let _ = fs::remove_dir_all(&root);
    let state = ApiState::new(ProfileService::new(LocalRepository::open(&root).unwrap()));
    state
        .provision_web_identity(
            "acct_sessions_http",
            "Session.Owner",
            "session.owner@example.com",
            "correct horse battery staple",
        )
        .unwrap();
    let app: Router = router(state);
    let login_body = json!({
        "identifier": "session.owner",
        "password": "correct horse battery staple"
    });
    let first = app
        .clone()
        .oneshot(web_json_request(
            "POST",
            "/v1/auth/login",
            login_body.clone(),
        ))
        .await
        .unwrap();
    let (first_cookies, first_csrf) = auth_cookies(&first);
    let second = app
        .clone()
        .oneshot(web_json_request("POST", "/v1/auth/login", login_body))
        .await
        .unwrap();
    let (second_cookies, _) = auth_cookies(&second);

    let listed = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/auth/sessions")
                .header("cookie", &first_cookies)
                .header("x-correlation-id", "http-session-list")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(listed.status(), StatusCode::OK);
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(listed.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    let sessions = body["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 2);
    assert!(sessions.iter().all(|session| session["kind"] == "browser"));
    let current_id = sessions
        .iter()
        .find(|session| session["current"] == true)
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let other_id = sessions
        .iter()
        .find(|session| session["current"] == false)
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let missing_idempotency = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/v1/auth/sessions/{other_id}"))
                .header("origin", "http://127.0.0.1:3000")
                .header("cookie", &first_cookies)
                .header("x-csrf-token", &first_csrf)
                .header("x-correlation-id", "http-session-revoke-missing-idem")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing_idempotency.status(), StatusCode::BAD_REQUEST);

    let revoked_other = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/v1/auth/sessions/{other_id}"))
                .header("origin", "http://127.0.0.1:3000")
                .header("cookie", &first_cookies)
                .header("x-csrf-token", &first_csrf)
                .header("idempotency-key", "session-revoke-other-0001")
                .header("x-correlation-id", "http-session-revoke-other")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(revoked_other.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        revoked_other
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .count(),
        0
    );

    let second_invalid = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/auth/session")
                .header("cookie", second_cookies)
                .header("x-correlation-id", "http-session-other-revoked")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(second_invalid.status(), StatusCode::UNAUTHORIZED);

    let revoked_current = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/v1/auth/sessions/{current_id}"))
                .header("origin", "http://127.0.0.1:3000")
                .header("cookie", &first_cookies)
                .header("x-csrf-token", &first_csrf)
                .header("idempotency-key", "session-revoke-current-001")
                .header("x-correlation-id", "http-session-revoke-current")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(revoked_current.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        revoked_current
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .count(),
        2
    );

    let current_invalid = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/auth/session")
                .header("cookie", first_cookies)
                .header("x-correlation-id", "http-session-current-revoked")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(current_invalid.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn device_authorization_approves_once_and_issues_bound_session() {
    let root = root("device-flow");
    let _ = fs::remove_dir_all(&root);
    let state = ApiState::new(ProfileService::new(LocalRepository::open(&root).unwrap()));
    let approval_token = state.issue_token("acct_device_http").unwrap();
    let app: Router = router(state.clone());

    let started = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/v1/auth/device/start",
            json!({"deviceId":"desktop-http-1","clientName":"BugLogin Desktop"}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(started.status(), StatusCode::OK);
    let start_body: Value = serde_json::from_slice(
        &axum::body::to_bytes(started.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        start_body["verificationUri"],
        "http://127.0.0.1:3000/device"
    );
    let device_code = start_body["deviceCode"].as_str().unwrap().to_owned();
    let user_code = start_body["userCode"].as_str().unwrap().to_owned();

    let missing_auth = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/v1/auth/device/approve",
            json!({"userCode":user_code}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(missing_auth.status(), StatusCode::UNAUTHORIZED);

    let approved = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/v1/auth/device/approve",
            json!({"userCode":user_code}),
            Some(&approval_token),
        ))
        .await
        .unwrap();
    assert_eq!(approved.status(), StatusCode::OK);

    let token_response = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/v1/auth/device/token",
            json!({"deviceCode":device_code}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(token_response.status(), StatusCode::OK);
    let token_body: Value = serde_json::from_slice(
        &axum::body::to_bytes(token_response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    let session = token_body["accessToken"].as_str().unwrap();
    assert_eq!(token_body["tokenType"], "Bearer");
    assert_eq!(
        state
            .authenticate_token(session, u64::MAX)
            .unwrap_err()
            .code,
        "session_expired"
    );

    let reused = app
        .oneshot(json_request(
            "POST",
            "/v1/auth/device/token",
            json!({"deviceCode":device_code}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(reused.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn device_token_reports_pending_and_slow_down() {
    let root = root("device-pending");
    let _ = fs::remove_dir_all(&root);
    let state = ApiState::new(ProfileService::new(LocalRepository::open(&root).unwrap()));
    let app: Router = router(state);
    let started = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/v1/auth/device/start",
            json!({"deviceId":"desktop-http-2","clientName":"BugLogin Desktop"}),
            None,
        ))
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(started.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    let device_code = body["deviceCode"].as_str().unwrap();
    let first = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/v1/auth/device/token",
            json!({"deviceCode":device_code}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::BAD_REQUEST);
    let first_body: Value = serde_json::from_slice(
        &axum::body::to_bytes(first.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(first_body["code"], "authorization_pending");

    let second = app
        .oneshot(json_request(
            "POST",
            "/v1/auth/device/token",
            json!({"deviceCode":device_code}),
            None,
        ))
        .await
        .unwrap();
    let second_body: Value = serde_json::from_slice(
        &axum::body::to_bytes(second.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(second_body["code"], "slow_down");
}

#[derive(Default)]
struct TestRecoveryDelivery {
    messages: Mutex<Vec<RecoveryDeliveryMessage>>,
}

impl RecoveryDelivery for TestRecoveryDelivery {
    fn available(&self) -> bool {
        true
    }

    fn deliver(&self, message: RecoveryDeliveryMessage) -> Result<(), Problem> {
        self.messages.lock().unwrap().push(message);
        Ok(())
    }
}

#[tokio::test]
async fn web_recovery_is_generic_single_use_and_revokes_sessions() {
    let root = root("web-recovery");
    let _ = fs::remove_dir_all(&root);
    let delivery = Arc::new(TestRecoveryDelivery::default());
    let state = ApiState::new(ProfileService::new(LocalRepository::open(&root).unwrap()))
        .with_recovery_delivery(delivery.clone());
    state
        .provision_web_identity(
            "acct_recovery_http",
            "Recovery.Owner",
            "recovery.owner@example.com",
            "old correct horse battery staple",
        )
        .unwrap();
    let app: Router = router(state);
    let login = app
        .clone()
        .oneshot(web_json_request(
            "POST",
            "/v1/auth/login",
            json!({
                "identifier": "recovery.owner",
                "password": "old correct horse battery staple"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(login.status(), StatusCode::OK);
    let (cookies, _) = auth_cookies(&login);

    let unknown = app
        .clone()
        .oneshot(web_json_request(
            "POST",
            "/v1/auth/recovery/request",
            json!({"identifier": "missing@example.com"}),
        ))
        .await
        .unwrap();
    assert_eq!(unknown.status(), StatusCode::ACCEPTED);
    let known = app
        .clone()
        .oneshot(web_json_request(
            "POST",
            "/v1/auth/recovery/request",
            json!({"identifier": "recovery.owner@example.com"}),
        ))
        .await
        .unwrap();
    assert_eq!(known.status(), StatusCode::ACCEPTED);
    let token = delivery
        .messages
        .lock()
        .unwrap()
        .pop()
        .unwrap()
        .raw_token
        .clone();

    let completed = app
        .clone()
        .oneshot(web_json_idempotent_request(
            "POST",
            "/v1/auth/recovery/complete",
            json!({
                "token": token.clone(),
                "newPassword": "new correct horse battery staple"
            }),
            "recovery-http-complete-0001",
        ))
        .await
        .unwrap();
    assert_eq!(completed.status(), StatusCode::NO_CONTENT);

    let revoked = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/auth/session")
                .header("cookie", cookies)
                .header("x-correlation-id", "http-recovery-revoked")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(revoked.status(), StatusCode::UNAUTHORIZED);

    let replay = app
        .clone()
        .oneshot(web_json_idempotent_request(
            "POST",
            "/v1/auth/recovery/complete",
            json!({
                "token": token,
                "newPassword": "third password value"
            }),
            "recovery-http-complete-0002",
        ))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::BAD_REQUEST);

    let old_login = app
        .clone()
        .oneshot(web_json_request(
            "POST",
            "/v1/auth/login",
            json!({
                "identifier": "recovery.owner",
                "password": "old correct horse battery staple"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(old_login.status(), StatusCode::UNAUTHORIZED);
    let new_login = app
        .oneshot(web_json_request(
            "POST",
            "/v1/auth/login",
            json!({
                "identifier": "recovery.owner",
                "password": "new correct horse battery staple"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(new_login.status(), StatusCode::OK);
}

#[tokio::test]
async fn web_api_tokens_are_one_time_scoped_listable_and_revocable() {
    let root = root("web-api-token");
    let _ = fs::remove_dir_all(&root);
    let state = ApiState::new(ProfileService::new(LocalRepository::open(&root).unwrap()));
    state
        .provision_web_identity(
            "acct_api_token_http",
            "Token.Owner",
            "token.owner@example.com",
            "correct horse battery staple",
        )
        .unwrap();
    let probe = state.clone();
    let app: Router = router(state);
    let login = app
        .clone()
        .oneshot(web_json_request(
            "POST",
            "/v1/auth/login",
            json!({
                "identifier": "token.owner",
                "password": "correct horse battery staple"
            }),
        ))
        .await
        .unwrap();
    let (cookies, csrf) = auth_cookies(&login);
    let expires_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 3600;
    let created = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/auth/api-tokens")
                .header("content-type", "application/json")
                .header("origin", "http://127.0.0.1:3000")
                .header("cookie", &cookies)
                .header("x-csrf-token", &csrf)
                .header("x-correlation-id", "http-api-token-create")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "name": "MCP automation",
                        "scopes": ["profile.read:read"],
                        "expiresAt": expires_at
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(created.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    let raw = body["token"].as_str().unwrap().to_owned();
    let token_id = body["tokenRecord"]["id"].as_str().unwrap().to_owned();
    assert!(raw.starts_with("blapi_"));
    let principal = probe.authenticate_token(&raw, expires_at - 1).unwrap();
    assert_eq!(
        principal.token_scopes,
        Some(std::collections::BTreeSet::from([
            "profile.read:read".into()
        ]))
    );

    let listed = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/auth/api-tokens")
                .header("cookie", &cookies)
                .header("x-correlation-id", "http-api-token-list")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(listed.status(), StatusCode::OK);
    let listed_bytes = axum::body::to_bytes(listed.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(!String::from_utf8_lossy(&listed_bytes).contains(&raw));

    let revoked = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/v1/auth/api-tokens/{token_id}"))
                .header("origin", "http://127.0.0.1:3000")
                .header("cookie", cookies)
                .header("x-csrf-token", csrf)
                .header("idempotency-key", "api-token-revoke-http-0001")
                .header("x-correlation-id", "http-api-token-revoke")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(revoked.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        probe
            .authenticate_token(&raw, expires_at - 1)
            .unwrap_err()
            .code,
        "invalid_api_token"
    );
}
