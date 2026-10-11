use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use buglogin_domain::{
    entities::{Profile, ProfileState, Runtime},
    ids::ProfileId,
};
use buglogin_migration::rollback::{confirm_rollback, rollback_activation};
use buglogin_storage::local::LocalRepository;
use serde_json::json;

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct Temp(PathBuf);

impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "buglogin-rollback-retry-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn retry_completes_after_checkpointed_partial_quarantine() {
    let destination = Temp::new();
    let repository = LocalRepository::open(&destination.0).unwrap();
    let profile = Profile {
        id: ProfileId::new("profile-retry").unwrap(),
        name: "Retry".into(),
        runtime: Runtime::Wayfern,
        runtime_version: None,
        runtime_config: None,
        state: ProfileState::Created,
        archived: false,
        fingerprint: None,
        note: None,
        proxy_id: None,
        vpn_id: None,
        group_ids: Vec::new(),
        tags: Vec::new(),
        extension_group_id: None,
        sync_mode: None,
        created_at: "2026-07-21T00:00:00Z".into(),
        updated_at: "2026-07-21T00:00:00Z".into(),
    };
    let version = repository.create(profile.clone()).unwrap();
    let mut remaining_profile = profile.clone();
    remaining_profile.id = ProfileId::new("profile-remaining").unwrap();
    remaining_profile.name = "Remaining".into();
    let remaining_version = repository.create(remaining_profile.clone()).unwrap();
    let journal_directory = destination.0.join("migration-activations");
    fs::create_dir_all(&journal_directory).unwrap();
    let journal_path = journal_directory.join("retry-run.json");
    fs::write(
        &journal_path,
        serde_json::to_vec_pretty(&json!({
            "schema": "buglogin.activation/1",
            "runId": "retry-run",
            "status": "active",
            "inputSha256": "test-input",
            "credentials": [],
            "extensionArtifacts": [],
            "extensions": [],
            "extensionGroups": [],
            "groups": [],
            "proxies": [],
            "profiles": [{
                "profileId": profile.id.as_str(),
                "revision": version.revision,
                "sha256": buglogin_storage::blob::checksum(&serde_json::to_vec(&profile).unwrap()),
                "created": true
            }, {
                "profileId": remaining_profile.id.as_str(),
                "revision": remaining_version.revision,
                "sha256": buglogin_storage::blob::checksum(&serde_json::to_vec(&remaining_profile).unwrap()),
                "created": true
            }],
            "vpnConfigs": [],
            "vpns": [],
            "rollbackProgress": { "started": ["profile:profile-retry"], "completed": [] }
        }))
        .unwrap(),
    )
    .unwrap();

    let quarantine = repository
        .quarantine::<Profile>(version.revision, profile.id.as_str())
        .unwrap();
    assert!(quarantine.exists());

    let receipt = rollback_activation(
        &destination.0,
        "retry-run",
        confirm_rollback("ROLLBACK_MIGRATION_RUN").unwrap(),
    )
    .unwrap();

    assert_eq!(receipt.quarantined_profiles.len(), 1);
    assert!(
        repository
            .get::<Profile>(profile.id.as_str())
            .unwrap()
            .is_none()
    );
    assert!(
        repository
            .get::<Profile>(remaining_profile.id.as_str())
            .unwrap()
            .is_none()
    );
    let journal: serde_json::Value =
        serde_json::from_slice(&fs::read(journal_path).unwrap()).unwrap();
    assert_eq!(journal["status"], "rolled_back");
}
