use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use buglogin_domain::{
    entities::{Profile, ProfileState, Proxy, ProxyProtocol, Runtime, SyncMode, Vpn, VpnKind},
    ids::ProfileId,
};
use buglogin_migration::{
    MigrationKey, activate_package, activate_package_with_vault,
    inspect::inspect_source,
    legacy::{
        analyze_legacy_proxy_batch, map_legacy_control_plane_json, map_legacy_extensions,
        map_legacy_profiles_for_encrypted_package,
    },
    package::{
        NeutralPackage, NeutralPackageManifest, PackageMetadata, build_encrypted_profile_package,
        build_neutral_package,
    },
    plan_package_activation, plan_package_activation_with_vault,
    reconcile::{ReconciliationStatus, reconcile_activation},
    rollback::{confirm_rollback, rollback_activation, rollback_activation_with_vault},
    snapshot::create_snapshot,
    transform::{discard_staging, stage_package, stage_package_with_available_bytes},
    verify::{validate_path_length, validate_relative_path, verify_package},
};
use buglogin_storage::{
    local::LocalRepository,
    secrets::{OsVault, RotationReceipt, SecretError, SecretKey, SecretValue, SecretVersion},
};
use fs2::FileExt;
use serde_json::{Value, json};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Default)]
struct FakeVault {
    values: BTreeMap<(String, String), (SecretValue, SecretVersion)>,
}

impl OsVault for FakeVault {
    fn read(&self, key: &SecretKey) -> Result<(SecretValue, SecretVersion), SecretError> {
        self.values
            .get(&(key.namespace().into(), key.name().into()))
            .cloned()
            .ok_or(SecretError::NotFound)
    }

    fn create(
        &mut self,
        key: &SecretKey,
        value: SecretValue,
    ) -> Result<SecretVersion, SecretError> {
        let version = SecretVersion::new(1).expect("valid initial secret version");
        if self
            .values
            .insert(
                (key.namespace().into(), key.name().into()),
                (value, version),
            )
            .is_some()
        {
            return Err(SecretError::VersionConflict);
        }
        Ok(version)
    }

    fn rotate(
        &mut self,
        _key: &SecretKey,
        _expected: SecretVersion,
        _replacement: SecretValue,
    ) -> Result<RotationReceipt, SecretError> {
        Err(SecretError::OperationFailed)
    }

    fn remove(&mut self, key: &SecretKey, expected: SecretVersion) -> Result<(), SecretError> {
        let map_key = (key.namespace().into(), key.name().into());
        let (_, version) = self.values.get(&map_key).ok_or(SecretError::NotFound)?;
        if *version != expected {
            return Err(SecretError::VersionConflict);
        }
        self.values.remove(&map_key);
        Ok(())
    }
}

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "buglogin-migration-fixture-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("create fixture test directory");
        Self(path)
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn mig_001_empty_source_matches_golden_and_remains_unchanged() {
    let fixture = fixture("MIG-001-empty-1.1.84");
    let source = fixture.join("source");
    let expected = fixture.join("expected");
    let before = create_snapshot(&source).expect("snapshot fixture before test");

    let (report, snapshot) = inspect_source(&source).expect("inspect fixture");
    assert_json_file(&expected.join("inspect.json"), &report);
    assert_json_file(&expected.join("snapshot.json"), &snapshot);

    let package = build_neutral_package(
        &report,
        &snapshot,
        &PackageMetadata {
            created_at: "2026-07-22T00:00:00Z".to_owned(),
            tool_version: "3.0.0".to_owned(),
            tool_commit: "0000000000000000000000000000000000000000".to_owned(),
        },
    )
    .expect("export fixture");
    let verified = verify_package(&package).expect("verify fixture package");
    let manifest: NeutralPackageManifest =
        serde_json::from_slice(package.file("manifest.json").expect("fixture manifest"))
            .expect("parse fixture manifest");
    assert_json_file(
        &expected.join("export-summary.json"),
        &json!({
            "packageId": manifest.package_id,
            "files": package.files().len(),
            "sourceTreeSha256": snapshot.source_tree_sha256,
            "checksumsSha256": verified.checksums_sha256,
            "containsSecrets": manifest.security.contains_secrets,
        }),
    );

    let key = MigrationKey::from_bytes([7; 32]);
    let encrypted_package = build_encrypted_profile_package(
        &source,
        &report,
        &snapshot,
        &PackageMetadata {
            created_at: "2026-07-22T00:00:00Z".to_owned(),
            tool_version: "3.0.0".to_owned(),
            tool_commit: "0000000000000000000000000000000000000000".to_owned(),
        },
        &key,
    )
    .expect("build encrypted fixture package");
    verify_package(&encrypted_package).expect("verify encrypted fixture package");
    let destination = TestDirectory::new();
    let destination_root = destination.0.join("destination");
    let plan = plan_package_activation(&destination_root, &encrypted_package, &key)
        .expect("plan empty fixture import");
    assert!(!destination_root.exists());
    assert_json_lines_file(
        &expected.join("import-results.jsonl"),
        &[json!({
            "status": "completed",
            "dryRun": true,
            "destinationChanged": false,
            "plan": plan,
            "sourceUnchanged": true,
        })],
    );

    let after = create_snapshot(&source).expect("snapshot fixture after test");
    assert_eq!(before, after);
}

#[test]
fn mig_002_wayfern_vertical_plans_activates_and_repeats_idempotently() {
    let fixture = fixture("MIG-002-wayfern-complete");
    let source = fixture.join("source");
    let expected = fixture.join("expected");
    let before = create_snapshot(&source).expect("snapshot vertical fixture before test");
    let (report, snapshot) = inspect_source(&source).expect("inspect vertical fixture");
    assert_json_file(&expected.join("inspect.json"), &report);
    assert_json_file(&expected.join("snapshot.json"), &snapshot);

    let redacted = build_neutral_package(&report, &snapshot, &fixture_metadata())
        .expect("build vertical redacted export");
    let verified = verify_package(&redacted).expect("verify vertical redacted export");
    let manifest: NeutralPackageManifest =
        serde_json::from_slice(redacted.file("manifest.json").expect("vertical manifest"))
            .expect("parse vertical manifest");
    assert_json_file(
        &expected.join("export-summary.json"),
        &json!({
            "packageId": manifest.package_id,
            "files": redacted.files().len(),
            "sourceTreeSha256": snapshot.source_tree_sha256,
            "checksumsSha256": verified.checksums_sha256,
            "containsSecrets": manifest.security.contains_secrets,
        }),
    );

    let key = MigrationKey::from_bytes([2; 32]);
    let package =
        build_encrypted_profile_package(&source, &report, &snapshot, &fixture_metadata(), &key)
            .expect("build vertical encrypted package");
    verify_package(&package).expect("verify vertical encrypted package");
    let destination = TestDirectory::new();
    let destination_root = destination.0.join("destination");
    let plan =
        plan_package_activation(&destination_root, &package, &key).expect("plan vertical import");
    let first = activate_package(&destination_root, "mig-002-first", &package, Some(&key))
        .expect("activate vertical import");
    let second = activate_package(&destination_root, "mig-002-repeat", &package, Some(&key))
        .expect("repeat vertical import");
    assert_json_lines_file(
        &expected.join("import-results.jsonl"),
        &[
            json!({
                "status": "planned",
                "create": {
                    "profiles": plan.create_profile_ids.len(),
                    "groups": plan.create_group_ids.len(),
                    "proxies": plan.create_proxy_ids.len(),
                    "extensions": plan.create_extension_ids.len(),
                    "extensionGroups": plan.create_extension_group_ids.len(),
                    "extensionArtifacts": plan.create_extension_artifact_ids.len(),
                },
            }),
            json!({
                "status": "activated",
                "created": {
                    "profiles": first.created_profiles,
                    "groups": first.created_groups,
                    "proxies": first.created_proxies,
                    "extensions": first.created_extensions,
                    "extensionGroups": first.created_extension_groups,
                    "extensionArtifacts": first.created_extension_artifacts,
                },
            }),
            json!({
                "status": "repeated",
                "reused": {
                    "profiles": second.reused_profiles,
                    "groups": second.reused_groups,
                    "proxies": second.reused_proxies,
                    "extensions": second.reused_extensions,
                    "extensionGroups": second.reused_extension_groups,
                    "extensionArtifacts": second.reused_extension_artifacts,
                },
                "sourceUnchanged": true,
            }),
        ],
    );
    assert_eq!(
        before,
        create_snapshot(&source).expect("snapshot vertical fixture after test")
    );
}

#[test]
fn mig_003_cookie_bytes_survive_import_repeat_and_rollback() {
    let fixture = fixture("MIG-003-cookie-persistence");
    let source = fixture.join("source");
    let expected = fixture.join("expected");
    let cookie_source =
        source.join("profiles/11111111-1111-1111-1111-111111111111/profile/Cookies");
    let expected_cookie = fs::read(&cookie_source).expect("read cookie fixture");
    let before = create_snapshot(&source).expect("snapshot cookie fixture before test");
    let (report, snapshot) = inspect_source(&source).expect("inspect cookie fixture");
    assert_json_file(&expected.join("inspect.json"), &report);
    assert_json_file(&expected.join("snapshot.json"), &snapshot);

    let redacted = build_neutral_package(&report, &snapshot, &fixture_metadata())
        .expect("build cookie redacted export");
    let verified = verify_package(&redacted).expect("verify cookie redacted export");
    let manifest: NeutralPackageManifest = serde_json::from_slice(
        redacted
            .file("manifest.json")
            .expect("cookie redacted manifest"),
    )
    .expect("parse cookie redacted manifest");
    assert_json_file(
        &expected.join("export-summary.json"),
        &json!({
            "packageId": manifest.package_id,
            "files": redacted.files().len(),
            "sourceTreeSha256": snapshot.source_tree_sha256,
            "checksumsSha256": verified.checksums_sha256,
            "containsSecrets": manifest.security.contains_secrets,
        }),
    );

    let key = MigrationKey::from_bytes([3; 32]);
    let package =
        build_encrypted_profile_package(&source, &report, &snapshot, &fixture_metadata(), &key)
            .expect("build cookie encrypted package");
    verify_package(&package).expect("verify cookie package");
    let encrypted_index = String::from_utf8(
        package
            .file("encrypted/profile-data/index.json")
            .expect("cookie encrypted index")
            .to_vec(),
    )
    .expect("cookie index utf8");
    assert!(!encrypted_index.contains("plaintextSha256"));
    assert!(!encrypted_index.contains(&buglogin_storage::blob::checksum(&expected_cookie)));

    let destination = TestDirectory::new();
    let destination_root = destination.0.join("destination");
    let plan =
        plan_package_activation(&destination_root, &package, &key).expect("plan cookie import");
    assert_eq!(plan.create_profile_ids.len(), 1);
    let run_id = "mig-003-cookie";
    let first = activate_package(&destination_root, run_id, &package, Some(&key))
        .expect("activate cookie import");
    let repeated = activate_package(&destination_root, run_id, &package, Some(&key))
        .expect("repeat cookie import");
    assert_eq!(first.created_profiles, 1);
    assert_eq!(repeated.reused_profiles, 1);

    let profile_id = plan.create_profile_ids[0].clone();
    let restored_cookie = destination_root
        .join("profiles")
        .join(&profile_id)
        .join("profile/Cookies");
    assert_eq!(fs::read(&restored_cookie).unwrap(), expected_cookie);
    assert!(
        !destination_root
            .join("profiles")
            .join(&profile_id)
            .join(format!(".buglogin-migration-{run_id}"))
            .exists()
    );

    let rollback = rollback_activation(
        &destination_root,
        run_id,
        confirm_rollback("ROLLBACK_MIGRATION_RUN").unwrap(),
    )
    .expect("rollback cookie import");
    assert_eq!(rollback.quarantined_profiles.len(), 1);
    assert!(!restored_cookie.exists());
    assert_eq!(
        fs::read(rollback.quarantined_profiles[0].join("profile/Cookies")).unwrap(),
        expected_cookie
    );
    assert_json_lines_file(
        &expected.join("import-results.jsonl"),
        &[
            json!({
                "status": "planned",
                "createProfiles": plan.create_profile_ids.len(),
                "destinationChanged": false,
            }),
            json!({
                "status": "activated",
                "createdProfiles": first.created_profiles,
                "cookieBytesPreserved": true,
                "markerRemoved": true,
            }),
            json!({
                "status": "repeated",
                "reusedProfiles": repeated.reused_profiles,
            }),
            json!({
                "status": "rolledBack",
                "quarantinedProfiles": rollback.quarantined_profiles.len(),
                "cookieBytesPreserved": true,
            }),
        ],
    );
    assert_eq!(
        before,
        create_snapshot(&source).expect("snapshot cookie fixture after test")
    );
}

