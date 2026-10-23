use buglogin_migration::legacy::{
    canonicalize_legacy_control_plane_json, map_legacy_control_plane_json,
};
use serde_json::{Value, json};

fn canonical_export() -> Value {
    json!({
        "organizations": [{
            "id": "organization-audit",
            "name": "Synthetic Organization",
            "slug": "synthetic-organization",
            "status": "active",
            "createdBy": "owner-audit",
            "createdAt": "2026-01-01T00:00:00Z",
            "updatedAt": "2026-01-01T00:00:00Z"
        }],
        "workspaces": [{
            "id": "workspace-audit",
            "organizationId": "organization-audit",
            "name": "Synthetic Workspace",
            "mode": "team",
            "createdAt": "2026-01-01T00:00:00Z",
            "createdBy": "owner-audit",
            "profileLimit": 100,
            "memberLimit": 20,
            "maxStorageBytes": 1073741824_u64,
            "entitlementState": "active",
            "localEntitlementState": "active"
        }],
        "memberships": [{
            "workspaceId": "workspace-audit",
            "userId": "owner-audit",
            "email": "owner-audit@example.test",
            "role": "owner",
            "resourceAccessMode": "full",
            "customCapabilities": null,
            "accountState": "active",
            "platformRole": null,
            "accessSource": "direct",
            "createdAt": "2026-01-01T00:00:00Z"
        }],
        "shareGrants": []
    })
}

#[test]
fn canonical_control_plane_export_is_sanitized_and_deterministic() {
    let bytes = serde_json::to_vec(&canonical_export()).unwrap();
    let mapped = map_legacy_control_plane_json(&bytes, "synthetic-audit").unwrap();
    assert_eq!(mapped.organizations.len(), 1);
    assert_eq!(mapped.workspaces.len(), 1);
    assert_eq!(mapped.members.len(), 1);

    let canonical = canonicalize_legacy_control_plane_json(&bytes, "synthetic-audit").unwrap();
    let value: Value = serde_json::from_slice(&canonical).unwrap();
    let keys = value
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(
        keys,
        ["memberships", "organizations", "shareGrants", "workspaces"]
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>()
    );
    let lower = String::from_utf8(canonical).unwrap().to_ascii_lowercase();
    for forbidden in ["password", "bearer", "cookie", "secret", "token"] {
        assert!(!lower.contains(forbidden), "forbidden field: {forbidden}");
    }
}

#[test]
fn raw_legacy_snapshot_is_not_accepted_as_a_canonical_control_export() {
    let snapshot = json!({
        "organizations": [],
        "workspaces": [],
        "memberships": [],
        "shareGrants": [],
        "authUsers": [{
            "userId": "synthetic-user",
            "email": "synthetic-user@example.test",
            "passwordHash": "<synthetic-redacted>",
            "passwordSalt": "<synthetic-redacted>"
        }],
        "workspaceProfiles": [],
        "workspaceProxies": [],
        "invites": [{"token": "<synthetic-redacted>"}],
        "tiktokCookies": [{"cookie": "<synthetic-redacted>"}]
    });
    let bytes = serde_json::to_vec(&snapshot).unwrap();
    let error = map_legacy_control_plane_json(&bytes, "synthetic-audit").unwrap_err();
    assert_eq!(error.code, "migration_control_plane_invalid");
    let error = canonicalize_legacy_control_plane_json(&bytes, "synthetic-audit").unwrap_err();
    assert_eq!(error.code, "migration_control_plane_invalid");
}
