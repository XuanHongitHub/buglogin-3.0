use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use buglogin_domain::entities::Profile;
use buglogin_migration::{
    MigrationKey,
    activate::ActivationReceipt,
    cutover::{confirm_cutover, promote_staged},
    inspect::inspect_source,
    legacy::canonicalize_legacy_control_plane_json,
    package::{PackageMetadata, build_encrypted_profile_package},
    plan_package_activation_with_vault,
    reconcile::{ReconciliationStatus, reconcile_activation_with_vault},
    rollback::{confirm_rollback, rollback_activation_with_vault, rollback_cutover},
    signature::{SignatureMetadata, sign_package, verify_signed_package},
    snapshot::create_snapshot,
    transform::stage_package,
    verify::verify_package,
};
use buglogin_storage::local::LocalRepository;
use buglogin_storage::secrets::{
    OsVault, RotationReceipt, SecretError, SecretKey, SecretValue, SecretVersion,
};
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
        let map_key = (key.namespace().into(), key.name().into());
        if self.values.contains_key(&map_key) {
            return Err(SecretError::VersionConflict);
        }
        let version = SecretVersion::new(1).expect("valid initial secret version");
        self.values.insert(map_key, (value, version));
        Ok(version)
    }

    fn rotate(
        &mut self,
        key: &SecretKey,
        expected: SecretVersion,
        replacement: SecretValue,
    ) -> Result<RotationReceipt, SecretError> {
        let current = self
            .values
            .get_mut(&(key.namespace().into(), key.name().into()))
            .ok_or(SecretError::NotFound)?;
        if current.1 != expected {
            return Err(SecretError::VersionConflict);
        }
        let next =
            SecretVersion::new(expected.generation() + 1).expect("valid rotated secret version");
        *current = (replacement, next);
        Ok(RotationReceipt {
            previous: expected,
            current: next,
        })
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

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new(label: &str) -> Self {
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "buglogin-migration-rehearsal-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create synthetic rehearsal directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn synthetic_rehearsal_runs_package_to_cutover_activation_reconcile_and_rollback() {
    let source = TempDirectory::new("source");
    fs::write(
        source.path().join("version.json"),
        br#"{"system":"buglogin-legacy","version":"1.1.84","instanceId":"rehearsal-fixture","os":"windows","architecture":"x86_64"}"#,
    )
    .expect("write synthetic source manifest");
    let profile_id = "11111111-1111-4111-8111-111111111111";
    fs::create_dir_all(source.path().join(format!("profiles/{profile_id}/profile")))
        .expect("create synthetic profile");
    fs::write(
        source
            .path()
            .join(format!("profiles/{profile_id}/metadata.json")),
        format!(r#"{{"id":"{profile_id}","name":"One","runtime":"wayfern"}}"#),
    )
    .expect("write synthetic profile metadata");
    fs::write(
        source
            .path()
            .join(format!("profiles/{profile_id}/profile/Preferences")),
        b"synthetic-profile-data",
    )
    .expect("write synthetic profile data");

    let source_before = create_snapshot(source.path()).expect("snapshot source before rehearsal");
    let (report, snapshot) = inspect_source(source.path()).expect("inspect synthetic source");
    let key = MigrationKey::from_bytes([61; 32]);
    let package = build_encrypted_profile_package(
        source.path(),
        &report,
        &snapshot,
        &PackageMetadata {
            created_at: "2026-08-21T00:00:00Z".into(),
            tool_version: "synthetic-rehearsal".into(),
            tool_commit: "0000000000000000000000000000000000000000".into(),
        },
        &key,
    )
    .expect("build synthetic encrypted package");
    let signed = sign_package(
        &package,
        &[71; 32],
        SignatureMetadata {
            key_id: "synthetic-rehearsal-key".into(),
            signer: "migration-rehearsal".into(),
            signed_at: "2026-08-21T00:01:00Z".into(),
        },
    )
    .expect("sign synthetic package");
    verify_signed_package(&signed, &[71; 32]).expect("verify synthetic signed package");
    verify_package(&signed).expect("verify synthetic package checksums");

    let destination = TempDirectory::new("destination");
    fs::write(destination.path().join("foreign.txt"), b"preserve me")
        .expect("write foreign destination artifact");
    let destination_before =
        create_snapshot(destination.path()).expect("snapshot destination before rehearsal");
    let staging = TempDirectory::new("staging");
    let run_id = "rehearsal-001";
    let mut vault = FakeVault::default();

    let plan = plan_package_activation_with_vault(destination.path(), &signed, &key, &vault)
        .expect("plan synthetic import");
    assert_eq!(plan.create_profile_ids.len(), 1);
    assert_eq!(
        fs::read(destination.path().join("foreign.txt")).expect("read foreign artifact"),
        b"preserve me"
    );

    let staged = stage_package(&signed, staging.path(), run_id).expect("stage synthetic package");
    assert!(staged.staging_root.is_dir());
    assert_eq!(
        confirm_cutover("WRONG_CONFIRMATION").unwrap_err().code,
        "migration_cutover_not_confirmed"
    );
    assert_eq!(
        confirm_rollback("WRONG_CONFIRMATION").unwrap_err().code,
        "migration_rollback_not_confirmed"
    );
    let cutover = promote_staged(
        &staged,
        destination.path(),
        confirm_cutover("CUTOVER_AND_PROMOTE").expect("confirm synthetic cutover"),
    )
    .expect("promote synthetic package");
    assert!(cutover.run_root.join("manifest.json").is_file());

    let activation: ActivationReceipt = buglogin_migration::activate_package_with_vault(
        destination.path(),
        run_id,
        &signed,
        &key,
        &mut vault,
    )
    .expect("activate synthetic package");
    assert_eq!(activation.created_profiles, 1);
    let repeated_activation = buglogin_migration::activate_package_with_vault(
        destination.path(),
        run_id,
        &signed,
        &key,
        &mut vault,
    )
    .expect("repeat synthetic activation");
    assert_eq!(repeated_activation.created_profiles, 0);
    assert_eq!(repeated_activation.reused_profiles, 1);

    let reconciliation =
        reconcile_activation_with_vault(&report, destination.path(), run_id, &vault)
            .expect("reconcile synthetic activation");
    assert_eq!(reconciliation.status, ReconciliationStatus::Passed);
    assert_eq!(reconciliation.expected_profiles, 1);
    assert_eq!(reconciliation.activated_profiles, 1);

    let evidence = json!({
        "schema": "buglogin.synthetic-rehearsal-evidence/1",
        "evidenceClass": "synthetic-only",
        "productionReadiness": "not-proven",
        "sourceAccess": "sanitized-fixture",
        "runId": run_id,
        "packageId": cutover.package_id,
        "sourceTreeSha256": source_before.source_tree_sha256.clone(),
        "destinationPreStateSha256": destination_before.source_tree_sha256.clone(),
        "expectedProfiles": reconciliation.expected_profiles,
        "activatedProfiles": reconciliation.activated_profiles,
        "reconciliation": "passed",
        "sourceMutation": "not-observed",
        "hostedTransactionality": "not-tested",
        "rollback": "passed",
        "redaction": "passed",
        "sourcePath": "<redacted>",
        "secrets": "absent"
    });
    let evidence_root = TempDirectory::new("evidence");
    let evidence_path = evidence_root
        .path()
        .join("synthetic-rehearsal-evidence.json");
    fs::write(
        &evidence_path,
        serde_json::to_vec_pretty(&evidence).expect("serialize rehearsal evidence"),
    )
    .expect("write rehearsal evidence");
    let evidence_text = fs::read_to_string(&evidence_path).expect("read rehearsal evidence");
    assert!(!evidence_text.contains(source.path().to_string_lossy().as_ref()));
    assert!(!evidence_text.contains("synthetic-profile-data"));
    assert_eq!(
        serde_json::from_str::<Value>(&evidence_text).expect("parse rehearsal evidence")["reconciliation"],
        "passed"
    );
    let evidence_value: Value =
        serde_json::from_str(&evidence_text).expect("parse evidence fields");
    for (field, expected) in [
        ("evidenceClass", "synthetic-only"),
        ("productionReadiness", "not-proven"),
        ("sourceAccess", "sanitized-fixture"),
        ("sourceMutation", "not-observed"),
        ("hostedTransactionality", "not-tested"),
        ("rollback", "passed"),
        ("redaction", "passed"),
        ("secrets", "absent"),
    ] {
        assert_eq!(
            evidence_value[field], expected,
            "missing evidence guard: {field}"
        );
    }
    assert_ne!(evidence_value["productionReadiness"], "ready");

    let rollback_confirmation =
        confirm_rollback("ROLLBACK_MIGRATION_RUN").expect("confirm synthetic rollback");
    let activation_rollback = rollback_activation_with_vault(
        destination.path(),
        run_id,
        rollback_confirmation,
        &mut vault,
    )
    .expect("rollback synthetic activation");
    assert_eq!(activation_rollback.quarantined_profiles.len(), 1);
    let repeated_activation_rollback = rollback_activation_with_vault(
        destination.path(),
        run_id,
        confirm_rollback("ROLLBACK_MIGRATION_RUN").expect("confirm repeated synthetic rollback"),
        &mut vault,
    )
    .expect("repeat synthetic activation rollback");
    assert!(repeated_activation_rollback.quarantined_profiles.is_empty());
    let repository =
        LocalRepository::open(destination.path()).expect("open rolled back repository");
    assert!(
        repository
            .list::<Profile>()
            .expect("list rolled back profiles")
            .is_empty()
    );

    let cutover_rollback = rollback_cutover(
        destination.path(),
        run_id,
        confirm_rollback("ROLLBACK_MIGRATION_RUN").expect("confirm synthetic cutover rollback"),
    )
    .expect("rollback synthetic cutover");
    assert!(cutover_rollback.removed_artifacts > 0);
    assert_eq!(
        fs::read(destination.path().join("foreign.txt")).expect("read retained artifact"),
        b"preserve me"
    );
    assert!(!destination.path().join("migration-imports").exists());
    let destination_after =
        create_snapshot(destination.path()).expect("snapshot destination after rehearsal");
    assert!(
        destination_after
            .files
            .iter()
            .any(|entry| entry.path == "foreign.txt")
    );
    assert!(
        destination.path().join("quarantine/profiles").is_dir(),
        "rollback must retain migration-owned quarantine evidence"
    );
    assert_ne!(
        destination_before.source_tree_sha256, destination_after.source_tree_sha256,
        "rollback evidence must remain outside the restored user-visible state"
    );
    assert_eq!(
        source_before,
        create_snapshot(source.path()).expect("snapshot source after rehearsal")
    );
}

#[test]
fn synthetic_control_plane_rehearsal_preserves_acl_boundary_and_dry_run_state() {
    let source = fixture("MIG-018-acl-complete").join("source");
    let source_before = create_snapshot(&source).expect("snapshot ACL source before rehearsal");
    let control_plane = fs::read(source.join("control-plane-export.json"))
        .expect("read synthetic control-plane export");
    let canonical = canonicalize_legacy_control_plane_json(&control_plane, "instance-acl")
        .expect("canonicalize synthetic control-plane export");
    let canonical_value: Value = serde_json::from_slice(&canonical).expect("parse canonical ACL");
    let keys = canonical_value
        .as_object()
        .expect("canonical ACL object")
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(
        keys,
        [
            "memberships",
            "organizations",
            "profiles",
            "shareGrants",
            "workspaces",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>()
    );
    let canonical_text = String::from_utf8(canonical.clone()).expect("canonical ACL UTF-8");
    for forbidden in ["password", "bearer", "cookie", "secret", "token"] {
        assert!(!canonical_text.to_ascii_lowercase().contains(forbidden));
    }

    let (report, snapshot) = inspect_source(&source).expect("inspect ACL source");
    let key = MigrationKey::from_bytes([83; 32]);
    let package = build_encrypted_profile_package(
        &source,
        &report,
        &snapshot,
        &PackageMetadata {
            created_at: "2026-08-21T00:00:00Z".into(),
            tool_version: "synthetic-control-plane-rehearsal".into(),
            tool_commit: "0000000000000000000000000000000000000000".into(),
        },
        &key,
    )
    .expect("build synthetic ACL package");
    let signed = sign_package(
        &package,
        &[97; 32],
        SignatureMetadata {
            key_id: "synthetic-acl-key".into(),
            signer: "migration-rehearsal".into(),
            signed_at: "2026-08-21T00:01:00Z".into(),
        },
    )
    .expect("sign synthetic ACL package");
    verify_signed_package(&signed, &[97; 32]).expect("verify synthetic ACL package");

    for (path, expected_records) in [
        ("entities/accounts.jsonl", 3),
        ("entities/organizations.jsonl", 1),
        ("entities/workspaces.jsonl", 1),
        ("entities/teams.jsonl", 1),
        ("entities/members.jsonl", 3),
        ("entities/resources.jsonl", 1),
        ("entities/profile-resources.jsonl", 1),
        ("entities/grants.jsonl", 2),
    ] {
        let records = signed
            .file(path)
            .unwrap_or_else(|| panic!("missing {path}"));
        assert_eq!(
            records
                .split(|byte| *byte == b'\n')
                .filter(|line| !line.is_empty())
                .count(),
            expected_records,
            "wrong record count for {path}"
        );
    }
    for (path, bytes) in signed.files() {
        if path.starts_with("entities/") {
            continue;
        }
        let text = String::from_utf8_lossy(bytes);
        assert!(!text.contains("owner-acl@example.com"));
        assert!(!text.contains("member-acl@example.com"));
        assert!(!text.contains("viewer-acl@example.com"));
    }

    let destination = TempDirectory::new("control-plane-dry-run");
    let vault = FakeVault::default();
    let plan = plan_package_activation_with_vault(destination.path(), &signed, &key, &vault)
        .expect("plan ACL package without local control-plane mutation");
    assert_eq!(plan.create_profile_ids.len(), 1);
    assert!(!destination.path().join("profiles").exists());
    assert!(!destination.path().join("entities").exists());
    assert!(vault.values.is_empty());

    assert_eq!(
        source_before,
        create_snapshot(&source).expect("snapshot ACL source after rehearsal")
    );
}

fn fixture(id: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("fixtures/migration")
        .join(id)
}
