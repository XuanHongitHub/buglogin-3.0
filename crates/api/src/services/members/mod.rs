use serde::{Deserialize, Serialize};

use std::collections::{BTreeMap, Bound};

use buglogin_domain::{
    errors::Problem,
    ids::{AccountId, MemberId, TeamId, WorkspaceId},
    member::{Member, MemberRole, MemberStatus},
    organization::Organization,
    permissions::{Action, Capability, Grant, Principal, ResourceScope, is_allowed},
};

const CAPABILITY: &str = "CAP-MEM-001";

#[derive(Clone, Default, Deserialize, Serialize)]
pub struct MemberService {
    members: BTreeMap<MemberId, Member>,
}

impl MemberService {
    /// Remove bootstrap owner memberships only after the caller has checked that
    /// the workspace has no resources, groups or other dependencies.
    pub(crate) fn remove_owned_workspace_memberships(
        &mut self,
        workspace_id: &WorkspaceId,
        owner_id: &AccountId,
    ) -> Result<Vec<MemberId>, Problem> {
        let scoped: Vec<Member> = self.members.values().filter(|member| &member.workspace_id == workspace_id).cloned().collect();
        if scoped.iter().any(|member| &member.account_id != owner_id || member.role != MemberRole::Owner || member.team_id.is_some()) {
            return Err(problem("workspace_has_members", "Remove workspace members before deleting the workspace", 409));
        }
        let ids: Vec<MemberId> = scoped.into_iter().map(|member| member.id).collect();
        for id in &ids { self.members.remove(id); }
        Ok(ids)
    }

    /// Create an already-active membership as part of administrator-driven account provisioning.
    /// Unlike invitation redemption, this never creates an invitation that the account holder
    /// must accept. The caller must still prove the account identity separately before login.
    pub fn provision_active(
        &mut self,
        principal: &Principal,
        grants: &[Grant],
        member: Member,
    ) -> Result<Member, Problem> {
        authorize(principal, grants, &member.workspace_id, Action::Create)?;
        if member.status != MemberStatus::Active || member.invite_expires_at.is_some() {
            return Err(problem(
                "invalid_account_provisioning_membership",
                "Provisioned membership must be active and have no invitation expiry",
                400,
            ));
        }
        self.ensure_role_assignment_allowed(principal, &member.workspace_id, &member.role)?;

        self.insert_active_membership(member)
    }

    pub fn authorize_org_owner_account_creation(
        principal: &Principal,
        grants: &[Grant],
        organization: &Organization,
        workspace_id: &WorkspaceId,
        role: &MemberRole,
    ) -> Result<(), Problem> {
        authorize_org_owner(
            principal,
            grants,
            organization,
            workspace_id,
            Action::Create,
        )?;
        if role == &MemberRole::Owner {
            return Err(problem(
                "role_escalation",
                "Organization owners may assign only roles below Owner",
                403,
            ));
        }
        Ok(())
    }

    pub fn authorize_org_owner_access(
        principal: &Principal,
        grants: &[Grant],
        organization: &Organization,
        workspace_id: &WorkspaceId,
    ) -> Result<(), Problem> {
        authorize_org_owner(
            principal,
            grants,
            organization,
            workspace_id,
            Action::Manage,
        )
    }

    pub fn authorize_org_owner_member_action(
        principal: &Principal,
        grants: &[Grant],
        organization: &Organization,
        workspace_id: &WorkspaceId,
        action: Action,
    ) -> Result<(), Problem> {
        authorize_org_owner(principal, grants, organization, workspace_id, action)
    }

    pub fn provision_active_as_org_owner(
        &mut self,
        principal: &Principal,
        grants: &[Grant],
        organization: &Organization,
        member: Member,
    ) -> Result<Member, Problem> {
        Self::authorize_org_owner_account_creation(
            principal,
            grants,
            organization,
            &member.workspace_id,
            &member.role,
        )?;
        if member.status != MemberStatus::Active || member.invite_expires_at.is_some() {
            return Err(problem(
                "invalid_account_provisioning_membership",
                "Provisioned membership must be active and have no invitation expiry",
                400,
            ));
        }
        self.insert_active_membership(member)
    }