#[test]
fn mig_005_mixed_proxy_rows_report_each_failure_and_block_atomic_commit() {
    let fixture = fixture("MIG-005-proxy-invalid-mixed");
    let source = fixture.join("source");
    let expected = fixture.join("expected");
    let before = create_snapshot(&source).expect("snapshot mixed proxy fixture before test");

    let analysis = analyze_legacy_proxy_batch(&source, "mig-005-proxy-invalid-mixed")
        .expect("analyze mixed proxy fixture");
    assert_eq!(analysis.policy, "atomic");
    assert_eq!(analysis.valid_records, 2);
    assert_eq!(analysis.invalid_records, 3);
    assert_json_file(&expected.join("proxy-analysis.json"), &analysis);
    let analysis_bytes = serde_json::to_vec(&analysis).unwrap();
    assert!(!String::from_utf8_lossy(&analysis_bytes).contains("proxy-valid.example.test"));

    let (report, snapshot) = inspect_source(&source).expect("inspect mixed proxy fixture");
    assert_json_file(&expected.join("inspect.json"), &report);
    assert_json_file(&expected.join("snapshot.json"), &snapshot);
    assert_eq!(report.invalid_records.len(), 3);
    assert_eq!(
        report.cutover_eligibility,
        buglogin_migration::inspect::CutoverEligibility::Blocked
    );

    let destination = TestDirectory::new();
    let destination_root = destination.0.join("destination");
    let error = build_encrypted_profile_package(
        &source,
        &report,
        &snapshot,
        &fixture_metadata(),
        &MigrationKey::from_bytes([5; 32]),
    )
    .expect_err("atomic mixed proxy export must be blocked");
    assert_eq!(error.code, "migration_cutover_blocked");
    assert!(!destination_root.exists());
    assert_json_file(
        &expected.join("export-summary.json"),
        &json!({
            "status": "blocked",
            "code": error.code,
            "policy": analysis.policy,
            "validRows": analysis.valid_records,
            "invalidRows": analysis.invalid_records,
        }),
    );
    assert_json_lines_file(
        &expected.join("import-results.jsonl"),
        &[json!({
            "status": "blocked",
            "code": "migration_cutover_blocked",
            "policy": "atomic",
            "createdProxies": 0,
            "destinationChanged": false,
            "sourceUnchanged": true,
        })],
    );
    assert_eq!(
        before,
        create_snapshot(&source).expect("snapshot mixed proxy fixture after test")
    );
}

#[test]
fn mig_004_proxy_protocols_activate_without_secret_leakage() {
    let fixture = fixture("MIG-004-proxy-protocols");
    let source = fixture.join("source");
    let expected = fixture.join("expected");
    let before = create_snapshot(&source).expect("snapshot proxy fixture before test");
    let (report, snapshot) = inspect_source(&source).expect("inspect proxy fixture");
    assert_json_file(&expected.join("inspect.json"), &report);
    assert_json_file(&expected.join("snapshot.json"), &snapshot);
    assert_eq!(report.entity_counts["proxies"], 4);
    assert_eq!(report.secret_scan.files_with_secret_fields, 2);
    assert_eq!(report.secret_scan.secret_fields_present, 4);
    assert_eq!(
        snapshot
            .files
            .iter()
            .filter(|entry| entry.path.starts_with("proxies/"))
            .filter(|entry| entry.sha256 == "redacted")
            .count(),
        2
    );

    let redacted = build_neutral_package(&report, &snapshot, &fixture_metadata())
        .expect("build proxy redacted export");
    let verified = verify_package(&redacted).expect("verify proxy redacted export");
    let manifest: NeutralPackageManifest = serde_json::from_slice(
        redacted
            .file("manifest.json")
            .expect("proxy redacted manifest"),
    )
    .expect("parse proxy redacted manifest");
    assert_json_file(
        &expected.join("export-summary.json"),
        &json!({
            "packageId": manifest.package_id,
            "files": redacted.files().len(),
            "sourceTreeSha256": snapshot.source_tree_sha256,
            "checksumsSha256": verified.checksums_sha256,
            "containsSecrets": manifest.security.contains_secrets,
        }),
    );

    let key = MigrationKey::from_bytes([4; 32]);
    let package =
        build_encrypted_profile_package(&source, &report, &snapshot, &fixture_metadata(), &key)
            .expect("build proxy encrypted package");
    verify_package(&package).expect("verify proxy encrypted package");
    let mut policy_files = package.files().clone();
    let mut policy_manifest: NeutralPackageManifest = serde_json::from_slice(
        policy_files
            .get("manifest.json")
            .expect("proxy encrypted manifest"),
    )
    .expect("parse proxy encrypted manifest");
    policy_manifest.security.contains_secrets = false;
    let mut manifest_bytes =
        serde_json::to_vec(&policy_manifest).expect("serialize tampered policy");
    manifest_bytes.push(b'\n');
    policy_files.insert("manifest.json".into(), manifest_bytes);
    policy_files.remove("checksums.sha256");
    let checksums = policy_files
        .iter()
        .map(|(path, bytes)| format!("{}  {path}\n", buglogin_storage::blob::checksum(bytes)))
        .collect::<String>();
    policy_files.insert("checksums.sha256".into(), checksums.into_bytes());
    assert_eq!(
        verify_package(&buglogin_migration::package::NeutralPackage::from_files(
            policy_files
        ))
        .expect_err("tampered secret marker must fail")
        .code,
        "migration_package_secret_policy_failed"
    );
    let package_bytes = package
        .files()
        .values()
        .flat_map(|bytes| bytes.iter().copied())
        .collect::<Vec<_>>();
    let package_text = String::from_utf8_lossy(&package_bytes);
    for forbidden in [
        "fixture-http-user",
        "fixture-socks-user",
        "MIG-004-HTTP-PASSWORD",
        "MIG-004-SOCKS-PASSWORD",
    ] {
        assert!(!package_text.contains(forbidden));
    }

    let destination = TestDirectory::new();
    let destination_root = destination.0.join("destination");
    let mut vault = FakeVault::default();
    let plan = plan_package_activation_with_vault(&destination_root, &package, &key, &vault)
        .expect("plan proxy fixture import");
    assert!(!destination_root.exists());
    assert_eq!(plan.create_proxy_ids.len(), 4);
    assert_eq!(plan.create_credential_ids.len(), 2);

    let run_id = "mig-004-proxies";
    let first = activate_package_with_vault(&destination_root, run_id, &package, &key, &mut vault)
        .expect("activate proxy fixture");
    let repeated =
        activate_package_with_vault(&destination_root, run_id, &package, &key, &mut vault)
            .expect("repeat proxy fixture");
    let repository = LocalRepository::open(&destination_root).expect("open proxy destination");
    let mut protocols = repository
        .list::<Proxy>()
        .expect("list imported proxies")
        .into_iter()
        .map(|proxy| match proxy.value.protocol {
            ProxyProtocol::Http => "http",
            ProxyProtocol::Https => "https",
            ProxyProtocol::Socks4 => "socks4",
            ProxyProtocol::Socks5 => "socks5",
        })
        .collect::<Vec<_>>();
    protocols.sort_unstable();
    assert_eq!(protocols, ["http", "https", "socks4", "socks5"]);
    assert!(
        repository
            .list::<Proxy>()
            .expect("list imported proxy hosts")
            .iter()
            .any(|proxy| proxy.value.host.contains("2001:db8::10"))
    );

    let rollback = rollback_activation_with_vault(
        &destination_root,
        run_id,
        confirm_rollback("ROLLBACK_MIGRATION_RUN").expect("confirm proxy rollback"),
        &mut vault,
    )
    .expect("rollback proxy fixture");
    assert!(vault.values.is_empty());
    assert_json_lines_file(
        &expected.join("import-results.jsonl"),
        &[
            json!({
                "status": "planned",
                "createProxies": plan.create_proxy_ids.len(),
                "createCredentials": plan.create_credential_ids.len(),
                "destinationChanged": false,
            }),
            json!({
                "status": "activated",
                "createdProxies": first.created_proxies,
                "createdCredentials": first.created_credentials,
                "protocols": protocols,
                "ipv6Preserved": true,
            }),
            json!({
                "status": "repeated",
                "reusedProxies": repeated.reused_proxies,
                "reusedCredentials": repeated.reused_credentials,
            }),
            json!({
                "status": "rolledBack",
                "quarantinedProxies": rollback.quarantined_proxies.len(),
                "removedCredentials": rollback.removed_credentials.len(),
                "vaultEmpty": true,
            }),
        ],
    );
    assert_eq!(
        before,
        create_snapshot(&source).expect("snapshot proxy fixture after test")
    );
}

#[test]
fn mig_006_wireguard_vault_binding_repeats_and_rolls_back() {
    let fixture = fixture("MIG-006-vpn-wireguard");
    let source = fixture.join("source");
    let expected = fixture.join("expected");
    let before = create_snapshot(&source).expect("snapshot VPN fixture before test");
    let (report, snapshot) = inspect_source(&source).expect("inspect VPN fixture");
    assert_json_file(&expected.join("inspect.json"), &report);
    assert_json_file(&expected.join("snapshot.json"), &snapshot);
    assert_eq!(report.entity_counts["profiles"], 1);
    assert_eq!(report.entity_counts["vpns"], 1);
    assert_eq!(
        snapshot
            .files
            .iter()
            .filter(|entry| entry.path.starts_with("vpn/"))
            .filter(|entry| entry.sha256 == "redacted")
            .count(),
        2
    );

    let redacted = build_neutral_package(&report, &snapshot, &fixture_metadata())
        .expect("build VPN redacted export");
    let verified = verify_package(&redacted).expect("verify VPN redacted export");
    let manifest: NeutralPackageManifest = serde_json::from_slice(
        redacted
            .file("manifest.json")
            .expect("VPN redacted manifest"),
    )
    .expect("parse VPN redacted manifest");
    assert_json_file(
        &expected.join("export-summary.json"),
        &json!({
            "packageId": manifest.package_id,
            "files": redacted.files().len(),
            "sourceTreeSha256": snapshot.source_tree_sha256,
            "checksumsSha256": verified.checksums_sha256,
            "containsSecrets": manifest.security.contains_secrets,
        }),
    );

    let key = MigrationKey::from_bytes([6; 32]);
    let package =
        build_encrypted_profile_package(&source, &report, &snapshot, &fixture_metadata(), &key)
            .expect("build VPN encrypted package");
    verify_package(&package).expect("verify VPN encrypted package");
    let encrypted_manifest: NeutralPackageManifest = serde_json::from_slice(
        package
            .file("manifest.json")
            .expect("VPN encrypted manifest"),
    )
    .expect("parse VPN encrypted manifest");
    assert!(encrypted_manifest.security.encrypted);
    assert!(encrypted_manifest.security.contains_secrets);
    let package_bytes = package
        .files()
        .values()
        .flat_map(|bytes| bytes.iter().copied())
        .collect::<Vec<_>>();
    let package_text = String::from_utf8_lossy(&package_bytes);
    for forbidden in ["MIG-006-WG-PRIVATE-KEY", "MIG-006-WG-PUBLIC-KEY"] {
        assert!(!package_text.contains(forbidden));
    }

    let destination = TestDirectory::new();
    let destination_root = destination.0.join("destination");
    let mut vault = FakeVault::default();
    let plan = plan_package_activation_with_vault(&destination_root, &package, &key, &vault)
        .expect("plan VPN fixture import");
    assert!(!destination_root.exists());
    assert_eq!(plan.create_profile_ids.len(), 1);
    assert_eq!(plan.create_vpn_ids.len(), 1);
    assert_eq!(plan.create_vpn_config_ids.len(), 1);

    let run_id = "mig-006-wireguard";
    let first = activate_package_with_vault(&destination_root, run_id, &package, &key, &mut vault)
        .expect("activate VPN fixture");
    let repeated =
        activate_package_with_vault(&destination_root, run_id, &package, &key, &mut vault)
            .expect("repeat VPN fixture");
    let repository = LocalRepository::open(&destination_root).expect("open VPN destination");
    let profiles = repository
        .list::<Profile>()
        .expect("list VPN-bound profiles");
    assert_eq!(profiles.len(), 1);
    assert!(profiles[0].value.proxy_id.is_none());
    assert!(profiles[0].value.vpn_id.is_some());
    let vpns = repository.list::<Vpn>().expect("list imported VPNs");
    assert_eq!(vpns.len(), 1);
    assert_eq!(vpns[0].value.kind, VpnKind::Wireguard);
    let config_key = SecretKey::new("vpn-configs", &plan.create_vpn_config_ids[0])
        .expect("valid VPN config key");
    let (secret, _) = vault.read(&config_key).expect("read imported VPN config");
    let stored: buglogin_migration::encrypted_vpn_configs::VpnConfigSecret =
        serde_json::from_slice(secret.as_bytes()).expect("parse imported VPN config");
    assert!(stored.config_data.contains("MIG-006-WG-PRIVATE-KEY"));
    assert!(stored.config_data.contains("MIG-006-WG-PUBLIC-KEY"));

    let rollback = rollback_activation_with_vault(
        &destination_root,
        run_id,
        confirm_rollback("ROLLBACK_MIGRATION_RUN").expect("confirm VPN rollback"),
        &mut vault,
    )
    .expect("rollback VPN fixture");
    assert!(vault.values.is_empty());
    assert_json_lines_file(
        &expected.join("import-results.jsonl"),
        &[
            json!({
                "status": "planned",
                "createProfiles": plan.create_profile_ids.len(),
                "createVpns": plan.create_vpn_ids.len(),
                "createVpnConfigs": plan.create_vpn_config_ids.len(),
                "destinationChanged": false,
            }),
            json!({
                "status": "activated",
                "createdProfiles": first.created_profiles,
                "createdVpns": first.created_vpns,
                "createdVpnConfigs": first.created_vpn_configs,
                "profileVpnBound": true,
                "profileProxyAbsent": true,
            }),
            json!({
                "status": "repeated",
                "reusedProfiles": repeated.reused_profiles,
                "reusedVpns": repeated.reused_vpns,
                "reusedVpnConfigs": repeated.reused_vpn_configs,
            }),
            json!({
                "status": "rolledBack",
                "quarantinedProfiles": rollback.quarantined_profiles.len(),
                "quarantinedVpns": rollback.quarantined_vpns.len(),
                "removedVpnConfigs": rollback.removed_vpn_config_ids.len(),
                "vaultEmpty": true,
            }),
        ],
    );
    assert_eq!(
        before,
        create_snapshot(&source).expect("snapshot VPN fixture after test")
    );
}

