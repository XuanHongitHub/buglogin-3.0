use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use buglogin_migration::MigrationKey;
use buglogin_migration::inspect::inspect_source;
use buglogin_migration::package::{PackageMetadata, build_encrypted_profile_package};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct Temp(PathBuf);

impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "buglogin-package-control-plane-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn write(&self, relative: &str, bytes: &[u8]) {
        let path = self.0.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn source(control_plane: &[u8]) -> Temp {
    let source = Temp::new();
    source.write(
        "version.json",
        br#"{"system":"buglogin-legacy","version":"1.1.84","instanceId":"control-package","os":"windows","architecture":"x86_64"}"#,
    );
    source.write("control-plane-export.json", control_plane);
    source.write(
        "profiles/01234567-89ab-cdef-0123-456789abcdef/metadata.json",
        br#"{"id":"01234567-89ab-cdef-0123-456789abcdef","name":"Primary profile","runtime":"wayfern"}"#,
    );
    source.write(
        "profiles/01234567-89ab-cdef-0123-456789abcdef/profile/Preferences",
        b"profile-data",
    );
    source
}

fn valid_export() -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "organizations": [{
            "id": "organization-a",
            "name": "BugLogin Operations",
            "slug": "buglogin-operations",
            "status": "active",
            "createdBy": "owner-a",
            "createdAt": "2024-12-31T00:00:00Z",
            "updatedAt": "2025-01-03T00:00:00Z"
        }],
        "workspaces": [{
            "id": "workspace-a",
            "organizationId": "organization-a",
            "name": "Operations",
            "mode": "team",
            "createdAt": "2025-01-01T00:00:00Z",
            "createdBy": "owner-a",
            "profileLimit": 100,
            "memberLimit": 20,
            "maxStorageBytes": 1073741824_u64,
            "entitlementState": "active",
            "localEntitlementState": "active"
        }],
        "memberships": [{
            "workspaceId": "workspace-a",
            "userId": "owner-a",
            "email": "owner-sensitive@example.com",
            "role": "owner",
            "resourceAccessMode": "full",
            "customCapabilities": null,
            "accountState": "active",
            "platformRole": null,
            "accessSource": "direct",
            "createdAt": "2025-01-01T00:00:00Z"
        }, {
            "workspaceId": "workspace-a",
            "userId": "member-a",
            "email": "member-sensitive@example.com",
            "role": "member",
            "resourceAccessMode": "restricted",
            "customCapabilities": [],
            "accountState": "active",
            "platformRole": null,
            "accessSource": "direct",
            "createdAt": "2025-01-02T00:00:00Z"
        }],
        "groups": [{
            "id": "group-a",
            "workspaceId": "workspace-a",
            "name": "Operations",
            "color": "#3366ff",
            "syncEnabled": true,
            "lastSync": null,
            "createdBy": "owner-a",
            "createdAt": "2025-01-01T00:00:00Z",
            "updatedAt": "2025-01-03T00:00:00Z"
        }],
        "profileGroupAssignments": [{
            "profileId": "prf_0123456789abcdef0123456789abcdef",
            "workspaceId": "workspace-a",
            "groupId": "group-a"
        }],
        "profiles": [{
            "id": "prf_0123456789abcdef0123456789abcdef",
            "workspaceId": "workspace-a",
            "name": "Primary profile",
            "runtime": "wayfern",
            "runtimeVersion": "154.0.8037.57",
            "releaseType": "stable",
            "proxyId": null,
            "vpnId": null,
            "groupId": null,
            "extensionGroupId": null,
            "ephemeral": false,
            "createdAt": "2025-01-01T00:00:00Z",
            "updatedAt": "2025-01-03T00:00:00Z",
            "tags": [],
            "note": null
        }],
        "shareGrants": [{
            "id": "share-a",
            "workspaceId": "workspace-a",
            "resourceType": "profile",
            "resourceId": "prf_0123456789abcdef0123456789abcdef",
            "recipientEmail": "member-sensitive@example.com",
            "accessMode": "run_sync_limited",
            "createdAt": "2025-01-03T00:00:00Z",
            "createdBy": "owner-a",
            "revokedAt": null
        }, {
            "id": "group-share-a",
            "workspaceId": "workspace-a",
            "resourceType": "group",
            "resourceId": "group-a",
            "recipientEmail": "member-sensitive@example.com",
            "accessMode": "run_sync_limited",
            "createdAt": "2025-01-03T00:00:00Z",
            "createdBy": "owner-a",
            "revokedAt": null
        }, {
            "id": "group-share-revoked",
            "workspaceId": "workspace-a",
            "resourceType": "group",
            "resourceId": "group-a",
            "recipientEmail": "member-sensitive@example.com",
            "accessMode": "run_sync_limited",
            "createdAt": "2025-01-03T00:00:00Z",
            "createdBy": "owner-a",
            "revokedAt": "2025-02-01T00:00:00Z"
        }]
    }))
    .unwrap()
}

