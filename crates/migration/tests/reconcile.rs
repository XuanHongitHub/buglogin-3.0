use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use buglogin_domain::{entities::Profile, ids::migration_uuid};
use buglogin_migration::activate::activate_profiles;
use buglogin_migration::inspect::inspect_source;
use buglogin_migration::reconcile::{ReconciliationStatus, reconcile_activation};
use buglogin_migration::transform::{
    NeutralProfileData, NeutralProfileRecord, ProfileDataClassification, ProfileDataDisposition,
    ProfileDataObject, ProfileSourceRef,
};
use buglogin_storage::local::LocalRepository;
use serde_json::Map;

static SEQUENCE: AtomicU64 = AtomicU64::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "buglogin-reconcile-{label}-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self) -> &Path {
        &self.0
    }
    fn write(&self, relative: &str, value: &[u8]) {
        let path = self.0.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, value).unwrap();
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn reconciliation_proves_owned_profiles_and_blocks_drift_or_unsupported_entities() {
    let source = Temp::new("source");
    source.write("version.json", br#"{"system":"buglogin-legacy","version":"1.1.84","instanceId":"reconcile","os":"windows","architecture":"x86_64"}"#);
    source.write(
        "profiles/one/metadata.json",
        br#"{"id":"one","name":"One","runtime":"wayfern"}"#,
    );
    let (report, _) = inspect_source(source.path()).unwrap();
    let destination = Temp::new("destination");
    let record = profile_record();
    activate_profiles(destination.path(), "reconcile-run", &[record.clone()]).unwrap();

    let passed = reconcile_activation(&report, destination.path(), "reconcile-run").unwrap();
    assert_eq!(passed.status, ReconciliationStatus::Passed);
    assert_eq!(passed.expected_profiles, 1);
    assert_eq!(passed.activated_profiles, 1);

    let repository = LocalRepository::open(destination.path()).unwrap();
    let mut profile = repository
        .get::<Profile>(&record.id)
        .unwrap()
        .unwrap()
        .value;
    profile.name = "Changed".into();
    repository.update(1, profile).unwrap();
    let drift = reconcile_activation(&report, destination.path(), "reconcile-run").unwrap();
    assert_eq!(drift.status, ReconciliationStatus::Blocked);
    assert_eq!(drift.mismatched_profile_ids, vec![record.id.clone()]);

    let mut unsupported = report;
    unsupported.entity_counts.insert("proxies".into(), 1);
    let blocked = reconcile_activation(&unsupported, destination.path(), "reconcile-run").unwrap();
    assert_eq!(blocked.status, ReconciliationStatus::Blocked);
    assert_eq!(blocked.expected_proxies, 1);
    assert_eq!(blocked.activated_proxies, 0);
    assert!(!blocked.unsupported_entity_counts.contains_key("proxies"));
}

fn profile_record() -> NeutralProfileRecord {
    let source_id = "one";
    NeutralProfileRecord {
        schema: "buglogin.profile/1".into(),
        source_ref: ProfileSourceRef {
            system: "buglogin-legacy".into(),
            instance_id: "reconcile".into(),
            id: source_id.into(),
        },
        id: migration_uuid("buglogin-legacy", "reconcile", "profile", source_id).to_string(),
        revision: 1,
        created_at: Some("2026-07-21T00:00:00Z".into()),
        updated_at: Some("2026-07-21T00:00:00Z".into()),
        data: NeutralProfileData {
            name: "One".into(),
            runtime: "wayfern".into(),
            runtime_version: None,
            release_type: "stable".into(),
            ephemeral: false,
            source_data_object: ProfileDataObject {
                path: "profiles/one/profile".into(),
                logical_type: "profile-directory".into(),
                disposition: ProfileDataDisposition {
                    classification: ProfileDataClassification::Excluded,
                    reason_code: "redacted".into(),
                },
            },
            migration_class: "direct".into(),
            group_id: None,
            proxy_id: None,
            vpn_id: None,
            extension_group_id: None,
            tags: vec![],
            note: None,
            legacy_runtime_config: None,
        },
        extensions: Map::new(),
    }
}