#[test]
fn mig_009_dangling_references_drop_optional_and_block_required() {
    let fixture = fixture("MIG-009-dangling-references");
    let source = fixture.join("source");
    let expected = fixture.join("expected");
    let before = create_snapshot(&source).expect("snapshot dangling fixture before test");

    let mapping = map_legacy_profiles_for_encrypted_package(&source, "mig-009-dangling-references")
        .expect("map dangling profile references");
    assert_eq!(mapping.records.len(), 1);
    assert_eq!(mapping.warnings.len(), 4);
    let profile = &mapping.records[0];
    assert!(profile.data.group_id.is_none());
    assert!(profile.data.proxy_id.is_none());
    assert!(profile.data.vpn_id.is_none());
    assert!(profile.data.extension_group_id.is_none());
    assert_eq!(
        profile.extensions["migration_reference_warnings"]
            .as_array()
            .unwrap()
            .len(),
        4
    );

    let required_error = map_legacy_extensions(&source, "mig-009-dangling-references")
        .expect_err("required extension assignment must block");
    assert_eq!(required_error.code, "migration_extension_invalid");
    assert_eq!(
        required_error.fields["field"],
        "dangling extension group assignment"
    );

    let (report, snapshot) = inspect_source(&source).expect("inspect dangling fixture");
    assert_json_file(&expected.join("inspect.json"), &report);
    assert_json_file(&expected.join("snapshot.json"), &snapshot);
    assert_eq!(report.warnings.len(), 4);
    assert_eq!(report.dangling_references.len(), 1);
    assert_eq!(
        report.cutover_eligibility,
        buglogin_migration::inspect::CutoverEligibility::Blocked
    );

    let destination = TestDirectory::new();
    let destination_root = destination.0.join("destination");
    let error = build_encrypted_profile_package(
        &source,
        &report,
        &snapshot,
        &fixture_metadata(),
        &MigrationKey::from_bytes([9; 32]),
    )
    .expect_err("required dangling reference must block package");
    assert_eq!(error.code, "migration_cutover_blocked");
    assert!(!destination_root.exists());
    assert_json_file(
        &expected.join("export-summary.json"),
        &json!({
            "status": "blocked",
            "code": error.code,
            "optionalReferencesDropped": mapping.warnings.len(),
            "requiredDanglingReferences": report.dangling_references.len(),
        }),
    );
    assert_json_lines_file(
        &expected.join("import-results.jsonl"),
        &[json!({
            "status": "blocked",
            "createdProfiles": 0,
            "destinationChanged": false,
            "sourceUnchanged": true,
        })],
    );
    assert_eq!(
        before,
        create_snapshot(&source).expect("snapshot dangling fixture after test")
    );
}

#[test]
fn mig_010_duplicate_proxy_ids_fail_closed_without_a_winner() {
    let fixture = fixture("MIG-010-duplicate-source-ids");
    let source = fixture.join("source");
    let expected = fixture.join("expected");
    let before = create_snapshot(&source).expect("snapshot duplicate fixture before test");
    let analysis = analyze_legacy_proxy_batch(&source, "mig-010-duplicate-source-ids")
        .expect("analyze duplicate proxy fixture");
    assert_eq!(analysis.valid_records, 0);
    assert_eq!(analysis.invalid_records, 2);
    assert!(
        analysis
            .rows
            .iter()
            .all(|row| { row.code.as_deref() == Some("migration_proxy_duplicate_source_id") })
    );
    assert_json_file(&expected.join("proxy-analysis.json"), &analysis);

    let (report, snapshot) = inspect_source(&source).expect("inspect duplicate fixture");
    assert_json_file(&expected.join("inspect.json"), &report);
    assert_json_file(&expected.join("snapshot.json"), &snapshot);
    assert_eq!(report.invalid_records.len(), 2);
    let mapping_error = buglogin_migration::legacy::map_legacy_proxies_for_encrypted_package(
        &source,
        "mig-010-duplicate-source-ids",
    )
    .err()
    .expect("duplicate proxy mapping must fail");
    assert_eq!(mapping_error.code, "migration_proxy_duplicate_source_id");

    let destination = TestDirectory::new();
    let destination_root = destination.0.join("destination");
    let error = build_encrypted_profile_package(
        &source,
        &report,
        &snapshot,
        &fixture_metadata(),
        &MigrationKey::from_bytes([10; 32]),
    )
    .expect_err("duplicate proxy package must be blocked");
    assert_eq!(error.code, "migration_cutover_blocked");
    assert!(!destination_root.exists());
    assert_json_file(
        &expected.join("export-summary.json"),
        &json!({"status":"blocked","code":error.code,"validRows":0,"invalidRows":2}),
    );
    assert_json_lines_file(
        &expected.join("import-results.jsonl"),
        &[json!({
            "status":"blocked",
            "code":"migration_cutover_blocked",
            "createdProxies":0,
            "destinationChanged":false,
            "sourceUnchanged":true
        })],
    );
    assert_eq!(
        before,
        create_snapshot(&source).expect("snapshot duplicate fixture after test")
    );
}

#[test]
fn mig_011_exact_package_reuses_across_distinct_run_ids() {
    let fixture = fixture("MIG-011-idempotent-repeat");
    let source = fixture.join("source");
    let expected = fixture.join("expected");
    let before = create_snapshot(&source).expect("snapshot idempotent fixture before test");
    let (report, snapshot) = inspect_source(&source).expect("inspect idempotent fixture");
    assert_json_file(&expected.join("inspect.json"), &report);
    assert_json_file(&expected.join("snapshot.json"), &snapshot);

    let redacted = build_neutral_package(&report, &snapshot, &fixture_metadata())
        .expect("build idempotent redacted package");
    let redacted_verified = verify_package(&redacted).expect("verify idempotent redacted package");
    let redacted_manifest: NeutralPackageManifest =
        serde_json::from_slice(redacted.file("manifest.json").expect("idempotent manifest"))
            .expect("parse idempotent manifest");
    assert_json_file(
        &expected.join("export-summary.json"),
        &json!({
            "packageId":redacted_manifest.package_id,
            "files":redacted.files().len(),
            "sourceTreeSha256":snapshot.source_tree_sha256,
            "checksumsSha256":redacted_verified.checksums_sha256,
            "containsSecrets":redacted_manifest.security.contains_secrets
        }),
    );

    let key = MigrationKey::from_bytes([11; 32]);
    let package =
        build_encrypted_profile_package(&source, &report, &snapshot, &fixture_metadata(), &key)
            .expect("build idempotent encrypted package");
    let destination = TestDirectory::new();
    let destination_root = destination.0.join("destination");
    let first_plan = plan_package_activation(&destination_root, &package, &key)
        .expect("plan first idempotent import");
    assert_eq!(first_plan.create_profile_ids.len(), 1);
    let first = activate_package(&destination_root, "mig-011-first", &package, Some(&key))
        .expect("activate first idempotent import");
    assert_eq!(first.created_profiles, 1);

    let second_plan = plan_package_activation(&destination_root, &package, &key)
        .expect("plan second idempotent import");
    assert!(second_plan.create_profile_ids.is_empty());
    assert_eq!(second_plan.reuse_profile_ids.len(), 1);
    let second = activate_package(&destination_root, "mig-011-second", &package, Some(&key))
        .expect("activate second idempotent import");
    assert_eq!(second.created_profiles, 0);
    assert_eq!(second.reused_profiles, 1);
    let repository = LocalRepository::open(&destination_root).expect("open idempotent repository");
    let profiles = repository.list::<Profile>().unwrap();
    assert_eq!(profiles.len(), 1);
    assert_eq!(profiles[0].revision, 1);

    let second_rollback = rollback_activation(
        &destination_root,
        "mig-011-second",
        confirm_rollback("ROLLBACK_MIGRATION_RUN").unwrap(),
    )
    .expect("rollback reuse-only run");
    assert!(second_rollback.quarantined_profiles.is_empty());
    assert!(
        repository
            .get::<Profile>(profiles[0].value.id.as_str())
            .unwrap()
            .is_some()
    );
    let first_rollback = rollback_activation(
        &destination_root,
        "mig-011-first",
        confirm_rollback("ROLLBACK_MIGRATION_RUN").unwrap(),
    )
    .expect("rollback creating run");
    assert_eq!(first_rollback.quarantined_profiles.len(), 1);
    assert!(
        repository
            .get::<Profile>(profiles[0].value.id.as_str())
            .unwrap()
            .is_none()
    );

    assert_json_lines_file(
        &expected.join("import-results.jsonl"),
        &[
            json!({"status":"first","createdProfiles":first.created_profiles,"revision":1}),
            json!({"status":"second","createdProfiles":second.created_profiles,"reusedProfiles":second.reused_profiles,"revision":1}),
            json!({"status":"secondRollback","quarantinedProfiles":second_rollback.quarantined_profiles.len(),"profileRetained":true}),
            json!({"status":"firstRollback","quarantinedProfiles":first_rollback.quarantined_profiles.len(),"profileRetained":false}),
        ],
    );
    assert_eq!(
        before,
        create_snapshot(&source).expect("snapshot idempotent fixture after test")
    );
}

