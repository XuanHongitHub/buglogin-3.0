use buglogin_api::{http::ApiState, services::profile_hosted::HostedProfileService};
use buglogin_browser::profile::ProfileService;
use buglogin_domain::{
    entities::{Profile, ProfileState, Runtime, SyncMode},
    ids::{ProfileId, WorkspaceId},
};
use buglogin_storage::control::MemoryControlRecordStore;

fn profile() -> Profile {
    Profile {
        id: ProfileId::from_str("prf_authority").unwrap(),
        name: "Authority profile".into(),
        runtime: Runtime::Wayfern,
        runtime_version: Some("1.0.0".into()),
        runtime_config: None,
        state: ProfileState::Created,
        archived: false,
        fingerprint: None,
        note: None,
        proxy_id: None,
        vpn_id: None,
        group_ids: vec![],
        tags: vec![],
        extension_group_id: None,
        sync_mode: Some(SyncMode::Managed),
        created_at: "2026-07-22T00:00:00Z".into(),
        updated_at: "2026-07-22T00:00:00Z".into(),
    }
}

#[test]
fn provision_plan_is_deterministic_and_promotes_profile_to_ready() {
    let plan = HostedProfileService::prepare_provision(
        profile(),
        WorkspaceId::from_str("ws_authority").unwrap(),
    )
    .unwrap();
    assert_eq!(plan.profile.state, ProfileState::Ready);
    assert_eq!(plan.binding.profile_id, plan.profile.id);
    assert_eq!(plan.binding.resource_id, plan.resource_id);
    assert_eq!(plan.manifest.generation, 1);
    assert_eq!(
        HostedProfileService::managed_resource_id(&plan.profile.id).unwrap(),
        plan.resource_id
    );
}

#[test]
fn archive_restore_share_hosted_revision_authority() {
    let state = ApiState::hosted_with_control_store(
        ProfileService::disabled(),
        MemoryControlRecordStore::default(),
    )
    .unwrap();
    let plan = HostedProfileService::prepare_provision(
        profile(),
        WorkspaceId::from_str("ws_authority").unwrap(),
    )
    .unwrap();
    let principal = Default::default();
    let grants = vec![];
    let provisioned = HostedProfileService::provision(&state, &principal, &grants, &plan);
    assert!(provisioned.is_err());
}
