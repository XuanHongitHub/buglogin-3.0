use std::collections::{BTreeMap, BTreeSet};

use buglogin_domain::{errors::Problem, ids::migration_uuid};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use super::{NeutralGroupData, NeutralGroupRecord, ProfileSourceRef};

const LEGACY_ACTIONS: &[&str] = &[
    "create_profile",
    "clone_profile",
    "rename_profile",
    "delete_profile",
    "stop_profile",
    "delete_selected_profiles",
    "assign_group",
    "assign_proxy",
    "assign_extension_group",
    "toggle_profile_sync",
    "update_profile_tags",
    "update_profile_note",
    "update_profile_vpn",
    "share_profile",
    "share_group",
    "export_profile",
    "force_take_lock",
];

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LegacyControlPlaneExport {
    pub organizations: Vec<LegacyControlOrganization>,
    pub workspaces: Vec<LegacyControlWorkspace>,
    pub memberships: Vec<LegacyControlMembership>,
    pub share_grants: Vec<LegacyControlShareGrant>,
    #[serde(default)]
    pub profiles: Vec<LegacyControlProfile>,
    #[serde(default)]
    pub groups: Vec<LegacyControlGroup>,
    #[serde(default)]
    pub profile_group_assignments: Vec<LegacyControlProfileGroupAssignment>,
    #[serde(default)]
    pub pending_share_grants: Vec<LegacyPendingShareGrant>,
    #[serde(default)]
    pub pending_invites: Vec<LegacyPendingInvite>,
    #[serde(default)]
    pub legacy_credentials: Option<crate::legacy_account_credentials::EncryptedLegacyCredentials>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LegacyControlProfile {
    pub id: String,
    pub workspace_id: String,
    pub name: String,
    pub runtime: String,
    pub runtime_version: Option<String>,
    pub release_type: String,
    pub proxy_id: Option<String>,
    pub vpn_id: Option<String>,
    pub group_id: Option<String>,
    pub extension_group_id: Option<String>,
    pub ephemeral: bool,
    pub created_at: String,
    pub updated_at: String,
    pub tags: Vec<String>,
    pub note: Option<String>,
}

/// Source grants aimed at an unconsumed invite are preserved as migration
/// provenance only. They must never become an active BugLogin grant.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LegacyPendingShareGrant {
    pub id: String,
    pub workspace_id: String,
    pub resource_type: String,
    pub resource_id: String,
    pub resource_name: Option<String>,
    pub recipient_identity_sha256: String,
    pub access_mode: String,
    pub created_at: String,
    pub created_by: String,
    pub revoked_at: Option<String>,
    pub disposition: String,
    pub invite_id: Option<String>,
    pub invite_workspace_id: Option<String>,
    pub invite_identity_sha256: Option<String>,
    pub invite_consumed_at: Option<String>,
    pub invite_role: Option<String>,
    pub invite_created_at: Option<String>,
    pub invite_expires_at: Option<String>,
    pub invite_created_by: Option<String>,
}