#[test]
fn mig_012_destination_collision_never_overwrites_foreign_data() {
    let fixture = fixture("MIG-012-destination-collision");
    let source = fixture.join("source");
    let expected = fixture.join("expected");
    let before = create_snapshot(&source).expect("snapshot collision fixture before test");
    let (report, snapshot) = inspect_source(&source).expect("inspect collision fixture");
    assert_json_file(&expected.join("inspect.json"), &report);
    assert_json_file(&expected.join("snapshot.json"), &snapshot);

    let redacted = build_neutral_package(&report, &snapshot, &fixture_metadata())
        .expect("build collision redacted package");
    let redacted_verified = verify_package(&redacted).expect("verify collision redacted package");
    let redacted_manifest: NeutralPackageManifest =
        serde_json::from_slice(redacted.file("manifest.json").expect("collision manifest"))
            .expect("parse collision manifest");
    assert_json_file(
        &expected.join("export-summary.json"),
        &json!({
            "packageId":redacted_manifest.package_id,
            "files":redacted.files().len(),
            "sourceTreeSha256":snapshot.source_tree_sha256,
            "checksumsSha256":redacted_verified.checksums_sha256,
            "containsSecrets":redacted_manifest.security.contains_secrets
        }),
    );

    let key = MigrationKey::from_bytes([12; 32]);
    let package =
        build_encrypted_profile_package(&source, &report, &snapshot, &fixture_metadata(), &key)
            .expect("build collision encrypted package");
    let destination = TestDirectory::new();
    let destination_root = destination.0.join("destination");
    let initial = plan_package_activation(&destination_root, &package, &key)
        .expect("plan collision package before destination seed");
    assert_eq!(initial.create_profile_ids.len(), 1);
    let profile_id = initial.create_profile_ids[0].clone();
    let repository = LocalRepository::open(&destination_root).expect("open collision repository");
    let foreign = foreign_profile(&profile_id);
    let stored = repository
        .create(foreign.clone())
        .expect("seed foreign profile");
    let profile_data = destination_root
        .join("profiles")
        .join(&profile_id)
        .join("profile")
        .join("Preferences");
    fs::create_dir_all(profile_data.parent().unwrap()).expect("create foreign profile data root");
    fs::copy(
        source
            .join("profiles")
            .join("11000000-0000-4000-8000-000000000011")
            .join("profile")
            .join("Preferences"),
        &profile_data,
    )
    .expect("seed matching foreign profile data");
    let data_before = fs::read(&profile_data).expect("read foreign profile data before plan");

    let plan_error = plan_package_activation(&destination_root, &package, &key)
        .expect_err("foreign destination profile must block plan");
    assert_eq!(plan_error.code, "migration_activation_collision");
    let activation_error =
        activate_package(&destination_root, "mig-012-collision", &package, Some(&key))
            .expect_err("foreign destination profile must block activation");
    assert_eq!(activation_error.code, "migration_activation_collision");

    let retained = repository
        .get::<Profile>(&profile_id)
        .expect("read foreign profile after collision")
        .expect("foreign profile retained");
    assert_eq!(retained.revision, stored.revision);
    assert_eq!(retained.value, foreign);
    assert_eq!(fs::read(&profile_data).unwrap(), data_before);
    let journal = destination_root
        .join("migration-activations")
        .join("mig-012-collision.json");
    let pending_journal = destination_root
        .join("migration-activations")
        .join(".mig-012-collision.pending.json");
    let staging_marker = profile_data
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join(".buglogin-migration-mig-012-collision");
    assert!(!journal.exists());
    assert!(!pending_journal.exists());
    assert!(!staging_marker.exists());
    assert_json_lines_file(
        &expected.join("import-results.jsonl"),
        &[json!({
            "status":"blocked",
            "code":activation_error.code,
            "foreignRevision":retained.revision,
            "foreignUnchanged":retained.value == foreign,
            "profileDataUnchanged":fs::read(&profile_data).unwrap() == data_before,
            "journalCreated":journal.exists() || pending_journal.exists(),
            "stagingMarkerCreated":staging_marker.exists(),
            "sourceUnchanged":before == create_snapshot(&source).unwrap()
        })],
    );
    assert_eq!(
        before,
        create_snapshot(&source).expect("snapshot collision fixture after test")
    );
}

#[test]
fn mig_013_soft_deleted_profiles_are_reported_but_never_imported() {
    let fixture = fixture("MIG-013-soft-deleted");
    let source = fixture.join("source");
    let expected = fixture.join("expected");
    let before = create_snapshot(&source).expect("snapshot soft-deleted fixture before test");
    let (report, snapshot) = inspect_source(&source).expect("inspect soft-deleted fixture");
    assert_json_file(&expected.join("inspect.json"), &report);
    assert_json_file(&expected.join("snapshot.json"), &snapshot);

    let redacted = build_neutral_package(&report, &snapshot, &fixture_metadata())
        .expect("build soft-deleted redacted package");
    let redacted_verified =
        verify_package(&redacted).expect("verify soft-deleted redacted package");
    let redacted_manifest: NeutralPackageManifest = serde_json::from_slice(
        redacted
            .file("manifest.json")
            .expect("soft-deleted manifest"),
    )
    .expect("parse soft-deleted manifest");
    assert_json_file(
        &expected.join("export-summary.json"),
        &json!({
            "packageId":redacted_manifest.package_id,
            "files":redacted.files().len(),
            "sourceTreeSha256":snapshot.source_tree_sha256,
            "checksumsSha256":redacted_verified.checksums_sha256,
            "containsSecrets":redacted_manifest.security.contains_secrets
        }),
    );

    let key = MigrationKey::from_bytes([13; 32]);
    let package =
        build_encrypted_profile_package(&source, &report, &snapshot, &fixture_metadata(), &key)
            .expect("build soft-deleted encrypted package");
    let package_bytes = package
        .files()
        .values()
        .flat_map(|bytes| bytes.iter().copied())
        .collect::<Vec<_>>();
    let deleted_bytes_packaged =
        String::from_utf8_lossy(&package_bytes).contains("MIG-013-DELETED-MUST-NOT-PACKAGE");
    assert!(!deleted_bytes_packaged);

    let destination = TestDirectory::new();
    let destination_root = destination.0.join("destination");
    let plan = plan_package_activation(&destination_root, &package, &key)
        .expect("plan soft-deleted package");
    assert_eq!(plan.create_profile_ids.len(), 1);
    let activation = activate_package(
        &destination_root,
        "mig-013-soft-deleted",
        &package,
        Some(&key),
    )
    .expect("activate soft-deleted package");
    let repository = LocalRepository::open(&destination_root).expect("open destination repository");
    let profiles = repository
        .list::<Profile>()
        .expect("list imported profiles");
    assert_eq!(profiles.len(), 1);
    assert_eq!(profiles[0].value.name, "Active Profile");
    let reconciliation = reconcile_activation(&report, &destination_root, "mig-013-soft-deleted")
        .expect("reconcile soft-deleted activation");
    assert_eq!(reconciliation.status, ReconciliationStatus::Passed);
    assert!(reconciliation.unsupported_entity_counts.is_empty());
    let rollback = rollback_activation(
        &destination_root,
        "mig-013-soft-deleted",
        confirm_rollback("ROLLBACK_MIGRATION_RUN").unwrap(),
    )
    .expect("rollback soft-deleted activation");

    assert_json_lines_file(
        &expected.join("import-results.jsonl"),
        &[
            json!({
                "status":"planned",
                "activeProfiles":plan.create_profile_ids.len(),
                "deletedProfilesReported":report.entity_counts["profilesDeleted"],
                "deletedRuntimeExcluded":report.runtime_findings.iter().all(|finding| finding.runtime != "camoufox")
            }),
            json!({
                "status":"activated",
                "createdProfiles":activation.created_profiles,
                "deletedProfilesImported":profiles.iter().filter(|profile| profile.value.name == "Deleted Camoufox Profile").count(),
                "deletedBytesPackaged":deleted_bytes_packaged
            }),
            json!({
                "status":"reconciled",
                "result":reconciliation.status,
                "unsupportedEntityCounts":reconciliation.unsupported_entity_counts
            }),
            json!({
                "status":"rolledBack",
                "quarantinedProfiles":rollback.quarantined_profiles.len(),
                "sourceUnchanged":before == create_snapshot(&source).unwrap()
            }),
        ],
    );
    assert_eq!(
        before,
        create_snapshot(&source).expect("snapshot soft-deleted fixture after test")
    );
}

#[test]
fn mig_015_rejects_package_traversal_and_source_links_before_extraction() {
    let fixture = fixture("MIG-015-path-traversal");
    let source = fixture.join("source");
    let expected = fixture.join("expected");
    let before = create_snapshot(&source).expect("snapshot traversal fixture before test");
    let (report, snapshot) = inspect_source(&source).expect("inspect traversal fixture");
    assert_json_file(&expected.join("inspect.json"), &report);
    assert_json_file(&expected.join("snapshot.json"), &snapshot);

    let package = build_neutral_package(&report, &snapshot, &fixture_metadata())
        .expect("build traversal baseline package");
    let verified = verify_package(&package).expect("verify traversal baseline package");
    let manifest: NeutralPackageManifest = serde_json::from_slice(
        package
            .file("manifest.json")
            .expect("traversal baseline manifest"),
    )
    .expect("parse traversal baseline manifest");
    assert_json_file(
        &expected.join("export-summary.json"),
        &json!({
            "packageId":manifest.package_id,
            "files":package.files().len(),
            "sourceTreeSha256":snapshot.source_tree_sha256,
            "checksumsSha256":verified.checksums_sha256,
            "containsSecrets":manifest.security.contains_secrets
        }),
    );

    let attack_paths: Vec<String> = serde_json::from_slice(
        &fs::read(fixture.join("attack/package-paths.json")).expect("read traversal paths"),
    )
    .expect("parse traversal paths");
    let destination = TestDirectory::new();
    let mut results = Vec::new();
    for (index, attack_path) in attack_paths.iter().enumerate() {
        let malicious = package_with_extra_path(&package, attack_path);
        let staging = destination.0.join(format!("staging-{index}"));
        let error = stage_package(&malicious, &staging, &format!("mig-015-{index}"))
            .expect_err("traversal package must fail before staging");
        assert_eq!(error.code, "migration_package_path_invalid");
        assert!(!staging.exists());
        results.push(json!({
            "status":"blocked",
            "path":attack_path,
            "code":error.code,
            "stagingCreated":staging.exists()
        }));
    }

    let linked_source = destination.0.join("linked-source");
    let outside = destination.0.join("outside");
    fs::create_dir_all(&linked_source).expect("create linked source");
    fs::create_dir_all(&outside).expect("create outside directory");
    fs::copy(
        source.join("version.json"),
        linked_source.join("version.json"),
    )
    .expect("copy linked source metadata");
    let canary = outside.join("outside-canary.txt");
    fs::write(&canary, b"MIG-015-OUTSIDE").expect("write outside canary");
    let link = linked_source.join("profiles");
    create_directory_link(&outside, &link);
    let link_error = create_snapshot(&linked_source).expect_err("source link must be rejected");
    assert_eq!(link_error.code, "migration_source_symlink_unsupported");
    let outside_unchanged = fs::read(&canary).unwrap() == b"MIG-015-OUTSIDE";
    results.push(json!({
        "status":"blocked",
        "path":"source-directory-link",
        "code":link_error.code,
        "outsideUnchanged":outside_unchanged
    }));
    fs::remove_dir(&link).expect("remove test directory link");

    assert_json_lines_file(&expected.join("import-results.jsonl"), &results);
    assert_eq!(
        before,
        create_snapshot(&source).expect("snapshot traversal fixture after test")
    );
}

