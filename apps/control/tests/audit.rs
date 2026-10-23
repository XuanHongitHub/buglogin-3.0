use std::collections::{BTreeMap, BTreeSet};

use buglogin_api::services::audit::AuditService;
use buglogin_domain::{
    audit::{AuditEvent, AuditOutcome, AuditResource, REDACTED, SupportAccess},
    ids::{AccountId, GrantId, WorkspaceId},
    permissions::{Action, Capability, Effect, Grant, Principal, ResourceScope},
};

fn account(value: &str) -> AccountId {
    AccountId::from_str(value).unwrap()
}
fn workspace(value: &str) -> WorkspaceId {
    WorkspaceId::from_str(value).unwrap()
}
fn principal(value: &str) -> Principal {
    Principal {
        account_id: Some(account(value)),
        member_id: None,
        workspace_ids: BTreeSet::new(),
        roles: BTreeSet::new(),
        token_scopes: None,
    }
}
fn grant(actor: &str, workspace_id: &WorkspaceId, action: Action) -> Grant {
    Grant {
        id: GrantId::from_str(&format!("grant_{actor}_{action:?}")).unwrap(),
        account_id: Some(account(actor)),
        member_id: None,
        capability: Capability::new("CAP-AUD-001").unwrap(),
        action,
        scope: ResourceScope::workspace(workspace_id.clone()),
        effect: Effect::Allow,
    }
}
fn event(event_id: &str, workspace_id: &WorkspaceId) -> AuditEvent {
    AuditEvent::new(
        event_id,
        10,
        account("acct_actor"),
        AuditResource {
            resource_type: "profile".into(),
            resource_id: "prf_1".into(),
            scope: ResourceScope::workspace(workspace_id.clone()),
        },
        "profile.launch",
        "cor_1",
        AuditOutcome::Succeeded,
        BTreeMap::from([
            ("authorization".into(), "Bearer production-token".into()),
            ("proxyUrl".into(), "http://user:pass@127.0.0.1:8080".into()),
            ("runtime".into(), "wayfern".into()),
        ]),
    )
    .unwrap()
}

#[test]
fn audit_is_append_only_redacted_and_scope_authorized() {
    let workspace_a = workspace("ws_a");
    let workspace_b = workspace("ws_b");
    let auditor = principal("acct_auditor");
    let grants = [grant("acct_auditor", &workspace_a, Action::Read)];
    let mut service = AuditService::default();
    service.append(event("evt_1", &workspace_a)).unwrap();
    service.append(event("evt_2", &workspace_b)).unwrap();
    assert_eq!(
        service
            .append(event("evt_1", &workspace_a))
            .unwrap_err()
            .code,
        "audit_event_exists"
    );
    assert_eq!(
        service
            .read(
                &auditor,
                &[],
                &ResourceScope::workspace(workspace_a.clone())
            )
            .unwrap_err()
            .code,
        "audit_forbidden"
    );
    let events = service
        .read(&auditor, &grants, &ResourceScope::workspace(workspace_a))
        .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].actor_id.as_str(), "acct_actor");
    assert_eq!(events[0].command_id, "profile.launch");
    assert_eq!(events[0].correlation_id, "cor_1");
    assert_eq!(events[0].metadata["authorization"], REDACTED);
    assert_eq!(events[0].metadata["proxyUrl"], REDACTED);
    assert_eq!(events[0].metadata["runtime"], "wayfern");
}

#[test]
fn support_access_is_actor_scoped_expiring_and_revocable() {
    let workspace = workspace("ws_support");
    let issuer = principal("acct_admin");
    let support_actor = account("acct_support");
    let grants = [grant("acct_admin", &workspace, Action::Manage)];
    let mut service = AuditService::default();
    service.append(event("evt_support", &workspace)).unwrap();
    service
        .issue_support_access(
            &issuer,
            &grants,
            SupportAccess::new(
                "support_1",
                support_actor.clone(),
                account("acct_admin"),
                ResourceScope::workspace(workspace.clone()),
                "Investigate failed profile launch",
                100,
                200,
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(
        service
            .read_with_support(
                &account("acct_other"),
                "support_1",
                &ResourceScope::workspace(workspace.clone()),
                150
            )
            .unwrap_err()
            .code,
        "support_access_denied"
    );
    assert_eq!(
        service
            .read_with_support(
                &support_actor,
                "support_1",
                &ResourceScope::workspace(workspace.clone()),
                150
            )
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        service
            .read_with_support(
                &support_actor,
                "support_1",
                &ResourceScope::workspace(workspace.clone()),
                200
            )
            .unwrap_err()
            .code,
        "support_access_denied"
    );
    service
        .revoke_support_access(&issuer, &grants, "support_1", 160)
        .unwrap();
    assert_eq!(
        service
            .read_with_support(
                &support_actor,
                "support_1",
                &ResourceScope::workspace(workspace),
                170
            )
            .unwrap_err()
            .code,
        "support_access_denied"
    );
}
