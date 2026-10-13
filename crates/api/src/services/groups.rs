use std::collections::BTreeMap;

use buglogin_domain::{
    entities::Group,
    errors::Problem,
    ids::{GroupId, WorkspaceId},
    permissions::{Action, Capability, Grant, Principal, ResourceScope, is_allowed},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

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
pub struct ProfileGroupsReplaceInput {
    pub workspace_id: WorkspaceId,
    pub profile_id: buglogin_domain::ids::ProfileId,
    pub expected_revision: u64,
    pub group_ids: Vec<GroupId>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct GroupService {
    groups: BTreeMap<GroupId, GroupRecord>,
}

impl GroupService {
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
        if in_use {
            return Err(problem("group_in_use", "Group still has profiles", 409));
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
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .map_err(|_| problem("timestamp_invalid", "Timestamp is invalid", 500))
}

fn problem(code: &str, title: &str, status: u16) -> Problem {
    Problem::new(code, title, status, "groups")
}

#[cfg(test)]
mod tests {
    use buglogin_domain::{
        ids::{GroupId, WorkspaceId},
        permissions::{Principal, Role},
    };

    use super::{GroupCreateInput, GroupDeleteInput, GroupService, GroupUpdateInput};

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
}
