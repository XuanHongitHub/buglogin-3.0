use serde::{Deserialize, Serialize};

use std::collections::{BTreeMap, Bound};

use buglogin_domain::{
    errors::Problem,
    ids::{AccountId, MemberId, TeamId, WorkspaceId},
    member::{Member, MemberRole, MemberStatus},
    permissions::{Action, Capability, Grant, Principal, ResourceScope, is_allowed},
};

const CAPABILITY: &str = "CAP-MEM-001";

#[derive(Clone, Default, Deserialize, Serialize)]
pub struct MemberService {
    members: BTreeMap<MemberId, Member>,
}

impl MemberService {
    pub fn active_in_workspace(&self, id: &MemberId, workspace_id: &WorkspaceId) -> bool {
        self.members.get(id).is_some_and(|member| {
            &member.workspace_id == workspace_id && member.status == MemberStatus::Active
        })
    }

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

    pub fn list_page(
        &self,
        principal: &Principal,
        grants: &[Grant],
        workspace: &WorkspaceId,
        after: Option<&MemberId>,
        limit: usize,
    ) -> Result<(Vec<Member>, Option<MemberId>), Problem> {
        if !(1..=100).contains(&limit) {
            return Err(problem(
                "invalid_page_limit",
                "Page limit must be between 1 and 100",
                400,
            ));
        }
        authorize(principal, grants, workspace, Action::Read)?;
        let lower = after.map_or(Bound::Unbounded, Bound::Excluded);
        let mut rows = self
            .members
            .range((lower, Bound::Unbounded))
            .filter(|(_, member)| &member.workspace_id == workspace)
            .map(|(_, member)| member);
        let items = rows.by_ref().take(limit).cloned().collect::<Vec<_>>();
        let has_more = rows.next().is_some();
        let next_cursor = if has_more {
            items.last().map(|member| member.id.clone())
        } else {
            None
        };
        Ok((items, next_cursor))
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
        self.ensure_role_assignment_allowed(principal, &member.workspace_id, &member.role)?;
        if self.members.values().any(|existing| {
            existing.account_id == member.account_id
                && existing.workspace_id == member.workspace_id
                && existing.status == MemberStatus::Active
        }) {
            return Err(problem(
                "member_workspace_membership_exists",
                "Account already has an active membership in this workspace",
                409,
            ));
        }
        if self.members.contains_key(&member.id) {
            return Err(problem("member_exists", "Member exists", 409));
        }
        self.members.insert(member.id.clone(), member);
        Ok(())
    }

    pub fn authorize_invitation(
        &self,
        principal: &Principal,
        grants: &[Grant],
        workspace_id: &WorkspaceId,
        role: &MemberRole,
    ) -> Result<(), Problem> {
        authorize(principal, grants, workspace_id, Action::Create)?;
        self.ensure_role_assignment_allowed(principal, workspace_id, role)
    }

    pub fn authorize_invitation_revocation(
        &self,
        principal: &Principal,
        grants: &[Grant],
        workspace_id: &WorkspaceId,
        role: &MemberRole,
    ) -> Result<(), Problem> {
        authorize(principal, grants, workspace_id, Action::Manage)?;
        self.ensure_actor_can_manage_role(principal, workspace_id, role)
    }

    /// Adds the pending membership after the API has authenticated and verified
    /// the invitee and consumed the matching single-use invitation code.
    pub fn redeem_invitation(
        &mut self,
        workspace_id: WorkspaceId,
        account_id: AccountId,
        member_id: MemberId,
        role: MemberRole,
        expires_at: u64,
        now: u64,
    ) -> Result<Member, Problem> {
        if expires_at <= now || matches!(&role, MemberRole::Owner) {
            return Err(problem("invalid_invite", "Invalid invite", 400));
        }
        if self.members.values().any(|existing| {
            existing.account_id == account_id
                && existing.workspace_id == workspace_id
                && matches!(
                    &existing.status,
                    MemberStatus::Active | MemberStatus::Invited
                )
        }) {
            return Err(problem(
                "member_workspace_membership_exists",
                "Account already has a membership in this workspace",
                409,
            ));
        }
        if self.members.contains_key(&member_id) {
            return Err(problem("member_exists", "Member exists", 409));
        }
        let member = Member {
            id: member_id,
            account_id,
            workspace_id,
            team_id: None,
            role,
            status: MemberStatus::Invited,
            invite_expires_at: Some(expires_at),
        };
        self.members.insert(member.id.clone(), member.clone());
        Ok(member)
    }

