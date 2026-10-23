use buglogin_domain::{
    entities::{
        PROFILE_RUNTIME_CONFIG_SCHEMA_VERSION, Profile, ProfileCreateInput, ProfileState, Runtime,
        SyncMode, WayfernRuntimeConfigError, validate_wayfern_fingerprint_target,
        validate_wayfern_runtime_config,
    },
    errors::Problem,
    ids::{ProfileId, ProxyId},
    permissions::Principal,
};
use sha2::{Digest, Sha256};

pub struct ProfileCreationService;

impl ProfileCreationService {
    pub fn validate_legacy_profile_data(
        runtime: &Runtime,
        runtime_config: Option<&serde_json::Value>,
        fingerprint: Option<&serde_json::Value>,
    ) -> Result<(), Problem> {
        if runtime_config
            .into_iter()
            .chain(fingerprint)
            .any(buglogin_domain::security::contains_secret)
        {
            return Err(problem(
                "profile_secret_data_rejected",
                "Profile runtime configuration contains secret data; store credentials in the operating system vault",
                400,
            ));
        }
        if let Some(runtime_config) = runtime_config {
            validate_wayfern_runtime_config(runtime, runtime_config)
                .map_err(runtime_config_problem)?;
        }
        Ok(())
    }

    pub fn legacy_input(profile: &Profile, workspace_id: Option<&str>) -> ProfileCreateInput {
        ProfileCreateInput {
            id: Some(profile.id.as_str().to_owned()),
            name: profile.name.clone(),
            runtime: profile.runtime.clone(),
            runtime_version: profile.runtime_version.clone(),
            workspace_id: workspace_id.map(str::to_owned),
            proxy_id: profile.proxy_id.as_ref().map(|id| id.as_str().to_owned()),
            note: profile.note.clone(),
            fingerprint_target: None,
        }
    }

    pub fn resolve_workspace(
        requested: Option<&str>,
        header: Option<&str>,
        principal: &Principal,
        managed: bool,
    ) -> Result<Option<buglogin_domain::ids::WorkspaceId>, Problem> {
        let selected = requested.or(header);
        let workspace_id = selected
            .map(buglogin_domain::ids::WorkspaceId::from_str)
            .transpose()
            .map_err(|_| problem("invalid_workspace_id", "Workspace ID is invalid", 400))?
            .or_else(|| {
                (managed && principal.workspace_ids.len() == 1)
                    .then(|| principal.workspace_ids.iter().next().cloned())
                    .flatten()
            });
        if managed && workspace_id.is_none() {
            return Err(problem(
                "workspace_selection_required",
                "Managed profile requires an explicit workspace",
                400,
            ));
        }
        Ok(workspace_id)
    }

    pub fn same_created_profile(current: &Profile, requested: &Profile) -> bool {
        let mut expected = requested.clone();
        expected.created_at.clone_from(&current.created_at);
        expected.updated_at.clone_from(&current.updated_at);
        current == &expected
    }

    pub fn profile_id(
        requested_id: Option<&str>,
        workspace_id: Option<&str>,
        idempotency_key: &str,
    ) -> Result<ProfileId, Problem> {
        let value = requested_id.map(str::to_owned).unwrap_or_else(|| {
            let seed = format!("{}:{idempotency_key}", workspace_id.unwrap_or_default());
            let digest = format!("{:x}", Sha256::digest(seed.as_bytes()));
            format!("prf_{}", &digest[..24])
        });
        ProfileId::from_str(&value)
            .map_err(|_| problem("invalid_profile_id", "Profile ID is invalid", 400))
    }

