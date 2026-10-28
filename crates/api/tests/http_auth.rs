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
use buglogin_api::http::{ApiState, FirstOwnerBootstrapInput, router};
use buglogin_api::services::recovery::{RecoveryDelivery, RecoveryDeliveryMessage};
use buglogin_browser::profile::ProfileService;
use buglogin_domain::errors::Problem;
use buglogin_storage::{control::MemoryControlRecordStore, local::LocalRepository};
use serde_json::{Value, json};
use tower::ServiceExt;

fn root(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!("buglogin-http-auth-{label}-{}", std::process::id()))
}

fn hosted_state_with_verified_owner(
    profiles: ProfileService,
    account_id: &str,
    username: &str,
    email: &str,
    password: &str,
) -> ApiState {
    let state =
        ApiState::hosted_with_control_store(profiles, MemoryControlRecordStore::default()).unwrap();
    state
        .bootstrap_first_owner(FirstOwnerBootstrapInput {
            account_id: Some(account_id.to_owned()),
            username: username.to_owned(),
            email: email.to_owned(),
            operator_attested_email: true,
            password: zeroize::Zeroizing::new(password.to_owned()),
            organization_name: "HTTP Auth Test Organization".into(),
            workspace_name: "HTTP Auth Test Workspace".into(),
            max_workspaces: 10,
            max_profiles: 1_000,
            max_members: 100,
            max_storage_bytes: 10 * 1024 * 1024 * 1024,
        })
        .unwrap();
    state
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

fn json_idempotent_request(
    method: &str,
    uri: &str,
    body: Value,
    token: &str,
    idempotency_key: &str,
) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {token}"))
        .header("idempotency-key", idempotency_key)
        .header("x-correlation-id", "http-device-key-test")
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

#[tokio::test]
async fn login_rejects_oversized_json_before_password_work() {
    let root = root("login-body-limit");
    let _ = fs::remove_dir_all(&root);
    let state = ApiState::new(ProfileService::new(LocalRepository::open(&root).unwrap()));
    let app = router(state);
    let response = app
        .oneshot(web_json_request(
            "POST",
            "/v1/auth/login",
            json!({
                "identifier": "a".repeat(9 * 1024),
                "password": "correct horse battery staple"
            }),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(body["code"], "payload_too_large");
    assert_eq!(body["correlationId"], "http-web-auth-test");
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

async fn web_login_when_password_capacity_is_busy(
    app: &Router,
    body: Value,
) -> axum::response::Response {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let response = app
            .clone()
            .oneshot(web_json_request("POST", "/v1/auth/login", body.clone()))
            .await
            .expect("web login request should complete");
        if response.status() != StatusCode::SERVICE_UNAVAILABLE {
            return response;
        }

        let bytes = axum::body::to_bytes(response.into_body(), 16 * 1024)
            .await
            .expect("capacity error body should be readable");
        let problem: Value = serde_json::from_slice(&bytes).expect("capacity error should be JSON");
        assert_eq!(
            problem["code"], "credential_capacity_exhausted",
            "unexpected service unavailable response: {problem}"
        );
        assert!(
            tokio::time::Instant::now() < deadline,
            "password hashing capacity remained busy for thirty seconds"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

async fn desktop_login_when_password_capacity_is_busy(
    app: &Router,
    body: Value,
) -> axum::response::Response {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let response = app
            .clone()
            .oneshot(json_request(
                "POST",
                "/v1/auth/desktop/login",
                body.clone(),
                None,
            ))
            .await
            .expect("desktop login request should complete");
        if response.status() != StatusCode::SERVICE_UNAVAILABLE {
            return response;
        }

        let bytes = axum::body::to_bytes(response.into_body(), 16 * 1024)
            .await
            .expect("capacity error body should be readable");
        let problem: Value = serde_json::from_slice(&bytes).expect("capacity error should be JSON");
        assert_eq!(
            problem["code"], "credential_capacity_exhausted",
            "unexpected service unavailable response: {problem}"
        );
        assert!(
            tokio::time::Instant::now() < deadline,
            "password hashing capacity remained busy for thirty seconds"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
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
        .oneshot({
            let mut logout = request("POST", "/v1/auth/logout", replacement);
            logout.headers_mut().insert(
                "idempotency-key",
                "http-auth-logout-replacement-0001".parse().unwrap(),
            );
            logout
        })
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
async fn profile_key_envelope_route_requires_authentication_before_resource_lookup() {
    let root = root("profile-key-envelope-auth");
    let _ = fs::remove_dir_all(&root);
    let state = ApiState::new(ProfileService::new(LocalRepository::open(&root).unwrap()));
    let app = router(state);
    let request = Request::builder()
        .method("GET")
        .uri("/v1/profiles/prf_unknown/key-envelope")
        .header("x-correlation-id", "profile-key-envelope-auth-test")
        .body(Body::empty())
        .unwrap();

    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = String::from_utf8(body.to_vec()).unwrap();
    assert!(!body.contains("keyVersion"));
    assert!(!body.contains("ciphertext"));

    let rotation_request = Request::builder()
        .method("GET")
        .uri("/v1/profiles/prf_unknown/key-rotation-recipients")
        .header("x-correlation-id", "profile-key-rotation-auth-test")
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(rotation_request).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = String::from_utf8(body.to_vec()).unwrap();
    assert!(!body.contains("deviceKeys"));
    assert!(!body.contains("publicKey"));
    assert!(!body.contains("fingerprint"));
}

#[tokio::test]
async fn web_login_uses_secure_cookies_session_projection_and_csrf_logout() {
    let root = root("web-login");
    let _ = fs::remove_dir_all(&root);
    let state = hosted_state_with_verified_owner(
        ProfileService::new(LocalRepository::open(&root).unwrap()),
        "acct_web_http",
        "Web.Owner",
        "web.owner@example.com",
        "correct horse battery staple",
    );
    let app: Router = router(state);

    let login = web_login_when_password_capacity_is_busy(
        &app,
        json!({
            "identifier": "WEB.OWNER",
            "password": "correct horse battery staple"
        }),
    )
    .await;
    assert_eq!(login.status(), StatusCode::OK);
    assert_eq!(login.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(login.headers()[header::PRAGMA], "no-cache");
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
    assert_eq!(session.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(session.headers()[header::PRAGMA], "no-cache");

    let missing_csrf = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/auth/logout")
                .header("origin", "http://127.0.0.1:3000")
                .header("cookie", &cookie_header)
                .header("idempotency-key", "http-web-logout-missing-csrf-0001")
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
                .header("idempotency-key", "http-web-logout-test-key-0001")
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
async fn desktop_login_issues_device_bound_tokens_and_rejects_bad_credentials_generically() {
    let root = root("desktop-login");
    let _ = fs::remove_dir_all(&root);
    let state = hosted_state_with_verified_owner(
        ProfileService::new(LocalRepository::open(&root).unwrap()),
        "acct_desktop_login_http",
        "Desktop.Owner",
        "desktop.owner@example.com",
        "correct horse battery staple",
    );
    let probe = state.clone();
    let app: Router = router(state);

    let login = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/v1/auth/desktop/login",
            json!({
                "identifier": "desktop.owner@example.com",
                "password": "correct horse battery staple",
                "deviceId": "desktop-login-device-01"
            }),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(login.status(), StatusCode::OK);
    assert_eq!(login.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(login.headers()[header::PRAGMA], "no-cache");
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(login.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    let access_token = body["accessToken"].as_str().unwrap();
    assert!(!body["refreshToken"].as_str().unwrap().is_empty());
    assert_eq!(body["tokenType"], "Bearer");
    let expires_at = body["expiresAt"].as_u64().unwrap();
    let principal = probe
        .authenticate_token(access_token, expires_at - 1)
        .unwrap();
    assert_eq!(
        principal.account_id.as_ref().unwrap().as_str(),
        "acct_desktop_login_http"
    );
    let account_id = buglogin_domain::ids::AccountId::new("acct_desktop_login_http").unwrap();
    let sessions = probe
        .auth
        .read()
        .unwrap()
        .list_sessions(&account_id, None)
        .unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].kind, buglogin_api::auth::SessionKind::Desktop);
    assert_eq!(
        sessions[0].device_id.as_deref(),
        Some("desktop-login-device-01")
    );

    let wrong_password = desktop_login_when_password_capacity_is_busy(
        &app,
        json!({
            "identifier": "desktop.owner@example.com",
            "password": "incorrect password",
            "deviceId": "desktop-login-device-01"
        }),
    )
    .await;
    assert_eq!(wrong_password.status(), StatusCode::UNAUTHORIZED);
    let wrong_body: Value = serde_json::from_slice(
        &axum::body::to_bytes(wrong_password.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(wrong_body["code"], "invalid_credentials");
    assert!(wrong_body.get("accountId").is_none());

    let unknown_account = desktop_login_when_password_capacity_is_busy(
        &app,
        json!({
            "identifier": "not-a-real-user@example.com",
            "password": "incorrect password",
            "deviceId": "desktop-login-device-01"
        }),
    )
    .await;
    assert_eq!(unknown_account.status(), StatusCode::UNAUTHORIZED);
    let unknown_body: Value = serde_json::from_slice(
        &axum::body::to_bytes(unknown_account.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(unknown_body["code"], wrong_body["code"]);
    assert_eq!(unknown_body["title"], wrong_body["title"]);
}

#[tokio::test]
async fn desktop_login_rejects_invalid_device_identity() {
    let root = root("desktop-login-invalid-device");
    let _ = fs::remove_dir_all(&root);
    let state = hosted_state_with_verified_owner(
        ProfileService::new(LocalRepository::open(&root).unwrap()),
        "acct_desktop_invalid_device",
        "Desktop.InvalidDevice",
        "desktop.invalid-device@example.com",
        "correct horse battery staple",
    );
    let app: Router = router(state);

    let response = app
        .oneshot(json_request(
            "POST",
            "/v1/auth/desktop/login",
            json!({
                "identifier": "desktop.invalid-device@example.com",
                "password": "correct horse battery staple",
                "deviceId": "bad id"
            }),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(body["code"], "invalid_device_id");
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
    let state = hosted_state_with_verified_owner(
        ProfileService::new(LocalRepository::open(&root).unwrap()),
        "acct_sessions_http",
        "Session.Owner",
        "session.owner@example.com",
        "correct horse battery staple",
    );
    let app: Router = router(state);
    let login_body = json!({
        "identifier": "session.owner",
        "password": "correct horse battery staple"
    });
    let first = web_login_when_password_capacity_is_busy(&app, login_body.clone()).await;
    let (first_cookies, first_csrf) = auth_cookies(&first);
    let second = web_login_when_password_capacity_is_busy(&app, login_body).await;
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
    assert_eq!(started.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(started.headers()[header::PRAGMA], "no-cache");
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
    assert_eq!(
        token_response.headers().get("cache-control").unwrap(),
        "no-store"
    );
    let token_body: Value = serde_json::from_slice(
        &axum::body::to_bytes(token_response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    let session = token_body["accessToken"].as_str().unwrap();
    let refresh_token = token_body["refreshToken"].as_str().unwrap();
    assert!(!refresh_token.is_empty());
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
    let reused_body: Value = serde_json::from_slice(
        &axum::body::to_bytes(reused.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(reused_body["code"], "invalid_grant");
}

#[tokio::test]
async fn device_refresh_route_rotates_credentials_and_replay_revokes_family() {
    let root = root("device-refresh-route");
    let _ = fs::remove_dir_all(&root);
    let state = ApiState::new(ProfileService::new(LocalRepository::open(&root).unwrap()));
    let approval_token = state.issue_token("acct_device_refresh_http").unwrap();
    let app: Router = router(state.clone());
    let start = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/v1/auth/device/start",
            json!({"deviceId":"desktop-refresh-http","clientName":"BugLogin Desktop"}),
            None,
        ))
        .await
        .unwrap();
    let start_body: Value = serde_json::from_slice(
        &axum::body::to_bytes(start.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    let device_code = start_body["deviceCode"].as_str().unwrap().to_owned();
    let user_code = start_body["userCode"].as_str().unwrap().to_owned();
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
    let issued = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/v1/auth/device/token",
            json!({"deviceCode":device_code}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(issued.status(), StatusCode::OK);
    let issued_body: Value = serde_json::from_slice(
        &axum::body::to_bytes(issued.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    let access_token = issued_body["accessToken"].as_str().unwrap().to_owned();
    let refresh_token = issued_body["refreshToken"].as_str().unwrap().to_owned();
    let request = || {
        json_request(
            "POST",
            "/v1/auth/device/refresh",
            json!({"deviceId":"desktop-refresh-http","refreshToken":refresh_token}),
            None,
        )
    };
    let refreshed = app.clone().oneshot(request()).await.unwrap();
    assert_eq!(refreshed.status(), StatusCode::OK);
    assert_eq!(
        refreshed.headers().get("cache-control").unwrap(),
        "no-store"
    );
    let refreshed_body: Value = serde_json::from_slice(
        &axum::body::to_bytes(refreshed.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    let next_access = refreshed_body["accessToken"].as_str().unwrap();
    let next_refresh = refreshed_body["refreshToken"].as_str().unwrap();
    assert_ne!(next_access, access_token);
    assert_ne!(next_refresh, refresh_token);
    let current = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    assert!(state.authenticate_token(next_access, current).is_ok());

    let replay = app.oneshot(request()).await.unwrap();
    assert_eq!(replay.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        state
            .authenticate_token(next_access, current)
            .unwrap_err()
            .code,
        "session_revoked"
    );
}

#[tokio::test]
async fn device_encryption_key_registration_requires_bound_device_session_and_is_immutable() {
    let root = root("device-key-register");
    let _ = fs::remove_dir_all(&root);
    let state = ApiState::new(ProfileService::new(LocalRepository::open(&root).unwrap()));
    let account_token = state.issue_token("acct_device_key_http").unwrap();
    let app: Router = router(state.clone());
    let start = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/v1/auth/device/start",
            json!({"deviceId":"desktop-sharekey-http","clientName":"BugLogin Desktop"}),
            None,
        ))
        .await
        .unwrap();
    let start_body: Value = serde_json::from_slice(
        &axum::body::to_bytes(start.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    let device_code = start_body["deviceCode"].as_str().unwrap().to_owned();
    let user_code = start_body["userCode"].as_str().unwrap().to_owned();
    let approved = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/v1/auth/device/approve",
            json!({"userCode":user_code}),
            Some(&account_token),
        ))
        .await
        .unwrap();
    assert_eq!(approved.status(), StatusCode::OK);
    let issued = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/v1/auth/device/token",
            json!({"deviceCode":device_code}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(issued.status(), StatusCode::OK);
    let issued_body: Value = serde_json::from_slice(
        &axum::body::to_bytes(issued.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    let device_token = issued_body["accessToken"].as_str().unwrap().to_owned();
    let private_key = buglogin_sync::crypto::DevicePrivateKey::generate().unwrap();
    let public_key = private_key
        .public_key()
        .unwrap()
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let body = json!({"publicKey":public_key});

    let forbidden = app
        .clone()
        .oneshot(json_idempotent_request(
            "PUT",
            "/v1/auth/device/encryption-key",
            body.clone(),
            &account_token,
            "device-key-account-0001",
        ))
        .await
        .unwrap();
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);

    let registered = app
        .clone()
        .oneshot(json_idempotent_request(
            "PUT",
            "/v1/auth/device/encryption-key",
            body.clone(),
            &device_token,
            "device-key-register-0001",
        ))
        .await
        .unwrap();
    assert_eq!(registered.status(), StatusCode::OK);
    let registered_body: Value = serde_json::from_slice(
        &axum::body::to_bytes(registered.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(registered_body["deviceId"], "desktop-sharekey-http");
    assert_eq!(registered_body["fingerprint"].as_str().unwrap().len(), 64);

    let replay = app
        .clone()
        .oneshot(json_idempotent_request(
            "PUT",
            "/v1/auth/device/encryption-key",
            body.clone(),
            &device_token,
            "device-key-register-0001",
        ))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::OK);

    let replacement = app
        .clone()
        .oneshot(json_idempotent_request(
            "PUT",
            "/v1/auth/device/encryption-key",
            json!({"publicKey":"ab".repeat(32)}),
            &device_token,
            "device-key-register-0002",
        ))
        .await
        .unwrap();
    assert_eq!(replacement.status(), StatusCode::CONFLICT);
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
    assert_eq!(second_body["fields"]["retryAfterSeconds"], 10);
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
    let state = hosted_state_with_verified_owner(
        ProfileService::new(LocalRepository::open(&root).unwrap()),
        "acct_recovery_http",
        "Recovery.Owner",
        "recovery.owner@example.com",
        "old correct horse battery staple",
    )
    .with_recovery_delivery(delivery.clone());
    let app: Router = router(state);
    let login = web_login_when_password_capacity_is_busy(
        &app,
        json!({
            "identifier": "recovery.owner",
            "password": "old correct horse battery staple"
        }),
    )
    .await;
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
    let new_login = web_login_when_password_capacity_is_busy(
        &app,
        json!({
            "identifier": "recovery.owner",
            "password": "new correct horse battery staple"
        }),
    )
    .await;
    assert_eq!(new_login.status(), StatusCode::OK);
}

#[tokio::test]
async fn web_api_tokens_are_one_time_scoped_listable_and_revocable() {
    let root = root("web-api-token");
    let _ = fs::remove_dir_all(&root);
    let state = hosted_state_with_verified_owner(
        ProfileService::new(LocalRepository::open(&root).unwrap()),
        "acct_api_token_http",
        "Token.Owner",
        "token.owner@example.com",
        "correct horse battery staple",
    );
    let probe = state.clone();
    let app: Router = router(state);
    let login = web_login_when_password_capacity_is_busy(
        &app,
        json!({
            "identifier": "token.owner",
            "password": "correct horse battery staple"
        }),
    )
    .await;
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
    assert_eq!(created.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(created.headers()[header::PRAGMA], "no-cache");
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

#[tokio::test]
async fn api_tokens_cannot_manage_credentials_or_approve_devices() {
    let root = root("api-token-credential-boundary");
    let _ = fs::remove_dir_all(&root);
    let state = hosted_state_with_verified_owner(
        ProfileService::new(LocalRepository::open(&root).unwrap()),
        "acct_api_token_boundary_http",
        "Api.Token.Boundary",
        "api.token.boundary@example.com",
        "correct horse battery staple",
    );
    let session = state.issue_token("acct_api_token_boundary_http").unwrap();
    let app = router(state);
    let expires_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 3600;
    let created = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/v1/auth/api-tokens",
            json!({
                "name": "Narrow proxy reader",
                "scopes": ["CAP-NET-001:read"],
                "expiresAt": expires_at
            }),
            Some(&session),
        ))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);
    let created: Value = serde_json::from_slice(
        &axum::body::to_bytes(created.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    let api_token = created["token"].as_str().unwrap().to_owned();
    let token_id = created["tokenRecord"]["id"].as_str().unwrap().to_owned();

    let started = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/v1/auth/device/start",
            json!({"deviceId":"desktop-boundary-1","clientName":"BugLogin Desktop"}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(started.status(), StatusCode::OK);
    let started: Value = serde_json::from_slice(
        &axum::body::to_bytes(started.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    let user_code = started["userCode"].as_str().unwrap().to_owned();

    let attempts = [
        json_request(
            "POST",
            "/v1/auth/api-tokens",
            json!({
                "name": "Escalated",
                "scopes": ["CAP-ADM-001:*"],
                "expiresAt": expires_at
            }),
            Some(&api_token),
        ),
        request("GET", "/v1/auth/api-tokens", &api_token),
        json_idempotent_request(
            "DELETE",
            &format!("/v1/auth/api-tokens/{token_id}"),
            json!({}),
            &api_token,
            "api-token-boundary-revoke-0001",
        ),
        json_request(
            "POST",
            "/v1/auth/device/approve",
            json!({"userCode": user_code}),
            Some(&api_token),
        ),
        json_idempotent_request(
            "DELETE",
            "/v1/auth/sessions/sess_api_token_boundary",
            json!({}),
            &api_token,
            "api-token-boundary-session-0001",
        ),
    ];
    for attempt in attempts {
        let label = format!("{} {}", attempt.method(), attempt.uri());
        let response = app.clone().oneshot(attempt).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{label}");
        let body: Value = serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            body["code"], "api_token_credential_management_forbidden",
            "{label}"
        );
    }

    let still_listed = app
        .oneshot(request("GET", "/v1/auth/api-tokens", &session))
        .await
        .unwrap();
    assert_eq!(still_listed.status(), StatusCode::OK);
    let still_listed: Value = serde_json::from_slice(
        &axum::body::to_bytes(still_listed.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(still_listed["tokens"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn desktop_session_bearer_can_manage_account_api_tokens() {
    let root = root("desktop-api-token-management");
    let _ = fs::remove_dir_all(&root);
    let state = hosted_state_with_verified_owner(
        ProfileService::new(LocalRepository::open(&root).unwrap()),
        "acct_desktop_api_token_http",
        "Desktop.Token.Owner",
        "desktop.token.owner@example.com",
        "correct horse battery staple",
    );
    let probe = state.clone();
    let session = state.issue_token("acct_desktop_api_token_http").unwrap();
    let app = router(state);
    let expires_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 3600;

    let created = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/v1/auth/api-tokens",
            json!({
                "name": "Desktop MCP",
                "scopes": ["profile.read:read"],
                "expiresAt": expires_at
            }),
            Some(&session),
        ))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);
    let created: Value = serde_json::from_slice(
        &axum::body::to_bytes(created.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    let api_token = created["token"].as_str().unwrap().to_owned();
    let token_id = created["tokenRecord"]["id"].as_str().unwrap();

    let listed = app
        .clone()
        .oneshot(request("GET", "/v1/auth/api-tokens", &session))
        .await
        .unwrap();
    assert_eq!(listed.status(), StatusCode::OK);
    let listed: Value = serde_json::from_slice(
        &axum::body::to_bytes(listed.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert!(
        listed["tokens"]
            .as_array()
            .unwrap()
            .iter()
            .any(|token| token["id"] == token_id)
    );

    let revoked = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/v1/auth/api-tokens/{token_id}"))
                .header("authorization", format!("Bearer {session}"))
                .header("idempotency-key", "desktop-api-token-revoke-0001")
                .header("x-correlation-id", "http-desktop-api-token-revoke")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(revoked.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        probe
            .authenticate_token(&api_token, expires_at - 1)
            .unwrap_err()
            .code,
        "invalid_api_token"
    );
}
