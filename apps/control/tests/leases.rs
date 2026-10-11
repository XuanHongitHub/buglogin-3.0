use buglogin_api::services::leases::LeaseService;
use buglogin_domain::ids::{ResourceId, WorkspaceId};

fn ids() -> (ResourceId, WorkspaceId) {
    (
        ResourceId::from_str("res_1").unwrap(),
        WorkspaceId::from_str("ws_1").unwrap(),
    )
}

#[test]
fn renew_and_release_require_owner_token() {
    let (resource, workspace) = ids();
    let mut service = LeaseService::default();
    let lease = service
        .acquire(resource.clone(), workspace, "member_1", 100, 30)
        .unwrap();
    assert_eq!(
        service
            .renew(&resource, "member_2", &lease.token, 110, 30)
            .unwrap_err()
            .code,
        "lease_not_owner"
    );
    let renewed = service
        .renew(&resource, "member_1", &lease.token, 110, 40)
        .unwrap();
    assert_eq!(renewed.expires_at, 150);
    service
        .release(&resource, "member_1", &renewed.token, 120)
        .unwrap();
    assert!(service.current(&resource, 120).is_none());
}

#[test]
fn expired_lease_is_reclaimable_after_partition() {
    let (resource, workspace) = ids();
    let mut service = LeaseService::default();
    let old = service
        .acquire(
            resource.clone(),
            workspace.clone(),
            "offline-client",
            100,
            10,
        )
        .unwrap();
    assert_eq!(
        service
            .acquire(resource.clone(), workspace, "recovered-client", 109, 10)
            .unwrap_err()
            .code,
        "resource_lease_held"
    );
    let recovered = service
        .acquire(
            resource,
            WorkspaceId::from_str("ws_1").unwrap(),
            "recovered-client",
            110,
            10,
        )
        .unwrap();
    assert_ne!(old.lease_id, recovered.lease_id);
    assert!(recovered.fencing_token > old.fencing_token);
    assert_eq!(recovered.owner_id, "recovered-client");
}

#[test]
fn expired_local_lock_cannot_be_renewed_or_leave_permanent_lock() {
    let (resource, workspace) = ids();
    let mut service = LeaseService::default();
    let lease = service
        .acquire(resource.clone(), workspace, "client", 1, 1)
        .unwrap();
    assert_eq!(
        service
            .renew(&resource, "client", &lease.token, 2, 10)
            .unwrap_err()
            .code,
        "lease_expired"
    );
    let replacement = service
        .acquire(
            resource.clone(),
            WorkspaceId::from_str("ws_1").unwrap(),
            "new-client",
            2,
            10,
        )
        .unwrap();
    assert_eq!(replacement.owner_id, "new-client");
}

#[test]
fn invalid_ttl_is_rejected() {
    let (resource, workspace) = ids();
    let mut service = LeaseService::default();
    assert_eq!(
        service
            .acquire(resource, workspace, "client", 1, 0)
            .unwrap_err()
            .code,
        "invalid_lease_ttl"
    );
}
