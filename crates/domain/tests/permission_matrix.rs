use buglogin_domain::ids::{AccountId, GrantId, MemberId, ResourceId, WorkspaceId};
use buglogin_domain::permissions::{
    Action, Capability, Effect, Grant, Principal, ResourceScope, Role, is_allowed,
};
use std::collections::BTreeSet;

fn id<T>(
    value: &str,
    constructor: impl FnOnce(&str) -> Result<T, buglogin_domain::ids::IdError>,
) -> T {
    constructor(value).expect("valid canonical ID")
}

fn capability(value: &str) -> Capability {
    Capability::new(value).expect("valid capability")
}

fn member_principal(role: Role) -> Principal {
    Principal {
        account_id: Some(id("acct_1", AccountId::from_str)),
        member_id: Some(id("mem_1", MemberId::from_str)),
        workspace_ids: BTreeSet::from([id("ws_1", WorkspaceId::from_str)]),
        blocked_organization_ids: BTreeSet::new(),
        blocked_workspace_ids: BTreeSet::new(),
        roles: BTreeSet::from([role]),
        token_scopes: None,
    }
}

fn workspace_scope() -> ResourceScope {
    ResourceScope::workspace(id("ws_1", WorkspaceId::from_str))
}

#[test]
fn denies_by_default_without_role_or_grant() {
    let principal = Principal::default();
    assert!(!is_allowed(
        &principal,
        &capability("CAP-PROF-001"),
        Action::Read,
        &workspace_scope(),
        &[]
    ));
}

#[test]
fn member_reads_and_executes_only_inside_workspace() {
    let principal = member_principal(Role::WorkspaceMember);
    let other_workspace = ResourceScope::workspace(id("ws_2", WorkspaceId::from_str));

    assert!(is_allowed(
        &principal,
        &capability("CAP-PROF-001"),
        Action::Read,
        &workspace_scope(),
        &[]
    ));
    assert!(is_allowed(
        &principal,
        &capability("CAP-PROF-002"),
        Action::Execute,
        &workspace_scope(),
        &[]
    ));
    assert!(!is_allowed(
        &principal,
        &capability("CAP-PROF-001"),
        Action::Delete,
        &workspace_scope(),
        &[]
    ));
    assert!(!is_allowed(
        &principal,
        &capability("CAP-PROF-001"),
        Action::Read,
        &other_workspace,
        &[]
    ));
}

#[test]
fn admin_manages_workspace_resources_but_not_global_scope() {
    let principal = member_principal(Role::WorkspaceAdmin);

    assert!(is_allowed(
        &principal,
        &capability("CAP-RES-001"),
        Action::Manage,
        &workspace_scope(),
        &[]
    ));
    assert!(!is_allowed(
        &principal,
        &capability("CAP-RES-001"),
        Action::Manage,
        &ResourceScope::global(),
        &[]
    ));
}

#[test]
fn explicit_deny_overrides_role_and_explicit_allow() {
    let principal = member_principal(Role::WorkspaceAdmin);
    let scope = ResourceScope::resource(
        id("ws_1", WorkspaceId::from_str),
        id("res_1", ResourceId::from_str),
    );
    let capability = capability("CAP-PROF-001");
    let grants = vec![
        Grant {
            id: id("grant_allow", GrantId::from_str),
            account_id: None,
            member_id: Some(id("mem_1", MemberId::from_str)),
            capability: capability.clone(),
            action: Action::Delete,
            scope: ResourceScope::workspace(id("ws_1", WorkspaceId::from_str)),
            effect: Effect::Allow,
        },
        Grant {
            id: id("grant_deny", GrantId::from_str),
            account_id: None,
            member_id: Some(id("mem_1", MemberId::from_str)),
            capability: capability.clone(),
            action: Action::Delete,
            scope: scope.clone(),
            effect: Effect::Deny,
        },
    ];

    assert!(!is_allowed(
        &principal,
        &capability,
        Action::Delete,
        &scope,
        &grants
    ));
}

#[test]
fn service_requires_explicit_scoped_grant() {
    let principal = member_principal(Role::Service);
    let capability = capability("CAP-SYNC-001");
    let scope = workspace_scope();

    assert!(!is_allowed(
        &principal,
        &capability,
        Action::Read,
        &scope,
        &[]
    ));
    let grants = vec![Grant {
        id: id("grant_sync", GrantId::from_str),
        account_id: None,
        member_id: None,
        capability: capability.clone(),
        action: Action::Read,
        scope: scope.clone(),
        effect: Effect::Allow,
    }];
    assert!(is_allowed(
        &principal,
        &capability,
        Action::Read,
        &scope,
        &grants
    ));
}

#[test]
fn api_token_scopes_are_an_additional_deny_boundary() {
    let workspace = id("ws_1", WorkspaceId::from_str);
    let mut principal = member_principal(Role::WorkspaceAdmin);
    principal.token_scopes = Some(BTreeSet::from(["profile.read:read".into()]));
    let grants = vec![Grant {
        id: id("grant_1", GrantId::from_str),
        account_id: principal.account_id.clone(),
        member_id: principal.member_id.clone(),
        capability: Capability::new("profile.read").unwrap(),
        action: Action::Read,
        scope: ResourceScope::workspace(workspace),
        effect: Effect::Allow,
    }];
    assert!(is_allowed(
        &principal,
        &Capability::new("profile.read").unwrap(),
        Action::Read,
        &ResourceScope::workspace(id("ws_1", WorkspaceId::from_str)),
        &grants,
    ));
    assert!(!is_allowed(
        &principal,
        &Capability::new("profile.read").unwrap(),
        Action::Update,
        &ResourceScope::workspace(id("ws_1", WorkspaceId::from_str)),
        &grants,
    ));
    assert!(!is_allowed(
        &principal,
        &Capability::new("proxy.parse").unwrap(),
        Action::Read,
        &ResourceScope::global(),
        &grants,
    ));
}
