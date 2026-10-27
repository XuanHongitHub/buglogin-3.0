use std::{fs, path::PathBuf};

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use buglogin_api::http::{ApiState, router};
use buglogin_api::services::profile_hosted::HostedProfileService;
use buglogin_api::services::workspaces::WorkspaceService;
use buglogin_browser::profile::ProfileService;
use buglogin_domain::{
    account::Account,
    entities::{Profile, ProfileState, Runtime, SyncMode},
    ids::{AccountId, GrantId, OrganizationId, ProfileId, ProxyId, WorkspaceId},
    organization::{Organization, OrganizationPolicy},
    permissions::{Action, Capability, Effect, Grant, Principal, ResourceScope, Role},
    profile::ProfileResource,
    workspace::{Workspace, WorkspacePolicy},
};
use buglogin_storage::{
    control::{ControlRecordStore, MemoryControlRecordStore, PutControlRecord},
    local::LocalRepository,
};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use tower::ServiceExt;

fn root(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!("buglogin-http-{label}-{}", std::process::id()))
}

fn profile() -> Profile {
    Profile {
        id: ProfileId::from_str("prf_http_1").unwrap(),
        name: "HTTP profile".into(),
        runtime: Runtime::Wayfern,
        runtime_version: Some("1.0.0".into()),
        runtime_config: None,
        state: ProfileState::Created,
        archived: false,
        fingerprint: None,
        note: None,
        proxy_id: None,
        vpn_id: None,
        group_ids: vec![],
        tags: vec!["http".into()],
        extension_group_id: None,
        sync_mode: Some(SyncMode::Local),
        created_at: "2026-07-20T00:00:00Z".into(),
        updated_at: "2026-07-20T00:00:00Z".into(),
    }
}

fn request(method: &str, uri: &str, token: &str, body: Option<String>) -> Request<Body> {
    let builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .header("x-correlation-id", "http-test-1");
    match body {
        Some(body) => builder
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    }
}

fn lifecycle_request(
    method: &str,
    uri: &str,
    token: &str,
    key: &str,
    body: Option<String>,
) -> Request<Body> {
    let mut request = request(method, uri, token, body);
    request
        .headers_mut()
        .insert("idempotency-key", key.parse().unwrap());
    request
}

