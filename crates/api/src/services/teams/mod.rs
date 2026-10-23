use std::collections::{BTreeMap, BTreeSet};

use buglogin_domain::{
    errors::Problem,
    ids::{TeamId, WorkspaceId},
    permissions::{Action, Capability, Grant, Principal, ResourceScope, is_allowed},
    team::Team,
};
use serde::{Deserialize, Serialize};

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

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct TeamService {
    teams: BTreeMap<TeamId, TeamRecord>,
}

impl TeamService {
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
}

fn validate_team(workspace_id: &WorkspaceId, team: &Team) -> Result<(), Problem> {
    ensure_workspace(workspace_id, team)?;
    validate_name(&team.name)?;
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