    pub fn build(
        input: ProfileCreateInput,
        profile_id: ProfileId,
        managed: bool,
    ) -> Result<Profile, Problem> {
        if input.runtime == Runtime::Preserved {
            return Err(problem(
                "profile_runtime_migration_only",
                "Preserved legacy runtimes cannot be selected for new profiles",
                400,
            ));
        }
        if !matches!(input.runtime, Runtime::Wayfern | Runtime::Bugmini) {
            return Err(problem(
                "profile_runtime_unsupported",
                "This legacy browser runtime is not available for new profiles",
                409,
            ));
        }
        let name = input.name.trim();
        if name.is_empty() || name.chars().count() > 200 {
            return Err(problem(
                "profile_name_invalid",
                "Profile name must contain 1 to 200 characters",
                400,
            ));
        }
        if input
            .note
            .as_ref()
            .is_some_and(|note| note.chars().count() > 4096)
        {
            return Err(problem(
                "profile_note_invalid",
                "Profile note is too long",
                400,
            ));
        }
        if let Some(target) = input.fingerprint_target.as_ref() {
            if input.runtime != Runtime::Wayfern {
                return Err(problem(
                    "fingerprint_runtime_unsupported",
                    "Fingerprint targets are supported only by the Wayfern runtime",
                    400,
                ));
            }
            validate_wayfern_fingerprint_target(target).map_err(runtime_config_problem)?;
        }
        let timestamp = crate::clock::rfc3339("profile.create")?;
        let proxy_id = input
            .proxy_id
            .as_deref()
            .map(ProxyId::from_str)
            .transpose()
            .map_err(|_| problem("invalid_proxy_id", "Proxy ID is invalid", 400))?;
        let runtime_config = input.fingerprint_target.map(|target| {
            serde_json::json!({
                "schemaVersion": PROFILE_RUNTIME_CONFIG_SCHEMA_VERSION,
                "wayfern": {
                    "fingerprintTarget": target
                }
            })
        });
        Ok(Profile {
            id: profile_id,
            name: name.to_owned(),
            runtime: input.runtime,
            runtime_version: input.runtime_version,
            runtime_config,
            state: if managed {
                ProfileState::Created
            } else {
                ProfileState::Ready
            },
            archived: false,
            fingerprint: None,
            note: input.note,
            proxy_id,
            vpn_id: None,
            group_ids: Vec::new(),
            tags: Vec::new(),
            extension_group_id: None,
            sync_mode: Some(if managed {
                SyncMode::Managed
            } else {
                SyncMode::Local
            }),
            created_at: timestamp.clone(),
            updated_at: timestamp,
        })
    }
}

fn problem(code: &'static str, title: &'static str, status: u16) -> Problem {
    Problem::new(code, title, status, "profile")
}

fn runtime_config_problem(error: WayfernRuntimeConfigError) -> Problem {
    let (code, title, status) = error.problem_details();
    problem(code, title, status)
}

#[cfg(test)]
mod tests {
    use buglogin_domain::{
        entities::{
            FingerprintOperatingSystem, ProfileCreateInput, ProfileFingerprintTarget, ProfileState,
            Runtime, SyncMode,
        },
        ids::{ProfileId, WorkspaceId},
        permissions::Principal,
    };

    use super::ProfileCreationService;

    #[test]
    fn workspace_resolution_uses_one_shared_precedence_rule() {
        let first = WorkspaceId::from_str("wsp_first").unwrap();
        let second = WorkspaceId::from_str("wsp_second").unwrap();
        let mut principal = Principal::default();
        principal.workspace_ids.insert(first.clone());

        assert_eq!(
            ProfileCreationService::resolve_workspace(
                Some("wsp_second"),
                Some("wsp_first"),
                &principal,
                true
            )
            .unwrap(),
            Some(second)
        );
        assert_eq!(
            ProfileCreationService::resolve_workspace(None, Some("wsp_first"), &principal, true)
                .unwrap(),
            Some(first.clone())
        );
        assert_eq!(
            ProfileCreationService::resolve_workspace(None, None, &principal, true).unwrap(),
            Some(first)
        );
        assert_eq!(
            ProfileCreationService::resolve_workspace(None, None, &principal, false).unwrap(),
            None
        );
    }

    #[test]
    fn managed_workspace_resolution_rejects_ambiguous_or_invalid_scope() {
        let mut principal = Principal::default();
        principal
            .workspace_ids
            .insert(WorkspaceId::from_str("wsp_first").unwrap());
        principal
            .workspace_ids
            .insert(WorkspaceId::from_str("wsp_second").unwrap());

        assert_eq!(
            ProfileCreationService::resolve_workspace(None, None, &principal, true)
                .unwrap_err()
                .code,
            "workspace_selection_required"
        );
        assert_eq!(
            ProfileCreationService::resolve_workspace(Some("bad id"), None, &principal, true)
                .unwrap_err()
                .code,
            "invalid_workspace_id"
        );
    }

