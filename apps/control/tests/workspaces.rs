use buglogin_api::services::workspaces::WorkspaceService;
use buglogin_domain::{
    ids::{AccountId, GrantId, MemberId, OrganizationId, WorkspaceId},
    organization::{Organization, OrganizationPolicy},
    permissions::{Action, Capability, Effect, Grant, Principal, ResourceScope, Role},
    workspace::{Workspace, WorkspacePolicy, WorkspaceUsage},
};
use std::collections::BTreeSet;

fn principal(role: Role, workspace: &str) -> Principal {
    Principal {
        account_id: Some(AccountId::from_str("acct_1").unwrap()),
        member_id: Some(MemberId::from_str("mem_1").unwrap()),
        workspace_ids: BTreeSet::from([WorkspaceId::from_str(workspace).unwrap()]),
        roles: BTreeSet::from([role]),
        token_scopes: None,
    }
}

fn organization(id: &str) -> Organization {
    Organization {
        id: OrganizationId::from_str(id).unwrap(),
        account_id: AccountId::from_str("acct_1").unwrap(),
        name: id.into(),
        created_at: "2026-07-20T00:00:00Z".into(),
    }
}

fn workspace(id: &str, organization_id: &OrganizationId) -> Workspace {
    Workspace {
        id: WorkspaceId::from_str(id).unwrap(),
        organization_id: organization_id.clone(),
        name: id.into(),
        policy: WorkspacePolicy {
            max_profiles: 2,
            max_members: 2,
            max_storage_bytes: 1000,
            offline_entitled: true,
        },
        created_at: "2026-07-20T00:00:00Z".into(),
    }
}

fn grant(capability: &str, action: Action, scope: ResourceScope, effect: Effect) -> Grant {
    Grant {
        id: GrantId::from_str("grant_1").unwrap(),
        account_id: Some(AccountId::from_str("acct_1").unwrap()),
        member_id: Some(MemberId::from_str("mem_1").unwrap()),
        capability: Capability::new(capability).unwrap(),
        action,
        scope,
        effect,
    }
}

#[test]
fn acl_denies_before_lookup_and_explicit_deny_overrides_role() {
    let org = organization("org_1");
    let ws = workspace("ws_1", &org.id);
    let actor = principal(Role::WorkspaceAdmin, "ws_1");
    let deny = grant(
        "CAP-WS-001",
        Action::Read,
        ResourceScope::workspace(ws.id.clone()),
        Effect::Deny,
    );
    let mut service = WorkspaceService::new(
        org.clone(),
        OrganizationPolicy {
            max_workspaces: 1,
            offline_entitled: true,
        },
    )
    .unwrap();
    service
        .create_workspace(&actor, &[], &org.id, ws.clone())
        .unwrap();
    assert_eq!(
        service
            .get_workspace(&actor, &[deny], &org.id, &ws.id)
            .unwrap_err()
            .code,
        "forbidden"
    );
    assert_eq!(
        service
            .get_workspace(
                &Principal::default(),
                &[],
                &org.id,
                &WorkspaceId::from_str("missing").unwrap()
            )
            .unwrap_err()
            .code,
        "forbidden"
    );
}

#[test]
fn workspace_ids_are_unique_across_organizations_and_quota_is_enforced() {
    let org_1 = organization("org_1");
    let org_2 = organization("org_2");
    let ws = workspace("ws_1", &org_1.id);
    let actor = principal(Role::PlatformAdmin, "ws_1");
    let mut service = WorkspaceService::new(
        org_1.clone(),
        OrganizationPolicy {
            max_workspaces: 1,
            offline_entitled: true,
        },
    )
    .unwrap();
    service
        .create_workspace(&actor, &[], &org_1.id, ws.clone())
        .unwrap();
    let org_grant = grant(
        "CAP-ORG-001",
        Action::Create,
        ResourceScope {
            account_id: Some(org_2.account_id.clone()),
            organization_id: Some(org_2.id.clone()),
            ..ResourceScope::default()
        },
        Effect::Allow,
    );
    service
        .add_organization(
            &actor,
            &[org_grant],
            org_2.clone(),
            OrganizationPolicy {
                max_workspaces: 1,
                offline_entitled: true,
            },
        )
        .unwrap();
    let duplicate = workspace("ws_1", &org_2.id);
    assert_eq!(
        service
            .create_workspace(&actor, &[], &org_2.id, duplicate)
            .unwrap_err()
            .code,
        "workspace_exists"
    );
    assert_eq!(
        service
            .check_usage(
                &actor,
                &[],
                &org_1.id,
                &ws.id,
                WorkspaceUsage {
                    profiles: 3,
                    members: 0,
                    storage_bytes: 0
                }
            )
            .unwrap_err()
            .code,
        "profile_quota_exceeded"
    );
}
