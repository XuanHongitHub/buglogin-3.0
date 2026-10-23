use buglogin_domain::entities::{Proxy, ProxyProtocol};
use buglogin_domain::ids::{CredentialId, IdError, ProfileId, ProxyId, WorkspaceId};
use buglogin_domain::permissions::{Principal, Role};
use buglogin_network::proxy::repository::{ProxyRepositorySnapshot, ProxySnapshotRecord};
use buglogin_network::proxy::{
    AssignmentDelete, AssignmentError, AssignmentUpdate, ProxyAssignmentRepository, ProxyDelete,
    ProxyRepository, ProxyUpdate, RepositoryError,
};
use std::collections::BTreeSet;

fn id<T>(value: &str, parse: fn(&str) -> Result<T, IdError>) -> T {
    parse(value).unwrap()
}

fn principal(workspace: &WorkspaceId, role: Role) -> Principal {
    Principal {
        workspace_ids: BTreeSet::from([workspace.clone()]),
        roles: BTreeSet::from([role]),
        ..Principal::default()
    }
}

fn proxy(id_value: &str, host: &str, port: u16, credential: Option<&str>) -> Proxy {
    Proxy {
        id: id(id_value, ProxyId::from_str),
        name: Some(id_value.to_owned()),
        protocol: ProxyProtocol::Http,
        host: host.to_owned(),
        port,
        credential_ref: credential.map(|value| id(value, CredentialId::from_str)),
        rotation_url: None,
        bypass_rules: Vec::new(),
        created_at: "2026-07-22T00:00:00Z".to_owned(),
        updated_at: "2026-07-22T00:00:00Z".to_owned(),
    }
}

#[test]
fn inventory_crud_deduplicates_endpoint_and_scopes_workspaces() {
    let workspace = id("ws_proxy", WorkspaceId::from_str);
    let other_workspace = id("ws_other", WorkspaceId::from_str);
    let admin = principal(&workspace, Role::WorkspaceAdmin);
    let other_admin = principal(&other_workspace, Role::WorkspaceAdmin);
    let mut repository = ProxyRepository::default();

    let created = repository
        .create(
            workspace.clone(),
            proxy("proxy_one", " Example.COM ", 8080, Some("credential_one")),
            &admin,
            &[],
        )
        .unwrap();
    assert!(!created.deduplicated);
    assert_eq!(created.record.revision, 1);
    assert_eq!(created.record.proxy.host, "example.com");
    assert_eq!(
        created.record.proxy.credential_ref,
        Some(id("credential_one", CredentialId::from_str))
    );

    let duplicate = repository
        .create(
            workspace.clone(),
            proxy(
                "proxy_duplicate",
                "example.com",
                8080,
                Some("credential_two"),
            ),
            &admin,
            &[],
        )
        .unwrap();
    assert!(duplicate.deduplicated);
    assert_eq!(duplicate.record.proxy.id, created.record.proxy.id);
    assert_eq!(repository.list(&workspace, &admin, &[]).unwrap().len(), 1);

    repository
        .create(
            other_workspace.clone(),
            proxy("proxy_other", "example.com", 8080, None),
            &other_admin,
            &[],
        )
        .unwrap();
    assert_eq!(
        repository
            .list(&other_workspace, &other_admin, &[])
            .unwrap()
            .len(),
        1
    );

    let mut changed = created.record.proxy.clone();
    changed.host = "proxy.example".to_owned();
    changed.credential_ref = Some(id("credential_replaced", CredentialId::from_str));
    let updated = repository
        .update(
            &workspace,
            &changed.id.clone(),
            ProxyUpdate {
                proxy: changed.clone(),
                expected_revision: 1,
            },
            &admin,
            &[],
        )
        .unwrap();
    assert_eq!(updated.revision, 2);
    assert_eq!(updated.proxy.credential_ref, changed.credential_ref);
    assert_eq!(
        repository.update(
            &workspace,
            &changed.id,
            ProxyUpdate {
                proxy: changed.clone(),
                expected_revision: 1,
            },
            &admin,
            &[],
        ),
        Err(RepositoryError::RevisionConflict {
            expected: 1,
            actual: 2,
        })
    );

    let removed = repository
        .delete(
            &workspace,
            &changed.id,
            ProxyDelete {
                expected_revision: 2,
            },
            &ProxyAssignmentRepository::default(),
            &admin,
            &[],
        )
        .unwrap();
    assert_eq!(removed.proxy.id, changed.id);
    assert!(repository.list(&workspace, &admin, &[]).unwrap().is_empty());
}