    #[test]
    fn unavailable_mobile_fingerprint_targets_are_rejected_before_profile_build() {
        for operating_system in [
            FingerprintOperatingSystem::Android,
            FingerprintOperatingSystem::Ios,
        ] {
            let error = ProfileCreationService::build(
                ProfileCreateInput {
                    id: None,
                    name: "Mobile profile".into(),
                    runtime: Runtime::Wayfern,
                    runtime_version: None,
                    workspace_id: None,
                    proxy_id: None,
                    note: None,
                    fingerprint_target: Some(ProfileFingerprintTarget {
                        operating_system,
                        operating_system_version: Some("26".into()),
                    }),
                },
                ProfileId::from_str("prf_mobile_target").unwrap(),
                true,
            )
            .unwrap_err();

            assert_eq!(error.code, "fingerprint_provider_unavailable");
            assert_eq!(error.status, 409);
        }
    }

    #[test]
    fn managed_profile_can_be_created_before_a_runtime_version_is_published() {
        let profile = ProfileCreationService::build(
            ProfileCreateInput {
                id: None,
                name: "Settings-only profile".into(),
                runtime: Runtime::Wayfern,
                runtime_version: None,
                workspace_id: Some("wsp_metadata_only".into()),
                proxy_id: None,
                note: None,
                fingerprint_target: None,
            },
            ProfileId::from_str("prf_metadata_only").unwrap(),
            true,
        )
        .unwrap();

        assert_eq!(profile.state, ProfileState::Created);
        assert_eq!(profile.runtime_version, None);
        assert_eq!(profile.sync_mode, Some(SyncMode::Managed));
    }

    #[test]
    fn legacy_profile_cannot_bypass_mobile_target_preflight() {
        let runtime_config = serde_json::json!({
            "schemaVersion": 1,
            "wayfern": {
                "fingerprintTarget": {
                    "operatingSystem": "ios",
                    "operatingSystemVersion": "26"
                }
            }
        });

        let error = ProfileCreationService::validate_legacy_profile_data(
            &Runtime::Wayfern,
            Some(&runtime_config),
            None,
        )
        .unwrap_err();

        assert_eq!(error.code, "fingerprint_provider_unavailable");
        assert_eq!(error.status, 409);
    }

    #[test]
    fn unavailable_legacy_runtimes_are_rejected_for_new_profiles() {
        for runtime in [
            Runtime::Bugox,
            Runtime::Bugium,
            Runtime::Bugthor,
            Runtime::Bughelium,
        ] {
            let error = ProfileCreationService::build(
                ProfileCreateInput {
                    id: None,
                    name: "Legacy runtime profile".into(),
                    runtime,
                    runtime_version: None,
                    workspace_id: None,
                    proxy_id: None,
                    note: None,
                    fingerprint_target: None,
                },
                ProfileId::from_str("prf_unsupported_runtime").unwrap(),
                true,
            )
            .unwrap_err();

            assert_eq!(error.code, "profile_runtime_unsupported");
            assert_eq!(error.status, 409);
        }
    }

    #[test]
    fn migration_preserved_runtime_cannot_be_selected_for_new_profiles() {
        let error = ProfileCreationService::build(
            ProfileCreateInput {
                id: None,
                name: "Imported profile".into(),
                runtime: Runtime::Preserved,
                runtime_version: None,
                workspace_id: None,
                proxy_id: None,
                note: None,
                fingerprint_target: None,
            },
            ProfileId::from_str("prf_preserved_runtime").unwrap(),
            true,
        )
        .unwrap_err();

        assert_eq!(error.code, "profile_runtime_migration_only");
    }

    #[test]
    fn migration_preserved_runtime_is_rejected_at_profile_create_deserialization() {
        let input = serde_json::from_value::<ProfileCreateInput>(serde_json::json!({
            "name": "Imported profile",
            "runtime": "preserved"
        }));

        assert!(input.is_err());
    }
}
