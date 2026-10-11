use buglogin_domain::{
    entities::Profile,
    errors::Problem,
    ids::{GroupId, ProfileId, WorkspaceId},
    permissions::{Action, Capability, Grant, Principal, ResourceScope, is_allowed},
};
use buglogin_storage::local::{LocalRepository, Versioned};

use super::{
    ProfileService,
    groups::{self, GROUPS_CAPABILITY, GroupChange},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BulkTarget {
    pub profile_id: ProfileId,
    pub expected_revision: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BulkOperation {
    AssignGroup(GroupId),
    RemoveGroup(GroupId),
    Archive,
    Restore,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BulkTargetStatus {
    Success,
    Denied,
    Failed,
    Unchanged,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BulkTargetResult {
    pub profile_id: ProfileId,
    pub status: BulkTargetStatus,
    pub revision: Option<u64>,
    pub problem: Option<Problem>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BulkReport {
    pub results: Vec<BulkTargetResult>,
}

impl BulkReport {
    pub fn count(&self, status: BulkTargetStatus) -> usize {
        self.results
            .iter()
            .filter(|item| item.status == status)
            .count()
    }
    pub fn success_count(&self) -> usize {
        self.count(BulkTargetStatus::Success)
    }
    pub fn denied_count(&self) -> usize {
        self.count(BulkTargetStatus::Denied)
    }
    pub fn failed_count(&self) -> usize {
        self.count(BulkTargetStatus::Failed)
    }
    pub fn unchanged_count(&self) -> usize {
        self.count(BulkTargetStatus::Unchanged)
    }
}

#[derive(Clone)]
pub struct BulkProfileService {
    profiles: ProfileService,
}

impl BulkProfileService {
    pub fn new(repository: LocalRepository) -> Self {
        Self {
            profiles: ProfileService::new(repository),
        }
    }

    pub fn execute(
        &self,
        workspace_id: &WorkspaceId,
        targets: impl IntoIterator<Item = BulkTarget>,
        operation: BulkOperation,
        principal: &Principal,
        grants: &[Grant],
    ) -> BulkReport {
        BulkReport {
            results: targets
                .into_iter()
                .map(|target| {
                    self.execute_target(workspace_id, target, &operation, principal, grants)
                })
                .collect(),
        }
    }

    fn execute_target(
        &self,
        workspace_id: &WorkspaceId,
        target: BulkTarget,
        operation: &BulkOperation,
        principal: &Principal,
        grants: &[Grant],
    ) -> BulkTargetResult {
        if let Err(error) = authorize(&target.profile_id, workspace_id, principal, grants) {
            return result(target.profile_id, BulkTargetStatus::Denied, None, error);
        }
        let current = match self.profiles.get(&target.profile_id) {
            Ok(Some(value)) => value,
            Ok(None) => {
                return result(
                    target.profile_id,
                    BulkTargetStatus::Failed,
                    None,
                    problem("profile_not_found", "Profile not found", 404),
                );
            }
            Err(error) => return result(target.profile_id, BulkTargetStatus::Failed, None, error),
        };
        if current.revision != target.expected_revision {
            return result(
                target.profile_id,
                BulkTargetStatus::Failed,
                Some(current.revision),
                problem("revision_conflict", "Profile changed", 409)
                    .with_field("expectedRevision", target.expected_revision)
                    .with_field("actualRevision", current.revision),
            );
        }
        let (next, changed) =
            match apply_operation(current.clone(), workspace_id, operation, principal, grants) {
                Ok(value) => value,
                Err(error) if error.status == 403 => {
                    return result(target.profile_id, BulkTargetStatus::Denied, None, error);
                }
                Err(error) => {
                    return result(target.profile_id, BulkTargetStatus::Failed, None, error);
                }
            };
        if !changed {
            return BulkTargetResult {
                profile_id: target.profile_id,
                status: BulkTargetStatus::Unchanged,
                revision: Some(current.revision),
                problem: None,
            };
        }
        match self.profiles.update(current.revision, next) {
            Ok(updated) => BulkTargetResult {
                profile_id: target.profile_id,
                status: BulkTargetStatus::Success,
                revision: Some(updated.revision),
                problem: None,
            },
            Err(error) => result(
                target.profile_id,
                BulkTargetStatus::Failed,
                Some(current.revision),
                error,
            ),
        }
    }
}

fn apply_operation(
    current: Versioned<Profile>,
    workspace_id: &WorkspaceId,
    operation: &BulkOperation,
    principal: &Principal,
    grants: &[Grant],
) -> Result<(Profile, bool), Problem> {
    let mut next = current.value;
    let change = match operation {
        BulkOperation::AssignGroup(group_id) => {
            groups::assign(&mut next, workspace_id, group_id.clone(), principal, grants)?
        }
        BulkOperation::RemoveGroup(group_id) => {
            groups::remove(&mut next, workspace_id, group_id, principal, grants)?
        }
        BulkOperation::Archive => {
            authorize(&next.id, workspace_id, principal, grants)?;
            if next.archived {
                GroupChange::Unchanged
            } else {
                next.archived = true;
                GroupChange::Assigned
            }
        }
        BulkOperation::Restore => {
            authorize(&next.id, workspace_id, principal, grants)?;
            if next.archived {
                next.archived = false;
                GroupChange::Removed
            } else {
                GroupChange::Unchanged
            }
        }
    };
    Ok((next, change != GroupChange::Unchanged))
}

fn authorize(
    profile_id: &ProfileId,
    workspace_id: &WorkspaceId,
    principal: &Principal,
    grants: &[Grant],
) -> Result<(), Problem> {
    let capability = Capability::new(GROUPS_CAPABILITY)
        .map_err(|_| problem("invalid_capability", "Invalid profile capability", 500))?;
    let scope = ResourceScope {
        workspace_id: Some(workspace_id.clone()),
        profile_id: Some(profile_id.clone()),
        ..ResourceScope::default()
    };
    if is_allowed(principal, &capability, Action::Update, &scope, grants) {
        Ok(())
    } else {
        Err(problem("forbidden", "Profile access denied", 403))
    }
}

fn result(
    profile_id: ProfileId,
    status: BulkTargetStatus,
    revision: Option<u64>,
    problem: Problem,
) -> BulkTargetResult {
    BulkTargetResult {
        profile_id,
        status,
        revision,
        problem: Some(problem),
    }
}

fn problem(code: &str, title: &str, status: u16) -> Problem {
    Problem::new(code, title, status, "profile-bulk")
}
