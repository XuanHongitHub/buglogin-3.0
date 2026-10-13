use std::path::PathBuf;

use buglogin_domain::{entities::ProfileState, errors::Problem, ids::ProfileId};
use serde::Serialize;

use crate::{http::ApiState, services::profile_hosted::HostedProfileService};

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileDeleteReceipt {
    pub id: ProfileId,
    pub deleted: bool,
    pub recoverable: bool,
    pub quarantine_path: PathBuf,
}

pub struct ProfileDeletionService;

impl ProfileDeletionService {
    pub fn delete_local(
        state: &ApiState,
        profile_id: &ProfileId,
        expected_revision: u64,
        confirmation: &str,
    ) -> Result<ProfileDeleteReceipt, Problem> {
        HostedProfileService::ensure_delete_supported(state)?;
        if confirmation != "QUARANTINE_PROFILE_DATA" {
            return Err(Problem::new(
                "delete_not_confirmed",
                "Profile deletion is not confirmed",
                409,
                "profile",
            ));
        }

        let quarantine_path = match state.profiles.get(profile_id)? {
            Some(current) => {
                if matches!(
                    current.value.state,
                    ProfileState::Running | ProfileState::Launching | ProfileState::Stopping
                ) {
                    return Err(Problem::new(
                        "profile_running",
                        "Running profile cannot be deleted",
                        409,
                        "profile",
                    ));
                }
                match state.profiles.delete(expected_revision, &current.value) {
                    Ok(path) => path,
                    Err(delete_error) => state
                        .profiles
                        .quarantined_path(expected_revision, profile_id)?
                        .ok_or(delete_error)?,
                }
            }
            None => state
                .profiles
                .quarantined_path(expected_revision, profile_id)?
                .ok_or_else(|| {
                    Problem::new("profile_not_found", "Profile not found", 404, "profile")
                })?,
        };

        Ok(ProfileDeleteReceipt {
            id: profile_id.clone(),
            deleted: true,
            recoverable: true,
            quarantine_path,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use buglogin_browser::profile::ProfileService;
    use buglogin_domain::{
        entities::{Profile, ProfileState, Runtime, SyncMode},
        ids::ProfileId,
    };
    use buglogin_storage::local::LocalRepository;

    use crate::{http::ApiState, services::profile_deletion::ProfileDeletionService};

    fn root() -> PathBuf {
        std::env::temp_dir().join(format!("buglogin-profile-delete-{}", std::process::id()))
    }

    fn profile() -> Profile {
        Profile {
            id: ProfileId::from_str("prf_delete_recovery").unwrap(),
            name: "Delete recovery".into(),
            runtime: Runtime::Wayfern,
            runtime_version: None,
            runtime_config: None,
            state: ProfileState::Created,
            archived: false,
            fingerprint: None,
            note: None,
            proxy_id: None,
            vpn_id: None,
            group_ids: vec![],
            tags: vec![],
            extension_group_id: None,
            sync_mode: Some(SyncMode::Local),
            created_at: "2026-10-12T00:00:00Z".into(),
            updated_at: "2026-10-12T00:00:00Z".into(),
        }
    }

    #[test]
    fn deletion_retry_recovers_only_the_matching_quarantine() {
        let root = root();
        let _ = fs::remove_dir_all(&root);
        let repository = LocalRepository::open(&root).unwrap();
        let profiles = ProfileService::new(repository);
        let created = profiles.create(profile()).unwrap();
        let profile_id = created.value.id.clone();
        let state = ApiState::local(profiles.clone());

        let first = ProfileDeletionService::delete_local(
            &state,
            &profile_id,
            created.revision,
            "QUARANTINE_PROFILE_DATA",
        )
        .unwrap();
        drop(state);
        drop(profiles);

        let reopened_profiles = ProfileService::new(LocalRepository::open(&root).unwrap());
        let reopened_state = ApiState::local(reopened_profiles.clone());
        let retry = ProfileDeletionService::delete_local(
            &reopened_state,
            &profile_id,
            created.revision,
            "QUARANTINE_PROFILE_DATA",
        )
        .unwrap();
        assert_eq!(first, retry);
        assert!(
            ProfileDeletionService::delete_local(
                &reopened_state,
                &profile_id,
                created.revision + 1,
                "QUARANTINE_PROFILE_DATA",
            )
            .is_err()
        );

        drop(reopened_state);
        drop(reopened_profiles);
        let _ = fs::remove_dir_all(root);
    }
}