/// Pending 1.0 invitations are evidence only. Their bearer tokens are never
/// exported and a new invitation must be issued in BugLogin 3.0.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LegacyPendingInvite {
    pub id: String,
    pub workspace_id: String,
    pub identity_sha256: String,
    pub role: String,
    pub created_at: String,
    pub expires_at: String,
    pub created_by: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LegacyControlGroup {
    pub id: String,
    pub workspace_id: String,
    pub name: String,
    pub color: Option<String>,
    pub sync_enabled: bool,
    pub last_sync: Option<String>,
    pub created_by: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LegacyControlProfileGroupAssignment {
    pub profile_id: String,
    pub workspace_id: String,
    pub group_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LegacyControlOrganization {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub status: String,
    pub created_by: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LegacyControlWorkspace {
    pub id: String,
    pub organization_id: String,
    pub name: String,
    pub mode: String,
    pub created_at: String,
    pub created_by: String,
    pub profile_limit: u32,
    pub member_limit: u32,
    pub max_storage_bytes: u64,
    pub entitlement_state: String,
    pub local_entitlement_state: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LegacyControlMembership {
    pub workspace_id: String,
    pub user_id: String,
    pub email: String,
    #[serde(default)]
    pub legacy_auth_provider: Option<String>,
    pub role: String,
    pub resource_access_mode: String,
    #[serde(default, deserialize_with = "deserialize_capabilities")]
    pub custom_capabilities: LegacyCapabilityOverride,
    pub account_state: String,
    pub platform_role: Option<String>,
    pub access_source: String,
    pub created_at: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum LegacyCapabilityOverride {
    #[default]
    Missing,
    RoleDefault,
    Explicit(Vec<String>),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LegacyControlShareGrant {
    pub id: String,
    pub workspace_id: String,
    pub resource_type: String,
    pub resource_id: String,
    #[serde(default)]
    pub resource_name: Option<String>,
    pub recipient_email: String,
    pub access_mode: String,
    pub created_at: String,
    pub created_by: String,
    pub revoked_at: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct NeutralGroupShareGrantData {
    pub workspace_id: String,
    pub group_id: String,
    pub member_id: String,
    pub action: String,
    pub effect: String,
    pub revoked_at: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct NeutralGroupShareGrantRecord {
    pub schema: String,
    pub source_ref: ProfileSourceRef,
    pub id: String,
    pub revision: u32,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub data: NeutralGroupShareGrantData,
    pub extensions: Map<String, Value>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct NeutralProfileGroupAssignment {
    pub profile_id: String,
    pub workspace_id: String,
    pub group_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LegacyControlPlaneMapping {
    pub accounts: Vec<NeutralAccountRecord>,
    pub organizations: Vec<NeutralOrganizationRecord>,
    pub workspaces: Vec<NeutralWorkspaceRecord>,
    pub teams: Vec<NeutralTeamRecord>,
    pub members: Vec<NeutralMemberRecord>,
    pub resources: Vec<NeutralResourceRecord>,
    pub profile_resources: Vec<NeutralProfileResourceRecord>,
    pub grants: Vec<NeutralGrantRecord>,
    pub groups: Vec<NeutralGroupRecord>,
    pub profiles: Vec<super::NeutralProfileRecord>,
    pub group_grants: Vec<NeutralGroupShareGrantRecord>,
    pub profile_group_assignments: Vec<NeutralProfileGroupAssignment>,
    pub pending_share_grants: Vec<LegacyPendingShareGrant>,
    pub pending_invites: Vec<LegacyPendingInvite>,
}

macro_rules! neutral_record {
    ($record:ident, $data:ident) => {
        #[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
        #[serde(rename_all = "snake_case", deny_unknown_fields)]
        pub struct $record {
            pub schema: String,
            pub source_ref: ProfileSourceRef,
            pub id: String,
            pub revision: u32,
            pub created_at: Option<String>,
            pub updated_at: Option<String>,
            pub data: $data,
            pub extensions: Map<String, Value>,
        }
    };
}

neutral_record!(NeutralAccountRecord, NeutralAccountData);
neutral_record!(NeutralOrganizationRecord, NeutralOrganizationData);
neutral_record!(NeutralWorkspaceRecord, NeutralWorkspaceData);
neutral_record!(NeutralTeamRecord, NeutralTeamData);
neutral_record!(NeutralMemberRecord, NeutralMemberData);
neutral_record!(NeutralResourceRecord, NeutralResourceData);
neutral_record!(NeutralProfileResourceRecord, NeutralProfileResourceData);
neutral_record!(NeutralGrantRecord, NeutralGrantData);

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct NeutralAccountData {
    pub status: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct NeutralOrganizationData {
    pub account_id: String,
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct NeutralWorkspacePolicy {
    pub max_profiles: u32,
    pub max_members: u32,
    pub max_storage_bytes: u64,
    pub offline_entitled: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct NeutralWorkspaceData {
    pub organization_id: String,
    pub name: String,
    pub policy: NeutralWorkspacePolicy,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct NeutralTeamData {
    pub workspace_id: String,
    pub name: String,
    pub member_ids: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct NeutralMemberData {
    pub account_id: String,
    pub workspace_id: String,
    pub team_id: Option<String>,
    pub role: String,
    pub status: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct NeutralResourceData {
    pub workspace_id: String,
    pub kind: String,
    pub state: String,
    pub display_name: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct NeutralProfileResourceData {
    pub profile_id: String,
    pub resource_id: String,
    pub workspace_id: String,
    pub archive_state: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct NeutralGrantData {
    pub workspace_id: String,
    pub resource_id: String,
    pub member_id: String,
    pub action: String,
    pub effect: String,
}

#[derive(Clone)]
struct AccountAccumulator {
    email: String,
    status: String,
    platform_role: Option<String>,
    legacy_auth_provider: Option<String>,
    created_at: String,
    updated_at: String,
}

pub fn map_legacy_control_plane_json(
    bytes: &[u8],
    source_instance_id: &str,
) -> Result<LegacyControlPlaneMapping, Problem> {
    let export = serde_json::from_slice(bytes)
        .map_err(|error| invalid("control export JSON").with_detail(error.to_string()))?;
    map_legacy_control_plane(&export, source_instance_id)
}

pub fn canonicalize_legacy_control_plane_json(
    bytes: &[u8],
    source_instance_id: &str,
) -> Result<Vec<u8>, Problem> {
    let mut value: Value = serde_json::from_slice(bytes)
        .map_err(|error| invalid("control export JSON").with_detail(error.to_string()))?;
    let export: LegacyControlPlaneExport = serde_json::from_value(value.clone())
        .map_err(|error| invalid("control export JSON").with_detail(error.to_string()))?;
    map_legacy_control_plane(&export, source_instance_id)?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| invalid("control export JSON"))?;
    sort_records(object, "organizations", &["id"])?;
    sort_records(object, "workspaces", &["organizationId", "id"])?;
    sort_records(object, "memberships", &["workspaceId", "email", "userId"])?;
    sort_records(object, "shareGrants", &["workspaceId", "resourceId", "id"])?;
    if object.contains_key("profiles") {
        sort_records(object, "profiles", &["workspaceId", "id"])?;
    }
    if object.contains_key("pendingShareGrants") {
        sort_records(
            object,
            "pendingShareGrants",
            &["workspaceId", "resourceId", "id"],
        )?;
    }
    if object.contains_key("pendingInvites") {
        sort_records(object, "pendingInvites", &["workspaceId", "id"])?;
    }
    if object.contains_key("groups") {
        sort_records(object, "groups", &["workspaceId", "id"])?;
    }
    if object.contains_key("profileGroupAssignments") {
        sort_records(
            object,
            "profileGroupAssignments",
            &["workspaceId", "profileId"],
        )?;
    }
    let mut canonical =
        serde_json::to_vec_pretty(&value).map_err(|_| invalid("control export serialization"))?;
    canonical.push(b'\n');
    Ok(canonical)
}

fn sort_records(
    object: &mut Map<String, Value>,
    field: &str,
    keys: &[&str],
) -> Result<(), Problem> {
    let records = object
        .get_mut(field)
        .and_then(Value::as_array_mut)
        .ok_or_else(|| invalid("control export collection"))?;
    records.sort_by_cached_key(|record| {
        keys.iter()
            .map(|key| record.get(*key).and_then(Value::as_str).unwrap_or_default())
            .collect::<Vec<_>>()
            .join("\u{0}")
    });
    Ok(())
}

pub fn map_legacy_control_plane(
    export: &LegacyControlPlaneExport,
    source_instance_id: &str,
) -> Result<LegacyControlPlaneMapping, Problem> {
    required(source_instance_id, "source instance ID")?;

    let mut organizations_by_source = BTreeMap::new();
    for organization in &export.organizations {
        validate_organization(organization)?;
        if organizations_by_source
            .insert(organization.id.clone(), organization)
            .is_some()
        {
            return Err(invalid("duplicate organization ID"));
        }
    }

    let mut workspaces_by_source = BTreeMap::new();
    for workspace in &export.workspaces {
        validate_workspace(workspace, &organizations_by_source)?;
        if workspaces_by_source
            .insert(workspace.id.clone(), workspace)
            .is_some()
        {
            return Err(invalid("duplicate workspace ID"));
        }
    }

    let mut groups_by_source = BTreeMap::new();
    for group in &export.groups {
        validate_control_group(group, &workspaces_by_source)?;
        if groups_by_source.insert(group.id.clone(), group).is_some() {
            return Err(invalid("duplicate workspace group ID"));
        }
    }
    let mut profiles_by_source = BTreeMap::new();
    for profile in &export.profiles {
        validate_control_profile(profile, &workspaces_by_source)?;
        if profiles_by_source
            .insert(profile.id.clone(), profile)
            .is_some()
        {
            return Err(invalid("duplicate hosted profile ID"));
        }
    }
    if !profiles_by_source.is_empty() {
        for assignment in &export.profile_group_assignments {
            if profiles_by_source
                .get(&assignment.profile_id)
                .is_none_or(|profile| profile.workspace_id != assignment.workspace_id)
            {
                return Err(invalid(
                    "profile group assignment has no matching hosted profile",
                ));
            }
        }
    }
    let mut accounts_by_source = BTreeMap::<String, AccountAccumulator>::new();
    let mut account_by_email = BTreeMap::<String, String>::new();
    let mut memberships_by_key = BTreeMap::new();
    for membership in &export.memberships {
        validate_membership(membership, &workspaces_by_source)?;
        let key = (membership.workspace_id.clone(), membership.user_id.clone());
        if memberships_by_key.insert(key, membership).is_some() {
            return Err(invalid("duplicate workspace membership"));
        }

        let email = normalize_email(&membership.email)?;
        if let Some(existing_user_id) =
            account_by_email.insert(email.clone(), membership.user_id.clone())
            && existing_user_id != membership.user_id
        {
            return Err(invalid("email resolves to multiple accounts"));
        }

        let status = map_account_state(&membership.account_state)?;
        validate_platform_role(membership.platform_role.as_deref())?;
        validate_legacy_auth_provider(membership.legacy_auth_provider.as_deref())?;
        match accounts_by_source.get_mut(&membership.user_id) {
            Some(account) => {
                if account.email != email
                    || account.status != status
                    || account.platform_role != membership.platform_role
                    || account.legacy_auth_provider != membership.legacy_auth_provider
                {
                    return Err(invalid("conflicting account identity"));
                }
                if parse_timestamp(&membership.created_at, "membership createdAt")?
                    < parse_timestamp(&account.created_at, "account createdAt")?
                {
                    account.created_at.clone_from(&membership.created_at);
                }
                if parse_timestamp(&membership.created_at, "membership createdAt")?
                    > parse_timestamp(&account.updated_at, "account updatedAt")?
                {
                    account.updated_at.clone_from(&membership.created_at);
                }
            }
            None => {
                accounts_by_source.insert(
                    membership.user_id.clone(),
                    AccountAccumulator {
                        email,
                        status,
                        platform_role: membership.platform_role.clone(),
                        legacy_auth_provider: membership.legacy_auth_provider.clone(),
                        created_at: membership.created_at.clone(),
                        updated_at: membership.created_at.clone(),
                    },
                );
            }
        }
    }

    for workspace in workspaces_by_source.values() {
        if !accounts_by_source.contains_key(&workspace.created_by) {
            return Err(invalid("workspace creator account"));
        }
    }
    for organization in organizations_by_source.values() {
        if !accounts_by_source.contains_key(&organization.created_by) {
            return Err(invalid("organization creator account"));
        }
    }

    build_mapping(
        export,
        source_instance_id,
        organizations_by_source,
        workspaces_by_source,
        groups_by_source,
        profiles_by_source,
        accounts_by_source,
        account_by_email,
        memberships_by_key,
    )
}

fn build_mapping(
    export: &LegacyControlPlaneExport,
    instance_id: &str,
    organizations_by_source: BTreeMap<String, &LegacyControlOrganization>,
    workspaces_by_source: BTreeMap<String, &LegacyControlWorkspace>,
    groups_by_source: BTreeMap<String, &LegacyControlGroup>,
    profiles_by_source: BTreeMap<String, &LegacyControlProfile>,
    accounts_by_source: BTreeMap<String, AccountAccumulator>,
    account_by_email: BTreeMap<String, String>,
    memberships_by_key: BTreeMap<(String, String), &LegacyControlMembership>,
) -> Result<LegacyControlPlaneMapping, Problem> {
    let legacy_credentials_encrypted = export.legacy_credentials.is_some();
    let accounts = accounts_by_source
        .iter()
        .map(|(source_id, account)| {
            account_record(
                instance_id,
                source_id,
                account,
                legacy_credentials_encrypted,
            )
        })
        .collect();
    let organizations = organizations_by_source
        .values()
        .map(|organization| organization_record(instance_id, organization))
        .collect();

    let mut workspaces = Vec::with_capacity(workspaces_by_source.len());
    let mut teams = Vec::new();
    let mut default_team_by_workspace = BTreeMap::new();
    for workspace in workspaces_by_source.values() {
        workspaces.push(workspace_record(instance_id, workspace));
        if workspace.mode == "team" {
            let source_id = format!("{}:default", workspace.id);
            let team_id = mapped_id(instance_id, "team", &source_id);
            default_team_by_workspace.insert(workspace.id.clone(), team_id.clone());
            teams.push(team_record(instance_id, workspace, team_id));
        }
    }

    let mut members = Vec::new();
    let mut member_id_by_workspace_email = BTreeMap::new();
    for membership in memberships_by_key.values() {
        if membership.access_source == "platform" {
            if membership.platform_role.as_deref() != Some("platform_admin") {
                return Err(invalid("platform access without platform_admin"));
            }
            continue;
        }
        let member = member_record(
            instance_id,
            membership,
            default_team_by_workspace
                .get(&membership.workspace_id)
                .cloned(),
        )?;
        member_id_by_workspace_email.insert(
            (
                membership.workspace_id.clone(),
                normalize_email(&membership.email)?,
            ),
            member.id.clone(),
        );
        members.push(member);
    }
    for team in &mut teams {
        team.data.member_ids = members
            .iter()
            .filter(|member| member.data.team_id.as_ref() == Some(&team.id))
            .map(|member| member.id.clone())
            .collect();
    }

    let groups = groups_by_source
        .values()
        .map(|group| control_group_record(instance_id, group))
        .collect::<Vec<_>>();
    let profiles = profiles_by_source
        .values()
        .map(|profile| hosted_profile_record(instance_id, profile))
        .collect::<Result<Vec<_>, _>>()?;
    let (resources, profile_resources, grants, group_grants) = map_share_grants(
        export,
        instance_id,
        &workspaces_by_source,
        &groups_by_source,
        &memberships_by_key,
        &account_by_email,
        &member_id_by_workspace_email,
    )?;

    let pending_invites = validate_pending_invites(export, &workspaces_by_source)?;
    Ok(LegacyControlPlaneMapping {
        accounts,
        organizations,
        workspaces,
        teams,
        members,
        resources,
        profile_resources,
        grants,
        groups,
        profiles,
        group_grants,
        profile_group_assignments: map_profile_group_assignments(
            export,
            instance_id,
            &workspaces_by_source,
            &groups_by_source,
        )?,
        pending_share_grants: validate_pending_share_grants(
            export,
            &workspaces_by_source,
            &groups_by_source,
            &pending_invites,
        )?,
        pending_invites,
    })
}

fn validate_pending_invites(
    export: &LegacyControlPlaneExport,
    workspaces: &BTreeMap<String, &LegacyControlWorkspace>,
) -> Result<Vec<LegacyPendingInvite>, Problem> {
    let mut seen = BTreeSet::new();
    let mut invites = export.pending_invites.clone();
    for invite in &invites {
        if !seen.insert(invite.id.as_str()) {
            return Err(invalid("duplicate pending invite ID"));
        }
        required(&invite.id, "pending invite ID")?;
        if !workspaces.contains_key(&invite.workspace_id) {
            return Err(invalid("pending invite workspace reference"));
        }
        validate_identity_hash(&invite.identity_sha256)?;
        map_member_role(&invite.role)?;
        required(&invite.created_by, "pending invite createdBy")?;
        parse_timestamp(&invite.created_at, "pending invite createdAt")?;
        parse_timestamp(&invite.expires_at, "pending invite expiresAt")?;
    }
    invites.sort_by(|left, right| {
        (&left.workspace_id, &left.id).cmp(&(&right.workspace_id, &right.id))
    });
    Ok(invites)
}

fn validate_pending_share_grants(
    export: &LegacyControlPlaneExport,
    workspaces: &BTreeMap<String, &LegacyControlWorkspace>,
    groups: &BTreeMap<String, &LegacyControlGroup>,
    pending_invites: &[LegacyPendingInvite],
) -> Result<Vec<LegacyPendingShareGrant>, Problem> {
    let hosted_profiles = export
        .profiles
        .iter()
        .map(|profile| (profile.workspace_id.as_str(), profile.id.as_str()))
        .collect::<BTreeSet<_>>();
    let direct_members = export
        .memberships
        .iter()
        .filter(|membership| membership.access_source == "direct")
        .map(|membership| {
            (
                membership.workspace_id.as_str(),
                identity_hash(&membership.email),
            )
        })
        .collect::<BTreeSet<_>>();
    let platform_members = export
        .memberships
        .iter()
        .filter(|membership| membership.access_source == "platform")
        .map(|membership| {
            (
                membership.workspace_id.as_str(),
                identity_hash(&membership.email),
            )
        })
        .collect::<BTreeSet<_>>();
    let active_ids = export
        .share_grants
        .iter()
        .filter(|grant| {
            !(grant.resource_type == "profile"
                && grant.revoked_at.is_some()
                && direct_members.contains(&(
                    grant.workspace_id.as_str(),
                    identity_hash(&grant.recipient_email),
                )))
        })
        .map(|grant| grant.id.as_str())
        .collect::<BTreeSet<_>>();
    let mut seen = BTreeSet::new();
    let mut pending = export.pending_share_grants.clone();
    for share in export
        .share_grants
        .iter()
        .filter(|share| share.resource_type == "profile" && share.revoked_at.is_some())
    {
        let recipient_hash = identity_hash(&share.recipient_email);
        if direct_members.contains(&(share.workspace_id.as_str(), recipient_hash.clone())) {
            pending.push(LegacyPendingShareGrant {
                id: share.id.clone(),
                workspace_id: share.workspace_id.clone(),
                resource_type: share.resource_type.clone(),
                resource_id: share.resource_id.clone(),
                resource_name: share.resource_name.clone(),
                recipient_identity_sha256: recipient_hash,
                access_mode: share.access_mode.clone(),
                created_at: share.created_at.clone(),
                created_by: share.created_by.clone(),
                revoked_at: share.revoked_at.clone(),
                disposition: "revoked_member".into(),
                invite_id: None,
                invite_workspace_id: None,
                invite_identity_sha256: None,
                invite_consumed_at: None,
                invite_role: None,
                invite_created_at: None,
                invite_expires_at: None,
                invite_created_by: None,
            });
        }
    }
    for grant in &pending {
        if !seen.insert(grant.id.as_str()) || active_ids.contains(grant.id.as_str()) {
            return Err(invalid("duplicate share grant provenance ID"));
        }
        required(&grant.id, "pending share grant ID")?;
        if !workspaces.contains_key(&grant.workspace_id) {
            return Err(invalid("pending share grant workspace reference"));
        }
        match grant.resource_type.as_str() {
            "profile" => {
                legacy_local_profile_id(&grant.resource_id)?;
            }
            "group" => {
                let group = groups
                    .get(&grant.resource_id)
                    .ok_or_else(|| invalid("pending share group reference"))?;
                if group.workspace_id != grant.workspace_id {
                    return Err(invalid("pending share group workspace"));
                }
            }
            _ => return Err(invalid("pending share resource type")),
        }
        validate_identity_hash(&grant.recipient_identity_sha256)?;
        let is_direct_member = direct_members.contains(&(
            grant.workspace_id.as_str(),
            grant.recipient_identity_sha256.clone(),
        ));
        required(&grant.created_by, "pending share createdBy")?;
        parse_timestamp(&grant.created_at, "pending share createdAt")?;
        share_actions(&grant.access_mode)?;
        if grant.disposition == "orphaned_profile" {
            if grant.resource_type != "profile"
                || grant.revoked_at.is_some()
                || hosted_profiles
                    .contains(&(grant.workspace_id.as_str(), grant.resource_id.as_str()))
                || grant.invite_id.is_some()
                || grant.invite_workspace_id.is_some()
                || grant.invite_identity_sha256.is_some()
                || grant.invite_consumed_at.is_some()
                || grant.invite_role.is_some()
                || grant.invite_created_at.is_some()
                || grant.invite_expires_at.is_some()
                || grant.invite_created_by.is_some()
            {
                return Err(invalid("orphaned profile share provenance state"));
            }
        } else if grant.disposition == "platform_recipient" {
            if grant.revoked_at.is_some()
                || !platform_members.contains(&(
                    grant.workspace_id.as_str(),
                    grant.recipient_identity_sha256.clone(),
                ))
                || grant.resource_type == "profile"
                    && !hosted_profiles
                        .contains(&(grant.workspace_id.as_str(), grant.resource_id.as_str()))
                || grant.invite_id.is_some()
                || grant.invite_workspace_id.is_some()
                || grant.invite_identity_sha256.is_some()
                || grant.invite_consumed_at.is_some()
                || grant.invite_role.is_some()
                || grant.invite_created_at.is_some()
                || grant.invite_expires_at.is_some()
                || grant.invite_created_by.is_some()
            {
                return Err(invalid("platform recipient share provenance state"));
            }
        } else if grant.disposition == "pending_invite" {
            if is_direct_member {
                return Err(invalid(
                    "pending invite recipient is already a workspace member",
                ));
            }
            required(
                grant.invite_id.as_deref().unwrap_or_default(),
                "pending share invite ID",
            )?;
            required(
                grant.invite_role.as_deref().unwrap_or_default(),
                "pending share invite role",
            )?;
            map_member_role(grant.invite_role.as_deref().unwrap_or_default())?;
            if grant.invite_workspace_id.as_deref() != Some(grant.workspace_id.as_str())
                || grant.invite_identity_sha256.as_deref()
                    != Some(grant.recipient_identity_sha256.as_str())
                || grant.invite_consumed_at.is_some()
            {
                return Err(invalid("pending share invite identity"));
            }
            let invite = pending_invites
                .iter()
                .find(|invite| invite.id == grant.invite_id.as_deref().unwrap_or_default())
                .ok_or_else(|| invalid("pending share invite reference"))?;
            if invite.workspace_id != grant.workspace_id
                || invite.identity_sha256 != grant.recipient_identity_sha256
                || Some(invite.role.as_str()) != grant.invite_role.as_deref()
                || Some(invite.created_at.as_str()) != grant.invite_created_at.as_deref()
                || Some(invite.expires_at.as_str()) != grant.invite_expires_at.as_deref()
                || Some(invite.created_by.as_str()) != grant.invite_created_by.as_deref()
            {
                return Err(invalid("pending share invite evidence mismatch"));
            }
            validate_identity_hash(grant.invite_identity_sha256.as_deref().unwrap_or_default())?;
            required(
                grant.invite_created_by.as_deref().unwrap_or_default(),
                "pending share invite createdBy",
            )?;
            parse_timestamp(
                grant
                    .invite_created_at
                    .as_deref()
                    .ok_or_else(|| invalid("pending share invite createdAt"))?,
                "pending share invite createdAt",
            )?;
            parse_timestamp(
                grant
                    .invite_expires_at
                    .as_deref()
                    .ok_or_else(|| invalid("pending share invite expiresAt"))?,
                "pending share invite expiresAt",
            )?;
            if grant.revoked_at.is_some() {
                return Err(invalid(
                    "revoked share cannot have pending invite disposition",
                ));
            }
        } else if grant.disposition == "revoked_without_member" {
            if grant.revoked_at.is_none()
                || is_direct_member
                || grant.invite_id.is_some()
                || grant.invite_workspace_id.is_some()
                || grant.invite_identity_sha256.is_some()
                || grant.invite_consumed_at.is_some()
                || grant.invite_role.is_some()
                || grant.invite_created_at.is_some()
                || grant.invite_expires_at.is_some()
                || grant.invite_created_by.is_some()
            {
                return Err(invalid("revoked share provenance state"));
            }
        } else if grant.disposition == "revoked_member" {
            if grant.revoked_at.is_none()
                || !is_direct_member
                || grant.invite_id.is_some()
                || grant.invite_workspace_id.is_some()
                || grant.invite_identity_sha256.is_some()
                || grant.invite_consumed_at.is_some()
                || grant.invite_role.is_some()
                || grant.invite_created_at.is_some()
                || grant.invite_expires_at.is_some()
                || grant.invite_created_by.is_some()
            {
                return Err(invalid("revoked member share provenance state"));
            }
        } else {
            return Err(invalid("pending share disposition"));
        }
        if let Some(revoked_at) = grant.revoked_at.as_deref() {
            parse_timestamp(revoked_at, "pending share revokedAt")?;
        }
    }
    pending.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(pending)
}

fn validate_identity_hash(value: &str) -> Result<(), Problem> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(invalid("pending share identity hash"))
    }
}

fn identity_hash(email: &str) -> String {
    let normalized = email.trim().to_ascii_lowercase();
    Sha256::digest(normalized.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn map_share_grants(
    export: &LegacyControlPlaneExport,
    instance_id: &str,
    workspaces: &BTreeMap<String, &LegacyControlWorkspace>,
    groups: &BTreeMap<String, &LegacyControlGroup>,
    memberships: &BTreeMap<(String, String), &LegacyControlMembership>,
    account_by_email: &BTreeMap<String, String>,
    member_by_workspace_email: &BTreeMap<(String, String), String>,
) -> Result<
    (
        Vec<NeutralResourceRecord>,
        Vec<NeutralProfileResourceRecord>,
        Vec<NeutralGrantRecord>,
        Vec<NeutralGroupShareGrantRecord>,
    ),
    Problem,
> {
    let mut seen_share_ids = BTreeSet::new();
    let hosted_profiles = export
        .profiles
        .iter()
        .map(|profile| {
            (
                (profile.workspace_id.as_str(), profile.id.as_str()),
                profile,
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut resources = BTreeMap::<(String, String), NeutralResourceRecord>::new();
    let mut profile_resources = BTreeMap::<String, NeutralProfileResourceRecord>::new();
    let mut grants = Vec::new();
    let mut group_grants = Vec::new();
    for profile in &export.profiles {
        let source_id = format!("profile:{}", profile.id);
        let resource_id = mapped_id(instance_id, "resource", &source_id);
        let profile_id = legacy_local_profile_id(&profile.id)?;
        let resource = NeutralResourceRecord {
            schema: "buglogin.resource/1".into(),
            source_ref: source_ref(instance_id, &source_id),
            id: resource_id.clone(),
            revision: 1,
            created_at: Some(profile.created_at.clone()),
            updated_at: Some(profile.updated_at.clone()),
            data: NeutralResourceData {
                workspace_id: mapped_id(instance_id, "workspace", &profile.workspace_id),
                kind: "profile".into(),
                state: "active".into(),
                display_name: profile.name.clone(),
            },
            extensions: Map::new(),
        };
        let binding = NeutralProfileResourceRecord {
            schema: "buglogin.profile-resource/1".into(),
            source_ref: source_ref(instance_id, &source_id),
            id: mapped_id(instance_id, "profile-resource", &source_id),
            revision: 1,
            created_at: Some(profile.created_at.clone()),
            updated_at: Some(profile.updated_at.clone()),
            data: NeutralProfileResourceData {
                profile_id: mapped_id(instance_id, "profile", &profile_id),
                resource_id: resource_id.clone(),
                workspace_id: mapped_id(instance_id, "workspace", &profile.workspace_id),
                archive_state: "active".into(),
            },
            extensions: Map::new(),
        };
        resources.insert((profile.workspace_id.clone(), profile.id.clone()), resource);
        profile_resources.insert(resource_id, binding);
    }
    for share in &export.share_grants {
        validate_share_grant(share, workspaces, groups)?;
        if !seen_share_ids.insert(share.id.clone()) {
            return Err(invalid("duplicate share grant ID"));
        }
        let email = normalize_email(&share.recipient_email)?;
        let account_id = account_by_email
            .get(&email)
            .ok_or_else(|| invalid("share recipient email does not resolve"))?;
        let member_id = member_by_workspace_email
            .get(&(share.workspace_id.clone(), email))
            .ok_or_else(|| invalid("share recipient is not a direct workspace member"))?;
        if memberships
            .get(&(share.workspace_id.clone(), account_id.clone()))
            .is_some_and(|membership| membership.access_source == "platform")
        {
            return Err(invalid(
                "platform access cannot receive workspace ownership",
            ));
        }

        if share.resource_type == "group" {
            for action in share_actions(&share.access_mode)? {
                group_grants.push(group_share_grant_record(
                    instance_id,
                    share,
                    &mapped_id(instance_id, "group", &share.resource_id),
                    member_id,
                    action,
                ));
            }
            continue;
        }
        if share.revoked_at.is_some() {
            continue;
        }

        if share.resource_type == "profile"
            && !hosted_profiles
                .contains_key(&(share.workspace_id.as_str(), share.resource_id.as_str()))
        {
            return Err(invalid("profile share grant has no hosted profile"));
        }

        let key = (share.workspace_id.clone(), share.resource_id.clone());
        let resource = resource_record(instance_id, share);
        if let Some(existing) = resources.get(&key) {
            if existing.data.workspace_id != resource.data.workspace_id
                || existing.data.kind != resource.data.kind
            {
                return Err(invalid("conflicting shared resource"));
            }
        } else {
            resources.insert(key, resource);
        }
        let binding = profile_resource_record(instance_id, share)?;
        if let Some(existing) = profile_resources.get(&binding.data.resource_id) {
            if existing.data != binding.data {
                return Err(invalid("ambiguous profile resource binding"));
            }
        } else {
            profile_resources.insert(binding.data.resource_id.clone(), binding);
        }
        for action in share_actions(&share.access_mode)? {
            grants.push(grant_record(instance_id, share, member_id, action));
        }
    }
    Ok((
        resources.into_values().collect(),
        profile_resources.into_values().collect(),
        grants,
        group_grants,
    ))
}

fn validate_organization(organization: &LegacyControlOrganization) -> Result<(), Problem> {
    required(&organization.id, "organization ID")?;
    required(&organization.name, "organization name")?;
    required(&organization.slug, "organization slug")?;
    required(&organization.created_by, "organization createdBy")?;
    parse_timestamp(&organization.created_at, "organization createdAt")?;
    parse_timestamp(&organization.updated_at, "organization updatedAt")?;
    if !matches!(organization.status.as_str(), "active" | "archived") {
        return Err(invalid("organization status"));
    }
    Ok(())
}

fn validate_workspace(
    workspace: &LegacyControlWorkspace,
    organizations: &BTreeMap<String, &LegacyControlOrganization>,
) -> Result<(), Problem> {
    required(&workspace.id, "workspace ID")?;
    required(&workspace.organization_id, "workspace organization ID")?;
    if !organizations.contains_key(&workspace.organization_id) {
        return Err(invalid("workspace organization reference"));
    }
    required(&workspace.name, "workspace name")?;
    required(&workspace.created_by, "workspace createdBy")?;
    parse_timestamp(&workspace.created_at, "workspace createdAt")?;
    if !matches!(workspace.mode.as_str(), "personal" | "team") {
        return Err(invalid("workspace mode"));
    }
    validate_entitlement(&workspace.entitlement_state)?;
    if let Some(local) = workspace.local_entitlement_state.as_deref() {
        validate_entitlement(local)?;
        if local != workspace.entitlement_state {
            return Err(invalid("workspace entitlement conflict"));
        }
    }
    Ok(())
}

fn validate_control_group(
    group: &LegacyControlGroup,
    workspaces: &BTreeMap<String, &LegacyControlWorkspace>,
) -> Result<(), Problem> {
    required(&group.id, "group ID")?;
    if !workspaces.contains_key(&group.workspace_id) {
        return Err(invalid("group workspace reference"));
    }
    required(&group.name, "group name")?;
    required(&group.created_by, "group createdBy")?;
    parse_timestamp(&group.created_at, "group createdAt")?;
    parse_timestamp(&group.updated_at, "group updatedAt")?;
    if let Some(last_sync) = group.last_sync.as_deref() {
        parse_timestamp(last_sync, "group lastSync")?;
    }
    Ok(())
}

fn validate_control_profile(
    profile: &LegacyControlProfile,
    workspaces: &BTreeMap<String, &LegacyControlWorkspace>,
) -> Result<(), Problem> {
    if !profile.id.starts_with("prf_")
        || !workspaces.contains_key(&profile.workspace_id)
        || profile.name.trim().is_empty()
        || profile.runtime.trim().is_empty()
    {
        return Err(invalid("hosted profile identity or workspace"));
    }
    let _ = legacy_local_profile_id(&profile.id)?;
    parse_timestamp(&profile.created_at, "profile createdAt")?;
    parse_timestamp(&profile.updated_at, "profile updatedAt")?;
    Ok(())
}

fn hosted_profile_record(
    instance_id: &str,
    profile: &LegacyControlProfile,
) -> Result<super::NeutralProfileRecord, Problem> {
    let local_id = legacy_local_profile_id(&profile.id)?;
    let metadata = serde_json::json!({
        "id": local_id,
        "name": profile.name.clone(),
        "runtime": profile.runtime.clone(),
        "version": profile.runtime_version.clone(),
        "release_type": profile.release_type.clone(),
        "proxy_id": profile.proxy_id.clone(),
        "vpn_id": profile.vpn_id.clone(),
        "group_id": profile.group_id.clone(),
        "extension_group_id": profile.extension_group_id.clone(),
        "ephemeral": profile.ephemeral,
        "created_at": profile.created_at.clone(),
        "updated_at": profile.updated_at.clone(),
        "tags": profile.tags.clone(),
        "note": profile.note.clone()
    });
    let disposition = super::ProfileDataDisposition {
        classification: super::ProfileDataClassification::Excluded,
        reason_code: "profile_payload_not_verified_in_source_snapshot".into(),
    };
    let mut record = super::map_profile(&metadata, &local_id, instance_id, Some(disposition))?;
    record.data.migration_class = "preserve-only".into();
    record.extensions.insert(
        "legacy_workspace_id".into(),
        Value::String(profile.workspace_id.clone()),
    );
    Ok(record)
}

fn control_group_record(instance_id: &str, group: &LegacyControlGroup) -> NeutralGroupRecord {
    let mut extensions = Map::new();
    extensions.insert(
        "legacyCreatedBy".into(),
        Value::String(group.created_by.clone()),
    );
    extensions.insert("legacySyncEnabled".into(), Value::Bool(group.sync_enabled));
    if let Some(last_sync) = group.last_sync.as_ref() {
        extensions.insert("legacyLastSync".into(), Value::String(last_sync.clone()));
    }
    NeutralGroupRecord {
        schema: "buglogin.group/1".into(),
        source_ref: source_ref(instance_id, &group.id),
        id: mapped_id(instance_id, "group", &group.id),
        revision: 1,
        created_at: Some(group.created_at.clone()),
        updated_at: Some(group.updated_at.clone()),
        data: NeutralGroupData {
            name: group.name.trim().to_owned(),
            appearance: group
                .color
                .as_ref()
                .map(|color| serde_json::json!({"color": color})),
            workspace_id: Some(mapped_id(instance_id, "workspace", &group.workspace_id)),
        },
        extensions,
    }
}

fn map_profile_group_assignments(
    export: &LegacyControlPlaneExport,
    instance_id: &str,
    workspaces: &BTreeMap<String, &LegacyControlWorkspace>,
    groups: &BTreeMap<String, &LegacyControlGroup>,
) -> Result<Vec<NeutralProfileGroupAssignment>, Problem> {
    let mut seen_profiles = BTreeSet::new();
    let mut assignments = Vec::new();
    for assignment in &export.profile_group_assignments {
        if !seen_profiles.insert(assignment.profile_id.clone()) {
            return Err(invalid("duplicate profile group assignment"));
        }
        if !workspaces.contains_key(&assignment.workspace_id) {
            return Err(invalid("profile group assignment workspace reference"));
        }
        let group = groups
            .get(&assignment.group_id)
            .ok_or_else(|| invalid("profile group assignment group reference"))?;
        if group.workspace_id != assignment.workspace_id {
            return Err(invalid("profile group assignment tenant mismatch"));
        }
        let profile_id = legacy_local_profile_id(&assignment.profile_id)?;
        assignments.push(NeutralProfileGroupAssignment {
            profile_id: mapped_id(instance_id, "profile", &profile_id),
            workspace_id: mapped_id(instance_id, "workspace", &assignment.workspace_id),
            group_id: mapped_id(instance_id, "group", &assignment.group_id),
        });
    }
    assignments.sort_by(|left, right| left.profile_id.cmp(&right.profile_id));
    Ok(assignments)
}

fn validate_membership(
    membership: &LegacyControlMembership,
    workspaces: &BTreeMap<String, &LegacyControlWorkspace>,
) -> Result<(), Problem> {
    required(&membership.user_id, "membership user ID")?;
    if !workspaces.contains_key(&membership.workspace_id) {
        return Err(invalid("membership workspace reference"));
    }
    normalize_email(&membership.email)?;
    map_member_role(&membership.role)?;
    if !matches!(
        membership.resource_access_mode.as_str(),
        "full" | "restricted"
    ) {
        return Err(invalid("membership resource access mode"));
    }
    match &membership.custom_capabilities {
        LegacyCapabilityOverride::Missing => {
            return Err(invalid("membership customCapabilities missing"));
        }
        LegacyCapabilityOverride::RoleDefault => {}
        LegacyCapabilityOverride::Explicit(actions) => {
            let mut unique = BTreeSet::new();
            for action in actions {
                if !LEGACY_ACTIONS.contains(&action.as_str()) || !unique.insert(action) {
                    return Err(invalid("membership custom capability"));
                }
            }
        }
    }
    if !matches!(
        membership.access_source.as_str(),
        "direct" | "organization" | "platform"
    ) {
        return Err(invalid("membership access source"));
    }
    parse_timestamp(&membership.created_at, "membership createdAt")?;
    Ok(())
}

fn validate_share_grant(
    share: &LegacyControlShareGrant,
    workspaces: &BTreeMap<String, &LegacyControlWorkspace>,
    groups: &BTreeMap<String, &LegacyControlGroup>,
) -> Result<(), Problem> {
    required(&share.id, "share grant ID")?;
    if !workspaces.contains_key(&share.workspace_id) {
        return Err(invalid("share grant workspace reference"));
    }
    required(&share.resource_id, "share grant resource ID")?;
    match share.resource_type.as_str() {
        "profile" => {
            legacy_local_profile_id(&share.resource_id)?;
        }
        "group" => {
            let group = groups
                .get(&share.resource_id)
                .ok_or_else(|| invalid("share grant group reference"))?;
            if group.workspace_id != share.workspace_id {
                return Err(invalid("share grant group tenant mismatch"));
            }
        }
        _ => {
            return Err(Problem::new(
                "migration_control_plane_contract_gap",
                "Legacy control-plane data cannot be represented by the current domain contract",
                422,
                "migration",
            )
            .with_field("resourceType", share.resource_type.clone()));
        }
    }
    normalize_email(&share.recipient_email)?;
    required(&share.created_by, "share grant createdBy")?;
    parse_timestamp(&share.created_at, "share grant createdAt")?;
    if let Some(revoked_at) = share.revoked_at.as_deref() {
        parse_timestamp(revoked_at, "share grant revokedAt")?;
    }
    share_actions(&share.access_mode)?;
    Ok(())
}

fn group_share_grant_record(
    instance_id: &str,
    share: &LegacyControlShareGrant,
    group_id: &str,
    member_id: &str,
    action: &str,
) -> NeutralGroupShareGrantRecord {
    let source_id = format!("{}:{action}", share.id);
    let mut extensions = Map::new();
    extensions.insert("legacyShareGrantId".into(), Value::String(share.id.clone()));
    extensions.insert(
        "legacyRecipientEmail".into(),
        Value::String(share.recipient_email.clone()),
    );
    extensions.insert(
        "legacyAccessMode".into(),
        Value::String(share.access_mode.clone()),
    );
    extensions.insert(
        "legacyCreatedBy".into(),
        Value::String(share.created_by.clone()),
    );
    NeutralGroupShareGrantRecord {
        schema: "buglogin.group-grant/1".into(),
        source_ref: source_ref(instance_id, &source_id),
        id: mapped_id(instance_id, "group-grant", &source_id),
        revision: if share.revoked_at.is_some() { 2 } else { 1 },
        created_at: Some(share.created_at.clone()),
        updated_at: share
            .revoked_at
            .clone()
            .or_else(|| Some(share.created_at.clone())),
        data: NeutralGroupShareGrantData {
            workspace_id: mapped_id(instance_id, "workspace", &share.workspace_id),
            group_id: group_id.to_owned(),
            member_id: member_id.to_owned(),
            action: action.to_owned(),
            effect: "allow".into(),
            revoked_at: share.revoked_at.clone(),
        },
        extensions,
    }
}

fn account_record(
    instance_id: &str,
    source_id: &str,
    account: &AccountAccumulator,
    legacy_credentials_encrypted: bool,
) -> NeutralAccountRecord {
    let mut extensions = Map::new();
    extensions.insert("legacyEmail".into(), Value::String(account.email.clone()));
    extensions.insert(
        "legacyPlatformRole".into(),
        account
            .platform_role
            .clone()
            .map_or(Value::Null, Value::String),
    );
    extensions.insert(
        "legacyAuthProvider".into(),
        account
            .legacy_auth_provider
            .clone()
            .map_or(Value::Null, Value::String),
    );
    extensions.insert(
        "legacyCredentialDisposition".into(),
        Value::String(
            if legacy_credentials_encrypted
                && matches!(
                    account.legacy_auth_provider.as_deref(),
                    Some("password" | "password_google")
                )
            {
                "encrypted_legacy_scrypt_imported".into()
            } else {
                "not_transferred_reprovision_required".into()
            },
        ),
    );
    NeutralAccountRecord {
        schema: "buglogin.account/1".into(),
        source_ref: source_ref(instance_id, source_id),
        id: mapped_id(instance_id, "account", source_id),
        revision: 1,
        created_at: Some(account.created_at.clone()),
        updated_at: Some(account.updated_at.clone()),
        data: NeutralAccountData {
            status: account.status.clone(),
        },
        extensions,
    }
}

fn validate_legacy_auth_provider(provider: Option<&str>) -> Result<(), Problem> {
    match provider.map(str::trim) {
        None | Some("password") | Some("password_google") | Some("google") => Ok(()),
        Some("") => Err(invalid("account auth provider")),
        Some(_) => Err(invalid("unsupported account auth provider")),
    }
}

fn organization_record(
    instance_id: &str,
    organization: &LegacyControlOrganization,
) -> NeutralOrganizationRecord {
    NeutralOrganizationRecord {
        schema: "buglogin.organization/1".into(),
        source_ref: source_ref(instance_id, &organization.id),
        id: mapped_id(instance_id, "organization", &organization.id),
        revision: 1,
        created_at: Some(organization.created_at.clone()),
        updated_at: Some(organization.updated_at.clone()),
        data: NeutralOrganizationData {
            account_id: mapped_id(instance_id, "account", &organization.created_by),
            name: organization.name.clone(),
        },
        extensions: Map::from_iter([
            (
                "legacySlug".into(),
                Value::String(organization.slug.clone()),
            ),
            (
                "legacyStatus".into(),
                Value::String(organization.status.clone()),
            ),
            (
                "legacyCreatedBy".into(),
                Value::String(organization.created_by.clone()),
            ),
        ]),
    }
}

fn workspace_record(
    instance_id: &str,
    workspace: &LegacyControlWorkspace,
) -> NeutralWorkspaceRecord {
    let mut extensions = Map::new();
    extensions.insert("legacyMode".into(), Value::String(workspace.mode.clone()));
    extensions.insert(
        "legacyCreatedBy".into(),
        Value::String(workspace.created_by.clone()),
    );
    extensions.insert(
        "legacyEntitlementState".into(),
        Value::String(workspace.entitlement_state.clone()),
    );
    NeutralWorkspaceRecord {
        schema: "buglogin.workspace/1".into(),
        source_ref: source_ref(instance_id, &workspace.id),
        id: mapped_id(instance_id, "workspace", &workspace.id),
        revision: 1,
        created_at: Some(workspace.created_at.clone()),
        updated_at: Some(workspace.created_at.clone()),
        data: NeutralWorkspaceData {
            organization_id: mapped_id(instance_id, "organization", &workspace.organization_id),
            name: workspace.name.clone(),
            policy: NeutralWorkspacePolicy {
                max_profiles: workspace.profile_limit,
                max_members: workspace.member_limit,
                max_storage_bytes: workspace.max_storage_bytes,
                offline_entitled: false,
            },
        },
        extensions,
    }
}

fn team_record(
    instance_id: &str,
    workspace: &LegacyControlWorkspace,
    team_id: String,
) -> NeutralTeamRecord {
    NeutralTeamRecord {
        schema: "buglogin.team/1".into(),
        source_ref: source_ref(instance_id, &format!("{}:default", workspace.id)),
        id: team_id,
        revision: 1,
        created_at: Some(workspace.created_at.clone()),
        updated_at: Some(workspace.created_at.clone()),
        data: NeutralTeamData {
            workspace_id: mapped_id(instance_id, "workspace", &workspace.id),
            name: format!("{} — Default", workspace.name),
            member_ids: Vec::new(),
        },
        extensions: Map::from_iter([("legacyDefaultTeam".into(), Value::Bool(true))]),
    }
}

fn member_record(
    instance_id: &str,
    membership: &LegacyControlMembership,
    team_id: Option<String>,
) -> Result<NeutralMemberRecord, Problem> {
    let source_id = format!("{}:{}", membership.workspace_id, membership.user_id);
    let mut extensions = Map::new();
    extensions.insert(
        "legacyEmail".into(),
        Value::String(normalize_email(&membership.email)?),
    );
    extensions.insert(
        "legacyResourceAccessMode".into(),
        Value::String(membership.resource_access_mode.clone()),
    );
    extensions.insert(
        "legacyCustomCapabilities".into(),
        match &membership.custom_capabilities {
            LegacyCapabilityOverride::Missing => unreachable!("validated before mapping"),
            LegacyCapabilityOverride::RoleDefault => Value::Null,
            LegacyCapabilityOverride::Explicit(actions) => {
                serde_json::to_value(actions).expect("string vectors always serialize")
            }
        },
    );
    extensions.insert(
        "legacyAccessSource".into(),
        Value::String(membership.access_source.clone()),
    );
    extensions.insert(
        "legacyPlatformRole".into(),
        membership
            .platform_role
            .clone()
            .map_or(Value::Null, Value::String),
    );
    Ok(NeutralMemberRecord {
        schema: "buglogin.member/1".into(),
        source_ref: source_ref(instance_id, &source_id),
        id: mapped_id(instance_id, "member", &source_id),
        revision: 1,
        created_at: Some(membership.created_at.clone()),
        updated_at: Some(membership.created_at.clone()),
        data: NeutralMemberData {
            account_id: mapped_id(instance_id, "account", &membership.user_id),
            workspace_id: mapped_id(instance_id, "workspace", &membership.workspace_id),
            team_id,
            role: map_member_role(&membership.role)?.into(),
            status: "active".into(),
        },
        extensions,
    })
}

fn resource_record(instance_id: &str, share: &LegacyControlShareGrant) -> NeutralResourceRecord {
    let source_id = format!("{}:{}", share.resource_type, share.resource_id);
    NeutralResourceRecord {
        schema: "buglogin.resource/1".into(),
        source_ref: source_ref(instance_id, &source_id),
        id: mapped_id(instance_id, "resource", &source_id),
        revision: 1,
        created_at: Some(share.created_at.clone()),
        updated_at: Some(share.created_at.clone()),
        data: NeutralResourceData {
            workspace_id: mapped_id(instance_id, "workspace", &share.workspace_id),
            kind: "profile".into(),
            state: "active".into(),
            display_name: share
                .resource_name
                .as_deref()
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| profile_display_fallback(&share.resource_id)),
        },
        extensions: Map::from_iter([(
            "legacyResourceType".into(),
            Value::String(share.resource_type.clone()),
        )]),
    }
}

fn profile_resource_record(
    instance_id: &str,
    share: &LegacyControlShareGrant,
) -> Result<NeutralProfileResourceRecord, Problem> {
    let source_id = format!("{}:{}", share.resource_type, share.resource_id);
    let local_profile_id = legacy_local_profile_id(&share.resource_id)?;
    Ok(NeutralProfileResourceRecord {
        schema: "buglogin.profile-resource/1".into(),
        source_ref: source_ref(instance_id, &source_id),
        id: mapped_id(instance_id, "profile-resource", &source_id),
        revision: 1,
        created_at: Some(share.created_at.clone()),
        updated_at: Some(share.created_at.clone()),
        data: NeutralProfileResourceData {
            profile_id: mapped_id(instance_id, "profile", &local_profile_id),
            resource_id: mapped_id(instance_id, "resource", &source_id),
            workspace_id: mapped_id(instance_id, "workspace", &share.workspace_id),
            archive_state: "active".into(),
        },
        extensions: Map::new(),
    })
}

pub fn legacy_local_profile_id(resource_id: &str) -> Result<String, Problem> {
    let compact = resource_id
        .strip_prefix("prf_")
        .filter(|value| {
            value.len() == 32
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
        .ok_or_else(|| invalid("share grant profile resource ID"))?;
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &compact[0..8],
        &compact[8..12],
        &compact[12..16],
        &compact[16..20],
        &compact[20..32]
    ))
}

fn profile_display_fallback(resource_id: &str) -> String {
    format!("profile-{}", &resource_id[resource_id.len() - 7..])
}

fn grant_record(
    instance_id: &str,
    share: &LegacyControlShareGrant,
    member_id: &str,
    action: &str,
) -> NeutralGrantRecord {
    let source_id = format!("{}:{action}", share.id);
    let resource_source_id = format!("{}:{}", share.resource_type, share.resource_id);
    let mut extensions = Map::new();
    extensions.insert("legacyShareGrantId".into(), Value::String(share.id.clone()));
    extensions.insert(
        "legacyRecipientEmail".into(),
        Value::String(share.recipient_email.clone()),
    );
    extensions.insert(
        "legacyAccessMode".into(),
        Value::String(share.access_mode.clone()),
    );
    extensions.insert(
        "legacyCreatedBy".into(),
        Value::String(share.created_by.clone()),
    );
    NeutralGrantRecord {
        schema: "buglogin.grant/1".into(),
        source_ref: source_ref(instance_id, &source_id),
        id: mapped_id(instance_id, "grant", &source_id),
        revision: 1,
        created_at: Some(share.created_at.clone()),
        updated_at: Some(share.created_at.clone()),
        data: NeutralGrantData {
            workspace_id: mapped_id(instance_id, "workspace", &share.workspace_id),
            resource_id: mapped_id(instance_id, "resource", &resource_source_id),
            member_id: member_id.into(),
            action: action.into(),
            effect: "allow".into(),
        },
        extensions,
    }
}

fn deserialize_capabilities<'de, D>(deserializer: D) -> Result<LegacyCapabilityOverride, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(match Option::<Vec<String>>::deserialize(deserializer)? {
        None => LegacyCapabilityOverride::RoleDefault,
        Some(actions) => LegacyCapabilityOverride::Explicit(actions),
    })
}

fn map_member_role(role: &str) -> Result<&'static str, Problem> {
    match role {
        "owner" => Ok("owner"),
        "admin" => Ok("admin"),
        "member" => Ok("operator"),
        "viewer" => Ok("member"),
        _ => Err(invalid("membership role")),
    }
}

fn map_account_state(state: &str) -> Result<String, Problem> {
    match state {
        "active" => Ok("active".into()),
        "locked" => Ok("suspended".into()),
        _ => Err(invalid("account state")),
    }
}

fn validate_platform_role(role: Option<&str>) -> Result<(), Problem> {
    if role.is_none_or(|role| role == "platform_admin") {
        Ok(())
    } else {
        Err(invalid("platform role"))
    }
}

fn validate_entitlement(state: &str) -> Result<(), Problem> {
    if matches!(state, "active" | "grace_active" | "read_only") {
        Ok(())
    } else {
        Err(invalid("entitlement state"))
    }
}

fn share_actions(access_mode: &str) -> Result<&'static [&'static str], Problem> {
    match access_mode {
        "full" => Ok(&["read", "update", "execute", "share"]),
        "run_sync_limited" => Ok(&["read", "execute"]),
        _ => Err(invalid("share grant access mode")),
    }
}

fn normalize_email(email: &str) -> Result<String, Problem> {
    let email = email.trim().to_ascii_lowercase();
    let Some((local, domain)) = email.split_once('@') else {
        return Err(invalid("email"));
    };
    if local.is_empty()
        || domain.is_empty()
        || domain.starts_with('.')
        || domain.ends_with('.')
        || !domain.contains('.')
        || email.chars().any(char::is_whitespace)
    {
        return Err(invalid("email"));
    }
    Ok(email)
}

fn parse_timestamp(value: &str, field: &str) -> Result<OffsetDateTime, Problem> {
    OffsetDateTime::parse(value, &Rfc3339).map_err(|_| invalid(field))
}

fn required<'a>(value: &'a str, field: &str) -> Result<&'a str, Problem> {
    if value.trim().is_empty() {
        Err(invalid(field))
    } else {
        Ok(value)
    }
}

fn source_ref(instance_id: &str, source_id: &str) -> ProfileSourceRef {
    ProfileSourceRef {
        system: "buglogin-legacy".into(),
        instance_id: instance_id.into(),
        id: source_id.into(),
    }
}

fn mapped_id(instance_id: &str, entity: &str, source_id: &str) -> String {
    migration_uuid("buglogin-legacy", instance_id, entity, source_id).to_string()
}

fn invalid(field: &str) -> Problem {
    Problem::new(
        "migration_control_plane_invalid",
        "Legacy control-plane migration data is invalid",
        422,
        "migration",
    )
    .with_field("field", field)
}
