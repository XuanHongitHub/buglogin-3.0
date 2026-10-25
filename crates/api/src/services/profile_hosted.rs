use crate::clock::{rfc3339, unix_time_seconds};
use crate::http::ApiState;
use crate::services::device_key_directory::DeviceEncryptionKey;
use buglogin_domain::{
    entities::{Profile, ProfileState, validate_wayfern_runtime_config},
    errors::Problem,
    grant::{GroupShareGrant, ResourceGrant},
    ids::{GrantId, GroupId, MemberId, ProfileId, ResourceId, WorkspaceId},
    member::{Member, MemberRole},
    permissions::{Action, Capability, Effect, Grant, Principal, ResourceScope, is_allowed},
    profile::{
        ProfileArchiveState, ProfileCloneInput, ProfileCloneReceipt,
        ProfileKeyRotationEnvelopeUpdate, ProfileResource, ProfileShareInput,
        ProfileSharePermission, ProfileShareReceipt, ProfileShareRevocationReceipt,
        ProfileShareRevokeInput,
    },
    resource::{Resource, ResourceKind, ResourceState},
};
use buglogin_storage::local::Versioned;
use buglogin_sync::{manifest::SyncManifest, payload::PayloadDescriptor};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub struct HostedProfileProvisionPlan {
    pub profile: Profile,
    pub workspace_id: WorkspaceId,
    pub resource_id: ResourceId,
    pub manifest: SyncManifest,
    pub resource: Resource,
    pub binding: ProfileResource,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct HostedProfileService;

pub(crate) const MAX_PROFILE_KEY_ROTATION_MEMBERS: usize = 256;
pub(crate) const MAX_PROFILE_KEY_ROTATION_DEVICES: usize = 64;
pub(crate) const MAX_KEY_ROTATION_PROFILE_FANOUT: usize = 256;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileKeyRotationRecipientDevices {
    pub member_id: MemberId,
    pub device_keys: Vec<DeviceEncryptionKey>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileUpdateInput {
    pub workspace_id: WorkspaceId,
    pub profile_id: ProfileId,
    pub expected_revision: u64,
    pub name: String,
    pub note: Option<String>,
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_config: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostedProfileDeleteReceipt {
    pub id: ProfileId,
    pub deleted: bool,
    pub recoverable: bool,
}

impl HostedProfileService {
    pub fn activate_member(
        state: &ApiState,
        principal: &Principal,
        grants: &[Grant],
        workspace_id: &WorkspaceId,
        member_id: &MemberId,
    ) -> Result<Member, Problem> {
        let before = state.control_aggregate()?;
        let result = (|| {
            state
                .members
                .write()
                .map_err(|_| problem("member_unavailable", "Member service unavailable", 503))?
                .activate(
                    principal,
                    grants,
                    workspace_id,
                    member_id,
                    unix_time_seconds("member-activate")?,
                )?;
            Self::mark_member_profiles_for_key_rotation(state, workspace_id, member_id)?;
            Self::validate_member_key_rotation_bounds(state, workspace_id, member_id)?;
            state
                .members
                .read()
                .map_err(|_| problem("member_unavailable", "Member service unavailable", 503))?
                .get_in_workspace(principal, grants, member_id, workspace_id)
        })();
        if result.is_err() {
            state.replace_control_aggregate(before)?;
        }
        result
    }

    pub fn suspend_member(
        state: &ApiState,
        principal: &Principal,
        grants: &[Grant],
        workspace_id: &WorkspaceId,
        member_id: &MemberId,
    ) -> Result<Member, Problem> {
        // Authorize and validate the workspace/member/owner invariants before
        // touching profile key-rotation state. The mutation below repeats this
        // check while holding the control mutation lock.
        state
            .members
            .read()
            .map_err(|_| problem("member_unavailable", "Member service unavailable", 503))?
            .validate_suspend(principal, grants, workspace_id, member_id)?;
        let before = state.control_aggregate()?;
        let result = (|| {
            Self::mark_member_profiles_for_key_rotation(state, workspace_id, member_id)?;
            state
                .members
                .write()
                .map_err(|_| problem("member_unavailable", "Member service unavailable", 503))?
                .suspend(principal, grants, workspace_id, member_id)?;
            state
                .members
                .read()
                .map_err(|_| problem("member_unavailable", "Member service unavailable", 503))?
                .get_in_workspace(principal, grants, member_id, workspace_id)
        })();
        if result.is_err() {
            state.replace_control_aggregate(before)?;
        }
        result
    }

    pub fn create_group_share(
        state: &ApiState,
        principal: &Principal,
        grants: &[Grant],
        input: crate::services::groups::GroupShareCreateInput,
    ) -> Result<GroupShareGrant, Problem> {
        let prepared = state
            .groups
            .read()
            .map_err(|_| problem("group_unavailable", "Group service unavailable", 503))?
            .prepare_share(principal, grants, &input.workspace_id, &input)?;
        if !state
            .members
            .read()
            .map_err(|_| problem("member_unavailable", "Member service unavailable", 503))?
            .active_in_workspace(&input.member_id, &input.workspace_id)
        {
            return Err(problem(
                "group_share_member_inactive",
                "Group share recipient must be an active workspace member",
                422,
            ));
        }
        let before = state.control_aggregate()?;
        let result = (|| {
            let granted = state
                .groups
                .write()
                .map_err(|_| problem("group_unavailable", "Group service unavailable", 503))?
                .grant_share(principal, grants, &input.workspace_id, prepared)?;
            if matches!(granted.action, Action::Read | Action::Execute) {
                Self::mark_group_profiles_for_key_rotation(
                    state,
                    &input.workspace_id,
                    &input.group_id,
                )?;
            }
            Ok(granted)
        })();
        if result.is_err() {
            state.replace_control_aggregate(before)?;
        }
        result
    }

    pub fn revoke_group_share(
        state: &ApiState,
        principal: &Principal,
        grants: &[Grant],
        input: crate::services::groups::GroupShareRevokeInput,
    ) -> Result<GroupShareGrant, Problem> {
        let existing = state
            .groups
            .read()
            .map_err(|_| problem("group_unavailable", "Group service unavailable", 503))?
            .validate_share_revocation(
                principal,
                grants,
                &input.workspace_id,
                &input.grant_id,
                input.expected_revision,
            )?;
        if existing.group_id != input.group_id {
            return Err(problem(
                "group_share_not_found",
                "Group share not found",
                404,
            ));
        }
        let before = state.control_aggregate()?;
        let result = (|| {
            let revoked = state
                .groups
                .write()
                .map_err(|_| problem("group_unavailable", "Group service unavailable", 503))?
                .revoke_share(
                    principal,
                    grants,
                    &input.workspace_id,
                    &input.grant_id,
                    input.expected_revision,
                )?;
            if matches!(existing.action, Action::Read | Action::Execute) {
                Self::mark_group_profiles_for_key_rotation(
                    state,
                    &input.workspace_id,
                    &input.group_id,
                )?;
            }
            Ok(revoked)
        })();
        if result.is_err() {
            state.replace_control_aggregate(before)?;
        }
        result
    }

    pub fn replace_profile_groups(
        state: &ApiState,
        input: crate::services::groups::ProfileGroupsReplaceInput,
    ) -> Result<Versioned<Profile>, Problem> {
        let binding = state
            .profile_resource_record(&input.profile_id)?
            .filter(|binding| binding.workspace_id == input.workspace_id)
            .ok_or_else(|| problem("binding_not_found", "Profile binding not found", 404))?;
        let current = state
            .get_profile_record(&input.profile_id)?
            .ok_or_else(|| problem("profile_not_found", "Profile not found", 404))?;
        let encrypted = state
            .current_sync_manifest(&input.workspace_id, &binding.resource_id)?
            .is_some_and(|manifest| manifest.payload_encryption.is_some());
        let requires_rotation = encrypted && current.value.group_ids != input.group_ids;
        let before = state.control_aggregate()?;
        let result = (|| {
            let updated = state.replace_hosted_profile_groups(input)?;
            if requires_rotation {
                Self::validate_profile_key_rotation_recipient_bounds(
                    state,
                    &updated.value.id,
                    &binding,
                )?;
                state.mark_resource_key_rotation_pending(
                    &binding.workspace_id,
                    &binding.resource_id,
                )?;
            }
            Ok(updated)
        })();
        if result.is_err() {
            state.replace_control_aggregate(before)?;
        }
        result
    }

    pub fn mark_resource_key_rotation_if_encrypted(
        state: &ApiState,
        workspace_id: &WorkspaceId,
        resource_id: &buglogin_domain::ids::ResourceId,
    ) -> Result<(), Problem> {
        let Some(binding) = state.profile_resource_for_resource(workspace_id, resource_id)? else {
            return Ok(());
        };
        if state
            .current_sync_manifest(workspace_id, resource_id)?
            .is_some_and(|manifest| manifest.payload_encryption.is_some())
        {
            state.mark_resource_key_rotation_pending(&binding.workspace_id, resource_id)?;
        }
        Ok(())
    }

    pub fn mark_group_profiles_for_key_rotation(
        state: &ApiState,
        workspace_id: &WorkspaceId,
        group_id: &GroupId,
    ) -> Result<(), Problem> {
        let mut affected = Vec::new();
        for profile in state.list_profile_records()? {
            if !profile.value.group_ids.contains(group_id) {
                continue;
            }
            let Some(binding) = state.profile_resource_record(&profile.value.id)? else {
                continue;
            };
            if binding.workspace_id != *workspace_id
                || !state
                    .current_sync_manifest(&binding.workspace_id, &binding.resource_id)?
                    .is_some_and(|manifest| manifest.payload_encryption.is_some())
            {
                continue;
            }
            Self::validate_profile_key_rotation_recipient_bounds(
                state,
                &profile.value.id,
                &binding,
            )?;
            affected.push(binding.resource_id);
            if affected.len() > MAX_KEY_ROTATION_PROFILE_FANOUT {
                return Err(problem(
                    "profile_key_rotation_fanout_exceeded",
                    "This group change affects too many encrypted profiles to rotate atomically",
                    413,
                ));
            }
        }
        for resource_id in affected {
            state.mark_resource_key_rotation_pending(workspace_id, &resource_id)?;
        }
        Ok(())
    }

    fn validate_profile_key_rotation_recipient_bounds(
        state: &ApiState,
        profile_id: &ProfileId,
        binding: &ProfileResource,
    ) -> Result<(), Problem> {
        Self::resolve_profile_key_rotation_recipient_devices(state, profile_id, binding, false)?;
        Ok(())
    }

    pub fn mark_member_profiles_for_key_rotation(
        state: &ApiState,
        workspace_id: &WorkspaceId,
        member_id: &MemberId,
    ) -> Result<(), Problem> {
        let mut affected = Vec::new();
        for profile in state.list_profile_records()? {
            let Some(binding) = state.profile_resource_record(&profile.value.id)? else {
                continue;
            };
            if binding.workspace_id != *workspace_id
                || !state
                    .current_sync_manifest(&binding.workspace_id, &binding.resource_id)?
                    .is_some_and(|manifest| manifest.payload_encryption.is_some())
            {
                continue;
            }
            let recipients = Self::effective_profile_key_recipients(
                state,
                &profile.value.id,
                &binding,
                &[Action::Read, Action::Execute],
            )?;
            if recipients.contains(member_id) {
                affected.push(binding.resource_id);
                if affected.len() > MAX_KEY_ROTATION_PROFILE_FANOUT {
                    return Err(problem(
                        "profile_key_rotation_fanout_exceeded",
                        "This membership change affects too many encrypted profiles to rotate atomically",
                        413,
                    ));
                }
            }
        }
        for resource_id in affected {
            state.mark_resource_key_rotation_pending(workspace_id, &resource_id)?;
        }
        Ok(())
    }

    fn validate_member_key_rotation_bounds(
        state: &ApiState,
        workspace_id: &WorkspaceId,
        member_id: &MemberId,
    ) -> Result<(), Problem> {
        for profile in state.list_profile_records()? {
            let Some(binding) = state.profile_resource_record(&profile.value.id)? else {
                continue;
            };
            if binding.workspace_id != *workspace_id
                || !state
                    .current_sync_manifest(&binding.workspace_id, &binding.resource_id)?
                    .is_some_and(|manifest| manifest.payload_encryption.is_some())
            {
                continue;
            }
            let recipients = Self::effective_profile_key_recipients(
                state,
                &profile.value.id,
                &binding,
                &[Action::Read, Action::Execute],
            )?;
            if recipients.contains(member_id) {
                Self::validate_profile_key_rotation_recipient_bounds(
                    state,
                    &profile.value.id,
                    &binding,
                )?;
            }
        }
        Ok(())
    }

    pub fn update_metadata(
        state: &ApiState,
        principal: &Principal,
        grants: &[Grant],
        mut input: ProfileUpdateInput,
    ) -> Result<Versioned<Profile>, Problem> {
        input.name = input.name.trim().to_owned();
        if input.expected_revision == 0
            || input.name.is_empty()
            || input.name.chars().count() > 200
            || input
                .note
                .as_ref()
                .is_some_and(|note| note.chars().count() > 4096)
            || input.tags.len() > 64
        {
            return Err(problem(
                "profile_update_invalid",
                "Profile metadata is invalid",
                400,
            ));
        }
        let mut tags = std::collections::BTreeSet::new();
        for tag in input.tags {
            let tag = tag.trim().to_owned();
            if tag.is_empty() || tag.chars().count() > 64 || !tags.insert(tag) {
                return Err(problem(
                    "profile_tags_invalid",
                    "Profile tags must be non-empty, unique, and at most 64 characters",
                    400,
                ));
            }
        }
        input.tags = tags.into_iter().collect();

        let binding = state
            .profile_resource_record(&input.profile_id)?
            .ok_or_else(|| problem("binding_not_found", "Profile binding not found", 404))?;
        if binding.workspace_id != input.workspace_id {
            return Err(problem(
                "binding_not_found",
                "Profile binding not found",
                404,
            ));
        }
        if !Self::authorized(
            state,
            principal,
            grants,
            &input.profile_id,
            Action::Update,
            "profile.update",
        )? {
            return Err(problem("profile_forbidden", "Profile access denied", 403));
        }
        state
            .ensure_no_pending_profile_proxy_assignment(&binding.workspace_id, &input.profile_id)?;
        let current = state
            .get_profile_record(&input.profile_id)?
            .ok_or_else(|| problem("profile_not_found", "Profile not found", 404))?;
        if current.revision != input.expected_revision {
            return Err(
                problem("revision_conflict", "Profile revision conflict", 409)
                    .with_field("expectedRevision", input.expected_revision)
                    .with_field("actualRevision", current.revision),
            );
        }
        if current.value.archived || current.value.state == ProfileState::Running {
            return Err(problem(
                "profile_update_not_allowed",
                "Only active stopped profiles can be edited",
                409,
            ));
        }

        let mut profile = current.value;
        profile.name = input.name;
        profile.note = input.note;
        profile.tags = input.tags;
        if let Some(runtime_config) = input.runtime_config {
            validate_wayfern_runtime_config(&profile.runtime, &runtime_config).map_err(
                |error| {
                    let (code, title, status) = error.problem_details();
                    problem(code, title, status)
                },
            )?;
            profile.runtime_config = Some(runtime_config);
        }
        profile.updated_at = timestamp()?;
        state.update_profile_record(input.expected_revision, profile)
    }

    pub(crate) fn effective_profile_key_recipients(
        state: &ApiState,
        profile_id: &ProfileId,
        binding: &ProfileResource,
        actions: &[Action],
    ) -> Result<std::collections::BTreeSet<MemberId>, Problem> {
        let grants = state.grants()?;
        Self::effective_profile_key_recipients_with_grants(
            state, profile_id, binding, actions, &grants,
        )
    }

    fn effective_profile_key_recipients_with_grants(
        state: &ApiState,
        profile_id: &ProfileId,
        binding: &ProfileResource,
        actions: &[Action],
        capability_grants: &[Grant],
    ) -> Result<std::collections::BTreeSet<MemberId>, Problem> {
        let active_members = state
            .members
            .read()
            .map_err(|_| problem("member_unavailable", "Member service unavailable", 503))?
            .active_members()
            .into_iter()
            .filter(|member| member.workspace_id == binding.workspace_id)
            .collect::<Vec<_>>();
        let group_ids = state
            .get_profile_record(profile_id)?
            .map(|record| record.value.group_ids)
            .unwrap_or_default();
        let resources = state
            .resources
            .read()
            .map_err(|_| problem("resource_unavailable", "Resource service unavailable", 503))?;
        let groups = state
            .groups
            .read()
            .map_err(|_| problem("group_unavailable", "Group service unavailable", 503))?;
        // Use the same grant snapshot as authorization. Normal requests include
        // role-derived membership defaults, but a narrower authorization
        // context must never be silently widened here.
        let capability_scope = ResourceScope {
            workspace_id: Some(binding.workspace_id.clone()),
            resource_id: Some(binding.resource_id.clone()),
            profile_id: Some(profile_id.clone()),
            ..ResourceScope::default()
        };
        Ok(active_members
            .into_iter()
            .filter(|member| {
                actions.iter().any(|action| {
                    let capability = match action {
                        Action::Read => "profile.read",
                        Action::Execute => "profile.launch",
                        _ => return false,
                    };
                    let explicitly_denied = capability_grants.iter().any(|grant| {
                        grant.capability.as_str() == capability
                            && grant.action == *action
                            && grant.effect == Effect::Deny
                            && grant.scope.contains(&capability_scope)
                            && grant
                                .account_id
                                .as_ref()
                                .is_none_or(|id| id == &member.account_id)
                            && grant.member_id.as_ref().is_none_or(|id| id == &member.id)
                    });
                    let denied = resources.is_denied_in_workspace(
                        &binding.resource_id,
                        &binding.workspace_id,
                        &member.id,
                        *action,
                    ) || groups.is_denied_for_profile(
                        &group_ids,
                        &binding.workspace_id,
                        &member.id,
                        *action,
                    );
                    let allowed = resources.is_allowed_in_workspace(
                        &binding.resource_id,
                        &binding.workspace_id,
                        &member.id,
                        *action,
                    ) || groups.is_allowed_for_profile(
                        &group_ids,
                        &binding.workspace_id,
                        &member.id,
                        *action,
                    ) || capability_grants.iter().any(|grant| {
                        grant.capability.as_str() == capability
                            && grant.action == *action
                            && grant.effect == Effect::Allow
                            // Membership defaults grant authorization but do
                            // not create encrypted key-sharing obligations.
                            && !grant.id.as_str().starts_with("membership:")
                            && grant.scope.contains(&capability_scope)
                            && grant
                                .account_id
                                .as_ref()
                                .is_none_or(|id| id == &member.account_id)
                            && grant.member_id.as_ref().is_none_or(|id| id == &member.id)
                    });
                    !explicitly_denied && !denied && allowed
                })
            })
            .map(|member| member.id)
            .collect())
    }

    fn resolve_profile_key_rotation_recipient_devices(
        state: &ApiState,
        profile_id: &ProfileId,
        binding: &ProfileResource,
        require_complete_device_set: bool,
    ) -> Result<Vec<ProfileKeyRotationRecipientDevices>, Problem> {
        let shared_members = Self::effective_profile_key_recipients(
            state,
            profile_id,
            binding,
            &[Action::Read, Action::Execute],
        )?;
        if shared_members.len() > MAX_PROFILE_KEY_ROTATION_MEMBERS {
            return Err(problem(
                "profile_key_rotation_recipient_limit_exceeded",
                "This encrypted profile exceeds the recipient limit",
                413,
            ));
        }

        let active_members = state
            .members
            .read()
            .map_err(|_| problem("member_unavailable", "Member service unavailable", 503))?
            .active_members();
        let now = unix_time_seconds("profile-key-rotation")?;
        let auth = state
            .auth
            .read()
            .map_err(|_| problem("auth_unavailable", "Authentication unavailable", 503))?;
        let mut recipients = Vec::with_capacity(shared_members.len());

        for member_id in shared_members {
            let Some(member) = active_members.iter().find(|member| member.id == member_id) else {
                return Err(profile_key_distribution_not_ready());
            };
            let device_ids = auth.active_device_ids(&member.account_id, now)?;
            if device_ids.len() > MAX_PROFILE_KEY_ROTATION_DEVICES {
                return Err(problem(
                    "profile_key_rotation_device_limit_exceeded",
                    "A profile recipient exceeds the active device limit",
                    413,
                ));
            }
            if !require_complete_device_set {
                recipients.push(ProfileKeyRotationRecipientDevices {
                    member_id,
                    device_keys: Vec::new(),
                });
                continue;
            }
            if device_ids.is_empty() {
                return Err(profile_key_distribution_not_ready());
            }
            let device_keys = state
                .device_key_directory
                .list_for_account(&member.account_id, &device_ids)?;
            if device_keys.len() != device_ids.len()
                || device_keys
                    .iter()
                    .any(|key| !device_ids.contains(&key.device_id))
            {
                return Err(profile_key_distribution_not_ready());
            }
            recipients.push(ProfileKeyRotationRecipientDevices {
                member_id,
                device_keys,
            });
        }
        Ok(recipients)
    }

    pub fn list_profile_key_rotation_recipients(
        state: &ApiState,
        principal: &Principal,
        grants: &[Grant],
        profile_id: &ProfileId,
        current_device_id: &str,
    ) -> Result<Vec<ProfileKeyRotationRecipientDevices>, Problem> {
        require_hosted(state)?;
        if !Self::authorized(
            state,
            principal,
            grants,
            profile_id,
            Action::Update,
            "profile.update",
        )? {
            return Err(problem("profile_forbidden", "Profile access denied", 403));
        }
        let binding = state
            .profile_resource_record(profile_id)?
            .ok_or_else(|| problem("binding_not_found", "Profile binding not found", 404))?;
        if !state.resource_key_rotation_pending(&binding.workspace_id, &binding.resource_id)? {
            return Self::initial_owner_key_envelope_recipient(
                state,
                principal,
                grants,
                profile_id,
                &binding,
                current_device_id,
            );
        }
        Self::resolve_profile_key_rotation_recipient_devices(state, profile_id, &binding, true)
    }

    fn initial_owner_key_envelope_recipient(
        state: &ApiState,
        principal: &Principal,
        grants: &[Grant],
        profile_id: &ProfileId,
        binding: &ProfileResource,
        current_device_id: &str,
    ) -> Result<Vec<ProfileKeyRotationRecipientDevices>, Problem> {
        let genesis = state
            .current_sync_manifest(&binding.workspace_id, &binding.resource_id)?
            .is_some_and(|manifest| {
                manifest.generation == 1
                    && manifest.objects.is_empty()
                    && manifest.payload_encryption.is_none()
            });
        if !genesis
            || !Self::authorized(
                state,
                principal,
                grants,
                profile_id,
                Action::Update,
                "profile.update",
            )?
        {
            return Err(problem(
                "sync_key_rotation_not_pending",
                "Profile key rotation is not pending",
                409,
            ));
        }
        let account_id = principal
            .account_id
            .as_ref()
            .ok_or_else(profile_key_distribution_not_ready)?;
        let members = state
            .members
            .read()
            .map_err(|_| problem("member_unavailable", "Member service unavailable", 503))?
            .active_for_account(account_id);
        let member = members
            .into_iter()
            .find(|member| {
                member.account_id == *account_id
                    && member.workspace_id == binding.workspace_id
                    && member.role == MemberRole::Owner
            })
            .ok_or_else(profile_key_enrollment_required)?;
        let now = unix_time_seconds("initial-profile-key-envelope")?;
        let active_devices = state
            .auth
            .read()
            .map_err(|_| problem("auth_unavailable", "Authentication unavailable", 503))?
            .active_device_ids(&member.account_id, now)?;
        if !active_devices
            .iter()
            .any(|device| device == current_device_id)
        {
            return Err(profile_key_distribution_not_ready());
        }
        let device_keys = state
            .device_key_directory
            .list_for_account(&member.account_id, &active_devices)?
            .into_iter()
            .filter(|key| key.device_id == current_device_id)
            .collect::<Vec<_>>();
        if device_keys.len() != 1 {
            return Err(profile_key_distribution_not_ready());
        }
        Ok(vec![ProfileKeyRotationRecipientDevices {
            member_id: member.id,
            device_keys,
        }])
    }

    pub fn activate_initial_owner_key_envelope(
        state: &ApiState,
        principal: &Principal,
        profile_id: &ProfileId,
        current_device_id: &str,
        key_version: &str,
        updates: &[ProfileKeyRotationEnvelopeUpdate],
    ) -> Result<(), Problem> {
        let binding = state
            .profile_resource_record(profile_id)?
            .ok_or_else(|| problem("binding_not_found", "Profile binding not found", 404))?;
        let Some(genesis) =
            state.current_sync_manifest(&binding.workspace_id, &binding.resource_id)?
        else {
            return Err(profile_key_distribution_not_ready());
        };
        if genesis.generation != 1
            || !genesis.objects.is_empty()
            || genesis.payload_encryption.is_some()
            || state.resource_key_rotation_pending(&binding.workspace_id, &binding.resource_id)?
        {
            return Err(profile_key_distribution_not_ready());
        }
        if updates.is_empty() {
            return Err(profile_key_enrollment_required());
        }
        if updates.len() != 1 {
            return Err(profile_key_distribution_not_ready());
        }
        let account_id = principal
            .account_id
            .as_ref()
            .ok_or_else(profile_key_distribution_not_ready)?;
        let active_members = state
            .members
            .read()
            .map_err(|_| problem("member_unavailable", "Member service unavailable", 503))?
            .active_for_account(account_id);
        let member = active_members
            .into_iter()
            .find(|member| {
                member.account_id == *account_id
                    && member.workspace_id == binding.workspace_id
                    && member.role == MemberRole::Owner
            })
            .ok_or_else(profile_key_enrollment_required)?;
        let update = &updates[0];
        if update.member_id != member.id || update.recipient_key_envelopes.len() != 1 {
            return Err(profile_key_distribution_not_ready());
        }
        let now = unix_time_seconds("initial-profile-key-envelope")?;
        let active_devices = state
            .auth
            .read()
            .map_err(|_| problem("auth_unavailable", "Authentication unavailable", 503))?
            .active_device_ids(&member.account_id, now)?;
        if !active_devices
            .iter()
            .any(|device| device == current_device_id)
        {
            return Err(profile_key_distribution_not_ready());
        }
        let keys = state
            .device_key_directory
            .list_for_account(&member.account_id, &active_devices)?;
        let key = keys
            .iter()
            .find(|key| key.device_id == current_device_id)
            .ok_or_else(profile_key_distribution_not_ready)?;
        let confirmed = &update.recipient_key_envelopes[0];
        let envelope = &confirmed.envelope;
        if confirmed.confirmed_fingerprint != key.fingerprint
            || envelope.schema_version != 2
            || envelope.workspace_id != binding.workspace_id
            || envelope.resource_id != binding.resource_id
            || envelope.recipient_device_id != current_device_id
            || envelope.key_version != key_version
            || envelope.encapped_key.len() != 32
            || envelope.ciphertext.len() != 80
            || state
                .profile_key_share_envelope(&binding.resource_id, &member.id, current_device_id)?
                .is_some()
        {
            return Err(profile_key_distribution_not_ready());
        }
        state.replace_profile_key_share_envelope_set(
            profile_id,
            &binding.workspace_id,
            &binding.resource_id,
            vec![(member.id, update.recipient_key_envelopes.clone())],
        )
    }

    pub fn authorized(
        state: &ApiState,
        principal: &Principal,
        grants: &[Grant],
        profile_id: &ProfileId,
        action: Action,
        capability_name: &str,
    ) -> Result<bool, Problem> {
        if let Some(quarantine) = state.profile_quarantine_record(profile_id)? {
            return Ok(quarantine.deleted_by() == principal.account_id.as_ref()
                && matches!(
                    capability_name,
                    "profile.read" | "profile.restore" | "profile.delete"
                )
                && matches!(action, Action::Read | Action::Update | Action::Delete));
        }
        let binding = state.profile_resource_record(profile_id)?;
        let scope = match &binding {
            Some(binding) => ResourceScope {
                workspace_id: Some(binding.workspace_id.clone()),
                resource_id: Some(binding.resource_id.clone()),
                profile_id: Some(profile_id.clone()),
                ..ResourceScope::default()
            },
            None => ResourceScope {
                profile_id: Some(profile_id.clone()),
                ..ResourceScope::default()
            },
        };
        let update_capability = Capability::new("profile.update")
            .map_err(|_| problem("invalid_capability", "Invalid capability", 500))?;
        let can_update = is_allowed(
            principal,
            &update_capability,
            Action::Update,
            &scope,
            grants,
        );
        if let Some(binding) = binding.as_ref()
            && state.resource_key_rotation_pending(&binding.workspace_id, &binding.resource_id)?
        {
            if !can_update || !matches!(action, Action::Read | Action::Update) {
                return Ok(false);
            }
            // Metadata updates may proceed during key rotation, but reads and
            // launches still require an unwrap-capable key envelope below.
            if action == Action::Update {
                return Ok(true);
            }
        }
        let capability = Capability::new(capability_name)
            .map_err(|_| problem("invalid_capability", "Invalid capability", 500))?;
        let capability_allowed = is_allowed(principal, &capability, action, &scope, grants);
        if capability_allowed && !matches!(action, Action::Read | Action::Execute) {
            return Ok(true);
        }
        let Some(binding) = binding else {
            return Ok(capability_allowed);
        };
        if !matches!(
            action,
            Action::Read | Action::Update | Action::Execute | Action::Share
        ) {
            return Ok(false);
        }
        let current_manifest =
            state.current_sync_manifest(&binding.workspace_id, &binding.resource_id)?;
        if capability_allowed
            && !current_manifest
                .as_ref()
                .is_some_and(|manifest| manifest.payload_encryption.is_some())
        {
            return Ok(true);
        }

        // Resource shares are an additional grant path. They must not bypass
        // token restrictions or an explicit capability deny.
        let action_name = match action {
            Action::Read => "read",
            Action::Create => "create",
            Action::Update => "update",
            Action::Delete => "delete",
            Action::Execute => "execute",
            Action::Share => "share",
            Action::Manage => "manage",
        };
        if principal.token_scopes.as_ref().is_some_and(|scopes| {
            !scopes.contains(&format!("{}:{action_name}", capability.as_str()))
                && !scopes.contains(&format!("{}:*", capability.as_str()))
        }) || grants.iter().any(|grant| {
            grant.capability == capability
                && grant.action == action
                && grant.scope.contains(&scope)
                && grant.effect == Effect::Deny
                && grant
                    .account_id
                    .as_ref()
                    .is_none_or(|id| principal.account_id.as_ref() == Some(id))
                && grant
                    .member_id
                    .as_ref()
                    .is_none_or(|id| principal.member_id.as_ref() == Some(id))
        }) {
            return Ok(false);
        }

        let Some(account_id) = principal.account_id.as_ref() else {
            return Ok(false);
        };
        let active_members = state
            .members
            .read()
            .map_err(|_| problem("member_unavailable", "Member service unavailable", 503))?
            .active_for_account(account_id)
            .into_iter()
            .filter(|member| member.workspace_id == binding.workspace_id)
            .collect::<Vec<_>>();
        if active_members.is_empty() {
            return Ok(false);
        }
        let share_members = Self::effective_profile_key_recipients_with_grants(
            state,
            profile_id,
            &binding,
            &[action],
            grants,
        )?;
        if let Some(manifest) = current_manifest
            && let Some(encryption) = manifest.payload_encryption.as_ref()
        {
            let Some(key_version) = encryption.key_version.as_deref() else {
                // Legacy v1 manifests carry the initial resource key envelope
                // in the manifest itself and predate per-device share rows.
                // Preserve that initial workspace-owner access only; other
                // readers still need explicit grants and a recipient envelope.
                return Ok(encryption.schema_version == 1
                    && active_members
                        .iter()
                        .any(|member| member.role == MemberRole::Owner && capability_allowed));
            };
            for member in &active_members {
                let member_id = &member.id;
                if !capability_allowed && !share_members.contains(member_id) {
                    continue;
                }
                if state.has_profile_key_share_envelope_for_member(
                    profile_id,
                    &binding.workspace_id,
                    &binding.resource_id,
                    member_id,
                    key_version,
                )? {
                    return Ok(true);
                }
            }
            return Ok(false);
        }
        Ok(capability_allowed
            || active_members
                .iter()
                .any(|member| share_members.contains(&member.id)))
    }

    pub fn authorize_share_mutation(
        state: &ApiState,
        principal: &Principal,
        grants: &[Grant],
        profile_id: &ProfileId,
    ) -> Result<(), Problem> {
        if state.profile_quarantine_record(profile_id)?.is_some() {
            return Err(problem("profile_forbidden", "Profile access denied", 403));
        }
        let binding = state.profile_resource_record(profile_id)?;
        let scope = match binding {
            Some(binding) => ResourceScope {
                workspace_id: Some(binding.workspace_id),
                resource_id: Some(binding.resource_id),
                profile_id: Some(profile_id.clone()),
                ..ResourceScope::default()
            },
            None => ResourceScope {
                profile_id: Some(profile_id.clone()),
                ..ResourceScope::default()
            },
        };
        let capability = Capability::new("CAP-SHARE-001")
            .map_err(|_| problem("invalid_capability", "Invalid capability", 500))?;
        if is_allowed(principal, &capability, Action::Manage, &scope, grants) {
            Ok(())
        } else {
            Err(problem("profile_forbidden", "Profile access denied", 403))
        }
    }

    pub fn ensure_encrypted_manifest_share_compatible(
        state: &ApiState,
        profile_id: &ProfileId,
        key_version: Option<&str>,
    ) -> Result<(), Problem> {
        let Some(binding) = state.profile_resource_record(profile_id)? else {
            return Err(problem(
                "binding_not_found",
                "Profile binding not found",
                404,
            ));
        };
        let shared_members = Self::effective_profile_key_recipients(
            state,
            profile_id,
            &binding,
            &[Action::Read, Action::Execute],
        )?;
        let rotation_pending =
            state.resource_key_rotation_pending(&binding.workspace_id, &binding.resource_id)?;
        if !shared_members.is_empty() && !rotation_pending {
            let Some(key_version) = key_version else {
                return Err(profile_key_distribution_not_ready());
            };
            let recipients = Self::resolve_profile_key_rotation_recipient_devices(
                state, profile_id, &binding, true,
            )?;
            for recipient in recipients {
                for device in recipient.device_keys {
                    let envelope = state
                        .profile_key_share_envelope(
                            &binding.resource_id,
                            &recipient.member_id,
                            &device.device_id,
                        )?
                        .ok_or_else(profile_key_distribution_not_ready)?;
                    if envelope.schema_version != 2
                        || envelope.workspace_id != binding.workspace_id
                        || envelope.resource_id != binding.resource_id
                        || envelope.recipient_device_id != device.device_id
                        || envelope.key_version != key_version
                    {
                        return Err(profile_key_distribution_not_ready());
                    }
                }
            }
        }
        Ok(())
    }

    pub fn activate_profile_key_rotation_envelopes(
        state: &ApiState,
        profile_id: &ProfileId,
        key_version: &str,
        updates: &[ProfileKeyRotationEnvelopeUpdate],
    ) -> Result<(), Problem> {
        use std::collections::{BTreeMap, BTreeSet};

        let binding = state
            .profile_resource_record(profile_id)?
            .ok_or_else(|| problem("binding_not_found", "Profile binding not found", 404))?;
        if !state.resource_key_rotation_pending(&binding.workspace_id, &binding.resource_id)? {
            return Err(problem(
                "sync_key_rotation_not_pending",
                "Profile key rotation is not pending",
                409,
            ));
        }
        if updates.len() > MAX_PROFILE_KEY_ROTATION_MEMBERS {
            return Err(profile_key_distribution_not_ready());
        }
        let expected = Self::resolve_profile_key_rotation_recipient_devices(
            state, profile_id, &binding, true,
        )?
        .into_iter()
        .map(|recipient| (recipient.member_id, recipient.device_keys))
        .collect::<BTreeMap<_, _>>();
        if updates.len() != expected.len() {
            return Err(profile_key_distribution_not_ready());
        }
        let mut supplied = BTreeMap::new();
        for update in updates {
            if supplied
                .insert(update.member_id.clone(), &update.recipient_key_envelopes)
                .is_some()
            {
                return Err(problem(
                    "recipient_device_envelope_duplicate",
                    "Only one key envelope update per shared member is allowed",
                    400,
                ));
            }
        }
        if supplied.keys().ne(expected.keys()) {
            return Err(profile_key_distribution_not_ready());
        }

        let mut validated = Vec::with_capacity(expected.len());
        for (member_id, keys) in expected {
            let envelopes = supplied
                .get(&member_id)
                .copied()
                .ok_or_else(profile_key_distribution_not_ready)?;
            if envelopes.len() != keys.len() {
                return Err(profile_key_distribution_not_ready());
            }
            let keys_by_device: BTreeMap<_, _> = keys
                .into_iter()
                .map(|key| (key.device_id.clone(), key))
                .collect();
            let mut supplied_devices = BTreeSet::new();
            for confirmed in envelopes {
                let envelope = &confirmed.envelope;
                let key = keys_by_device
                    .get(&envelope.recipient_device_id)
                    .ok_or_else(profile_key_distribution_not_ready)?;
                if !supplied_devices.insert(envelope.recipient_device_id.clone())
                    || confirmed.confirmed_fingerprint != key.fingerprint
                    || envelope.schema_version != 2
                    || envelope.workspace_id != binding.workspace_id
                    || envelope.resource_id != binding.resource_id
                    || envelope.key_version != key_version
                    || envelope.encapped_key.len() != 32
                    || envelope.ciphertext.len() != 80
                {
                    return Err(profile_key_distribution_not_ready());
                }
            }
            validated.push((member_id, envelopes.to_vec()));
        }

        state.replace_profile_key_share_envelope_set(
            profile_id,
            &binding.workspace_id,
            &binding.resource_id,
            validated,
        )
    }

    pub fn managed_resource_id(profile_id: &ProfileId) -> Result<ResourceId, Problem> {
        let digest = format!("{:x}", Sha256::digest(profile_id.as_str().as_bytes()));
        ResourceId::new(format!("res_{}", &digest[..24])).map_err(|_| {
            problem(
                "resource_id_invalid",
                "Managed profile resource ID is invalid",
                500,
            )
        })
    }

    pub fn authorize_resource_creation(
        principal: &Principal,
        grants: &[Grant],
        workspace_id: &WorkspaceId,
        resource_id: &ResourceId,
    ) -> Result<(), Problem> {
        let capability = Capability::new("CAP-RES-001")
            .map_err(|_| problem("invalid_capability", "Invalid capability", 500))?;
        let scope = ResourceScope::resource(workspace_id.clone(), resource_id.clone());
        let resource_create_allowed =
            is_allowed(principal, &capability, Action::Create, &scope, grants);
        // Managed profile creation provisions its resource as an internal part of
        // profile.create. Keep API keys scoped to that operation instead of also
        // granting the broader resource.create capability needed by resource.register.
        let profile_create_key_allowed = principal.token_scopes.as_ref().is_some_and(|scopes| {
            scopes.contains("profile.create:create") || scopes.contains("profile.create:*")
        }) && {
            let mut account_principal = principal.clone();
            account_principal.token_scopes = None;
            is_allowed(
                &account_principal,
                &capability,
                Action::Create,
                &scope,
                grants,
            )
        };
        if resource_create_allowed || profile_create_key_allowed {
            Ok(())
        } else {
            Err(problem(
                "coordination_forbidden",
                "Coordination access denied",
                403,
            ))
        }
    }

    pub fn prepare_provision(
        profile: Profile,
        workspace_id: WorkspaceId,
    ) -> Result<HostedProfileProvisionPlan, Problem> {
        if !profile.group_ids.is_empty() {
            return Err(problem(
                "profile_groups_mutation_required",
                "Profile groups must be assigned through the authorized group operation",
                400,
            ));
        }
        if profile.state != ProfileState::Created || profile.archived {
            return Err(problem(
                "managed_profile_invalid",
                "Managed profile must be created in active created state",
                400,
            ));
        }
        let resource_id = Self::managed_resource_id(&profile.id)?;
        let mut profile = profile;
        profile.state = ProfileState::Ready;
        let binding = ProfileResource {
            profile_id: profile.id.clone(),
            resource_id: resource_id.clone(),
            workspace_id: workspace_id.clone(),
            archive_state: ProfileArchiveState::Active,
        };
        let empty_payload =
            PayloadDescriptor::new(workspace_id.clone(), resource_id.clone(), 1, Vec::new())?;
        let manifest = SyncManifest::new(
            workspace_id.clone(),
            resource_id.clone(),
            1,
            1,
            None,
            &empty_payload.canonical_root_bytes(),
            Vec::new(),
        )?;
        let resource = Resource {
            id: resource_id.clone(),
            workspace_id: workspace_id.clone(),
            kind: ResourceKind::Profile,
            state: ResourceState::Active,
            display_name: profile.name.clone(),
        };
        Ok(HostedProfileProvisionPlan {
            profile,
            workspace_id,
            resource_id,
            manifest,
            resource,
            binding,
        })
    }

    pub fn clone_metadata(
        state: &ApiState,
        principal: &Principal,
        grants: &[Grant],
        input: ProfileCloneInput,
    ) -> Result<ProfileCloneReceipt, Problem> {
        require_hosted(state)?;
        if !valid_profile_clone_name(&input.name) {
            return Err(problem(
                "profile_name_invalid",
                "Profile name is invalid",
                400,
            ));
        }
        if input.source_profile_id == input.target_profile_id {
            return Err(problem(
                "profile_clone_same_source",
                "Source and target profiles must differ",
                409,
            ));
        }
        let before = state.control_aggregate()?;
        let result = (|| {
            let source = state
                .get_profile_record(&input.source_profile_id)?
                .ok_or_else(|| problem("profile_not_found", "Source profile not found", 404))?;
            if source.revision != input.expected_revision {
                return Err(
                    problem("revision_conflict", "Profile revision conflict", 409)
                        .with_field("expectedRevision", input.expected_revision)
                        .with_field("actualRevision", source.revision),
                );
            }
            if source.value.archived || source.value.state == ProfileState::Running {
                return Err(problem(
                    "profile_not_cloneable",
                    "Profile cannot be cloned in its current state",
                    409,
                ));
            }
            if state
                .get_profile_record(&input.target_profile_id)?
                .is_some()
            {
                return Err(problem(
                    "profile_exists",
                    "Target profile already exists",
                    409,
                ));
            }
            let binding = state
                .profile_resource_record(&input.source_profile_id)?
                .ok_or_else(|| {
                    problem("binding_not_found", "Source profile binding not found", 404)
                })?;
            authorize_profile_read(principal, grants, &binding)?;
            authorize_profile_create(principal, grants, &binding)?;
            state.ensure_workspace_profile_capacity(&binding.workspace_id)?;
            let mut target = source.value.clone();
            target.id = input.target_profile_id;
            target.name = input.name.trim().to_owned();
            target.runtime_version = None;
            target.runtime_config = None;
            target.proxy_id = None;
            target.vpn_id = None;
            // Group assignment is an ACL mutation, not cloneable metadata.
            // Cloning must not grant the source profile's group recipients
            // access without the separate profile.groups.replace authority.
            target.group_ids.clear();
            target.extension_group_id = None;
            target.state = ProfileState::Created;
            target.archived = false;
            target.sync_mode = Some(buglogin_domain::entities::SyncMode::Managed);
            target.created_at = timestamp()?;
            target.updated_at = target.created_at.clone();
            let target_resource_id = Self::managed_resource_id(&target.id)?;
            Self::authorize_resource_creation(
                principal,
                grants,
                &binding.workspace_id,
                &target_resource_id,
            )?;
            let plan = Self::prepare_provision(target, binding.workspace_id.clone())?;
            state
                .resources
                .write()
                .map_err(|_| problem("resource_unavailable", "Resource service unavailable", 503))?
                .register(principal, grants, plan.resource.clone())?;
            state.replace_profile_resource(plan.binding)?;
            state.replace_sync_manifest(plan.manifest)?;
            let created = state.create_profile_record(plan.profile.clone())?;
            Ok(ProfileCloneReceipt {
                source_profile_id: input.source_profile_id,
                profile: buglogin_domain::entities::ProfileRecord::new(
                    created.value,
                    created.revision,
                ),
                resource_id: plan.resource_id,
                metadata_only: true,
                copied_fields: vec![
                    "runtime".into(),
                    "fingerprint".into(),
                    "note".into(),
                    "tags".into(),
                ],
                excluded_fields: vec![
                    "runtimeVersion".into(),
                    "runtimeConfig".into(),
                    "proxyId".into(),
                    "vpnId".into(),
                    "groupIds".into(),
                    "extensionGroupId".into(),
                    "cookies".into(),
                    "secrets".into(),
                    "runtimeData".into(),
                    "syncObjects".into(),
                ],
            })
        })();
        if result.is_err() {
            state.replace_control_aggregate(before)?;
        }
        result
    }

    pub fn share(
        state: &ApiState,
        principal: &Principal,
        grants: &[Grant],
        input: ProfileShareInput,
    ) -> Result<ProfileShareReceipt, Problem> {
        require_hosted(state)?;
        Self::authorize_share_mutation(state, principal, grants, &input.profile_id)?;
        let before = state.control_aggregate()?;
        let result = (|| {
            let current = state
                .get_profile_record(&input.profile_id)?
                .ok_or_else(|| problem("profile_not_found", "Profile not found", 404))?;
            let binding = state
                .profile_resource_record(&input.profile_id)?
                .ok_or_else(|| problem("binding_not_found", "Profile binding not found", 404))?;
            state.ensure_no_pending_profile_proxy_assignment(
                &binding.workspace_id,
                &input.profile_id,
            )?;
            check_revision(current.revision, input.expected_revision)?;
            if state.resource_key_rotation_pending(&binding.workspace_id, &binding.resource_id)? {
                return Err(problem(
                    "sync_key_rotation_pending",
                    "Profile sharing is unavailable until key rotation completes",
                    409,
                ));
            }
            ensure_member_in_workspace(state, &input.member_id, &binding.workspace_id)?;
            let manifest =
                state.current_sync_manifest(&binding.workspace_id, &binding.resource_id)?;
            if manifest
                .as_ref()
                .is_some_and(|manifest| manifest.payload_encryption.is_some())
            {
                validate_encrypted_profile_share(state, &binding, &input)?;
            } else if !input.recipient_key_envelopes.is_empty() {
                return Err(problem(
                    "profile_key_envelope_unexpected",
                    "Unencrypted profile shares must not include resource key envelopes",
                    400,
                ));
            }
            let actions = share_actions(&input.permission);
            let mut resources = state.resources.write().map_err(|_| {
                problem("resource_unavailable", "Resource service unavailable", 503)
            })?;
            for action in actions {
                resources.grant(
                    principal,
                    grants,
                    share_grant(&binding, &input.member_id, action)?,
                )?;
            }
            state.replace_profile_key_share_envelopes(
                &input.profile_id,
                &binding.workspace_id,
                &binding.resource_id,
                &input.member_id,
                input.recipient_key_envelopes.clone(),
            )?;
            let updated = state.update_profile_record(input.expected_revision, current.value)?;
            Ok(ProfileShareReceipt {
                profile_id: input.profile_id,
                member_id: input.member_id,
                permission: Some(input.permission),
                revision: updated.revision,
                shared: true,
            })
        })();
        if result.is_err() {
            state.replace_control_aggregate(before)?;
        }
        result
    }

    pub fn revoke_share(
        state: &ApiState,
        principal: &Principal,
        grants: &[Grant],
        input: ProfileShareRevokeInput,
    ) -> Result<ProfileShareRevocationReceipt, Problem> {
        require_hosted(state)?;
        Self::authorize_share_mutation(state, principal, grants, &input.profile_id)?;
        let before = state.control_aggregate()?;
        let result = (|| {
            let current = state
                .get_profile_record(&input.profile_id)?
                .ok_or_else(|| problem("profile_not_found", "Profile not found", 404))?;
            let binding = state
                .profile_resource_record(&input.profile_id)?
                .ok_or_else(|| problem("binding_not_found", "Profile binding not found", 404))?;
            state.ensure_no_pending_profile_proxy_assignment(
                &binding.workspace_id,
                &input.profile_id,
            )?;
            check_revision(current.revision, input.expected_revision)?;
            let actions = share_actions(&input.permission);
            let mut resources = state.resources.write().map_err(|_| {
                problem("resource_unavailable", "Resource service unavailable", 503)
            })?;
            for action in actions {
                let grant = share_grant(&binding, &input.member_id, action)?;
                resources.revoke(principal, grants, &grant.id)?;
            }
            state.remove_profile_key_share_envelopes(&input.profile_id, &input.member_id)?;
            let updated = state.update_profile_record(input.expected_revision, current.value)?;
            if state
                .current_sync_manifest(&binding.workspace_id, &binding.resource_id)?
                .is_some_and(|manifest| manifest.payload_encryption.is_some())
            {
                state.mark_resource_key_rotation_pending(
                    &binding.workspace_id,
                    &binding.resource_id,
                )?;
            }
            Ok(ProfileShareRevocationReceipt {
                profile_id: input.profile_id,
                member_id: input.member_id,
                revision: updated.revision,
                revoked: true,
            })
        })();
        if result.is_err() {
            state.replace_control_aggregate(before)?;
        }
        result
    }

    pub fn share_grant_id(
        binding: &ProfileResource,
        member_id: &MemberId,
        action: Action,
    ) -> Result<GrantId, Problem> {
        share_grant(binding, member_id, action).map(|grant| grant.id)
    }

    pub fn provision(
        state: &ApiState,
        principal: &Principal,
        grants: &[Grant],
        plan: &HostedProfileProvisionPlan,
    ) -> Result<Versioned<Profile>, Problem> {
        require_hosted(state)?;
        if !plan.profile.group_ids.is_empty() {
            return Err(problem(
                "profile_groups_mutation_required",
                "Profile groups must be assigned through the authorized group operation",
                400,
            ));
        }
        if let Some(proxy_id) = plan.profile.proxy_id.as_ref() {
            state
                .proxies
                .read()
                .map_err(|_| problem("proxy_unavailable", "Proxy state unavailable", 503))?
                .get(&plan.workspace_id, proxy_id, principal, grants)
                .map_err(crate::services::profile_sync::proxy_repository_problem)?;
        }
        state.ensure_workspace_profile_capacity(&plan.workspace_id)?;
        Self::authorize_resource_creation(
            principal,
            grants,
            &plan.workspace_id,
            &plan.resource_id,
        )?;
        let mut resource_principal = principal.clone();
        if principal.token_scopes.as_ref().is_some_and(|scopes| {
            scopes.contains("profile.create:create") || scopes.contains("profile.create:*")
        }) {
            // authorize_resource_creation above has checked the account grant and
            // the narrower profile.create token scope. ResourceService.register
            // performs the same grant check, but cannot see that this is the
            // profile provisioning sub-operation, so remove only the token scope
            // filter for this already-authorized internal call.
            resource_principal.token_scopes = None;
        }
        state
            .resources
            .write()
            .map_err(|_| problem("resource_unavailable", "Resource service unavailable", 503))?
            .register(&resource_principal, grants, plan.resource.clone())?;
        state.replace_profile_resource(plan.binding.clone())?;
        state.replace_sync_manifest(plan.manifest.clone())?;
        state.create_profile_record(plan.profile.clone())
    }

    pub fn set_archive_state(
        state: &ApiState,
        profile_id: &ProfileId,
        expected_revision: u64,
        archive: bool,
    ) -> Result<Versioned<Profile>, Problem> {
        require_hosted(state)?;
        let now_ms = crate::clock::unix_time_millis("profile-archive")?;
        if !archive
            && state
                .profile_quarantine_record(profile_id)?
                .is_some_and(|record| record.retention_deadline_unix_ms() <= now_ms)
        {
            return Err(problem(
                "profile_quarantine_expired",
                "Profile quarantine retention has expired",
                410,
            ));
        }
        let current = state
            .get_profile_record(profile_id)?
            .ok_or_else(|| problem("profile_not_found", "Profile not found", 404))?;
        if archive
            && matches!(
                current.value.state,
                ProfileState::Running | ProfileState::Launching | ProfileState::Stopping
            )
        {
            return Err(problem(
                "profile_running",
                "Running profile must be stopped before archive",
                409,
            ));
        }
        let mut binding = state
            .profile_resource_record(profile_id)?
            .ok_or_else(|| problem("binding_not_found", "Profile binding not found", 404))?;
        state.ensure_no_pending_profile_proxy_assignment(&binding.workspace_id, profile_id)?;
        if binding.profile_id != *profile_id {
            return Err(problem(
                "profile_binding_invalid",
                "Profile binding is invalid",
                409,
            ));
        }
        let mut profile = current.value;
        let restore_archive_state = if archive {
            true
        } else {
            state
                .profile_quarantine_record(profile_id)?
                .is_some_and(|record| record.was_archived())
        };
        profile.archived = restore_archive_state;
        let updated = state.update_profile_record(expected_revision, profile)?;
        binding.archive_state = if restore_archive_state {
            ProfileArchiveState::Archived
        } else {
            ProfileArchiveState::Active
        };
        state.replace_profile_resource(binding)?;
        if !archive {
            state.restore_quarantined_profile(profile_id)?;
        }
        Ok(updated)
    }

    pub fn delete_to_quarantine(
        state: &ApiState,
        profile_id: &ProfileId,
        expected_revision: u64,
        confirmation: &str,
        deleted_by: Option<buglogin_domain::ids::AccountId>,
    ) -> Result<HostedProfileDeleteReceipt, Problem> {
        require_hosted(state)?;
        if confirmation != "QUARANTINE_PROFILE_DATA" {
            return Err(problem(
                "delete_not_confirmed",
                "Profile deletion is not confirmed",
                409,
            ));
        }
        if state.profile_quarantine_record(profile_id)?.is_some() {
            return Err(problem(
                "profile_already_quarantined",
                "Profile is already in quarantine",
                409,
            ));
        }
        let was_archived = state
            .get_profile_record(profile_id)?
            .ok_or_else(|| problem("profile_not_found", "Profile not found", 404))?
            .value
            .archived;
        Self::set_archive_state(state, profile_id, expected_revision, true)?;
        state.quarantine_profile(profile_id, deleted_by, was_archived)?;
        Ok(HostedProfileDeleteReceipt {
            id: profile_id.clone(),
            deleted: true,
            recoverable: true,
        })
    }
}

fn ensure_member_in_workspace(
    state: &ApiState,
    member_id: &MemberId,
    workspace_id: &WorkspaceId,
) -> Result<(), Problem> {
    let exists = state
        .members
        .read()
        .map_err(|_| problem("member_unavailable", "Member service unavailable", 503))?
        .active_members()
        .into_iter()
        .any(|member| member.id == *member_id && member.workspace_id == *workspace_id);
    if exists {
        Ok(())
    } else {
        Err(problem(
            "member_not_found",
            "Active workspace member not found",
            404,
        ))
    }
}

fn authorize_profile_read(
    principal: &Principal,
    grants: &[Grant],
    binding: &ProfileResource,
) -> Result<(), Problem> {
    let capability = Capability::new("profile.read")
        .map_err(|_| problem("invalid_capability", "Invalid capability", 500))?;
    let scope = ResourceScope {
        workspace_id: Some(binding.workspace_id.clone()),
        resource_id: Some(binding.resource_id.clone()),
        profile_id: Some(binding.profile_id.clone()),
        ..ResourceScope::default()
    };
    if is_allowed(principal, &capability, Action::Read, &scope, grants) {
        Ok(())
    } else {
        Err(problem("profile_forbidden", "Profile access denied", 403))
    }
}

fn authorize_profile_create(
    principal: &Principal,
    grants: &[Grant],
    binding: &ProfileResource,
) -> Result<(), Problem> {
    let capability = Capability::new("profile.create")
        .map_err(|_| problem("invalid_capability", "Invalid capability", 500))?;
    let scope = ResourceScope {
        workspace_id: Some(binding.workspace_id.clone()),
        resource_id: Some(binding.resource_id.clone()),
        profile_id: Some(binding.profile_id.clone()),
        ..ResourceScope::default()
    };
    if is_allowed(principal, &capability, Action::Create, &scope, grants) {
        Ok(())
    } else {
        Err(problem("profile_forbidden", "Profile clone denied", 403))
    }
}

fn check_revision(actual: u64, expected: u64) -> Result<(), Problem> {
    if actual == expected {
        Ok(())
    } else {
        Err(
            problem("revision_conflict", "Profile revision conflict", 409)
                .with_field("expectedRevision", expected)
                .with_field("actualRevision", actual),
        )
    }
}

fn share_actions(permission: &ProfileSharePermission) -> Vec<Action> {
    match permission {
        ProfileSharePermission::Read => vec![Action::Read],
        ProfileSharePermission::Operate => vec![Action::Read, Action::Execute],
    }
}

fn share_grant(
    binding: &ProfileResource,
    member_id: &MemberId,
    action: Action,
) -> Result<ResourceGrant, Problem> {
    let action_name = match action {
        Action::Read => "read",
        Action::Execute => "execute",
        _ => {
            return Err(problem(
                "share_permission_invalid",
                "Invalid share permission",
                400,
            ));
        }
    };
    let digest = format!(
        "{:x}",
        Sha256::digest(
            format!(
                "profile-share-v1:{}:{}:{}",
                binding.resource_id.as_str(),
                member_id.as_str(),
                action_name
            )
            .as_bytes(),
        )
    );
    Ok(ResourceGrant {
        id: GrantId::new(format!("grant_ps_{}", &digest[..24]))
            .map_err(|_| problem("grant_id_invalid", "Share grant ID is invalid", 500))?,
        workspace_id: binding.workspace_id.clone(),
        resource_id: binding.resource_id.clone(),
        member_id: member_id.clone(),
        action,
        effect: buglogin_domain::permissions::Effect::Allow,
    })
}

fn timestamp() -> Result<String, Problem> {
    rfc3339("profile.share")
}

fn require_hosted(state: &ApiState) -> Result<(), Problem> {
    if state.local_authority_enabled() {
        return Err(problem(
            "hosted_profile_authority_required",
            "Hosted profile authority is required",
            409,
        ));
    }
    Ok(())
}

fn validate_encrypted_profile_share(
    state: &ApiState,
    binding: &ProfileResource,
    input: &ProfileShareInput,
) -> Result<(), Problem> {
    use std::collections::{BTreeMap, BTreeSet};

    let manifest = state
        .current_sync_manifest(&binding.workspace_id, &binding.resource_id)?
        .ok_or_else(|| {
            problem(
                "profile_manifest_missing",
                "Encrypted profile manifest is unavailable",
                409,
            )
        })?;
    let key_version = manifest
        .payload_encryption
        .as_ref()
        .and_then(|encryption| encryption.key_version.as_deref())
        .ok_or_else(|| {
            problem(
                "profile_key_version_missing",
                "Encrypted profile has no supported resource key version",
                409,
            )
        })?;
    let recipient = state
        .members
        .read()
        .map_err(|_| problem("member_unavailable", "Member service unavailable", 503))?
        .active_members()
        .into_iter()
        .find(|member| member.id == input.member_id && member.workspace_id == binding.workspace_id)
        .ok_or_else(|| problem("member_not_found", "Active workspace member not found", 404))?;
    let now = unix_time_seconds("profile-share")?;
    let active_device_ids = state
        .auth
        .read()
        .map_err(|_| problem("auth_unavailable", "Authentication unavailable", 503))?
        .active_device_ids(&recipient.account_id, now)?;
    if active_device_ids.is_empty() || active_device_ids.len() > 64 {
        return Err(problem(
            "recipient_device_key_missing",
            "Recipient must have between one and 64 active registered devices before encrypted sharing",
            409,
        ));
    }
    let keys = state
        .device_key_directory
        .list_for_account(&recipient.account_id, &active_device_ids)?;
    if keys.len() != active_device_ids.len()
        || keys
            .iter()
            .any(|key| !active_device_ids.contains(&key.device_id))
        || input.recipient_key_envelopes.len() != keys.len()
    {
        return Err(problem(
            "recipient_device_key_missing",
            "Every active recipient device must have an enrolled key and a confirmed envelope",
            409,
        ));
    }

    let keys_by_device: BTreeMap<_, _> = keys
        .into_iter()
        .map(|key| (key.device_id.clone(), key))
        .collect();
    let mut supplied_devices = BTreeSet::new();
    for confirmed in &input.recipient_key_envelopes {
        let envelope = &confirmed.envelope;
        let key = keys_by_device
            .get(&envelope.recipient_device_id)
            .ok_or_else(|| {
                problem(
                    "recipient_device_key_mismatch",
                    "Envelope target is not an active registered recipient device",
                    409,
                )
            })?;
        if !supplied_devices.insert(envelope.recipient_device_id.clone()) {
            return Err(problem(
                "recipient_device_envelope_duplicate",
                "Only one envelope per active recipient device is allowed",
                400,
            ));
        }
        if confirmed.confirmed_fingerprint != key.fingerprint {
            return Err(problem(
                "recipient_device_fingerprint_changed",
                "Recipient device fingerprint changed; review it again before sharing",
                409,
            ));
        }
        if envelope.schema_version != 2
            || envelope.workspace_id != binding.workspace_id
            || envelope.resource_id != binding.resource_id
            || envelope.key_version != key_version
            || envelope.encapped_key.len() != 32
            || envelope.ciphertext.len() != 80
        {
            return Err(problem(
                "profile_key_envelope_scope_mismatch",
                "Recipient envelope does not match the active profile key and scope",
                400,
            ));
        }
    }
    Ok(())
}

fn problem(code: &'static str, message: &'static str, status: u16) -> Problem {
    Problem::new(code, message, status, "profile")
}

fn valid_profile_clone_name(name: &str) -> bool {
    !name.trim().is_empty() && name.chars().count() <= 200
}

fn profile_key_distribution_not_ready() -> Problem {
    problem(
        "profile_key_distribution_not_ready",
        "Encrypted profile access requires a current key envelope for every active recipient device",
        503,
    )
}

fn profile_key_enrollment_required() -> Problem {
    Problem::new(
        "profile_key_enrollment_required",
        "The first encrypted manifest requires a current key envelope for the workspace owner device",
        409,
        "profile",
    )
}

#[cfg(test)]
mod authorization_tests {
    use std::collections::BTreeSet;

    use super::*;
    use buglogin_domain::{
        grant::ResourceGrant,
        ids::{AccountId, GrantId, OrganizationId},
        member::{Member, MemberRole, MemberStatus},
        organization::{Organization, OrganizationPolicy},
        permissions::{Effect, Role},
        resource::{ResourceKind, ResourceState},
        workspace::{Workspace, WorkspacePolicy},
    };

    #[test]
    fn profile_clone_name_limit_counts_characters_like_create_and_update() {
        assert!(valid_profile_clone_name(&"é".repeat(200)));
        assert!(!valid_profile_clone_name(&"é".repeat(201)));
        assert!(!valid_profile_clone_name("   "));
    }

    #[test]
    fn managed_profile_provision_rejects_implicit_group_acl_assignment() {
        let mut profile = buglogin_domain::entities::Profile {
            id: ProfileId::new("prf_implicit_group_acl").unwrap(),
            name: "Implicit group ACL".into(),
            runtime: buglogin_domain::entities::Runtime::Wayfern,
            runtime_version: None,
            runtime_config: None,
            state: ProfileState::Created,
            archived: false,
            fingerprint: None,
            note: None,
            proxy_id: None,
            vpn_id: None,
            group_ids: vec![GroupId::new("grp_implicit_acl").unwrap()],
            tags: Vec::new(),
            extension_group_id: None,
            sync_mode: Some(buglogin_domain::entities::SyncMode::Managed),
            created_at: "2026-10-19T00:00:00Z".into(),
            updated_at: "2026-10-19T00:00:00Z".into(),
        };
        let error = HostedProfileService::prepare_provision(
            profile.clone(),
            WorkspaceId::new("ws_implicit_group_acl").unwrap(),
        )
        .err()
        .expect("group assignment must use the separate authorized mutation");
        assert_eq!(error.code, "profile_groups_mutation_required");

        // Also defend the mutation authority if a plan is constructed outside
        // `prepare_provision` (the plan fields are public for adapter reuse).
        profile.state = ProfileState::Ready;
        let workspace_id = WorkspaceId::new("ws_implicit_group_acl").unwrap();
        let resource_id = HostedProfileService::managed_resource_id(&profile.id).unwrap();
        let manifest = SyncManifest::new(
            workspace_id.clone(),
            resource_id.clone(),
            1,
            1,
            None,
            b"",
            Vec::new(),
        )
        .unwrap();
        let plan = HostedProfileProvisionPlan {
            profile: profile.clone(),
            workspace_id: workspace_id.clone(),
            resource_id: resource_id.clone(),
            manifest,
            resource: Resource {
                id: resource_id.clone(),
                workspace_id: workspace_id.clone(),
                kind: ResourceKind::Profile,
                state: ResourceState::Active,
                display_name: profile.name.clone(),
            },
            binding: ProfileResource {
                profile_id: profile.id.clone(),
                resource_id,
                workspace_id,
                archive_state: ProfileArchiveState::Active,
            },
        };
        let state = crate::http::ApiState::hosted_with_control_store(
            buglogin_browser::profile::ProfileService::disabled(),
            buglogin_storage::control::MemoryControlRecordStore::default(),
        )
        .unwrap();
        let error = HostedProfileService::provision(&state, &Principal::default(), &[], &plan)
            .err()
            .expect("provision must not trust a hand-built plan");
        assert_eq!(error.code, "profile_groups_mutation_required");
        assert!(state.get_profile_record(&profile.id).unwrap().is_none());
    }

    use buglogin_sync::manifest::{
        ManifestObject, ManifestObjectReference, ManifestPayloadEncryption,
        PayloadEncryptionAlgorithm, SyncManifest,
    };

    fn seed_workspace_owner(state: &ApiState, account_id: &AccountId, workspace_id: &WorkspaceId) {
        let admin = Principal {
            roles: [Role::PlatformAdmin].into_iter().collect(),
            ..Principal::default()
        };
        let member_id = MemberId::new(format!("mem_owner_{}", account_id.as_str())).unwrap();
        state
            .members
            .write()
            .unwrap()
            .invite(
                &admin,
                &[],
                Member {
                    id: member_id.clone(),
                    account_id: account_id.clone(),
                    workspace_id: workspace_id.clone(),
                    team_id: None,
                    role: MemberRole::Owner,
                    status: MemberStatus::Invited,
                    invite_expires_at: Some(100),
                },
                1,
            )
            .unwrap();
        state
            .members
            .write()
            .unwrap()
            .activate(
                &Principal {
                    account_id: Some(account_id.clone()),
                    ..Principal::default()
                },
                &[],
                workspace_id,
                &member_id,
                2,
            )
            .unwrap();
    }

    #[test]
    fn hosted_profile_capacity_uses_authoritative_active_workspace_bindings() {
        let account_id = AccountId::new("acct_quota_test").unwrap();
        let organization = Organization {
            id: OrganizationId::new("org_quota_test").unwrap(),
            account_id,
            name: "Quota test".into(),
            created_at: "2026-10-12T00:00:00Z".into(),
        };
        let workspace_id = WorkspaceId::new("ws_quota_test").unwrap();
        let mut workspaces = crate::services::workspaces::WorkspaceService::new(
            organization.clone(),
            OrganizationPolicy {
                max_workspaces: 1,
                offline_entitled: false,
            },
        )
        .unwrap();
        workspaces
            .create_workspace(
                &Principal {
                    roles: std::collections::BTreeSet::from([Role::PlatformAdmin]),
                    ..Principal::default()
                },
                &[],
                &organization.id,
                Workspace {
                    id: workspace_id.clone(),
                    organization_id: organization.id.clone(),
                    name: "Quota test".into(),
                    policy: WorkspacePolicy {
                        max_profiles: 1,
                        max_members: 10,
                        max_storage_bytes: 1024,
                        offline_entitled: false,
                    },
                    created_at: "2026-10-12T00:00:00Z".into(),
                },
            )
            .unwrap();

        let state = crate::http::ApiState::hosted_with_control_store(
            buglogin_browser::profile::ProfileService::disabled(),
            buglogin_storage::control::MemoryControlRecordStore::default(),
        )
        .unwrap();
        *state.workspaces.write().unwrap() = workspaces;
        state
            .replace_profile_resource(ProfileResource {
                profile_id: ProfileId::new("prf_quota_existing").unwrap(),
                resource_id: ResourceId::new("res_quota_existing").unwrap(),
                workspace_id: workspace_id.clone(),
                archive_state: ProfileArchiveState::Active,
            })
            .unwrap();

        assert_eq!(
            state
                .ensure_workspace_profile_capacity(&workspace_id)
                .unwrap_err()
                .code,
            "profile_quota_exceeded"
        );
        state
            .replace_profile_resource(ProfileResource {
                profile_id: ProfileId::new("prf_quota_existing").unwrap(),
                resource_id: ResourceId::new("res_quota_existing").unwrap(),
                workspace_id: workspace_id.clone(),
                archive_state: ProfileArchiveState::Archived,
            })
            .unwrap();
        state
            .ensure_workspace_profile_capacity(&workspace_id)
            .expect("archived profile should not consume active profile capacity");
    }

    #[test]
    fn profile_clone_does_not_copy_group_acl_assignments() {
        let owner = AccountId::new("acct_clone_groups").unwrap();
        let organization = Organization {
            id: OrganizationId::new("org_clone_groups").unwrap(),
            account_id: owner,
            name: "Clone groups".into(),
            created_at: "2026-10-15T00:00:00Z".into(),
        };
        let workspace_id = WorkspaceId::new("ws_clone_groups").unwrap();
        let mut workspaces = crate::services::workspaces::WorkspaceService::new(
            organization.clone(),
            OrganizationPolicy {
                max_workspaces: 1,
                offline_entitled: false,
            },
        )
        .unwrap();
        let principal = Principal {
            roles: std::collections::BTreeSet::from([Role::PlatformAdmin]),
            ..Principal::default()
        };
        workspaces
            .create_workspace(
                &principal,
                &[],
                &organization.id,
                Workspace {
                    id: workspace_id.clone(),
                    organization_id: organization.id.clone(),
                    name: "Clone groups".into(),
                    policy: WorkspacePolicy {
                        max_profiles: 10,
                        max_members: 10,
                        max_storage_bytes: 1024,
                        offline_entitled: false,
                    },
                    created_at: "2026-10-15T00:00:00Z".into(),
                },
            )
            .unwrap();

        let state = crate::http::ApiState::hosted_with_control_store(
            buglogin_browser::profile::ProfileService::disabled(),
            buglogin_storage::control::MemoryControlRecordStore::default(),
        )
        .unwrap();
        *state.workspaces.write().unwrap() = workspaces;
        let source_id = ProfileId::new("prf_clone_groups_source").unwrap();
        let source_resource_id = ResourceId::new("res_clone_groups_source").unwrap();
        let group_id = GroupId::new("grp_clone_groups_private").unwrap();
        state
            .resources
            .write()
            .unwrap()
            .register(
                &principal,
                &[],
                Resource {
                    id: source_resource_id.clone(),
                    workspace_id: workspace_id.clone(),
                    kind: ResourceKind::Profile,
                    state: ResourceState::Active,
                    display_name: "Grouped source".into(),
                },
            )
            .unwrap();
        state
            .replace_profile_resource(ProfileResource {
                profile_id: source_id.clone(),
                resource_id: source_resource_id,
                workspace_id: workspace_id.clone(),
                archive_state: ProfileArchiveState::Active,
            })
            .unwrap();
        let source = buglogin_domain::entities::Profile {
            id: source_id.clone(),
            name: "Grouped source".into(),
            runtime: buglogin_domain::entities::Runtime::Wayfern,
            runtime_version: Some("1.0.0".into()),
            runtime_config: None,
            state: ProfileState::Created,
            archived: false,
            fingerprint: None,
            note: None,
            proxy_id: None,
            vpn_id: None,
            group_ids: vec![group_id],
            tags: Vec::new(),
            extension_group_id: None,
            sync_mode: Some(buglogin_domain::entities::SyncMode::Managed),
            created_at: "2026-10-15T00:00:00Z".into(),
            updated_at: "2026-10-15T00:00:00Z".into(),
        };
        let source = state.create_profile_record(source).unwrap();

        let receipt = HostedProfileService::clone_metadata(
            &state,
            &principal,
            &[],
            ProfileCloneInput {
                source_profile_id: source_id,
                target_profile_id: ProfileId::new("prf_clone_groups_target").unwrap(),
                name: "Ungrouped clone".into(),
                expected_revision: source.revision,
            },
        )
        .unwrap();

        assert!(receipt.profile.profile.group_ids.is_empty());
        assert!(
            !receipt
                .copied_fields
                .iter()
                .any(|field| field == "groupIds")
        );
        assert!(
            receipt
                .excluded_fields
                .iter()
                .any(|field| field == "groupIds")
        );
    }

    #[test]
    fn encrypted_profile_share_requires_a_supported_manifest_key_version() {
        let workspace = WorkspaceId::from_str("ws_share_key_gate").unwrap();
        let resource = ResourceId::from_str("res_share_key_gate").unwrap();
        let envelope = ManifestObject::new(
            buglogin_sync::manifest::RESOURCE_KEY_ENVELOPE_KEY,
            b"opaque-envelope",
        )
        .unwrap();
        let mut manifest = SyncManifest::new(
            workspace.clone(),
            resource.clone(),
            1,
            1,
            None,
            b"encrypted-profile-root",
            vec![envelope.clone()],
        )
        .unwrap();
        manifest.payload_encryption = Some(ManifestPayloadEncryption {
            schema_version: 1,
            algorithm: PayloadEncryptionAlgorithm::Aes256GcmChunkV1,
            key_version: None,
            resource_key_envelope: ManifestObjectReference::from(&envelope),
        });
        let state = crate::http::ApiState::hosted_with_control_store(
            buglogin_browser::profile::ProfileService::disabled(),
            buglogin_storage::control::MemoryControlRecordStore::default(),
        )
        .unwrap();
        state.replace_sync_manifest(manifest).unwrap();

        let binding = ProfileResource {
            profile_id: ProfileId::from_str("prf_share_key_gate").unwrap(),
            resource_id: resource,
            workspace_id: workspace,
            archive_state: ProfileArchiveState::Active,
        };
        let input = ProfileShareInput {
            profile_id: binding.profile_id.clone(),
            member_id: MemberId::from_str("mem_share_key_gate").unwrap(),
            permission: ProfileSharePermission::Read,
            expected_revision: 1,
            recipient_key_envelopes: Vec::new(),
        };
        let error = validate_encrypted_profile_share(&state, &binding, &input).unwrap_err();
        assert_eq!(error.code, "profile_key_version_missing");
        assert_eq!(error.status, 409);
    }

    #[test]
    fn initial_encrypted_manifest_enrolls_only_the_authenticated_owner_device() {
        use buglogin_sync::crypto::{
            ClientKey, DevicePrivateKey, RecoveryKey, seal_profile_key_share,
        };

        let account_id = AccountId::new("acct_initial_key_owner").unwrap();
        let member_id = MemberId::new("mem_owner_acct_initial_key_owner").unwrap();
        let workspace_id = WorkspaceId::new("ws_initial_key_owner").unwrap();
        let profile_id = ProfileId::new("prf_initial_key_owner").unwrap();
        let resource_id = ResourceId::new("res_initial_key_owner").unwrap();
        let device_id = "device_initial_key_owner";
        let state = crate::http::ApiState::hosted_with_control_store(
            buglogin_browser::profile::ProfileService::disabled(),
            buglogin_storage::control::MemoryControlRecordStore::default(),
        )
        .unwrap()
        .with_profile_resource(ProfileResource {
            profile_id: profile_id.clone(),
            resource_id: resource_id.clone(),
            workspace_id: workspace_id.clone(),
            archive_state: ProfileArchiveState::Active,
        });
        seed_workspace_owner(&state, &account_id, &workspace_id);
        let owner = Principal {
            account_id: Some(account_id.clone()),
            member_id: Some(member_id.clone()),
            workspace_ids: [workspace_id.clone()].into_iter().collect(),
            ..Principal::default()
        };
        let genesis = SyncManifest::new(
            workspace_id.clone(),
            resource_id.clone(),
            1,
            1,
            None,
            b"initial-owner-genesis",
            Vec::new(),
        )
        .unwrap();
        state.replace_sync_manifest(genesis).unwrap();

        let now = unix_time_seconds("initial-owner-envelope-test").unwrap();
        state
            .auth
            .write()
            .unwrap()
            .issue_device_session(
                account_id.clone(),
                now + 3_600,
                now + 7_200,
                device_id.into(),
                None,
            )
            .unwrap();
        let device_private = DevicePrivateKey::generate().unwrap();
        let device_public = device_private.public_key().unwrap();
        let public_key_hex = device_public
            .as_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let device_key = state
            .device_key_directory
            .register(
                &account_id,
                device_id,
                &public_key_hex,
                "initial-owner-key-enroll",
                now,
            )
            .unwrap();
        let mut grants = state.grants().unwrap();
        grants.push(capability_grant(
            "grant_initial_key_owner_update",
            &account_id,
            "profile.update",
            Action::Update,
            ResourceScope::resource(workspace_id.clone(), resource_id.clone()),
        ));
        let recipients = HostedProfileService::list_profile_key_rotation_recipients(
            &state,
            &owner,
            &grants,
            &profile_id,
            device_id,
        )
        .unwrap();
        assert_eq!(recipients.len(), 1);
        assert_eq!(recipients[0].member_id, member_id);
        assert_eq!(recipients[0].device_keys.len(), 1);
        assert_eq!(recipients[0].device_keys[0].device_id, device_id);
        let non_owner_id = AccountId::new("acct_initial_key_non_owner").unwrap();
        let non_owner = Principal {
            account_id: Some(non_owner_id.clone()),
            workspace_ids: [workspace_id.clone()].into_iter().collect(),
            ..Principal::default()
        };
        let non_owner_update_grant = capability_grant(
            "grant_initial_key_non_owner_update",
            &non_owner_id,
            "profile.update",
            Action::Update,
            ResourceScope::resource(workspace_id.clone(), resource_id.clone()),
        );
        assert_eq!(
            HostedProfileService::list_profile_key_rotation_recipients(
                &state,
                &non_owner,
                &[non_owner_update_grant],
                &profile_id,
                device_id,
            )
            .unwrap_err()
            .code,
            "profile_key_enrollment_required"
        );

        let client_key = ClientKey::from_bytes([0x61; 32]);
        let recovery_key = RecoveryKey::from_bytes([0x72; 32]);
        let key_version = format!("{:x}", Sha256::digest(client_key.as_bytes()));
        let valid = buglogin_domain::profile::ProfileKeyRotationEnvelopeUpdate {
            member_id: member_id.clone(),
            recipient_key_envelopes: vec![
                buglogin_domain::profile::ConfirmedProfileKeyShareEnvelope {
                    envelope: seal_profile_key_share(
                        workspace_id.clone(),
                        resource_id.clone(),
                        device_id.into(),
                        key_version.clone(),
                        &device_public,
                        &client_key,
                        &recovery_key,
                    )
                    .unwrap(),
                    confirmed_fingerprint: device_key.fingerprint.clone(),
                },
            ],
        };
        assert_eq!(
            HostedProfileService::activate_initial_owner_key_envelope(
                &state,
                &owner,
                &profile_id,
                device_id,
                &key_version,
                &[],
            )
            .unwrap_err()
            .code,
            "profile_key_enrollment_required"
        );
        let mut wrong_device = valid.clone();
        wrong_device.recipient_key_envelopes[0]
            .envelope
            .recipient_device_id = "device_other_owner".into();
        assert_eq!(
            HostedProfileService::activate_initial_owner_key_envelope(
                &state,
                &owner,
                &profile_id,
                device_id,
                &key_version,
                &[wrong_device],
            )
            .unwrap_err()
            .code,
            "profile_key_distribution_not_ready"
        );
        let mut stale_key_version = valid.clone();
        stale_key_version.recipient_key_envelopes[0]
            .envelope
            .key_version = "0".repeat(64);
        assert_eq!(
            HostedProfileService::activate_initial_owner_key_envelope(
                &state,
                &owner,
                &profile_id,
                device_id,
                &key_version,
                &[stale_key_version],
            )
            .unwrap_err()
            .code,
            "profile_key_distribution_not_ready"
        );
        assert_eq!(
            HostedProfileService::activate_initial_owner_key_envelope(
                &state,
                &owner,
                &profile_id,
                "device_other_owner",
                &key_version,
                std::slice::from_ref(&valid),
            )
            .unwrap_err()
            .code,
            "profile_key_distribution_not_ready"
        );
        HostedProfileService::activate_initial_owner_key_envelope(
            &state,
            &owner,
            &profile_id,
            device_id,
            &key_version,
            std::slice::from_ref(&valid),
        )
        .unwrap();
        assert!(
            state
                .has_profile_key_share_envelope_for_member(
                    &profile_id,
                    &workspace_id,
                    &resource_id,
                    &member_id,
                    &key_version,
                )
                .unwrap()
        );

        let envelope_object = ManifestObject::new(
            buglogin_sync::manifest::RESOURCE_KEY_ENVELOPE_KEY,
            b"resource-key-envelope",
        )
        .unwrap();
        let mut encrypted = SyncManifest::new(
            workspace_id,
            resource_id,
            1,
            2,
            Some(1),
            b"initial-encrypted-root",
            vec![envelope_object.clone()],
        )
        .unwrap();
        encrypted.payload_encryption = Some(ManifestPayloadEncryption {
            schema_version: 1,
            algorithm: PayloadEncryptionAlgorithm::Aes256GcmChunkV2,
            key_version: Some(key_version),
            resource_key_envelope: ManifestObjectReference::from(&envelope_object),
        });
        state.replace_sync_manifest(encrypted).unwrap();
        assert!(
            HostedProfileService::authorized(
                &state,
                &owner,
                &grants,
                &profile_id,
                Action::Read,
                "profile.read",
            )
            .unwrap()
        );
    }

    #[test]
    fn hosted_share_authorization_respects_token_scope_and_resource_deny() {
        let workspace_id = WorkspaceId::from_str("ws_profile_share").unwrap();
        let profile_id = ProfileId::from_str("prf_profile_share").unwrap();
        let resource_id = ResourceId::from_str("res_profile_share").unwrap();
        let owner_id = AccountId::from_str("acct_profile_owner").unwrap();
        let recipient_id = AccountId::from_str("acct_profile_recipient").unwrap();
        let member_id = MemberId::from_str("mem_profile_recipient").unwrap();
        let store = buglogin_storage::control::MemoryControlRecordStore::default();
        let state = crate::http::ApiState::hosted_with_control_store(
            buglogin_browser::profile::ProfileService::disabled(),
            store.clone(),
        )
        .unwrap()
        .with_profile_resource(ProfileResource {
            profile_id: profile_id.clone(),
            resource_id: resource_id.clone(),
            workspace_id: workspace_id.clone(),
            archive_state: ProfileArchiveState::Active,
        });
        let owner = Principal {
            account_id: Some(owner_id.clone()),
            workspace_ids: [workspace_id.clone()].into_iter().collect(),
            roles: [Role::WorkspaceAdmin].into_iter().collect(),
            ..Principal::default()
        };
        let admin_grants = vec![
            capability_grant(
                "grant_member_create",
                &owner_id,
                "CAP-MEM-001",
                Action::Create,
                ResourceScope::workspace(workspace_id.clone()),
            ),
            capability_grant(
                "grant_resource_create",
                &owner_id,
                "CAP-RES-001",
                Action::Create,
                ResourceScope::workspace(workspace_id.clone()),
            ),
            capability_grant(
                "grant_share_manage",
                &owner_id,
                "CAP-SHARE-001",
                Action::Manage,
                ResourceScope::resource(workspace_id.clone(), resource_id.clone()),
            ),
            capability_grant(
                "grant_profile_update",
                &owner_id,
                "profile.update",
                Action::Update,
                ResourceScope::resource(workspace_id.clone(), resource_id.clone()),
            ),
        ];

        seed_workspace_owner(&state, &owner_id, &workspace_id);

        state
            .members
            .write()
            .unwrap()
            .invite(
                &owner,
                &admin_grants,
                Member {
                    id: member_id.clone(),
                    account_id: recipient_id.clone(),
                    workspace_id: workspace_id.clone(),
                    team_id: None,
                    role: MemberRole::Member,
                    status: MemberStatus::Invited,
                    invite_expires_at: Some(100),
                },
                1,
            )
            .unwrap();
        state
            .members
            .write()
            .unwrap()
            .activate(
                &Principal {
                    account_id: Some(recipient_id.clone()),
                    ..Principal::default()
                },
                &[],
                &workspace_id,
                &member_id,
                2,
            )
            .unwrap();
        state
            .resources
            .write()
            .unwrap()
            .register(
                &owner,
                &admin_grants,
                Resource {
                    id: resource_id.clone(),
                    workspace_id: workspace_id.clone(),
                    kind: ResourceKind::Profile,
                    state: ResourceState::Active,
                    display_name: "Shared profile".into(),
                },
            )
            .unwrap();
        state
            .resources
            .write()
            .unwrap()
            .grant(
                &owner,
                &admin_grants,
                ResourceGrant {
                    id: GrantId::from_str("rg_profile_share_allow").unwrap(),
                    workspace_id: workspace_id.clone(),
                    resource_id: resource_id.clone(),
                    member_id: member_id.clone(),
                    action: Action::Read,
                    effect: Effect::Allow,
                },
            )
            .unwrap();

        let recipient = Principal {
            account_id: Some(recipient_id.clone()),
            workspace_ids: [workspace_id.clone()].into_iter().collect(),
            ..Principal::default()
        };
        assert_eq!(
            HostedProfileService::authorize_share_mutation(&state, &recipient, &[], &profile_id,)
                .unwrap_err()
                .code,
            "profile_forbidden"
        );
        assert!(
            HostedProfileService::authorized(
                &state,
                &recipient,
                &[],
                &profile_id,
                Action::Read,
                "profile.read",
            )
            .unwrap()
        );
        let mut mcp_policy = crate::mcp::ProductionMcpPolicy::new(state.clone());
        let profile_get = crate::commands::CommandRegistry::get("profile.get").unwrap();
        let mcp_scope = crate::mcp::McpExecutionPolicy::resolve_scope(
            &mut mcp_policy,
            &recipient,
            profile_get,
            &serde_json::json!({ "profileId": profile_id.clone() }),
        )
        .unwrap();
        crate::mcp::McpExecutionPolicy::authorize(
            &mut mcp_policy,
            &recipient,
            profile_get,
            &mcp_scope,
        )
        .unwrap();
        assert!(
            !HostedProfileService::authorized(
                &state,
                &recipient,
                &[],
                &profile_id,
                Action::Delete,
                "profile.delete",
            )
            .unwrap()
        );
        assert!(
            !HostedProfileService::authorized(
                &state,
                &Principal {
                    token_scopes: Some(Default::default()),
                    ..recipient.clone()
                },
                &[],
                &profile_id,
                Action::Read,
                "profile.read",
            )
            .unwrap()
        );

        let envelope = ManifestObject::new(
            buglogin_sync::manifest::RESOURCE_KEY_ENVELOPE_KEY,
            b"opaque-resource-key-envelope",
        )
        .unwrap();
        let sharing_key = buglogin_sync::crypto::ClientKey::from_bytes([0x4d; 32]);
        let recovery_key = buglogin_sync::crypto::RecoveryKey::from_bytes([0x5e; 32]);
        let key_version = format!("{:x}", Sha256::digest(sharing_key.as_bytes()));
        let mut manifest = SyncManifest::new(
            workspace_id.clone(),
            resource_id.clone(),
            1,
            1,
            None,
            b"shared-profile-manifest",
            vec![envelope.clone()],
        )
        .unwrap();
        manifest.payload_encryption = Some(ManifestPayloadEncryption {
            schema_version: 1,
            algorithm: PayloadEncryptionAlgorithm::Aes256GcmChunkV2,
            key_version: Some(key_version.clone()),
            resource_key_envelope: ManifestObjectReference::from(&envelope),
        });
        state.replace_sync_manifest(manifest).unwrap();
        assert!(
            !HostedProfileService::authorized(
                &state,
                &owner,
                &[],
                &profile_id,
                Action::Read,
                "profile.read",
            )
            .unwrap(),
            "role-default permission must not authorize encrypted content without a recipient envelope"
        );
        let binding = state.profile_resource_record(&profile_id).unwrap().unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let active_until = now + 3_600;
        let device_id = "device_profile_share_test";
        state
            .auth
            .write()
            .unwrap()
            .issue_device_session(
                recipient_id.clone(),
                active_until,
                now + 7_200,
                device_id.into(),
                None,
            )
            .unwrap();
        let device_private = buglogin_sync::crypto::DevicePrivateKey::generate().unwrap();
        let device_public = device_private.public_key().unwrap();
        let public_key_hex = device_public
            .as_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let device_key = state
            .device_key_directory
            .register(
                &recipient_id,
                device_id,
                &public_key_hex,
                "profile-share-key-register-0001",
                now,
            )
            .unwrap();
        let wrapped_key = buglogin_sync::crypto::seal_profile_key_share(
            workspace_id.clone(),
            resource_id.clone(),
            device_id.into(),
            key_version.clone(),
            &device_public,
            &sharing_key,
            &recovery_key,
        )
        .unwrap();
        let mut share_input = ProfileShareInput {
            profile_id: profile_id.clone(),
            member_id: member_id.clone(),
            permission: ProfileSharePermission::Read,
            expected_revision: 1,
            recipient_key_envelopes: vec![
                buglogin_domain::profile::ConfirmedProfileKeyShareEnvelope {
                    envelope: wrapped_key,
                    confirmed_fingerprint: device_key.fingerprint.clone(),
                },
            ],
        };
        validate_encrypted_profile_share(&state, &binding, &share_input).unwrap();
        let valid_envelopes = share_input.recipient_key_envelopes.clone();
        share_input.recipient_key_envelopes[0].confirmed_fingerprint = "0".repeat(64);
        assert_eq!(
            validate_encrypted_profile_share(&state, &binding, &share_input)
                .unwrap_err()
                .code,
            "recipient_device_fingerprint_changed"
        );
        share_input.recipient_key_envelopes.clear();
        assert_eq!(
            validate_encrypted_profile_share(&state, &binding, &share_input)
                .unwrap_err()
                .code,
            "recipient_device_key_missing"
        );
        assert!(
            !HostedProfileService::authorized(
                &state,
                &recipient,
                &[],
                &profile_id,
                Action::Read,
                "profile.read",
            )
            .unwrap()
        );
        assert_eq!(
            HostedProfileService::ensure_encrypted_manifest_share_compatible(
                &state,
                &profile_id,
                Some(&key_version),
            )
            .unwrap_err()
            .code,
            "profile_key_distribution_not_ready"
        );
        state
            .replace_profile_key_share_envelopes(
                &profile_id,
                &workspace_id,
                &resource_id,
                &member_id,
                valid_envelopes,
            )
            .unwrap();
        HostedProfileService::ensure_encrypted_manifest_share_compatible(
            &state,
            &profile_id,
            Some(&key_version),
        )
        .unwrap();
        assert_eq!(
            HostedProfileService::ensure_encrypted_manifest_share_compatible(
                &state,
                &profile_id,
                Some(&"f".repeat(64)),
            )
            .unwrap_err()
            .code,
            "profile_key_distribution_not_ready"
        );
        assert!(
            HostedProfileService::authorized(
                &state,
                &recipient,
                &[],
                &profile_id,
                Action::Read,
                "profile.read",
            )
            .unwrap()
        );

        state
            .mark_resource_key_rotation_pending(&workspace_id, &resource_id)
            .unwrap();
        let rotated_key = buglogin_sync::crypto::ClientKey::from_bytes([0x6f; 32]);
        let rotated_recovery = buglogin_sync::crypto::RecoveryKey::from_bytes([0x7a; 32]);
        let rotated_key_version = format!("{:x}", Sha256::digest(rotated_key.as_bytes()));
        assert_eq!(
            HostedProfileService::activate_profile_key_rotation_envelopes(
                &state,
                &profile_id,
                &rotated_key_version,
                &[],
            )
            .unwrap_err()
            .code,
            "profile_key_distribution_not_ready"
        );
        let rotated_envelope = buglogin_sync::crypto::seal_profile_key_share(
            workspace_id.clone(),
            resource_id.clone(),
            device_id.into(),
            rotated_key_version.clone(),
            &device_public,
            &rotated_key,
            &rotated_recovery,
        )
        .unwrap();
        let stale_member_id = MemberId::new("mem_stale_group_recipient").unwrap();
        let stale_device_id = "device_stale_group_recipient";
        let stale_envelope = buglogin_sync::crypto::seal_profile_key_share(
            workspace_id.clone(),
            resource_id.clone(),
            stale_device_id.into(),
            rotated_key_version.clone(),
            &device_public,
            &rotated_key,
            &rotated_recovery,
        )
        .unwrap();
        state
            .replace_profile_key_share_envelopes(
                &profile_id,
                &workspace_id,
                &resource_id,
                &stale_member_id,
                vec![buglogin_domain::profile::ConfirmedProfileKeyShareEnvelope {
                    envelope: stale_envelope,
                    confirmed_fingerprint: device_key.fingerprint.clone(),
                }],
            )
            .unwrap();
        let mut invalid_update = buglogin_domain::profile::ProfileKeyRotationEnvelopeUpdate {
            member_id: member_id.clone(),
            recipient_key_envelopes: vec![
                buglogin_domain::profile::ConfirmedProfileKeyShareEnvelope {
                    envelope: rotated_envelope.clone(),
                    confirmed_fingerprint: "0".repeat(64),
                },
            ],
        };
        assert_eq!(
            HostedProfileService::activate_profile_key_rotation_envelopes(
                &state,
                &profile_id,
                &rotated_key_version,
                &[invalid_update.clone()],
            )
            .unwrap_err()
            .code,
            "profile_key_distribution_not_ready"
        );
        invalid_update.recipient_key_envelopes[0].confirmed_fingerprint =
            device_key.fingerprint.clone();
        HostedProfileService::activate_profile_key_rotation_envelopes(
            &state,
            &profile_id,
            &rotated_key_version,
            &[invalid_update],
        )
        .unwrap();
        assert!(
            state
                .has_profile_key_share_envelope_for_member(
                    &profile_id,
                    &workspace_id,
                    &resource_id,
                    &member_id,
                    &rotated_key_version,
                )
                .unwrap()
        );
        assert!(
            state
                .profile_key_share_envelope(&resource_id, &stale_member_id, stale_device_id)
                .unwrap()
                .is_none()
        );

        state
            .resources
            .write()
            .unwrap()
            .grant(
                &owner,
                &admin_grants,
                ResourceGrant {
                    id: GrantId::from_str("rg_profile_share_deny").unwrap(),
                    workspace_id: workspace_id.clone(),
                    resource_id: resource_id.clone(),
                    member_id: member_id.clone(),
                    action: Action::Read,
                    effect: Effect::Deny,
                },
            )
            .unwrap();
        assert!(
            !HostedProfileService::authorized(
                &state,
                &recipient,
                &[],
                &profile_id,
                Action::Read,
                "profile.read",
            )
            .unwrap()
        );

        state
            .mark_resource_key_rotation_pending(&workspace_id, &resource_id)
            .unwrap();
        assert!(
            !HostedProfileService::authorized(
                &state,
                &recipient,
                &[],
                &profile_id,
                Action::Read,
                "profile.read",
            )
            .unwrap()
        );
        assert!(
            HostedProfileService::authorized(
                &state,
                &owner,
                &admin_grants,
                &profile_id,
                Action::Update,
                "profile.update",
            )
            .unwrap()
        );

        let aggregate = state.control_aggregate().unwrap();
        let encoded = serde_json::to_vec(&aggregate).unwrap();
        let restored: crate::http::ControlAggregate = serde_json::from_slice(&encoded).unwrap();
        state.replace_control_aggregate(restored).unwrap();
        assert!(
            state
                .resource_key_rotation_pending(&workspace_id, &resource_id)
                .unwrap()
        );
        state
            .persist_control_aggregate(
                "test.profile.key.rotation.pending",
                "profile-key-rotation-pending-0001".into(),
                owner_id.as_str().to_owned(),
                true,
                &"a".repeat(64),
                serde_json::json!({"resourceId": resource_id}),
                200,
                serde_json::json!({"persisted": true}),
            )
            .unwrap();
        let reopened = crate::http::ApiState::hosted_with_control_store(
            buglogin_browser::profile::ProfileService::disabled(),
            store.reopen(),
        )
        .unwrap();
        assert!(
            reopened
                .resource_key_rotation_pending(&workspace_id, &resource_id)
                .unwrap()
        );
    }

    #[test]
    fn profile_create_api_scope_covers_only_its_internal_resource_provision() {
        let workspace_id = WorkspaceId::from_str("ws_profile_create_scope").unwrap();
        let resource_id = ResourceId::from_str("res_profile_create_scope").unwrap();
        let account_id = AccountId::from_str("acct_profile_create_scope").unwrap();
        let principal = Principal {
            account_id: Some(account_id.clone()),
            workspace_ids: [workspace_id.clone()].into_iter().collect(),
            token_scopes: Some(BTreeSet::from(["profile.create:create".to_owned()])),
            ..Principal::default()
        };
        let grants = vec![capability_grant(
            "grant_profile_create_resource",
            &account_id,
            "CAP-RES-001",
            Action::Create,
            ResourceScope::workspace(workspace_id.clone()),
        )];

        assert!(
            HostedProfileService::authorize_resource_creation(
                &principal,
                &grants,
                &workspace_id,
                &resource_id,
            )
            .is_ok()
        );

        let mut unrelated_scope = principal.clone();
        unrelated_scope.token_scopes = Some(BTreeSet::from(["profile.read:read".to_owned()]));
        assert_eq!(
            HostedProfileService::authorize_resource_creation(
                &unrelated_scope,
                &grants,
                &workspace_id,
                &resource_id,
            )
            .unwrap_err()
            .code,
            "coordination_forbidden"
        );

        assert_eq!(
            HostedProfileService::authorize_resource_creation(
                &principal,
                &[],
                &workspace_id,
                &resource_id,
            )
            .unwrap_err()
            .code,
            "coordination_forbidden"
        );
    }

    #[test]
    fn effective_profile_key_recipients_enforce_group_and_capability_denies() {
        let workspace_id = WorkspaceId::new("ws_group_key_recipients").unwrap();
        let profile_id = ProfileId::new("prf_group_key_recipients").unwrap();
        let resource_id = ResourceId::new("res_group_key_recipients").unwrap();
        let group_id = buglogin_domain::ids::GroupId::new("grp_group_key_recipients").unwrap();
        let owner_id = AccountId::new("acct_group_key_owner").unwrap();
        let member_id = MemberId::new("mem_group_key_recipient").unwrap();
        let account_id = AccountId::new("acct_group_key_recipient").unwrap();
        let state = crate::http::ApiState::hosted_with_control_store(
            buglogin_browser::profile::ProfileService::disabled(),
            buglogin_storage::control::MemoryControlRecordStore::default(),
        )
        .unwrap()
        .with_profile_resource(ProfileResource {
            profile_id: profile_id.clone(),
            resource_id: resource_id.clone(),
            workspace_id: workspace_id.clone(),
            archive_state: ProfileArchiveState::Active,
        });
        let owner = Principal {
            account_id: Some(owner_id.clone()),
            workspace_ids: [workspace_id.clone()].into_iter().collect(),
            roles: [Role::WorkspaceAdmin].into_iter().collect(),
            ..Principal::default()
        };
        let grants = vec![
            capability_grant(
                "group_recipient_member_create",
                &owner_id,
                "CAP-MEM-001",
                Action::Create,
                ResourceScope::workspace(workspace_id.clone()),
            ),
            capability_grant(
                "group_recipient_resource_create",
                &owner_id,
                "CAP-RES-001",
                Action::Create,
                ResourceScope::workspace(workspace_id.clone()),
            ),
            capability_grant(
                "group_recipient_profile_update",
                &owner_id,
                "profile.update",
                Action::Update,
                ResourceScope {
                    profile_id: Some(profile_id.clone()),
                    ..ResourceScope::default()
                },
            ),
        ];
        seed_workspace_owner(&state, &owner_id, &workspace_id);
        state
            .create_profile_record(Profile {
                id: profile_id.clone(),
                name: "Group key recipient".into(),
                runtime: buglogin_domain::entities::Runtime::Wayfern,
                runtime_version: Some("1.0.0".into()),
                runtime_config: None,
                state: ProfileState::Ready,
                archived: false,
                fingerprint: None,
                note: None,
                proxy_id: None,
                vpn_id: None,
                group_ids: vec![group_id.clone()],
                tags: Vec::new(),
                extension_group_id: None,
                sync_mode: Some(buglogin_domain::entities::SyncMode::Managed),
                created_at: "2026-10-14T00:00:00Z".into(),
                updated_at: "2026-10-14T00:00:00Z".into(),
            })
            .unwrap();
        state
            .members
            .write()
            .unwrap()
            .invite(
                &owner,
                &grants,
                Member {
                    id: member_id.clone(),
                    account_id: account_id.clone(),
                    workspace_id: workspace_id.clone(),
                    team_id: None,
                    role: MemberRole::Member,
                    status: MemberStatus::Invited,
                    invite_expires_at: Some(100),
                },
                1,
            )
            .unwrap();
        state
            .members
            .write()
            .unwrap()
            .activate(
                &Principal {
                    account_id: Some(AccountId::new("acct_group_key_recipient").unwrap()),
                    ..Principal::default()
                },
                &[],
                &workspace_id,
                &member_id,
                2,
            )
            .unwrap();
        state
            .resources
            .write()
            .unwrap()
            .register(
                &owner,
                &grants,
                Resource {
                    id: resource_id.clone(),
                    workspace_id: workspace_id.clone(),
                    kind: ResourceKind::Profile,
                    state: ResourceState::Active,
                    display_name: "Group key recipient".into(),
                },
            )
            .unwrap();
        state
            .groups
            .write()
            .unwrap()
            .import_records(
                vec![crate::services::groups::GroupRecord {
                    group: buglogin_domain::entities::Group {
                        id: group_id.clone(),
                        name: "Encrypted profiles".into(),
                        appearance: None,
                        created_at: "2026-10-14T00:00:00Z".into(),
                        updated_at: "2026-10-14T00:00:00Z".into(),
                    },
                    workspace_id: workspace_id.clone(),
                    revision: 1,
                }],
                vec![buglogin_domain::grant::GroupShareGrant {
                    id: GrantId::new("grt_group_key_recipient_allow").unwrap(),
                    workspace_id: workspace_id.clone(),
                    group_id: group_id.clone(),
                    member_id: member_id.clone(),
                    action: Action::Read,
                    effect: Effect::Allow,
                    revision: 1,
                    created_at: "2026-10-14T00:00:00Z".into(),
                    revoked_at: None,
                }],
            )
            .unwrap();
        let binding = state.profile_resource_record(&profile_id).unwrap().unwrap();
        let owner_member_id = state
            .members
            .read()
            .unwrap()
            .active_for_account(&owner_id)
            .into_iter()
            .find(|member| member.workspace_id == workspace_id)
            .expect("seeded owner membership")
            .id;
        let lead_account_id = AccountId::new("acct_group_key_lead_recipient").unwrap();
        let lead_member_id = MemberId::new("mem_group_key_lead_recipient").unwrap();
        state
            .members
            .write()
            .unwrap()
            .invite(
                &Principal {
                    roles: [Role::PlatformAdmin].into_iter().collect(),
                    ..Principal::default()
                },
                &[],
                Member {
                    id: lead_member_id.clone(),
                    account_id: lead_account_id.clone(),
                    workspace_id: workspace_id.clone(),
                    team_id: None,
                    role: MemberRole::Lead,
                    status: MemberStatus::Invited,
                    invite_expires_at: Some(100),
                },
                3,
            )
            .unwrap();
        state
            .members
            .write()
            .unwrap()
            .activate(
                &Principal {
                    account_id: Some(lead_account_id),
                    ..Principal::default()
                },
                &[],
                &workspace_id,
                &lead_member_id,
                4,
            )
            .unwrap();
        let effective_grants = state.grants().unwrap();
        let lead = Principal {
            account_id: Some(AccountId::new("acct_group_key_lead_recipient").unwrap()),
            workspace_ids: [workspace_id.clone()].into_iter().collect(),
            ..Principal::default()
        };
        assert!(
            HostedProfileService::authorized(
                &state,
                &lead,
                &effective_grants,
                &profile_id,
                Action::Read,
                "profile.read",
            )
            .unwrap()
        );
        let default_owner_recipients = HostedProfileService::effective_profile_key_recipients(
            &state,
            &profile_id,
            &binding,
            &[Action::Read, Action::Execute],
        )
        .unwrap();
        assert!(
            HostedProfileService::authorized(
                &state,
                &owner,
                &effective_grants,
                &profile_id,
                Action::Read,
                "profile.read",
            )
            .unwrap()
        );
        assert!(
            !default_owner_recipients.contains(&owner_member_id)
                && !default_owner_recipients.contains(&lead_member_id),
            "membership role defaults authorize unencrypted access but do not create key-share recipients"
        );
        state
            .members
            .write()
            .unwrap()
            .suspend(&owner, &effective_grants, &workspace_id, &lead_member_id)
            .unwrap();
        state.grants.write().unwrap().push(capability_grant(
            "group_recipient_workspace_profile_read",
            &owner_id,
            "profile.read",
            Action::Read,
            ResourceScope::workspace(workspace_id.clone()),
        ));
        assert!(
            HostedProfileService::effective_profile_key_recipients(
                &state,
                &profile_id,
                &binding,
                &[Action::Read],
            )
            .unwrap()
            .contains(&owner_member_id)
        );
        assert_eq!(
            HostedProfileService::effective_profile_key_recipients(
                &state,
                &profile_id,
                &binding,
                &[Action::Read, Action::Execute],
            )
            .unwrap(),
            std::collections::BTreeSet::from([member_id.clone(), owner_member_id.clone()])
        );
        state
            .grants
            .write()
            .unwrap()
            .retain(|grant| grant.id.as_str() != "group_recipient_workspace_profile_read");
        state.grants.write().unwrap().push(Grant {
            id: GrantId::new("grt_group_key_capability_deny").unwrap(),
            account_id: Some(account_id.clone()),
            member_id: Some(member_id.clone()),
            capability: Capability::new("profile.read").unwrap(),
            action: Action::Read,
            scope: ResourceScope {
                profile_id: Some(profile_id.clone()),
                ..ResourceScope::default()
            },
            effect: Effect::Deny,
        });
        assert!(
            !HostedProfileService::effective_profile_key_recipients(
                &state,
                &profile_id,
                &binding,
                &[Action::Read],
            )
            .unwrap()
            .contains(&member_id),
            "explicit capability deny must prevent key delivery despite a group allow"
        );
        state.grants.write().unwrap().clear();
        state.grants.write().unwrap().push(Grant {
            id: GrantId::new("grt_group_key_owner_rotation_deny").unwrap(),
            account_id: Some(owner_id.clone()),
            member_id: Some(owner_member_id),
            capability: Capability::new("profile.read").unwrap(),
            action: Action::Read,
            scope: ResourceScope {
                profile_id: Some(profile_id.clone()),
                ..ResourceScope::default()
            },
            effect: Effect::Deny,
        });

        state
            .mark_resource_key_rotation_pending(&workspace_id, &resource_id)
            .unwrap();
        assert_eq!(
            HostedProfileService::list_profile_key_rotation_recipients(
                &state,
                &owner,
                &grants,
                &profile_id,
                "device_group_key_recipient",
            )
            .unwrap_err()
            .code,
            "profile_key_distribution_not_ready",
            "an active shared member without a device must block key rotation"
        );

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let device_id = "device_group_key_recipient";
        state
            .auth
            .write()
            .unwrap()
            .issue_device_session(
                account_id.clone(),
                now + 3_600,
                now + 7_200,
                device_id.into(),
                None,
            )
            .unwrap();
        let device_private = buglogin_sync::crypto::DevicePrivateKey::generate().unwrap();
        let device_public = device_private.public_key().unwrap();
        let public_key_hex = device_public
            .as_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        state
            .device_key_directory
            .register(
                &account_id,
                device_id,
                &public_key_hex,
                "group-key-rotation-key-register-0001",
                now,
            )
            .unwrap();
        let rotation_recipients = HostedProfileService::list_profile_key_rotation_recipients(
            &state,
            &owner,
            &grants,
            &profile_id,
            device_id,
        )
        .unwrap();
        assert_eq!(rotation_recipients.len(), 1);
        assert_eq!(rotation_recipients[0].member_id, member_id);
        assert_eq!(rotation_recipients[0].device_keys.len(), 1);
        assert_eq!(rotation_recipients[0].device_keys[0].device_id, device_id);
        let reader = Principal {
            account_id: Some(account_id),
            workspace_ids: [workspace_id.clone()].into_iter().collect(),
            ..Principal::default()
        };
        assert_eq!(
            HostedProfileService::list_profile_key_rotation_recipients(
                &state,
                &reader,
                &[],
                &profile_id,
                device_id,
            )
            .unwrap_err()
            .code,
            "profile_forbidden"
        );

        let direct_share_manager = capability_grant(
            "group_recipient_direct_share_manage",
            &owner_id,
            "CAP-SHARE-001",
            Action::Manage,
            ResourceScope::resource(workspace_id.clone(), resource_id.clone()),
        );
        let direct_deny_id = GrantId::new("rg_group_key_recipient_direct_deny").unwrap();
        state
            .resources
            .write()
            .unwrap()
            .grant(
                &owner,
                &[direct_share_manager.clone()],
                ResourceGrant {
                    id: direct_deny_id.clone(),
                    workspace_id: workspace_id.clone(),
                    resource_id: resource_id.clone(),
                    member_id: member_id.clone(),
                    action: Action::Read,
                    effect: Effect::Deny,
                },
            )
            .unwrap();
        assert!(
            HostedProfileService::effective_profile_key_recipients(
                &state,
                &profile_id,
                &binding,
                &[Action::Read],
            )
            .unwrap()
            .is_empty(),
            "direct resource deny must override a group allow"
        );
        state
            .resources
            .write()
            .unwrap()
            .revoke(&owner, &[direct_share_manager], &direct_deny_id)
            .unwrap();

        state
            .groups
            .write()
            .unwrap()
            .import_records(
                Vec::new(),
                vec![buglogin_domain::grant::GroupShareGrant {
                    id: GrantId::new("grt_group_key_recipient_deny").unwrap(),
                    workspace_id: workspace_id.clone(),
                    group_id,
                    member_id,
                    action: Action::Read,
                    effect: Effect::Deny,
                    revision: 1,
                    created_at: "2026-10-14T00:00:00Z".into(),
                    revoked_at: None,
                }],
            )
            .unwrap();
        assert!(
            HostedProfileService::effective_profile_key_recipients(
                &state,
                &profile_id,
                &binding,
                &[Action::Read, Action::Execute],
            )
            .unwrap()
            .is_empty()
        );
    }

    fn capability_grant(
        id: &str,
        account_id: &buglogin_domain::ids::AccountId,
        capability: &str,
        action: Action,
        scope: ResourceScope,
    ) -> Grant {
        Grant {
            id: GrantId::from_str(id).unwrap(),
            account_id: Some(account_id.clone()),
            member_id: None,
            capability: Capability::new(capability).unwrap(),
            action,
            scope,
            effect: Effect::Allow,
        }
    }
}
