use std::{fs, path::PathBuf};

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use buglogin_api::http::{ApiState, router};
use buglogin_browser::profile::ProfileService;
use buglogin_domain::{
    ids::{AccountId, GrantId},
    permissions::{Action, Capability, Effect, Grant, ResourceScope},
};
use buglogin_migration::MigrationKey;
use buglogin_storage::{blob::checksum, local::LocalRepository, secrets::SecretValue};
use serde_json::Value;
use tower::ServiceExt;

fn root(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "buglogin-http-migration-{label}-{}",
        std::process::id()
    ))
}

fn request(uri: &str, token: &str, key: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .header("x-correlation-id", "migration-http-test")
        .header("idempotency-key", key)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

#[tokio::test]
async fn migration_http_flow_inspects_imports_and_preserves_source() {
    let base = root("e2e");
    let _ = fs::remove_dir_all(&base);
    let source = base.join("source");
    let staging = base.join("staging");
    let destination = base.join("local");
    fs::create_dir_all(source.join("profiles/01234567-89ab-cdef-0123-456789abcdef/profile"))
        .unwrap();
    fs::write(
        source.join("version.json"),
        r#"{"system":"buglogin-legacy","version":"1.1.84","instanceId":"http-fixture","os":"windows","architecture":"x86_64"}"#,
    )
    .unwrap();
    fs::write(
        source.join("profiles/01234567-89ab-cdef-0123-456789abcdef/metadata.json"),
        r#"{"id":"01234567-89ab-cdef-0123-456789abcdef","runtime":"wayfern"}"#,
    )
    .unwrap();
    fs::write(
        source.join("profiles/01234567-89ab-cdef-0123-456789abcdef/profile/Preferences"),
        "immutable",
    )
    .unwrap();
    fs::write(
        source.join("control-plane-export.json"),
        serde_json::to_vec(&control_plane_export()).unwrap(),
    )
    .unwrap();
    let source_hash = checksum(
        &fs::read(source.join("profiles/01234567-89ab-cdef-0123-456789abcdef/profile/Preferences"))
            .unwrap(),
    );

    let state = ApiState::new(ProfileService::new(
        LocalRepository::open(base.join("local")).unwrap(),
    ))
    .with_migration_key(MigrationKey::from_bytes([41; 32]))
    .with_migration_signing_key(SecretValue::new([42; 32].to_vec()).unwrap());
    let token = state.issue_token("acct_migration").unwrap();
    let account = AccountId::from_str("acct_migration").unwrap();
    let mut grants = state.grants.write().unwrap();
    for (name, action) in [
        ("migration.read", Action::Read),
        ("migration.write", Action::Execute),
    ] {
        grants.push(Grant {
            id: GrantId::from_str(format!("grant_{}", name.replace('.', "_")).as_str()).unwrap(),
            account_id: Some(account.clone()),
            member_id: None,
            capability: Capability::new(name).unwrap(),
            action,
            scope: ResourceScope::global(),
            effect: Effect::Allow,
        });
    }
    drop(grants);
    let app = router(state);

    let inspect = app
        .clone()
        .oneshot(request(
            "/v1/migrations/inspect",
            &token,
            "migration-inspect-0001",
            serde_json::json!({"sourceRoot": source}),
        ))
        .await
        .unwrap();
    if inspect.status() != StatusCode::ACCEPTED {
        let body = axum::body::to_bytes(inspect.into_body(), 16 * 1024)
            .await
            .unwrap();
        panic!("inspect failed: {}", String::from_utf8_lossy(&body));
    }
    let inspect_body: Value = serde_json::from_slice(
        &axum::body::to_bytes(inspect.into_body(), 64 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(inspect_body["status"], "completed");
    assert!(inspect_body["result"].is_object());
    assert!(inspect_body["result"]["report"].is_object());
    assert!(inspect_body["result"]["snapshot"].is_object());

    let import = app
        .oneshot(request(
            "/v1/migrations/import",
            &token,
            "migration-import-0001",
            serde_json::json!({
                "sourceRoot": source,
                "stagingParent": staging,
                "destinationRoot": destination,
                "runId": "http-cutover-001",
                "confirmation": "CUTOVER_AND_PROMOTE"
            }),
        ))
        .await
        .unwrap();
    if import.status() != StatusCode::ACCEPTED {
        let status = import.status();
        let body = axum::body::to_bytes(import.into_body(), 16 * 1024)
            .await
            .unwrap();
        panic!(
            "import failed ({status}): {}",
            String::from_utf8_lossy(&body)
        );
    }
    let import_body: Value = serde_json::from_slice(
        &axum::body::to_bytes(import.into_body(), 64 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(import_body["status"], "completed");
    assert_eq!(
        import_body["result"]["controlImport"]["create"]["profileResources"],
        1
    );
    assert_eq!(
        import_body["result"]["controlImport"]["create"]["grants"],
        2
    );
    assert!(
        destination
            .join("migration-imports/http-cutover-001/manifest.json")
            .is_file()
    );
    assert!(
        destination
            .join("migration-imports/http-cutover-001/signature.json")
            .is_file()
    );
    assert_eq!(
        checksum(
            &fs::read(
                source.join("profiles/01234567-89ab-cdef-0123-456789abcdef/profile/Preferences")
            )
            .unwrap()
        ),
        source_hash
    );
    let _ = fs::remove_dir_all(base);
}

fn control_plane_export() -> Value {
    serde_json::json!({
        "organizations": [{
            "id": "organization-http",
            "name": "HTTP Migration",
            "slug": "http-migration",
            "status": "active",
            "createdBy": "owner-http",
            "createdAt": "2025-01-01T00:00:00Z",
            "updatedAt": "2025-01-01T00:00:00Z"
        }],
        "workspaces": [{
            "id": "workspace-http",
            "organizationId": "organization-http",
            "name": "HTTP Workspace",
            "mode": "team",
            "createdAt": "2025-01-01T00:00:00Z",
            "createdBy": "owner-http",
            "profileLimit": 10,
            "memberLimit": 10,
            "maxStorageBytes": 1048576,
            "entitlementState": "active",
            "localEntitlementState": "active"
        }],
        "memberships": [{
            "workspaceId": "workspace-http",
            "userId": "owner-http",
            "email": "owner-http@example.test",
            "role": "owner",
            "resourceAccessMode": "full",
            "customCapabilities": null,
            "accountState": "active",
            "platformRole": null,
            "accessSource": "direct",
            "createdAt": "2025-01-01T00:00:00Z"
        }, {
            "workspaceId": "workspace-http",
            "userId": "member-http",
            "email": "member-http@example.test",
            "role": "member",
            "resourceAccessMode": "restricted",
            "customCapabilities": [],
            "accountState": "active",
            "platformRole": null,
            "accessSource": "direct",
            "createdAt": "2025-01-02T00:00:00Z"
        }],
        "shareGrants": [{
            "id": "share-http",
            "workspaceId": "workspace-http",
            "resourceType": "profile",
            "resourceId": "prf_0123456789abcdef0123456789abcdef",
            "recipientEmail": "member-http@example.test",
            "accessMode": "run_sync_limited",
            "createdAt": "2025-01-03T00:00:00Z",
            "createdBy": "owner-http",
            "revokedAt": null
        }]
    })
}
