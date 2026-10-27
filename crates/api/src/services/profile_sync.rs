use buglogin_domain::{
    errors::Problem,
    ids::{ProxyId, ResourceId, WorkspaceId},
    permissions::{Action, Capability, Grant, Principal, ResourceScope, is_allowed},
};
use buglogin_network::proxy::{ProxyRecord, RepositoryError};
use buglogin_sync::manifest::{ManifestDependency, SyncManifest};

use crate::http::ApiState;

pub(crate) fn ensure_proxy_update_allowed(
    state: &ApiState,
    workspace_id: &WorkspaceId,
    proxy_id: &ProxyId,
    correlation_id: &str,
) -> Result<(), Problem> {
    for profile in state.list_profile_records()? {
        if profile.value.proxy_id.as_ref() != Some(proxy_id)
            || !state
                .profile_resource_record(&profile.value.id)?
                .is_some_and(|binding| binding.workspace_id == *workspace_id)
        {
            continue;
        }
        if matches!(
            profile.value.state,
            buglogin_domain::entities::ProfileState::Launching
                | buglogin_domain::entities::ProfileState::Running
                | buglogin_domain::entities::ProfileState::Stopping
                | buglogin_domain::entities::ProfileState::Recovering
        ) {
            return Err(Problem::new(
                "proxy_in_use",
                "Stop profiles using this proxy before changing it",
                409,
                correlation_id,
            )
            .with_instance(correlation_id));
        }
    }
    let assignments = state.profile_proxy_assignments.read().map_err(|_| {
        Problem::new(
            "profile_proxy_assignment_unavailable",
            "Profile proxy assignment service unavailable",
            503,
            "proxy",
        )
    })?;
    if assignments.references_proxy(workspace_id, proxy_id) {
        return Err(Problem::new(
            "proxy_assignment_pending",
            "Proxy cannot be changed while a profile assignment is pending sync",
            409,
            "proxy",
        )
        .with_instance(correlation_id));
    }
    Ok(())
}

pub(crate) fn ensure_proxy_unassigned(
    state: &ApiState,
    workspace_id: &WorkspaceId,
    proxy_id: &ProxyId,
    correlation_id: &str,
) -> Result<(), Problem> {
    for profile in state.list_profile_records()? {
        if profile.value.proxy_id.as_ref() != Some(proxy_id) {
            continue;
        }
        let binding = state.profile_resource_record(&profile.value.id)?;
        if binding.is_some_and(|binding| binding.workspace_id == *workspace_id) {
            return Err(Problem::new(
                "proxy_assigned",
                "Proxy is assigned to a profile",
                409,
                "proxy",
            )
            .with_instance(correlation_id));
        }
    }
    let assignments = state.profile_proxy_assignments.read().map_err(|_| {
        Problem::new(
            "profile_proxy_assignment_unavailable",
            "Profile proxy assignment service unavailable",
            503,
            "proxy",
        )
    })?;
    if assignments.references_proxy(workspace_id, proxy_id) {
        return Err(Problem::new(
            "proxy_assigned",
            "Proxy is selected by a pending profile assignment",
            409,
            "proxy",
        )
        .with_instance(correlation_id));
    }
    Ok(())
}

