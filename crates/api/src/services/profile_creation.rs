use buglogin_domain::{
    entities::{Profile, ProfileCreateInput, ProfileState, SyncMode},
    errors::Problem,
    ids::{ProfileId, ProxyId},
    permissions::Principal,
};
use sha2::{Digest, Sha256};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

pub struct ProfileCreationService;

impl ProfileCreationService {
    pub fn legacy_input(profile: &Profile, workspace_id: Option<&str>) -> ProfileCreateInput {
        ProfileCreateInput {
            id: Some(profile.id.as_str().to_owned()),
            name: profile.name.clone(),
            runtime: profile.runtime.clone(),
            runtime_version: profile.runtime_version.clone(),
            workspace_id: workspace_id.map(str::to_owned),
            proxy_id: profile.proxy_id.as_ref().map(|id| id.as_str().to_owned()),
            note: profile.note.clone(),
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
        let timestamp = OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .map_err(|_| problem("timestamp_invalid", "Timestamp is invalid", 500))?;
        let proxy_id = input
            .proxy_id
            .as_deref()
            .map(ProxyId::from_str)
            .transpose()
            .map_err(|_| problem("invalid_proxy_id", "Proxy ID is invalid", 400))?;
        Ok(Profile {
            id: profile_id,
            name: name.to_owned(),
            runtime: input.runtime,
            runtime_version: input.runtime_version,
            runtime_config: None,
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

#[cfg(test)]
mod tests {
    use buglogin_domain::{ids::WorkspaceId, permissions::Principal};

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
}
