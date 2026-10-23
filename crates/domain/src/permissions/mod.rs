use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::ids::{
    AccountId, GrantId, MemberId, OrganizationId, ProfileId, ResourceId, TeamId, WorkspaceId,
};

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Capability(String);

impl Capability {
    pub fn new(value: impl Into<String>) -> Result<Self, PermissionError> {
        let value = value.into();
        if value.is_empty() || value.len() > 128 {
            return Err(PermissionError::InvalidCapability);
        }
        if !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
        {
            return Err(PermissionError::InvalidCapability);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Read,
    Create,
    Update,
    Delete,
    Execute,
    Share,
    Manage,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceScope {
    pub account_id: Option<AccountId>,
    pub organization_id: Option<OrganizationId>,
    pub workspace_id: Option<WorkspaceId>,
    pub team_id: Option<TeamId>,
    pub member_id: Option<MemberId>,
    pub resource_id: Option<ResourceId>,
    pub profile_id: Option<ProfileId>,
}

impl ResourceScope {
    pub fn global() -> Self {
        Self::default()
    }

    pub fn account(account_id: AccountId) -> Self {
        Self {
            account_id: Some(account_id),
            ..Self::default()
        }
    }

    pub fn workspace(workspace_id: WorkspaceId) -> Self {
        Self {
            workspace_id: Some(workspace_id),
            ..Self::default()
        }
    }

    pub fn resource(workspace_id: WorkspaceId, resource_id: ResourceId) -> Self {
        Self {
            workspace_id: Some(workspace_id),
            resource_id: Some(resource_id),
            ..Self::default()
        }
    }

    pub fn contains(&self, requested: &Self) -> bool {
        self.account_id
            .as_ref()
            .is_none_or(|id| requested.account_id.as_ref() == Some(id))
            && self
                .organization_id
                .as_ref()
                .is_none_or(|id| requested.organization_id.as_ref() == Some(id))
            && self
                .workspace_id
                .as_ref()
                .is_none_or(|id| requested.workspace_id.as_ref() == Some(id))
            && self
                .team_id
                .as_ref()
                .is_none_or(|id| requested.team_id.as_ref() == Some(id))
            && self
                .member_id
                .as_ref()
                .is_none_or(|id| requested.member_id.as_ref() == Some(id))
            && self
                .resource_id
                .as_ref()
                .is_none_or(|id| requested.resource_id.as_ref() == Some(id))
            && self
                .profile_id
                .as_ref()
                .is_none_or(|id| requested.profile_id.as_ref() == Some(id))
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Role {
    LocalOwner,
    Account,
    WorkspaceMember,
    WorkspaceOperator,
    WorkspaceAdmin,
    PlatformAdmin,
    Service,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Principal {
    pub account_id: Option<AccountId>,
    pub member_id: Option<MemberId>,
    pub workspace_ids: BTreeSet<WorkspaceId>,
    pub roles: BTreeSet<Role>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_scopes: Option<BTreeSet<String>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effect {
    Allow,
    Deny,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Grant {
    pub id: GrantId,
    pub account_id: Option<AccountId>,
    pub member_id: Option<MemberId>,
    pub capability: Capability,
    pub action: Action,
    pub scope: ResourceScope,
    pub effect: Effect,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PermissionError {
    InvalidCapability,
}

impl std::fmt::Display for PermissionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidCapability => formatter.write_str("invalid capability"),
        }
    }
}

impl std::error::Error for PermissionError {}

pub fn is_allowed(
    principal: &Principal,
    capability: &Capability,
    action: Action,
    scope: &ResourceScope,
    grants: &[Grant],
) -> bool {
    if principal.token_scopes.as_ref().is_some_and(|scopes| {
        let action = match action {
            Action::Read => "read",
            Action::Create => "create",
            Action::Update => "update",
            Action::Delete => "delete",
            Action::Execute => "execute",
            Action::Share => "share",
            Action::Manage => "manage",
        };
        !scopes.contains(&format!("{}:{action}", capability.as_str()))
            && !scopes.contains(&format!("{}:*", capability.as_str()))
    }) {
        return false;
    }
    let mut matching_grants = grants.iter().filter(|grant| {
        grant.capability == *capability
            && grant.action == action
            && grant.scope.contains(scope)
            && grant_matches_principal(grant, principal)
    });

    if matching_grants
        .clone()
        .any(|grant| grant.effect == Effect::Deny)
    {
        return false;
    }
    if matching_grants.any(|grant| grant.effect == Effect::Allow) {
        return true;
    }

    principal
        .roles
        .iter()
        .any(|role| role_allows(principal, *role, capability.as_str(), action, scope))
}

fn grant_matches_principal(grant: &Grant, principal: &Principal) -> bool {
    grant
        .account_id
        .as_ref()
        .is_none_or(|id| principal.account_id.as_ref() == Some(id))
        && grant
            .member_id
            .as_ref()
            .is_none_or(|id| principal.member_id.as_ref() == Some(id))
}

fn role_allows(
    principal: &Principal,
    role: Role,
    capability: &str,
    action: Action,
    scope: &ResourceScope,
) -> bool {
    if role == Role::PlatformAdmin {
        return true;
    }
    if role == Role::Service {
        return false;
    }
    if role == Role::LocalOwner {
        return capability.starts_with("CAP-DESK-")
            || capability.starts_with("CAP-PROF-")
            || capability.starts_with("CAP-NET-")
            || capability.starts_with("CAP-COOK-")
            || capability.starts_with("CAP-EXT-")
            || capability.starts_with("CAP-SYNC-")
            || capability.starts_with("CAP-RUN-")
            || capability.starts_with("CAP-STOR-");
    }

    let Some(workspace_id) = scope.workspace_id.as_ref() else {
        return false;
    };
    if !principal.workspace_ids.contains(workspace_id) {
        return false;
    }

    let workspace_operation = capability.starts_with("CAP-WS-")
        || capability.starts_with("CAP-TEAM-")
        || capability.starts_with("CAP-MEM-")
        || capability.starts_with("CAP-RES-")
        || capability.starts_with("CAP-PROF-")
        || capability.starts_with("CAP-NET-")
        || capability.starts_with("CAP-COOK-")
        || capability.starts_with("CAP-EXT-")
        || capability.starts_with("CAP-SYNC-")
        || capability.starts_with("CAP-RUN-")
        || capability.starts_with("CAP-STOR-");

    match role {
        Role::Account => capability == "CAP-WEB-002" || capability.starts_with("CAP-ID-"),
        Role::WorkspaceMember => {
            workspace_operation && matches!(action, Action::Read | Action::Execute)
        }
        Role::WorkspaceOperator => {
            workspace_operation
                && matches!(
                    action,
                    Action::Read
                        | Action::Create
                        | Action::Update
                        | Action::Execute
                        | Action::Share
                )
        }
        Role::WorkspaceAdmin => {
            workspace_operation
                && matches!(
                    action,
                    Action::Read
                        | Action::Create
                        | Action::Update
                        | Action::Delete
                        | Action::Execute
                        | Action::Share
                        | Action::Manage
                )
        }
        Role::LocalOwner | Role::PlatformAdmin | Role::Service => false,
    }
}
