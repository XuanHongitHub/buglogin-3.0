use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use buglogin_migration::inspect::inspect_source;
use buglogin_migration::package::{PackageMetadata, build_neutral_package};
use buglogin_migration::signature::{SignatureMetadata, sign_package, verify_signed_package};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "buglogin-signature-{}-{}",
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
fn signature_authenticates_checksum_manifest_and_rejects_wrong_key() {
    let source = Temp::new();
    source.write("version.json", br#"{"system":"buglogin-legacy","version":"1.1.84","instanceId":"signature-fixture","os":"windows","architecture":"x86_64"}"#);
    source.write(
        "profiles/one/metadata.json",
        br#"{"id":"one","name":"One","runtime":"wayfern"}"#,
    );
    let (report, snapshot) = inspect_source(source.path()).unwrap();
    let package = build_neutral_package(
        &report,
        &snapshot,
        &PackageMetadata {
            created_at: "2026-07-21T00:00:00Z".into(),
            tool_version: "0.1.0".into(),
            tool_commit: "0123456789abcdef0123456789abcdef01234567".into(),
        },
    )
    .unwrap();
    let signed = sign_package(
        &package,
        &[41; 32],
        SignatureMetadata {
            key_id: "migration-signing-v1".into(),
            signer: "security-reviewer".into(),
            signed_at: "2026-07-21T00:01:00Z".into(),
        },
    )
    .unwrap();

    assert_eq!(
        verify_signed_package(&signed, &[41; 32])
            .unwrap()
            .files
            .len(),
        4
    );
    assert_eq!(
        verify_signed_package(&signed, &[42; 32]).unwrap_err().code,
        "migration_signature_mismatch"
    );

    let mut files = signed.files().clone();
    let signature = files.get_mut("signature.json").unwrap();
    let last = signature
        .iter_mut()
        .rev()
        .find(|byte| **byte == b'}')
        .unwrap();
    *last = b']';
    assert_eq!(
        verify_signed_package(
            &buglogin_migration::package::NeutralPackage::from_files(files),
            &[41; 32]
        )
        .unwrap_err()
        .code,
        "migration_signature_invalid"
    );
}
