use std::collections::BTreeMap;

use buglogin_domain::{
    entities::Group,
    errors::Problem,
    grant::GroupShareGrant,
    ids::{GrantId, GroupId, MemberId, WorkspaceId},
    permissions::{Action, Capability, Effect, Grant, Principal, ResourceScope, is_allowed},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const CAPABILITY: &str = "CAP-PROF-003";

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GroupRecord {
    pub group: Group,
    pub workspace_id: WorkspaceId,
    pub revision: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GroupCreateInput {
    pub workspace_id: WorkspaceId,
    pub group_id: GroupId,
    pub name: String,
    pub appearance: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GroupUpdateInput {
    pub workspace_id: WorkspaceId,
    pub group_id: GroupId,
    pub expected_revision: u64,
    pub name: String,
    pub appearance: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GroupDeleteInput {
    pub workspace_id: WorkspaceId,
    pub group_id: GroupId,
    pub expected_revision: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GroupListInput {
    pub workspace_id: WorkspaceId,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GroupIdInput {
    pub workspace_id: WorkspaceId,
    pub group_id: GroupId,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GroupShareCreateInput {
    pub workspace_id: WorkspaceId,
    pub group_id: GroupId,
    pub member_id: MemberId,
    pub action: Action,
    pub effect: Effect,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GroupShareRevokeInput {
    pub workspace_id: WorkspaceId,
    pub group_id: GroupId,
    pub grant_id: GrantId,
    pub expected_revision: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileGroupsReplaceInput {
    pub workspace_id: WorkspaceId,
    pub profile_id: buglogin_domain::ids::ProfileId,
    pub expected_revision: u64,
    pub group_ids: Vec<GroupId>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct GroupService {
    #[serde(default)]
    groups: BTreeMap<GroupId, GroupRecord>,
    #[serde(default)]
    share_grants: BTreeMap<GrantId, GroupShareGrant>,
}

impl GroupService {
    pub(crate) fn remove_member_references(&mut self, workspace: &WorkspaceId, member: &MemberId) {
        self.share_grants.retain(|_, grant| {
            &grant.workspace_id != workspace || &grant.member_id != member
        });
    }

    pub(crate) fn has_workspace_records(&self, workspace_id: &WorkspaceId) -> bool {
        self.groups.values().any(|record| &record.workspace_id == workspace_id)
            || self.share_grants.values().any(|grant| &grant.workspace_id == workspace_id)
    }

    pub(crate) fn all_share_grants(&self) -> impl Iterator<Item = &GroupShareGrant> {
        self.share_grants.values()
    }

    pub fn import_records(
        &mut self,
        groups: Vec<GroupRecord>,
        grants: Vec<GroupShareGrant>,
    ) -> Result<(usize, usize, usize, usize), Problem> {
        let mut imported_groups = self.groups.clone();
        let mut imported_grants = self.share_grants.clone();
        let mut created_groups = 0;
        let mut reused_groups = 0;
        let mut created_grants = 0;
        let mut reused_grants = 0;
        for record in groups {
            if record.revision == 0
                || record.group.name.trim().is_empty()
                || record.group.created_at.trim().is_empty()
                || record.group.updated_at.trim().is_empty()
            {
                return Err(problem("group_invalid", "Imported group is invalid", 422));
            }
            match imported_groups.get(&record.group.id) {
                Some(existing) if existing == &record => reused_groups += 1,
                Some(_) => return Err(problem("group_exists", "Imported group conflicts", 409)),
                None => {
                    imported_groups.insert(record.group.id.clone(), record);
                    created_groups += 1;
                }
            }
        }
        for grant in grants {
            if grant.revision == 0
                || grant.created_at.trim().is_empty()
                || !matches!(
                    grant.action,
                    Action::Read | Action::Update | Action::Execute | Action::Share
                )
                || !matches!(grant.effect, Effect::Allow | Effect::Deny)
                || !imported_groups
                    .get(&grant.group_id)
                    .is_some_and(|group| group.workspace_id == grant.workspace_id)
            {
                return Err(problem(
                    "group_share_invalid",
                    "Imported group share is invalid",
                    422,
                ));
            }
            match imported_grants.get(&grant.id) {
                Some(existing) if existing == &grant => reused_grants += 1,
                Some(_) => {
                    return Err(problem(
                        "group_share_exists",
                        "Imported group share conflicts",
                        409,
                    ));
                }
                None => {
                    imported_grants.insert(grant.id.clone(), grant);
                    created_grants += 1;
                }
            }
        }
        self.groups = imported_groups;
        self.share_grants = imported_grants;
        Ok((created_groups, reused_groups, created_grants, reused_grants))
    }

    pub fn create_share(
        &mut self,
        principal: &Principal,
        grants: &[Grant],
        workspace_id: &WorkspaceId,
        input: GroupShareCreateInput,
    ) -> Result<GroupShareGrant, Problem> {
        let grant = self.prepare_share(principal, grants, workspace_id, &input)?;
        self.grant_share(principal, grants, workspace_id, grant)
    }

    pub fn prepare_share(
        &self,
        principal: &Principal,
        grants: &[Grant],
        workspace_id: &WorkspaceId,
        input: &GroupShareCreateInput,
    ) -> Result<GroupShareGrant, Problem> {
        self.validate_share_creation(principal, grants, workspace_id, input)?;
        let mut random = [0_u8; 16];
        getrandom::fill(&mut random).map_err(|_| {
            problem(
                "group_share_id_unavailable",
                "Could not create share ID",
                503,
            )
        })?;
        let id = GrantId::new(format!("grt_group_{}", hex(&random)))
            .map_err(|_| problem("group_share_id_invalid", "Could not create share ID", 500))?;
        let grant = GroupShareGrant {
            id,
            workspace_id: input.workspace_id.clone(),
            group_id: input.group_id.clone(),
            member_id: input.member_id.clone(),
            action: input.action,
            effect: input.effect,
            revision: 1,
            created_at: timestamp()?,
            revoked_at: None,
        };
        Ok(grant)
    }

    pub fn validate_share_creation(
        &self,
        principal: &Principal,
        grants: &[Grant],
        workspace_id: &WorkspaceId,
        input: &GroupShareCreateInput,
    ) -> Result<(), Problem> {
        ensure_workspace(workspace_id, &input.workspace_id)?;
        authorize(principal, grants, workspace_id, Action::Share)?;
        self.groups
            .get(&input.group_id)
            .filter(|record| &record.workspace_id == workspace_id)
            .ok_or_else(|| problem("group_not_found", "Group not found", 404))?;
        if !matches!(
            input.action,
            Action::Read | Action::Update | Action::Execute | Action::Share
        ) {
            return Err(problem(
                "group_share_action_invalid",
                "Action cannot be granted through a group share",
                400,
            ));
        }
        if self.share_grants.values().any(|existing| {
            existing.workspace_id == input.workspace_id
                && existing.group_id == input.group_id
                && existing.member_id == input.member_id
                && existing.action == input.action
                && existing.effect == input.effect
                && existing.revoked_at.is_none()
        }) {
            return Err(problem(
                "group_share_exists",
                "Group share already exists",
                409,
            ));
        }
        Ok(())
    }

    pub fn create(
        &mut self,
        principal: &Principal,
        grants: &[Grant],
        workspace_id: &WorkspaceId,
        input: GroupCreateInput,
    ) -> Result<GroupRecord, Problem> {
        ensure_workspace(workspace_id, &input.workspace_id)?;
        authorize(principal, grants, workspace_id, Action::Create)?;
        validate_group_fields(&input.name, input.appearance.as_ref())?;
        if self.groups.contains_key(&input.group_id) {
            return Err(problem("group_exists", "Group already exists", 409));
        }
        let timestamp = timestamp()?;
        let record = GroupRecord {
            group: Group {
                id: input.group_id.clone(),
                name: input.name.trim().to_owned(),
                appearance: input.appearance,
                created_at: timestamp.clone(),
                updated_at: timestamp,
            },
            workspace_id: workspace_id.clone(),
            revision: 1,
        };
        self.groups.insert(input.group_id, record.clone());
        Ok(record)
    }

    pub fn list(
        &self,
        principal: &Principal,
        grants: &[Grant],
        workspace_id: &WorkspaceId,
    ) -> Result<Vec<GroupRecord>, Problem> {
        authorize(principal, grants, workspace_id, Action::Read)?;
        Ok(self
            .groups
            .values()
            .filter(|record| &record.workspace_id == workspace_id)
            .cloned()
            .collect())
    }

    pub fn get(
        &self,
        principal: &Principal,
        grants: &[Grant],
        workspace_id: &WorkspaceId,
        group_id: &GroupId,
    ) -> Result<GroupRecord, Problem> {
        authorize(principal, grants, workspace_id, Action::Read)?;
        self.get_in_workspace(workspace_id, group_id)
    }

    pub fn get_in_workspace(
        &self,
        workspace_id: &WorkspaceId,
        group_id: &GroupId,
    ) -> Result<GroupRecord, Problem> {
        self.groups
            .get(group_id)
            .filter(|record| &record.workspace_id == workspace_id)
            .cloned()
            .ok_or_else(|| problem("group_not_found", "Group not found", 404))
    }

    pub fn validate_membership(
        &self,
        workspace_id: &WorkspaceId,
        group_ids: &[GroupId],
    ) -> Result<(), Problem> {
        let unique = group_ids.iter().collect::<std::collections::BTreeSet<_>>();
        if unique.len() != group_ids.len() {
            return Err(problem(
                "duplicate_profile_group",
                "Profile group list contains duplicates",
                400,
            ));
        }
        for group_id in group_ids {
            self.get_in_workspace(workspace_id, group_id)?;
        }
        Ok(())
    }

    pub fn grant_share(
        &mut self,
        principal: &Principal,
        grants: &[Grant],
        workspace_id: &WorkspaceId,
        grant: GroupShareGrant,
    ) -> Result<GroupShareGrant, Problem> {
        ensure_workspace(workspace_id, &grant.workspace_id)?;
        authorize(principal, grants, workspace_id, Action::Share)?;
        let group = self
            .groups
            .get(&grant.group_id)
            .filter(|record| &record.workspace_id == workspace_id)
            .ok_or_else(|| problem("group_not_found", "Group not found", 404))?;
        if grant.revision != 1 || grant.revoked_at.is_some() || grant.created_at.trim().is_empty() {
            return Err(problem(
                "group_share_invalid",
                "Group share grant is invalid",
                400,
            ));
        }
        let permitted_action = matches!(
            grant.action,
            Action::Read | Action::Update | Action::Execute | Action::Share
        );
        if !permitted_action {
            return Err(problem(
                "group_share_action_invalid",
                "Action cannot be granted through a group share",
                400,
            ));
        }
        if group.workspace_id != grant.workspace_id {
            return Err(problem("tenant_mismatch", "Group tenant mismatch", 403));
        }
        if self.share_grants.contains_key(&grant.id) {
            return Err(problem("group_share_exists", "Group share exists", 409));
        }
        if self.share_grants.values().any(|existing| {
            existing.workspace_id == grant.workspace_id
                && existing.group_id == grant.group_id
                && existing.member_id == grant.member_id
                && existing.action == grant.action
                && existing.effect == grant.effect
                && existing.revoked_at.is_none()
        }) {
            return Err(problem(
                "group_share_exists",
                "Group share already exists",
                409,
            ));
        }
        self.share_grants.insert(grant.id.clone(), grant.clone());
        Ok(grant)
    }

    pub fn list_share_grants(
        &self,
        principal: &Principal,
        grants: &[Grant],
        workspace_id: &WorkspaceId,
        group_id: Option<&GroupId>,
        member_id: Option<&MemberId>,
    ) -> Result<Vec<GroupShareGrant>, Problem> {
        authorize(principal, grants, workspace_id, Action::Read)?;
        Ok(self
            .share_grants
            .values()
            .filter(|grant| &grant.workspace_id == workspace_id)
            .filter(|grant| group_id.is_none_or(|id| &grant.group_id == id))
            .filter(|grant| member_id.is_none_or(|id| &grant.member_id == id))
            .cloned()
            .collect())
    }

    pub fn get_share_grant(
        &self,
        principal: &Principal,
        grants: &[Grant],
        workspace_id: &WorkspaceId,
        grant_id: &GrantId,
    ) -> Result<GroupShareGrant, Problem> {
        authorize(principal, grants, workspace_id, Action::Share)?;
        self.share_grants
            .get(grant_id)
            .filter(|grant| &grant.workspace_id == workspace_id)
            .cloned()
            .ok_or_else(|| problem("group_share_not_found", "Group share not found", 404))
    }

    pub fn revoke_share(
        &mut self,
        principal: &Principal,
        grants: &[Grant],
        workspace_id: &WorkspaceId,
        grant_id: &GrantId,
        expected_revision: u64,
    ) -> Result<GroupShareGrant, Problem> {
        self.validate_share_revocation(
            principal,
            grants,
            workspace_id,
            grant_id,
            expected_revision,
        )?;
        let grant = self
            .share_grants
            .get_mut(grant_id)
            .filter(|grant| &grant.workspace_id == workspace_id)
            .ok_or_else(|| problem("group_share_not_found", "Group share not found", 404))?;
        grant.revision = grant
            .revision
            .checked_add(1)
            .ok_or_else(|| problem("revision_exhausted", "Revision exhausted", 409))?;
        grant.revoked_at = Some(timestamp()?);
        Ok(grant.clone())
    }

    pub fn validate_share_revocation(
        &self,
        principal: &Principal,
        grants: &[Grant],
        workspace_id: &WorkspaceId,
        grant_id: &GrantId,
        expected_revision: u64,
    ) -> Result<GroupShareGrant, Problem> {
        authorize(principal, grants, workspace_id, Action::Share)?;
        let grant = self
            .share_grants
            .get(grant_id)
            .filter(|grant| &grant.workspace_id == workspace_id)
            .ok_or_else(|| problem("group_share_not_found", "Group share not found", 404))?;
        check_revision(grant.revision, expected_revision)?;
        if grant.revoked_at.is_some() {
            return Err(problem(
                "group_share_revoked",
                "Group share is already revoked",
                409,
            ));
        }
        Ok(grant.clone())
    }

    pub fn is_allowed_for_profile(
        &self,
        group_ids: &[GroupId],
        workspace_id: &WorkspaceId,
        member_id: &MemberId,
        action: Action,
    ) -> bool {
        !self.is_denied_for_profile(group_ids, workspace_id, member_id, action)
            && self.share_grants.values().any(|grant| {
                &grant.workspace_id == workspace_id
                    && group_ids.contains(&grant.group_id)
                    && &grant.member_id == member_id
                    && grant.action == action
                    && grant.effect == Effect::Allow
                    && grant.revoked_at.is_none()
            })
    }

    pub fn has_active_profile_shares(
        &self,
        group_ids: &[GroupId],
        workspace_id: &WorkspaceId,
    ) -> bool {
        self.share_grants.values().any(|grant| {
            &grant.workspace_id == workspace_id
                && group_ids.contains(&grant.group_id)
                && grant.effect == Effect::Allow
                && matches!(grant.action, Action::Read | Action::Execute)
                && grant.revoked_at.is_none()
        })
    }

    pub fn is_denied_for_profile(
        &self,
        group_ids: &[GroupId],
        workspace_id: &WorkspaceId,
        member_id: &MemberId,
        action: Action,
    ) -> bool {
        if self.validate_membership(workspace_id, group_ids).is_err() {
            return true;
        }
        self.share_grants.values().any(|grant| {
            &grant.workspace_id == workspace_id
                && group_ids.contains(&grant.group_id)
                && &grant.member_id == member_id
                && grant.action == action
                && grant.effect == Effect::Deny
                && grant.revoked_at.is_none()
        })
    }

    pub fn update(
        &mut self,
        principal: &Principal,
        grants: &[Grant],
        workspace_id: &WorkspaceId,
        group_id: &GroupId,
        input: GroupUpdateInput,
    ) -> Result<GroupRecord, Problem> {
        ensure_workspace(workspace_id, &input.workspace_id)?;
        if group_id != &input.group_id {
            return Err(problem(
                "group_scope_mismatch",
                "Group scope does not match",
                400,
            ));
        }
        authorize(principal, grants, workspace_id, Action::Update)?;
        validate_group_fields(&input.name, input.appearance.as_ref())?;
        let record = self
            .groups
            .get_mut(group_id)
            .filter(|record| &record.workspace_id == workspace_id)
            .ok_or_else(|| problem("group_not_found", "Group not found", 404))?;
        check_revision(record.revision, input.expected_revision)?;
        record.revision = record
            .revision
            .checked_add(1)
            .ok_or_else(|| problem("revision_exhausted", "Revision exhausted", 409))?;
        record.group.name = input.name.trim().to_owned();
        record.group.appearance = input.appearance;
        record.group.updated_at = timestamp()?;
        Ok(record.clone())
    }

    pub fn delete(
        &mut self,
        principal: &Principal,
        grants: &[Grant],
        workspace_id: &WorkspaceId,
        group_id: &GroupId,
        input: GroupDeleteInput,
        in_use: bool,
    ) -> Result<GroupRecord, Problem> {
        ensure_workspace(workspace_id, &input.workspace_id)?;
        if group_id != &input.group_id {
            return Err(problem(
                "group_scope_mismatch",
                "Group scope does not match",
                400,
            ));
        }
        authorize(principal, grants, workspace_id, Action::Delete)?;
        let record = self
            .groups
            .get(group_id)
            .filter(|record| &record.workspace_id == workspace_id)
            .ok_or_else(|| problem("group_not_found", "Group not found", 404))?;
        check_revision(record.revision, input.expected_revision)?;
        let has_active_shares = self.share_grants.values().any(|grant| {
            &grant.workspace_id == workspace_id
                && &grant.group_id == group_id
                && grant.revoked_at.is_none()
        });
        let has_share_history = self
            .share_grants
            .values()
            .any(|grant| &grant.workspace_id == workspace_id && &grant.group_id == group_id);
        if in_use {
            return Err(problem("group_in_use", "Group still has profiles", 409));
        }
        if has_active_shares {
            return Err(problem(
                "group_has_active_shares",
                "Revoke active group shares before deleting this group",
                409,
            ));
        }
        if has_share_history {
            return Err(problem(
                "group_share_history_retained",
                "Group cannot be deleted while share history is retained",
                409,
            ));
        }
        self.groups
            .remove(group_id)
            .ok_or_else(|| problem("group_not_found", "Group not found", 404))
    }
}

fn validate_group_fields(name: &str, appearance: Option<&Value>) -> Result<(), Problem> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 120 {
        return Err(problem("invalid_group_name", "Group name is invalid", 400));
    }
    if appearance.is_some_and(|value| !value.is_object()) {
        return Err(problem(
            "invalid_group_appearance",
            "Group appearance must be an object",
            400,
        ));
    }
    Ok(())
}

fn authorize(
    principal: &Principal,
    grants: &[Grant],
    workspace_id: &WorkspaceId,
    action: Action,
) -> Result<(), Problem> {
    let capability = Capability::new(CAPABILITY)
        .map_err(|_| problem("invalid_capability", "Invalid group capability", 500))?;
    let scope = ResourceScope::workspace(workspace_id.clone());
    if is_allowed(principal, &capability, action, &scope, grants) {
        Ok(())
    } else {
        Err(problem("group_forbidden", "Group access denied", 403))
    }
}

fn ensure_workspace(expected: &WorkspaceId, actual: &WorkspaceId) -> Result<(), Problem> {
    if expected == actual {
        Ok(())
    } else {
        Err(problem(
            "workspace_scope_mismatch",
            "Workspace scope does not match",
            403,
        ))
    }
}

fn check_revision(actual: u64, expected: u64) -> Result<(), Problem> {
    if actual == expected {
        Ok(())
    } else {
        Err(problem("revision_conflict", "Revision conflict", 409)
            .with_field("expectedRevision", expected)
            .with_field("actualRevision", actual))
    }
}

fn timestamp() -> Result<String, Problem> {
    crate::clock::rfc3339("groups")
}

fn problem(code: &str, title: &str, status: u16) -> Problem {
    Problem::new(code, title, status, "groups")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use buglogin_domain::{
        grant::GroupShareGrant,
        ids::{GrantId, GroupId, MemberId, WorkspaceId},
        permissions::{Action, Effect, Principal, Role},
    };

    use super::{
        GroupCreateInput, GroupDeleteInput, GroupRecord, GroupService, GroupShareCreateInput,
        GroupUpdateInput,
    };
    use buglogin_domain::entities::Group;

    fn principal(workspaces: &[&str]) -> Principal {
        Principal {
            workspace_ids: workspaces
                .iter()
                .map(|id| WorkspaceId::from_str(id).unwrap())
                .collect(),
            roles: [Role::WorkspaceAdmin].into_iter().collect(),
            ..Principal::default()
        }
    }

    fn create_input(id: &str, name: &str) -> GroupCreateInput {
        GroupCreateInput {
            workspace_id: WorkspaceId::from_str("wsp_alpha").unwrap(),
            group_id: GroupId::from_str(id).unwrap(),
            name: name.into(),
            appearance: None,
        }
    }

    fn share_grant(id: &str, group_id: &str, member_id: &str, action: Action) -> GroupShareGrant {
        GroupShareGrant {
            id: GrantId::from_str(id).unwrap(),
            workspace_id: WorkspaceId::from_str("wsp_alpha").unwrap(),
            group_id: GroupId::from_str(group_id).unwrap(),
            member_id: MemberId::from_str(member_id).unwrap(),
            action,
            effect: Effect::Allow,
            revision: 1,
            created_at: "2026-10-14T00:00:00Z".into(),
            revoked_at: None,
        }
    }

    #[test]
    fn groups_are_workspace_scoped_and_authorized() {
        let mut service = GroupService::default();
        let admin = principal(&["wsp_alpha", "wsp_beta"]);
        let alpha = WorkspaceId::from_str("wsp_alpha").unwrap();
        let beta = WorkspaceId::from_str("wsp_beta").unwrap();
        let record = service
            .create(&admin, &[], &alpha, create_input("grp_shared", "Alpha"))
            .unwrap();

        assert_eq!(service.list(&admin, &[], &alpha).unwrap().len(), 1);
        assert!(service.list(&admin, &[], &beta).unwrap().is_empty());
        assert_eq!(
            service
                .get_in_workspace(&beta, &record.group.id)
                .unwrap_err()
                .code,
            "group_not_found"
        );
        let mut denied_input = create_input("grp_denied", "Denied");
        denied_input.workspace_id = beta.clone();
        assert_eq!(
            service
                .create(&principal(&["wsp_alpha"]), &[], &beta, denied_input,)
                .unwrap_err()
                .code,
            "group_forbidden"
        );
    }

    #[test]
    fn group_revision_and_delete_in_use_are_enforced() {
        let mut service = GroupService::default();
        let admin = principal(&["wsp_alpha"]);
        let workspace = WorkspaceId::from_str("wsp_alpha").unwrap();
        let created = service
            .create(&admin, &[], &workspace, create_input("grp_alpha", "Alpha"))
            .unwrap();
        let group_id = created.group.id.clone();

        let updated = service
            .update(
                &admin,
                &[],
                &workspace,
                &group_id,
                GroupUpdateInput {
                    workspace_id: workspace.clone(),
                    group_id: group_id.clone(),
                    expected_revision: 1,
                    name: "Renamed".into(),
                    appearance: None,
                },
            )
            .unwrap();
        assert_eq!(updated.revision, 2);
        assert_eq!(updated.group.name, "Renamed");
        assert_eq!(
            service
                .update(
                    &admin,
                    &[],
                    &workspace,
                    &group_id,
                    GroupUpdateInput {
                        workspace_id: workspace.clone(),
                        group_id: group_id.clone(),
                        expected_revision: 1,
                        name: "Stale".into(),
                        appearance: None,
                    },
                )
                .unwrap_err()
                .code,
            "revision_conflict"
        );
        assert_eq!(
            service
                .delete(
                    &admin,
                    &[],
                    &workspace,
                    &group_id,
                    GroupDeleteInput {
                        workspace_id: workspace.clone(),
                        group_id: group_id.clone(),
                        expected_revision: 2,
                    },
                    true,
                )
                .unwrap_err()
                .code,
            "group_in_use"
        );
        assert!(
            service
                .delete(
                    &admin,
                    &[],
                    &workspace,
                    &group_id,
                    GroupDeleteInput {
                        workspace_id: workspace.clone(),
                        group_id: group_id.clone(),
                        expected_revision: 2,
                    },
                    false,
                )
                .is_ok()
        );
    }

    #[test]
    fn group_shares_follow_current_group_membership_and_revoke_is_durable() {
        let mut service = GroupService::default();
        let admin = principal(&["wsp_alpha"]);
        let workspace = WorkspaceId::from_str("wsp_alpha").unwrap();
        let group = GroupId::from_str("grp_alpha").unwrap();
        service
            .create(&admin, &[], &workspace, create_input("grp_alpha", "Alpha"))
            .unwrap();
        let grant = share_grant(
            "grt_group_share",
            "grp_alpha",
            "mem_recipient",
            Action::Execute,
        );
        service
            .grant_share(&admin, &[], &workspace, grant.clone())
            .unwrap();
        assert_eq!(
            service
                .validate_share_creation(
                    &admin,
                    &[],
                    &workspace,
                    &GroupShareCreateInput {
                        workspace_id: workspace.clone(),
                        group_id: group.clone(),
                        member_id: grant.member_id.clone(),
                        action: grant.action,
                        effect: grant.effect,
                    },
                )
                .unwrap_err()
                .code,
            "group_share_exists"
        );
        assert_eq!(
            service
                .validate_share_revocation(&admin, &[], &workspace, &grant.id, 0)
                .unwrap_err()
                .code,
            "revision_conflict"
        );

        assert!(service.is_allowed_for_profile(
            std::slice::from_ref(&group),
            &workspace,
            &grant.member_id,
            Action::Execute
        ));
        assert!(!service.is_allowed_for_profile(
            &[],
            &workspace,
            &grant.member_id,
            Action::Execute
        ));
        assert!(!service.is_allowed_for_profile(
            std::slice::from_ref(&group),
            &workspace,
            &grant.member_id,
            Action::Update
        ));

        let snapshot = serde_json::to_vec(&service).unwrap();
        let mut reopened: GroupService = serde_json::from_slice(&snapshot).unwrap();
        let revoked = reopened
            .revoke_share(&admin, &[], &workspace, &grant.id, 1)
            .unwrap();
        assert_eq!(revoked.revision, 2);
        assert!(revoked.revoked_at.is_some());
        assert!(!reopened.is_allowed_for_profile(
            std::slice::from_ref(&group),
            &workspace,
            &grant.member_id,
            Action::Execute
        ));
    }

    #[test]
    fn group_share_cannot_cross_workspace_or_group_removal() {
        let mut service = GroupService::default();
        let admin = principal(&["wsp_alpha", "wsp_beta"]);
        let alpha = WorkspaceId::from_str("wsp_alpha").unwrap();
        let beta = WorkspaceId::from_str("wsp_beta").unwrap();
        let group = GroupId::from_str("grp_alpha").unwrap();
        let grant = share_grant(
            "grt_group_share",
            "grp_alpha",
            "mem_recipient",
            Action::Read,
        );
        assert_eq!(
            service
                .grant_share(&admin, &[], &beta, grant.clone())
                .unwrap_err()
                .code,
            "workspace_scope_mismatch"
        );
        service
            .create(&admin, &[], &alpha, create_input("grp_alpha", "Alpha"))
            .unwrap();
        service
            .grant_share(&admin, &[], &alpha, grant.clone())
            .unwrap();
        assert!(!service.is_allowed_for_profile(
            std::slice::from_ref(&group),
            &beta,
            &grant.member_id,
            Action::Read
        ));
        assert_eq!(
            service
                .delete(
                    &admin,
                    &[],
                    &alpha,
                    &group,
                    GroupDeleteInput {
                        workspace_id: alpha.clone(),
                        group_id: group.clone(),
                        expected_revision: 1,
                    },
                    false,
                )
                .unwrap_err()
                .code,
            "group_has_active_shares"
        );
    }

    #[test]
    fn revoked_group_share_history_blocks_group_removal() {
        let mut service = GroupService::default();
        let admin = principal(&["wsp_alpha"]);
        let workspace = WorkspaceId::from_str("wsp_alpha").unwrap();
        let group = GroupId::from_str("grp_alpha").unwrap();
        service
            .create(&admin, &[], &workspace, create_input("grp_alpha", "Alpha"))
            .unwrap();
        let grant = share_grant(
            "grt_group_share",
            "grp_alpha",
            "mem_recipient",
            Action::Read,
        );
        service
            .grant_share(&admin, &[], &workspace, grant.clone())
            .unwrap();
        service
            .revoke_share(&admin, &[], &workspace, &grant.id, 1)
            .unwrap();

        assert_eq!(
            service
                .delete(
                    &admin,
                    &[],
                    &workspace,
                    &group,
                    GroupDeleteInput {
                        workspace_id: workspace.clone(),
                        group_id: group.clone(),
                        expected_revision: 1,
                    },
                    false,
                )
                .unwrap_err()
                .code,
            "group_share_history_retained"
        );
    }

    #[test]
    fn group_import_is_atomic_when_a_later_grant_is_invalid() {
        let mut service = GroupService::default();
        let workspace = WorkspaceId::from_str("wsp_alpha").unwrap();
        let group = GroupRecord {
            group: Group {
                id: GroupId::from_str("grp_imported").unwrap(),
                name: "Imported".into(),
                appearance: None,
                created_at: "2026-10-14T00:00:00Z".into(),
                updated_at: "2026-10-14T00:00:00Z".into(),
            },
            workspace_id: workspace.clone(),
            revision: 1,
        };
        let invalid_grant = GroupShareGrant {
            id: GrantId::from_str("grt_invalid_import").unwrap(),
            workspace_id: workspace,
            group_id: GroupId::from_str("grp_missing").unwrap(),
            member_id: MemberId::from_str("mem_recipient").unwrap(),
            action: Action::Read,
            effect: Effect::Allow,
            revision: 1,
            created_at: "2026-10-14T00:00:00Z".into(),
            revoked_at: None,
        };

        assert_eq!(
            service
                .import_records(vec![group], vec![invalid_grant])
                .unwrap_err()
                .code,
            "group_share_invalid"
        );
        assert!(service.groups.is_empty());
        assert!(service.share_grants.is_empty());
    }
}
