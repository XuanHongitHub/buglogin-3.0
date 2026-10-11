use std::{
    env, fs,
    path::{Path, PathBuf},
    process,
    sync::atomic::{AtomicU64, Ordering},
};

use buglogin_domain::{
    entities::{Profile, ProfileState, Runtime, SyncMode},
    ids::ProfileId,
};
use buglogin_migration::{
    MigrationKey, activate_package,
    inspect::inspect_source,
    package::{NeutralPackageManifest, PackageMetadata, build_encrypted_profile_package},
    reconcile::{ReconciliationStatus, reconcile_activation},
    rollback::{confirm_rollback, rollback_activation},
    snapshot::create_snapshot,
};
use buglogin_storage::local::LocalRepository;
use serde_json::{Value, json};

const SECRET_CANARY: &str = "rehearsal-secret-canary-never-report";
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new(label: &str) -> Result<Self, String> {
        let path = std::env::temp_dir().join(format!(
            "buglogin-rehearsal-{label}-{}-{}",
            process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).map_err(|error| error.to_string())?;
        Ok(Self(path))
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

fn main() {
    match rehearse() {
        Ok(evidence) => println!("{evidence}"),
        Err(error) => {
            eprintln!("migration rehearsal failed: {error}");
            process::exit(1);
        }
    }
}

fn rehearse() -> Result<Value, String> {
    let generated_at = env::var("BUGLOGIN_EVIDENCE_GENERATED_AT")
        .map_err(|_| "BUGLOGIN_EVIDENCE_GENERATED_AT is required".to_owned())?;
    require(
        generated_at.ends_with('Z') && generated_at.contains('T'),
        "evidence timestamp must be UTC RFC3339",
    )?;
    let source = fixture_source()?;
    let source_before = create_snapshot(source.path()).map_err(problem)?;
    let (report, snapshot) = inspect_source(source.path()).map_err(problem)?;
    let key = MigrationKey::from_bytes([21; 32]);
    let package = build_encrypted_profile_package(
        source.path(),
        &report,
        &snapshot,
        &PackageMetadata {
            created_at: generated_at.clone(),
            tool_version: "0.1.0".to_owned(),
            tool_commit: "rehearsal".to_owned(),
        },
        &key,
    )
    .map_err(problem)?;
    let manifest: NeutralPackageManifest = serde_json::from_slice(
        package
            .file("manifest.json")
            .ok_or_else(|| "manifest missing".to_owned())?,
    )
    .map_err(|error| error.to_string())?;
    let package_bytes = package
        .files()
        .values()
        .flat_map(|bytes| bytes.iter())
        .copied()
        .collect::<Vec<_>>();
    let plaintext_secret_absent = !String::from_utf8_lossy(&package_bytes).contains(SECRET_CANARY);
    require(manifest.security.encrypted, "package is not encrypted")?;
    require(plaintext_secret_absent, "package leaked plaintext secret")?;

    let mut runs = Vec::new();
    for run_id in ["migration-rehearsal-1", "migration-rehearsal-2"] {
        let destination = TempDirectory::new(run_id)?;
        let repository = LocalRepository::open(destination.path()).map_err(problem)?;
        repository.create(foreign_profile()).map_err(problem)?;
        let first =
            activate_package(destination.path(), run_id, &package, Some(&key)).map_err(problem)?;
        let second =
            activate_package(destination.path(), run_id, &package, Some(&key)).map_err(problem)?;
        require(
            first.created_profiles == 1,
            "first activation did not create one profile",
        )?;
        require(
            second.created_profiles == 0,
            "second activation created duplicate data",
        )?;
        require(
            second.reused_profiles == 1,
            "second activation did not reuse profile",
        )?;
        let reconciliation =
            reconcile_activation(&report, destination.path(), run_id).map_err(problem)?;
        require(
            reconciliation.status == ReconciliationStatus::Passed,
            "rehearsal reconciliation did not pass",
        )?;
        let rollback = rollback_activation(
            destination.path(),
            run_id,
            confirm_rollback("ROLLBACK_MIGRATION_RUN").map_err(problem)?,
        )
        .map_err(problem)?;
        require(
            repository
                .get::<Profile>("prf_foreign")
                .map_err(problem)?
                .is_some(),
            "rollback removed foreign profile",
        )?;
        runs.push(json!({
            "run_id": run_id,
            "first_activation_created": first.created_profiles,
            "second_activation_created": second.created_profiles,
            "second_activation_reused": second.reused_profiles,
            "reconciliation": {
                "status": reconciliation.status,
                "expected_profiles": reconciliation.expected_profiles,
                "activated_profiles": reconciliation.activated_profiles,
            },
            "rollback_quarantined": rollback.quarantined_profiles.len(),
            "foreign_data_preserved": true
        }));
    }

    let source_after = create_snapshot(source.path()).map_err(problem)?;
    require(source_before == source_after, "source hash changed")?;
    let evidence = json!({
        "schema": "buglogin.migration-rehearsal-evidence/1",
        "status": "passed",
        "generated_at": generated_at,
        "redaction": {
            "absolute_paths_omitted": true,
            "encryption_key_omitted": true,
            "secret_values_omitted": true
        },
        "package": {
            "encrypted": manifest.security.encrypted,
            "encryption": manifest.security.encryption,
            "plaintext_secret_absent": plaintext_secret_absent
        },
        "source": {
            "hash_before": source_before.source_tree_sha256,
            "hash_after": source_after.source_tree_sha256,
            "unchanged": true
        },
        "rehearsals": runs
    });
    require(
        !evidence.to_string().contains(SECRET_CANARY),
        "evidence leaked secret",
    )?;
    Ok(evidence)
}

fn fixture_source() -> Result<TempDirectory, String> {
    let source = TempDirectory::new("source")?;
    write(
        source.path().join("version.json"),
        br#"{"system":"buglogin-legacy","version":"1.1.84","instanceId":"rehearsal-fixture","os":"windows","architecture":"x86_64"}"#,
    )?;
    let profile = source
        .path()
        .join("profiles/00000000-0000-0000-0000-000000000001");
    write(
        profile.join("metadata.json"),
        br#"{"id":"00000000-0000-0000-0000-000000000001","name":"Rehearsal Profile","runtime":"wayfern"}"#,
    )?;
    write(profile.join("profile/Cookies"), SECRET_CANARY.as_bytes())?;
    write(
        profile.join("profile/Preferences"),
        b"immutable-profile-data",
    )?;
    Ok(source)
}

fn write(path: PathBuf, bytes: &[u8]) -> Result<(), String> {
    fs::create_dir_all(path.parent().ok_or_else(|| "missing parent".to_owned())?)
        .map_err(|error| error.to_string())?;
    fs::write(path, bytes).map_err(|error| error.to_string())
}

fn foreign_profile() -> Profile {
    Profile {
        id: ProfileId::new("prf_foreign").expect("valid foreign ID"),
        name: "Foreign Profile".to_owned(),
        runtime: Runtime::Wayfern,
        runtime_version: None,
        state: ProfileState::Ready,
        archived: false,
        fingerprint: None,
        proxy_id: None,
        vpn_id: None,
        group_ids: vec![],
        tags: vec!["foreign".to_owned()],
        extension_group_id: None,
        sync_mode: Some(SyncMode::Local),
        created_at: "2026-07-22T00:00:00Z".to_owned(),
        updated_at: "2026-07-22T00:00:00Z".to_owned(),
    }
}

fn require(condition: bool, message: &str) -> Result<(), String> {
    condition.then_some(()).ok_or_else(|| message.to_owned())
}

fn problem(error: impl std::fmt::Debug) -> String {
    format!("{error:?}")
}