    pub fn activate(
        &mut self,
        principal: &Principal,
        grants: &[Grant],
        workspace_id: &WorkspaceId,
        id: &MemberId,
        now: u64,
    ) -> Result<(), Problem> {
        let self_accept = self.members.get(id).is_some_and(|member| {
            &member.workspace_id == workspace_id
                && (principal.account_id.as_ref() == Some(&member.account_id)
                    || principal.member_id.as_ref() == Some(id))
        });
        if !self_accept {
            authorize(principal, grants, workspace_id, Action::Update)?;
        }
        let member = self
            .members
            .get(id)
            .filter(|member| &member.workspace_id == workspace_id)
            .ok_or_else(|| problem("member_not_found", "Member not found", 404))?;
        if member.status != MemberStatus::Invited
            || member
                .invite_expires_at
                .is_none_or(|expires| expires <= now)
        {
            return Err(problem("invite_expired", "Invite expired", 409));
        }
        if self.members.values().any(|existing| {
            existing.id != *id
                && existing.account_id == member.account_id
                && existing.workspace_id == member.workspace_id
                && existing.status == MemberStatus::Active
        }) {
            return Err(problem(
                "member_workspace_membership_exists",
                "Account already has an active membership in this workspace",
                409,
            ));
        }
        let member = self.members.get_mut(id).expect("member checked above");
        member.status = MemberStatus::Active;
        member.invite_expires_at = None;
        Ok(())
    }

    pub fn suspend(
        &mut self,
        principal: &Principal,
        grants: &[Grant],
        workspace_id: &WorkspaceId,
        id: &MemberId,
    ) -> Result<(), Problem> {
        let member = self.validate_suspend(principal, grants, workspace_id, id)?;
        self.members
            .get_mut(&member.id)
            .expect("member was validated above")
            .status = MemberStatus::Suspended;
        Ok(())
    }

    pub fn validate_suspend(
        &self,
        principal: &Principal,
        grants: &[Grant],
        workspace_id: &WorkspaceId,
        id: &MemberId,
    ) -> Result<Member, Problem> {
        authorize(principal, grants, workspace_id, Action::Manage)?;
        let member = self
            .members
            .get(id)
            .filter(|member| &member.workspace_id == workspace_id)
            .ok_or_else(|| problem("member_not_found", "Member not found", 404))?;
        self.ensure_actor_can_manage_role(principal, &member.workspace_id, &member.role)?;
        if member.status == MemberStatus::Active && member.role == MemberRole::Owner {
            self.ensure_workspace_owner_remains(member)?;
        }
        Ok(member.clone())
    }

    pub fn set_role(
        &mut self,
        principal: &Principal,
        grants: &[Grant],
        workspace_id: &WorkspaceId,
        id: &MemberId,
        role: MemberRole,
    ) -> Result<(), Problem> {
        authorize(principal, grants, workspace_id, Action::Manage)?;
        let target = self
            .members
            .get(id)
            .filter(|member| &member.workspace_id == workspace_id)
            .ok_or_else(|| problem("member_not_found", "Member not found", 404))?;
        self.ensure_role_assignment_allowed(principal, &target.workspace_id, &role)?;
        self.ensure_actor_can_manage_role(principal, &target.workspace_id, &target.role)?;
        if target.status == MemberStatus::Active
            && target.role == MemberRole::Owner
            && role != MemberRole::Owner
        {
            self.ensure_workspace_owner_remains(target)?;
        }
        self.members.get_mut(id).expect("member checked above").role = role;
        Ok(())
    }

    fn ensure_role_assignment_allowed(
        &self,
        principal: &Principal,
        workspace_id: &WorkspaceId,
        requested_role: &MemberRole,
    ) -> Result<(), Problem> {
        if let Some(actor_role) = self.actor_role_in_workspace(principal, workspace_id)?
            && role_rank(requested_role) > role_rank(&actor_role)
        {
            return Err(problem(
                "role_escalation",
                "Role assignment exceeds actor authority",
                403,
            ));
        }
        Ok(())
    }

    fn ensure_actor_can_manage_role(
        &self,
        principal: &Principal,
        workspace_id: &WorkspaceId,
        target_role: &MemberRole,
    ) -> Result<(), Problem> {
        if let Some(actor_role) = self.actor_role_in_workspace(principal, workspace_id)?
            && role_rank(target_role) > role_rank(&actor_role)
        {
            return Err(problem(
                "role_escalation",
                "Target role exceeds actor authority",
                403,
            ));
        }
        Ok(())
    }