    /// Assign an existing active organization member directly to a workspace.
    /// The caller must first verify the parent membership and organization/workspace
    /// relationship; this method preserves the normal workspace role ceiling.
    pub fn assign_active_as_org_owner(
        &mut self,
        principal: &Principal,
        grants: &[Grant],
        organization: &Organization,
        member: Member,
    ) -> Result<Member, Problem> {
        Self::authorize_org_owner_account_creation(
            principal,
            grants,
            organization,
            &member.workspace_id,
            &member.role,
        )?;
        if member.status != MemberStatus::Active || member.invite_expires_at.is_some() {
            return Err(problem(
                "invalid_workspace_assignment",
                "Workspace assignment must be active and have no invitation expiry",
                400,
            ));
        }
        self.insert_active_membership(member)
    }

    pub fn list_as_org_owner(
        &self,
        principal: &Principal,
        grants: &[Grant],
        organization: &Organization,
        workspace_id: &WorkspaceId,
    ) -> Result<Vec<Member>, Problem> {
        authorize_org_owner(principal, grants, organization, workspace_id, Action::Read)?;
        Ok(self
            .members
            .values()
            .filter(|member| &member.workspace_id == workspace_id)
            .cloned()
            .collect())
    }

    pub fn list_page_as_org_owner(
        &self,
        principal: &Principal,
        grants: &[Grant],
        organization: &Organization,
        workspace_id: &WorkspaceId,
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
        authorize_org_owner(principal, grants, organization, workspace_id, Action::Read)?;
        let lower = after.map_or(Bound::Unbounded, Bound::Excluded);
        let mut rows = self
            .members
            .range((lower, Bound::Unbounded))
            .filter(|(_, member)| &member.workspace_id == workspace_id)
            .map(|(_, member)| member);
        let items = rows.by_ref().take(limit).cloned().collect::<Vec<_>>();
        let next_cursor = if rows.next().is_some() {
            items.last().map(|member| member.id.clone())
        } else {
            None
        };
        Ok((items, next_cursor))
    }

    fn insert_active_membership(&mut self, member: Member) -> Result<Member, Problem> {
        if let Some(existing) = self.members.values().find(|existing| {
            existing.account_id == member.account_id && existing.workspace_id == member.workspace_id
        }) {
            if existing.id == member.id
                && existing.role == member.role
                && existing.status == MemberStatus::Active
            {
                return Ok(existing.clone());
            }
            return Err(problem(
                "member_workspace_membership_exists",
                "Account already has a membership in this workspace",
                409,
            ));
        }
        if self.members.contains_key(&member.id) {
            return Err(problem("member_exists", "Member exists", 409));
        }
        self.members.insert(member.id.clone(), member.clone());
        Ok(member)
    }

    /// Restore the organization owner's workspace-owner membership when a
    /// legacy workspace has no active owner. This is an idempotent control
    /// state backfill; an existing active owner is always preserved.
    pub fn backfill_workspace_owners(&mut self, candidates: Vec<Member>) -> usize {
        let mut changed = 0;
        for mut candidate in candidates {
            if self.members.values().any(|member| {
                member.workspace_id == candidate.workspace_id
                    && member.role == MemberRole::Owner
                    && member.status == MemberStatus::Active
            }) {
                continue;
            }
            let existing = self.members.values().find(|member| {
                member.workspace_id == candidate.workspace_id
                    && member.account_id == candidate.account_id
            });
            if let Some(existing) = existing {
                if existing.status != MemberStatus::Active {
                    continue;
                }
                if existing.role == MemberRole::Owner {
                    continue;
                }
                let id = existing.id.clone();
                if let Some(existing) = self.members.get_mut(&id) {
                    existing.role = MemberRole::Owner;
                    changed += 1;
                }
                continue;
            }
            if self.members.contains_key(&candidate.id) {
                continue;
            }
            candidate.role = MemberRole::Owner;
            candidate.status = MemberStatus::Active;
            candidate.invite_expires_at = None;
            self.members.insert(candidate.id.clone(), candidate);
            changed += 1;
        }
        changed
    }

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

