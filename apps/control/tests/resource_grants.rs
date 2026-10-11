use buglogin_api::services::resources::ResourceService;
use buglogin_domain::{
    grant::ResourceGrant,
    ids::{AccountId, GrantId, MemberId, ResourceId, WorkspaceId},
    permissions::{Action, Capability, Effect, Grant, Principal, ResourceScope, Role},
    resource::{Resource, ResourceKind, ResourceState},
};
use std::collections::BTreeSet;

fn admin() -> Principal {
    Principal {
        account_id: Some(AccountId::from_str("acct_1").unwrap()),
        member_id: Some(MemberId::from_str("mem_1").unwrap()),
        workspace_ids: BTreeSet::from([WorkspaceId::from_str("ws_1").unwrap()]),
        roles: BTreeSet::from([Role::WorkspaceAdmin]),
        token_scopes: None,
    }
}

fn resource(name: &str) -> Resource {
    Resource {
        id: ResourceId::from_str("res_1").unwrap(),
        workspace_id: WorkspaceId::from_str("ws_1").unwrap(),
        kind: ResourceKind::Profile,
        state: ResourceState::Active,
        display_name: name.into(),
    }
}

fn grant(id: &str, effect: Effect) -> ResourceGrant {
    ResourceGrant {
        id: GrantId::from_str(id).unwrap(),
        workspace_id: WorkspaceId::from_str("ws_1").unwrap(),
        resource_id: ResourceId::from_str("res_1").unwrap(),
        member_id: MemberId::from_str("mem_2").unwrap(),
        action: Action::Read,
        effect,
    }
}

fn sharing_grant() -> Grant {
    Grant {
        id: GrantId::from_str("acl_share_1").unwrap(),
        account_id: Some(AccountId::from_str("acct_1").unwrap()),
        member_id: None,
        capability: Capability::new("CAP-SHARE-001").unwrap(),
        action: Action::Manage,
        scope: ResourceScope::resource(
            WorkspaceId::from_str("ws_1").unwrap(),
            ResourceId::from_str("res_1").unwrap(),
        ),
        effect: Effect::Allow,
    }
}

#[test]
fn resources_and_grants_are_acl_backed_and_ids_are_not_replaced() {
    let actor = admin();
    let mut service = ResourceService::default();
    service.register(&actor, &[], resource("original")).unwrap();
    assert_eq!(
        service
            .register(&actor, &[], resource("replacement"))
            .unwrap_err()
            .code,
        "resource_exists"
    );
    assert_eq!(
        service
            .get(
                &actor,
                &[],
                &ResourceId::from_str("res_1").unwrap(),
                &WorkspaceId::from_str("ws_1").unwrap()
            )
            .unwrap()
            .display_name,
        "original"
    );
    let sharing = [sharing_grant()];
    service
        .grant(&actor, &sharing, grant("grant_1", Effect::Allow))
        .unwrap();
    service
        .grant(&actor, &sharing, grant("grant_2", Effect::Deny))
        .unwrap();
    assert!(!service.is_allowed(
        &ResourceId::from_str("res_1").unwrap(),
        &MemberId::from_str("mem_2").unwrap(),
        Action::Read
    ));
    assert_eq!(
        service
            .grant(&actor, &sharing, grant("grant_1", Effect::Allow))
            .unwrap_err()
            .code,
        "grant_exists"
    );
    service
        .revoke(&actor, &sharing, &GrantId::from_str("grant_2").unwrap())
        .unwrap();
    assert!(service.is_allowed(
        &ResourceId::from_str("res_1").unwrap(),
        &MemberId::from_str("mem_2").unwrap(),
        Action::Read
    ));
}

#[test]
fn resource_tenant_and_acl_boundaries_fail_closed() {
    let mut service = ResourceService::default();
    assert_eq!(
        service
            .register(&Principal::default(), &[], resource("blocked"))
            .unwrap_err()
            .code,
        "resource_forbidden"
    );
    let actor = admin();
    service.register(&actor, &[], resource("profile")).unwrap();
    let sharing = [sharing_grant()];
    let mut wrong = grant("grant_1", Effect::Allow);
    wrong.workspace_id = WorkspaceId::from_str("ws_2").unwrap();
    assert_eq!(
        service.grant(&actor, &sharing, wrong).unwrap_err().code,
        "resource_forbidden"
    );
}
