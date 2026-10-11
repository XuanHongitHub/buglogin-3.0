use buglogin_browser::profile::{
    ProfileService,
    bulk::{BulkOperation, BulkProfileService, BulkTarget, BulkTargetStatus},
};
use buglogin_domain::{
    entities::{Profile, ProfileState, Runtime, SyncMode},
    ids::{AccountId, GrantId, GroupId, IdError, ProfileId, WorkspaceId},
    permissions::{Action, Capability, Effect, Grant, Principal, ResourceScope, Role},
};
use buglogin_storage::local::LocalRepository;
use std::{collections::BTreeSet, fs, path::PathBuf};

fn id<T>(value: &str, parser: fn(&str) -> Result<T, IdError>) -> T {
    parser(value).unwrap()
}

fn profile(value: &str, group_ids: Vec<GroupId>) -> Profile {
    Profile {
        id: id(value, ProfileId::from_str),
        name: value.into(),
        runtime: Runtime::Wayfern,
        runtime_version: None,
        runtime_config: None,
        state: ProfileState::Created,
        archived: false,
        fingerprint: None,
        note: None,
        proxy_id: None,
        vpn_id: None,
        group_ids,
        tags: vec![],
        extension_group_id: None,
        sync_mode: Some(SyncMode::Local),
        created_at: "2026-07-20T00:00:00Z".into(),
        updated_at: "2026-07-20T00:00:00Z".into(),
    }
}

#[test]
fn bulk_group_assignment_reports_every_target() {
    let root: PathBuf =
        std::env::temp_dir().join(format!("buglogin-profile-bulk-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let repository = LocalRepository::open(&root).unwrap();
    let profiles = ProfileService::new(repository.clone());
    let workspace = id("ws_1", WorkspaceId::from_str);
    let group = id("grp_1", GroupId::from_str);
    profiles.create(profile("prf_1", vec![])).unwrap();
    profiles.create(profile("prf_2", vec![])).unwrap();
    profiles
        .create(profile("prf_3", vec![group.clone()]))
        .unwrap();
    let principal = Principal {
        account_id: Some(id("acct_1", AccountId::from_str)),
        workspace_ids: BTreeSet::from([workspace.clone()]),
        roles: BTreeSet::from([Role::WorkspaceOperator]),
        ..Principal::default()
    };
    let denied = Grant {
        id: id("grant_1", GrantId::from_str),
        account_id: principal.account_id.clone(),
        member_id: None,
        capability: Capability::new("CAP-PROF-003").unwrap(),
        action: Action::Update,
        scope: ResourceScope {
            workspace_id: Some(workspace.clone()),
            profile_id: Some(id("prf_2", ProfileId::from_str)),
            ..ResourceScope::default()
        },
        effect: Effect::Deny,
    };
    let report = BulkProfileService::new(repository).execute(
        &workspace,
        [
            BulkTarget {
                profile_id: id("prf_1", ProfileId::from_str),
                expected_revision: 1,
            },
            BulkTarget {
                profile_id: id("prf_2", ProfileId::from_str),
                expected_revision: 1,
            },
            BulkTarget {
                profile_id: id("prf_missing", ProfileId::from_str),
                expected_revision: 1,
            },
            BulkTarget {
                profile_id: id("prf_3", ProfileId::from_str),
                expected_revision: 1,
            },
        ],
        BulkOperation::AssignGroup(group.clone()),
        &principal,
        &[denied],
    );

    assert_eq!(report.results.len(), 4);
    assert_eq!(
        (
            report.success_count(),
            report.denied_count(),
            report.failed_count(),
            report.unchanged_count()
        ),
        (1, 1, 1, 1)
    );
    assert_eq!(
        report
            .results
            .iter()
            .map(|item| item.status)
            .collect::<Vec<_>>(),
        vec![
            BulkTargetStatus::Success,
            BulkTargetStatus::Denied,
            BulkTargetStatus::Failed,
            BulkTargetStatus::Unchanged
        ]
    );
    assert!(report.results[1].problem.is_some());
    assert_eq!(
        profiles
            .get(&id("prf_1", ProfileId::from_str))
            .unwrap()
            .unwrap()
            .value
            .group_ids,
        vec![group]
    );
    assert!(
        profiles
            .get(&id("prf_2", ProfileId::from_str))
            .unwrap()
            .unwrap()
            .value
            .group_ids
            .is_empty()
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn bulk_rejects_stale_revision_without_mutation() {
    let root = std::env::temp_dir().join(format!("buglogin-profile-stale-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let repository = LocalRepository::open(&root).unwrap();
    let profiles = ProfileService::new(repository.clone());
    profiles.create(profile("prf_1", vec![])).unwrap();
    let workspace = id("ws_1", WorkspaceId::from_str);
    let principal = Principal {
        roles: BTreeSet::from([Role::LocalOwner]),
        ..Principal::default()
    };
    let report = BulkProfileService::new(repository).execute(
        &workspace,
        [BulkTarget {
            profile_id: id("prf_1", ProfileId::from_str),
            expected_revision: 9,
        }],
        BulkOperation::Archive,
        &principal,
        &[],
    );
    assert_eq!(report.results[0].status, BulkTargetStatus::Failed);
    assert!(
        !profiles
            .get(&id("prf_1", ProfileId::from_str))
            .unwrap()
            .unwrap()
            .value
            .archived
    );
    let _ = fs::remove_dir_all(root);
}
