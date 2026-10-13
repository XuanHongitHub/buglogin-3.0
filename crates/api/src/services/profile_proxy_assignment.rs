use std::collections::BTreeMap;

use buglogin_domain::{
    entities::{Profile, ProfileState},
    errors::Problem,
    ids::{ProfileId, ProxyId, WorkspaceId},
};
use serde::{Deserialize, Serialize};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileProxyAssignment {
    pub profile_id: ProfileId,
    pub workspace_id: WorkspaceId,
    pub expected_profile_revision: u64,
    pub proxy_id: Option<ProxyId>,
    pub requested_at: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileProxyAssignmentSetInput {
    pub workspace_id: WorkspaceId,
    pub profile_id: ProfileId,
    pub expected_profile_revision: u64,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub proxy_id: Option<ProxyId>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileProxyAssignmentGetInput {
    pub workspace_id: WorkspaceId,
    pub profile_id: ProfileId,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(transparent)]
pub struct ProfileProxyAssignmentService(BTreeMap<ProfileId, ProfileProxyAssignment>);

impl ProfileProxyAssignmentService {
    pub fn validate_change(
        profile: &Profile,
        actual_revision: u64,
        input: &ProfileProxyAssignmentSetInput,
    ) -> Result<(), Problem> {
        if actual_revision != input.expected_profile_revision {
            return Err(
                problem("revision_conflict", "Profile revision conflict", 409)
                    .with_field("expectedRevision", input.expected_profile_revision)
                    .with_field("actualRevision", actual_revision),
            );
        }
        if profile.archived || profile.state == ProfileState::Running {
            return Err(problem(
                "profile_proxy_assignment_not_allowed",
                "Proxy assignment requires a stopped active profile",
                409,
            ));
        }
        if profile.proxy_id == input.proxy_id {
            return Err(problem(
                "profile_proxy_assignment_unchanged",
                "The profile already uses this proxy",
                409,
            ));
        }
        Ok(())
    }

    pub fn get(
        &self,
        workspace_id: &WorkspaceId,
        profile_id: &ProfileId,
    ) -> Result<ProfileProxyAssignment, Problem> {
        self.0
            .get(profile_id)
            .filter(|assignment| &assignment.workspace_id == workspace_id)
            .cloned()
            .ok_or_else(|| {
                problem(
                    "profile_proxy_assignment_not_found",
                    "Profile proxy assignment not found",
                    404,
                )
            })
    }

    pub fn get_optional(
        &self,
        workspace_id: &WorkspaceId,
        profile_id: &ProfileId,
    ) -> Option<ProfileProxyAssignment> {
        self.0
            .get(profile_id)
            .filter(|assignment| &assignment.workspace_id == workspace_id)
            .cloned()
    }

    pub fn set(
        &mut self,
        workspace_id: &WorkspaceId,
        input: ProfileProxyAssignmentSetInput,
    ) -> Result<ProfileProxyAssignment, Problem> {
        if input.workspace_id != *workspace_id {
            return Err(problem(
                "workspace_scope_mismatch",
                "Workspace scope does not match",
                403,
            ));
        }
        if input.expected_profile_revision == 0 {
            return Err(problem(
                "profile_revision_invalid",
                "Profile revision must be positive",
                400,
            ));
        }
        let assignment = ProfileProxyAssignment {
            profile_id: input.profile_id.clone(),
            workspace_id: workspace_id.clone(),
            expected_profile_revision: input.expected_profile_revision,
            proxy_id: input.proxy_id,
            requested_at: timestamp()?,
        };
        self.0.insert(input.profile_id, assignment.clone());
        Ok(assignment)
    }

    pub fn consume(
        &mut self,
        workspace_id: &WorkspaceId,
        profile_id: &ProfileId,
        expected_profile_revision: u64,
    ) -> Result<ProfileProxyAssignment, Problem> {
        let assignment = self
            .0
            .get(profile_id)
            .filter(|assignment| {
                &assignment.workspace_id == workspace_id
                    && assignment.expected_profile_revision == expected_profile_revision
            })
            .cloned()
            .ok_or_else(|| {
                problem(
                    "profile_proxy_assignment_conflict",
                    "Pending profile proxy assignment changed",
                    409,
                )
            })?;
        self.0.remove(profile_id);
        Ok(assignment)
    }
}

fn deserialize_required_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

fn timestamp() -> Result<String, Problem> {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .map_err(|_| problem("clock_unavailable", "Clock unavailable", 500))
}

fn problem(code: &'static str, title: &'static str, status: u16) -> Problem {
    Problem::new(code, title, status, "profile")
}

#[cfg(test)]
mod tests {
    use super::*;
    use buglogin_domain::entities::{Runtime, SyncMode};

    fn profile(state: ProfileState, archived: bool, proxy_id: Option<ProxyId>) -> Profile {
        Profile {
            id: ProfileId::from_str("prf_assignment_validation").unwrap(),
            name: "assignment validation".into(),
            runtime: Runtime::Wayfern,
            runtime_version: None,
            runtime_config: None,
            state,
            archived,
            fingerprint: None,
            note: None,
            proxy_id,
            vpn_id: None,
            group_ids: Vec::new(),
            tags: Vec::new(),
            extension_group_id: None,
            sync_mode: Some(SyncMode::Managed),
            created_at: "2026-10-13T00:00:00Z".into(),
            updated_at: "2026-10-13T00:00:00Z".into(),
        }
    }

    fn change_input(proxy_id: Option<ProxyId>) -> ProfileProxyAssignmentSetInput {
        ProfileProxyAssignmentSetInput {
            workspace_id: WorkspaceId::from_str("ws_assignment_validation").unwrap(),
            profile_id: ProfileId::from_str("prf_assignment_validation").unwrap(),
            expected_profile_revision: 7,
            proxy_id,
        }
    }

    #[test]
    fn assignment_change_rules_are_canonical_for_all_adapters() {
        let proxy = ProxyId::from_str("pxy_assignment_validation").unwrap();
        let input = change_input(Some(proxy.clone()));
        let ready = profile(ProfileState::Ready, false, None);
        assert!(ProfileProxyAssignmentService::validate_change(&ready, 7, &input).is_ok());
        assert_eq!(
            ProfileProxyAssignmentService::validate_change(&ready, 6, &input)
                .unwrap_err()
                .code,
            "revision_conflict"
        );
        assert_eq!(
            ProfileProxyAssignmentService::validate_change(
                &profile(ProfileState::Running, false, None),
                7,
                &input,
            )
            .unwrap_err()
            .code,
            "profile_proxy_assignment_not_allowed"
        );
        assert_eq!(
            ProfileProxyAssignmentService::validate_change(
                &profile(ProfileState::Ready, true, None),
                7,
                &input,
            )
            .unwrap_err()
            .code,
            "profile_proxy_assignment_not_allowed"
        );
        assert_eq!(
            ProfileProxyAssignmentService::validate_change(
                &profile(ProfileState::Ready, false, Some(proxy)),
                7,
                &input,
            )
            .unwrap_err()
            .code,
            "profile_proxy_assignment_unchanged"
        );
    }

    #[test]
    fn assignment_is_workspace_scoped_and_consumed_by_matching_revision() {
        let workspace = WorkspaceId::from_str("ws_proxy_assignment").unwrap();
        let other = WorkspaceId::from_str("ws_proxy_other").unwrap();
        let profile = ProfileId::from_str("prf_proxy_assignment").unwrap();
        let proxy = ProxyId::from_str("pxy_proxy_assignment").unwrap();
        let mut service = ProfileProxyAssignmentService::default();
        let input = ProfileProxyAssignmentSetInput {
            workspace_id: workspace.clone(),
            profile_id: profile.clone(),
            expected_profile_revision: 7,
            proxy_id: Some(proxy.clone()),
        };
        let assignment = service.set(&workspace, input).unwrap();
        assert_eq!(assignment.proxy_id, Some(proxy));
        assert!(service.get(&other, &profile).is_err());
        assert!(service.consume(&workspace, &profile, 6).is_err());
        assert_eq!(
            service.consume(&workspace, &profile, 7).unwrap(),
            assignment
        );
        assert!(service.get_optional(&workspace, &profile).is_none());
    }

    #[test]
    fn assignment_requires_explicit_proxy_id_field_even_when_clearing() {
        let missing = serde_json::from_str::<ProfileProxyAssignmentSetInput>(
            r#"{"workspaceId":"ws_proxy_assignment","profileId":"prf_proxy_assignment","expectedProfileRevision":1}"#,
        );
        assert!(missing.is_err());
        let clearing = serde_json::from_str::<ProfileProxyAssignmentSetInput>(
            r#"{"workspaceId":"ws_proxy_assignment","profileId":"prf_proxy_assignment","expectedProfileRevision":1,"proxyId":null}"#,
        )
        .unwrap();
        assert_eq!(clearing.proxy_id, None);
    }
}
