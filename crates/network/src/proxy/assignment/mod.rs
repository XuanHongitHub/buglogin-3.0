use super::repository::{PROXY_CAPABILITY, ProxyRepository};
use buglogin_domain::ids::{ProfileId, ProxyId, WorkspaceId};
use buglogin_domain::permissions::{
    Action, Capability, Grant, Principal, ResourceScope, is_allowed,
};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProxyAssignment {
    pub proxy_id: ProxyId,
    pub profile_id: ProfileId,
    pub workspace_id: WorkspaceId,
    pub revision: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssignmentUpdate {
    pub proxy_id: ProxyId,
    pub expected_revision: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssignmentDelete {
    pub expected_revision: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AssignmentError {
    Forbidden,
    AssignmentNotFound,
    ProxyNotFound,
    RevisionConflict { expected: u64, actual: u64 },
    RevisionExhausted,
    IntegrityViolation,
}

#[derive(Default)]
pub struct ProxyAssignmentRepository {
    assignments: BTreeMap<(WorkspaceId, ProfileId), ProxyAssignment>,
}

impl ProxyAssignmentRepository {
    pub fn create(
        &mut self,
        proxies: &ProxyRepository,
        proxy_id: ProxyId,
        profile_id: ProfileId,
        workspace_id: WorkspaceId,
        principal: &Principal,
        grants: &[Grant],
    ) -> Result<ProxyAssignment, AssignmentError> {
        authorize(
            principal,
            grants,
            &workspace_id,
            &profile_id,
            Action::Execute,
        )?;
        ensure_proxy(proxies, &workspace_id, &proxy_id)?;
        let key = (workspace_id.clone(), profile_id.clone());
        if let Some(existing) = self.assignments.get(&key) {
            if existing.proxy_id == proxy_id {
                return Ok(existing.clone());
            }
            return Err(AssignmentError::IntegrityViolation);
        }
        let assignment = ProxyAssignment {
            proxy_id,
            profile_id,
            workspace_id,
            revision: 1,
        };
        self.assignments.insert(key, assignment.clone());
        Ok(assignment)
    }

    pub fn list(
        &self,
        workspace_id: &WorkspaceId,
        principal: &Principal,
        grants: &[Grant],
    ) -> Result<Vec<ProxyAssignment>, AssignmentError> {
        authorize_workspace(principal, grants, workspace_id, Action::Read)?;
        Ok(self
            .assignments
            .values()
            .filter(|assignment| &assignment.workspace_id == workspace_id)
            .cloned()
            .collect())
    }

    pub fn update(
        &mut self,
        proxies: &ProxyRepository,
        workspace_id: &WorkspaceId,
        profile_id: &ProfileId,
        input: AssignmentUpdate,
        principal: &Principal,
        grants: &[Grant],
    ) -> Result<ProxyAssignment, AssignmentError> {
        authorize(principal, grants, workspace_id, profile_id, Action::Execute)?;
        ensure_proxy(proxies, workspace_id, &input.proxy_id)?;
        let key = (workspace_id.clone(), profile_id.clone());
        let assignment = self
            .assignments
            .get_mut(&key)
            .ok_or(AssignmentError::AssignmentNotFound)?;
        check_revision(assignment.revision, input.expected_revision)?;
        assignment.revision = assignment
            .revision
            .checked_add(1)
            .ok_or(AssignmentError::RevisionExhausted)?;
        assignment.proxy_id = input.proxy_id;
        Ok(assignment.clone())
    }

    pub fn delete(
        &mut self,
        workspace_id: &WorkspaceId,
        profile_id: &ProfileId,
        input: AssignmentDelete,
        principal: &Principal,
        grants: &[Grant],
    ) -> Result<ProxyAssignment, AssignmentError> {
        authorize(principal, grants, workspace_id, profile_id, Action::Execute)?;
        let key = (workspace_id.clone(), profile_id.clone());
        let assignment = self
            .assignments
            .get(&key)
            .ok_or(AssignmentError::AssignmentNotFound)?;
        check_revision(assignment.revision, input.expected_revision)?;
        self.assignments
            .remove(&key)
            .ok_or(AssignmentError::AssignmentNotFound)
    }

    pub fn check_integrity(
        &self,
        proxies: &ProxyRepository,
        workspace_id: &WorkspaceId,
    ) -> Result<(), AssignmentError> {
        if self
            .assignments
            .values()
            .filter(|assignment| &assignment.workspace_id == workspace_id)
            .all(|assignment| {
                proxies
                    .get_in_workspace(&assignment.proxy_id, workspace_id)
                    .is_some()
            })
        {
            Ok(())
        } else {
            Err(AssignmentError::IntegrityViolation)
        }
    }

    pub(crate) fn is_proxy_assigned(&self, workspace_id: &WorkspaceId, proxy_id: &ProxyId) -> bool {
        self.assignments.values().any(|assignment| {
            &assignment.workspace_id == workspace_id && &assignment.proxy_id == proxy_id
        })
    }
}

fn ensure_proxy(
    proxies: &ProxyRepository,
    workspace_id: &WorkspaceId,
    proxy_id: &ProxyId,
) -> Result<(), AssignmentError> {
    if proxies.get_in_workspace(proxy_id, workspace_id).is_some() {
        Ok(())
    } else {
        Err(AssignmentError::ProxyNotFound)
    }
}

fn check_revision(actual: u64, expected: u64) -> Result<(), AssignmentError> {
    if actual == expected {
        Ok(())
    } else {
        Err(AssignmentError::RevisionConflict { expected, actual })
    }
}

fn authorize(
    principal: &Principal,
    grants: &[Grant],
    workspace_id: &WorkspaceId,
    profile_id: &ProfileId,
    action: Action,
) -> Result<(), AssignmentError> {
    authorize_scope(
        principal,
        grants,
        action,
        &ResourceScope {
            workspace_id: Some(workspace_id.clone()),
            profile_id: Some(profile_id.clone()),
            ..ResourceScope::default()
        },
    )
}

fn authorize_workspace(
    principal: &Principal,
    grants: &[Grant],
    workspace_id: &WorkspaceId,
    action: Action,
) -> Result<(), AssignmentError> {
    authorize_scope(
        principal,
        grants,
        action,
        &ResourceScope::workspace(workspace_id.clone()),
    )
}

fn authorize_scope(
    principal: &Principal,
    grants: &[Grant],
    action: Action,
    scope: &ResourceScope,
) -> Result<(), AssignmentError> {
    let capability = Capability::new(PROXY_CAPABILITY).map_err(|_| AssignmentError::Forbidden)?;
    if is_allowed(principal, &capability, action, scope, grants) {
        Ok(())
    } else {
        Err(AssignmentError::Forbidden)
    }
}