pub(crate) fn authorize_resource_access(
    state: &ApiState,
    principal: &Principal,
    grants: &[Grant],
    workspace_id: &WorkspaceId,
    resource_id: &ResourceId,
    capability_name: &str,
    action: Action,
) -> Result<(), Problem> {
    if let Some(binding) = state.profile_resource_for_resource(workspace_id, resource_id)? {
        if binding.archive_state == buglogin_domain::profile::ProfileArchiveState::Archived {
            return Err(problem(
                "profile_archived",
                "Archived profiles cannot be synchronized or launched",
                409,
            ));
        }
        let managed_key_available = state
            .current_sync_manifest(workspace_id, resource_id)?
            .and_then(|manifest| manifest.payload_encryption)
            .and_then(|encryption| encryption.key_version)
            .map(|key_version| {
                state.has_managed_profile_key_envelope(
                    &binding.profile_id,
                    workspace_id,
                    resource_id,
                    &key_version,
                )
            })
            .transpose()?
            .unwrap_or(false);
        if state.resource_key_rotation_pending(workspace_id, resource_id)?
            && !managed_key_available
        {
            let can_rotate = crate::services::profile_hosted::HostedProfileService::authorized(
                state,
                principal,
                grants,
                &binding.profile_id,
                Action::Update,
                "profile.update",
            )?;
            if can_rotate && action == Action::Execute {
                return Err(problem(
                    "profile_key_rotation_required",
                    "Complete profile key rotation before launching this shared profile",
                    409,
                ));
            }
            if !can_rotate || !matches!(action, Action::Read | Action::Update) {
                return Err(problem(
                    "coordination_forbidden",
                    "Coordination access denied",
                    403,
                ));
            }
            if action == Action::Read {
                return Ok(());
            }
        }
        // HostedProfileService checks active share grants and the caller's
        // device envelope for encrypted manifests. Requiring profile.update
        // here would silently disable read-only profile sharing.
        if crate::services::profile_hosted::HostedProfileService::authorized(
            state,
            principal,
            grants,
            &binding.profile_id,
            action,
            capability_name,
        )? {
            return Ok(());
        }
        // An Operate share permits a member to launch and use an encrypted
        // profile. Allow that same device-authorized session to upload the
        // resulting browser snapshot under its lease, without granting profile
        // metadata updates or access to unencrypted/shared read-only profiles.
        // The pending-key-rotation branch above deliberately runs first and
        // continues to require the profile owner/update authority.
        if action == Action::Update
            && capability_name == "profile.update"
            && state
                .current_sync_manifest(workspace_id, resource_id)?
                .is_some_and(|manifest| manifest.payload_encryption.is_some())
            && !profile_update_is_restricted(principal, grants, &binding)
            && crate::services::profile_hosted::HostedProfileService::authorized(
                state,
                principal,
                grants,
                &binding.profile_id,
                Action::Execute,
                "profile.launch",
            )?
        {
            return Ok(());
        }
    }
    let capability = Capability::new(capability_name)
        .map_err(|_| problem("invalid_capability", "Invalid capability", 500))?;
    if is_allowed(
        principal,
        &capability,
        action,
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

fn profile_update_is_restricted(
    principal: &Principal,
    grants: &[Grant],
    binding: &buglogin_domain::profile::ProfileResource,
) -> bool {
    let capability = Capability::new("profile.update").expect("static capability");
    let scope = ResourceScope {
        workspace_id: Some(binding.workspace_id.clone()),
        resource_id: Some(binding.resource_id.clone()),
        profile_id: Some(binding.profile_id.clone()),
        ..ResourceScope::default()
    };
    principal.token_scopes.as_ref().is_some_and(|scopes| {
        !scopes.contains("profile.update:update") && !scopes.contains("profile.update:*")
    }) || grants.iter().any(|grant| {
        grant.capability == capability
            && grant.action == Action::Update
            && grant.effect == buglogin_domain::permissions::Effect::Deny
            && grant.scope.contains(&scope)
            && grant
                .account_id
                .as_ref()
                .is_none_or(|id| principal.account_id.as_ref() == Some(id))
            && grant
                .member_id
                .as_ref()
                .is_none_or(|id| principal.member_id.as_ref() == Some(id))
    })
}

pub(crate) fn reconcile_managed_profile_resource_registration(
    state: &ApiState,
    workspace_id: &WorkspaceId,
    resource_id: &ResourceId,
) -> Result<bool, Problem> {
    let Some(binding) = state.profile_resource_for_resource(workspace_id, resource_id)? else {
        return Ok(false);
    };
    if binding.archive_state != buglogin_domain::profile::ProfileArchiveState::Active {
        return Ok(false);
    }
    let Some(profile) = state.get_profile_record(&binding.profile_id)? else {
        return Ok(false);
    };
    // The active profile-resource binding is the authoritative indication that
    // this profile is hosted. Older hosted profiles may predate `sync_mode` and
    // have it unset; that must not strand their lease behind a missing resource
    // registry row.
    if profile.value.archived {
        return Ok(false);
    }
    let resource = buglogin_domain::resource::Resource {
        id: binding.resource_id,
        workspace_id: binding.workspace_id,
        kind: buglogin_domain::resource::ResourceKind::Profile,
        state: buglogin_domain::resource::ResourceState::Active,
        display_name: profile.value.name,
    };
    state
        .resources
        .write()
        .map_err(|_| {
            Problem::new(
                "resource_unavailable",
                "Resource service unavailable",
                503,
                "sync",
            )
        })?
        .reconcile_managed_profile_registration(resource)?;
    Ok(true)
}

pub(crate) fn validate_proxy_dependency(
    state: &ApiState,
    principal: &Principal,
    grants: &[Grant],
    workspace_id: &WorkspaceId,
    resource_id: &ResourceId,
    manifest: &SyncManifest,
    allow_stale_revision: bool,
    require_credential_version: bool,
) -> Result<Option<ProxyRecord>, Problem> {
    let Some(binding) = state.profile_resource_for_resource(workspace_id, resource_id)? else {
        return if manifest.dependencies.is_empty() {
            Ok(None)
        } else {
            Err(problem(
                "manifest_dependency_profile_missing",
                "Proxy dependencies require a bound profile",
                409,
            ))
        };
    };
    let profile = state
        .get_profile_record(&binding.profile_id)?
        .ok_or_else(|| {
            problem(
                "profile_binding_invalid",
                "Profile resource binding has no profile record",
                409,
            )
        })?
        .value;

    let Some(proxy_id) = profile.proxy_id else {
        return if manifest.dependencies.is_empty() {
            Ok(None)
        } else {
            Err(problem(
                "manifest_dependency_mismatch",
                "Manifest contains a proxy dependency that the profile does not select",
                409,
            ))
        };
    };

    let [
        ManifestDependency::Proxy {
            proxy_id: dependency_id,
            revision,
            credential_version,
            credential_envelope_object,
            ..
        },
    ] = manifest.dependencies.as_slice()
    else {
        return Err(problem(
            "manifest_dependency_mismatch",
            "Manifest must contain exactly the profile's selected proxy dependency",
            409,
        ));
    };
    if dependency_id != &proxy_id {
        return Err(problem(
            "manifest_dependency_mismatch",
            "Manifest proxy dependency does not match profile selection",
            409,
        ));
    }
    let record = state
        .proxies
        .read()
        .map_err(|_| problem("proxy_unavailable", "Proxy service unavailable", 503))?
        .get(workspace_id, &proxy_id, principal, grants)
        .map_err(proxy_repository_problem)?;

    if *revision != record.revision && !allow_stale_revision {
        return Err(problem(
            "manifest_dependency_stale",
            "Proxy dependency revision is stale",
            409,
        )
        .with_field("expectedRevision", record.revision)
        .with_field("actualRevision", *revision));
    }
    if !allow_stale_revision
        && record.proxy.credential_ref.is_some() != credential_envelope_object.is_some()
    {
        return Err(problem(
            "proxy_credential_envelope_required",
            "Proxy credential envelope does not match proxy credential state",
            409,
        ));
    }
    if !allow_stale_revision
        && credential_version
            .as_ref()
            .is_some_and(|version| record.proxy.credential_ref.as_ref() != Some(version))
    {
        return Err(problem(
            "proxy_credential_version_stale",
            "Proxy credential changed; sync the current credential before activating this profile generation",
            409,
        ));
    }
    if require_credential_version
        && !allow_stale_revision
        && record.proxy.credential_ref.is_some()
        && credential_version.is_none()
    {
        return Err(problem(
            "proxy_credential_version_required",
            "New profile generations must identify the current proxy credential version",
            409,
        ));
    }
    Ok(Some(record))
}

pub(crate) fn proxy_repository_problem(error: RepositoryError) -> Problem {
    match error {
        RepositoryError::Forbidden => {
            Problem::new("forbidden", "Proxy access denied", 403, "proxy")
        }
        RepositoryError::InvalidProxy => {
            Problem::new("invalid_proxy", "Proxy is invalid", 400, "proxy")
        }
        RepositoryError::ProxyIdConflict => Problem::new(
            "proxy_conflict",
            "Proxy conflicts with existing inventory",
            409,
            "proxy",
        ),
        RepositoryError::ProxyNotFound | RepositoryError::WorkspaceMismatch => {
            Problem::new("proxy_not_found", "Proxy not found", 404, "proxy")
        }
        RepositoryError::RevisionConflict { expected, actual } => Problem::new(
            "storage_revision_conflict",
            "Storage revision conflict",
            409,
            "storage",
        )
        .with_field("expectedRevision", expected)
        .with_field("actualRevision", actual),
        RepositoryError::RevisionExhausted | RepositoryError::ProxyAssigned => {
            Problem::new("proxy_conflict", "Proxy cannot be changed", 409, "proxy")
        }
        RepositoryError::SnapshotVersionUnsupported { .. }
        | RepositoryError::SnapshotInvalidWorkspace
        | RepositoryError::SnapshotInvalidRevision
        | RepositoryError::SnapshotDuplicateProxyId
        | RepositoryError::SnapshotDuplicateEndpoint => Problem::new(
            "proxy_state_invalid",
            "Proxy state is invalid",
            500,
            "control",
        ),
    }
}

fn problem(code: &'static str, title: &'static str, status: u16) -> Problem {
    Problem::new(code, title, status, "sync")
}

#[cfg(test)]
mod tests {
    use super::*;
    use buglogin_browser::profile::ProfileService;
    use buglogin_domain::{
        entities::{Profile, ProfileState, Runtime, SyncMode},
        grant::ResourceGrant,
        ids::{AccountId, GrantId, MemberId, ProfileId},
        member::{Member, MemberRole, MemberStatus},
        permissions::{Effect, Role},
        profile::{
            ConfirmedProfileKeyShareEnvelope, ProfileArchiveState, ProfileKeyShareEnvelope,
            ProfileResource,
        },
        resource::{Resource, ResourceKind, ResourceState},
    };
    use buglogin_storage::control::MemoryControlRecordStore;
    use buglogin_sync::manifest::{
        ManifestObject, ManifestObjectReference, ManifestPayloadEncryption,
        PayloadEncryptionAlgorithm, SyncManifest,
    };

    #[test]
    fn encrypted_operate_share_can_sync_under_lease_without_metadata_update_authority() {
        let workspace = WorkspaceId::new("ws_operate_sync_write").unwrap();
        let resource = ResourceId::new("res_operate_sync_write").unwrap();
        let profile = ProfileId::new("prf_operate_sync_write").unwrap();
        let owner_id = AccountId::new("acct_operate_sync_owner").unwrap();
        let recipient_id = AccountId::new("acct_operate_sync_recipient").unwrap();
        let member_id = MemberId::new("mem_operate_sync_recipient").unwrap();
        let state = ApiState::hosted_with_control_store(
            ProfileService::disabled(),
            MemoryControlRecordStore::default(),
        )
        .unwrap()
        .with_profile_resource(ProfileResource {
            profile_id: profile.clone(),
            resource_id: resource.clone(),
            workspace_id: workspace.clone(),
            archive_state: ProfileArchiveState::Active,
        });
        let owner = Principal {
            account_id: Some(owner_id),
            roles: [Role::PlatformAdmin].into_iter().collect(),
            ..Principal::default()
        };
        state
            .members
            .write()
            .unwrap()
            .invite(
                &owner,
                &[],
                Member {
                    id: member_id.clone(),
                    account_id: recipient_id.clone(),
                    workspace_id: workspace.clone(),
                    team_id: None,
                    role: MemberRole::Member,
                    status: MemberStatus::Invited,
                    invite_expires_at: Some(100),
                },
                1,
            )
            .unwrap();
        let recipient = Principal {
            account_id: Some(recipient_id),
            workspace_ids: [workspace.clone()].into_iter().collect(),
            ..Principal::default()
        };
        state
            .members
            .write()
            .unwrap()
            .activate(&recipient, &[], &workspace, &member_id, 2)
            .unwrap();
        state
            .resources
            .write()
            .unwrap()
            .register(
                &owner,
                &[],
                Resource {
                    id: resource.clone(),
                    workspace_id: workspace.clone(),
                    kind: ResourceKind::Profile,
                    state: ResourceState::Active,
                    display_name: "Shared encrypted profile".into(),
                },
            )
            .unwrap();
        for (id, action) in [
            ("rg_operate_sync_read", Action::Read),
            ("rg_operate_sync_execute", Action::Execute),
        ] {
            state
                .resources
                .write()
                .unwrap()
                .grant(
                    &owner,
                    &[],
                    ResourceGrant {
                        id: GrantId::new(id).unwrap(),
                        workspace_id: workspace.clone(),
                        resource_id: resource.clone(),
                        member_id: member_id.clone(),
                        action,
                        effect: Effect::Allow,
                    },
                )
                .unwrap();
        }

        let unencrypted = SyncManifest::new(
            workspace.clone(),
            resource.clone(),
            1,
            1,
            None,
            b"unencrypted-genesis",
            Vec::new(),
        )
        .unwrap();
        state.replace_sync_manifest(unencrypted).unwrap();
        assert!(
            authorize_resource_access(
                &state,
                &recipient,
                &[],
                &workspace,
                &resource,
                "profile.update",
                Action::Update,
            )
            .is_err()
        );

        let envelope_object = ManifestObject::new(
            buglogin_sync::manifest::RESOURCE_KEY_ENVELOPE_KEY,
            b"envelope",
        )
        .unwrap();
        let key_version = "a".repeat(64);
        let mut encrypted = SyncManifest::new(
            workspace.clone(),
            resource.clone(),
            1,
            2,
            Some(1),
            b"encrypted-profile",
            vec![envelope_object.clone()],
        )
        .unwrap();
        encrypted.payload_encryption = Some(ManifestPayloadEncryption {
            schema_version: 1,
            algorithm: PayloadEncryptionAlgorithm::Aes256GcmChunkV2,
            key_version: Some(key_version.clone()),
            resource_key_envelope: ManifestObjectReference::from(&envelope_object),
        });
        state.replace_sync_manifest(encrypted).unwrap();
        state
            .replace_profile_key_share_envelope_set(
                &profile,
                &workspace,
                &resource,
                vec![(
                    member_id.clone(),
                    vec![ConfirmedProfileKeyShareEnvelope {
                        envelope: ProfileKeyShareEnvelope {
                            schema_version: 2,
                            workspace_id: workspace.clone(),
                            resource_id: resource.clone(),
                            recipient_device_id: "device_operate_sync_recipient".into(),
                            key_version,
                            encapped_key: vec![1; 32],
                            ciphertext: vec![2; 80],
                        },
                        confirmed_fingerprint: "b".repeat(64),
                    }],
                )],
            )
            .unwrap();

        authorize_resource_access(
            &state,
            &recipient,
            &[],
            &workspace,
            &resource,
            "profile.update",
            Action::Update,
        )
        .unwrap();
        assert!(
            !crate::services::profile_hosted::HostedProfileService::authorized(
                &state,
                &recipient,
                &[],
                &profile,
                Action::Update,
                "profile.update",
            )
            .unwrap(),
            "Operate must not grant profile metadata updates"
        );

        let update_capability = Capability::new("profile.update").unwrap();
        let scope = ResourceScope::resource(workspace.clone(), resource.clone());
        let explicit_deny = Grant {
            id: GrantId::new("grant_operate_sync_update_deny").unwrap(),
            account_id: recipient.account_id.clone(),
            member_id: None,
            capability: update_capability.clone(),
            action: Action::Update,
            scope: scope.clone(),
            effect: Effect::Deny,
        };
        assert!(
            authorize_resource_access(
                &state,
                &recipient,
                std::slice::from_ref(&explicit_deny),
                &workspace,
                &resource,
                "profile.update",
                Action::Update,
            )
            .is_err()
        );
        let scoped_recipient = Principal {
            token_scopes: Some(["profile.launch:execute".to_owned()].into_iter().collect()),
            ..recipient.clone()
        };
        assert!(
            authorize_resource_access(
                &state,
                &scoped_recipient,
                &[],
                &workspace,
                &resource,
                "profile.update",
                Action::Update,
            )
            .is_err()
        );

        state
            .resources
            .write()
            .unwrap()
            .revoke(
                &owner,
                &[],
                &GrantId::new("rg_operate_sync_execute").unwrap(),
            )
            .unwrap();
        assert!(
            authorize_resource_access(
                &state,
                &recipient,
                &[],
                &workspace,
                &resource,
                "profile.update",
                Action::Update,
            )
            .is_err(),
            "read-only shares must not upload snapshots"
        );
        state
            .resources
            .write()
            .unwrap()
            .grant(
                &owner,
                &[],
                ResourceGrant {
                    id: GrantId::new("rg_operate_sync_execute_again").unwrap(),
                    workspace_id: workspace.clone(),
                    resource_id: resource.clone(),
                    member_id,
                    action: Action::Execute,
                    effect: Effect::Allow,
                },
            )
            .unwrap();
        state
            .mark_resource_key_rotation_pending(&workspace, &resource)
            .unwrap();
        assert!(
            authorize_resource_access(
                &state,
                &recipient,
                &[],
                &workspace,
                &resource,
                "profile.update",
                Action::Update,
            )
            .is_err(),
            "Operate cannot bypass pending key rotation"
        );
    }

    #[test]
    fn proxy_update_is_blocked_while_a_bound_profile_is_running() {
        let workspace = WorkspaceId::new("ws_proxy_update_running").unwrap();
        let profile_id = ProfileId::new("prf_proxy_update_running").unwrap();
        let proxy_id = ProxyId::new("pxy_proxy_update_running").unwrap();
        let state = ApiState::hosted_with_control_store(
            ProfileService::disabled(),
            MemoryControlRecordStore::default(),
        )
        .unwrap()
        .with_profile_resource(ProfileResource {
            profile_id: profile_id.clone(),
            resource_id: ResourceId::new("res_proxy_update_running").unwrap(),
            workspace_id: workspace.clone(),
            archive_state: ProfileArchiveState::Active,
        });
        state
            .create_profile_record(Profile {
                id: profile_id,
                name: "Running proxy consumer".into(),
                runtime: Runtime::Wayfern,
                runtime_version: Some("1.0.0".into()),
                runtime_config: None,
                state: ProfileState::Running,
                archived: false,
                fingerprint: None,
                note: None,
                proxy_id: Some(proxy_id.clone()),
                vpn_id: None,
                group_ids: Vec::new(),
                tags: Vec::new(),
                extension_group_id: None,
                sync_mode: Some(SyncMode::Managed),
                created_at: "2026-10-17T00:00:00Z".into(),
                updated_at: "2026-10-17T00:00:00Z".into(),
            })
            .unwrap();

        let error = ensure_proxy_update_allowed(&state, &workspace, &proxy_id, "proxy-update-test")
            .unwrap_err();

        assert_eq!(error.status, 409);
        assert_eq!(error.code, "proxy_in_use");
        assert_eq!(error.correlation_id, "proxy-update-test");
    }

    #[test]
    fn proxy_update_is_blocked_while_a_profile_assignment_is_pending() {
        let workspace = WorkspaceId::new("ws_proxy_assignment_pending").unwrap();
        let profile_id = ProfileId::new("prf_proxy_assignment_pending").unwrap();
        let proxy_id = ProxyId::new("pxy_proxy_assignment_pending").unwrap();
        let state = ApiState::hosted_with_control_store(
            ProfileService::disabled(),
            MemoryControlRecordStore::default(),
        )
        .unwrap()
        .with_profile_resource(ProfileResource {
            profile_id: profile_id.clone(),
            resource_id: ResourceId::new("res_proxy_assignment_pending").unwrap(),
            workspace_id: workspace.clone(),
            archive_state: ProfileArchiveState::Active,
        });
        state
            .create_profile_record(Profile {
                id: profile_id.clone(),
                name: "Pending proxy consumer".into(),
                runtime: Runtime::Wayfern,
                runtime_version: Some("1.0.0".into()),
                runtime_config: None,
                state: ProfileState::Ready,
                archived: false,
                fingerprint: None,
                note: None,
                proxy_id: None,
                vpn_id: None,
                group_ids: Vec::new(),
                tags: Vec::new(),
                extension_group_id: None,
                sync_mode: Some(SyncMode::Managed),
                created_at: "2026-10-17T00:00:00Z".into(),
                updated_at: "2026-10-17T00:00:00Z".into(),
            })
            .unwrap();
        state
            .profile_proxy_assignments
            .write()
            .unwrap()
            .set(
                &workspace,
                crate::services::profile_proxy_assignment::ProfileProxyAssignmentSetInput {
                    workspace_id: workspace.clone(),
                    profile_id,
                    expected_profile_revision: 1,
                    proxy_id: Some(proxy_id.clone()),
                },
            )
            .unwrap();

        let error = ensure_proxy_update_allowed(
            &state,
            &workspace,
            &proxy_id,
            "proxy-assignment-update-test",
        )
        .unwrap_err();

        assert_eq!(error.status, 409);
        assert_eq!(error.code, "proxy_assignment_pending");
    }

    #[test]
    fn repairs_missing_registration_for_active_hosted_profile_without_legacy_sync_mode() {
        let workspace = WorkspaceId::new("ws_resource_repair").unwrap();
        let profile_id = ProfileId::new("prf_resource_repair").unwrap();
        let resource_id = ResourceId::new("res_resource_repair").unwrap();
        let state = ApiState::hosted_with_control_store(
            ProfileService::disabled(),
            MemoryControlRecordStore::default(),
        )
        .unwrap()
        .with_profile_resource(ProfileResource {
            profile_id: profile_id.clone(),
            resource_id: resource_id.clone(),
            workspace_id: workspace.clone(),
            archive_state: ProfileArchiveState::Active,
        });
        state
            .create_profile_record(Profile {
                id: profile_id,
                name: "Recoverable profile".into(),
                runtime: Runtime::Wayfern,
                runtime_version: Some("1.0.0".into()),
                runtime_config: None,
                state: ProfileState::Ready,
                archived: false,
                fingerprint: None,
                note: None,
                proxy_id: None,
                vpn_id: None,
                group_ids: Vec::new(),
                tags: Vec::new(),
                extension_group_id: None,
                // Older hosted profile records may omit this optional field.
                sync_mode: None,
                created_at: "2026-10-24T00:00:00Z".into(),
                updated_at: "2026-10-24T00:00:00Z".into(),
            })
            .unwrap();

        assert!(
            reconcile_managed_profile_resource_registration(&state, &workspace, &resource_id,)
                .unwrap()
        );
        assert!(
            state
                .resources
                .read()
                .unwrap()
                .exists_in_workspace(&resource_id, &workspace)
        );
        assert!(
            !reconcile_managed_profile_resource_registration(
                &state,
                &WorkspaceId::new("ws_other_resource_repair").unwrap(),
                &resource_id,
            )
            .unwrap()
        );
    }
}
