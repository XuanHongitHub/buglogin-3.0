use std::{fs, io::Write, path::PathBuf};

use buglogin_browser::runtime::{
    catalog::{RuntimeCatalog, RuntimePackageFormat, RuntimeRelease, RuntimeSignatureVerifier},
    download::{RuntimeArtifactSource, install, rollback},
};
use buglogin_domain::{
    entities::{Runtime, RuntimeArchitecture, RuntimePlatform},
    errors::Problem,
};
use buglogin_storage::blob::checksum;

struct ChecksumVerifier;
impl RuntimeSignatureVerifier for ChecksumVerifier {
    fn verify(&self, key_id: &str, payload: &[u8], signature: &str) -> bool {
        key_id == "release-key-1" && signature == checksum(payload)
    }
}

struct MemorySource(Vec<u8>);
impl RuntimeArtifactSource for MemorySource {
    fn fetch_to(
        &self,
        _source: &str,
        maximum_bytes: u64,
        output: &mut dyn Write,
    ) -> Result<(), Problem> {
        if self.0.len() as u64 > maximum_bytes {
            return Err(Problem::new(
                "runtime_download_too_large",
                "Runtime download too large",
                413,
                "test",
            ));
        }
        output.write_all(&self.0).map_err(|_| {
            Problem::new(
                "runtime_download_write_failed",
                "Runtime download write failed",
                500,
                "test",
            )
        })
    }
}

struct OversizedSource;
impl RuntimeArtifactSource for OversizedSource {
    fn fetch_to(
        &self,
        _source: &str,
        maximum_bytes: u64,
        output: &mut dyn Write,
    ) -> Result<(), Problem> {
        let bytes = vec![0_u8; maximum_bytes as usize + 1];
        output.write_all(&bytes).map_err(|_| {
            Problem::new(
                "runtime_download_too_large",
                "Runtime download too large",
                413,
                "test",
            )
        })
    }
}

fn root(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "buglogin-runtime-download-{}-{}",
        std::process::id(),
        label
    ))
}

fn signed_release(bytes: &[u8]) -> RuntimeRelease {
    let mut release = RuntimeRelease {
        runtime: Runtime::Wayfern,
        version: "1.0.0".into(),
        platform: RuntimePlatform::Windows,
        architecture: RuntimeArchitecture::X64,
        relative_path: "wayfern/wayfern.exe".into(),
        package_format: RuntimePackageFormat::Raw,
        archive_prefix: None,
        source: "https://releases.buglogin.example/wayfern.exe".into(),
        sha256: checksum(bytes),
        size_bytes: bytes.len() as u64,
        key_id: "release-key-1".into(),
        signature: String::new(),
    };
    release.signature = checksum(&release.signing_payload());
    release
}

#[cfg(not(target_os = "macos"))]
fn signed_dmg_release(bytes: &[u8]) -> RuntimeRelease {
    let mut release = RuntimeRelease {
        runtime: Runtime::Wayfern,
        version: "154.0.8037.57".into(),
        platform: RuntimePlatform::Macos,
        architecture: RuntimeArchitecture::Arm64,
        relative_path: "Wayfern.app/Contents/MacOS/Wayfern".into(),
        package_format: RuntimePackageFormat::Dmg,
        archive_prefix: None,
        source: "https://releases.buglogin.example/wayfern.dmg".into(),
        sha256: checksum(bytes),
        size_bytes: bytes.len() as u64,
        key_id: "release-key-1".into(),
        signature: String::new(),
    };
    release.signature = checksum(&release.signing_payload_v2());
    release
}

#[cfg(not(target_os = "macos"))]
#[test]
fn dmg_install_is_rejected_on_non_macos_before_downloading_or_writing() {
    let root = root("dmg-unsupported-platform");
    let _ = fs::remove_dir_all(&root);
    let release = RuntimeCatalog {
        schema_version: 2,
        releases: vec![signed_dmg_release(b"verified-dmg")],
    }
    .verify(&ChecksumVerifier)
    .unwrap()
    .remove(0);

    let error = install(&root, &release, &MemorySource(b"verified-dmg".to_vec())).unwrap_err();
    assert_eq!(error.code, "runtime_package_format_unsupported");
    assert!(!root.exists());
}

