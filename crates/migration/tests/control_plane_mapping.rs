use buglogin_domain::ids::migration_uuid;
use buglogin_migration::legacy::map_legacy_control_plane_json;
use serde_json::{Value, json};

fn export() -> Value {
    json!({
        "organizations": [{
            "id": "organization-a",
            "name": "BugLogin Operations",
            "slug": "buglogin-operations",
            "status": "active",
            "createdBy": "owner-a",
            "createdAt": "2024-12-31T00:00:00Z",
            "updatedAt": "2025-01-05T00:00:00Z"
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
        "profiles": [{
            "id": "prf_0123456789abcdef0123456789abcdef",
            "workspaceId": "workspace-a",
            "name": "Profile A",
            "runtime": "bugmini",
            "runtimeVersion": null,
            "releaseType": "stable",
            "proxyId": null,
            "vpnId": null,
            "groupId": null,
            "extensionGroupId": null,
            "ephemeral": false,
            "createdAt": "2025-01-04T00:00:00Z",
            "updatedAt": "2025-01-04T00:00:00Z",
            "tags": [],
            "note": null
        }],
        "memberships": [
            {
                "workspaceId": "workspace-a",
                "userId": "owner-a",
                "email": "Owner@Example.com",
                "legacyAuthProvider": "password",
                "role": "owner",
                "resourceAccessMode": "full",
                "customCapabilities": null,
                "accountState": "active",
                "platformRole": null,
                "accessSource": "direct",
                "createdAt": "2025-01-01T00:00:00Z"
            },
            {
                "workspaceId": "workspace-a",
                "userId": "member-a",
                "email": "member@example.com",
                "legacyAuthProvider": "password_google",
                "role": "member",
                "resourceAccessMode": "restricted",
                "customCapabilities": [],
                "accountState": "active",
                "platformRole": null,
                "accessSource": "direct",
                "createdAt": "2025-01-02T00:00:00Z"
            },
            {
                "workspaceId": "workspace-a",
                "userId": "platform-a",
                "email": "platform@example.com",
                "legacyAuthProvider": "password",
                "role": "owner",
                "resourceAccessMode": "full",
                "customCapabilities": null,
                "accountState": "active",
                "platformRole": "platform_admin",
                "accessSource": "platform",
                "createdAt": "2025-01-03T00:00:00Z"
            }
        ],
        "shareGrants": [{
            "id": "share-a",
            "workspaceId": "workspace-a",
            "resourceType": "profile",
            "resourceId": "prf_0123456789abcdef0123456789abcdef",
            "recipientEmail": "MEMBER@example.com",
            "accessMode": "run_sync_limited",
            "createdAt": "2025-01-04T00:00:00Z",
            "createdBy": "owner-a",
            "revokedAt": null
        }]
    })
}

fn map(
    value: &Value,
) -> Result<buglogin_migration::legacy::LegacyControlPlaneMapping, buglogin_domain::errors::Problem>
{
    map_legacy_control_plane_json(
        serde_json::to_vec(value).unwrap().as_slice(),
        "control-source",
    )
}

#[test]
fn maps_accounts_default_team_members_and_profile_grants() {
    let mapping = map(&export()).unwrap();
    assert_eq!(mapping.accounts.len(), 3);
    assert_eq!(mapping.organizations.len(), 1);
    assert_eq!(mapping.workspaces.len(), 1);
    assert_eq!(mapping.teams.len(), 1);
    assert_eq!(mapping.members.len(), 2);
    assert_eq!(mapping.resources.len(), 1);
    assert_eq!(mapping.profile_resources.len(), 1);
    assert_eq!(mapping.grants.len(), 2);
    assert!(
        mapping
            .members
            .iter()
            .all(|member| member.source_ref.id != "workspace-a:platform-a")
    );
    assert_eq!(mapping.teams[0].data.member_ids.len(), 2);
    assert_eq!(mapping.organizations[0].data.name, "BugLogin Operations");
    let owner = mapping
        .accounts
        .iter()
        .find(|account| account.extensions["legacyEmail"] == "owner@example.com")
        .unwrap();
    assert_eq!(owner.extensions["legacyAuthProvider"], "password");
    assert_eq!(
        owner.extensions["legacyCredentialDisposition"],
        "not_transferred_reprovision_required"
    );
    assert_eq!(
        mapping.organizations[0].extensions["legacySlug"],
        json!("buglogin-operations")
    );
    assert_eq!(
        mapping.organizations[0].extensions["legacyStatus"],
        json!("active")
    );
    assert_eq!(
        mapping.workspaces[0].data.organization_id,
        mapping.organizations[0].id
    );
    assert_eq!(mapping.workspaces[0].data.policy.offline_entitled, false);
    let binding = &mapping.profile_resources[0];
    assert_eq!(binding.schema, "buglogin.profile-resource/1");
    assert_eq!(
        binding.data.profile_id,
        migration_uuid(
            "buglogin-legacy",
            "control-source",
            "profile",
            "01234567-89ab-cdef-0123-456789abcdef"
        )
        .to_string()
    );
    assert_eq!(binding.data.resource_id, mapping.resources[0].id);
    assert_eq!(binding.data.workspace_id, mapping.workspaces[0].id);
    assert_eq!(mapping.resources[0].data.display_name, "Profile A");
    assert_eq!(
        mapping
            .grants
            .iter()
            .map(|grant| grant.data.action.as_str())
            .collect::<Vec<_>>(),
        vec!["read", "execute"]
    );
}

#[test]
fn rejects_unsupported_legacy_auth_provider_without_importing_auth_material() {
    let mut source = export();
    source["memberships"][0]["legacyAuthProvider"] = json!("unsupported-provider");

    let error = map(&source).unwrap_err();

    assert_eq!(error.code, "migration_control_plane_invalid");
    assert_eq!(error.status, 422);
}

#[test]
fn blocks_missing_or_dangling_organization_records() {
    let mut missing = export();
    missing.as_object_mut().unwrap().remove("organizations");
    assert_eq!(
        map(&missing).unwrap_err().code,
        "migration_control_plane_invalid"
    );

    let mut dangling = export();
    dangling["workspaces"][0]["organizationId"] = json!("organization-missing");
    assert_eq!(
        map(&dangling).unwrap_err().code,
        "migration_control_plane_invalid"
    );
}

#[test]
fn blocks_missing_capability_state_and_entitlement_conflict() {
    let mut missing = export();
    missing["memberships"][0]
        .as_object_mut()
        .unwrap()
        .remove("customCapabilities");
    assert_eq!(
        map(&missing).unwrap_err().code,
        "migration_control_plane_invalid"
    );

    let mut conflict = export();
    conflict["workspaces"][0]["localEntitlementState"] = json!("read_only");
    assert_eq!(
        map(&conflict).unwrap_err().code,
        "migration_control_plane_invalid"
    );
}

#[test]
fn blocks_non_unique_email_and_maps_group_share_grants() {
    let mut duplicate = export();
    duplicate["memberships"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "workspaceId": "workspace-a",
            "userId": "duplicate-a",
            "email": "member@example.com",
            "role": "viewer",
            "resourceAccessMode": "restricted",
            "customCapabilities": null,
            "accountState": "active",
            "platformRole": null,
            "accessSource": "direct",
            "createdAt": "2025-01-05T00:00:00Z"
        }));
    assert_eq!(
        map(&duplicate).unwrap_err().code,
        "migration_control_plane_invalid"
    );

    let mut group = export();
    group["shareGrants"][0]["resourceType"] = json!("group");
    group["shareGrants"][0]["resourceId"] = json!("group-a");
    group["groups"] = json!([{
        "id": "group-a",
        "workspaceId": "workspace-a",
        "name": "Operations",
        "color": "#3366ff",
        "syncEnabled": true,
        "lastSync": null,
        "createdBy": "owner-a",
        "createdAt": "2025-01-01T00:00:00Z",
        "updatedAt": "2025-01-05T00:00:00Z"
    }]);
    let mapped = map(&group).unwrap();
    assert_eq!(mapped.groups.len(), 1);
    assert_eq!(mapped.group_grants.len(), 2);
    assert!(
        mapped
            .group_grants
            .iter()
            .any(|grant| grant.data.action == "read")
    );
    assert!(
        mapped
            .group_grants
            .iter()
            .all(|grant| grant.data.member_id == mapped.group_grants[0].data.member_id)
    );
    assert!(mapped.grants.is_empty());
}

