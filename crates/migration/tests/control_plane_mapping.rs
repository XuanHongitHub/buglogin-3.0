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
        "memberships": [
            {
                "workspaceId": "workspace-a",
                "userId": "owner-a",
                "email": "Owner@Example.com",
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
    assert_eq!(mapping.resources[0].data.display_name, "profile-9abcdef");
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
fn blocks_non_unique_email_and_group_contract_gap() {
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
    assert_eq!(
        map(&group).unwrap_err().code,
        "migration_control_plane_contract_gap"
    );
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
    assert_eq!(
        map(&ambiguous).unwrap_err().fields["field"],
        json!("ambiguous profile resource binding")
    );
}
