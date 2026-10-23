use buglogin_api::services::{
    events::{EventService, NewEvent},
    operations::{Operation, OperationService, OperationStatus},
};
use buglogin_domain::{
    ids::{GrantId, WorkspaceId},
    permissions::{Action, Capability, Effect, Grant, Principal, ResourceScope},
};
use serde_json::json;

fn workspace(value: &str) -> WorkspaceId {
    WorkspaceId::from_str(value).expect("workspace ID")
}

fn read_grant(capability: &str, scope: ResourceScope) -> Grant {
    Grant {
        id: GrantId::new(format!("grant_{}", capability.replace('.', "_"))).expect("grant ID"),
        account_id: None,
        member_id: None,
        capability: Capability::new(capability).expect("capability"),
        action: Action::Read,
        scope,
        effect: Effect::Allow,
    }
}

fn event(id: &str, scope: ResourceScope, revision: u64) -> NewEvent {
    NewEvent {
        event_id: id.into(),
        event_version: "1.0".into(),
        name: "operation.updated".into(),
        occurred_at: "2026-07-20T00:00:00Z".into(),
        correlation_id: "cor_1".into(),
        causation_id: "cmd_1".into(),
        actor_id: "usr_1".into(),
        scope,
        resource_type: "operation".into(),
        resource_id: id.into(),
        resource_revision: revision,
        data: json!({"status": "running"}),
    }
}

#[test]
fn operation_reads_deny_by_default_and_stay_scoped() {
    let first_scope = ResourceScope::workspace(workspace("ws_1"));
    let second_scope = ResourceScope::workspace(workspace("ws_2"));
    let mut service = OperationService::default();
    service
        .insert(Operation::queued("op_1", "profile.launch", first_scope.clone()).unwrap())
        .unwrap();
    service
        .insert(Operation::queued("op_2", "profile.launch", second_scope).unwrap())
        .unwrap();

    let principal = Principal::default();
    assert_eq!(
        service.get(&principal, &[], "op_1").unwrap_err().status,
        403
    );

    let grants = [read_grant("operation.read", first_scope.clone())];
    assert_eq!(
        service
            .get(&principal, &grants, "op_1")
            .unwrap()
            .operation_id,
        "op_1"
    );
    let listed = service.list(&principal, &grants, &first_scope).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].operation_id, "op_1");
}

#[test]
fn operation_terminal_states_are_explicit_and_bound_storage() {
    let scope = ResourceScope::workspace(workspace("ws_1"));
    let mut service = OperationService::with_capacity(1).unwrap();
    service
        .insert(Operation::queued("op_1", "profile.launch", scope.clone()).unwrap())
        .unwrap();
    assert_eq!(
        service
            .insert(Operation::queued("op_2", "profile.stop", scope.clone()).unwrap())
            .unwrap_err()
            .code,
        "operation_store_full"
    );

    service
        .transition("op_1", OperationStatus::Running, Some(0.5), None, None)
        .unwrap();
    service
        .transition(
            "op_1",
            OperationStatus::Completed,
            Some(1.0),
            Some(json!({"profileId": "prf_1"})),
            None,
        )
        .unwrap();
    assert_eq!(
        service
            .transition("op_1", OperationStatus::Running, Some(1.0), None, None)
            .unwrap_err()
            .code,
        "invalid_state_transition"
    );

    service
        .insert(Operation::queued("op_2", "profile.stop", scope).unwrap())
        .unwrap();
    assert_eq!(service.len(), 1);
}

#[test]
fn operation_and_event_payloads_reject_secret_fields() {
    let scope = ResourceScope::workspace(workspace("ws_1"));
    let mut operations = OperationService::default();
    operations
        .insert(Operation::queued("op_1", "profile.launch", scope.clone()).unwrap())
        .unwrap();
    operations
        .transition("op_1", OperationStatus::Running, Some(0.5), None, None)
        .unwrap();
    assert_eq!(
        operations
            .transition(
                "op_1",
                OperationStatus::Completed,
                Some(1.0),
                Some(json!({"accessToken": "hidden"})),
                None
            )
            .unwrap_err()
            .code,
        "secret_material_rejected"
    );

    let mut events = EventService::default();
    let mut unsafe_event = event("evt_1", scope, 1);
    unsafe_event.data = json!({"nested": {"password": "hidden"}});
    assert_eq!(
        events.publish(unsafe_event).unwrap_err().code,
        "secret_material_rejected"
    );
}

#[test]
fn event_sequences_increase_and_retained_reads_are_scoped() {
    let first_scope = ResourceScope::workspace(workspace("ws_1"));
    let second_scope = ResourceScope::workspace(workspace("ws_2"));
    let mut service = EventService::with_capacity(2).unwrap();
    assert_eq!(
        service
            .publish(event("evt_1", first_scope.clone(), 1))
            .unwrap()
            .sequence,
        1
    );
    assert_eq!(
        service
            .publish(event("evt_2", second_scope, 1))
            .unwrap()
            .sequence,
        2
    );
    assert_eq!(
        service
            .publish(event("evt_3", first_scope.clone(), 2))
            .unwrap()
            .sequence,
        3
    );
    assert_eq!(service.len(), 2);

    let principal = Principal::default();
    assert_eq!(
        service
            .read_after(&principal, &[], &first_scope, None, 2)
            .unwrap_err()
            .status,
        403
    );
    let grants = [read_grant("events.read", first_scope.clone())];
    let retained = service
        .read_after(&principal, &grants, &first_scope, None, 2)
        .unwrap();
    assert_eq!(
        retained
            .iter()
            .map(|event| event.sequence)
            .collect::<Vec<_>>(),
        vec![3]
    );
    assert_eq!(
        service
            .read_after(&principal, &grants, &first_scope, Some(0), 2)
            .unwrap_err()
            .code,
        "event_cursor_expired"
    );
}
