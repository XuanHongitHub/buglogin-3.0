use std::collections::{BTreeMap, BTreeSet};

use buglogin_domain::{
    errors::Problem,
    ids::{MemberId, TeamId, WorkspaceId},
    permissions::{Action, Capability, Grant, Principal, ResourceScope, is_allowed},
    team::Team,
};
use serde::{Deserialize, Serialize};

use super::members::MemberService;

const CAPABILITY: &str = "CAP-TEAM-001";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamRecord {
    pub team: Team,
    pub revision: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamUpdate {
    pub name: String,
    pub expected_revision: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamDelete {
    pub expected_revision: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TeamMembersReplace {
    pub member_ids: Vec<MemberId>,
    pub expected_revision: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct TeamService {
    teams: BTreeMap<TeamId, TeamRecord>,
}

impl TeamService {
    pub(crate) fn remove_member_references(&mut self, workspace: &WorkspaceId, member: &MemberId) {
        for record in self.teams.values_mut() {
            if &record.team.workspace_id == workspace && record.team.member_ids.contains(member) {
                record.team.member_ids.retain(|id| id != member);
                record.revision = record.revision.saturating_add(1);
            }
        }
    }

    pub(crate) fn has_workspace_records(&self, workspace_id: &WorkspaceId) -> bool {
        self.teams.values().any(|record| &record.team.workspace_id == workspace_id)
    }

    pub fn create(
        &mut self,
        principal: &Principal,
        grants: &[Grant],
        workspace_id: &WorkspaceId,
        team: Team,
    ) -> Result<TeamRecord, Problem> {
        authorize(
            principal,
            grants,
            workspace_id,
            Some(&team.id),
            Action::Create,
        )?;
        validate_team(workspace_id, &team)?;
        if self.teams.contains_key(&team.id) {
            return Err(problem("team_exists", "Team already exists", 409));
        }
        let record = TeamRecord { team, revision: 1 };
        self.teams.insert(record.team.id.clone(), record.clone());
        Ok(record)
    }

    pub fn list(
        &self,
        principal: &Principal,
        grants: &[Grant],
        workspace_id: &WorkspaceId,
    ) -> Result<Vec<TeamRecord>, Problem> {
        authorize(principal, grants, workspace_id, None, Action::Read)?;
        Ok(self
            .teams
            .values()
            .filter(|record| &record.team.workspace_id == workspace_id)
            .cloned()
            .collect())
    }

    pub fn get(
        &self,
        principal: &Principal,
        grants: &[Grant],
        workspace_id: &WorkspaceId,
        team_id: &TeamId,
    ) -> Result<TeamRecord, Problem> {
        authorize(principal, grants, workspace_id, Some(team_id), Action::Read)?;
        let record = self
            .teams
            .get(team_id)
            .ok_or_else(|| problem("team_not_found", "Team not found", 404))?;
        ensure_workspace(workspace_id, &record.team)?;
        Ok(record.clone())
    }

    pub fn update(
        &mut self,
        principal: &Principal,
        grants: &[Grant],
        workspace_id: &WorkspaceId,
        team_id: &TeamId,
        update: TeamUpdate,
    ) -> Result<TeamRecord, Problem> {
        authorize(
            principal,
            grants,
            workspace_id,
            Some(team_id),
            Action::Update,
        )?;
        validate_name(&update.name)?;
        let record = self
            .teams
            .get_mut(team_id)
            .ok_or_else(|| problem("team_not_found", "Team not found", 404))?;
        ensure_workspace(workspace_id, &record.team)?;
        check_revision(record.revision, update.expected_revision)?;
        record.revision = record
            .revision
            .checked_add(1)
            .ok_or_else(|| problem("revision_exhausted", "Revision exhausted", 409))?;
        record.team.name = update.name.trim().to_owned();
        Ok(record.clone())
    }

    /// Replaces one team's membership while keeping both persisted membership
    /// projections aligned. The HTTP aggregate mutation caller must hold its
    /// global mutation lock; this method stages clones and commits both
    /// services only after every ACL, revision, scope, and consistency check.
    pub fn replace_members(
        &mut self,
        members: &mut MemberService,
        principal: &Principal,
        grants: &[Grant],
        workspace_id: &WorkspaceId,
        team_id: &TeamId,
        input: TeamMembersReplace,
    ) -> Result<TeamRecord, Problem> {
        authorize(
            principal,
            grants,
            workspace_id,
            Some(team_id),
            Action::Update,
        )?;
        let target = self
            .teams
            .get(team_id)
            .filter(|record| &record.team.workspace_id == workspace_id)
            .cloned()
            .ok_or_else(|| problem("team_not_found", "Team not found", 404))?;
        check_revision(target.revision, input.expected_revision)?;

        let desired = input.member_ids.iter().cloned().collect::<BTreeSet<_>>();
        if desired.len() != input.member_ids.len() {
            return Err(problem(
                "duplicate_team_member",
                "Team contains duplicate members",
                400,
            ));
        }

        let workspace_teams = self
            .teams
            .values()
            .filter(|record| &record.team.workspace_id == workspace_id)
            .map(|record| record.team.clone())
            .collect::<Vec<_>>();
        let workspace_members = members.team_assignment_members(workspace_id);
        validate_membership_consistency(&workspace_teams, &workspace_members)?;

        let known_members = workspace_members
            .iter()
            .map(|member| (member.id.clone(), member))
            .collect::<BTreeMap<_, _>>();
        for member_id in &desired {
            if !known_members.contains_key(member_id) {
                return Err(problem(
                    "member_not_found",
                    "Member not found in this workspace",
                    404,
                ));
            }
        }

        let mut assignments = BTreeMap::new();
        for member in &workspace_members {
            let next_team_id = if desired.contains(&member.id) {
                Some(team_id.clone())
            } else if member.team_id.as_ref() == Some(team_id) {
                None
            } else {
                member.team_id.clone()
            };
            if member.team_id != next_team_id {
                assignments.insert(member.id.clone(), next_team_id);
            }
        }

        for (member_id, next_team_id) in &assignments {
            let previous_team_id = known_members
                .get(member_id)
                .and_then(|member| member.team_id.as_ref());
            if let Some(previous_team_id) = previous_team_id
                && Some(previous_team_id) != next_team_id.as_ref()
            {
                authorize(
                    principal,
                    grants,
                    workspace_id,
                    Some(previous_team_id),
                    Action::Update,
                )?;
            }
        }

        let mut next_members = members.clone();
        next_members.replace_team_ids(principal, grants, workspace_id, &assignments)?;

        let projected_member_teams = workspace_members
            .iter()
            .map(|member| {
                let team_id = assignments
                    .get(&member.id)
                    .cloned()
                    .unwrap_or_else(|| member.team_id.clone());
                (member.id.clone(), team_id)
            })
            .collect::<BTreeMap<_, _>>();
        let mut next_teams = self.clone();
        for record in next_teams
            .teams
            .values_mut()
            .filter(|record| &record.team.workspace_id == workspace_id)
        {
            let expected_member_ids = projected_member_teams
                .iter()
                .filter_map(|(member_id, assigned_team)| {
                    (assigned_team.as_ref() == Some(&record.team.id)).then_some(member_id.clone())
                })
                .collect::<Vec<_>>();
            if record.team.member_ids != expected_member_ids {
                record.revision = record
                    .revision
                    .checked_add(1)
                    .ok_or_else(|| problem("revision_exhausted", "Revision exhausted", 409))?;
                record.team.member_ids = expected_member_ids;
            }
        }

        let updated_target = next_teams
            .teams
            .get(team_id)
            .cloned()
            .ok_or_else(|| problem("team_not_found", "Team not found", 404))?;
        *members = next_members;
        *self = next_teams;
        Ok(updated_target)
    }

    pub fn delete(
        &mut self,
        principal: &Principal,
        grants: &[Grant],
        workspace_id: &WorkspaceId,
        team_id: &TeamId,
        input: TeamDelete,
    ) -> Result<TeamRecord, Problem> {
        authorize(
            principal,
            grants,
            workspace_id,
            Some(team_id),
            Action::Delete,
        )?;
        let record = self
            .teams
            .get(team_id)
            .ok_or_else(|| problem("team_not_found", "Team not found", 404))?;
        ensure_workspace(workspace_id, &record.team)?;
        check_revision(record.revision, input.expected_revision)?;
        if !record.team.member_ids.is_empty() {
            return Err(problem("team_not_empty", "Team still has members", 409));
        }
        self.teams
            .remove(team_id)
            .ok_or_else(|| problem("team_not_found", "Team not found", 404))
    }

    pub fn delete_consistent(
        &mut self,
        members: &MemberService,
        principal: &Principal,
        grants: &[Grant],
        workspace_id: &WorkspaceId,
        team_id: &TeamId,
        input: TeamDelete,
    ) -> Result<TeamRecord, Problem> {
        authorize(
            principal,
            grants,
            workspace_id,
            Some(team_id),
            Action::Delete,
        )?;
        let workspace_teams = self
            .teams
            .values()
            .filter(|record| &record.team.workspace_id == workspace_id)
            .map(|record| record.team.clone())
            .collect::<Vec<_>>();
        validate_membership_consistency(
            &workspace_teams,
            &members.team_assignment_members(workspace_id),
        )?;
        self.delete(principal, grants, workspace_id, team_id, input)
    }
}

fn validate_team(workspace_id: &WorkspaceId, team: &Team) -> Result<(), Problem> {
    ensure_workspace(workspace_id, team)?;
    validate_name(&team.name)?;
    if !team.member_ids.is_empty() {
        return Err(problem(
            "team_membership_assignment_required",
            "Create the team without members, then assign members through the membership operation",
            400,
        ));
    }
    let unique: BTreeSet<_> = team.member_ids.iter().collect();
    if unique.len() != team.member_ids.len() {
        return Err(problem(
            "duplicate_team_member",
            "Team contains duplicate members",
            400,
        ));
    }
    Ok(())
}

fn validate_membership_consistency(
    teams: &[Team],
    members: &[buglogin_domain::member::Member],
) -> Result<(), Problem> {
    let team_by_id = teams
        .iter()
        .map(|team| (team.id.clone(), team))
        .collect::<BTreeMap<_, _>>();
    let member_by_id = members
        .iter()
        .map(|member| (member.id.clone(), member))
        .collect::<BTreeMap<_, _>>();

    for team in teams {
        let unique_members = team.member_ids.iter().collect::<BTreeSet<_>>();
        if unique_members.len() != team.member_ids.len()
            || team.member_ids.iter().any(|member_id| {
                member_by_id
                    .get(member_id)
                    .is_none_or(|member| member.team_id.as_ref() != Some(&team.id))
            })
        {
            return Err(membership_inconsistent());
        }
    }
    for member in members {
        if let Some(team_id) = member.team_id.as_ref()
            && team_by_id
                .get(team_id)
                .is_none_or(|team| !team.member_ids.iter().any(|id| id == &member.id))
        {
            return Err(membership_inconsistent());
        }
    }
    Ok(())
}

fn membership_inconsistent() -> Problem {
    problem(
        "team_membership_inconsistent",
        "Team and member records disagree; repair the membership before changing it",
        409,
    )
}

fn ensure_workspace(workspace_id: &WorkspaceId, team: &Team) -> Result<(), Problem> {
    if &team.workspace_id == workspace_id {
        Ok(())
    } else {
        Err(problem(
            "tenant_mismatch",
            "Team belongs to another workspace",
            403,
        ))
    }
}

fn validate_name(name: &str) -> Result<(), Problem> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 128 {
        return Err(problem("invalid_team_name", "Team name is invalid", 400));
    }
    Ok(())
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

fn authorize(
    principal: &Principal,
    grants: &[Grant],
    workspace_id: &WorkspaceId,
    team_id: Option<&TeamId>,
    action: Action,
) -> Result<(), Problem> {
    let capability = Capability::new(CAPABILITY)
        .map_err(|_| problem("invalid_capability", "Invalid capability", 500))?;
    let scope = ResourceScope {
        workspace_id: Some(workspace_id.clone()),
        team_id: team_id.cloned(),
        ..ResourceScope::default()
    };
    if is_allowed(principal, &capability, action, &scope, grants) {
        Ok(())
    } else {
        Err(problem("team_forbidden", "Team access denied", 403))
    }
}

fn problem(code: &str, title: &str, status: u16) -> Problem {
    Problem::new(code, title, status, "team")
}

#[cfg(test)]
mod tests {
    use super::*;
    use buglogin_domain::{
        ids::{AccountId, MemberId},
        member::{Member, MemberRole, MemberStatus},
        permissions::{Action, Capability, Effect, Grant, Principal, ResourceScope, Role},
    };
    use std::collections::BTreeSet;

    fn platform_admin() -> Principal {
        Principal {
            roles: BTreeSet::from([Role::PlatformAdmin]),
            ..Principal::default()
        }
    }

    fn add_member(
        members: &mut MemberService,
        principal: &Principal,
        workspace_id: &WorkspaceId,
        member_id: &str,
        account_id: &str,
        role: MemberRole,
    ) -> Member {
        let member = Member {
            id: MemberId::new(member_id).unwrap(),
            account_id: AccountId::new(account_id).unwrap(),
            workspace_id: workspace_id.clone(),
            team_id: None,
            role,
            status: MemberStatus::Invited,
            invite_expires_at: Some(100),
        };
        members.invite(principal, &[], member.clone(), 1).unwrap();
        members
            .activate(principal, &[], workspace_id, &member.id, 2)
            .unwrap();
        Member {
            status: MemberStatus::Active,
            invite_expires_at: None,
            ..member
        }
    }

    fn create_team(
        teams: &mut TeamService,
        principal: &Principal,
        workspace_id: &WorkspaceId,
        team_id: &str,
    ) -> TeamRecord {
        teams
            .create(
                principal,
                &[],
                workspace_id,
                Team {
                    id: TeamId::new(team_id).unwrap(),
                    workspace_id: workspace_id.clone(),
                    name: team_id.into(),
                    member_ids: Vec::new(),
                },
            )
            .unwrap()
    }

    #[test]
    fn replace_members_moves_and_clears_memberships_consistently() {
        let workspace = WorkspaceId::new("ws_team_replace").unwrap();
        let actor = platform_admin();
        let mut members = MemberService::default();
        let first = add_member(
            &mut members,
            &actor,
            &workspace,
            "mem_team_replace_first",
            "acct_team_replace_first",
            MemberRole::Member,
        );
        let second = add_member(
            &mut members,
            &actor,
            &workspace,
            "mem_team_replace_second",
            "acct_team_replace_second",
            MemberRole::Operator,
        );
        let mut teams = TeamService::default();
        let source = create_team(&mut teams, &actor, &workspace, "team_replace_source");
        let target = create_team(&mut teams, &actor, &workspace, "team_replace_target");

        let source = teams
            .replace_members(
                &mut members,
                &actor,
                &[],
                &workspace,
                &source.team.id,
                TeamMembersReplace {
                    member_ids: vec![first.id.clone(), second.id.clone()],
                    expected_revision: source.revision,
                },
            )
            .unwrap();
        assert_eq!(
            source.team.member_ids,
            vec![first.id.clone(), second.id.clone()]
        );
        assert_eq!(
            members
                .get_in_workspace(&actor, &[], &first.id, &workspace)
                .unwrap()
                .team_id,
            Some(source.team.id.clone())
        );

        let target = teams
            .replace_members(
                &mut members,
                &actor,
                &[],
                &workspace,
                &target.team.id,
                TeamMembersReplace {
                    member_ids: vec![first.id.clone()],
                    expected_revision: target.revision,
                },
            )
            .unwrap();
        assert_eq!(target.revision, 2);
        assert_eq!(target.team.member_ids, vec![first.id.clone()]);
        let source_after_move = teams.get(&actor, &[], &workspace, &source.team.id).unwrap();
        assert_eq!(source_after_move.revision, 3);
        assert_eq!(source_after_move.team.member_ids, vec![second.id.clone()]);
        assert_eq!(
            members
                .get_in_workspace(&actor, &[], &first.id, &workspace)
                .unwrap()
                .team_id,
            Some(target.team.id.clone())
        );

        let cleared = teams
            .replace_members(
                &mut members,
                &actor,
                &[],
                &workspace,
                &target.team.id,
                TeamMembersReplace {
                    member_ids: Vec::new(),
                    expected_revision: target.revision,
                },
            )
            .unwrap();
        assert_eq!(cleared.revision, 3);
        assert!(cleared.team.member_ids.is_empty());
        assert_eq!(
            members
                .get_in_workspace(&actor, &[], &first.id, &workspace)
                .unwrap()
                .team_id,
            None
        );
    }

    #[test]
    fn replace_members_rejects_duplicate_unknown_and_stale_requests_without_mutation() {
        let workspace = WorkspaceId::new("ws_team_validation").unwrap();
        let actor = platform_admin();
        let mut members = MemberService::default();
        let member = add_member(
            &mut members,
            &actor,
            &workspace,
            "mem_team_validation",
            "acct_team_validation",
            MemberRole::Member,
        );
        let mut teams = TeamService::default();
        let target = create_team(&mut teams, &actor, &workspace, "team_validation");

        let duplicate = teams
            .replace_members(
                &mut members,
                &actor,
                &[],
                &workspace,
                &target.team.id,
                TeamMembersReplace {
                    member_ids: vec![member.id.clone(), member.id.clone()],
                    expected_revision: target.revision,
                },
            )
            .unwrap_err();
        assert_eq!(duplicate.code, "duplicate_team_member");

        let unknown = teams
            .replace_members(
                &mut members,
                &actor,
                &[],
                &workspace,
                &target.team.id,
                TeamMembersReplace {
                    member_ids: vec![MemberId::new("mem_team_unknown").unwrap()],
                    expected_revision: target.revision,
                },
            )
            .unwrap_err();
        assert_eq!(unknown.code, "member_not_found");

        let stale = teams
            .replace_members(
                &mut members,
                &actor,
                &[],
                &workspace,
                &target.team.id,
                TeamMembersReplace {
                    member_ids: vec![member.id.clone()],
                    expected_revision: target.revision + 1,
                },
            )
            .unwrap_err();
        assert_eq!(stale.code, "revision_conflict");
        assert!(
            teams
                .get(&actor, &[], &workspace, &target.team.id)
                .unwrap()
                .team
                .member_ids
                .is_empty()
        );
        assert_eq!(
            members
                .get_in_workspace(&actor, &[], &member.id, &workspace)
                .unwrap()
                .team_id,
            None
        );
    }

    #[test]
    fn replace_members_rejects_legacy_membership_divergence_without_repairing_it_silently() {
        let workspace = WorkspaceId::new("ws_team_divergence").unwrap();
        let actor = platform_admin();
        let mut members = MemberService::default();
        let member = add_member(
            &mut members,
            &actor,
            &workspace,
            "mem_team_divergence",
            "acct_team_divergence",
            MemberRole::Member,
        );
        let mut teams = TeamService::default();
        let mut team = create_team(&mut teams, &actor, &workspace, "team_divergence");
        team.team.member_ids.push(member.id.clone());
        teams.teams.insert(team.team.id.clone(), team.clone());

        let error = teams
            .replace_members(
                &mut members,
                &actor,
                &[],
                &workspace,
                &team.team.id,
                TeamMembersReplace {
                    member_ids: Vec::new(),
                    expected_revision: team.revision,
                },
            )
            .unwrap_err();
        assert_eq!(error.code, "team_membership_inconsistent");
        assert_eq!(
            teams
                .get(&actor, &[], &workspace, &team.team.id)
                .unwrap()
                .team
                .member_ids,
            vec![member.id]
        );
    }

    #[test]
    fn workspace_lead_cannot_reassign_a_higher_role_membership() {
        let workspace = WorkspaceId::new("ws_team_lead_ceiling").unwrap();
        let bootstrap = platform_admin();
        let mut members = MemberService::default();
        let lead = add_member(
            &mut members,
            &bootstrap,
            &workspace,
            "mem_team_lead_actor",
            "acct_team_lead_actor",
            MemberRole::Lead,
        );
        let protected = add_member(
            &mut members,
            &bootstrap,
            &workspace,
            "mem_team_lead_protected",
            "acct_team_lead_protected",
            MemberRole::Admin,
        );
        let actor = Principal {
            account_id: Some(lead.account_id),
            workspace_ids: BTreeSet::from([workspace.clone()]),
            ..Principal::default()
        };
        let grants = [
            Grant {
                id: buglogin_domain::ids::GrantId::new("grant_team_lead_team_update").unwrap(),
                account_id: None,
                member_id: None,
                capability: Capability::new("CAP-TEAM-001").unwrap(),
                action: Action::Update,
                scope: ResourceScope::workspace(workspace.clone()),
                effect: Effect::Allow,
            },
            Grant {
                id: buglogin_domain::ids::GrantId::new("grant_team_lead_member_manage").unwrap(),
                account_id: None,
                member_id: None,
                capability: Capability::new("CAP-MEM-001").unwrap(),
                action: Action::Manage,
                scope: ResourceScope::workspace(workspace.clone()),
                effect: Effect::Allow,
            },
        ];
        let mut teams = TeamService::default();
        let team = create_team(&mut teams, &bootstrap, &workspace, "team_lead_ceiling");

        let error = teams
            .replace_members(
                &mut members,
                &actor,
                &grants,
                &workspace,
                &team.team.id,
                TeamMembersReplace {
                    member_ids: vec![protected.id.clone()],
                    expected_revision: team.revision,
                },
            )
            .unwrap_err();
        assert_eq!(error.code, "role_escalation");
        assert_eq!(
            members
                .get_in_workspace(&bootstrap, &[], &protected.id, &workspace)
                .unwrap()
                .team_id,
            None
        );
    }
}
