use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use buglogin_domain::ids::{ResourceId, WorkspaceId};
use buglogin_migration::{
    MigrationKey,
    inspect::inspect_source,
    package::{NeutralPackageManifest, PackageMetadata, build_encrypted_profile_package},
    snapshot::create_snapshot,
    verify::verify_package,
};
use buglogin_sync::{
    export::{SyncExport, SyncExportBlob, SyncRevision, canonical_manifest_checksum},
    manifest::{ManifestObject, ManifestTombstone, SyncManifest},
};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct Temp(PathBuf);

impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "buglogin-sync-package-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn write(&self, relative: &str, bytes: &[u8]) {
        let path = self.0.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn sync_export() -> SyncExport {
    let manifest = SyncManifest::new_with_tombstones(
        WorkspaceId::new("ws-migration-sync").unwrap(),
        ResourceId::new("res-migration-sync").unwrap(),
        12,
        2,
        Some(1),
        b"migration-sync-root",
        vec![ManifestObject::new("profile/Preferences", b"profile-data").unwrap()],
        vec![ManifestTombstone::new("profile/deleted", 1, 1_000, 9_000).unwrap()],
    )
    .unwrap();
    let current_checksum = canonical_manifest_checksum(&manifest).unwrap();
    SyncExport::new(
        "migration-sync-export-1",
        "legacy-sync-production",
        manifest,
        None,
        vec![
            SyncRevision {
                generation: 1,
                parent_generation: None,
                manifest_sha256: "1".repeat(64),
            },
            SyncRevision {
                generation: 2,
                parent_generation: Some(1),
                manifest_sha256: current_checksum,
            },
        ],
        vec![SyncExportBlob {
            key: "profile/Preferences".into(),
            size: 12,
            checksum: buglogin_storage::blob::checksum(b"profile-data"),
            data: b"profile-data".to_vec(),
        }],
        1_753_142_400_000,
        "0.1.0",
    )
    .unwrap()
}

fn source(export: &SyncExport) -> Temp {
    let source = Temp::new();
    source.write(
        "version.json",
        br#"{"system":"buglogin-legacy","version":"1.1.84","instanceId":"sync-package","os":"windows","architecture":"x86_64"}"#,
    );
    source.write("sync-export.json", &serde_json::to_vec(export).unwrap());
    source
}

#[test]
fn encrypted_package_preserves_verified_sync_export_and_source() {
    let expected = sync_export();
    let source = source(&expected);
    let before = create_snapshot(source.path()).unwrap();
    let (report, snapshot) = inspect_source(source.path()).unwrap();
    let package = build_encrypted_profile_package(
        source.path(),
        &report,
        &snapshot,
        &PackageMetadata {
            created_at: "2026-07-22T00:00:00Z".into(),
            tool_version: "0.1.0".into(),
            tool_commit: "0000000000000000000000000000000000000000".into(),
        },
        &MigrationKey::from_bytes([4; 32]),
    )
    .unwrap();
    verify_package(&package).unwrap();
    let packaged: SyncExport =
        serde_json::from_slice(package.file("sync/export.json").unwrap()).unwrap();
    assert_eq!(packaged, expected);
    packaged.validate().unwrap();
    let manifest: NeutralPackageManifest =
        serde_json::from_slice(package.file("manifest.json").unwrap()).unwrap();
    assert!(
        manifest
            .contents
            .iter()
            .any(|content| content.path == "sync/export.json")
    );
    assert_eq!(before, create_snapshot(source.path()).unwrap());
}

#[test]
fn package_rejects_tampered_sync_export_without_source_mutation() {
    let mut export = sync_export();
    export.blobs[0].data[0] ^= 1;
    let source = source(&export);
    let before = create_snapshot(source.path()).unwrap();
    let (report, snapshot) = inspect_source(source.path()).unwrap();
    let error = build_encrypted_profile_package(
        source.path(),
        &report,
        &snapshot,
        &PackageMetadata {
            created_at: "2026-07-22T00:00:00Z".into(),
            tool_version: "0.1.0".into(),
            tool_commit: "0000000000000000000000000000000000000000".into(),
        },
        &MigrationKey::from_bytes([4; 32]),
    )
    .unwrap_err();
    assert_eq!(error.code, "migration_sync_export_invalid");
    assert_eq!(before, create_snapshot(source.path()).unwrap());
}