    fn actor_role_in_workspace(
        &self,
        principal: &Principal,
        workspace_id: &WorkspaceId,
    ) -> Result<Option<MemberRole>, Problem> {
        if principal
            .roles
            .contains(&buglogin_domain::permissions::Role::PlatformAdmin)
        {
            return Ok(None);
        }
        let actor = if let Some(account_id) = principal.account_id.as_ref() {
            let mut candidates = self.members.values().filter(|member| {
                member.account_id == *account_id
                    && member.workspace_id == *workspace_id
                    && member.status == MemberStatus::Active
            });
            let actor = candidates.next().ok_or_else(|| {
                problem("role_escalation", "Actor membership is unavailable", 403)
            })?;
            if candidates.next().is_some() {
                return Err(problem(
                    "role_escalation",
                    "Actor has multiple active memberships in this workspace",
                    403,
                ));
            }
            actor
        } else {
            principal
                .member_id
                .as_ref()
                .and_then(|actor_id| self.members.get(actor_id))
                .filter(|actor| {
                    actor.workspace_id == *workspace_id && actor.status == MemberStatus::Active
                })
                .ok_or_else(|| problem("role_escalation", "Actor membership is unavailable", 403))?
        };
        Ok(Some(actor.role.clone()))
    }

    fn ensure_workspace_owner_remains(&self, target: &Member) -> Result<(), Problem> {
        let another_active_owner = self.members.values().any(|member| {
            member.id != target.id
                && member.workspace_id == target.workspace_id
                && member.status == MemberStatus::Active
                && member.role == MemberRole::Owner
        });
        if another_active_owner {
            Ok(())
        } else {
            Err(problem(
                "last_workspace_owner",
                "The last active workspace owner cannot be suspended or demoted",
                409,
            ))
        }
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
        matches!(
            role,
            MemberRole::Owner | MemberRole::Admin | MemberRole::Lead
        )
    }

    pub(super) fn team_assignment_members(&self, workspace_id: &WorkspaceId) -> Vec<Member> {
        self.members
            .values()
            .filter(|member| &member.workspace_id == workspace_id)
            .cloned()
            .collect()
    }

    pub(super) fn replace_team_ids(
        &mut self,
        principal: &Principal,
        grants: &[Grant],
        workspace_id: &WorkspaceId,
        assignments: &BTreeMap<MemberId, Option<TeamId>>,
    ) -> Result<(), Problem> {
        authorize(principal, grants, workspace_id, Action::Manage)?;

        for member_id in assignments.keys() {
            let member = self
                .members
                .get(member_id)
                .filter(|member| &member.workspace_id == workspace_id)
                .ok_or_else(|| problem("member_not_found", "Member not found", 404))?;
            self.ensure_actor_can_manage_role(principal, workspace_id, &member.role)?;
        }

        for (member_id, team_id) in assignments {
            self.members
                .get_mut(member_id)
                .expect("team assignment target was validated")
                .team_id = team_id.clone();
        }
        Ok(())
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
        MemberRole::Lead => 2,
        MemberRole::Admin => 3,
        MemberRole::Owner => 4,
    }
}

fn problem(code: &str, title: &str, status: u16) -> Problem {
    Problem::new(code, title, status, "member")
}

#[cfg(test)]
mod tests {
    use super::MemberService;
    use buglogin_domain::{
        ids::{AccountId, MemberId, WorkspaceId},
        member::{Member, MemberRole, MemberStatus},
        permissions::{Action, Capability, Effect, Grant, Principal, ResourceScope, Role},
    };
    use std::collections::BTreeSet;

    fn invited_member(workspace_id: WorkspaceId) -> Member {
        Member {
            id: MemberId::new("mem_scope_guard").unwrap(),
            account_id: AccountId::new("acct_scope_guard").unwrap(),
            workspace_id,
            team_id: None,
            role: MemberRole::Member,
            status: MemberStatus::Invited,
            invite_expires_at: Some(100),
        }
    }

