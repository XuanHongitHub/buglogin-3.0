use serde::{Deserialize, Serialize};

use std::collections::BTreeMap;

use buglogin_domain::{
    errors::Problem,
    ids::{AccountId, MemberId, WorkspaceId},
    member::{Member, MemberRole, MemberStatus},
    permissions::{Action, Capability, Grant, Principal, ResourceScope, is_allowed},
};

const CAPABILITY: &str = "CAP-MEM-001";

#[derive(Clone, Default, Deserialize, Serialize)]
pub struct MemberService {
    members: BTreeMap<MemberId, Member>,
}

impl MemberService {
    pub fn active_for_account(&self, account_id: &AccountId) -> Vec<Member> {
        self.members
            .values()
            .filter(|member| {
                &member.account_id == account_id && member.status == MemberStatus::Active
            })
            .cloned()
            .collect()
    }

    pub fn active_members(&self) -> Vec<Member> {
        self.members
            .values()
            .filter(|member| member.status == MemberStatus::Active)
            .cloned()
            .collect()
    }

    pub fn list(
        &self,
        principal: &Principal,
        grants: &[Grant],
        workspace: &WorkspaceId,
    ) -> Result<Vec<Member>, Problem> {
        authorize(principal, grants, workspace, Action::Read)?;
        let items = self
            .members
            .values()
            .filter(|member| &member.workspace_id == workspace)
            .cloned()
            .collect();
        Ok(items)
    }

    pub fn invite(
        &mut self,
        principal: &Principal,
        grants: &[Grant],
        member: Member,
        now: u64,
    ) -> Result<(), Problem> {
        authorize(principal, grants, &member.workspace_id, Action::Create)?;
        if member.status != MemberStatus::Invited
            || member
                .invite_expires_at
                .is_none_or(|expires| expires <= now)
        {
            return Err(problem("invalid_invite", "Invalid invite", 400));
        }
        if self.members.contains_key(&member.id) {
            return Err(problem("member_exists", "Member exists", 409));
        }
        self.members.insert(member.id.clone(), member);
        Ok(())
    }

    pub fn activate(
        &mut self,
        principal: &Principal,
        grants: &[Grant],
        id: &MemberId,
        now: u64,
    ) -> Result<(), Problem> {
        let member = self
            .members
            .get(id)
            .ok_or_else(|| problem("member_not_found", "Member not found", 404))?;
        let self_accept = principal.account_id.as_ref() == Some(&member.account_id)
            || principal.member_id.as_ref() == Some(id);
        if !self_accept {
            authorize(principal, grants, &member.workspace_id, Action::Update)?;
        }
        let member = self.members.get_mut(id).expect("member checked above");
        if member.status != MemberStatus::Invited
            || member
                .invite_expires_at
                .is_none_or(|expires| expires <= now)
        {
            return Err(problem("invite_expired", "Invite expired", 409));
        }
        member.status = MemberStatus::Active;
        member.invite_expires_at = None;
        Ok(())
    }

    pub fn suspend(
        &mut self,
        principal: &Principal,
        grants: &[Grant],
        id: &MemberId,
    ) -> Result<(), Problem> {
        let member = self
            .members
            .get(id)
            .ok_or_else(|| problem("member_not_found", "Member not found", 404))?;
        authorize(principal, grants, &member.workspace_id, Action::Manage)?;
        self.members
            .get_mut(id)
            .expect("member checked above")
            .status = MemberStatus::Suspended;
        Ok(())
    }

    pub fn set_role(
        &mut self,
        principal: &Principal,
        grants: &[Grant],
        id: &MemberId,
        role: MemberRole,
    ) -> Result<(), Problem> {
        let target = self
            .members
            .get(id)
            .ok_or_else(|| problem("member_not_found", "Member not found", 404))?;
        authorize(principal, grants, &target.workspace_id, Action::Manage)?;
        if !principal
            .roles
            .contains(&buglogin_domain::permissions::Role::PlatformAdmin)
        {
            let actor = principal
                .member_id
                .as_ref()
                .and_then(|actor_id| self.members.get(actor_id))
                .filter(|actor| actor.workspace_id == target.workspace_id)
                .ok_or_else(|| {
                    problem("role_escalation", "Actor membership is unavailable", 403)
                })?;
            if role_rank(&role) > role_rank(&actor.role)
                || role_rank(&target.role) > role_rank(&actor.role)
            {
                return Err(problem(
                    "role_escalation",
                    "Role change exceeds actor authority",
                    403,
                ));
            }
        }
        self.members.get_mut(id).expect("member checked above").role = role;
        Ok(())
    }

    pub fn get_in_workspace(
        &self,
        principal: &Principal,
        grants: &[Grant],
        id: &MemberId,
        workspace: &WorkspaceId,
    ) -> Result<Member, Problem> {
        authorize(principal, grants, workspace, Action::Read)?;
        let member = self
            .members
            .get(id)
            .ok_or_else(|| problem("member_not_found", "Member not found", 404))?;
        if &member.workspace_id != workspace {
            return Err(problem("member_forbidden", "Member access denied", 403));
        }
        Ok(member.clone())
    }

    pub fn can_manage(role: MemberRole) -> bool {
        matches!(role, MemberRole::Owner | MemberRole::Admin)
    }
}

fn authorize(
    principal: &Principal,
    grants: &[Grant],
    workspace: &WorkspaceId,
    action: Action,
) -> Result<(), Problem> {
    let capability = Capability::new(CAPABILITY)
        .map_err(|_| problem("invalid_capability", "Invalid capability", 500))?;
    if is_allowed(
        principal,
        &capability,
        action,
        &ResourceScope::workspace(workspace.clone()),
        grants,
    ) {
        Ok(())
    } else {
        Err(problem("member_forbidden", "Member access denied", 403))
    }
}

fn role_rank(role: &MemberRole) -> u8 {
    match role {
        MemberRole::Member => 0,
        MemberRole::Operator => 1,
        MemberRole::Admin => 2,
        MemberRole::Owner => 3,
    }
}

fn problem(code: &str, title: &str, status: u16) -> Problem {
    Problem::new(code, title, status, "member")
}
