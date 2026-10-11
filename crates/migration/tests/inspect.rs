use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use buglogin_migration::inspect::{CutoverEligibility, MigrationClass, inspect_source};
use buglogin_migration::package::{PackageMetadata, build_neutral_package};
use buglogin_migration::snapshot::{SourceSnapshot, create_snapshot};

static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);
const SECRET_CANARY: &str = "MIGRATION-SECRET-MUST-NOT-LEAK";
const UNKNOWN_RUNTIME_CANARY: &str = "UNKNOWN-RUNTIME-MUST-NOT-LEAK";

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "buglogin-migration-test-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create synthetic source root");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn write(&self, relative: &str, contents: &str) {
        let path = self.0.join(relative);
        fs::create_dir_all(path.parent().expect("fixture parent")).expect("create fixture parent");
        fs::write(path, contents).expect("write synthetic fixture");
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn synthetic_source() -> TestDirectory {
    let source = TestDirectory::new();
    source.write(
        "version.json",
        r#"{"system":"buglogin-legacy","version":"1.1.84","instanceId":"synthetic-instance","os":"windows","architecture":"x86_64"}"#,
    );
    source.write(
        "profiles/direct/metadata.json",
        &format!(
            r#"{{"id":"direct","name":"Direct","runtime":"wayfern","proxyPassword":"{SECRET_CANARY}"}}"#
        ),
    );
    source.write(
        "profiles/unsupported/metadata.json",
        r#"{"id":"unsupported","name":"Unsupported","runtime":"camoufox"}"#,
    );
    source.write(
        "profiles/unknown/metadata.json",
        &format!(r#"{{"id":"unknown","name":"Unknown","runtime":"{UNKNOWN_RUNTIME_CANARY}"}}"#),
    );
    source.write(
        "profiles/direct/profile/Preferences",
        "synthetic browser data",
    );
    source.write(
        "profiles/direct/profile/component_crx_cache/metadata.json",
        r#"{"browser":"wayfern"}"#,
    );
    source
}

#[test]
fn inspect_is_read_only_deterministic_and_redacted() {
    let source = synthetic_source();
    let before = create_snapshot(source.path()).expect("snapshot before inspect");

    let (first_report, first_snapshot) = inspect_source(source.path()).expect("inspect source");
    let (second_report, second_snapshot) = inspect_source(source.path()).expect("inspect again");
    let after = create_snapshot(source.path()).expect("snapshot after inspect");

    assert_eq!(before, after);
    assert_eq!(first_snapshot, second_snapshot);
    assert_eq!(first_report, second_report);
    assert_eq!(first_report.source.root, ".");
    assert_eq!(first_report.entity_counts["profiles"], 3);
    assert_eq!(first_report.secret_scan.secret_fields_present, 1);
    assert_eq!(first_report.secret_scan.files_with_secret_fields, 1);
    assert_eq!(
        first_report.cutover_eligibility,
        CutoverEligibility::Blocked
    );
    assert!(first_report.runtime_findings.iter().any(|finding| {
        finding.runtime == "camoufox" && finding.class == MigrationClass::Unsupported
    }));

    let report = serde_json::to_string(&first_report).expect("serialize report");
    assert!(!report.contains(SECRET_CANARY));
    assert!(!report.contains(UNKNOWN_RUNTIME_CANARY));
    assert!(first_report.runtime_findings.iter().any(|finding| {
        finding.runtime.starts_with("unknown:sha256:") && finding.class == MigrationClass::Unknown
    }));
    assert!(!report.contains(source.path().to_string_lossy().as_ref()));
}

#[test]
fn snapshot_redacts_sensitive_fingerprints_but_keeps_runtime_integrity() {
    let source = TestDirectory::new();
    source.write(
        "version.json",
        r#"{"system":"buglogin-legacy","version":"1.1.84","instanceId":"snapshot-redaction","os":"windows","architecture":"x86_64"}"#,
    );
    source.write("profiles/direct/profile/Cookies", SECRET_CANARY);
    source.write(
        "profiles/direct/metadata.json",
        &format!(r#"{{"id":"direct","proxyPassword":"{SECRET_CANARY}"}}"#),
    );
    source.write("proxy/credentials.json", SECRET_CANARY);
    source.write("vpn/configs.json", SECRET_CANARY);

    let first = create_snapshot(source.path()).expect("create redacted snapshot");
    let serialized = serde_json::to_string(&first).expect("serialize redacted snapshot");
    let secret_sha256 = buglogin_storage::blob::checksum(SECRET_CANARY.as_bytes());

    assert!(!serialized.contains(&secret_sha256));
    assert_eq!(
        first
            .files
            .iter()
            .filter(|entry| entry.sha256 == "redacted")
            .count(),
        4
    );

    source.write(
        "profiles/direct/profile/Cookies",
        &"X".repeat(SECRET_CANARY.len()),
    );
    let second = create_snapshot(source.path()).expect("create changed snapshot");
    assert_eq!(first.source_tree_sha256, second.source_tree_sha256);
    assert!(!first.has_same_integrity(&second));
}

#[test]
fn inspect_integrity_gate_rejects_same_length_sensitive_source_changes() {
    let source = TestDirectory::new();
    source.write(
        "version.json",
        r#"{"system":"buglogin-legacy","version":"1.1.84","instanceId":"integrity-gate","os":"windows","architecture":"x86_64"}"#,
    );
    source.write(
        "profiles/direct/metadata.json",
        r#"{"id":"direct","runtime":"wayfern"}"#,
    );
    source.write("profiles/direct/profile/Cookies", "AAAA");

    let snapshot = create_snapshot(source.path()).expect("create snapshot");
    source.write("profiles/direct/profile/Cookies", "BBBB");
    let current = create_snapshot(source.path()).expect("create current snapshot");

    assert_eq!(snapshot.source_tree_sha256, current.source_tree_sha256);
    assert!(!snapshot.has_same_integrity(&current));
    let error =
        buglogin_migration::inspect::ensure_snapshot_integrity_for_test(&snapshot, &current)
            .expect_err("inspection must reject source drift");
    assert_eq!(error.code, "migration_source_changed");
}

#[test]
fn neutral_package_is_deterministic_and_contains_no_secret_values() {
    let source = synthetic_source();
    let (report, snapshot) = inspect_source(source.path()).expect("inspect source");
    let metadata = PackageMetadata {
        created_at: "2026-07-20T00:00:00Z".to_owned(),
        tool_version: "0.1.0".to_owned(),
        tool_commit: "0000000000000000000000000000000000000000".to_owned(),
    };

    let first = build_neutral_package(&report, &snapshot, &metadata).expect("build package");
    let second = build_neutral_package(&report, &snapshot, &metadata).expect("build again");

    assert_eq!(first, second);
    let manifest: serde_json::Value =
        serde_json::from_slice(first.file("manifest.json").expect("manifest file"))
            .expect("valid manifest");
    assert_eq!(manifest["schema"], "buglogin.export/1");
    assert!(manifest.get("package_id").is_some());
    assert!(manifest.get("packageId").is_none());
    assert_eq!(manifest["source"]["os"], "windows");
    assert_eq!(manifest["compatibility"]["minimum_importer"], "0.1.0");
    assert_eq!(manifest["security"]["contains_secrets"], false);
    assert_eq!(
        first.files().keys().cloned().collect::<Vec<_>>(),
        vec![
            "checksums.sha256",
            "manifest.json",
            "reports/source-inventory.json",
            "reports/unsupported.json",
            "snapshot.json",
        ]
    );
    let combined = first
        .files()
        .values()
        .flat_map(|bytes| bytes.iter().copied())
        .collect::<Vec<_>>();
    assert!(!String::from_utf8_lossy(&combined).contains(SECRET_CANARY));
    assert!(!String::from_utf8_lossy(&combined).contains(UNKNOWN_RUNTIME_CANARY));
    assert!(
        String::from_utf8_lossy(first.file("reports/unsupported.json").unwrap())
            .contains("runtime_unsupported")
    );
}

#[test]
fn legacy_browser_field_reports_bugmini_adapter_pending_artifact_evidence() {
    let source = TestDirectory::new();
    source.write(
        "version.json",
        r#"{"system":"buglogin-legacy","version":"1.1.84","instanceId":"browser-runtime","os":"windows","architecture":"x86_64"}"#,
    );
    source.write(
        "profiles/one/metadata.json",
        r#"{"id":"one","name":"One","browser":"bugmini"}"#,
    );

    let (report, _) = inspect_source(source.path()).expect("inspect browser runtime");

    assert_eq!(report.entity_counts["profiles"], 1);
    assert!(report.runtime_findings.iter().any(|finding| {
        finding.runtime == "bugmini"
            && finding.class == MigrationClass::Partial
            && finding.reason_code == "runtime_adapter_available_artifact_pending"
            && finding.records == 1
    }));
    assert_eq!(report.cutover_eligibility, CutoverEligibility::Blocked);
}

#[test]
fn profile_directory_without_metadata_blocks_inspection() {
    let source = TestDirectory::new();
    source.write(
        "version.json",
        r#"{"system":"buglogin-legacy","version":"1.1.84","instanceId":"missing-metadata","os":"windows","architecture":"x86_64"}"#,
    );
    fs::create_dir_all(source.path().join("profiles/missing/profile")).unwrap();
    source.write(
        "profiles/complete/metadata.json",
        r#"{"id":"complete","name":"Complete","browser":"wayfern"}"#,
    );

    let (report, _) = inspect_source(source.path()).expect("inspect missing profile metadata");

    assert!(report.invalid_records.iter().any(|issue| {
        issue.source_ref == "profiles/missing/metadata.json"
            && issue.code == "profile_metadata_missing"
    }));
    assert_eq!(report.cutover_eligibility, CutoverEligibility::Blocked);
}

#[test]
fn unknown_source_version_is_inspectable_but_ineligible() {
    let source = TestDirectory::new();
    source.write(
        "version.json",
        r#"{"system":"buglogin-legacy","version":"1.1.83","instanceId":"synthetic-old","os":"windows","architecture":"x86_64"}"#,
    );

    let (report, _) = inspect_source(source.path()).expect("inspect unknown version");
    assert_eq!(report.cutover_eligibility, CutoverEligibility::Blocked);
}

#[test]
fn vpn_inventory_counts_container_records_and_ignores_runtime_residue() {
    let source = TestDirectory::new();
    source.write(
        "version.json",
        r#"{"system":"buglogin-legacy","version":"1.1.84","instanceId":"vpn-inventory","os":"windows","architecture":"x86_64"}"#,
    );
    source.write(
        "vpn/vpn_configs.json",
        r#"{"version":1,"configs":[{"id":"vpn-1","name":"WireGuard","vpn_type":"WireGuard","encrypted_data":"ciphertext-1","nonce":"nonce-1","created_at":1,"last_used":null},{"id":"vpn-2","name":"OpenVPN","vpn_type":"OpenVPN","encrypted_data":"ciphertext-2","nonce":"nonce-2","created_at":2,"last_used":3,"sync_enabled":true,"last_sync":4}]}"#,
    );
    source.write("vpn_worker_stale.json", "not-json");
    source.write("vpn/vpn_worker_stale.json", "also-not-json");

    let (report, _) = inspect_source(source.path()).expect("inspect VPN inventory");

    assert_eq!(report.entity_counts["vpns"], 2);
    assert!(report.invalid_records.is_empty());
    assert_eq!(report.cutover_eligibility, CutoverEligibility::Eligible);
}

#[test]
fn vpn_inventory_reports_invalid_version_and_records() {
    let source = TestDirectory::new();
    source.write(
        "version.json",
        r#"{"system":"buglogin-legacy","version":"1.1.84","instanceId":"vpn-invalid","os":"windows","architecture":"x86_64"}"#,
    );
    source.write(
        "vpn/vpn_configs.json",
        r#"{"version":2,"configs":[{"id":"vpn-1","name":"Broken","vpn_type":"Unknown","encrypted_data":"","nonce":"nonce","created_at":1}]}"#,
    );

    let (report, _) = inspect_source(source.path()).expect("inspect invalid VPN inventory");

    assert_eq!(report.entity_counts["vpns"], 1);
    assert!(report.invalid_records.iter().any(|issue| {
        issue.source_ref == "vpn/vpn_configs.json" && issue.code == "vpn_version_unsupported"
    }));
    assert!(report.invalid_records.iter().any(|issue| {
        issue.source_ref == "vpn/vpn_configs.json#configs/0" && issue.code == "vpn_record_invalid"
    }));
    assert_eq!(report.cutover_eligibility, CutoverEligibility::Blocked);
}
