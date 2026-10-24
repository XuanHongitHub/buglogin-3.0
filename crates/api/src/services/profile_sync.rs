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
        if state.resource_key_rotation_pending(workspace_id, resource_id)? {
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
        ids::ProfileId,
        profile::{ProfileArchiveState, ProfileResource},
    };
    use buglogin_storage::control::MemoryControlRecordStore;

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
