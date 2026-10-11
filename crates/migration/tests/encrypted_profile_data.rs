use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use buglogin_migration::encrypted_data::{
    decrypt_packaged_profile_data, encrypt_legacy_profile_data,
};
use buglogin_migration::inspect::inspect_source;
use buglogin_migration::package::{PackageMetadata, build_encrypted_profile_package};
use buglogin_migration::snapshot::create_snapshot;
use buglogin_storage::blob::checksum;
use buglogin_sync::crypto::ClientKey;

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "buglogin-encrypted-profile-{label}-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("create test directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn source() -> TestDirectory {
    let source = TestDirectory::new("source");
    fs::write(
        source.path().join("version.json"),
        r#"{"system":"buglogin-legacy","version":"1.1.84","instanceId":"encrypted-fixture","os":"windows","architecture":"x86_64"}"#,
    ).expect("write version");
    let profile = source
        .path()
        .join("profiles/00000000-0000-0000-0000-000000000001");
    fs::create_dir_all(profile.join("profile/nested")).expect("create profile");
    fs::write(
        profile.join("metadata.json"),
        r#"{"id":"00000000-0000-0000-0000-000000000001","name":"Encrypted","runtime":"wayfern"}"#,
    )
    .expect("write metadata");
    fs::write(profile.join("profile/Cookies"), b"cookie-value").expect("write cookie");
    fs::write(profile.join("profile/nested/Preferences"), b"profile-bytes")
        .expect("write preferences");
    source
}

#[test]
fn packages_encrypted_profile_data_without_plaintext_and_sets_manifest_security() {
    let source = source();
    let (report, snapshot) = inspect_source(source.path()).expect("inspect source");
    let package = build_encrypted_profile_package(
        source.path(),
        &report,
        &snapshot,
        &PackageMetadata {
            created_at: "2026-07-20T00:00:00Z".to_owned(),
            tool_version: "0.1.0".to_owned(),
            tool_commit: "test".to_owned(),
        },
        &ClientKey::from_bytes([7; 32]),
    )
    .expect("build encrypted package");
    let manifest: serde_json::Value =
        serde_json::from_slice(package.file("manifest.json").unwrap()).unwrap();
    assert_eq!(manifest["security"]["encrypted"], true);
    assert_eq!(manifest["security"]["encryption"], "aes-256-gcm-chunks/1");
    let bytes = package
        .files()
        .values()
        .flat_map(|bytes| bytes.iter())
        .copied()
        .collect::<Vec<_>>();
    assert!(!String::from_utf8_lossy(&bytes).contains("cookie-value"));
    assert!(!String::from_utf8_lossy(&bytes).contains("profile-bytes"));
    let index_json = String::from_utf8(
        package
            .file("encrypted/profile-data/index.json")
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(index_json.contains("buglogin.encrypted-profile-data/2"));
    assert!(!index_json.contains("plaintextSha256"));
    assert!(!index_json.contains(&checksum(b"cookie-value")));
    let restored = decrypt_packaged_profile_data(&package, &ClientKey::from_bytes([7; 32]))
        .expect("decrypt package");
    assert_eq!(
        restored
            .decrypt_file(
                "profiles/00000000-0000-0000-0000-000000000001/profile/Cookies",
                &ClientKey::from_bytes([7; 32])
            )
            .unwrap(),
        b"cookie-value"
    );
}

#[test]
fn tamper_and_wrong_key_fail_authentication() {
    let source = source();
    let key = ClientKey::from_bytes([1; 32]);
    let mut data = encrypt_legacy_profile_data(source.path(), &key).expect("encrypt data");
    let file = data.index.files.first().expect("encrypted file");
    let source_path = file.source_path.clone();
    let object_key = file
        .chunks
        .first()
        .expect("encrypted chunk")
        .object_key
        .clone();
    data.file_mut(&object_key).unwrap()[0] ^= 1;
    assert!(data.decrypt_file(&source_path, &key).is_err());
    let clean = encrypt_legacy_profile_data(source.path(), &key).expect("encrypt clean data");
    assert!(
        clean
            .decrypt_file(&source_path, &ClientKey::from_bytes([2; 32]))
            .is_err()
    );
}

#[test]
fn packaging_does_not_mutate_legacy_source() {
    let source = source();
    let before = create_snapshot(source.path()).expect("snapshot before");
    let (report, snapshot) = inspect_source(source.path()).expect("inspect source");
    build_encrypted_profile_package(
        source.path(),
        &report,
        &snapshot,
        &PackageMetadata {
            created_at: "2026-07-20T00:00:00Z".to_owned(),
            tool_version: "0.1.0".to_owned(),
            tool_commit: "test".to_owned(),
        },
        &ClientKey::from_bytes([3; 32]),
    )
    .expect("build package");
    assert_eq!(
        before,
        create_snapshot(source.path()).expect("snapshot after")
    );
}