    #[test]
    fn member_mutations_authorize_workspace_before_lookup_and_bind_target_to_path() {
        let actual_workspace = WorkspaceId::new("ws_scope_guard_actual").unwrap();
        let requested_workspace = WorkspaceId::new("ws_scope_guard_other").unwrap();
        let member_id = MemberId::new("mem_scope_guard").unwrap();
        let missing_id = MemberId::new("mem_scope_guard_missing").unwrap();
        let platform_admin = Principal {
            roles: BTreeSet::from([Role::PlatformAdmin]),
            ..Principal::default()
        };
        let mut members = MemberService::default();
        members
            .invite(
                &platform_admin,
                &[],
                invited_member(actual_workspace.clone()),
                1,
            )
            .unwrap();

        for id in [&member_id, &missing_id] {
            let error = members
                .activate(&Principal::default(), &[], &actual_workspace, id, 2)
                .unwrap_err();
            assert_eq!(error.code, "member_forbidden");
            assert_eq!(error.status, 403);
        }

        assert_eq!(
            members
                .activate(&platform_admin, &[], &requested_workspace, &member_id, 2)
                .unwrap_err()
                .code,
            "member_not_found"
        );
        assert_eq!(
            members
                .suspend(&platform_admin, &[], &requested_workspace, &member_id)
                .unwrap_err()
                .code,
            "member_not_found"
        );
        assert_eq!(
            members
                .set_role(
                    &platform_admin,
                    &[],
                    &requested_workspace,
                    &member_id,
                    MemberRole::Operator,
                )
                .unwrap_err()
                .code,
            "member_not_found"
        );
        let unchanged = members
            .get_in_workspace(&platform_admin, &[], &member_id, &actual_workspace)
            .unwrap();
        assert_eq!(unchanged.status, MemberStatus::Invited);
        assert_eq!(unchanged.role, MemberRole::Member);
    }

    #[test]
    fn lead_manages_subordinate_roles_but_cannot_assign_admin_or_owner() {
        let workspace = WorkspaceId::new("ws_lead_scope").unwrap();
        let lead_account = AccountId::new("acct_lead_scope").unwrap();
        let lead_id = MemberId::new("mem_lead_scope").unwrap();
        let target_id = MemberId::new("mem_target_scope").unwrap();
        let platform_admin = Principal {
            roles: BTreeSet::from([Role::PlatformAdmin]),
            ..Principal::default()
        };
        let mut members = MemberService::default();
        let lead = Member {
            id: lead_id,
            account_id: lead_account.clone(),
            workspace_id: workspace.clone(),
            team_id: None,
            role: MemberRole::Lead,
            status: MemberStatus::Invited,
            invite_expires_at: Some(100),
        };
        let target = Member {
            id: target_id.clone(),
            account_id: AccountId::new("acct_target_scope").unwrap(),
            workspace_id: workspace.clone(),
            team_id: None,
            role: MemberRole::Member,
            status: MemberStatus::Invited,
            invite_expires_at: Some(100),
        };
        members.invite(&platform_admin, &[], lead, 1).unwrap();
        members.invite(&platform_admin, &[], target, 1).unwrap();
        members
            .activate(
                &platform_admin,
                &[],
                &workspace,
                &MemberId::new("mem_lead_scope").unwrap(),
                2,
            )
            .unwrap();
        members
            .activate(&platform_admin, &[], &workspace, &target_id, 2)
            .unwrap();

        let lead_principal = Principal {
            account_id: Some(lead_account),
            workspace_ids: BTreeSet::from([workspace.clone()]),
            ..Principal::default()
        };
        let manage_members = Grant {
            id: buglogin_domain::ids::GrantId::new("grant_lead_member_manage").unwrap(),
            account_id: None,
            member_id: None,
            capability: Capability::new("CAP-MEM-001").unwrap(),
            action: Action::Manage,
            scope: ResourceScope::workspace(workspace.clone()),
            effect: Effect::Allow,
        };
        assert!(MemberService::can_manage(MemberRole::Lead));
        members
            .set_role(
                &lead_principal,
                std::slice::from_ref(&manage_members),
                &workspace,
                &target_id,
                MemberRole::Operator,
            )
            .unwrap();
        assert_eq!(
            members
                .get_in_workspace(&platform_admin, &[], &target_id, &workspace)
                .unwrap()
                .role,
            MemberRole::Operator
        );
        for forbidden_role in [MemberRole::Admin, MemberRole::Owner] {
            assert_eq!(
                members
                    .set_role(
                        &lead_principal,
                        &[manage_members.clone()],
                        &workspace,
                        &target_id,
                        forbidden_role,
                    )
                    .unwrap_err()
                    .code,
                "role_escalation"
            );
        }
    }
}
