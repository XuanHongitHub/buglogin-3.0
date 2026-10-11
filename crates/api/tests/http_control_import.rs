use std::fs;

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use buglogin_api::http::{ApiState, router};
use buglogin_browser::profile::ProfileService;
use buglogin_domain::{
    entities::{Profile, ProfileState, Runtime},
    ids::{AccountId, GrantId},
    permissions::{Action, Capability, Effect, Grant, ResourceScope},
};
use buglogin_migration::legacy::map_legacy_control_plane_json;
use buglogin_storage::local::LocalRepository;
use serde_json::{Value, json};
use tower::ServiceExt;

#[tokio::test]
async fn control_import_dry_run_activates_replays_and_rejects_changed_package() {
    let root = std::env::temp_dir().join(format!("buglogin-control-import-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let mapping = map_legacy_control_plane_json(&legacy_export(), "control-import-source").unwrap();
    let member_account_id = mapping
        .accounts
        .iter()
        .find(|account| account.source_ref.id == "member-a")
        .expect("mapped member account")
        .id
        .clone();
    let profiles = ProfileService::new(LocalRepository::open(&root).unwrap());
    profiles
        .create(local_profile(&mapping.profile_resources[0].data.profile_id))
        .unwrap();
    let state = ApiState::new(profiles);
    let token = state.issue_token("acct_import_admin").unwrap();
    let admin = AccountId::from_str("acct_import_admin").unwrap();
    state.grants.write().unwrap().extend([
        allow(&admin, "grant_import", "migration.write", Action::Execute),
        allow(&admin, "grant_account_list", "CAP-ID-001", Action::Read),
    ]);
    let profile_id = mapping.profile_resources[0].data.profile_id.clone();
    let base = json!({
        "packageId": "pkg-control-import",
        "accounts": mapping.accounts,
        "profiles": [{
            "schema": "buglogin.profile/1",
            "source_ref": {
                "system": "buglogin-legacy",
                "instance_id": "control-import-source",
                "id": "legacy-profile-1"
            },
            "id": profile_id,
            "revision": 1,
            "created_at": "2025-01-01T00:00:00Z",
            "updated_at": "2025-01-01T00:00:00Z",
            "data": {
                "name": "Control import profile",
                "runtime": "wayfern",
                "runtime_version": "146.0.7680.177.4",
                "release_type": "stable",
                "ephemeral": false,
                "source_data_object": {
                    "path": "profiles/legacy-profile-1/profile",
                    "logical_type": "profile-directory",
                    "disposition": {
                        "classification": "excluded",
                        "reason_code": "redacted"
                    }
                },
                "migration_class": "direct",
                "group_id": null,
                "proxy_id": null,
                "vpn_id": null,
                "extension_group_id": null,
                "tags": [],
                "note": "legacy note",
                "legacy_runtime_config": {
                    "schema_key": "chromium.runtime-config.v1",
                    "schema_version": 1,
                    "payload": {
                        "fingerprint": "{\"language\":\"en-US\",\"timezone\":\"America/New_York\"}"
                    }
                }
            },
            "extensions": {}
        }],        "organizations": mapping.organizations,
        "workspaces": mapping.workspaces,
        "teams": mapping.teams,
        "members": mapping.members,
        "resources": mapping.resources,
        "profileResources": mapping.profile_resources,
        "grants": mapping.grants
    });
    let app = router(state.clone());

    let mut dry = base.clone();
    dry["dryRun"] = json!(true);
    let dry_response = app
        .clone()
        .oneshot(request(&token, "control-import-dry-0001", dry))
        .await
        .unwrap();
    assert_eq!(dry_response.status(), StatusCode::OK);
    let dry_body = body(dry_response).await;
    assert_eq!(dry_body["create"]["accounts"], 2);
    assert_eq!(dry_body["create"]["profileResources"], 1);
    assert_eq!(account_count(app.clone(), &token).await, 1);

    let activated = app
        .clone()
        .oneshot(request(&token, "control-import-run-0001", base.clone()))
        .await
        .unwrap();
    assert_eq!(activated.status(), StatusCode::OK);
    let activated_body = body(activated).await;
    assert_eq!(activated_body["replayed"], false);
    assert_eq!(activated_body["create"]["workspaces"], 1);
    assert_eq!(activated_body["create"]["profileResources"], 1);
    assert_eq!(account_count(app.clone(), &token).await, 3);
    let member_token = state.issue_token(member_account_id).unwrap();
    let member_read = app
        .clone()
        .oneshot(get(
            &member_token,
            &format!("/v1/profiles/{profile_id}"),
            "control-import-member-read",
        ))
        .await
        .unwrap();
    assert_eq!(member_read.status(), StatusCode::OK);
    let member_profile = body(member_read).await;
    assert_eq!(
        member_profile["value"]["runtimeVersion"],
        "146.0.7680.177.4"
    );
    assert_eq!(member_profile["value"]["note"], "legacy note");
    assert_eq!(member_profile["value"]["fingerprint"]["language"], "en-US");
    assert_eq!(
        member_profile["value"]["runtimeConfig"]["schema_key"],
        "chromium.runtime-config.v1"
    );
    let member_update = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/v1/profiles/{profile_id}/archive"))
                .header("authorization", format!("Bearer {member_token}"))
                .header("x-correlation-id", "control-import-member-update")
                .header("idempotency-key", "control-import-member-update-0001")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"revision":1}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(member_update.status(), StatusCode::FORBIDDEN);

    let replay = app
        .clone()
        .oneshot(request(&token, "control-import-run-0002", base.clone()))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(body(replay).await["replayed"], true);

    let mut changed = base.clone();
    changed["workspaces"][0]["data"]["name"] = json!("Changed");
    let collision = app
        .clone()
        .oneshot(request(&token, "control-import-run-0003", changed))
        .await
        .unwrap();
    assert_eq!(collision.status(), StatusCode::CONFLICT);
    let rollback = app
        .clone()
        .oneshot(post(
            &token,
            "control-rollback-run-0001",
            "/v1/migrations/control/rollback",
            json!({
                "packageId": "pkg-control-import",
                "confirmation": "ROLLBACK_CONTROL_MIGRATION"
            }),
        ))
        .await
        .unwrap();
    let rollback_status = rollback.status();
    let rollback_body = body(rollback).await;
    assert_eq!(rollback_status, StatusCode::OK, "{rollback_body}");
    assert_eq!(rollback_body["state"], "rolled_back");
    assert_eq!(account_count(app.clone(), &token).await, 1);
    let replay_rollback = app
        .clone()
        .oneshot(post(
            &token,
            "control-rollback-run-0002",
            "/v1/migrations/control/rollback",
            json!({
                "packageId": "pkg-control-import",
                "confirmation": "ROLLBACK_CONTROL_MIGRATION"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(body(replay_rollback).await["replayed"], true);

    let reactivated = app
        .clone()
        .oneshot(request(&token, "control-import-reactivate-0001", base))
        .await
        .unwrap();
    let reactivated_status = reactivated.status();
    let reactivated_body = body(reactivated).await;
    assert_eq!(reactivated_status, StatusCode::OK, "{reactivated_body}");
    assert_eq!(reactivated_body["state"], "active");
    assert_eq!(reactivated_body["replayed"], false);
    assert_eq!(account_count(app.clone(), &token).await, 3);

    let final_rollback = app
        .oneshot(post(
            &token,
            "control-rollback-run-0003",
            "/v1/migrations/control/rollback",
            json!({
                "packageId": "pkg-control-import",
                "confirmation": "ROLLBACK_CONTROL_MIGRATION"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(final_rollback.status(), StatusCode::OK);
    let _ = fs::remove_dir_all(root);
}

fn local_profile(id: &str) -> Profile {
    Profile {
        id: buglogin_domain::ids::ProfileId::from_str(id).unwrap(),
        name: "Control import profile".into(),
        runtime: Runtime::Wayfern,
        runtime_version: Some("146.0.7680.177.4".into()),
        runtime_config: Some(json!({
            "schema_key": "chromium.runtime-config.v1",
            "schema_version": 1,
            "payload": {
                "fingerprint": r#"{"language":"en-US","timezone":"America/New_York"}"#
            }
        })),
        state: ProfileState::Ready,
        archived: false,
        fingerprint: Some(json!({"language": "en-US", "timezone": "America/New_York"})),
        note: Some("legacy note".into()),
        proxy_id: None,
        vpn_id: None,
        group_ids: vec![],
        tags: vec![],
        extension_group_id: None,
        sync_mode: None,
        created_at: "2025-01-01T00:00:00Z".into(),
        updated_at: "2025-01-01T00:00:00Z".into(),
    }
}

fn allow(account: &AccountId, id: &str, capability: &str, action: Action) -> Grant {
    Grant {
        id: GrantId::from_str(id).unwrap(),
        account_id: Some(account.clone()),
        member_id: None,
        capability: Capability::new(capability).unwrap(),
        action,
        scope: ResourceScope::global(),
        effect: Effect::Allow,
    }
}

fn request(token: &str, idempotency: &str, value: Value) -> Request<Body> {
    post(token, idempotency, "/v1/migrations/control/import", value)
}

fn post(token: &str, idempotency: &str, uri: &str, value: Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .header("x-correlation-id", "control-import-correlation")
        .header("idempotency-key", idempotency)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&value).unwrap()))
        .unwrap()
}

fn get(token: &str, uri: &str, correlation: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .header("x-correlation-id", correlation)
        .body(Body::empty())
        .unwrap()
}

async fn account_count(app: axum::Router, token: &str) -> usize {
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/accounts")
                .header("authorization", format!("Bearer {token}"))
                .header("x-correlation-id", "control-import-account-list")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    body(response).await.as_array().unwrap().len()
}

async fn body(response: axum::response::Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
}

fn legacy_export() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "organizations": [{
            "id": "organization-a", "name": "Operations", "slug": "operations",
            "status": "active", "createdBy": "owner-a",
            "createdAt": "2025-01-01T00:00:00Z", "updatedAt": "2025-01-01T00:00:00Z"
        }],
        "workspaces": [{
            "id": "workspace-a", "organizationId": "organization-a", "name": "Operations",
            "mode": "team", "createdAt": "2025-01-01T00:00:00Z", "createdBy": "owner-a",
            "profileLimit": 100, "memberLimit": 20, "maxStorageBytes": 1073741824_u64,
            "entitlementState": "active", "localEntitlementState": "active"
        }],
        "memberships": [{
            "workspaceId": "workspace-a", "userId": "owner-a", "email": "owner@example.com",
            "role": "owner", "resourceAccessMode": "full", "customCapabilities": null,
            "accountState": "active", "platformRole": null, "accessSource": "direct",
            "createdAt": "2025-01-01T00:00:00Z"
        }, {
            "workspaceId": "workspace-a", "userId": "member-a", "email": "member@example.com",
            "role": "member", "resourceAccessMode": "restricted", "customCapabilities": [],
            "accountState": "active", "platformRole": null, "accessSource": "direct",
            "createdAt": "2025-01-02T00:00:00Z"
        }],
        "shareGrants": [{
            "id": "share-a", "workspaceId": "workspace-a", "resourceType": "profile",
            "resourceId": "prf_11111111111111111111111111111111", "resourceName": "Profile A", "recipientEmail": "member@example.com",
            "accessMode": "run_sync_limited", "createdAt": "2025-01-03T00:00:00Z",
            "createdBy": "owner-a", "revokedAt": null
        }]
    })).unwrap()
}
