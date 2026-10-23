use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use buglogin_domain::entities::{Extension, ExtensionGroup};
use buglogin_migration::{
    MigrationKey, activate_package,
    inspect::inspect_source,
    legacy::map_legacy_extensions,
    package::{PackageMetadata, build_encrypted_profile_package},
    plan_package_activation,
};
use buglogin_storage::local::LocalRepository;

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct Temp(PathBuf);

impl Temp {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "buglogin-package-extension-{label}-{}-{}",
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
fn package_plan_and_activation_wire_extension_records_and_artifact_bytes() {
    let source = Temp::new("source");
    source.write(
        "version.json",
        br#"{"system":"buglogin-legacy","version":"1.1.84","instanceId":"package-extension","os":"windows","architecture":"x86_64"}"#,
    );
    source.write(
        "extensions/ext-a/metadata.json",
        br#"{"id":"ext-a","name":"Captcha","file_name":"captcha.xpi","file_type":"xpi","browser_compatibility":["firefox"],"sync_enabled":true}"#,
    );
    source.write(
        "extensions/ext-a/file/captcha.xpi",
        b"package-extension-bytes",
    );
    source.write(
        "data/extension_groups.json",
        br#"{"groups":[{"id":"group-a","name":"Firefox","extension_ids":["ext-a"],"sync_enabled":true}]}"#,
    );

    let (report, snapshot) = inspect_source(source.path()).unwrap();
    let key = MigrationKey::from_bytes([91; 32]);
    let package = build_encrypted_profile_package(
        source.path(),
        &report,
        &snapshot,
        &PackageMetadata {
            created_at: "2026-07-21T00:00:00Z".into(),
            tool_version: "0.1.0".into(),
            tool_commit: "0123456789abcdef0123456789abcdef01234567".into(),
        },
        &key,
    )
    .unwrap();
    let mapped = map_legacy_extensions(source.path(), &report.source.instance_id).unwrap();
    let destination = Temp::new("destination");
    fs::remove_dir(destination.path()).unwrap();
    let extension_id = mapped.records[0].id.clone();
    let extension_group_id = mapped.groups[0].id.clone();
    let artifact_ref = mapped.artifacts[0].artifact_ref.clone();
    let artifact_sha256 = buglogin_storage::blob::checksum(b"package-extension-bytes");

    let plan = plan_package_activation(destination.path(), &package, &key).unwrap();
    assert!(!destination.path().exists());
    assert_eq!(plan.create_extension_ids, vec![extension_id.clone()]);
    assert_eq!(
        plan.create_extension_group_ids,
        vec![extension_group_id.clone()]
    );
    assert_eq!(
        plan.create_extension_artifact_ids,
        vec![artifact_ref.clone()]
    );

    let first = activate_package(
        destination.path(),
        "package-extension",
        &package,
        Some(&key),
    )
    .unwrap();
    assert_eq!(first.created_extensions, 1);
    assert_eq!(first.created_extension_groups, 1);
    assert_eq!(first.created_extension_artifacts, 1);
    assert_eq!(
        fs::read(
            destination
                .path()
                .join("extension-artifacts")
                .join(&artifact_ref)
                .join(&artifact_sha256)
        )
        .unwrap(),
        b"package-extension-bytes"
    );
    let repository = LocalRepository::open(destination.path()).unwrap();
    assert!(
        repository
            .get::<Extension>(&extension_id)
            .unwrap()
            .is_some()
    );
    assert!(
        repository
            .get::<ExtensionGroup>(&extension_group_id)
            .unwrap()
            .is_some()
    );

    let rerun = activate_package(
        destination.path(),
        "package-extension",
        &package,
        Some(&key),
    )
    .unwrap();
    assert_eq!(rerun.reused_extensions, 1);
    assert_eq!(rerun.reused_extension_groups, 1);
    assert_eq!(rerun.reused_extension_artifacts, 1);
}