#[test]
fn mig_016_hash_mismatch_blocks_source_drift_and_tampered_package() {
    let fixture = fixture("MIG-016-hash-mismatch");
    let source = fixture.join("source");
    let expected = fixture.join("expected");
    let before = create_snapshot(&source).expect("snapshot hash fixture before test");
    let (report, snapshot) = inspect_source(&source).expect("inspect hash fixture");
    assert_json_file(&expected.join("inspect.json"), &report);
    assert_json_file(&expected.join("snapshot.json"), &snapshot);

    let redacted = build_neutral_package(&report, &snapshot, &fixture_metadata())
        .expect("build hash redacted package");
    let redacted_verified = verify_package(&redacted).expect("verify hash redacted package");
    let redacted_manifest: NeutralPackageManifest =
        serde_json::from_slice(redacted.file("manifest.json").expect("hash manifest"))
            .expect("parse hash manifest");
    assert_json_file(
        &expected.join("export-summary.json"),
        &json!({
            "packageId":redacted_manifest.package_id,
            "files":redacted.files().len(),
            "sourceTreeSha256":snapshot.source_tree_sha256,
            "checksumsSha256":redacted_verified.checksums_sha256,
            "containsSecrets":redacted_manifest.security.contains_secrets
        }),
    );

    let key = MigrationKey::from_bytes([16; 32]);
    let package =
        build_encrypted_profile_package(&source, &report, &snapshot, &fixture_metadata(), &key)
            .expect("build hash encrypted package");
    verify_package(&package).expect("verify clean hash package");

    let rehearsal = TestDirectory::new();
    let mutable_source = rehearsal.0.join("mutable-source");
    let mutable_profile =
        mutable_source.join("profiles/16000000-0000-4000-8000-000000000016/profile");
    fs::create_dir_all(&mutable_profile).expect("create mutable source");
    fs::copy(
        source.join("version.json"),
        mutable_source.join("version.json"),
    )
    .expect("copy mutable version");
    fs::copy(
        source.join("profiles/16000000-0000-4000-8000-000000000016/metadata.json"),
        mutable_profile.parent().unwrap().join("metadata.json"),
    )
    .expect("copy mutable metadata");
    fs::copy(
        source.join("profiles/16000000-0000-4000-8000-000000000016/profile/Preferences"),
        mutable_profile.join("Preferences"),
    )
    .expect("copy mutable profile data");
    let (_, mutable_snapshot) = inspect_source(&mutable_source).expect("inspect mutable source");
    fs::write(mutable_profile.join("Preferences"), b"MIG-016-SOURCE-DRIFT")
        .expect("mutate source after snapshot");
    let source_error = build_encrypted_profile_package(
        &mutable_source,
        &report,
        &mutable_snapshot,
        &fixture_metadata(),
        &key,
    )
    .expect_err("source drift must block packaging");
    assert_eq!(source_error.code, "migration_source_changed");
    let mutable_after = create_snapshot(&mutable_source).expect("snapshot mutated source");
    assert_ne!(
        mutable_snapshot.source_tree_sha256,
        mutable_after.source_tree_sha256
    );

    let tampered_path = package
        .files()
        .keys()
        .find(|path| {
            path.starts_with("encrypted/profile-data/")
                && path.as_str() != "encrypted/profile-data/index.json"
        })
        .cloned()
        .expect("encrypted profile object");
    let mut tampered_files = package.files().clone();
    tampered_files.get_mut(&tampered_path).unwrap()[0] ^= 1;
    let tampered_package = NeutralPackage::from_files(tampered_files);
    let tamper_error = verify_package(&tampered_package)
        .expect_err("tampered package must fail checksum verification");
    assert_eq!(tamper_error.code, "migration_checksum_mismatch");
    let staging = rehearsal.0.join("staging");
    let stage_error = stage_package(&tampered_package, &staging, "mig-016-tampered")
        .expect_err("tampered package must not stage");
    assert_eq!(stage_error.code, "migration_checksum_mismatch");
    assert!(!staging.exists());

    assert_json_lines_file(
        &expected.join("import-results.jsonl"),
        &[
            json!({
                "status":"blocked",
                "layer":"source",
                "code":source_error.code,
                "sourceAfterMutationStable":mutable_after == create_snapshot(&mutable_source).unwrap()
            }),
            json!({
                "status":"blocked",
                "layer":"package",
                "code":stage_error.code,
                "stagingCreated":staging.exists()
            }),
        ],
    );
    assert_eq!(
        before,
        create_snapshot(&source).expect("snapshot hash fixture after test")
    );
}

#[test]
fn mig_018_acl_fixture_preserves_effective_share_matrix() {
    let fixture = fixture("MIG-018-acl-complete");
    let source = fixture.join("source");
    let expected = fixture.join("expected");
    let before = create_snapshot(&source).expect("snapshot ACL fixture before test");
    let (report, snapshot) = inspect_source(&source).expect("inspect ACL fixture");
    assert_json_file(&expected.join("inspect.json"), &report);
    assert_json_file(&expected.join("snapshot.json"), &snapshot);

    let redacted = build_neutral_package(&report, &snapshot, &fixture_metadata())
        .expect("build ACL redacted package");
    let redacted_verified = verify_package(&redacted).expect("verify ACL redacted package");
    let manifest: NeutralPackageManifest =
        serde_json::from_slice(redacted.file("manifest.json").expect("ACL manifest"))
            .expect("parse ACL manifest");
    assert_json_file(
        &expected.join("export-summary.json"),
        &json!({
            "packageId":manifest.package_id,
            "files":redacted.files().len(),
            "sourceTreeSha256":snapshot.source_tree_sha256,
            "checksumsSha256":redacted_verified.checksums_sha256,
            "containsSecrets":manifest.security.contains_secrets
        }),
    );

    let control_export =
        fs::read(source.join("control-plane-export.json")).expect("read ACL control export");
    let mapping = map_legacy_control_plane_json(&control_export, &report.source.instance_id)
        .expect("map ACL control export");
    assert_eq!(mapping.accounts.len(), 3);
    assert_eq!(mapping.organizations.len(), 1);
    assert_eq!(mapping.workspaces.len(), 1);
    assert_eq!(mapping.teams.len(), 1);
    assert_eq!(mapping.members.len(), 3);
    assert_eq!(mapping.resources.len(), 1);
    assert_eq!(mapping.profile_resources.len(), 1);
    let shared_member = mapping
        .members
        .iter()
        .find(|member| member.source_ref.id == "workspace-acl:member-acl")
        .expect("shared member mapping");
    let unshared_member = mapping
        .members
        .iter()
        .find(|member| member.source_ref.id == "workspace-acl:viewer-acl")
        .expect("unshared member mapping");
    let shared_actions = mapping
        .grants
        .iter()
        .filter(|grant| grant.data.member_id == shared_member.id)
        .map(|grant| grant.data.action.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        shared_actions,
        std::collections::BTreeSet::from(["execute", "read"])
    );
    assert_eq!(
        mapping
            .grants
            .iter()
            .filter(|grant| grant.data.member_id == unshared_member.id)
            .count(),
        0
    );

    let package = build_encrypted_profile_package(
        &source,
        &report,
        &snapshot,
        &fixture_metadata(),
        &MigrationKey::from_bytes([18; 32]),
    )
    .expect("build ACL encrypted package");
    verify_package(&package).expect("verify ACL encrypted package");
    for (path, count) in [
        ("entities/accounts.jsonl", 3),
        ("entities/organizations.jsonl", 1),
        ("entities/workspaces.jsonl", 1),
        ("entities/teams.jsonl", 1),
        ("entities/members.jsonl", 3),
        ("entities/resources.jsonl", 1),
        ("entities/profile-resources.jsonl", 1),
        ("entities/grants.jsonl", 2),
    ] {
        assert_eq!(
            package
                .file(path)
                .unwrap()
                .split(|byte| *byte == b'\n')
                .filter(|line| !line.is_empty())
                .count(),
            count,
            "wrong ACL record count for {path}"
        );
    }
    assert_json_lines_file(
        &expected.join("import-results.jsonl"),
        &[json!({
            "status":"completed",
            "accounts":mapping.accounts.len(),
            "organizations":mapping.organizations.len(),
            "workspaces":mapping.workspaces.len(),
            "teams":mapping.teams.len(),
            "members":mapping.members.len(),
            "resources":mapping.resources.len(),
            "profileResources":mapping.profile_resources.len(),
            "shareActions":shared_actions,
            "unsharedGrantCount":mapping.grants.iter().filter(|grant| grant.data.member_id == unshared_member.id).count(),
            "leasePolicy":"destination_reissue",
            "sourceUnchanged":before == create_snapshot(&source).unwrap()
        })],
    );
    assert_eq!(
        before,
        create_snapshot(&source).expect("snapshot ACL fixture after test")
    );
}

#[test]
fn mig_019_unknown_acl_role_and_access_mode_fail_closed() {
    let fixture = fixture("MIG-019-acl-unknown-role");
    let source = fixture.join("source");
    let expected = fixture.join("expected");
    let before = create_snapshot(&source).expect("snapshot unknown ACL fixture before test");
    let (report, snapshot) = inspect_source(&source).expect("inspect unknown ACL fixture");
    assert_json_file(&expected.join("inspect.json"), &report);
    assert_json_file(&expected.join("snapshot.json"), &snapshot);

    let unknown_role =
        fs::read(source.join("control-plane-export.json")).expect("read unknown role export");
    let role_error = map_legacy_control_plane_json(&unknown_role, &report.source.instance_id)
        .expect_err("unknown role must block mapping");
    assert_eq!(role_error.code, "migration_control_plane_invalid");
    assert_eq!(role_error.fields["field"], "membership role");

    let unknown_access = fs::read(source.join("control-plane-unknown-access-mode.json"))
        .expect("read unknown access mode export");
    let access_error = map_legacy_control_plane_json(&unknown_access, &report.source.instance_id)
        .expect_err("unknown access mode must block mapping");
    assert_eq!(access_error.code, "migration_control_plane_invalid");
    assert_eq!(
        access_error.fields["field"],
        "membership resource access mode"
    );

    let destination = TestDirectory::new();
    let destination_root = destination.0.join("destination");
    let package_error = build_encrypted_profile_package(
        &source,
        &report,
        &snapshot,
        &fixture_metadata(),
        &MigrationKey::from_bytes([19; 32]),
    )
    .expect_err("unknown ACL role must block package creation");
    assert_eq!(package_error.code, "migration_control_plane_invalid");
    assert_json_file(
        &expected.join("export-summary.json"),
        &json!({
            "status":"blocked",
            "code":package_error.code,
            "unknownRoleField":role_error.fields["field"],
            "unknownAccessModeField":access_error.fields["field"],
            "packageBuilt":false
        }),
    );
    assert_json_lines_file(
        &expected.join("import-results.jsonl"),
        &[json!({
            "status":"blocked",
            "destinationChanged":destination_root.exists(),
            "sourceUnchanged":before == create_snapshot(&source).unwrap()
        })],
    );
    assert!(!destination_root.exists());
    assert_eq!(before, create_snapshot(&source).unwrap());
}