#[test]
fn blocks_malformed_or_ambiguous_profile_resource_bindings() {
    let mut malformed = export();
    malformed["shareGrants"][0]["resourceId"] = json!("prf_ABCDEF");
    assert_eq!(
        map(&malformed).unwrap_err().fields["field"],
        json!("share grant profile resource ID")
    );

    let mut ambiguous = export();
    ambiguous["workspaces"].as_array_mut().unwrap().push(json!({
        "id": "workspace-b",
        "organizationId": "organization-a",
        "name": "Secondary",
        "mode": "personal",
        "createdAt": "2025-01-01T00:00:00Z",
        "createdBy": "owner-a",
        "profileLimit": 100,
        "memberLimit": 20,
        "maxStorageBytes": 1073741824_u64,
        "entitlementState": "active",
        "localEntitlementState": "active"
    }));
    ambiguous["memberships"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "workspaceId": "workspace-b",
            "userId": "member-a",
            "email": "member@example.com",
            "legacyAuthProvider": "password_google",
            "role": "member",
            "resourceAccessMode": "restricted",
            "customCapabilities": [],
            "accountState": "active",
            "platformRole": null,
            "accessSource": "direct",
            "createdAt": "2025-01-02T00:00:00Z"
        }));
    let mut second_share = ambiguous["shareGrants"][0].clone();
    second_share["id"] = json!("share-b");
    second_share["workspaceId"] = json!("workspace-b");
    ambiguous["shareGrants"]
        .as_array_mut()
        .unwrap()
        .push(second_share);
    let error = map(&ambiguous).unwrap_err();
    assert_eq!(error.code, "migration_control_plane_invalid");
    assert_eq!(
        error.fields["field"],
        json!("profile share grant has no hosted profile")
    );
}

