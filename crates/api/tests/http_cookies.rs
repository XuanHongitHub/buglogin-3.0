use std::{fs, path::PathBuf};

use axum::{body::Body, http::Request};
use buglogin_api::http::{ApiState, router};
use buglogin_browser::profile::ProfileService;
use buglogin_domain::{
    entities::{Profile, ProfileState, Runtime, SyncMode},
    ids::{AccountId, GrantId, ProfileId, WorkspaceId},
    permissions::{Action, Capability, Effect, Grant, ResourceScope},
};
use buglogin_storage::local::LocalRepository;
use tower::ServiceExt;

fn root() -> PathBuf {
    std::env::temp_dir().join(format!("buglogin-http-cookies-{}", std::process::id()))
}

fn profile() -> Profile {
    Profile {
        id: ProfileId::new("prf_cookie_http").unwrap(),
        name: "Cookie HTTP".into(),
        runtime: Runtime::Wayfern,
        runtime_version: None,
        runtime_config: None,
        state: ProfileState::Created,
        archived: false,
        fingerprint: None,
        note: None,
        proxy_id: None,
        vpn_id: None,
        group_ids: vec![],
        tags: vec![],
        extension_group_id: None,
        sync_mode: Some(SyncMode::Local),
        created_at: "2026-08-22T00:00:00Z".into(),
        updated_at: "2026-08-22T00:00:00Z".into(),
    }
}

#[tokio::test]
async fn cookie_list_rest_returns_metadata_envelope_without_values() {
    let root = root();
    let _ = fs::remove_dir_all(&root);
    let repository = LocalRepository::open(&root).unwrap();
    let state = ApiState::new(ProfileService::new(repository.clone()));
    let token = state.issue_token("acct_cookie_http").unwrap();
    repository.create(profile()).unwrap();
    state.grants.write().unwrap().push(Grant {
        id: GrantId::new("grant_cookie_http").unwrap(),
        account_id: Some(AccountId::new("acct_cookie_http").unwrap()),
        member_id: None,
        capability: Capability::new("CAP-COOK-001").unwrap(),
        action: Action::Read,
        scope: ResourceScope {
            workspace_id: Some(WorkspaceId::new("ws_cookie_http").unwrap()),
            profile_id: Some(ProfileId::new("prf_cookie_http").unwrap()),
            ..ResourceScope::default()
        },
        effect: Effect::Allow,
    });

    let response = router(state)
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/profiles/prf_cookie_http/cookies?workspaceId=ws_cookie_http")
                .header("authorization", format!("Bearer {token}"))
                .header("x-correlation-id", "cookie-http-list-0001")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["generation"], 0);
    assert_eq!(value["cookies"], serde_json::json!([]));
    assert!(!value.to_string().contains("value"));
    fs::remove_dir_all(root).unwrap();
}
