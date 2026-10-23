use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use buglogin_migration::snapshot::create_snapshot;
use buglogin_migration::transform::{ProfileDataClassification, map_legacy_profiles};

static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);
const SECRET_CANARY: &str = "legacy-secret-canary";

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "buglogin-legacy-mapping-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create test directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn write_profile(&self, id: &str, metadata: &str) {
        let profile = self.0.join("profiles").join(id);
        fs::create_dir_all(profile.join("profile")).expect("create profile directory");
        fs::write(profile.join("metadata.json"), metadata).expect("write metadata");
        fs::write(profile.join("profile/Preferences"), "immutable source")
            .expect("write profile data");
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn maps_all_legacy_runtimes_to_canonical_destination_records() {
    let source = TestDirectory::new();
    let runtimes = [
        (
            "00000000-0000-0000-0000-000000000001",
            "wayfern",
            "wayfern",
            "direct",
        ),
        (
            "00000000-0000-0000-0000-000000000002",
            "bugox",
            "bugox",
            "unsupported",
        ),
        (
            "00000000-0000-0000-0000-000000000003",
            "camoufox",
            "camoufox",
            "unsupported",
        ),
        (
            "00000000-0000-0000-0000-000000000004",
            "bugium",
            "bugium",
            "unsupported",
        ),
        (
            "00000000-0000-0000-0000-000000000005",
            "bugmini",
            "bugmini",
            "unsupported",
        ),
        (
            "00000000-0000-0000-0000-000000000006",
            "cloak",
            "cloak",
            "unsupported",
        ),
        (
            "00000000-0000-0000-0000-000000000007",
            "bugthor",
            "bugthor",
            "unsupported",
        ),
        (
            "00000000-0000-0000-0000-000000000008",
            "thorium",
            "thorium",
            "unsupported",
        ),
        (
            "00000000-0000-0000-0000-000000000009",
            "bughelium",
            "bughelium",
            "unsupported",
        ),
        (
            "00000000-0000-0000-0000-000000000010",
            "helium",
            "helium",
            "unsupported",
        ),
    ];
    for (id, runtime, _, _) in runtimes {
        source.write_profile(
            id,
            &format!(
                r#"{{"id":"{id}","name":"{runtime}","browser":"{runtime}","version":"1.0.0"}}"#
            ),
        );
    }

    let before = create_snapshot(source.path()).expect("snapshot before mapping");
    let first = map_legacy_profiles(source.path(), "legacy-instance").expect("map profiles");
    let second = map_legacy_profiles(source.path(), "legacy-instance").expect("map again");
    let after = create_snapshot(source.path()).expect("snapshot after mapping");

    assert_eq!(before, after);
    assert_eq!(first, second);
    for ((_, source_runtime, canonical, class), record) in runtimes.iter().zip(&first.records) {
        assert_eq!(&record.data.runtime, canonical);
        assert_eq!(record.data.runtime_version.as_deref(), Some("1.0.0"));
        assert_eq!(&record.data.migration_class, class);
        assert_eq!(record.extensions["legacy_runtime"], *source_runtime);
        assert_eq!(record.schema, "buglogin.profile/1");
        assert_eq!(record.revision, 1);
        assert_eq!(record.id.len(), 36);
    }
}

#[test]
fn excludes_profile_data_without_leaking_unencrypted_secrets() {
    let source = TestDirectory::new();
    source.write_profile(
        "10000000-0000-0000-0000-000000000001",
        &format!(
            r#"{{"id":"10000000-0000-0000-0000-000000000001","name":"Secret","runtime":"wayfern","proxyPassword":"{SECRET_CANARY}","browser_adapter_config":{{"password":"{SECRET_CANARY}"}}}}"#
        ),
    );
    source.write_profile(
        "10000000-0000-0000-0000-000000000002",
        r#"{"id":"10000000-0000-0000-0000-000000000002","name":"Encrypted","runtime":"wayfern","sync_mode":"Encrypted","encryption_salt":"public-salt"}"#,
    );

    let mapping = map_legacy_profiles(source.path(), "legacy-instance").expect("map profiles");
    let excluded = &mapping.records[0];
    assert_eq!(
        excluded.data.source_data_object.disposition.classification,
        ProfileDataClassification::Excluded
    );
    assert_eq!(
        excluded.data.source_data_object.disposition.reason_code,
        "source_profile_data_contains_unencrypted_secrets"
    );
    assert_eq!(
        excluded.data.legacy_runtime_config.as_ref().unwrap()["classification"],
        "excluded"
    );
    assert!(
        !serde_json::to_string(&mapping.records)
            .unwrap()
            .contains(SECRET_CANARY)
    );

    let encrypted = &mapping.records[1];
    assert_eq!(
        encrypted.data.source_data_object.disposition.classification,
        ProfileDataClassification::Excluded
    );
    assert_eq!(
        encrypted.data.source_data_object.disposition.reason_code,
        "source_profile_data_not_authenticated_encrypted"
    );
}

#[test]
fn unknown_runtime_is_preserved_as_non_launchable_hashed_provenance() {
    let source = TestDirectory::new();
    source.write_profile(
        "20000000-0000-0000-0000-000000000001",
        r#"{"id":"20000000-0000-0000-0000-000000000001","name":"Unknown","runtime":"private-runtime"}"#,
    );

    let mapping = map_legacy_profiles(source.path(), "legacy-instance").expect("map profiles");
    let record = &mapping.records[0];
    assert!(record.data.runtime.starts_with("unknown:sha256:"));
    assert_eq!(record.data.migration_class, "unsupported");
    assert_eq!(record.extensions["legacy_runtime"], "private-runtime");
}

#[test]
fn rejects_metadata_id_that_does_not_match_profile_directory() {
    let source = TestDirectory::new();
    source.write_profile(
        "30000000-0000-0000-0000-000000000001",
        r#"{"id":"30000000-0000-0000-0000-000000000002","name":"Mismatch","runtime":"wayfern"}"#,
    );

    let error = map_legacy_profiles(source.path(), "legacy-instance").unwrap_err();
    assert_eq!(error.code, "migration_legacy_metadata_invalid");
}