#[test]
fn assignment_crud_enforces_cas_and_proxy_delete_integrity() {
    let workspace = id("ws_assign", WorkspaceId::from_str);
    let profile = id("profile_assign", ProfileId::from_str);
    let admin = principal(&workspace, Role::WorkspaceAdmin);
    let mut proxies = ProxyRepository::default();
    let first = proxies
        .create(
            workspace.clone(),
            proxy("proxy_first", "first.example", 8080, None),
            &admin,
            &[],
        )
        .unwrap()
        .record;
    let second = proxies
        .create(
            workspace.clone(),
            proxy("proxy_second", "second.example", 8080, None),
            &admin,
            &[],
        )
        .unwrap()
        .record;
    let mut assignments = ProxyAssignmentRepository::default();

    let created = assignments
        .create(
            &proxies,
            first.proxy.id.clone(),
            profile.clone(),
            workspace.clone(),
            &admin,
            &[],
        )
        .unwrap();
    assert_eq!(created.revision, 1);
    assert_eq!(
        assignments.list(&workspace, &admin, &[]).unwrap(),
        [created]
    );
    assignments.check_integrity(&proxies, &workspace).unwrap();

    let updated = assignments
        .update(
            &proxies,
            &workspace,
            &profile,
            AssignmentUpdate {
                proxy_id: second.proxy.id.clone(),
                expected_revision: 1,
            },
            &admin,
            &[],
        )
        .unwrap();
    assert_eq!(updated.revision, 2);
    assert_eq!(updated.proxy_id, second.proxy.id);
    assert_eq!(
        assignments.update(
            &proxies,
            &workspace,
            &profile,
            AssignmentUpdate {
                proxy_id: first.proxy.id,
                expected_revision: 1,
            },
            &admin,
            &[],
        ),
        Err(AssignmentError::RevisionConflict {
            expected: 1,
            actual: 2,
        })
    );
    assert_eq!(
        proxies.delete(
            &workspace,
            &second.proxy.id,
            ProxyDelete {
                expected_revision: 1,
            },
            &assignments,
            &admin,
            &[],
        ),
        Err(RepositoryError::ProxyAssigned)
    );

    assignments
        .delete(
            &workspace,
            &profile,
            AssignmentDelete {
                expected_revision: 2,
            },
            &admin,
            &[],
        )
        .unwrap();
    assignments.check_integrity(&proxies, &workspace).unwrap();
    proxies
        .delete(
            &workspace,
            &second.proxy.id,
            ProxyDelete {
                expected_revision: 1,
            },
            &assignments,
            &admin,
            &[],
        )
        .unwrap();
}

#[test]
fn assignment_rejects_cross_workspace_proxy() {
    let workspace = id("ws_scope", WorkspaceId::from_str);
    let other_workspace = id("ws_scope_other", WorkspaceId::from_str);
    let profile = id("profile_scope", ProfileId::from_str);
    let admin = principal(&workspace, Role::WorkspaceAdmin);
    let other_admin = principal(&other_workspace, Role::WorkspaceAdmin);
    let mut proxies = ProxyRepository::default();
    let proxy_id = proxies
        .create(
            workspace.clone(),
            proxy("proxy_scope", "scope.example", 8080, None),
            &admin,
            &[],
        )
        .unwrap()
        .record
        .proxy
        .id;

    assert_eq!(
        ProxyAssignmentRepository::default().create(
            &proxies,
            proxy_id,
            profile,
            other_workspace,
            &other_admin,
            &[],
        ),
        Err(AssignmentError::ProxyNotFound)
    );
}

