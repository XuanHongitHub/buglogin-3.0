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
    #[serde(default)]
    pub blocked_organization_ids: BTreeSet<OrganizationId>,
    #[serde(default)]
    pub blocked_workspace_ids: BTreeSet<WorkspaceId>,
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
    if !principal.roles.contains(&Role::PlatformAdmin)
        && (scope
            .organization_id
            .as_ref()
            .is_some_and(|id| principal.blocked_organization_ids.contains(id))
            || scope
                .workspace_id
                .as_ref()
                .is_some_and(|id| principal.blocked_workspace_ids.contains(id)))
    {
        return false;
    }
    if !token_scope_allows(principal, capability, action) {
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

/// Checks the API token's additional capability ceiling.
///
/// API-token scopes limit regular principals. An authenticated PlatformAdmin
/// is governed by its account role and explicit DENY grants, not by the
/// narrower scopes attached to the bearer token used to authenticate it.
pub fn token_scope_allows(principal: &Principal, capability: &Capability, action: Action) -> bool {
    if principal.roles.contains(&Role::PlatformAdmin) {
        return true;
    }
    let Some(scopes) = principal.token_scopes.as_ref() else {
        return true;
    };
    let action_name = match action {
        Action::Read => "read",
        Action::Create => "create",
        Action::Update => "update",
        Action::Delete => "delete",
        Action::Execute => "execute",
        Action::Share => "share",
        Action::Manage => "manage",
    };
    let direct_scope = format!("{}:{action_name}", capability.as_str());
    if scopes.contains(&direct_scope) || scopes.contains(&format!("{}:*", capability.as_str())) {
        return true;
    }

    false
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

    let workspace_operation = capability == "profile.create"
        || capability.starts_with("CAP-WS-")
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

#[cfg(test)]
mod organization_parent_gate_tests {
    use super::*;

    #[test]
    fn suspended_parent_denies_role_and_explicit_workspace_grants_but_not_other_orgs() {
        let account = AccountId::new("acct_parent_gate").unwrap();
        let blocked = WorkspaceId::new("ws_parent_blocked").unwrap();
        let allowed = WorkspaceId::new("ws_parent_allowed").unwrap();
        let capability = Capability::new("CAP-PROF-001").unwrap();
        let principal = Principal {
            account_id: Some(account.clone()),
            workspace_ids: BTreeSet::from([blocked.clone(), allowed.clone()]),
            blocked_workspace_ids: BTreeSet::from([blocked.clone()]),
            roles: BTreeSet::from([Role::WorkspaceAdmin]),
            ..Principal::default()
        };
        let direct_grant = Grant {
            id: GrantId::new("grant_parent_gate").unwrap(),
            account_id: Some(account),
            member_id: None,
            capability: capability.clone(),
            action: Action::Read,
            scope: ResourceScope::workspace(blocked.clone()),
            effect: Effect::Allow,
        };

        assert!(!is_allowed(
            &principal,
            &capability,
            Action::Read,
            &ResourceScope::workspace(blocked),
            &[direct_grant]
        ));
        assert!(is_allowed(
            &principal,
            &capability,
            Action::Read,
            &ResourceScope::workspace(allowed),
            &[]
        ));
    }

    #[test]
    fn platform_admin_overrides_parent_membership_gate_and_legacy_principals_deserialize() {
        let organization = OrganizationId::new("org_parent_gate").unwrap();
        let workspace = WorkspaceId::new("ws_parent_gate").unwrap();
        let principal = Principal {
            roles: BTreeSet::from([Role::PlatformAdmin]),
            blocked_organization_ids: BTreeSet::from([organization.clone()]),
            blocked_workspace_ids: BTreeSet::from([workspace.clone()]),
            ..Principal::default()
        };
        let capability = Capability::new("CAP-WS-001").unwrap();
        assert!(is_allowed(
            &principal,
            &capability,
            Action::Read,
            &ResourceScope {
                organization_id: Some(organization),
                ..ResourceScope::workspace(workspace)
            },
            &[]
        ));

        let legacy: Principal = serde_json::from_value(serde_json::json!({
            "accountId": "acct_legacy_principal",
            "memberId": null,
            "workspaceIds": [],
            "tokenScopes": null,
            "roles": ["account"]
        }))
        .unwrap();
        assert!(legacy.blocked_workspace_ids.is_empty());
    }

    #[test]
    fn platform_admin_role_is_not_limited_by_api_token_scopes() {
        let admin_scope = Capability::new("CAP-ADM-001").unwrap();
        let org_scope = Capability::new("CAP-ORG-001").unwrap();
        let platform_admin = Principal {
            roles: BTreeSet::from([Role::PlatformAdmin]),
            token_scopes: Some(BTreeSet::new()),
            ..Principal::default()
        };
        let org_create_scope = ResourceScope {
            organization_id: Some(OrganizationId::new("org_admin_scope").unwrap()),
            ..ResourceScope::default()
        };

        assert!(is_allowed(
            &platform_admin,
            &org_scope,
            Action::Create,
            &org_create_scope,
            &[]
        ));
        assert!(is_allowed(
            &platform_admin,
            &org_scope,
            Action::Read,
            &org_create_scope,
            &[]
        ));
        assert!(!is_allowed(
            &Principal {
                roles: BTreeSet::from([Role::WorkspaceAdmin]),
                token_scopes: Some(BTreeSet::from(["CAP-ADM-001:*".to_owned()])),
                ..Principal::default()
            },
            &org_scope,
            Action::Create,
            &org_create_scope,
            &[]
        ));
        assert!(is_allowed(
            &Principal {
                roles: BTreeSet::from([Role::PlatformAdmin]),
                token_scopes: Some(BTreeSet::new()),
                ..Principal::default()
            },
            &org_scope,
            Action::Create,
            &org_create_scope,
            &[]
        ));
        assert!(token_scope_allows(
            &platform_admin,
            &admin_scope,
            Action::Create
        ));
        assert!(token_scope_allows(
            &Principal {
                roles: BTreeSet::from([Role::PlatformAdmin]),
                token_scopes: Some(BTreeSet::from(["CAP-ORG-001:read".to_owned()])),
                ..Principal::default()
            },
            &org_scope,
            Action::Create
        ));
    }
}
