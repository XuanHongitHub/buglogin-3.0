use buglogin_domain::{
    entities::Profile,
    errors::Problem,
    ids::{GroupId, ProfileId, WorkspaceId},
    permissions::{Action, Capability, Grant, Principal, ResourceScope, is_allowed},
};

pub const GROUPS_CAPABILITY: &str = "CAP-PROF-003";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GroupChange {
    Assigned,
    Removed,
    Unchanged,
}

pub fn assign(
    profile: &mut Profile,
    workspace_id: &WorkspaceId,
    group_id: GroupId,
    principal: &Principal,
    grants: &[Grant],
) -> Result<GroupChange, Problem> {
    authorize(profile.id.clone(), workspace_id, principal, grants)?;
    if profile.group_ids.contains(&group_id) {
        return Ok(GroupChange::Unchanged);
    }
    profile.group_ids.push(group_id);
    profile.group_ids.sort();
    Ok(GroupChange::Assigned)
}

pub fn remove(
    profile: &mut Profile,
    workspace_id: &WorkspaceId,
    group_id: &GroupId,
    principal: &Principal,
    grants: &[Grant],
) -> Result<GroupChange, Problem> {
    authorize(profile.id.clone(), workspace_id, principal, grants)?;
    let before = profile.group_ids.len();
    profile.group_ids.retain(|current| current != group_id);
    Ok(if before == profile.group_ids.len() {
        GroupChange::Unchanged
    } else {
        GroupChange::Removed
    })
}

fn authorize(
    profile_id: ProfileId,
    workspace_id: &WorkspaceId,
    principal: &Principal,
    grants: &[Grant],
) -> Result<(), Problem> {
    let capability = Capability::new(GROUPS_CAPABILITY)
        .map_err(|_| problem("invalid_capability", "Invalid profile capability", 500))?;
    let scope = ResourceScope {
        workspace_id: Some(workspace_id.clone()),
        profile_id: Some(profile_id),
        ..ResourceScope::default()
    };
    if is_allowed(principal, &capability, Action::Update, &scope, grants) {
        Ok(())
    } else {
        Err(problem("forbidden", "Profile access denied", 403))
    }
}

fn problem(code: &str, title: &str, status: u16) -> Problem {
    Problem::new(code, title, status, "profile-groups")
}
