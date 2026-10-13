use buglogin_domain::{
    entities::{Profile, ProfileState},
    errors::Problem,
    grant::ResourceGrant,
    ids::{GrantId, MemberId, ProfileId, ResourceId, WorkspaceId},
    permissions::{Action, Capability, Effect, Grant, Principal, ResourceScope, is_allowed},
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
    pub fn authorized(
        state: &ApiState,
        principal: &Principal,
        grants: &[Grant],
        profile_id: &ProfileId,
        action: Action,
        capability_name: &str,
    ) -> Result<bool, Problem> {
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
        let capability = Capability::new(capability_name)
            .map_err(|_| problem("invalid_capability", "Invalid capability", 500))?;
        if is_allowed(principal, &capability, action, &scope, grants) {
            return Ok(true);
        }
        let Some(binding) = binding else {
            return Ok(false);
        };
        if !matches!(action, Action::Read | Action::Execute) {
            return Ok(false);
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
        let member_id = state
            .members
            .read()
            .map_err(|_| problem("member_unavailable", "Member service unavailable", 503))?
            .active_for_account(account_id)
            .into_iter()
            .find(|member| member.workspace_id == binding.workspace_id)
            .map(|member| member.id);
        let Some(member_id) = member_id else {
            return Ok(false);
        };
        Ok(state
            .resources
            .read()
            .map_err(|_| problem("resource_unavailable", "Resource service unavailable", 503))?
            .is_allowed_in_workspace(
                &binding.resource_id,
                &binding.workspace_id,
                &member_id,
                action,
            ))
    }

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
            state.ensure_workspace_profile_capacity(&binding.workspace_id)?;
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
            ensure_encrypted_profile_key_sharing_ready(
                state,
                &binding.workspace_id,
                &binding.resource_id,
            )?;
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
        state.ensure_workspace_profile_capacity(&plan.workspace_id)?;
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

fn ensure_encrypted_profile_key_sharing_ready(
    state: &ApiState,
    workspace_id: &WorkspaceId,
    resource_id: &ResourceId,
) -> Result<(), Problem> {
    if state
        .current_sync_manifest(workspace_id, resource_id)?
        .is_some_and(|manifest| manifest.payload_encryption.is_some())
    {
        return Err(problem(
            "profile_key_distribution_not_ready",
            "Encrypted profile sharing is unavailable until recipient key envelopes and revocation are configured",
            503,
        ));
    }
    Ok(())
}

fn problem(code: &'static str, message: &'static str, status: u16) -> Problem {
    Problem::new(code, message, status, "profile")
}

#[cfg(test)]
mod authorization_tests {
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
    use buglogin_sync::manifest::{
        ManifestObject, ManifestObjectReference, ManifestPayloadEncryption,
        PayloadEncryptionAlgorithm, SyncManifest,
    };

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
    fn encrypted_profile_sharing_fails_closed_without_recipient_key_lifecycle() {
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

        let error =
            ensure_encrypted_profile_key_sharing_ready(&state, &workspace, &resource).unwrap_err();
        assert_eq!(error.code, "profile_key_distribution_not_ready");
        assert_eq!(error.status, 503);
    }

    #[test]
    fn hosted_share_authorization_respects_token_scope_and_resource_deny() {
        let workspace_id = WorkspaceId::from_str("ws_profile_share").unwrap();
        let profile_id = ProfileId::from_str("prf_profile_share").unwrap();
        let resource_id = ResourceId::from_str("res_profile_share").unwrap();
        let owner_id = AccountId::from_str("acct_profile_owner").unwrap();
        let recipient_id = AccountId::from_str("acct_profile_recipient").unwrap();
        let member_id = MemberId::from_str("mem_profile_recipient").unwrap();
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
        ];

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
            account_id: Some(recipient_id),
            workspace_ids: [workspace_id.clone()].into_iter().collect(),
            ..Principal::default()
        };
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

        state
            .resources
            .write()
            .unwrap()
            .grant(
                &owner,
                &admin_grants,
                ResourceGrant {
                    id: GrantId::from_str("rg_profile_share_deny").unwrap(),
                    workspace_id,
                    resource_id,
                    member_id,
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