fn metadata() -> PackageMetadata {
    PackageMetadata {
        created_at: "2026-07-21T00:00:00Z".into(),
        tool_version: "0.1.0".into(),
        tool_commit: "test".into(),
    }
}

#[test]
fn packages_all_control_plane_entities_without_report_leakage() {
    let source = source(&valid_export());
    let (report, snapshot) = inspect_source(source.path()).unwrap();
    let package = build_encrypted_profile_package(
        source.path(),
        &report,
        &snapshot,
        &metadata(),
        &MigrationKey::from_bytes([91; 32]),
    )
    .unwrap();

    for (path, expected_records) in [
        ("entities/accounts.jsonl", 2),
        ("entities/organizations.jsonl", 1),
        ("entities/workspaces.jsonl", 1),
        ("entities/teams.jsonl", 1),
        ("entities/members.jsonl", 2),
        ("entities/resources.jsonl", 1),
        ("entities/profile-resources.jsonl", 1),
        ("entities/grants.jsonl", 2),
        ("entities/groups.jsonl", 1),
        ("entities/group-grants.jsonl", 4),
    ] {
        let records = package
            .file(path)
            .unwrap_or_else(|| panic!("missing {path}"));
        assert_eq!(
            records
                .split(|byte| *byte == b'\n')
                .filter(|line| !line.is_empty())
                .count(),
            expected_records,
            "wrong record count for {path}"
        );
    }
    let profile: serde_json::Value = serde_json::from_slice(
        package
            .file("entities/profiles.jsonl")
            .unwrap()
            .split(|byte| *byte == b'\n')
            .next()
            .unwrap(),
    )
    .unwrap();
    let group: serde_json::Value = serde_json::from_slice(
        package
            .file("entities/groups.jsonl")
            .unwrap()
            .split(|byte| *byte == b'\n')
            .next()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(profile["data"]["group_id"], group["id"]);
    let group_grants = package.file("entities/group-grants.jsonl").unwrap();
    let group_grant_records = group_grants
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice::<serde_json::Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert!(
        group_grant_records
            .iter()
            .any(|grant| { grant["data"]["revoked_at"] == "2025-02-01T00:00:00Z" })
    );

    for (path, bytes) in package.files() {
        if path.starts_with("entities/") {
            continue;
        }
        let text = String::from_utf8_lossy(bytes);
        assert!(
            !text.contains("owner-sensitive@example.com"),
            "email leaked into {path}"
        );
        assert!(
            !text.contains("member-sensitive@example.com"),
            "email leaked into {path}"
        );
    }
}

#[test]
fn includes_hosted_profiles_missing_from_local_snapshot_as_preserve_only() {
    let mut value: serde_json::Value = serde_json::from_slice(&valid_export()).unwrap();
    value["profiles"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "id": "prf_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "workspaceId": "workspace-a",
            "name": "Cloud-only profile",
            "runtime": "bugox",
            "runtimeVersion": "1.0",
            "releaseType": "stable",
            "proxyId": null,
            "vpnId": null,
            "groupId": null,
            "extensionGroupId": null,
            "ephemeral": false,
            "createdAt": "2025-01-03T00:00:00Z",
            "updatedAt": "2025-01-03T00:00:00Z",
            "tags": [],
            "note": null
        }));
    let source = source(&serde_json::to_vec(&value).unwrap());
    let (report, snapshot) = inspect_source(source.path()).unwrap();
    let package = build_encrypted_profile_package(
        source.path(),
        &report,
        &snapshot,
        &metadata(),
        &MigrationKey::from_bytes([92; 32]),
    )
    .unwrap();
    let profiles = package
        .file("entities/profiles.jsonl")
        .unwrap()
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice::<serde_json::Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(profiles.len(), 2);
    let cloud_only = profiles
        .iter()
        .find(|profile| profile["data"]["name"] == "Cloud-only profile")
        .unwrap();
    assert_eq!(cloud_only["data"]["migration_class"], "preserve-only");
    assert_eq!(
        cloud_only["data"]["source_data_object"]["disposition"]["classification"],
        "excluded"
    );
    assert_eq!(
        package
            .file("entities/resources.jsonl")
            .unwrap()
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .count(),
        2
    );
    assert_eq!(
        package
            .file("entities/profile-resources.jsonl")
            .unwrap()
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .count(),
        2
    );
}