#[test]
fn verified_runtime_install_is_atomic_and_rollback_restores_previous_binary() {
    let root = root("rollback");
    let _ = fs::remove_dir_all(&root);
    let bytes = b"new-runtime";
    let release = RuntimeCatalog {
        schema_version: 1,
        releases: vec![signed_release(bytes)],
    }
    .verify(&ChecksumVerifier)
    .unwrap()
    .remove(0);
    let previous_binary =
        buglogin_browser::runtime::download::runtime_directory(&root, release.release())
            .unwrap()
            .join(&release.release().relative_path);
    fs::create_dir_all(previous_binary.parent().unwrap()).unwrap();
    fs::write(&previous_binary, b"old-runtime").unwrap();

    let receipt = install(&root, &release, &MemorySource(bytes.to_vec())).unwrap();
    assert!(receipt.rollback_available);
    assert_eq!(fs::read(&receipt.installed_path).unwrap(), bytes);
    rollback(&root, &release).unwrap();
    assert_eq!(fs::read(receipt.installed_path).unwrap(), b"old-runtime");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn installing_another_runtime_version_keeps_both_versions_available() {
    let root = root("multiple-versions");
    let _ = fs::remove_dir_all(&root);
    let old_bytes = b"old-runtime";
    let new_bytes = b"new-runtime";
    let old_release = signed_release(old_bytes);
    let mut new_release = signed_release(new_bytes);
    new_release.version = "2.0.0".into();
    new_release.signature = checksum(&new_release.signing_payload());
    let verified = RuntimeCatalog {
        schema_version: 1,
        releases: vec![old_release, new_release],
    }
    .verify(&ChecksumVerifier)
    .unwrap();

    let old = verified
        .iter()
        .find(|release| release.release().version == "1.0.0")
        .unwrap();
    let new = verified
        .iter()
        .find(|release| release.release().version == "2.0.0")
        .unwrap();
    let old_receipt = install(&root, old, &MemorySource(old_bytes.to_vec())).unwrap();
    let new_receipt = install(&root, new, &MemorySource(new_bytes.to_vec())).unwrap();

    assert_ne!(old_receipt.installed_path, new_receipt.installed_path);
    assert_eq!(fs::read(old_receipt.installed_path).unwrap(), old_bytes);
    assert_eq!(fs::read(new_receipt.installed_path).unwrap(), new_bytes);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn invalid_rollback_backup_does_not_remove_the_installed_runtime() {
    let root = root("invalid-rollback-backup");
    let _ = fs::remove_dir_all(&root);
    let bytes = b"new-runtime";
    let release = RuntimeCatalog {
        schema_version: 1,
        releases: vec![signed_release(bytes)],
    }
    .verify(&ChecksumVerifier)
    .unwrap()
    .remove(0);
    let previous_binary =
        buglogin_browser::runtime::download::runtime_directory(&root, release.release())
            .unwrap()
            .join(&release.release().relative_path);
    fs::create_dir_all(previous_binary.parent().unwrap()).unwrap();
    fs::write(&previous_binary, b"old-runtime").unwrap();

    let receipt = install(&root, &release, &MemorySource(bytes.to_vec())).unwrap();
    let backup = previous_binary.with_file_name("wayfern.exe.rollback");
    fs::remove_file(&backup).unwrap();
    fs::create_dir(&backup).unwrap();

    let rollback_error = rollback(&root, &release).unwrap_err();
    assert_eq!(rollback_error.code, "runtime_backup_invalid");
    assert_eq!(rollback_error.status, 409);
    assert_eq!(fs::read(receipt.installed_path).unwrap(), bytes);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn invalid_signature_or_checksum_never_replaces_existing_binary() {
    let root = root("integrity");
    let _ = fs::remove_dir_all(&root);
    let bytes = b"new-runtime";
    let mut unsigned = signed_release(bytes);
    unsigned.signature = "invalid".into();
    assert_eq!(
        RuntimeCatalog {
            schema_version: 1,
            releases: vec![unsigned],
        }
        .verify(&ChecksumVerifier)
        .unwrap_err()
        .code,
        "runtime_signature_invalid"
    );

    let release = RuntimeCatalog {
        schema_version: 1,
        releases: vec![signed_release(bytes)],
    }
    .verify(&ChecksumVerifier)
    .unwrap()
    .remove(0);
    let target = buglogin_browser::runtime::download::runtime_directory(&root, release.release())
        .unwrap()
        .join(&release.release().relative_path);
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    fs::write(&target, b"old-runtime").unwrap();
    let checksum_error =
        install(&root, &release, &MemorySource(b"tampered!!!".to_vec())).unwrap_err();
    assert_eq!(checksum_error.code, "runtime_checksum_mismatch");
    assert_eq!(checksum_error.status, 422);
    assert_eq!(fs::read(target).unwrap(), b"old-runtime");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn installer_enforces_signed_size_even_when_source_ignores_limit() {
    let root = root("oversized");
    let _ = fs::remove_dir_all(&root);
    let release = RuntimeCatalog {
        schema_version: 1,
        releases: vec![signed_release(b"new-runtime")],
    }
    .verify(&ChecksumVerifier)
    .unwrap()
    .remove(0);
    let target = buglogin_browser::runtime::download::runtime_directory(&root, release.release())
        .unwrap()
        .join(&release.release().relative_path);
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    fs::write(&target, b"old-runtime").unwrap();

    assert_eq!(
        install(&root, &release, &OversizedSource).unwrap_err().code,
        "runtime_download_too_large"
    );
    assert_eq!(fs::read(&target).unwrap(), b"old-runtime");
    assert_eq!(
        fs::read_dir(target.parent().unwrap())
            .unwrap()
            .filter_map(Result::ok)
            .count(),
        1
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn artifact_verification_is_exact_and_catalog_order_is_deterministic() {
    let first = signed_release(b"first-runtime");
    let mut second = signed_release(b"second-runtime");
    second.version = "2.0.0".into();
    second.relative_path = "wayfern/wayfern-2.exe".into();
    second.signature = checksum(&second.signing_payload());

    let verified_first = RuntimeCatalog {
        schema_version: 1,
        releases: vec![first.clone()],
    }
    .verify(&ChecksumVerifier)
    .unwrap()
    .remove(0);
    verified_first.verify_artifact(b"first-runtime").unwrap();
    assert_eq!(
        verified_first
            .verify_artifact(b"tampered!")
            .unwrap_err()
            .code,
        "runtime_size_mismatch"
    );

    let verified = RuntimeCatalog {
        schema_version: 1,
        releases: vec![second, first],
    }
    .verify(&ChecksumVerifier)
    .unwrap();
    assert_eq!(verified[0].release().version, "1.0.0");
    assert_eq!(verified[1].release().version, "2.0.0");
}
