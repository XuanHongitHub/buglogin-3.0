use buglogin_api::services::members::MemberService;
use buglogin_domain::{
    ids::{AccountId, GrantId, MemberId, WorkspaceId},
    member::{Member, MemberRole, MemberStatus},
    permissions::{Action, Capability, Effect, Grant, Principal, ResourceScope, Role},
};
use std::collections::BTreeSet;

fn admin() -> Principal {
    Principal {
        account_id: Some(AccountId::from_str("acct_admin").unwrap()),
        member_id: Some(MemberId::from_str("mem_admin").unwrap()),
        workspace_ids: BTreeSet::from([WorkspaceId::from_str("ws_1").unwrap()]),
        roles: BTreeSet::from([Role::WorkspaceAdmin]),
        token_scopes: None,
        ..Principal::default()
    }
}

fn platform_admin() -> Principal {
    Principal {
        account_id: Some(AccountId::from_str("acct_platform_admin").unwrap()),
        member_id: Some(MemberId::from_str("mem_platform_admin").unwrap()),
        roles: BTreeSet::from([Role::PlatformAdmin]),
        ..Principal::default()
    }
}

fn member(id: &str, account: &str, role: MemberRole) -> Member {
    Member {
        id: MemberId::from_str(id).unwrap(),
        account_id: AccountId::from_str(account).unwrap(),
        workspace_id: WorkspaceId::from_str("ws_1").unwrap(),
        team_id: None,
        role,
        status: MemberStatus::Invited,
        invite_expires_at: Some(100),
    }
}

fn seed_active_workspace_admin(service: &mut MemberService) {
    let platform_admin = platform_admin();
    let workspace_admin = member("mem_admin", "acct_admin", MemberRole::Admin);
    service
        .invite(&platform_admin, &[], workspace_admin, 1)
        .unwrap();
    service
        .activate(
            &platform_admin,
            &[],
            &WorkspaceId::from_str("ws_1").unwrap(),
            &MemberId::from_str("mem_admin").unwrap(),
            99,
        )
        .unwrap();
}

fn deny_manage() -> Grant {
    Grant {
        id: GrantId::from_str("grant_deny").unwrap(),
        account_id: Some(AccountId::from_str("acct_admin").unwrap()),
        member_id: Some(MemberId::from_str("mem_admin").unwrap()),
        capability: Capability::new("CAP-MEM-001").unwrap(),
        action: Action::Manage,
        scope: ResourceScope::workspace(WorkspaceId::from_str("ws_1").unwrap()),
        effect: Effect::Deny,
    }
}

#[test]
fn member_mutations_are_acl_backed_and_duplicate_invites_do_not_replace() {
    let mut service = MemberService::default();
    seed_active_workspace_admin(&mut service);
    let actor = admin();
    let workspace_id = WorkspaceId::from_str("ws_1").unwrap();
    service
        .invite(
            &actor,
            &[],
            member("mem_1", "acct_1", MemberRole::Member),
            1,
        )
        .unwrap();
    assert_eq!(
        service
            .invite(&actor, &[], member("mem_1", "acct_2", MemberRole::Member), 1)
            .unwrap_err()
            .code,
        "member_exists"
    );
    assert_eq!(
        service
            .get_in_workspace(
                &actor,
                &[],
                &MemberId::from_str("mem_1").unwrap(),
                &WorkspaceId::from_str("ws_1").unwrap()
            )
            .unwrap()
            .account_id,
        AccountId::from_str("acct_1").unwrap()
    );
    assert_eq!(
        service
            .suspend(
                &actor,
                &[deny_manage()],
                &workspace_id,
                &MemberId::from_str("mem_1").unwrap()
            )
            .unwrap_err()
            .code,
        "member_forbidden"
    );
    assert_eq!(
        service
            .invite(
                &Principal::default(),
                &[],
                member("mem_2", "acct_2", MemberRole::Member),
                1
            )
            .unwrap_err()
            .code,
        "member_forbidden"
    );
}

#[test]
fn invite_self_accepts_but_role_changes_cannot_escalate_actor() {
    let mut service = MemberService::default();
    seed_active_workspace_admin(&mut service);
    let actor = admin();
    let workspace_id = WorkspaceId::from_str("ws_1").unwrap();
    let invitee = member("mem_1", "acct_1", MemberRole::Member);
    service.invite(&actor, &[], invitee, 1).unwrap();
    let self_principal = Principal {
        account_id: Some(AccountId::from_str("acct_1").unwrap()),
        ..Principal::default()
    };
    service
        .activate(
            &self_principal,
            &[],
            &workspace_id,
            &MemberId::from_str("mem_1").unwrap(),
            99,
        )
        .unwrap();
    assert_eq!(
        service
            .set_role(
                &actor,
                &[],
                &workspace_id,
                &MemberId::from_str("mem_1").unwrap(),
                MemberRole::Owner
            )
            .unwrap_err()
            .code,
        "role_escalation"
    );
}