#[test]
fn proxy_snapshot_roundtrip_is_ordered_and_rebuilds_endpoint_index() {
    let workspace = id("ws_snapshot", WorkspaceId::from_str);
    let admin = principal(&workspace, Role::WorkspaceAdmin);
    let mut repository = ProxyRepository::default();
    repository
        .create(
            workspace.clone(),
            proxy("proxy_b", "b.example", 8080, None),
            &admin,
            &[],
        )
        .unwrap();
    repository
        .create(
            workspace.clone(),
            proxy("proxy_a", "a.example", 8080, None),
            &admin,
            &[],
        )
        .unwrap();

    let snapshot = repository.snapshot();
    assert_eq!(
        snapshot
            .records
            .iter()
            .map(|record| record.proxy.id.as_str())
            .collect::<Vec<_>>(),
        ["proxy_a", "proxy_b"]
    );
    let encoded = serde_json::to_string(&snapshot).unwrap();
    let mut restored = ProxyRepository::restore(serde_json::from_str(&encoded).unwrap()).unwrap();
    assert_eq!(restored.snapshot(), snapshot);
    let duplicate = restored
        .create(
            workspace.clone(),
            proxy("proxy_duplicate", "a.example", 8080, None),
            &admin,
            &[],
        )
        .unwrap();
    assert!(duplicate.deduplicated);
    assert_eq!(duplicate.record.proxy.id.as_str(), "proxy_a");

    let updated = restored
        .update(
            &workspace,
            &id("proxy_a", ProxyId::from_str),
            ProxyUpdate {
                proxy: proxy("proxy_a", "restored.example", 8080, None),
                expected_revision: 1,
            },
            &admin,
            &[],
        )
        .unwrap();
    assert_eq!(updated.revision, 2);
}

#[test]
fn proxy_snapshot_rejects_duplicate_ids_endpoints_and_invalid_records() {
    let workspace = id("ws_snapshot_invalid", WorkspaceId::from_str);
    let first = ProxySnapshotRecord {
        workspace_id: workspace.as_str().to_owned(),
        proxy: proxy("proxy_first", "same.example", 8080, None),
        revision: 1,
    };

    let duplicate_id = ProxySnapshotRecord {
        proxy: proxy("proxy_first", "other.example", 8080, None),
        ..first.clone()
    };
    assert!(matches!(
        ProxyRepository::restore(ProxyRepositorySnapshot {
            schema_version: ProxyRepository::SNAPSHOT_SCHEMA_VERSION,
            records: vec![first.clone(), duplicate_id],
        }),
        Err(RepositoryError::SnapshotDuplicateProxyId)
    ));

    let duplicate_endpoint = ProxySnapshotRecord {
        proxy: proxy("proxy_second", "same.example", 8080, None),
        ..first.clone()
    };
    assert!(matches!(
        ProxyRepository::restore(ProxyRepositorySnapshot {
            schema_version: ProxyRepository::SNAPSHOT_SCHEMA_VERSION,
            records: vec![first.clone(), duplicate_endpoint],
        }),
        Err(RepositoryError::SnapshotDuplicateEndpoint)
    ));

    assert!(matches!(
        ProxyRepository::restore(ProxyRepositorySnapshot {
            schema_version: 99,
            records: vec![],
        }),
        Err(RepositoryError::SnapshotVersionUnsupported {
            expected: ProxyRepository::SNAPSHOT_SCHEMA_VERSION,
            actual: 99,
        })
    ));
    assert!(matches!(
        ProxyRepository::restore(ProxyRepositorySnapshot {
            schema_version: ProxyRepository::SNAPSHOT_SCHEMA_VERSION,
            records: vec![ProxySnapshotRecord {
                revision: 0,
                ..first.clone()
            }],
        }),
        Err(RepositoryError::SnapshotInvalidRevision)
    ));
    assert!(matches!(
        ProxyRepository::restore(ProxyRepositorySnapshot {
            schema_version: ProxyRepository::SNAPSHOT_SCHEMA_VERSION,
            records: vec![ProxySnapshotRecord {
                workspace_id: "".into(),
                ..first
            }],
        }),
        Err(RepositoryError::SnapshotInvalidWorkspace)
    ));
}