#[test]
fn rejects_dangling_profile_resource_binding() {
    let mut export: serde_json::Value = serde_json::from_slice(&valid_export()).unwrap();
    export["shareGrants"][0]["resourceId"] =
        serde_json::json!("prf_11111111111111111111111111111111");
    let source = source(&serde_json::to_vec(&export).unwrap());
    let (report, snapshot) = inspect_source(source.path()).unwrap();
    let error = build_encrypted_profile_package(
        source.path(),
        &report,
        &snapshot,
        &metadata(),
        &MigrationKey::from_bytes([94; 32]),
    )
    .unwrap_err();

    assert_eq!(error.code, "migration_control_plane_invalid");
    assert_eq!(
        error
            .fields
            .get("field")
            .and_then(serde_json::Value::as_str),
        Some("profile share grant has no hosted profile")
    );
}

#[test]
fn packages_pending_share_provenance_without_activating_it_or_exposing_email() {
    let mut export: serde_json::Value = serde_json::from_slice(&valid_export()).unwrap();
    let share = export["shareGrants"][0].clone();
    export["shareGrants"] = serde_json::json!([]);
    export["pendingShareGrants"] = serde_json::json!([{
        "id": share["id"],
        "workspaceId": "workspace-a",
        "resourceType": "profile",
        "resourceId": share["resourceId"],
        "resourceName": "Primary profile",
        "recipientIdentitySha256": "b".repeat(64),
        "accessMode": "run_sync_limited",
        "createdAt": "2025-01-03T00:00:00Z",
        "createdBy": "owner-a",
        "revokedAt": null,
        "disposition": "pending_invite",
        "inviteId": "invite-a",
        "inviteWorkspaceId": "workspace-a",
        "inviteIdentitySha256": "b".repeat(64),
        "inviteConsumedAt": null,
        "inviteRole": "member",
        "inviteCreatedAt": "2025-01-02T00:00:00Z",
        "inviteExpiresAt": "2025-02-01T00:00:00Z",
        "inviteCreatedBy": "owner-a"
    }]);
    export["pendingInvites"] = serde_json::json!([{
        "id": "invite-a",
        "workspaceId": "workspace-a",
        "identitySha256": "b".repeat(64),
        "role": "member",
        "createdAt": "2025-01-02T00:00:00Z",
        "expiresAt": "2025-02-01T00:00:00Z",
        "createdBy": "owner-a"
    }]);
    let source = source(&serde_json::to_vec(&export).unwrap());
    let (report, snapshot) = inspect_source(source.path()).unwrap();
    let package = build_encrypted_profile_package(
        source.path(),
        &report,
        &snapshot,
        &metadata(),
        &MigrationKey::from_bytes([92; 32]),
    )
    .unwrap();

    assert_eq!(package.file("entities/grants.jsonl"), Some(&b""[..]));
    let provenance = package
        .file("reports/pending-share-provenance.jsonl")
        .expect("pending share provenance report");
    assert_eq!(
        provenance
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .count(),
        1
    );
    let text = String::from_utf8_lossy(provenance);
    assert!(!text.contains("member-sensitive@example.com"));
    assert!(!text.contains("owner-sensitive@example.com"));
    let verified = buglogin_migration::verify::verify_package(&package).unwrap();
    assert!(
        verified
            .files
            .iter()
            .any(|file| file.path == "reports/pending-share-provenance.jsonl")
    );
    assert_eq!(
        package
            .file("reports/pending-invite-provenance.jsonl")
            .unwrap()
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .count(),
        1
    );
}

#[test]
fn rejects_uncontracted_control_tokens_without_leaking_values() {
    let mut export: serde_json::Value = serde_json::from_slice(&valid_export()).unwrap();
    export["rawInviteToken"] = serde_json::json!("never-export-this-token");
    let source = source(&serde_json::to_vec(&export).unwrap());
    let (report, snapshot) = inspect_source(source.path()).unwrap();
    let error = build_encrypted_profile_package(
        source.path(),
        &report,
        &snapshot,
        &metadata(),
        &MigrationKey::from_bytes([92; 32]),
    )
    .unwrap_err();

    assert_eq!(error.code, "migration_control_plane_invalid");
    assert!(!format!("{error:?}").contains("never-export-this-token"));
}

#[test]
fn rejects_control_export_without_matching_organization() {
    let mut export: serde_json::Value = serde_json::from_slice(&valid_export()).unwrap();
    export["organizations"] = serde_json::json!([]);
    let source = source(&serde_json::to_vec(&export).unwrap());
    let (report, snapshot) = inspect_source(source.path()).unwrap();
    let error = build_encrypted_profile_package(
        source.path(),
        &report,
        &snapshot,
        &metadata(),
        &MigrationKey::from_bytes([93; 32]),
    )
    .unwrap_err();

    assert_eq!(error.code, "migration_control_plane_invalid");
    assert_eq!(
        error
            .fields
            .get("field")
            .and_then(serde_json::Value::as_str),
        Some("workspace organization reference")
    );
}
