use buglogin_domain::{
    entities::{Profile, ProfileState},
    errors::Problem,
    grant::ResourceGrant,
    ids::{GrantId, MemberId, ProfileId, ResourceId, WorkspaceId},
    permissions::{Action, Capability, Grant, Principal, ResourceScope, is_allowed},
    profile::{
        ProfileArchiveState, ProfileCloneInput, ProfileCloneReceipt, ProfileResource,
        ProfileShareInput, ProfileSharePermission, ProfileShareReceipt,
        ProfileShareRevocationReceipt, ProfileShareRevokeInput,
    },
    resource::{Resource, ResourceKind, ResourceState},
};
use buglogin_storage::local::Versioned;
use buglogin_sync::manifest::SyncManifest;
use sha2::{Digest, Sha256};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::http::ApiState;

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

impl HostedProfileService {
    pub fn ensure_delete_supported(state: &ApiState) -> Result<(), Problem> {
        if state.local_authority_enabled() {
            Ok(())
        } else {
            Err(problem(
                "hosted_profile_delete_not_ready",
                "Hosted profile deletion is unavailable until quarantine and object retention are supported",
                503,
            ))
        }
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
        if is_allowed(
            principal,
            &capability,
            Action::Create,
            &ResourceScope::resource(workspace_id.clone(), resource_id.clone()),
            grants,
        ) {
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
        let manifest = SyncManifest::new(
            workspace_id.clone(),
            resource_id.clone(),
            1,
            1,
            None,
            b"",
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
        if input.name.trim().is_empty() || input.name.len() > 200 {
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
            let mut target = source.value.clone();
            target.id = input.target_profile_id;
            target.name = input.name.trim().to_owned();
            target.runtime_version = None;
            target.runtime_config = None;
            target.proxy_id = None;
            target.vpn_id = None;
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
                profile: created.value,
                resource_id: plan.resource_id,
                metadata_only: true,
                copied_fields: vec![
                    "runtime".into(),
                    "fingerprint".into(),
                    "note".into(),
                    "groupIds".into(),
                    "tags".into(),
                ],
                excluded_fields: vec![
                    "runtimeVersion".into(),
                    "runtimeConfig".into(),
                    "proxyId".into(),
                    "vpnId".into(),
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
        let before = state.control_aggregate()?;
        let result = (|| {
            let current = state
                .get_profile_record(&input.profile_id)?
                .ok_or_else(|| problem("profile_not_found", "Profile not found", 404))?;
            let binding = state
                .profile_resource_record(&input.profile_id)?
                .ok_or_else(|| problem("binding_not_found", "Profile binding not found", 404))?;
            check_revision(current.revision, input.expected_revision)?;
            ensure_member_in_workspace(state, &input.member_id, &binding.workspace_id)?;
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
        let before = state.control_aggregate()?;
        let result = (|| {
            let current = state
                .get_profile_record(&input.profile_id)?
                .ok_or_else(|| problem("profile_not_found", "Profile not found", 404))?;
            let binding = state
                .profile_resource_record(&input.profile_id)?
                .ok_or_else(|| problem("binding_not_found", "Profile binding not found", 404))?;
            check_revision(current.revision, input.expected_revision)?;
            let actions = share_actions(&input.permission);
            let mut resources = state.resources.write().map_err(|_| {
                problem("resource_unavailable", "Resource service unavailable", 503)
            })?;
            for action in actions {
                let grant = share_grant(&binding, &input.member_id, action)?;
                resources.revoke(principal, grants, &grant.id)?;
            }
            let updated = state.update_profile_record(input.expected_revision, current.value)?;
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
        state
            .resources
            .write()
            .map_err(|_| problem("resource_unavailable", "Resource service unavailable", 503))?
            .register(principal, grants, plan.resource.clone())?;
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
        let current = state
            .get_profile_record(profile_id)?
            .ok_or_else(|| problem("profile_not_found", "Profile not found", 404))?;
        if archive && current.value.state == ProfileState::Running {
            return Err(problem(
                "profile_running",
                "Running profile must be stopped before archive",
                409,
            ));
        }
        let mut binding = state
            .profile_resource_record(profile_id)?
            .ok_or_else(|| problem("binding_not_found", "Profile binding not found", 404))?;
        if binding.profile_id != *profile_id {
            return Err(problem(
                "profile_binding_invalid",
                "Profile binding is invalid",
                409,
            ));
        }
        let mut profile = current.value;
        profile.archived = archive;
        let updated = state.update_profile_record(expected_revision, profile)?;
        binding.archive_state = if archive {
            ProfileArchiveState::Archived
        } else {
            ProfileArchiveState::Active
        };
        state.replace_profile_resource(binding)?;
        Ok(updated)
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
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .map_err(|_| problem("timestamp_invalid", "Timestamp is invalid", 500))
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

fn problem(code: &'static str, message: &'static str, status: u16) -> Problem {
    Problem::new(code, message, status, "profile")
}