#[test]
fn preserves_unconsumed_invite_share_as_inert_provenance() {
    let mut value = export();
    let grant = value["shareGrants"][0].clone();
    value["shareGrants"] = json!([]);
    value["profiles"] = json!([]);
    value["pendingShareGrants"] = json!([{
        "id": grant["id"],
        "workspaceId": "workspace-a",
        "resourceType": "profile",
        "resourceId": grant["resourceId"],
        "resourceName": "Profile from pending invite",
        "recipientIdentitySha256": "a".repeat(64),
        "accessMode": "run_sync_limited",
        "createdAt": "2025-01-04T00:00:00Z",
        "createdBy": "owner-a",
        "revokedAt": null,
        "disposition": "pending_invite",
        "inviteId": "invite-a",
        "inviteWorkspaceId": "workspace-a",
        "inviteIdentitySha256": "a".repeat(64),
        "inviteConsumedAt": null,
        "inviteRole": "member",
        "inviteCreatedAt": "2025-01-02T00:00:00Z",
        "inviteExpiresAt": "2025-02-01T00:00:00Z",
        "inviteCreatedBy": "owner-a"
    }]);
    value["pendingInvites"] = json!([{
        "id": "invite-a",
        "workspaceId": "workspace-a",
        "identitySha256": "a".repeat(64),
        "role": "member",
        "createdAt": "2025-01-02T00:00:00Z",
        "expiresAt": "2025-02-01T00:00:00Z",
        "createdBy": "owner-a"
    }]);

    let mapping = map(&value).unwrap();
    assert!(mapping.grants.is_empty());
    assert!(mapping.resources.is_empty());
    assert_eq!(mapping.pending_share_grants.len(), 1);
    assert_eq!(mapping.pending_share_grants[0].id, "share-a");
    assert_eq!(
        mapping.pending_share_grants[0].disposition,
        "pending_invite"
    );

    value["pendingShareGrants"][0]["inviteConsumedAt"] = json!("2025-01-05T00:00:00Z");
    assert_eq!(
        map(&value).unwrap_err().code,
        "migration_control_plane_invalid"
    );
}

#[test]
fn preserves_active_grants_to_missing_profiles_as_inert_provenance() {
    let mut value = export();
    let grant = value["shareGrants"][0].clone();
    value["shareGrants"] = json!([]);
    value["profiles"] = json!([]);
    value["pendingShareGrants"] = json!([{
        "id": grant["id"],
        "workspaceId": "workspace-a",
        "resourceType": "profile",
        "resourceId": grant["resourceId"],
        "resourceName": null,
        "recipientIdentitySha256": "a".repeat(64),
        "accessMode": "run_sync_limited",
        "createdAt": "2025-01-04T00:00:00Z",
        "createdBy": "owner-a",
        "revokedAt": null,
        "disposition": "orphaned_profile",
        "inviteId": null,
        "inviteWorkspaceId": null,
        "inviteIdentitySha256": null,
        "inviteConsumedAt": null,
        "inviteRole": null,
        "inviteCreatedAt": null,
        "inviteExpiresAt": null,
        "inviteCreatedBy": null
    }]);

    let mapping = map(&value).unwrap();
    assert!(mapping.grants.is_empty());
    assert!(mapping.resources.is_empty());
    assert_eq!(mapping.pending_share_grants.len(), 1);
    assert_eq!(mapping.pending_share_grants[0].id, "share-a");
    assert_eq!(
        mapping.pending_share_grants[0].disposition,
        "orphaned_profile"
    );

    value["shareGrants"] = json!([grant]);
    value["pendingShareGrants"] = json!([]);
    assert_eq!(
        map(&value).unwrap_err().code,
        "migration_control_plane_invalid"
    );
}

