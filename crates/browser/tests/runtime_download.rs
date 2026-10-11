use std::{fs, path::PathBuf};

use buglogin_browser::runtime::{
    catalog::{RuntimeCatalog, RuntimeRelease, RuntimeSignatureVerifier},
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
    fn fetch(&self, _source: &str, maximum_bytes: u64) -> Result<Vec<u8>, Problem> {
        if self.0.len() as u64 > maximum_bytes {
            return Err(Problem::new(
                "runtime_download_too_large",
                "Runtime download too large",
                413,
                "test",
            ));
        }
        Ok(self.0.clone())
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
        source: "https://releases.buglogin.example/wayfern.exe".into(),
        sha256: checksum(bytes),
        size_bytes: bytes.len() as u64,
        key_id: "release-key-1".into(),
        signature: String::new(),
    };
    release.signature = checksum(&release.signing_payload());
    release
}

#[test]
fn verified_runtime_install_is_atomic_and_rollback_restores_previous_binary() {
    let root = root("rollback");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("wayfern")).unwrap();
    fs::write(root.join("wayfern/wayfern.exe"), b"old-runtime").unwrap();
    let bytes = b"new-runtime";
    let release = RuntimeCatalog {
        schema_version: 1,
        releases: vec![signed_release(bytes)],
    }
    .verify(&ChecksumVerifier)
    .unwrap()
    .remove(0);

    let receipt = install(&root, &release, &MemorySource(bytes.to_vec())).unwrap();
    assert!(receipt.rollback_available);
    assert_eq!(fs::read(&receipt.installed_path).unwrap(), bytes);
    rollback(&root, &release).unwrap();
    assert_eq!(fs::read(receipt.installed_path).unwrap(), b"old-runtime");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn invalid_signature_or_checksum_never_replaces_existing_binary() {
    let root = root("integrity");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("wayfern")).unwrap();
    let target = root.join("wayfern/wayfern.exe");
    fs::write(&target, b"old-runtime").unwrap();
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
    assert_eq!(
        install(&root, &release, &MemorySource(b"tampered!!!".to_vec()))
            .unwrap_err()
            .code,
        "runtime_checksum_mismatch"
    );
    assert_eq!(fs::read(target).unwrap(), b"old-runtime");
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