#[tokio::test]
async fn profile_http_flow_enforces_auth_scope_and_revision() {
    let root = root("profiles");
    let _ = fs::remove_dir_all(&root);
    let state = ApiState::new(ProfileService::new(LocalRepository::open(&root).unwrap()));
    let token = state.issue_token("acct_http").unwrap();
    let account = AccountId::from_str("acct_http").unwrap();
    let profile_id = ProfileId::from_str("prf_http_1").unwrap();
    let mut grants = state.grants.write().unwrap();
    for (capability, action) in [
        ("profile.create", Action::Create),
        ("profile.read", Action::Read),
        ("profile.archive", Action::Update),
        ("profile.restore", Action::Update),
        ("profile.delete", Action::Delete),
    ] {
        grants.push(Grant {
            id: GrantId::from_str(format!("grant_{}", capability.replace('.', "_")).as_str())
                .unwrap(),
            account_id: Some(account.clone()),
            member_id: None,
            capability: Capability::new(capability).unwrap(),
            action,
            scope: ResourceScope {
                profile_id: Some(profile_id.clone()),
                ..ResourceScope::default()
            },
            effect: Effect::Allow,
        });
    }
    drop(grants);
    let app: Router = router(state);

    let denied = app
        .clone()
        .oneshot(request("GET", "/v1/profiles/prf_http_1", &token, None))
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::NOT_FOUND);

    let create_body = serde_json::to_string(&profile()).unwrap();
    let missing_create_key = app
        .clone()
        .oneshot(request(
            "POST",
            "/v1/profiles",
            &token,
            Some(create_body.clone()),
        ))
        .await
        .unwrap();
    assert_eq!(missing_create_key.status(), StatusCode::BAD_REQUEST);
    let missing_profile = app
        .clone()
        .oneshot(request("GET", "/v1/profiles/prf_http_1", &token, None))
        .await
        .unwrap();
    assert_eq!(missing_profile.status(), StatusCode::NOT_FOUND);

    let create_request = || {
        lifecycle_request(
            "POST",
            "/v1/profiles",
            &token,
            "profile-create-http-0001",
            Some(create_body.clone()),
        )
    };
    let created = app.clone().oneshot(create_request()).await.unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);
    let created_body = axum::body::to_bytes(created.into_body(), usize::MAX)
        .await
        .unwrap();
    let replay = app.clone().oneshot(create_request()).await.unwrap();
    assert_eq!(replay.status(), StatusCode::CREATED);
    let replay_body = axum::body::to_bytes(replay.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(created_body, replay_body);

    let mut changed_profile = profile();
    changed_profile.name = "Changed profile".to_owned();
    let conflict = app
        .clone()
        .oneshot(lifecycle_request(
            "POST",
            "/v1/profiles",
            &token,
            "profile-create-http-0001",
            Some(serde_json::to_string(&changed_profile).unwrap()),
        ))
        .await
        .unwrap();
    assert_eq!(conflict.status(), StatusCode::CONFLICT);

    let listed = app
        .clone()
        .oneshot(request("GET", "/v1/profiles?limit=10", &token, None))
        .await
        .unwrap();
    assert_eq!(listed.status(), StatusCode::OK);
    let listed_body: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(listed.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(listed_body["items"][0]["value"]["id"], "prf_http_1");
    assert_eq!(listed_body["items"][0]["revision"], 1);
    assert!(listed_body["nextCursor"].is_null());

    let missing_archive_body = app
        .clone()
        .oneshot(request(
            "PUT",
            "/v1/profiles/prf_http_1/archive",
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        missing_archive_body.status(),
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    );
    let unknown_archive_field = app
        .clone()
        .oneshot(request(
            "PUT",
            "/v1/profiles/prf_http_1/archive",
            &token,
            Some(json!({"revision": 1, "unexpected": true}).to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(
        unknown_archive_field.status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );

    let archived = app
        .clone()
        .oneshot(request(
            "PUT",
            "/v1/profiles/prf_http_1/archive",
            &token,
            Some(json!({"revision": 1}).to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(archived.status(), StatusCode::OK);

    let stale = app
        .clone()
        .oneshot(request(
            "PUT",
            "/v1/profiles/prf_http_1/restore",
            &token,
            Some(json!({"revision": 1}).to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(stale.status(), StatusCode::CONFLICT);

    let missing_idempotency_key = app
        .clone()
        .oneshot(request(
            "DELETE",
            "/v1/profiles/prf_http_1",
            &token,
            Some(json!({"revision": 2, "confirmation": "QUARANTINE_PROFILE_DATA"}).to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(missing_idempotency_key.status(), StatusCode::BAD_REQUEST);

    let delete_request = || {
        lifecycle_request(
            "DELETE",
            "/v1/profiles/prf_http_1",
            &token,
            "profile-delete-http-0001",
            Some(json!({"revision": 2, "confirmation": "QUARANTINE_PROFILE_DATA"}).to_string()),
        )
    };
    let unknown_delete_field = app
        .clone()
        .oneshot(lifecycle_request(
            "DELETE",
            "/v1/profiles/prf_http_1",
            &token,
            "profile-delete-extra-0001",
            Some(
                json!({
                    "revision": 2,
                    "confirmation": "QUARANTINE_PROFILE_DATA",
                    "unexpected": true
                })
                .to_string(),
            ),
        ))
        .await
        .unwrap();
    assert_eq!(
        unknown_delete_field.status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let deleted = app.clone().oneshot(delete_request()).await.unwrap();
    assert_eq!(deleted.status(), StatusCode::OK);
    let deleted_body = axum::body::to_bytes(deleted.into_body(), usize::MAX)
        .await
        .unwrap();
    let replay = app.clone().oneshot(delete_request()).await.unwrap();
    assert_eq!(replay.status(), StatusCode::OK);
    let replay_body = axum::body::to_bytes(replay.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(deleted_body, replay_body);
    let conflicting_replay = app
        .clone()
        .oneshot(lifecycle_request(
            "DELETE",
            "/v1/profiles/prf_http_1",
            &token,
            "profile-delete-http-0001",
            Some(json!({"revision": 3, "confirmation": "QUARANTINE_PROFILE_DATA"}).to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(conflicting_replay.status(), StatusCode::CONFLICT);
    let missing = app
        .clone()
        .oneshot(request("GET", "/v1/profiles/prf_http_1", &token, None))
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn profile_list_uses_stable_cursor_pages() {
    let root = root("profile-pages");
    let _ = fs::remove_dir_all(&root);
    let state = ApiState::new(ProfileService::new(LocalRepository::open(&root).unwrap()));
    let account = AccountId::from_str("acct_profile_pages").unwrap();
    let token = state.issue_token(account.as_str()).unwrap();
    for id in ["prf_page_a", "prf_page_b", "prf_page_c"] {
        let mut value = profile();
        value.id = ProfileId::from_str(id).unwrap();
        state.profiles.create(value).unwrap();
        state.grants.write().unwrap().push(Grant {
            id: GrantId::from_str(format!("grant_{id}_read").as_str()).unwrap(),
            account_id: Some(account.clone()),
            member_id: None,
            capability: Capability::new("profile.read").unwrap(),
            action: Action::Read,
            scope: ResourceScope {
                profile_id: Some(ProfileId::from_str(id).unwrap()),
                ..ResourceScope::default()
            },
            effect: Effect::Allow,
        });
    }
    let app = router(state);
    let invalid_limit = app
        .clone()
        .oneshot(request("GET", "/v1/profiles?limit=101", &token, None))
        .await
        .unwrap();
    assert_eq!(invalid_limit.status(), StatusCode::BAD_REQUEST);
    let invalid_cursor = app
        .clone()
        .oneshot(request(
            "GET",
            "/v1/profiles?cursor=invalid%20cursor",
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(invalid_cursor.status(), StatusCode::BAD_REQUEST);
    let first = app
        .clone()
        .oneshot(request("GET", "/v1/profiles?limit=2", &token, None))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let first: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(first.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(first["items"].as_array().unwrap().len(), 2);
    let cursor = first["nextCursor"].as_str().unwrap();
    let second = app
        .oneshot(request(
            "GET",
            &format!("/v1/profiles?limit=2&cursor={cursor}"),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::OK);
    let second: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(second.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(second["items"].as_array().unwrap().len(), 1);
    assert!(second["nextCursor"].is_null());
    assert_eq!(second["items"][0]["value"]["id"], "prf_page_c");
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn local_profile_create_recovers_after_storage_reopen() {
    let root = root("profile-create-restart");
    let _ = fs::remove_dir_all(&root);
    let account = AccountId::from_str("acct_create_restart").unwrap();
    let profile_id = ProfileId::from_str("prf_http_1").unwrap();
    let first_state = ApiState::new(ProfileService::new(LocalRepository::open(&root).unwrap()));
    let first_token = first_state.issue_token(account.as_str()).unwrap();
    first_state.grants.write().unwrap().push(Grant {
        id: GrantId::from_str("grant_create_restart_first").unwrap(),
        account_id: Some(account.clone()),
        member_id: None,
        capability: Capability::new("profile.create").unwrap(),
        action: Action::Create,
        scope: ResourceScope {
            profile_id: Some(profile_id.clone()),
            ..ResourceScope::default()
        },
        effect: Effect::Allow,
    });
    let create_body = serde_json::to_string(&profile()).unwrap();
    let first_app = router(first_state);
    let first_response = first_app
        .oneshot(lifecycle_request(
            "POST",
            "/v1/profiles",
            &first_token,
            "profile-create-restart-0001",
            Some(create_body.clone()),
        ))
        .await
        .unwrap();
    assert_eq!(first_response.status(), StatusCode::CREATED);
    let first_body = axum::body::to_bytes(first_response.into_body(), usize::MAX)
        .await
        .unwrap();

    let restarted_state = ApiState::new(ProfileService::new(LocalRepository::open(&root).unwrap()));
    let restarted_token = restarted_state.issue_token(account.as_str()).unwrap();
    restarted_state.grants.write().unwrap().push(Grant {
        id: GrantId::from_str("grant_create_restart_second").unwrap(),
        account_id: Some(account),
        member_id: None,
        capability: Capability::new("profile.create").unwrap(),
        action: Action::Create,
        scope: ResourceScope {
            profile_id: Some(profile_id),
            ..ResourceScope::default()
        },
        effect: Effect::Allow,
    });
    let restarted_response = router(restarted_state)
        .oneshot(lifecycle_request(
            "POST",
            "/v1/profiles",
            &restarted_token,
            "profile-create-restart-0001",
            Some(create_body),
        ))
        .await
        .unwrap();
    let restarted_status = restarted_response.status();
    let restarted_body = axum::body::to_bytes(restarted_response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        restarted_status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&restarted_body)
    );
    assert_eq!(first_body, restarted_body);
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn canonical_profile_create_dto_is_server_normalized_and_idempotent() {
    let root = root("profile-create-command-dto");
    let _ = fs::remove_dir_all(&root);
    let state = ApiState::new(ProfileService::new(LocalRepository::open(&root).unwrap()));
    let token = state.issue_token("acct_create_command_dto").unwrap();
    let profile_id = ProfileId::from_str("prf_command_dto").unwrap();
    state.grants.write().unwrap().push(Grant {
        id: GrantId::from_str("grant_create_command_dto").unwrap(),
        account_id: Some(AccountId::from_str("acct_create_command_dto").unwrap()),
        member_id: None,
        capability: Capability::new("profile.create").unwrap(),
        action: Action::Create,
        scope: ResourceScope {
            profile_id: Some(profile_id.clone()),
            ..ResourceScope::default()
        },
        effect: Effect::Allow,
    });
    let app = router(state.clone());
    let body = json!({
        "id": profile_id,
        "name": "Command DTO profile",
        "runtime": "wayfern",
        "note": "Web form note"
    })
    .to_string();
    let create = || {
        lifecycle_request(
            "POST",
            "/v1/profiles",
            &token,
            "profile-create-command-dto-0001",
            Some(body.clone()),
        )
    };
    let created = app.clone().oneshot(create()).await.unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);
    let created_body: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(created.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(created_body["value"]["id"], "prf_command_dto");
    assert_eq!(created_body["value"]["state"], "ready");
    assert_eq!(created_body["value"]["syncMode"], "local");
    assert!(created_body["value"]["runtimeVersion"].is_null());
    assert_eq!(created_body["value"]["note"], "Web form note");
    assert_ne!(created_body["value"]["createdAt"], "");

    let replay = app.clone().oneshot(create()).await.unwrap();
    assert_eq!(replay.status(), StatusCode::CREATED);
    let replay_body = axum::body::to_bytes(replay.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        String::from_utf8(replay_body.to_vec()).unwrap(),
        serde_json::to_string(&created_body).unwrap()
    );

    let changed = json!({
        "id": "prf_command_dto",
        "name": "Changed command DTO",
        "runtime": "wayfern",
        "runtimeVersion": "152.0",
        "note": "Web form note"
    });
    let conflict = app
        .oneshot(lifecycle_request(
            "POST",
            "/v1/profiles",
            &token,
            "profile-create-command-dto-0001",
            Some(changed.to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(conflict.status(), StatusCode::CONFLICT);
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn hosted_profile_delete_moves_profile_to_recoverable_quarantine() {
    let account = AccountId::from_str("acct_hosted_delete").unwrap();
    let mut store = MemoryControlRecordStore::default();
    let account_value =
        serde_json::to_value(Account::new(account.clone(), "2026-10-12T00:00:00Z")).unwrap();
    store
        .put(PutControlRecord {
            tenant_id: "control",
            record_type: "account",
            record_id: account.as_str(),
            expected_revision: Some(0),
            idempotency_key: "seed-hosted-profile-delete-account",
            value: &account_value,
        })
        .unwrap();
    let mut hosted_profile = profile();
    let profile_id = ProfileId::from_str("prf_hosted_delete").unwrap();
    hosted_profile.id = profile_id.clone();
    hosted_profile.sync_mode = Some(SyncMode::Managed);
    let state = ApiState::hosted_with_control_store(ProfileService::disabled(), store)
        .unwrap()
        .with_profile_resource(ProfileResource {
            profile_id: profile_id.clone(),
            resource_id: buglogin_domain::ids::ResourceId::from_str("res_hosted_delete").unwrap(),
            workspace_id: WorkspaceId::from_str("ws_hosted_delete").unwrap(),
            archive_state: buglogin_domain::profile::ProfileArchiveState::Active,
        })
        .with_profile_record(hosted_profile);
    let token = state.issue_token(account.as_str()).unwrap();
    state.grants.write().unwrap().push(Grant {
        id: GrantId::from_str("grant_hosted_profile_delete").unwrap(),
        account_id: Some(account),
        member_id: None,
        capability: Capability::new("profile.delete").unwrap(),
        action: Action::Delete,
        scope: ResourceScope {
            profile_id: Some(profile_id.clone()),
            ..ResourceScope::default()
        },
        effect: Effect::Allow,
    });

    let response = router(state)
        .oneshot(lifecycle_request(
            "DELETE",
            "/v1/profiles/prf_hosted_delete",
            &token,
            "hosted-profile-delete-0001",
            Some(json!({"revision": 1, "confirmation": "QUARANTINE_PROFILE_DATA"}).to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(body["deleted"], true);
    assert_eq!(body["recoverable"], true);
}

#[tokio::test]
async fn profile_clone_and_share_routes_validate_path_identity() {
    let root = root("profile-route-identity");
    let _ = fs::remove_dir_all(&root);
    let state = ApiState::new(ProfileService::new(LocalRepository::open(&root).unwrap()));
    let token = state.issue_token("acct_route_identity").unwrap();
    let app = router(state);

    let clone = app
        .clone()
        .oneshot(request(
            "POST",
            "/v1/profiles/prf_source/clone",
            &token,
            Some(
                json!({
                    "sourceProfileId": "prf_other",
                    "targetProfileId": "prf_target",
                    "name": "Target",
                    "expectedRevision": 1
                })
                .to_string(),
            ),
        ))
        .await
        .unwrap();
    assert_eq!(clone.status(), StatusCode::BAD_REQUEST);

    let share = app
        .clone()
        .oneshot(request(
            "POST",
            "/v1/profiles/prf_source/shares",
            &token,
            Some(
                json!({
                    "profileId": "prf_other",
                    "memberId": "mem_one",
                    "permission": "read",
                    "expectedRevision": 1
                })
                .to_string(),
            ),
        ))
        .await
        .unwrap();
    assert_eq!(share.status(), StatusCode::BAD_REQUEST);

    let revoke = app
        .clone()
        .oneshot(request(
            "DELETE",
            "/v1/profiles/prf_source/shares/mem_one",
            &token,
            Some(
                json!({
                    "profileId": "prf_source",
                    "memberId": "mem_two",
                    "permission": "read",
                    "expectedRevision": 1
                })
                .to_string(),
            ),
        ))
        .await
        .unwrap();
    assert_eq!(revoke.status(), StatusCode::BAD_REQUEST);

    let unknown_share = app
        .clone()
        .oneshot(request(
            "POST",
            "/v1/profiles/prf_unknown_share/shares",
            &token,
            Some(
                json!({
                    "profileId": "prf_unknown_share",
                    "memberId": "mem_one",
                    "permission": "read",
                    "expectedRevision": 1
                })
                .to_string(),
            ),
        ))
        .await
        .unwrap();
    assert_eq!(unknown_share.status(), StatusCode::FORBIDDEN);

    let unknown_revoke = app
        .oneshot(request(
            "DELETE",
            "/v1/profiles/prf_unknown_share/shares/mem_one",
            &token,
            Some(
                json!({
                    "profileId": "prf_unknown_share",
                    "memberId": "mem_one",
                    "permission": "read",
                    "expectedRevision": 1
                })
                .to_string(),
            ),
        ))
        .await
        .unwrap();
    assert_eq!(unknown_revoke.status(), StatusCode::FORBIDDEN);
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn managed_profile_create_provisions_binding_and_initial_manifest() {
    let account = AccountId::from_str("acct_managed_provision").unwrap();
    let mut store = MemoryControlRecordStore::default();
    let account_value =
        serde_json::to_value(Account::new(account.clone(), "2026-07-22T00:00:00Z")).unwrap();
    store
        .put(PutControlRecord {
            tenant_id: "control",
            record_type: "account",
            record_id: account.as_str(),
            expected_revision: Some(0),
            idempotency_key: "seed-managed-provision-account",
            value: &account_value,
        })
        .unwrap();
    let state = ApiState::hosted_with_control_store(ProfileService::disabled(), store).unwrap();
    let token = state.issue_token(account.as_str()).unwrap();
    let workspace = buglogin_domain::ids::WorkspaceId::from_str("ws_managed_provision").unwrap();
    let organization = Organization {
        id: OrganizationId::from_str("org_managed_provision").unwrap(),
        account_id: account.clone(),
        name: "Managed provision test".into(),
        created_at: "2026-07-22T00:00:00Z".into(),
    };
    let mut workspace_service = WorkspaceService::new(
        organization.clone(),
        OrganizationPolicy {
            max_workspaces: 1,
            offline_entitled: false,
            default_extension_groups_by_workspace: Default::default(),
        },
    )
    .unwrap();
    workspace_service
        .create_workspace(
            &Principal {
                roles: BTreeSet::from([Role::PlatformAdmin]),
                ..Principal::default()
            },
            &[],
            &organization.id,
            Workspace {
                id: workspace.clone(),
                organization_id: organization.id.clone(),
                name: "Managed provision test".into(),
                policy: WorkspacePolicy {
                    max_profiles: 1,
                    max_members: 10,
                    max_storage_bytes: 1024 * 1024,
                    offline_entitled: false,
                },
                created_at: "2026-07-22T00:00:00Z".into(),
                default_extension_group_id: None,
            },
        )
        .unwrap();
    *state.workspaces.write().unwrap() = workspace_service;
    let profile_id = ProfileId::from_str("prf_managed_provision").unwrap();
    let mut grants = state.grants.write().unwrap();
    grants.extend([
        Grant {
            id: GrantId::from_str("grant_managed_profile_create").unwrap(),
            account_id: Some(account.clone()),
            member_id: None,
            capability: Capability::new("profile.create").unwrap(),
            action: Action::Create,
            scope: ResourceScope {
                profile_id: Some(profile_id.clone()),
                ..ResourceScope::default()
            },
            effect: Effect::Allow,
        },
        Grant {
            id: GrantId::from_str("grant_managed_resource_create").unwrap(),
            account_id: Some(account.clone()),
            member_id: None,
            capability: Capability::new("CAP-RES-001").unwrap(),
            action: Action::Create,
            scope: ResourceScope::workspace(workspace.clone()),
            effect: Effect::Allow,
        },
        Grant {
            id: GrantId::from_str("grant_managed_profile_read").unwrap(),
            account_id: Some(account.clone()),
            member_id: None,
            capability: Capability::new("profile.read").unwrap(),
            action: Action::Read,
            scope: ResourceScope {
                profile_id: Some(profile_id.clone()),
                ..ResourceScope::default()
            },
            effect: Effect::Allow,
        },
        Grant {
            id: GrantId::from_str("grant_managed_profile_update").unwrap(),
            account_id: Some(account.clone()),
            member_id: None,
            capability: Capability::new("profile.update").unwrap(),
            action: Action::Update,
            scope: ResourceScope {
                profile_id: Some(profile_id.clone()),
                ..ResourceScope::default()
            },
            effect: Effect::Allow,
        },
        Grant {
            id: GrantId::from_str("grant_managed_manifest_read").unwrap(),
            account_id: Some(account.clone()),
            member_id: None,
            capability: Capability::new("profile.read").unwrap(),
            action: Action::Read,
            scope: ResourceScope::workspace(workspace.clone()),
            effect: Effect::Allow,
        },
        Grant {
            id: GrantId::from_str("grant_managed_proxy_create").unwrap(),
            account_id: Some(account.clone()),
            member_id: None,
            capability: Capability::new("CAP-NET-001").unwrap(),
            action: Action::Create,
            scope: ResourceScope::workspace(workspace.clone()),
            effect: Effect::Allow,
        },
        Grant {
            id: GrantId::from_str("grant_managed_proxy_read").unwrap(),
            account_id: Some(account.clone()),
            member_id: None,
            capability: Capability::new("CAP-NET-001").unwrap(),
            action: Action::Read,
            scope: ResourceScope::workspace(workspace.clone()),
            effect: Effect::Allow,
        },
    ]);
    drop(grants);
    let app = router(state.clone());
    let mut managed = profile();
    managed.id = profile_id.clone();
    managed.sync_mode = Some(SyncMode::Managed);
    managed.proxy_id = Some(ProxyId::from_str("proxy_missing").unwrap());
    let stale_plan =
        HostedProfileService::prepare_provision(managed.clone(), workspace.clone()).unwrap();
    let provision_principal = Principal {
        account_id: Some(account.clone()),
        workspace_ids: BTreeSet::from([workspace.clone()]),
        ..Principal::default()
    };
    let provision_grants = state.grants.read().unwrap().clone();
    let stale_proxy = HostedProfileService::provision(
        &state,
        &provision_principal,
        &provision_grants,
        &stale_plan,
    )
    .err()
    .expect("provision must reject a proxy removed after request planning");
    assert_eq!(stale_proxy.code, "proxy_not_found");
    let mut missing_create = lifecycle_request(
        "POST",
        "/v1/profiles",
        &token,
        "managed-provision-missing-proxy-0001",
        Some(serde_json::to_string(&managed).unwrap()),
    );
    missing_create.headers_mut().insert(
        "x-buglogin-workspace-id",
        workspace.as_str().parse().unwrap(),
    );
    assert_eq!(
        app.clone().oneshot(missing_create).await.unwrap().status(),
        StatusCode::NOT_FOUND
    );
    let proxy_create = lifecycle_request(
        "POST",
        "/v1/workspaces/ws_managed_provision/proxies",
        &token,
        "managed-provision-proxy-0001",
        Some(
            json!({
                "workspaceId": workspace,
                "proxy": {
                    "id": "proxy_managed_1",
                    "name": "Managed proxy",
                    "protocol": "http",
                    "host": "proxy.example",
                    "port": 8080,
                    "bypassRules": [],
                    "createdAt": "2026-07-22T00:00:00Z",
                    "updatedAt": "2026-07-22T00:00:00Z"
                }
            })
            .to_string(),
        ),
    );
    assert_eq!(
        app.clone().oneshot(proxy_create).await.unwrap().status(),
        StatusCode::OK
    );
    managed.proxy_id = Some(ProxyId::from_str("proxy_managed_1").unwrap());
    let mut create = lifecycle_request(
        "POST",
        "/v1/profiles",
        &token,
        "managed-provision-key-0001",
        Some(serde_json::to_string(&managed).unwrap()),
    );
    create.headers_mut().insert(
        "x-buglogin-workspace-id",
        workspace.as_str().parse().unwrap(),
    );
    let created = app.clone().oneshot(create).await.unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);
    let created_body: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(created.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(created_body["value"]["state"], "ready");
    assert_eq!(created_body["value"]["proxyId"], "proxy_managed_1");

    let assignment_path = format!(
        "/v1/workspaces/{}/profiles/{}/proxy-assignment",
        workspace.as_str(),
        profile_id.as_str()
    );
    let assignment_input = json!({
        "workspaceId": workspace.as_str(),
        "profileId": profile_id.as_str(),
        "expectedProfileRevision": 1,
        "proxyId": null
    });
    let staged_assignment = app
        .clone()
        .oneshot(lifecycle_request(
            "PUT",
            &assignment_path,
            &token,
            "managed-proxy-assignment-stage-01",
            Some(assignment_input.to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(staged_assignment.status(), StatusCode::OK);
    let staged_body: Value = serde_json::from_slice(
        &axum::body::to_bytes(staged_assignment.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(staged_body["proxyId"], Value::Null);
    assert_eq!(staged_body["expectedProfileRevision"], 1);

    let pending_assignment = app
        .clone()
        .oneshot(request("GET", &assignment_path, &token, None))
        .await
        .unwrap();
    assert_eq!(pending_assignment.status(), StatusCode::OK);
    let pending_body: Value = serde_json::from_slice(
        &axum::body::to_bytes(pending_assignment.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(pending_body, staged_body);

    // Choosing the active proxy again cancels/replaces the pending clear,
    // instead of leaving a stale assignment that will apply on next sync.
    let cancel_pending = app
        .clone()
        .oneshot(lifecycle_request(
            "PUT",
            &assignment_path,
            &token,
            "managed-proxy-assignment-cancel-01",
            Some(
                json!({
                    "workspaceId": workspace.as_str(),
                    "profileId": profile_id.as_str(),
                    "expectedProfileRevision": 1,
                    "proxyId": "proxy_managed_1"
                })
                .to_string(),
            ),
        ))
        .await
        .unwrap();
    assert_eq!(cancel_pending.status(), StatusCode::OK);
    let cancel_body: Value = serde_json::from_slice(
        &axum::body::to_bytes(cancel_pending.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(cancel_body["proxyId"], "proxy_managed_1");
    let pending_after_cancel = app
        .clone()
        .oneshot(request("GET", &assignment_path, &token, None))
        .await
        .unwrap();
    assert_eq!(pending_after_cancel.status(), StatusCode::OK);
    let pending_after_cancel: Value = serde_json::from_slice(
        &axum::body::to_bytes(pending_after_cancel.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(pending_after_cancel["proxyId"], "proxy_managed_1");

    let active_profile = app
        .clone()
        .oneshot(request(
            "GET",
            &format!("/v1/profiles/{}", profile_id.as_str()),
            &token,
            None,
        ))
        .await
        .unwrap();
    let active_profile: Value = serde_json::from_slice(
        &axum::body::to_bytes(active_profile.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(active_profile["value"]["proxyId"], "proxy_managed_1");
    assert_eq!(active_profile["revision"], 1);

    let mut over_quota_profile = managed.clone();
    over_quota_profile.id = ProfileId::from_str("prf_managed_over_quota").unwrap();
    state.grants.write().unwrap().push(Grant {
        id: GrantId::from_str("grant_managed_profile_over_quota").unwrap(),
        account_id: Some(account.clone()),
        member_id: None,
        capability: Capability::new("profile.create").unwrap(),
        action: Action::Create,
        scope: ResourceScope {
            profile_id: Some(over_quota_profile.id.clone()),
            ..ResourceScope::default()
        },
        effect: Effect::Allow,
    });
    let mut over_quota = lifecycle_request(
        "POST",
        "/v1/profiles",
        &token,
        "managed-provision-overflow-0001",
        Some(serde_json::to_string(&over_quota_profile).unwrap()),
    );
    over_quota.headers_mut().insert(
        "x-buglogin-workspace-id",
        workspace.as_str().parse().unwrap(),
    );
    let over_quota = app.clone().oneshot(over_quota).await.unwrap();
    assert_eq!(over_quota.status(), StatusCode::CONFLICT);
    let over_quota_body: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(over_quota.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(over_quota_body["code"], "profile_quota_exceeded");

    let mut retry_profile = managed.clone();
    retry_profile.created_at = "2026-10-12T01:00:00Z".to_owned();
    retry_profile.updated_at = retry_profile.created_at.clone();
    let mut retry = lifecycle_request(
        "POST",
        "/v1/profiles",
        &token,
        "managed-provision-key-0001",
        Some(serde_json::to_string(&retry_profile).unwrap()),
    );
    retry.headers_mut().insert(
        "x-buglogin-workspace-id",
        workspace.as_str().parse().unwrap(),
    );
    let replayed = app.clone().oneshot(retry).await.unwrap();
    assert_eq!(replayed.status(), StatusCode::CREATED);
    let replayed_body: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(replayed.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(replayed_body, created_body);

    let binding_response = app
        .clone()
        .oneshot(request(
            "GET",
            "/v1/profiles/prf_managed_provision/binding",
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(binding_response.status(), StatusCode::OK);
    let binding: ProfileResource = serde_json::from_slice(
        &axum::body::to_bytes(binding_response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(binding.workspace_id, workspace);

    let manifest_path = format!(
        "/v1/workspaces/{}/resources/{}/manifest",
        binding.workspace_id.as_str(),
        binding.resource_id.as_str()
    );
    let mut manifest_request = request("GET", &manifest_path, &token, None);
    manifest_request
        .headers_mut()
        .insert("x-buglogin-actor-id", account.as_str().parse().unwrap());
    let manifest_response = app.clone().oneshot(manifest_request).await.unwrap();
    assert_eq!(manifest_response.status(), StatusCode::OK);
    let manifest: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(manifest_response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["generation"], 1);
    assert_eq!(manifest["objects"], serde_json::json!([]));

    let update_path = format!(
        "/v1/workspaces/{}/profiles/{}",
        workspace.as_str(),
        profile_id.as_str()
    );
    let updated = app
        .clone()
        .oneshot(lifecycle_request(
            "PUT",
            &update_path,
            &token,
            "managed-profile-metadata-update-01",
            Some(
                json!({
                    "workspaceId": workspace.as_str(),
                    "profileId": profile_id.as_str(),
                    "expectedRevision": 1,
                    "name": "Managed provision updated",
                    "note": null,
                    "tags": []
                })
                .to_string(),
            ),
        ))
        .await
        .unwrap();
    assert_eq!(updated.status(), StatusCode::CONFLICT);
    let update_problem: Value = serde_json::from_slice(
        &axum::body::to_bytes(updated.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(update_problem["code"], "profile_proxy_assignment_pending");

    let stale_assignment = app
        .clone()
        .oneshot(request("GET", &assignment_path, &token, None))
        .await
        .unwrap();
    assert_eq!(stale_assignment.status(), StatusCode::OK);
    let stale_assignment: Value = serde_json::from_slice(
        &axum::body::to_bytes(stale_assignment.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(stale_assignment["expectedProfileRevision"], 1);

    let mut restaged_assignment = assignment_input;
    restaged_assignment["expectedProfileRevision"] = json!(1);
    let restaged = app
        .oneshot(lifecycle_request(
            "PUT",
            &assignment_path,
            &token,
            "managed-proxy-assignment-restage-02",
            Some(restaged_assignment.to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(restaged.status(), StatusCode::OK);
    let restaged: Value = serde_json::from_slice(
        &axum::body::to_bytes(restaged.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(restaged["expectedProfileRevision"], 1);
}

#[tokio::test]
async fn profile_lifecycle_keeps_operation_and_state_fail_closed_without_runtime() {
    let root = root("lifecycle");
    let _ = fs::remove_dir_all(&root);
    let state = ApiState::new(ProfileService::new(LocalRepository::open(&root).unwrap()));
    let token = state.issue_token("acct_lifecycle").unwrap();
    let account = AccountId::from_str("acct_lifecycle").unwrap();
    let profile_id = ProfileId::from_str("prf_http_1").unwrap();
    state.profiles.create(profile()).unwrap();
    for capability in ["profile.launch", "profile.stop"] {
        state.grants.write().unwrap().push(Grant {
            id: GrantId::from_str(format!("grant_{}", capability.replace('.', "_")).as_str())
                .unwrap(),
            account_id: Some(account.clone()),
            member_id: None,
            capability: Capability::new(capability).unwrap(),
            action: Action::Execute,
            scope: ResourceScope {
                profile_id: Some(profile_id.clone()),
                ..ResourceScope::default()
            },
            effect: Effect::Allow,
        });
    }
    let app: Router = router(state.clone());

    let launch = app
        .clone()
        .oneshot(lifecycle_request(
            "POST",
            "/v1/profiles/prf_http_1/launch",
            &token,
            "lifecycle-launch-0001",
            Some(
                json!({
                    "profilesRoot": root.join("profiles"),
                    "extraArgs": []
                })
                .to_string(),
            ),
        ))
        .await
        .unwrap();
    assert_eq!(launch.status(), StatusCode::CONFLICT);
    let body = axum::body::to_bytes(launch.into_body(), usize::MAX)
        .await
        .unwrap();
    let problem: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(problem["code"], "conflict");
    let replay = app
        .clone()
        .oneshot(lifecycle_request(
            "POST",
            "/v1/profiles/prf_http_1/launch",
            &token,
            "lifecycle-launch-0001",
            Some(
                json!({
                    "profilesRoot": root.join("profiles"),
                    "extraArgs": []
                })
                .to_string(),
            ),
        ))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::ACCEPTED);
    let operation: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(replay.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(operation["status"], "failed");
    assert_eq!(operation["error"]["code"], "profile_not_launchable");
    assert!(state.lifecycles.read().unwrap().is_empty());
    assert_eq!(
        state
            .profiles
            .get(&profile_id)
            .unwrap()
            .unwrap()
            .value
            .state,
        ProfileState::Created
    );

    let stop = app
        .oneshot(lifecycle_request(
            "POST",
            "/v1/profiles/prf_http_1/stop",
            &token,
            "lifecycle-stop-0001",
            None,
        ))
        .await
        .unwrap();
    assert_eq!(stop.status(), StatusCode::CONFLICT);
    assert!(state.lifecycles.read().unwrap().is_empty());
}

#[tokio::test]
async fn hosted_profile_archive_restore_replays_and_survives_restart() {
    let root = root("hosted-archive-restore");
    let _ = fs::remove_dir_all(&root);
    let account = AccountId::from_str("acct_hosted_profile_mutation").unwrap();
    let profile_id = ProfileId::from_str("prf_hosted_profile_mutation").unwrap();
    let workspace_id = WorkspaceId::from_str("ws_hosted_profile_mutation").unwrap();
    let mut store = MemoryControlRecordStore::default();
    let account_value =
        serde_json::to_value(Account::new(account.clone(), "2026-07-22T00:00:00Z")).unwrap();
    store
        .put(PutControlRecord {
            tenant_id: "control",
            record_type: "account",
            record_id: account.as_str(),
            expected_revision: Some(0),
            idempotency_key: "seed-hosted-profile-account",
            value: &account_value,
        })
        .unwrap();
    let reopen = store.reopen();
    let state = ApiState::hosted_with_control_store(ProfileService::disabled(), store).unwrap();
    let token = state.issue_token(account.as_str()).unwrap();
    let organization = Organization {
        id: OrganizationId::from_str("org_hosted_profile_mutation").unwrap(),
        account_id: account.clone(),
        name: "Hosted profile test".into(),
        created_at: "2026-07-22T00:00:00Z".into(),
    };
    let mut workspace_service = WorkspaceService::new(
        organization.clone(),
        OrganizationPolicy {
            max_workspaces: 1,
            offline_entitled: false,
            default_extension_groups_by_workspace: Default::default(),
        },
    )
    .unwrap();
    workspace_service
        .create_workspace(
            &Principal {
                roles: BTreeSet::from([Role::PlatformAdmin]),
                ..Principal::default()
            },
            &[],
            &organization.id,
            Workspace {
                id: workspace_id.clone(),
                organization_id: organization.id.clone(),
                name: "Hosted profile test".into(),
                policy: WorkspacePolicy {
                    max_profiles: 2,
                    max_members: 10,
                    max_storage_bytes: 1024 * 1024,
                    offline_entitled: false,
                },
                created_at: "2026-07-22T00:00:00Z".into(),
                default_extension_group_id: None,
            },
        )
        .unwrap();
    *state.workspaces.write().unwrap() = workspace_service;
    state.grants.write().unwrap().extend([
        Grant {
            id: GrantId::from_str("grant_hosted_profile_create").unwrap(),
            account_id: Some(account.clone()),
            member_id: None,
            capability: Capability::new("profile.create").unwrap(),
            action: Action::Create,
            scope: ResourceScope {
                profile_id: Some(profile_id.clone()),
                ..ResourceScope::default()
            },
            effect: Effect::Allow,
        },
        Grant {
            id: GrantId::from_str("grant_hosted_profile_resource").unwrap(),
            account_id: Some(account.clone()),
            member_id: None,
            capability: Capability::new("CAP-RES-001").unwrap(),
            action: Action::Create,
            scope: ResourceScope::workspace(workspace_id.clone()),
            effect: Effect::Allow,
        },
        Grant {
            id: GrantId::from_str("grant_hosted_profile_read").unwrap(),
            account_id: Some(account.clone()),
            member_id: None,
            capability: Capability::new("profile.read").unwrap(),
            action: Action::Read,
            scope: ResourceScope {
                profile_id: Some(profile_id.clone()),
                ..ResourceScope::default()
            },
            effect: Effect::Allow,
        },
        Grant {
            id: GrantId::from_str("grant_hosted_profile_archive").unwrap(),
            account_id: Some(account.clone()),
            member_id: None,
            capability: Capability::new("profile.archive").unwrap(),
            action: Action::Update,
            scope: ResourceScope {
                profile_id: Some(profile_id.clone()),
                ..ResourceScope::default()
            },
            effect: Effect::Allow,
        },
        Grant {
            id: GrantId::from_str("grant_hosted_profile_restore").unwrap(),
            account_id: Some(account.clone()),
            member_id: None,
            capability: Capability::new("profile.restore").unwrap(),
            action: Action::Update,
            scope: ResourceScope {
                profile_id: Some(profile_id.clone()),
                ..ResourceScope::default()
            },
            effect: Effect::Allow,
        },
    ]);
    let app = router(state.clone());
    let mut managed = profile();
    managed.id = profile_id.clone();
    managed.state = ProfileState::Created;
    managed.sync_mode = Some(SyncMode::Managed);
    let mut create = lifecycle_request(
        "POST",
        "/v1/profiles",
        &token,
        "hosted-profile-create-0001",
        Some(serde_json::to_string(&managed).unwrap()),
    );
    create.headers_mut().insert(
        "x-buglogin-workspace-id",
        workspace_id.as_str().parse().unwrap(),
    );
    assert_eq!(
        app.clone().oneshot(create).await.unwrap().status(),
        StatusCode::CREATED
    );

    let archive_body = json!({"revision": 1}).to_string();
    let archived = app
        .clone()
        .oneshot(lifecycle_request(
            "PUT",
            "/v1/profiles/prf_hosted_profile_mutation/archive",
            &token,
            "hosted-profile-archive-0001",
            Some(archive_body.clone()),
        ))
        .await
        .unwrap();
    assert_eq!(archived.status(), StatusCode::OK);
    let archived_bytes = axum::body::to_bytes(archived.into_body(), usize::MAX)
        .await
        .unwrap();
    let archived_value: serde_json::Value = serde_json::from_slice(&archived_bytes).unwrap();
    assert_eq!(archived_value["value"]["archived"], true);
    assert_eq!(archived_value["revision"], 2);

    let replay = app
        .clone()
        .oneshot(lifecycle_request(
            "PUT",
            "/v1/profiles/prf_hosted_profile_mutation/archive",
            &token,
            "hosted-profile-archive-0001",
            Some(archive_body),
        ))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(
        axum::body::to_bytes(replay.into_body(), usize::MAX)
            .await
            .unwrap(),
        archived_bytes
    );

    let conflict = app
        .clone()
        .oneshot(lifecycle_request(
            "PUT",
            "/v1/profiles/prf_hosted_profile_mutation/archive",
            &token,
            "hosted-profile-archive-0001",
            Some(json!({"revision": 2}).to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(conflict.status(), StatusCode::CONFLICT);

    let stale = app
        .clone()
        .oneshot(lifecycle_request(
            "PUT",
            "/v1/profiles/prf_hosted_profile_mutation/restore",
            &token,
            "hosted-profile-restore-stale-0001",
            Some(json!({"revision": 1}).to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(stale.status(), StatusCode::CONFLICT);

    let restored = app
        .clone()
        .oneshot(lifecycle_request(
            "PUT",
            "/v1/profiles/prf_hosted_profile_mutation/restore",
            &token,
            "hosted-profile-restore-0001",
            Some(json!({"revision": 2}).to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(restored.status(), StatusCode::OK);

    drop(app);
    drop(state);
    let restarted =
        ApiState::hosted_with_control_store(ProfileService::disabled(), reopen).unwrap();
    let restarted_token = restarted.issue_token(account.as_str()).unwrap();
    let restarted_app = router(restarted);
    let loaded = restarted_app
        .clone()
        .oneshot(request(
            "GET",
            "/v1/profiles/prf_hosted_profile_mutation",
            &restarted_token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(loaded.status(), StatusCode::OK);
    let loaded: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(loaded.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(loaded["revision"], 3);
    assert_eq!(loaded["value"]["archived"], false);
    let binding = restarted_app
        .oneshot(request(
            "GET",
            "/v1/profiles/prf_hosted_profile_mutation/binding",
            &restarted_token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(binding.status(), StatusCode::OK);
    let binding: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(binding.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(binding["archiveState"], "active");
    let _ = fs::remove_dir_all(root);
}
