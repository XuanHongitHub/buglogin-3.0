use serde::{Deserialize, Serialize};

use std::collections::BTreeMap;

use buglogin_domain::{
    errors::Problem,
    ids::{OrganizationId, WorkspaceId},
    organization::{Organization, OrganizationPolicy, OrganizationSnapshot},
    permissions::{Action, Capability, Grant, Principal, ResourceScope, is_allowed},
    workspace::{OfflineEntitlement, Workspace, WorkspaceUsage},
};

const CAPABILITY: &str = "CAP-WS-001";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrganizationRecord {
    pub organization: Organization,
    pub policy: OrganizationPolicy,
    pub revision: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrganizationUpdate {
    pub organization: Organization,
    pub policy: OrganizationPolicy,
    pub expected_revision: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceRecord {
    pub workspace: Workspace,
    pub revision: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceUpdate {
    pub workspace: Workspace,
    pub expected_revision: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct StoredOrganizationRecord {
    organization: Organization,
    policy: OrganizationPolicy,
    #[serde(default = "initial_revision")]
    revision: u64,
    workspaces: BTreeMap<WorkspaceId, Workspace>,
    #[serde(default)]
    workspace_revisions: BTreeMap<WorkspaceId, u64>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct WorkspaceService {
    organizations: BTreeMap<OrganizationId, StoredOrganizationRecord>,
}

impl WorkspaceService {
    pub fn new(organization: Organization, policy: OrganizationPolicy) -> Result<Self, Problem> {
        validate_name(&organization.name)?;
        let mut service = Self::default();
        service.organizations.insert(
            organization.id.clone(),
            StoredOrganizationRecord {
                organization,
                policy,
                revision: initial_revision(),
                workspaces: BTreeMap::new(),
                workspace_revisions: BTreeMap::new(),
            },
        );
        Ok(service)
    }

    pub fn add_organization(
        &mut self,
        principal: &Principal,
        grants: &[Grant],
        organization: Organization,
        policy: OrganizationPolicy,
    ) -> Result<(), Problem> {
        validate_name(&organization.name)?;
        authorize_organization(principal, grants, &organization, Action::Create)?;
        if self.organizations.contains_key(&organization.id) {
            return Err(problem(
                "organization_exists",
                "Organization already exists",
                409,
            ));
        }
        self.organizations.insert(
            organization.id.clone(),
            StoredOrganizationRecord {
                organization,
                policy,
                revision: initial_revision(),
                workspaces: BTreeMap::new(),
                workspace_revisions: BTreeMap::new(),
            },
        );
        Ok(())
    }

    pub fn create_workspace(
        &mut self,
        principal: &Principal,
        grants: &[Grant],
        organization_id: &OrganizationId,
        workspace: Workspace,
    ) -> Result<(), Problem> {
        if workspace.organization_id != *organization_id {
            return Err(problem(
                "tenant_mismatch",
                "Workspace belongs to another organization",
                403,
            ));
        }
        authorize_workspace_create(principal, grants, organization_id, &workspace.id)?;
        validate_name(&workspace.name)?;
        if self
            .organizations
            .values()
            .any(|record| record.workspaces.contains_key(&workspace.id))
        {
            return Err(problem("workspace_exists", "Workspace already exists", 409));
        }
        let record = self
            .organizations
            .get_mut(organization_id)
            .ok_or_else(|| problem("organization_not_found", "Organization not found", 404))?;
        if record.workspaces.len() as u32 >= record.policy.max_workspaces {
            return Err(problem(
                "workspace_quota_exceeded",
                "Workspace quota exceeded",
                409,
            ));
        }
        record
            .workspace_revisions
            .insert(workspace.id.clone(), initial_revision());
        record.workspaces.insert(workspace.id.clone(), workspace);
        Ok(())
    }

    pub fn get_organization(
        &self,
        principal: &Principal,
        grants: &[Grant],
        organization_id: &OrganizationId,
    ) -> Result<OrganizationRecord, Problem> {
        let record = self
            .organizations
            .get(organization_id)
            .ok_or_else(|| problem("organization_not_found", "Organization not found", 404))?;
        authorize_organization(principal, grants, &record.organization, Action::Read)?;
        Ok(OrganizationRecord {
            organization: record.organization.clone(),
            policy: record.policy.clone(),
            revision: record.revision,
        })
    }

    pub fn update_organization(
        &mut self,
        principal: &Principal,
        grants: &[Grant],
        organization_id: &OrganizationId,
        update: OrganizationUpdate,
    ) -> Result<OrganizationRecord, Problem> {
        if update.organization.id != *organization_id {
            return Err(problem(
                "tenant_mismatch",
                "Organization identifier does not match",
                403,
            ));
        }
        validate_name(&update.organization.name)?;
        let record = self
            .organizations
            .get_mut(organization_id)
            .ok_or_else(|| problem("organization_not_found", "Organization not found", 404))?;
        if update.organization.account_id != record.organization.account_id {
            return Err(problem(
                "tenant_mismatch",
                "Organization belongs to another account",
                403,
            ));
        }
        authorize_organization(principal, grants, &record.organization, Action::Update)?;
        check_revision(record.revision, update.expected_revision)?;
        record.revision = next_revision(record.revision)?;
        record.organization.name = update.organization.name.trim().to_owned();
        record.policy = update.policy;
        Ok(OrganizationRecord {
            organization: record.organization.clone(),
            policy: record.policy.clone(),
            revision: record.revision,
        })
    }

    pub fn get_workspace(
        &self,
        principal: &Principal,
        grants: &[Grant],
        organization_id: &OrganizationId,
        workspace_id: &WorkspaceId,
    ) -> Result<Workspace, Problem> {
        authorize(principal, grants, workspace_id, Action::Read)?;
        Ok(self.workspace_in(organization_id, workspace_id)?.clone())
    }

    pub fn get_workspace_record(
        &self,
        principal: &Principal,
        grants: &[Grant],
        organization_id: &OrganizationId,
        workspace_id: &WorkspaceId,
    ) -> Result<WorkspaceRecord, Problem> {
        authorize(principal, grants, workspace_id, Action::Read)?;
        let record = self
            .organizations
            .get(organization_id)
            .ok_or_else(|| problem("organization_not_found", "Organization not found", 404))?;
        let workspace = record
            .workspaces
            .get(workspace_id)
            .ok_or_else(|| problem("workspace_not_found", "Workspace not found", 404))?;
        if workspace.organization_id != *organization_id {
            return Err(problem(
                "tenant_mismatch",
                "Workspace belongs to another organization",
                403,
            ));
        }
        Ok(WorkspaceRecord {
            workspace: workspace.clone(),
            revision: record
                .workspace_revisions
                .get(workspace_id)
                .copied()
                .unwrap_or_else(initial_revision),
        })
    }

    pub(crate) fn workspace_for_policy(
        &self,
        workspace_id: &WorkspaceId,
    ) -> Result<Workspace, Problem> {
        self.organizations
            .values()
            .find_map(|record| record.workspaces.get(workspace_id))
            .cloned()
            .ok_or_else(|| problem("workspace_not_found", "Workspace not found", 404))
    }

    pub fn update_workspace(
        &mut self,
        principal: &Principal,
        grants: &[Grant],
        organization_id: &OrganizationId,
        workspace_id: &WorkspaceId,
        update: WorkspaceUpdate,
    ) -> Result<WorkspaceRecord, Problem> {
        if update.workspace.id != *workspace_id
            || update.workspace.organization_id != *organization_id
        {
            return Err(problem(
                "tenant_mismatch",
                "Workspace tenant does not match",
                403,
            ));
        }
        authorize(principal, grants, workspace_id, Action::Update)?;
        validate_name(&update.workspace.name)?;
        let record = self
            .organizations
            .get_mut(organization_id)
            .ok_or_else(|| problem("organization_not_found", "Organization not found", 404))?;
        // Workspace limits are the organization's plan; workspace-level update
        // rights (held by operators) only cover the name.
        let policy_changed = record
            .workspaces
            .get(workspace_id)
            .is_some_and(|workspace| workspace.policy != update.workspace.policy);
        if policy_changed {
            authorize_organization(principal, grants, &record.organization, Action::Update)
                .map_err(|_| {
                    problem(
                        "workspace_policy_forbidden",
                        "Only organization administrators can change workspace limits",
                        403,
                    )
                })?;
        }
        let workspace = record
            .workspaces
            .get_mut(workspace_id)
            .ok_or_else(|| problem("workspace_not_found", "Workspace not found", 404))?;
        if workspace.organization_id != *organization_id {
            return Err(problem(
                "tenant_mismatch",
                "Workspace belongs to another organization",
                403,
            ));
        }
        let revision = record
            .workspace_revisions
            .get(workspace_id)
            .copied()
            .unwrap_or_else(initial_revision);
        check_revision(revision, update.expected_revision)?;
        let revision = next_revision(revision)?;
        workspace.name = update.workspace.name.trim().to_owned();
        workspace.policy = update.workspace.policy;
        record
            .workspace_revisions
            .insert(workspace_id.clone(), revision);
        Ok(WorkspaceRecord {
            workspace: workspace.clone(),
            revision,
        })
    }

    pub fn organization_snapshot(
        &self,
        principal: &Principal,
        grants: &[Grant],
        organization_id: &OrganizationId,
        workspace_id: &WorkspaceId,
    ) -> Result<OrganizationSnapshot, Problem> {
        let record = self
            .organizations
            .get(organization_id)
            .ok_or_else(|| problem("organization_not_found", "Organization not found", 404))?;
        if !record.workspaces.contains_key(workspace_id) {
            return Err(problem("workspace_not_found", "Workspace not found", 404));
        }
        authorize(principal, grants, workspace_id, Action::Read)?;
        Ok(OrganizationSnapshot {
            organization: record.organization.clone(),
            policy: record.policy.clone(),
            workspace_ids: record.workspaces.keys().cloned().collect(),
        })
    }

    pub fn check_usage(
        &self,
        principal: &Principal,
        grants: &[Grant],
        organization_id: &OrganizationId,
        workspace_id: &WorkspaceId,
        usage: WorkspaceUsage,
    ) -> Result<(), Problem> {
        authorize(principal, grants, workspace_id, Action::Read)?;
        let workspace = self.workspace_in(organization_id, workspace_id)?;
        if usage.profiles > workspace.policy.max_profiles {
            return Err(problem(
                "profile_quota_exceeded",
                "Profile quota exceeded",
                409,
            ));
        }
        if usage.members > workspace.policy.max_members {
            return Err(problem(
                "member_quota_exceeded",
                "Member quota exceeded",
                409,
            ));
        }
        if usage.storage_bytes > workspace.policy.max_storage_bytes {
            return Err(problem(
                "storage_quota_exceeded",
                "Storage quota exceeded",
                409,
            ));
        }
        Ok(())
    }

    pub fn offline_entitlement(
        &self,
        principal: &Principal,
        grants: &[Grant],
        organization_id: &OrganizationId,
        workspace_id: &WorkspaceId,
    ) -> Result<OfflineEntitlement, Problem> {
        authorize(principal, grants, workspace_id, Action::Read)?;
        let record = self
            .organizations
            .get(organization_id)
            .ok_or_else(|| problem("organization_not_found", "Organization not found", 404))?;
        let workspace = record
            .workspaces
            .get(workspace_id)
            .ok_or_else(|| problem("workspace_not_found", "Workspace not found", 404))?;
        Ok(OfflineEntitlement::from_flag(
            record.policy.offline_entitled && workspace.policy.offline_entitled,
        ))
    }

    pub fn list_organizations(
        &self,
        principal: &Principal,
        grants: &[Grant],
    ) -> Result<Vec<Organization>, Problem> {
        let mut list = Vec::new();
        for record in self.organizations.values() {
            let organization_readable =
                authorize_organization(principal, grants, &record.organization, Action::Read)
                    .is_ok();
            let has_readable_workspace = record
                .workspaces
                .values()
                .any(|workspace| authorize(principal, grants, &workspace.id, Action::Read).is_ok());
            if organization_readable || has_readable_workspace {
                list.push(record.organization.clone());
            }
        }
        Ok(list)
    }

    pub fn list_workspaces(
        &self,
        principal: &Principal,
        grants: &[Grant],
        organization_id: &OrganizationId,
    ) -> Result<Vec<Workspace>, Problem> {
        let record = self
            .organizations
            .get(organization_id)
            .ok_or_else(|| problem("organization_not_found", "Organization not found", 404))?;
        let organization_readable =
            authorize_organization(principal, grants, &record.organization, Action::Read).is_ok();
        let mut list = Vec::new();
        for ws in record.workspaces.values() {
            if authorize(principal, grants, &ws.id, Action::Read).is_ok() {
                list.push(ws.clone());
            }
        }
        if !organization_readable && list.is_empty() {
            return Err(problem("forbidden", "Organization access denied", 403));
        }
        Ok(list)
    }

    fn workspace_in(
        &self,
        organization_id: &OrganizationId,
        workspace_id: &WorkspaceId,
    ) -> Result<&Workspace, Problem> {
        self.organizations
            .get(organization_id)
            .ok_or_else(|| problem("organization_not_found", "Organization not found", 404))?
            .workspaces
            .get(workspace_id)
            .ok_or_else(|| problem("workspace_not_found", "Workspace not found", 404))
    }
}

fn authorize(
    principal: &Principal,
    grants: &[Grant],
    workspace_id: &WorkspaceId,
    action: Action,
) -> Result<(), Problem> {
    let capability = Capability::new(CAPABILITY)
        .map_err(|_| problem("invalid_capability", "Invalid capability", 500))?;
    if is_allowed(
        principal,
        &capability,
        action,
        &ResourceScope::workspace(workspace_id.clone()),
        grants,
    ) {
        Ok(())
    } else {
        Err(problem("forbidden", "Workspace access denied", 403))
    }
}

fn authorize_workspace_create(
    principal: &Principal,
    grants: &[Grant],
    organization_id: &OrganizationId,
    workspace_id: &WorkspaceId,
) -> Result<(), Problem> {
    let capability = Capability::new(CAPABILITY)
        .map_err(|_| problem("invalid_capability", "Invalid capability", 500))?;
    let scope = ResourceScope {
        organization_id: Some(organization_id.clone()),
        workspace_id: Some(workspace_id.clone()),
        ..ResourceScope::default()
    };
    if is_allowed(principal, &capability, Action::Create, &scope, grants) {
        Ok(())
    } else {
        Err(problem("forbidden", "Workspace access denied", 403))
    }
}

fn authorize_organization(
    principal: &Principal,
    grants: &[Grant],
    organization: &Organization,
    action: Action,
) -> Result<(), Problem> {
    let capability = Capability::new("CAP-ORG-001")
        .map_err(|_| problem("invalid_capability", "Invalid capability", 500))?;
    let scope = ResourceScope {
        account_id: Some(organization.account_id.clone()),
        organization_id: Some(organization.id.clone()),
        ..ResourceScope::default()
    };
    if is_allowed(principal, &capability, action, &scope, grants) {
        Ok(())
    } else {
        Err(problem("forbidden", "Organization access denied", 403))
    }
}

fn initial_revision() -> u64 {
    1
}

fn next_revision(revision: u64) -> Result<u64, Problem> {
    revision
        .checked_add(1)
        .ok_or_else(|| problem("revision_exhausted", "Revision exhausted", 409))
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

fn validate_name(name: &str) -> Result<(), Problem> {
    if name.trim().is_empty() || name.chars().count() > 128 {
        return Err(problem("invalid_name", "Name is invalid", 400));
    }
    Ok(())
}

fn problem(code: &str, title: &str, status: u16) -> Problem {
    Problem::new(code, title, status, "workspace")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use buglogin_domain::{
        ids::{AccountId, GrantId, OrganizationId, WorkspaceId},
        organization::{Organization, OrganizationPolicy},
        permissions::{Action, Capability, Effect, Grant, Principal, ResourceScope, Role},
        workspace::{Workspace, WorkspacePolicy},
    };

    use super::{CAPABILITY, OrganizationUpdate, WorkspaceService, WorkspaceUpdate};

    fn organization(id: &str, account_id: &str) -> Organization {
        Organization {
            id: OrganizationId::new(id).unwrap(),
            account_id: AccountId::new(account_id).unwrap(),
            name: "Organization".to_owned(),
            created_at: "2026-07-22T00:00:00Z".to_owned(),
        }
    }

    fn organization_policy() -> OrganizationPolicy {
        OrganizationPolicy {
            max_workspaces: 4,
            offline_entitled: true,
        }
    }

    fn workspace(id: &str, organization_id: &OrganizationId) -> Workspace {
        Workspace {
            id: WorkspaceId::new(id).unwrap(),
            organization_id: organization_id.clone(),
            name: "Workspace".to_owned(),
            policy: WorkspacePolicy {
                max_profiles: 10,
                max_members: 5,
                max_storage_bytes: 1024,
                offline_entitled: true,
            },
            created_at: "2026-07-22T00:00:00Z".to_owned(),
        }
    }

    fn platform_admin() -> Principal {
        Principal {
            roles: BTreeSet::from([Role::PlatformAdmin]),
            ..Principal::default()
        }
    }

    #[test]
    fn gets_organization_without_workspace_lookup() {
        let organization = organization("org-1", "account-1");
        let service = WorkspaceService::new(organization.clone(), organization_policy()).unwrap();

        let record = service
            .get_organization(&platform_admin(), &[], &organization.id)
            .unwrap();

        assert_eq!(record.organization, organization);
        assert_eq!(record.revision, 1);
    }

    #[test]
    fn organization_update_enforces_acl_tenant_and_revision() {
        let organization = organization("org-1", "account-1");
        let mut service =
            WorkspaceService::new(organization.clone(), organization_policy()).unwrap();
        let mut renamed = organization.clone();
        renamed.name = "  Renamed  ".to_owned();
        let update = OrganizationUpdate {
            organization: renamed,
            policy: OrganizationPolicy {
                max_workspaces: 8,
                offline_entitled: false,
            },
            expected_revision: 1,
        };

        assert_eq!(
            service
                .update_organization(&Principal::default(), &[], &organization.id, update.clone(),)
                .unwrap_err()
                .code,
            "forbidden"
        );
        let updated = service
            .update_organization(&platform_admin(), &[], &organization.id, update.clone())
            .unwrap();
        assert_eq!(updated.organization.name, "Renamed");
        assert_eq!(updated.revision, 2);
        assert_eq!(
            service
                .update_organization(&platform_admin(), &[], &organization.id, update)
                .unwrap_err()
                .code,
            "revision_conflict"
        );

        let mut wrong_tenant = organization.clone();
        wrong_tenant.account_id = AccountId::new("account-2").unwrap();
        assert_eq!(
            service
                .update_organization(
                    &platform_admin(),
                    &[],
                    &organization.id,
                    OrganizationUpdate {
                        organization: wrong_tenant,
                        policy: organization_policy(),
                        expected_revision: 2,
                    },
                )
                .unwrap_err()
                .code,
            "tenant_mismatch"
        );
    }

    #[test]
    fn workspace_update_enforces_tenant_and_revision() {
        let organization = organization("org-1", "account-1");
        let mut service =
            WorkspaceService::new(organization.clone(), organization_policy()).unwrap();
        let workspace = workspace("workspace-1", &organization.id);
        service
            .create_workspace(&platform_admin(), &[], &organization.id, workspace.clone())
            .unwrap();
        let mut renamed = workspace.clone();
        renamed.name = "  Renamed Workspace  ".to_owned();

        let updated = service
            .update_workspace(
                &platform_admin(),
                &[],
                &organization.id,
                &workspace.id,
                WorkspaceUpdate {
                    workspace: renamed.clone(),
                    expected_revision: 1,
                },
            )
            .unwrap();
        assert_eq!(updated.workspace.name, "Renamed Workspace");
        assert_eq!(updated.revision, 2);
        assert_eq!(
            service
                .update_workspace(
                    &platform_admin(),
                    &[],
                    &organization.id,
                    &workspace.id,
                    WorkspaceUpdate {
                        workspace: renamed.clone(),
                        expected_revision: 1,
                    },
                )
                .unwrap_err()
                .code,
            "revision_conflict"
        );

        renamed.organization_id = OrganizationId::new("org-2").unwrap();
        assert_eq!(
            service
                .update_workspace(
                    &platform_admin(),
                    &[],
                    &organization.id,
                    &workspace.id,
                    WorkspaceUpdate {
                        workspace: renamed,
                        expected_revision: 2,
                    },
                )
                .unwrap_err()
                .code,
            "tenant_mismatch"
        );
    }

    #[test]
    fn versioned_record_reads_return_current_revision_and_enforce_read_acl() {
        let organization = organization("org-record", "account-record");
        let mut service =
            WorkspaceService::new(organization.clone(), organization_policy()).unwrap();
        let workspace = workspace("workspace-record", &organization.id);
        service
            .create_workspace(&platform_admin(), &[], &organization.id, workspace.clone())
            .unwrap();

        let org_record = service
            .get_organization(&platform_admin(), &[], &organization.id)
            .unwrap();
        assert_eq!(org_record.revision, 1);
        let workspace_record = service
            .get_workspace_record(&platform_admin(), &[], &organization.id, &workspace.id)
            .unwrap();
        assert_eq!(workspace_record.revision, 1);

        let mut renamed = workspace;
        renamed.name = "Renamed".to_owned();
        let renamed_id = renamed.id.clone();
        service
            .update_workspace(
                &platform_admin(),
                &[],
                &organization.id,
                &renamed_id,
                WorkspaceUpdate {
                    workspace: renamed,
                    expected_revision: workspace_record.revision,
                },
            )
            .unwrap();
        assert_eq!(
            service
                .get_workspace_record(
                    &platform_admin(),
                    &[],
                    &organization.id,
                    &WorkspaceId::new("workspace-record").unwrap()
                )
                .unwrap()
                .revision,
            2
        );
        assert_eq!(
            service
                .get_workspace_record(
                    &Principal::default(),
                    &[],
                    &organization.id,
                    &WorkspaceId::new("workspace-record").unwrap()
                )
                .unwrap_err()
                .code,
            "forbidden"
        );
    }

    #[test]
    fn workspace_operators_can_rename_but_not_change_limits() {
        let organization = organization("org-limits", "account-owner");
        let mut service =
            WorkspaceService::new(organization.clone(), organization_policy()).unwrap();
        let workspace = workspace("workspace-limits", &organization.id);
        service
            .create_workspace(&platform_admin(), &[], &organization.id, workspace.clone())
            .unwrap();
        let operator_account = AccountId::new("account-operator").unwrap();
        let operator = Principal {
            account_id: Some(operator_account.clone()),
            roles: BTreeSet::from([Role::Account]),
            workspace_ids: BTreeSet::from([workspace.id.clone()]),
            ..Principal::default()
        };
        let grants = [Grant {
            id: GrantId::new("grant_operator_workspace_update").unwrap(),
            account_id: Some(operator_account),
            member_id: None,
            capability: Capability::new(CAPABILITY).unwrap(),
            action: Action::Update,
            scope: ResourceScope::workspace(workspace.id.clone()),
            effect: Effect::Allow,
        }];

        let mut renamed = workspace.clone();
        renamed.name = "Operator Renamed".to_owned();
        let updated = service
            .update_workspace(
                &operator,
                &grants,
                &organization.id,
                &workspace.id,
                WorkspaceUpdate {
                    workspace: renamed.clone(),
                    expected_revision: 1,
                },
            )
            .unwrap();
        assert_eq!(updated.workspace.name, "Operator Renamed");

        let mut raised = renamed.clone();
        raised.policy.max_profiles = 1_000_000;
        raised.policy.max_storage_bytes = u64::MAX / 2;
        assert_eq!(
            service
                .update_workspace(
                    &operator,
                    &grants,
                    &organization.id,
                    &workspace.id,
                    WorkspaceUpdate {
                        workspace: raised.clone(),
                        expected_revision: 2,
                    },
                )
                .unwrap_err()
                .code,
            "workspace_policy_forbidden"
        );

        let raised_by_admin = service
            .update_workspace(
                &platform_admin(),
                &[],
                &organization.id,
                &workspace.id,
                WorkspaceUpdate {
                    workspace: raised,
                    expected_revision: 2,
                },
            )
            .unwrap();
        assert_eq!(raised_by_admin.workspace.policy.max_profiles, 1_000_000);
    }

    #[test]
    fn organization_scoped_workspace_grant_can_create_workspace() {
        let organization = organization("org-create", "account-create");
        let mut service =
            WorkspaceService::new(organization.clone(), organization_policy()).unwrap();
        let principal = Principal {
            account_id: Some(organization.account_id.clone()),
            ..Principal::default()
        };
        let grants = [Grant {
            id: GrantId::new("grant_org_workspace_create").unwrap(),
            account_id: Some(organization.account_id.clone()),
            member_id: None,
            capability: Capability::new(CAPABILITY).unwrap(),
            action: Action::Create,
            scope: ResourceScope {
                organization_id: Some(organization.id.clone()),
                ..ResourceScope::default()
            },
            effect: Effect::Allow,
        }];

        service
            .create_workspace(
                &principal,
                &grants,
                &organization.id,
                workspace("workspace-created-by-org-admin", &organization.id),
            )
            .unwrap();

        assert_eq!(
            service
                .list_workspaces(&platform_admin(), &[], &organization.id)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn workspace_member_can_discover_only_organizations_and_workspaces_they_can_read() {
        let organization = organization("org-member", "account-member");
        let mut service =
            WorkspaceService::new(organization.clone(), organization_policy()).unwrap();
        let assigned = workspace("workspace-assigned", &organization.id);
        let unassigned = workspace("workspace-unassigned", &organization.id);
        service
            .create_workspace(&platform_admin(), &[], &organization.id, assigned.clone())
            .unwrap();
        service
            .create_workspace(&platform_admin(), &[], &organization.id, unassigned)
            .unwrap();
        let principal = Principal {
            account_id: Some(organization.account_id.clone()),
            workspace_ids: BTreeSet::from([assigned.id.clone()]),
            roles: BTreeSet::from([Role::WorkspaceMember]),
            ..Principal::default()
        };
        let grants = [Grant {
            id: GrantId::new("grant_member_workspace_read").unwrap(),
            account_id: Some(organization.account_id.clone()),
            member_id: None,
            capability: Capability::new(CAPABILITY).unwrap(),
            action: Action::Read,
            scope: ResourceScope::workspace(assigned.id.clone()),
            effect: Effect::Allow,
        }];

        assert_eq!(
            service.list_organizations(&principal, &grants).unwrap(),
            vec![organization.clone()]
        );
        assert_eq!(
            service
                .list_workspaces(&principal, &grants, &organization.id)
                .unwrap(),
            vec![assigned]
        );
        assert_eq!(
            service
                .list_workspaces(&Principal::default(), &[], &organization.id)
                .unwrap_err()
                .code,
            "forbidden"
        );
    }
}