    pub fn all_members(&self) -> Vec<Member> {
        self.members.values().cloned().collect()
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
        let is_self = self.members.get(id).is_some_and(|member| {
            &member.workspace_id == workspace_id
                && (principal.account_id.as_ref() == Some(&member.account_id)
                    || principal.member_id.as_ref() == Some(id))
        });
        let suspended_self = is_self
            && self
                .members
                .get(id)
                .is_some_and(|member| member.status == MemberStatus::Suspended);
        if suspended_self {
            return Err(problem(
                "member_forbidden",
                "A suspended member cannot reactivate their own membership",
                403,
            ));
        }
        let self_accept = is_self
            && self
                .members
                .get(id)
                .is_some_and(|member| member.status == MemberStatus::Invited);
        if !self_accept {
            authorize(principal, grants, workspace_id, Action::Manage)?;
        }
        let member = self
            .members
            .get(id)
            .filter(|member| &member.workspace_id == workspace_id)
            .ok_or_else(|| problem("member_not_found", "Member not found", 404))?;
        if !self_accept {
            self.ensure_actor_can_manage_role(principal, workspace_id, &member.role)?;
        }
        if member.status == MemberStatus::Invited
            && member
                .invite_expires_at
                .is_none_or(|expires| expires <= now)
        {
            return Err(problem("invite_expired", "Invite expired", 409));
        }
        if member.status == MemberStatus::Active {
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

    /// Permanently remove one workspace membership after the same role/owner
    /// safeguards used by suspension. Callers must revoke scoped references
    /// and persist the enclosing control aggregate in the same transaction.
    pub fn remove(
        &mut self,
        principal: &Principal,
        grants: &[Grant],
        workspace_id: &WorkspaceId,
        id: &MemberId,
    ) -> Result<Member, Problem> {
        self.validate_suspend(principal, grants, workspace_id, id)?;
        self.members
            .remove(id)
            .ok_or_else(|| problem("member_not_found", "Member not found", 404))
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
            && role_rank(requested_role) >= role_rank(&actor_role)
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
            && role_rank(target_role) >= role_rank(&actor_role)
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

fn authorize_org_owner(
    principal: &Principal,
    grants: &[Grant],
    organization: &Organization,
    workspace_id: &WorkspaceId,
    member_action: Action,
) -> Result<(), Problem> {
    if principal.token_scopes.as_ref().is_some_and(|scopes| {
        let action_name = match member_action {
            Action::Read => "read",
            Action::Create => "create",
            Action::Update => "update",
            Action::Delete => "delete",
            Action::Execute => "execute",
            Action::Share => "share",
            Action::Manage => "manage",
        };
        !scopes.contains(&format!("{CAPABILITY}:{action_name}"))
            && !scopes.contains(&format!("{CAPABILITY}:*"))
    }) {
        return Err(problem("member_forbidden", "Member access denied", 403));
    }
    if principal.account_id.as_ref() != Some(&organization.account_id) {
        return Err(problem(
            "member_forbidden",
            "Organization owner access denied",
            403,
        ));
    }
    let capability = Capability::new("CAP-ORG-001")
        .map_err(|_| problem("invalid_capability", "Invalid capability", 500))?;
    let scope = ResourceScope {
        account_id: Some(organization.account_id.clone()),
        organization_id: Some(organization.id.clone()),
        ..ResourceScope::default()
    };
    let member_capability = Capability::new(CAPABILITY)
        .map_err(|_| problem("invalid_capability", "Invalid capability", 500))?;
    let member_scope = ResourceScope {
        account_id: Some(organization.account_id.clone()),
        organization_id: Some(organization.id.clone()),
        workspace_id: Some(workspace_id.clone()),
        ..ResourceScope::default()
    };
    let explicit_member_deny = grants.iter().any(|grant| {
        grant.capability == member_capability
            && grant.action == member_action
            && grant.effect == buglogin_domain::permissions::Effect::Deny
            && grant.scope.contains(&member_scope)
            && grant
                .account_id
                .as_ref()
                .is_none_or(|id| principal.account_id.as_ref() == Some(id))
            && grant
                .member_id
                .as_ref()
                .is_none_or(|id| principal.member_id.as_ref() == Some(id))
    });
    if explicit_member_deny {
        return Err(problem("member_forbidden", "Member access denied", 403));
    }
    if principal.token_scopes.as_ref().is_some_and(|scopes| {
        !scopes.contains("CAP-ORG-001:manage") && !scopes.contains("CAP-ORG-001:*")
    }) {
        return Err(problem(
            "member_forbidden",
            "Organization owner access denied",
            403,
        ));
    }
    let explicit_organization_deny = grants.iter().any(|grant| {
        grant.capability == capability
            && grant.action == Action::Manage
            && grant.effect == buglogin_domain::permissions::Effect::Deny
            && grant.scope.contains(&scope)
            && grant
                .account_id
                .as_ref()
                .is_none_or(|id| principal.account_id.as_ref() == Some(id))
            && grant
                .member_id
                .as_ref()
                .is_none_or(|id| principal.member_id.as_ref() == Some(id))
    });
    if explicit_organization_deny {
        return Err(problem(
            "member_forbidden",
            "Organization owner access denied",
            403,
        ));
    }
    // Organization ownership is implicit authority, matching
    // WorkspaceService::authorize_organization_management. Keep explicit
    // organization and member denies, and scoped-token checks, authoritative.
    Ok(())
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
    fn removal_is_scoped_authorized_and_keeps_the_last_active_owner() {
        let workspace = WorkspaceId::new("ws_remove_scope").unwrap();
        let other = WorkspaceId::new("ws_remove_other").unwrap();
        let platform = Principal { roles: BTreeSet::from([Role::PlatformAdmin]), ..Principal::default() };
        let mut service = MemberService::default();
        let mut target = invited_member(workspace.clone());
        target.status = MemberStatus::Active;
        target.invite_expires_at = None;
        service.provision_active(&platform, &[], target.clone()).unwrap();
        assert_eq!(service.remove(&Principal::default(), &[], &workspace, &target.id).unwrap_err().status, 403);
        assert_eq!(service.remove(&platform, &[], &other, &target.id).unwrap_err().code, "member_not_found");
        assert!(service.active_in_workspace(&target.id, &workspace));
        assert_eq!(service.remove(&platform, &[], &workspace, &target.id).unwrap(), target);
        assert!(!service.active_in_workspace(&target.id, &workspace));

        let mut owner = target;
        owner.role = MemberRole::Owner;
        service.provision_active(&platform, &[], owner.clone()).unwrap();
        assert_eq!(service.remove(&platform, &[], &workspace, &owner.id).unwrap_err().code, "last_workspace_owner");
        assert!(service.active_in_workspace(&owner.id, &workspace));
    }

    #[test]
    fn workspace_owner_backfill_is_idempotent_and_preserves_existing_owners() {
        let workspace_id = WorkspaceId::new("ws_owner_backfill").unwrap();
        let owner = Member {
            id: MemberId::new("mem_owner_backfill").unwrap(),
            account_id: AccountId::new("acct_owner_backfill").unwrap(),
            workspace_id: workspace_id.clone(),
            team_id: None,
            role: MemberRole::Owner,
            status: MemberStatus::Active,
            invite_expires_at: None,
        };
        let mut service = MemberService::default();

        assert_eq!(service.backfill_workspace_owners(vec![owner.clone()]), 1);
        assert_eq!(service.backfill_workspace_owners(vec![owner.clone()]), 0);
        assert_eq!(service.active_for_account(&owner.account_id), vec![owner]);
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
    fn suspended_members_can_be_restored_by_managers_but_not_by_themselves() {
        let workspace = WorkspaceId::new("ws_member_restore").unwrap();
        let member = Member {
            id: MemberId::new("mem_member_restore").unwrap(),
            account_id: AccountId::new("acct_member_restore").unwrap(),
            workspace_id: workspace.clone(),
            team_id: None,
            role: MemberRole::Member,
            status: MemberStatus::Active,
            invite_expires_at: None,
        };
        let platform_admin = Principal {
            roles: BTreeSet::from([Role::PlatformAdmin]),
            ..Principal::default()
        };
        let mut members = MemberService::default();
        members
            .provision_active(&platform_admin, &[], member.clone())
            .unwrap();
        members
            .suspend(&platform_admin, &[], &workspace, &member.id)
            .unwrap();

        let suspended_member = Principal {
            account_id: Some(member.account_id.clone()),
            workspace_ids: BTreeSet::from([workspace.clone()]),
            ..Principal::default()
        };
        let self_restore = members
            .activate(&suspended_member, &[], &workspace, &member.id, 10)
            .unwrap_err();
        assert_eq!(self_restore.code, "member_forbidden");
        assert_eq!(self_restore.status, 403);

        members
            .activate(&platform_admin, &[], &workspace, &member.id, 10)
            .unwrap();
        assert_eq!(
            members
                .get_in_workspace(&platform_admin, &[], &member.id, &workspace)
                .unwrap()
                .status,
            MemberStatus::Active
        );
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

        let elevated_target_id = MemberId::new("mem_elevated_pending").unwrap();
        members
            .invite(
                &platform_admin,
                &[],
                Member {
                    id: elevated_target_id.clone(),
                    account_id: AccountId::new("acct_elevated_pending").unwrap(),
                    workspace_id: workspace.clone(),
                    team_id: None,
                    role: MemberRole::Admin,
                    status: MemberStatus::Invited,
                    invite_expires_at: Some(100),
                },
                3,
            )
            .unwrap();
        assert_eq!(
            members
                .activate(
                    &lead_principal,
                    &[manage_members],
                    &workspace,
                    &elevated_target_id,
                    4,
                )
                .unwrap_err()
                .code,
            "role_escalation"
        );
    }

    #[test]
    fn account_provisioning_activates_only_lower_roles_and_is_retry_safe() {
        let workspace = WorkspaceId::new("ws_provision_scope").unwrap();
        let platform_admin = Principal {
            roles: BTreeSet::from([Role::PlatformAdmin]),
            ..Principal::default()
        };
        let lead_account = AccountId::new("acct_provision_lead").unwrap();
        let mut members = MemberService::default();
        members
            .provision_active(
                &platform_admin,
                &[],
                Member {
                    id: MemberId::new("mem_provision_lead").unwrap(),
                    account_id: lead_account.clone(),
                    workspace_id: workspace.clone(),
                    team_id: None,
                    role: MemberRole::Lead,
                    status: MemberStatus::Active,
                    invite_expires_at: None,
                },
            )
            .unwrap();
        let lead = Principal {
            account_id: Some(lead_account),
            workspace_ids: BTreeSet::from([workspace.clone()]),
            ..Principal::default()
        };
        let grants = [
            Grant {
                id: buglogin_domain::ids::GrantId::new("grant_provision_create").unwrap(),
                account_id: None,
                member_id: None,
                capability: Capability::new("CAP-MEM-001").unwrap(),
                action: Action::Create,
                scope: ResourceScope::workspace(workspace.clone()),
                effect: Effect::Allow,
            },
            Grant {
                id: buglogin_domain::ids::GrantId::new("grant_provision_manage").unwrap(),
                account_id: None,
                member_id: None,
                capability: Capability::new("CAP-MEM-001").unwrap(),
                action: Action::Manage,
                scope: ResourceScope::workspace(workspace.clone()),
                effect: Effect::Allow,
            },
        ];
        let requested = Member {
            id: MemberId::new("mem_provision_operator").unwrap(),
            account_id: AccountId::new("acct_provision_operator").unwrap(),
            workspace_id: workspace.clone(),
            team_id: None,
            role: MemberRole::Operator,
            status: MemberStatus::Active,
            invite_expires_at: None,
        };
        let created = members
            .provision_active(&lead, &grants, requested.clone())
            .unwrap();
        assert_eq!(created, requested);
        assert_eq!(
            members
                .provision_active(&lead, &grants, requested.clone())
                .unwrap(),
            requested
        );

        for role in [MemberRole::Lead, MemberRole::Admin, MemberRole::Owner] {
            let error = members
                .provision_active(
                    &lead,
                    &grants,
                    Member {
                        id: MemberId::new(format!("mem_forbidden_{role:?}").to_lowercase())
                            .unwrap(),
                        account_id: AccountId::new(
                            format!("acct_forbidden_{role:?}").to_lowercase(),
                        )
                        .unwrap(),
                        workspace_id: workspace.clone(),
                        team_id: None,
                        role,
                        status: MemberStatus::Active,
                        invite_expires_at: None,
                    },
                )
                .unwrap_err();
            assert_eq!(error.code, "role_escalation");
        }
    }

    #[test]
    fn organization_owner_can_list_and_assign_lower_workspace_members() {
        use buglogin_domain::{
            ids::{GrantId, OrganizationId},
            organization::Organization,
        };

        let organization = Organization {
            id: OrganizationId::new("org_owner_scope").unwrap(),
            account_id: AccountId::new("acct_org_owner_scope").unwrap(),
            name: "Owner Org".into(),
            created_at: "2026-10-24T00:00:00Z".into(),
        };
        let workspace = WorkspaceId::new("ws_owner_scope").unwrap();
        let owner = Principal {
            account_id: Some(organization.account_id.clone()),
            ..Principal::default()
        };
        let organization_manage = Grant {
            id: GrantId::new("grant_org_owner_manage").unwrap(),
            account_id: Some(organization.account_id.clone()),
            member_id: None,
            capability: Capability::new("CAP-ORG-001").unwrap(),
            action: Action::Manage,
            scope: ResourceScope {
                account_id: Some(organization.account_id.clone()),
                organization_id: Some(organization.id.clone()),
                ..ResourceScope::default()
            },
            effect: Effect::Allow,
        };
        let mut members = MemberService::default();
        let created = members
            .assign_active_as_org_owner(
                &owner,
                std::slice::from_ref(&organization_manage),
                &organization,
                Member {
                    id: MemberId::new("mem_org_owner_created").unwrap(),
                    account_id: AccountId::new("acct_org_owner_created").unwrap(),
                    workspace_id: workspace.clone(),
                    team_id: None,
                    role: MemberRole::Admin,
                    status: MemberStatus::Active,
                    invite_expires_at: None,
                },
            )
            .unwrap();
        assert_eq!(created.role, MemberRole::Admin);
        assert_eq!(
            members
                .assign_active_as_org_owner(
                    &owner,
                    std::slice::from_ref(&organization_manage),
                    &organization,
                    created.clone(),
                )
                .unwrap(),
            created
        );
        assert_eq!(
            members
                .list_as_org_owner(
                    &owner,
                    &[organization_manage.clone()],
                    &organization,
                    &workspace,
                )
                .unwrap()
                .len(),
            1
        );

        let mut owner_assignment = created.clone();
        owner_assignment.id = MemberId::new("mem_org_owner_escalation").unwrap();
        owner_assignment.account_id = AccountId::new("acct_org_owner_escalation").unwrap();
        owner_assignment.role = MemberRole::Owner;
        assert_eq!(
            members
                .assign_active_as_org_owner(
                    &owner,
                    &[organization_manage.clone()],
                    &organization,
                    owner_assignment,
                )
                .unwrap_err()
                .code,
            "role_escalation"
        );

        let explicit_deny = Grant {
            id: GrantId::new("grant_org_owner_member_deny").unwrap(),
            account_id: Some(organization.account_id.clone()),
            member_id: None,
            capability: Capability::new("CAP-MEM-001").unwrap(),
            action: Action::Create,
            scope: ResourceScope::workspace(workspace.clone()),
            effect: Effect::Deny,
        };
        assert_eq!(
            MemberService::authorize_org_owner_account_creation(
                &owner,
                &[organization_manage.clone(), explicit_deny],
                &organization,
                &workspace,
                &MemberRole::Admin,
            )
            .unwrap_err()
            .status,
            403
        );

        let non_owner = Principal {
            account_id: Some(AccountId::new("acct_not_org_owner").unwrap()),
            ..Principal::default()
        };
        assert_eq!(
            members
                .list_as_org_owner(
                    &non_owner,
                    &[organization_manage],
                    &organization,
                    &workspace,
                )
                .unwrap_err()
                .status,
            403
        );
    }

    #[test]
    fn organization_owner_identity_needs_no_org_grant_but_honors_explicit_deny() {
        use buglogin_domain::{
            ids::{GrantId, OrganizationId},
            organization::Organization,
        };

        let organization = Organization {
            id: OrganizationId::new("org_owner_implicit_access").unwrap(),
            account_id: AccountId::new("acct_org_owner_implicit_access").unwrap(),
            name: "Implicit Owner Org".into(),
            created_at: "2026-10-24T00:00:00Z".into(),
        };
        let workspace = WorkspaceId::new("ws_owner_implicit_access").unwrap();
        let owner = Principal {
            account_id: Some(organization.account_id.clone()),
            roles: BTreeSet::from([Role::Account]),
            ..Principal::default()
        };

        assert!(
            MemberService::authorize_org_owner_account_creation(
                &owner,
                &[],
                &organization,
                &workspace,
                &MemberRole::Admin,
            )
            .is_ok()
        );

        let explicit_deny = Grant {
            id: GrantId::new("grant_org_owner_implicit_access_deny").unwrap(),
            account_id: Some(organization.account_id.clone()),
            member_id: None,
            capability: Capability::new("CAP-ORG-001").unwrap(),
            action: Action::Manage,
            scope: ResourceScope {
                account_id: Some(organization.account_id.clone()),
                organization_id: Some(organization.id.clone()),
                ..ResourceScope::default()
            },
            effect: Effect::Deny,
        };
        assert_eq!(
            MemberService::authorize_org_owner_account_creation(
                &owner,
                &[explicit_deny],
                &organization,
                &workspace,
                &MemberRole::Admin,
            )
            .unwrap_err()
            .status,
            403
        );
    }
}