#[test]
fn mig_020_sync_object_set_preserves_exact_hashes_and_blocks_set_drift() {
    let fixture = fixture("MIG-020-sync-object-set");
    let source = fixture.join("source");
    let expected = fixture.join("expected");
    let before = create_snapshot(&source).expect("snapshot sync object fixture before test");
    let (report, snapshot) = inspect_source(&source).expect("inspect sync object fixture");
    assert_json_file(&expected.join("inspect.json"), &report);
    assert_json_file(&expected.join("snapshot.json"), &snapshot);

    let package = build_encrypted_profile_package(
        &source,
        &report,
        &snapshot,
        &fixture_metadata(),
        &MigrationKey::from_bytes([20; 32]),
    )
    .expect("build exact sync object package");
    let verified = verify_package(&package).expect("verify exact sync object package");
    let export: buglogin_sync::export::SyncExport = serde_json::from_slice(
        package
            .file("sync/export.json")
            .expect("packaged sync export"),
    )
    .expect("parse packaged sync export");
    export.validate().expect("packaged sync export validates");
    assert_eq!(export.manifest.objects.len(), 3);
    assert_eq!(export.manifest.tombstones.len(), 1);
    assert_eq!(export.blobs.len(), 3);
    assert_eq!(
        export.manifest_sha256,
        "79c1ded49daf41d17c663e00d7bfae527d4e453fa3640300070b29168b446b65"
    );
    assert_eq!(
        export.blob_set_sha256,
        "95ed891b99f80e6dcf5481aa117cc1b3ca065cff691d947f78ab4f5e04b8ede1"
    );
    assert!(!verified.checksums_sha256.is_empty());
    assert_json_file(
        &expected.join("export-summary.json"),
        &json!({
            "status":"completed",
            "objectCount":export.manifest.objects.len(),
            "tombstoneCount":export.manifest.tombstones.len(),
            "blobCount":export.blobs.len(),
            "manifestSha256":export.manifest_sha256,
            "blobSetSha256":export.blob_set_sha256,
            "sourceUnchanged":before == create_snapshot(&source).unwrap()
        }),
    );

    let cases: [(&str, fn(&mut Value)); 4] = [
        ("tampered_blob", |value: &mut Value| {
            value["blobs"][0]["data"][0] = json!(255);
        }),
        ("missing_blob", |value: &mut Value| {
            value["blobs"].as_array_mut().unwrap().pop();
        }),
        ("extra_blob", |value: &mut Value| {
            let mut extra = value["blobs"][0].clone();
            extra["key"] = json!("profile/extra");
            value["blobs"].as_array_mut().unwrap().push(extra);
        }),
        ("duplicate_blob", |value: &mut Value| {
            let duplicate = value["blobs"][0].clone();
            value["blobs"].as_array_mut().unwrap().push(duplicate);
        }),
    ];
    let mut results = vec![json!({
        "status":"completed",
        "objectCount":export.manifest.objects.len(),
        "tombstoneCount":export.manifest.tombstones.len(),
        "blobCount":export.blobs.len(),
        "sourceUnchanged":true
    })];
    for (case, mutate) in cases {
        let temp = TestDirectory::new();
        let temp_source = temp.0.join("source");
        fs::create_dir_all(&temp_source).unwrap();
        fs::copy(
            source.join("version.json"),
            temp_source.join("version.json"),
        )
        .unwrap();
        let mut value: Value =
            serde_json::from_slice(&fs::read(source.join("sync-export.json")).unwrap()).unwrap();
        mutate(&mut value);
        fs::write(
            temp_source.join("sync-export.json"),
            serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();
        let (case_report, case_snapshot) = inspect_source(&temp_source).unwrap();
        let error = build_encrypted_profile_package(
            &temp_source,
            &case_report,
            &case_snapshot,
            &fixture_metadata(),
            &MigrationKey::from_bytes([20; 32]),
        )
        .expect_err("sync object set drift must block packaging");
        assert_eq!(error.code, "migration_sync_export_invalid");
        results.push(json!({
            "status":"blocked",
            "case":case,
            "code":error.code,
            "sourceUnchanged":true
        }));
    }
    assert_json_lines_file(&expected.join("import-results.jsonl"), &results);
    assert_eq!(before, create_snapshot(&source).unwrap());
}

#[test]
fn mig_021_interrupted_staging_resumes_or_discards_without_source_mutation() {
    let fixture = fixture("MIG-021-interrupted-copy");
    let source = fixture.join("source");
    let expected = fixture.join("expected");
    let before = create_snapshot(&source).expect("snapshot interrupted copy fixture before test");
    let (report, snapshot) = inspect_source(&source).expect("inspect interrupted copy fixture");
    assert_json_file(&expected.join("inspect.json"), &report);
    assert_json_file(&expected.join("snapshot.json"), &snapshot);
    let package = build_neutral_package(&report, &snapshot, &fixture_metadata())
        .expect("build interrupted copy package");
    verify_package(&package).expect("verify interrupted copy package");
    assert_json_file(
        &expected.join("export-summary.json"),
        &json!({
            "status":"completed",
            "profileCount":report.entity_counts["profiles"],
            "sourceUnchanged":before == create_snapshot(&source).unwrap()
        }),
    );

    let staging = TestDirectory::new();
    let resumed_run = "mig-021-resume";
    let first = stage_package(&package, &staging.0, resumed_run).expect("initial staging");
    let interrupted_file = first.staging_root.join("manifest.json");
    fs::remove_file(&interrupted_file).expect("simulate process stop after partial staging");
    let missing_before = !interrupted_file.exists();
    let resumed = stage_package(&package, &staging.0, resumed_run).expect("resume staging");
    let restored = resumed.staging_root.join("manifest.json").is_file();

    let partial_run = "mig-021-partial";
    let partial = stage_package(&package, &staging.0, partial_run).expect("stage partial case");
    fs::write(partial.staging_root.join("manifest.json"), b"partial")
        .expect("simulate partial file");
    let partial_error = stage_package(&package, &staging.0, partial_run)
        .expect_err("partial file must not be overwritten during resume");
    assert_eq!(partial_error.code, "migration_staging_file_mismatch");
    let partial_retained = partial.staging_root.exists();
    discard_staging(&package, &staging.0, partial_run).expect("discard owned partial staging");
    let partial_exists_after_discard = partial.staging_root.exists();

    let foreign_run = "mig-021-foreign";
    let foreign = stage_package(&package, &staging.0, foreign_run).expect("stage foreign case");
    let foreign_file = foreign.staging_root.join("foreign.txt");
    fs::write(&foreign_file, b"not migration owned").expect("write foreign staging file");
    let foreign_error = discard_staging(&package, &staging.0, foreign_run)
        .expect_err("discard must reject foreign staging files");
    assert_eq!(foreign_error.code, "migration_staging_replay_mismatch");
    let foreign_retained = foreign.staging_root.exists() && foreign_file.exists();
    fs::remove_file(&foreign_file).expect("remove foreign test file");
    discard_staging(&package, &staging.0, foreign_run).expect("discard owned staging only");
    let foreign_exists_after_discard = foreign.staging_root.exists();

    assert_json_lines_file(
        &expected.join("import-results.jsonl"),
        &[
            json!({
                "status":"resumed",
                "missingBefore":missing_before,
                "restored":restored,
                "sourceUnchanged":before == create_snapshot(&source).unwrap()
            }),
            json!({
                "status":"blocked",
                "case":"partial_file",
                "code":partial_error.code,
                "stagingRetained":partial_retained
            }),
            json!({
                "status":"discarded",
                "case":"partial_file",
                "stagingExists":partial_exists_after_discard,
                "sourceUnchanged":before == create_snapshot(&source).unwrap()
            }),
            json!({
                "status":"blocked",
                "case":"foreign_file",
                "code":foreign_error.code,
                "stagingRetained":foreign_retained
            }),
            json!({
                "status":"discarded",
                "case":"owned_only",
                "stagingExists":foreign_exists_after_discard,
                "sourceUnchanged":before == create_snapshot(&source).unwrap()
            }),
        ],
    );
    assert_eq!(before, create_snapshot(&source).unwrap());
}

#[test]
fn mig_022_disk_capacity_blocks_before_live_or_source_mutation() {
    let fixture = fixture("MIG-022-disk-full");
    let source = fixture.join("source");
    let expected = fixture.join("expected");
    let before = create_snapshot(&source).expect("snapshot disk capacity fixture before test");
    let (report, snapshot) = inspect_source(&source).expect("inspect disk capacity fixture");
    assert_json_file(&expected.join("inspect.json"), &report);
    assert_json_file(&expected.join("snapshot.json"), &snapshot);
    let package = build_neutral_package(&report, &snapshot, &fixture_metadata())
        .expect("build disk capacity package");
    verify_package(&package).expect("verify disk capacity package");
    assert_json_file(
        &expected.join("export-summary.json"),
        &json!({
            "status":"completed",
            "profileCount":report.entity_counts["profiles"],
            "sourceUnchanged":before == create_snapshot(&source).unwrap()
        }),
    );

    let quota = TestDirectory::new();
    let new_staging = quota.0.join("new");
    let new_error = stage_package_with_available_bytes(&package, &new_staging, "mig-022-new", 0)
        .expect_err("zero capacity must block new staging");
    assert_eq!(new_error.code, "migration_staging_capacity_insufficient");
    let new_staging_created = new_staging.join("mig-022-new").exists();
    let destination = quota.0.join("destination");

    let resume_staging = quota.0.join("resume");
    let staged = stage_package(&package, &resume_staging, "mig-022-resume")
        .expect("create interrupted staging");
    fs::remove_file(staged.staging_root.join("manifest.json")).expect("simulate interrupted copy");
    let resume_error =
        stage_package_with_available_bytes(&package, &resume_staging, "mig-022-resume", 0)
            .expect_err("zero capacity must block resume");
    assert_eq!(resume_error.code, "migration_staging_capacity_insufficient");
    let staging_retained = staged.staging_root.exists();
    let destination_changed = destination.exists();
    assert_json_lines_file(
        &expected.join("import-results.jsonl"),
        &[
            json!({
                "status":"blocked",
                "case":"new_staging",
                "code":new_error.code,
                "stagingCreated":new_staging_created,
                "destinationChanged":destination_changed,
                "sourceUnchanged":before == create_snapshot(&source).unwrap()
            }),
            json!({
                "status":"blocked",
                "case":"resume",
                "code":resume_error.code,
                "stagingRetained":staging_retained,
                "destinationChanged":destination_changed,
                "sourceUnchanged":before == create_snapshot(&source).unwrap()
            }),
        ],
    );
    assert_eq!(before, create_snapshot(&source).unwrap());
}

#[test]
fn mig_023_case_and_unicode_collisions_are_cross_platform_safe() {
    let fixture = fixture("MIG-023-case-unicode");
    let source = fixture.join("source");
    let expected = fixture.join("expected");
    let before = create_snapshot(&source).expect("snapshot case Unicode fixture before test");
    let (report, snapshot) = inspect_source(&source).expect("inspect case Unicode fixture");
    assert_json_file(&expected.join("inspect.json"), &report);
    assert_json_file(&expected.join("snapshot.json"), &snapshot);
    let first = map_legacy_profiles_for_encrypted_package(&source, &report.source.instance_id)
        .expect("map Unicode profile first");
    let second = map_legacy_profiles_for_encrypted_package(&source, &report.source.instance_id)
        .expect("map Unicode profile second");
    assert_eq!(first.records.len(), 1);
    assert_eq!(first.records[0].data.name, "Đăng Nhập — Hồ sơ Việt Nam");
    assert_eq!(first.records[0].id, second.records[0].id);
    let package = build_encrypted_profile_package(
        &source,
        &report,
        &snapshot,
        &fixture_metadata(),
        &MigrationKey::from_bytes([23; 32]),
    )
    .expect("build case Unicode package");
    verify_package(&package).expect("verify case Unicode package");
    assert_json_file(
        &expected.join("export-summary.json"),
        &json!({
            "status":"completed",
            "profileName":first.records[0].data.name,
            "deterministicId":first.records[0].id == second.records[0].id,
            "sourceUnchanged":before == create_snapshot(&source).unwrap()
        }),
    );

    let cases: Value = serde_json::from_slice(
        &fs::read(source.join("attack/path-cases.json")).expect("read path collision cases"),
    )
    .expect("parse path collision cases");
    let case_handled = path_collision_is_handled(
        cases["case"].as_array().unwrap()[0].as_str().unwrap(),
        cases["case"].as_array().unwrap()[1].as_str().unwrap(),
    );
    let unicode_handled = path_collision_is_handled(
        cases["unicode"].as_array().unwrap()[0].as_str().unwrap(),
        cases["unicode"].as_array().unwrap()[1].as_str().unwrap(),
    );
    assert_json_lines_file(
        &expected.join("import-results.jsonl"),
        &[
            json!({"status":"handled","case":"ascii_case","filesystemAliasOrMigrationBlock":case_handled}),
            json!({"status":"handled","case":"nfc_nfd","filesystemAliasOrMigrationBlock":unicode_handled}),
            json!({
                "status":"completed",
                "namePreserved":first.records[0].data.name == "Đăng Nhập — Hồ sơ Việt Nam",
                "sourceUnchanged":before == create_snapshot(&source).unwrap()
            }),
        ],
    );
    assert!(case_handled);
    assert!(unicode_handled);
    assert_eq!(before, create_snapshot(&source).unwrap());
}

#[test]
fn mig_024_long_paths_support_deep_files_and_reject_over_limit() {
    let fixture = fixture("MIG-024-long-paths");
    let source = fixture.join("source");
    let expected = fixture.join("expected");
    let before = create_snapshot(&source).expect("snapshot long path fixture before test");
    let (report, snapshot) = inspect_source(&source).expect("inspect long path fixture");
    assert_json_file(&expected.join("inspect.json"), &report);
    assert_json_file(&expected.join("snapshot.json"), &snapshot);
    let package = build_encrypted_profile_package(
        &source,
        &report,
        &snapshot,
        &fixture_metadata(),
        &MigrationKey::from_bytes([24; 32]),
    )
    .expect("build long path package");
    verify_package(&package).expect("verify long path package");

    let probe = TestDirectory::new();
    fs::write(
        probe.0.join("version.json"),
        br#"{"system":"buglogin-legacy","version":"1.1.84","instanceId":"long-path-probe","os":"windows","architecture":"x86_64"}"#,
    )
    .unwrap();
    let component = "segment-0123456789abcdef0123456789abcdef";
    let relative = format!(
        "data/{}/Preferences.backup.json",
        std::iter::repeat_n(component, 9)
            .collect::<Vec<_>>()
            .join("/")
    );
    let deep_file = probe.0.join(&relative);
    fs::create_dir_all(deep_file.parent().unwrap()).expect("create deep path");
    fs::write(&deep_file, b"deep path").expect("write deep path");
    let deep_snapshot = create_snapshot(&probe.0).expect("deep path within portable ceiling");
    assert_eq!(
        deep_snapshot
            .files
            .iter()
            .find(|file| file.path == relative)
            .unwrap()
            .path,
        relative
    );
    let deep_path_bytes = relative.len();

    let over_limit = format!(
        "{}/x",
        std::iter::repeat_n("a", 1024).collect::<Vec<_>>().join("/")
    );
    let source_error = validate_path_length(&over_limit, "migration_source_path_too_long")
        .expect_err("source path over ceiling must fail");
    assert_eq!(source_error.code, "migration_source_path_too_long");
    let package_error =
        validate_relative_path(&over_limit).expect_err("package path over ceiling must fail");
    assert_eq!(package_error.code, "migration_package_path_too_long");
    assert_json_file(
        &expected.join("export-summary.json"),
        &json!({
            "status":"completed",
            "profileCount":report.entity_counts["profiles"],
            "sourceUnchanged":before == create_snapshot(&source).unwrap()
        }),
    );
    assert_json_lines_file(
        &expected.join("import-results.jsonl"),
        &[
            json!({"status":"completed","deepPathBytes":deep_path_bytes,"sourceUnchanged":true}),
            json!({"status":"blocked","case":"source_over_limit","code":source_error.code}),
            json!({"status":"blocked","case":"package_over_limit","code":package_error.code}),
        ],
    );
    assert_eq!(before, create_snapshot(&source).unwrap());
}

#[test]
fn mig_025_locked_source_blocks_then_succeeds_after_release() {
    let fixture = fixture("MIG-025-locked-files");
    let source = fixture.join("source");
    let expected = fixture.join("expected");
    let before = create_snapshot(&source).expect("snapshot locked file fixture before test");
    let locked_path =
        source.join("profiles/25000000-0000-4000-8000-000000000025/profile/Preferences");
    let lock = fs::File::open(&locked_path).expect("open lock target");
    lock.lock_exclusive().expect("lock source file exclusively");
    let locked_error = inspect_source(&source).expect_err("locked source must block snapshot");
    assert_eq!(locked_error.code, "migration_source_locked");
    drop(lock);

    let (report, snapshot) = inspect_source(&source).expect("inspect after lock release");
    assert_json_file(&expected.join("inspect.json"), &report);
    assert_json_file(&expected.join("snapshot.json"), &snapshot);
    let package = build_encrypted_profile_package(
        &source,
        &report,
        &snapshot,
        &fixture_metadata(),
        &MigrationKey::from_bytes([25; 32]),
    )
    .expect("build package after lock release");
    verify_package(&package).expect("verify package after lock release");
    assert_json_file(
        &expected.join("export-summary.json"),
        &json!({
            "status":"completed",
            "lockedCode":locked_error.code,
            "profileCount":report.entity_counts["profiles"],
            "sourceUnchanged":before == create_snapshot(&source).unwrap()
        }),
    );
    assert_json_lines_file(
        &expected.join("import-results.jsonl"),
        &[
            json!({
                "status":"blocked",
                "case":"exclusive_lock",
                "code":locked_error.code,
                "packageBuilt":false,
                "destinationChanged":false,
                "sourceUnchanged":true
            }),
            json!({
                "status":"completed",
                "case":"lock_released",
                "packageBuilt":true,
                "sourceUnchanged":before == create_snapshot(&source).unwrap()
            }),
        ],
    );
    assert_eq!(before, create_snapshot(&source).unwrap());
}

fn path_collision_is_handled(first: &str, second: &str) -> bool {
    let probe = TestDirectory::new();
    fs::write(
        probe.0.join("version.json"),
        br#"{"system":"buglogin-legacy","version":"1.1.84","instanceId":"collision-probe","os":"windows","architecture":"x86_64"}"#,
    )
    .unwrap();
    let data = probe.0.join("data");
    fs::create_dir(&data).unwrap();
    fs::write(data.join(first), b"first").unwrap();
    fs::write(data.join(second), b"second").unwrap();
    if fs::read_dir(&data).unwrap().count() == 1 {
        return true;
    }
    create_snapshot(&probe.0)
        .expect_err("canonical path collision must block snapshot")
        .code
        == "migration_source_path_collision"
}

fn package_with_extra_path(package: &NeutralPackage, path: &str) -> NeutralPackage {
    let mut files = package.files().clone();
    files.insert(path.to_owned(), b"escape".to_vec());
    files.remove("checksums.sha256");
    let checksums = files
        .iter()
        .map(|(path, bytes)| format!("{}  {path}\n", buglogin_storage::blob::checksum(bytes)))
        .collect::<String>();
    files.insert("checksums.sha256".into(), checksums.into_bytes());
    NeutralPackage::from_files(files)
}

#[cfg(unix)]
fn create_directory_link(target: &Path, link: &Path) {
    std::os::unix::fs::symlink(target, link).expect("create source directory symlink");
}

#[cfg(windows)]
fn create_directory_link(target: &Path, link: &Path) {
    let status = std::process::Command::new("cmd")
        .args(["/c", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .status()
        .expect("run junction command");
    assert!(status.success(), "create source directory junction");
}

fn foreign_profile(id: &str) -> Profile {
    Profile {
        id: ProfileId::new(id).expect("valid canonical profile ID"),
        name: "Foreign destination profile".into(),
        runtime: Runtime::Wayfern,
        runtime_version: Some("foreign-build".into()),
        runtime_config: None,
        state: ProfileState::Ready,
        archived: false,
        fingerprint: None,
        note: None,
        proxy_id: None,
        vpn_id: None,
        group_ids: Vec::new(),
        tags: vec!["foreign".into()],
        extension_group_id: None,
        sync_mode: Some(SyncMode::Local),
        created_at: "2026-07-21T00:00:00Z".into(),
        updated_at: "2026-07-21T00:00:00Z".into(),
    }
}

#[test]
fn mig_027_encrypted_package_requires_matching_key_without_plaintext_residue() {
    let fixture = fixture("MIG-027-encrypted-package");
    let source = fixture.join("source");
    let expected = fixture.join("expected");
    let plaintext_path =
        source.join("profiles/27000000-0000-4000-8000-000000000027/profile/Preferences");
    let plaintext = fs::read(&plaintext_path).expect("read encrypted package fixture payload");
    let before = create_snapshot(&source).expect("snapshot encrypted package fixture before test");
    let (report, snapshot) = inspect_source(&source).expect("inspect encrypted package fixture");
    assert_json_file(&expected.join("inspect.json"), &report);
    assert_json_file(&expected.join("snapshot.json"), &snapshot);

    let key = MigrationKey::from_bytes([27; 32]);
    let package =
        build_encrypted_profile_package(&source, &report, &snapshot, &fixture_metadata(), &key)
            .expect("build authenticated encrypted package");
    verify_package(&package).expect("verify authenticated encrypted package");
    let manifest: NeutralPackageManifest = serde_json::from_slice(
        package
            .file("manifest.json")
            .expect("encrypted package manifest"),
    )
    .expect("parse encrypted package manifest");
    let package_bytes = package
        .files()
        .values()
        .flat_map(|bytes| bytes.iter().copied())
        .collect::<Vec<_>>();
    let plaintext_absent = !package_bytes
        .windows(plaintext.len())
        .any(|window| window == plaintext);
    assert!(plaintext_absent);
    assert_json_file(
        &expected.join("export-summary.json"),
        &json!({
            "packageId": manifest.package_id,
            "sourceTreeSha256": snapshot.source_tree_sha256,
            "containsSecrets": manifest.security.contains_secrets,
            "encrypted": manifest.security.encrypted,
            "encryption": manifest.security.encryption,
            "plaintextAbsent": plaintext_absent,
        }),
    );

    let destination = TestDirectory::new();
    let destination_root = destination.0.join("destination");
    let wrong_key = MigrationKey::from_bytes([28; 32]);
    let wrong_key_error = plan_package_activation(&destination_root, &package, &wrong_key)
        .expect_err("wrong key must fail before destination mutation");
    assert_eq!(wrong_key_error.code, "chunk_authentication_failed");
    assert!(!destination_root.exists());

    let plan = plan_package_activation(&destination_root, &package, &key)
        .expect("matching key plans encrypted package import");
    let receipt = activate_package(
        &destination_root,
        "mig-027-encrypted-package",
        &package,
        Some(&key),
    )
    .expect("matching key activates encrypted package");
    let restored = fs::read(
        destination_root
            .join("profiles")
            .join(&plan.create_profile_ids[0])
            .join("profile/Preferences"),
    )
    .expect("read restored encrypted package payload");
    assert_eq!(restored, plaintext);
    assert_json_lines_file(
        &expected.join("import-results.jsonl"),
        &[
            json!({
                "status": "blocked",
                "case": "wrong_key",
                "code": wrong_key_error.code,
                "destinationChanged": false,
            }),
            json!({
                "status": "activated",
                "createdProfiles": receipt.created_profiles,
                "restoredSha256": buglogin_storage::blob::checksum(&restored),
                "sourceUnchanged": before == create_snapshot(&source).unwrap(),
            }),
        ],
    );
    assert_eq!(before, create_snapshot(&source).unwrap());
}

#[test]
fn mig_008_runtime_aliases_remain_preserved_until_artifact_approval() {
    let fixture = fixture("MIG-008-runtime-aliases");
    let source = fixture.join("source");
    let expected = fixture.join("expected");
    let before = create_snapshot(&source).expect("snapshot runtime alias fixture before test");
    let (report, snapshot) = inspect_source(&source).expect("inspect runtime alias fixture");
    assert_json_file(&expected.join("inspect.json"), &report);
    assert_json_file(&expected.join("snapshot.json"), &snapshot);
    assert_eq!(report.runtime_findings.len(), 6);
    assert!(report.runtime_findings.iter().all(|finding| {
        finding.class == buglogin_migration::inspect::MigrationClass::Unsupported
    }));

    let key = MigrationKey::from_bytes([8; 32]);
    let redacted = build_neutral_package(&report, &snapshot, &fixture_metadata())
        .expect("build runtime alias redacted package");
    let redacted_verified =
        verify_package(&redacted).expect("verify runtime alias redacted package");
    let redacted_manifest: NeutralPackageManifest = serde_json::from_slice(
        redacted
            .file("manifest.json")
            .expect("runtime alias manifest"),
    )
    .expect("parse runtime alias manifest");
    assert_json_file(
        &expected.join("export-summary.json"),
        &json!({
            "packageId": redacted_manifest.package_id,
            "files": redacted.files().len(),
            "sourceTreeSha256": snapshot.source_tree_sha256,
            "checksumsSha256": redacted_verified.checksums_sha256,
            "containsSecrets": redacted_manifest.security.contains_secrets,
        }),
    );

    let package =
        build_encrypted_profile_package(&source, &report, &snapshot, &fixture_metadata(), &key)
            .expect("build runtime alias encrypted package");
    let manifest: NeutralPackageManifest = serde_json::from_slice(
        package
            .file("manifest.json")
            .expect("runtime alias encrypted manifest"),
    )
    .expect("parse runtime alias encrypted manifest");
    assert!(
        manifest
            .compatibility
            .required_capabilities
            .iter()
            .any(|capability| capability == "migration.preserve-only")
    );

    let destination = TestDirectory::new();
    let destination_root = destination.0.join("destination");
    let plan = plan_package_activation(&destination_root, &package, &key)
        .expect("plan runtime alias preservation");
    assert!(plan.create_profile_ids.is_empty());
    assert_eq!(plan.create_preserved_profile_ids.len(), 6);
    let run_id = "mig-008-runtime-aliases";
    let first = activate_package(&destination_root, run_id, &package, Some(&key))
        .expect("activate runtime alias preservation");
    let repeated = activate_package(&destination_root, run_id, &package, Some(&key))
        .expect("repeat runtime alias preservation");
    assert_eq!(first.created_profiles, 0);
    assert_eq!(first.created_preserved_profiles, 6);
    assert_eq!(repeated.created_preserved_profiles, 0);
    assert_eq!(repeated.reused_preserved_profiles, 6);

    let repository =
        LocalRepository::open(&destination_root).expect("open runtime alias repository");
    assert!(repository.list::<Profile>().unwrap().is_empty());
    let preserved_profiles = destination_root.join("migration-preserved/profiles");
    let mut preserved_runtimes = plan
        .create_preserved_profile_ids
        .iter()
        .map(|profile_id| {
            let record: Value = serde_json::from_slice(
                &fs::read(preserved_profiles.join(profile_id).join("record.json")).unwrap(),
            )
            .unwrap();
            record["runtime"].as_str().unwrap().to_owned()
        })
        .collect::<Vec<_>>();
    preserved_runtimes.sort();
    assert_eq!(
        preserved_runtimes,
        ["bugium", "bugmini", "bugox", "cloak", "helium", "thorium"]
    );
    let reconciliation = reconcile_activation(&report, &destination_root, run_id)
        .expect("reconcile runtime alias preservation");
    assert_eq!(reconciliation.status, ReconciliationStatus::Passed);
    assert_eq!(reconciliation.expected_profiles, 0);
    assert_eq!(reconciliation.expected_preserved_profiles, 6);
    assert_eq!(reconciliation.preserved_profiles, 6);

    let rollback = rollback_activation(
        &destination_root,
        run_id,
        confirm_rollback("ROLLBACK_MIGRATION_RUN").unwrap(),
    )
    .expect("rollback runtime alias preservation");
    assert!(rollback.quarantined_profiles.is_empty());
    assert_eq!(rollback.quarantined_preserved_profile_ids.len(), 6);
    assert_json_lines_file(
        &expected.join("import-results.jsonl"),
        &[json!({
            "status": "completed",
            "createdProfiles": first.created_profiles,
            "createdPreservedProfiles": first.created_preserved_profiles,
            "reusedPreservedProfiles": repeated.reused_preserved_profiles,
            "launchBlocked": true,
            "reconciledPreservedProfiles": reconciliation.preserved_profiles,
            "quarantinedPreservedProfiles": rollback.quarantined_preserved_profile_ids.len(),
        })],
    );
    assert_eq!(
        before,
        create_snapshot(&source).expect("snapshot runtime alias fixture after test")
    );
}

#[test]
fn mig_007_camoufox_is_preserved_but_never_activated() {
    const CANARY: &str = "MIG-007-CAMOUFOX-OPAQUE-BYTES";
    let fixture = fixture("MIG-007-camoufox-unsupported");
    let source = fixture.join("source");
    let expected = fixture.join("expected");
    let source_profile =
        source.join("profiles/77777777-7777-4777-8777-777777777777/profile/storage.bin");
    let expected_profile_bytes = fs::read(&source_profile).expect("read Camoufox fixture data");
    let before = create_snapshot(&source).expect("snapshot Camoufox fixture before test");
    let (report, snapshot) = inspect_source(&source).expect("inspect Camoufox fixture");
    assert_json_file(&expected.join("inspect.json"), &report);
    assert_json_file(&expected.join("snapshot.json"), &snapshot);
    assert_eq!(
        report.cutover_eligibility,
        buglogin_migration::inspect::CutoverEligibility::Blocked
    );

    let redacted = build_neutral_package(&report, &snapshot, &fixture_metadata())
        .expect("build Camoufox redacted package");
    let redacted_verified = verify_package(&redacted).expect("verify Camoufox redacted package");
    let redacted_manifest: NeutralPackageManifest = serde_json::from_slice(
        redacted
            .file("manifest.json")
            .expect("Camoufox redacted manifest"),
    )
    .expect("parse Camoufox redacted manifest");
    assert_json_file(
        &expected.join("export-summary.json"),
        &json!({
            "packageId": redacted_manifest.package_id,
            "files": redacted.files().len(),
            "sourceTreeSha256": snapshot.source_tree_sha256,
            "checksumsSha256": redacted_verified.checksums_sha256,
            "containsSecrets": redacted_manifest.security.contains_secrets,
            "preserveOnly": true,
        }),
    );

    let key = MigrationKey::from_bytes([7; 32]);
    let package =
        build_encrypted_profile_package(&source, &report, &snapshot, &fixture_metadata(), &key)
            .expect("build Camoufox preservation package");
    verify_package(&package).expect("verify Camoufox preservation package");
    let manifest: NeutralPackageManifest =
        serde_json::from_slice(package.file("manifest.json").expect("Camoufox manifest"))
            .expect("parse Camoufox manifest");
    assert!(
        manifest
            .compatibility
            .required_capabilities
            .iter()
            .any(|capability| capability == "migration.preserve-only")
    );
    let package_bytes = package
        .files()
        .values()
        .flat_map(|bytes| bytes.iter().copied())
        .collect::<Vec<_>>();
    assert!(!String::from_utf8_lossy(&package_bytes).contains(CANARY));

    let destination = TestDirectory::new();
    let destination_root = destination.0.join("destination");
    let plan = plan_package_activation(&destination_root, &package, &key)
        .expect("plan Camoufox preservation");
    assert!(plan.create_profile_ids.is_empty());
    assert_eq!(plan.create_preserved_profile_ids.len(), 1);
    assert!(!destination_root.exists());

    let profile_id = plan.create_preserved_profile_ids[0].clone();
    let run_id = "mig-007-camoufox";
    let first = activate_package(&destination_root, run_id, &package, Some(&key))
        .expect("activate Camoufox preservation");
    let repeated = activate_package(&destination_root, run_id, &package, Some(&key))
        .expect("repeat Camoufox preservation");
    assert_eq!(first.created_profiles, 0);
    assert_eq!(first.created_preserved_profiles, 1);
    assert_eq!(repeated.created_preserved_profiles, 0);
    assert_eq!(repeated.reused_preserved_profiles, 1);

    let repository = LocalRepository::open(&destination_root).expect("open destination repository");
    assert!(repository.get::<Profile>(&profile_id).unwrap().is_none());
    let preserved_root = destination_root
        .join("migration-preserved/profiles")
        .join(&profile_id);
    assert_eq!(
        fs::read(preserved_root.join("profile/storage.bin")).unwrap(),
        expected_profile_bytes
    );
    let record = fs::read_to_string(preserved_root.join("record.json")).unwrap();
    assert!(record.contains("camoufox"));
    assert!(!record.contains(CANARY));

    let reconciliation = reconcile_activation(&report, &destination_root, run_id)
        .expect("reconcile Camoufox preservation");
    assert_eq!(reconciliation.status, ReconciliationStatus::Passed);
    assert_eq!(reconciliation.expected_profiles, 0);
    assert_eq!(reconciliation.activated_profiles, 0);
    assert_eq!(reconciliation.expected_preserved_profiles, 1);
    assert_eq!(reconciliation.preserved_profiles, 1);
    assert!(reconciliation.mismatched_preserved_profile_ids.is_empty());
    let preserved_record_path = preserved_root.join("record.json");
    let preserved_record = fs::read(&preserved_record_path).unwrap();
    fs::write(&preserved_record_path, b"changed").unwrap();
    let drift = reconcile_activation(&report, &destination_root, run_id)
        .expect("reconcile Camoufox preservation drift");
    assert_eq!(drift.status, ReconciliationStatus::Blocked);
    assert_eq!(
        drift.mismatched_preserved_profile_ids,
        vec![profile_id.clone()]
    );
    fs::write(&preserved_record_path, preserved_record).unwrap();

    let rollback = rollback_activation(
        &destination_root,
        run_id,
        confirm_rollback("ROLLBACK_MIGRATION_RUN").unwrap(),
    )
    .expect("rollback Camoufox preservation");
    assert!(rollback.quarantined_profiles.is_empty());
    assert_eq!(
        rollback.quarantined_preserved_profile_ids,
        vec![profile_id.clone()]
    );
    assert!(!preserved_root.exists());
    let quarantine = destination_root
        .join("migration-preserved/quarantine")
        .join(run_id)
        .join(&profile_id);
    assert_eq!(
        fs::read(quarantine.join("profile/storage.bin")).unwrap(),
        expected_profile_bytes
    );
    let repeated_rollback = rollback_activation(
        &destination_root,
        run_id,
        confirm_rollback("ROLLBACK_MIGRATION_RUN").unwrap(),
    )
    .expect("repeat Camoufox rollback");
    assert!(
        repeated_rollback
            .quarantined_preserved_profile_ids
            .is_empty()
    );

    assert_json_lines_file(
        &expected.join("import-results.jsonl"),
        &[
            json!({
                "status": "planned",
                "createProfiles": plan.create_profile_ids.len(),
                "createPreservedProfiles": plan.create_preserved_profile_ids.len(),
                "destinationChanged": false,
            }),
            json!({
                "status": "activated",
                "createdProfiles": first.created_profiles,
                "createdPreservedProfiles": first.created_preserved_profiles,
                "launchBlocked": true,
                "bytesPreserved": true,
            }),
            json!({
                "status": "repeated",
                "reusedPreservedProfiles": repeated.reused_preserved_profiles,
            }),
            json!({
                "status": "reconciled",
                "result": "passed",
                "activeProfiles": reconciliation.activated_profiles,
                "preservedProfiles": reconciliation.preserved_profiles,
            }),
            json!({
                "status": "rolledBack",
                "quarantinedProfiles": rollback.quarantined_profiles.len(),
                "quarantinedPreservedProfiles": rollback.quarantined_preserved_profile_ids.len(),
                "bytesPreserved": true,
            }),
        ],
    );
    assert_eq!(
        before,
        create_snapshot(&source).expect("snapshot Camoufox fixture after test")
    );
}

#[test]
fn safety_fixture_goldens_match_and_sources_remain_unchanged() {
    for fixture_id in ["MIG-026-unknown-version", "MIG-014-corrupt-metadata"] {
        let fixture = fixture(fixture_id);
        let source = fixture.join("source");
        let expected = fixture.join("expected");
        let before = create_snapshot(&source).expect("snapshot safety fixture before test");
        let (report, snapshot) = inspect_source(&source).expect("inspect safety fixture");
        assert_json_file(&expected.join("inspect.json"), &report);
        assert_json_file(&expected.join("snapshot.json"), &snapshot);
        let error = build_encrypted_profile_package(
            &source,
            &report,
            &snapshot,
            &fixture_metadata(),
            &MigrationKey::from_bytes([9; 32]),
        )
        .expect_err("blocked safety fixture must not package");
        assert_json_lines_file(
            &expected.join("import-results.jsonl"),
            &[json!({
                "status": "blocked",
                "code": error.code,
                "sourceUnchanged": true,
            })],
        );
        assert_eq!(
            before,
            create_snapshot(&source).expect("snapshot safety fixture after test")
        );
    }
}

#[test]
fn secret_canary_fixture_is_redacted_from_export_evidence() {
    let fixture = fixture("MIG-017-secret-canary");
    let source = fixture.join("source");
    let expected = fixture.join("expected");
    let before = create_snapshot(&source).expect("snapshot secret fixture before test");
    let (report, snapshot) = inspect_source(&source).expect("inspect secret fixture");
    assert_json_file(&expected.join("inspect.json"), &report);
    let package = build_neutral_package(&report, &snapshot, &fixture_metadata())
        .expect("redacted secret fixture export");
    let verified = verify_package(&package).expect("verify redacted secret fixture export");
    let manifest: NeutralPackageManifest = serde_json::from_slice(
        package
            .file("manifest.json")
            .expect("secret fixture manifest"),
    )
    .expect("parse secret fixture manifest");
    let package_bytes = package
        .files()
        .values()
        .flat_map(|bytes| bytes.iter().copied())
        .collect::<Vec<_>>();
    assert!(!String::from_utf8_lossy(&package_bytes).contains("MIG-017-SECRET-MUST-NEVER-LEAK"));
    assert_json_file(
        &expected.join("export-summary.json"),
        &json!({
            "packageId": manifest.package_id,
            "files": package.files().len(),
            "sourceTreeSha256": snapshot.source_tree_sha256,
            "checksumsSha256": verified.checksums_sha256,
            "containsSecrets": manifest.security.contains_secrets,
        }),
    );
    assert_json_lines_file(
        &expected.join("import-results.jsonl"),
        &[json!({
            "status": "completed",
            "secretCanaryAbsent": true,
            "sourceUnchanged": true,
        })],
    );
    assert_eq!(
        before,
        create_snapshot(&source).expect("snapshot secret fixture after test")
    );
}

fn fixture_metadata() -> PackageMetadata {
    PackageMetadata {
        created_at: "2026-07-22T00:00:00Z".to_owned(),
        tool_version: "3.0.0".to_owned(),
        tool_commit: "0000000000000000000000000000000000000000".to_owned(),
    }
}

fn fixture(id: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("fixtures/migration")
        .join(id)
}

fn assert_json_file(path: &Path, actual: &impl serde::Serialize) {
    let expected: Value = serde_json::from_slice(&fs::read(path).expect("read golden JSON"))
        .expect("parse golden JSON");
    assert_eq!(
        serde_json::to_value(actual).expect("serialize actual JSON"),
        expected
    );
}

fn assert_json_lines_file(path: &Path, actual: &[Value]) {
    let expected = fs::read_to_string(path)
        .expect("read golden JSONL")
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("parse golden JSONL"))
        .collect::<Vec<_>>();
    assert_eq!(actual, expected);
}
