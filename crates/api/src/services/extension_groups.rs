use std::collections::BTreeMap;

use buglogin_domain::{
    entities::{ExtensionGroup, SyncMode},
    errors::Problem,
    ids::{ExtensionGroupId, ExtensionId, WorkspaceId},
    permissions::{Action, Capability, Grant, Principal, ResourceScope, is_allowed},
};
use serde::{Deserialize, Serialize};

const CAPABILITY: &str = "CAP-EXT-002";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExtensionGroupRecord {
    pub group: ExtensionGroup,
    pub workspace_id: WorkspaceId,
    pub revision: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExtensionGroupListInput {
    pub workspace_id: WorkspaceId,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExtensionGroupIdInput {
    pub workspace_id: WorkspaceId,
    pub extension_group_id: ExtensionGroupId,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExtensionGroupCreateInput {
    pub workspace_id: WorkspaceId,
    pub extension_group_id: ExtensionGroupId,
    pub name: String,
    #[serde(default)]
    pub extension_ids: Vec<ExtensionId>,
    pub sync_mode: Option<SyncMode>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExtensionGroupUpdateInput {
    pub workspace_id: WorkspaceId,
    pub extension_group_id: ExtensionGroupId,
    pub expected_revision: u64,
    pub name: String,
    #[serde(default)]
    pub extension_ids: Vec<ExtensionId>,
    pub sync_mode: Option<SyncMode>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExtensionGroupDeleteInput {
    pub workspace_id: WorkspaceId,
    pub extension_group_id: ExtensionGroupId,
    pub expected_revision: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileExtensionGroupSetInput {
    pub workspace_id: WorkspaceId,
    pub profile_id: buglogin_domain::ids::ProfileId,
    pub expected_revision: u64,
    pub extension_group_id: Option<ExtensionGroupId>,
}

#[derive(Clone, Default, Deserialize, Serialize)]
pub struct ExtensionGroupService {
    #[serde(default)]
    groups: BTreeMap<ExtensionGroupId, ExtensionGroupRecord>,
}

impl ExtensionGroupService {
    pub fn create(
        &mut self,
        principal: &Principal,
        grants: &[Grant],
        workspace_id: &WorkspaceId,
        input: ExtensionGroupCreateInput,
    ) -> Result<ExtensionGroupRecord, Problem> {
        ensure_workspace(workspace_id, &input.workspace_id)?;
        authorize(principal, grants, workspace_id, Action::Create)?;
        validate_fields(&input.name, &input.extension_ids)?;
        if self.groups.contains_key(&input.extension_group_id) {
            return Err(problem(
                "extension_group_exists",
                "Extension group already exists",
                409,
            ));
        }
        let now = crate::clock::rfc3339("extension-group")?;
        let record = ExtensionGroupRecord {
            group: ExtensionGroup {
                id: input.extension_group_id.clone(),
                name: input.name.trim().to_owned(),
                extension_ids: input.extension_ids,
                sync_mode: input.sync_mode,
                created_at: now.clone(),
                updated_at: now,
            },
            workspace_id: workspace_id.clone(),
            revision: 1,
        };
        self.groups.insert(input.extension_group_id, record.clone());
        Ok(record)
    }

    pub fn list(
        &self,
        principal: &Principal,
        grants: &[Grant],
        workspace_id: &WorkspaceId,
    ) -> Result<Vec<ExtensionGroupRecord>, Problem> {
        authorize(principal, grants, workspace_id, Action::Read)?;
        Ok(self
            .groups
            .values()
            .filter(|item| &item.workspace_id == workspace_id)
            .cloned()
            .collect())
    }

    pub fn get(
        &self,
        principal: &Principal,
        grants: &[Grant],
        workspace_id: &WorkspaceId,
        id: &ExtensionGroupId,
    ) -> Result<ExtensionGroupRecord, Problem> {
        authorize(principal, grants, workspace_id, Action::Read)?;
        self.get_in_workspace(workspace_id, id)
    }

    pub fn get_in_workspace(
        &self,
        workspace_id: &WorkspaceId,
        id: &ExtensionGroupId,
    ) -> Result<ExtensionGroupRecord, Problem> {
        self.groups
            .get(id)
            .filter(|record| &record.workspace_id == workspace_id)
            .cloned()
            .ok_or_else(|| {
                problem(
                    "extension_group_not_found",
                    "Extension group not found",
                    404,
                )
            })
    }

    pub fn update(
        &mut self,
        principal: &Principal,
        grants: &[Grant],
        workspace_id: &WorkspaceId,
        input: ExtensionGroupUpdateInput,
    ) -> Result<ExtensionGroupRecord, Problem> {
        ensure_workspace(workspace_id, &input.workspace_id)?;
        authorize(principal, grants, workspace_id, Action::Update)?;
        validate_fields(&input.name, &input.extension_ids)?;
        let record = self
            .groups
            .get_mut(&input.extension_group_id)
            .filter(|record| &record.workspace_id == workspace_id)
            .ok_or_else(|| {
                problem(
                    "extension_group_not_found",
                    "Extension group not found",
                    404,
                )
            })?;
        check_revision(record.revision, input.expected_revision)?;
        record.revision = record
            .revision
            .checked_add(1)
            .ok_or_else(|| problem("revision_exhausted", "Revision exhausted", 409))?;
        record.group.name = input.name.trim().to_owned();
        record.group.extension_ids = input.extension_ids;
        record.group.sync_mode = input.sync_mode;
        record.group.updated_at = crate::clock::rfc3339("extension-group")?;
        Ok(record.clone())
    }

    pub fn delete(
        &mut self,
        principal: &Principal,
        grants: &[Grant],
        workspace_id: &WorkspaceId,
        input: ExtensionGroupDeleteInput,
        in_use: bool,
    ) -> Result<ExtensionGroupRecord, Problem> {
        ensure_workspace(workspace_id, &input.workspace_id)?;
        authorize(principal, grants, workspace_id, Action::Delete)?;
        let record = self
            .groups
            .get(&input.extension_group_id)
            .filter(|record| &record.workspace_id == workspace_id)
            .ok_or_else(|| {
                problem(
                    "extension_group_not_found",
                    "Extension group not found",
                    404,
                )
            })?;
        check_revision(record.revision, input.expected_revision)?;
        if in_use {
            return Err(problem(
                "extension_group_in_use",
                "Extension group is assigned to profiles",
                409,
            ));
        }
        self.groups
            .remove(&input.extension_group_id)
            .ok_or_else(|| {
                problem(
                    "extension_group_not_found",
                    "Extension group not found",
                    404,
                )
            })
    }
}

fn validate_fields(name: &str, ids: &[ExtensionId]) -> Result<(), Problem> {
    let unique = ids.iter().collect::<std::collections::BTreeSet<_>>();
    if name.trim().is_empty() || name.chars().count() > 120 || unique.len() != ids.len() {
        return Err(problem(
            "extension_group_invalid",
            "Extension group fields are invalid",
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
        .map_err(|_| problem("invalid_capability", "Invalid extension capability", 500))?;
    if is_allowed(
        principal,
        &capability,
        action,
        &ResourceScope::workspace(workspace_id.clone()),
        grants,
    ) {
        Ok(())
    } else {
        Err(problem(
            "extension_group_forbidden",
            "Extension group access denied",
            403,
        ))
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

fn problem(code: &'static str, title: &'static str, status: u16) -> Problem {
    Problem::new(code, title, status, "extension-groups")
}

#[cfg(test)]
mod tests {
    use super::*;
    use buglogin_domain::permissions::Role;

    fn workspace(id: &str) -> WorkspaceId {
        WorkspaceId::from_str(id).unwrap()
    }

    fn admin() -> Principal {
        Principal {
            workspace_ids: [workspace("wsp_alpha"), workspace("wsp_beta")].into(),
            roles: [Role::WorkspaceAdmin].into(),
            ..Principal::default()
        }
    }

    fn create_input(workspace_id: &str, id: &str, name: &str) -> ExtensionGroupCreateInput {
        ExtensionGroupCreateInput {
            workspace_id: workspace(workspace_id),
            extension_group_id: ExtensionGroupId::from_str(id).unwrap(),
            name: name.into(),
            extension_ids: vec![],
            sync_mode: Some(SyncMode::Local),
        }
    }

    #[test]
    fn extension_groups_are_workspace_scoped_versioned_and_persistent() {
        let admin = admin();
        let alpha = workspace("wsp_alpha");
        let beta = workspace("wsp_beta");
        let id = ExtensionGroupId::from_str("extgrp_alpha").unwrap();
        let mut service = ExtensionGroupService::default();
        let created = service
            .create(
                &admin,
                &[],
                &alpha,
                create_input("wsp_alpha", "extgrp_alpha", "Alpha"),
            )
            .unwrap();
        assert_eq!(created.revision, 1);
        assert_eq!(service.list(&admin, &[], &alpha).unwrap().len(), 1);
        assert!(service.list(&admin, &[], &beta).unwrap().is_empty());
        assert_eq!(
            service.get(&admin, &[], &beta, &id).unwrap_err().code,
            "extension_group_not_found"
        );

        let updated = service
            .update(
                &admin,
                &[],
                &alpha,
                ExtensionGroupUpdateInput {
                    workspace_id: alpha.clone(),
                    extension_group_id: id.clone(),
                    expected_revision: 1,
                    name: "Renamed".into(),
                    extension_ids: vec![],
                    sync_mode: Some(SyncMode::Local),
                },
            )
            .unwrap();
        assert_eq!(updated.revision, 2);
        assert_eq!(
            service
                .update(
                    &admin,
                    &[],
                    &alpha,
                    ExtensionGroupUpdateInput {
                        workspace_id: alpha.clone(),
                        extension_group_id: id.clone(),
                        expected_revision: 1,
                        name: "Stale".into(),
                        extension_ids: vec![],
                        sync_mode: None,
                    }
                )
                .unwrap_err()
                .code,
            "revision_conflict"
        );

        let snapshot = serde_json::to_vec(&service).unwrap();
        let mut reopened: ExtensionGroupService = serde_json::from_slice(&snapshot).unwrap();
        assert_eq!(
            reopened.get(&admin, &[], &alpha, &id).unwrap().group.name,
            "Renamed"
        );
        assert_eq!(
            reopened
                .delete(
                    &admin,
                    &[],
                    &alpha,
                    ExtensionGroupDeleteInput {
                        workspace_id: alpha.clone(),
                        extension_group_id: id,
                        expected_revision: 2,
                    },
                    false
                )
                .unwrap()
                .revision,
            2
        );
    }

    #[test]
    fn extension_groups_reject_workspace_mismatch_and_invalid_names() {
        let admin = admin();
        let alpha = workspace("wsp_alpha");
        let beta = workspace("wsp_beta");
        let mut service = ExtensionGroupService::default();
        assert_eq!(
            service
                .create(
                    &admin,
                    &[],
                    &alpha,
                    create_input("wsp_beta", "extgrp_beta", "Beta")
                )
                .unwrap_err()
                .code,
            "workspace_scope_mismatch"
        );
        assert_eq!(
            service
                .create(
                    &admin,
                    &[],
                    &alpha,
                    create_input("wsp_alpha", "extgrp_blank", "   ")
                )
                .unwrap_err()
                .code,
            "extension_group_invalid"
        );
        assert!(service.list(&admin, &[], &beta).unwrap().is_empty());
    }
}
