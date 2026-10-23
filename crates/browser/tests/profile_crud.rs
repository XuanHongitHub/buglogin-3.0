use buglogin_browser::profile::{ProfileBindingService, ProfileService};
use buglogin_domain::{
    entities::{Profile, ProfileState, Runtime, SyncMode},
    ids::{ProfileId, ResourceId, WorkspaceId},
    profile::{ProfileArchiveState, ProfileResource},
};
use buglogin_storage::local::LocalRepository;
use std::{fs, path::PathBuf};

fn profile() -> Profile {
    Profile {
        id: ProfileId::from_str("prf_1").unwrap(),
        name: "Profile".into(),
        runtime: Runtime::Wayfern,
        runtime_version: None,
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
        sync_mode: Some(SyncMode::Local),
        created_at: "2026-07-20T00:00:00Z".into(),
        updated_at: "2026-07-20T00:00:00Z".into(),
    }
}

#[test]
fn profile_crud_archive_restore_is_revision_safe() {
    let root: PathBuf =
        std::env::temp_dir().join(format!("buglogin-profile-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let service = ProfileService::new(LocalRepository::open(&root).unwrap());
    let created = service.create(profile()).unwrap();
    let archived = service.archive(created.revision, created.value).unwrap();
    assert!(archived.value.archived);
    let restored = service.restore(archived.revision, archived.value).unwrap();
    assert!(!restored.value.archived);
    assert_eq!(
        service
            .get(&ProfileId::from_str("prf_1").unwrap())
            .unwrap()
            .unwrap()
            .revision,
        3
    );
    let profiles = service.list().unwrap();
    assert_eq!(profiles.len(), 1);
    assert_eq!(profiles[0].value.id.as_str(), "prf_1");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn profile_binding_persists_exact_revision_workspace_resource_roundtrip() {
    let root: PathBuf =
        std::env::temp_dir().join(format!("buglogin-profile-binding-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let service = ProfileBindingService::new(LocalRepository::open(&root).unwrap());
    let profile_id = ProfileId::from_str("prf_1").unwrap();
    let created = service
        .create(ProfileResource {
            profile_id: profile_id.clone(),
            resource_id: ResourceId::from_str("res_1").unwrap(),
            workspace_id: WorkspaceId::from_str("ws_1").unwrap(),
            archive_state: ProfileArchiveState::Active,
        })
        .unwrap();
    assert_eq!(created.revision, 1);
    assert_eq!(created.value.profile_id, profile_id);
    assert_eq!(created.value.resource_id.as_str(), "res_1");
    assert_eq!(created.value.workspace_id.as_str(), "ws_1");

    let updated = service
        .update(
            created.revision,
            ProfileResource {
                profile_id: created.value.profile_id,
                resource_id: ResourceId::from_str("res_2").unwrap(),
                workspace_id: WorkspaceId::from_str("ws_2").unwrap(),
                archive_state: ProfileArchiveState::Archived,
            },
        )
        .unwrap();
    assert_eq!(updated.revision, 2);

    let persisted = service.get(&profile_id).unwrap().unwrap();
    assert_eq!(persisted, updated);
    assert_eq!(service.list().unwrap(), vec![persisted]);
    assert!(
        root.join("profile-resources/prf_1/revisions/00000000000000000002.json")
            .is_file()
    );
    let _ = fs::remove_dir_all(root);
}
