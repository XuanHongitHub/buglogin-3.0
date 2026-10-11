use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use buglogin_domain::ids::migration_uuid;
use buglogin_migration::{
    MigrationKey,
    inspect::inspect_source,
    legacy::map_legacy_extensions,
    package::{PackageMetadata, build_encrypted_profile_package},
};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "buglogin-extension-mapping-{}-{}",
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
fn maps_extension_artifact_and_group_into_checksum_bound_package() {
    let source = Temp::new();
    source.write(
        "version.json",
        br#"{"system":"buglogin-legacy","version":"1.1.84","instanceId":"extensions","os":"windows","architecture":"x86_64"}"#,
    );
    source.write(
        "extensions/ext-a/metadata.json",
        br#"{"id":"ext-a","name":"Captcha","file_name":"captcha.xpi","file_type":"xpi","browser_compatibility":["firefox"],"created_at":1784592000,"updated_at":1784592060,"sync_enabled":true}"#,
    );
    source.write("extensions/ext-a/file/captcha.xpi", b"synthetic-xpi-bytes");
    source.write(
        "data/extension_groups.json",
        br#"{"groups":[{"id":"group-a","name":"Firefox","extension_ids":["ext-a"],"created_at":1784592000,"updated_at":1784592060,"sync_enabled":true}]}"#,
    );

    let mapped = map_legacy_extensions(source.path(), "extensions").unwrap();
    assert_eq!(mapped.records.len(), 1);
    assert_eq!(mapped.groups.len(), 1);
    assert_eq!(mapped.artifacts.len(), 1);
    assert_eq!(
        mapped.records[0].id,
        migration_uuid("buglogin-legacy", "extensions", "extension", "ext-a").to_string()
    );
    assert_eq!(
        mapped.groups[0].data.extension_ids,
        vec![mapped.records[0].id.clone()]
    );
    assert_eq!(
        mapped.records[0].data.artifact_sha256,
        buglogin_storage::blob::checksum(b"synthetic-xpi-bytes")
    );

    let (report, snapshot) = inspect_source(source.path()).unwrap();
    let packaged_mapping =
        map_legacy_extensions(source.path(), &report.source.instance_id).unwrap();
    let package = build_encrypted_profile_package(
        source.path(),
        &report,
        &snapshot,
        &PackageMetadata {
            created_at: "2026-07-21T00:00:00Z".into(),
            tool_version: "0.1.0".into(),
            tool_commit: "0123456789abcdef0123456789abcdef01234567".into(),
        },
        &MigrationKey::from_bytes([81; 32]),
    )
    .unwrap();
    assert!(package.file("entities/extensions.jsonl").is_some());
    assert!(package.file("entities/extension-groups.jsonl").is_some());
    assert_eq!(
        package
            .file(&packaged_mapping.artifacts[0].package_path)
            .unwrap(),
        b"synthetic-xpi-bytes"
    );
}

#[test]
fn rejects_dangling_extension_group_assignment() {
    let source = Temp::new();
    source.write(
        "data/extension_groups.json",
        br#"{"groups":[{"id":"group-a","name":"Broken","extension_ids":["missing"]}]}"#,
    );
    let error = map_legacy_extensions(source.path(), "extensions").unwrap_err();
    assert_eq!(error.code, "migration_extension_invalid");
    assert_eq!(error.fields["field"], "dangling extension group assignment");
}