#[test]
fn preserves_platform_admin_share_rows_without_creating_workspace_grants() {
    let mut value = export();
    let profile_id = "prf_0123456789abcdef0123456789abcdef";
    value["shareGrants"] = json!([]);
    value["profiles"] = json!([{
        "id": profile_id,
        "workspaceId": "workspace-a",
        "name": "Profile A",
        "runtime": "bugmini",
        "runtimeVersion": null,
        "releaseType": "stable",
        "proxyId": null,
        "vpnId": null,
        "groupId": null,
        "extensionGroupId": null,
        "ephemeral": false,
        "createdAt": "2025-01-04T00:00:00Z",
        "updatedAt": "2025-01-04T00:00:00Z",
        "tags": [],
        "note": null
    }]);
    value["pendingShareGrants"] = json!([{
        "id": "platform-share-a",
        "workspaceId": "workspace-a",
        "resourceType": "profile",
        "resourceId": profile_id,
        "resourceName": "Profile A",
        "recipientIdentitySha256": "6bfbb1749bc568ff770076a53e8272c273415d9b974d2a18155e987e31f403c4",
        "accessMode": "run_sync_limited",
        "createdAt": "2025-01-04T00:00:00Z",
        "createdBy": "owner-a",
        "revokedAt": null,
        "disposition": "platform_recipient",
        "inviteId": null,
        "inviteWorkspaceId": null,
        "inviteIdentitySha256": null,
        "inviteConsumedAt": null,
        "inviteRole": null,
        "inviteCreatedAt": null,
        "inviteExpiresAt": null,
        "inviteCreatedBy": null
    }]);

    let mapping = map(&value).unwrap();
    assert!(mapping.grants.is_empty());
    assert_eq!(mapping.pending_share_grants.len(), 1);
    assert_eq!(
        mapping.pending_share_grants[0].disposition,
        "platform_recipient"
    );
}

#[test]
fn does_not_allow_pending_provenance_to_downgrade_a_member_grant() {
    let mut value = export();
    let grant = value["shareGrants"][0].clone();
    value["shareGrants"] = json!([]);
    value["pendingShareGrants"] = json!([{
        "id": grant["id"],
        "workspaceId": "workspace-a",
        "resourceType": "profile",
        "resourceId": grant["resourceId"],
        "resourceName": null,
        "recipientIdentitySha256": "b6e346dee08f8e8cf029179eb5177b5c2fc1a6e8ba01ab8ff4e1b8d56e89298c",
        "accessMode": "run_sync_limited",
        "createdAt": "2025-01-04T00:00:00Z",
        "createdBy": "owner-a",
        "revokedAt": null,
        "disposition": "pending_invite",
        "inviteId": "invite-a",
        "inviteWorkspaceId": "workspace-a",
        "inviteIdentitySha256": "b6e346dee08f8e8cf029179eb5177b5c2fc1a6e8ba01ab8ff4e1b8d56e89298c",
        "inviteConsumedAt": null,
        "inviteRole": "member",
        "inviteCreatedAt": "2025-01-02T00:00:00Z",
        "inviteExpiresAt": "2025-02-01T00:00:00Z",
        "inviteCreatedBy": "owner-a"
    }]);

    assert_eq!(
        map(&value).unwrap_err().code,
        "migration_control_plane_invalid"
    );
}

#[test]
fn preserves_revoked_profile_grants_as_inert_provenance() {
    let mut value = export();
    value["profiles"] = json!([]);
    value["shareGrants"][0]["recipientEmail"] = json!("member@example.com");
    value["shareGrants"][0]["revokedAt"] = json!("2025-02-01T00:00:00Z");

    let mapping = map(&value).unwrap();
    assert!(mapping.grants.is_empty());
    assert!(mapping.resources.is_empty());
    assert_eq!(mapping.pending_share_grants.len(), 1);
    assert_eq!(
        mapping.pending_share_grants[0].disposition,
        "revoked_member"
    );
    assert_eq!(
        mapping.pending_share_grants[0].revoked_at.as_deref(),
        Some("2025-02-01T00:00:00Z")
    );
}
