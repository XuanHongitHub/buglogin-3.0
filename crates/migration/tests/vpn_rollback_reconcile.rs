use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use buglogin_domain::{
    entities::{Profile, ProfileState, Runtime, Vpn, VpnKind},
    ids::{CredentialId, ProfileId, VpnId},
};
use buglogin_migration::{
    activate::{
        ActivationJournal, ActivationJournalEntry, ActivationStatus, EntityActivationJournalEntry,
        VpnConfigActivationJournalEntry,
    },
    encrypted_vpn_configs::VpnConfigSecret,
    inspect::{CutoverEligibility, InspectReport, SecretScan, SourceIdentity},
    reconcile::{ReconciliationStatus, reconcile_activation_with_vault},
    rollback::{confirm_rollback, rollback_activation_with_vault},
};
use buglogin_storage::{
    local::{LocalEntity, LocalRepository},
    secrets::{OsVault, RotationReceipt, SecretError, SecretKey, SecretValue, SecretVersion},
};
use serde::Serialize;

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct Temp(PathBuf);

impl Temp {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "buglogin-vpn-rollback-{label}-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(Default)]
struct FakeVault {
    values: BTreeMap<(String, String), (SecretValue, SecretVersion)>,
}

impl FakeVault {
    fn insert(&mut self, config: &VpnConfigSecret) -> VpnConfigActivationJournalEntry {
        let bytes = serde_json::to_vec(config).unwrap();
        let key = SecretKey::new("vpn-configs", &config.config_ref).unwrap();
        let version = self
            .create(&key, SecretValue::new(bytes.clone()).unwrap())
            .unwrap();
        VpnConfigActivationJournalEntry {
            config_ref: config.config_ref.clone(),
            version: version.generation(),
            sha256: buglogin_storage::blob::checksum(&bytes),
            created: true,
        }
    }
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
        let version = SecretVersion::new(1).unwrap();
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
        let next = SecretVersion::new(expected.generation() + 1).unwrap();
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

#[test]
fn rollback_quarantines_owned_vpn_and_removes_only_owned_config() {
    let destination = Temp::new("owned");
    let repository = LocalRepository::open(destination.path()).unwrap();
    let owned_vpn = vpn("vpn-owned", "config-owned");
    let reused_vpn = vpn("vpn-reused", "config-reused");
    let profile = profile("profile-owned", Some(owned_vpn.id.clone()));
    let owned_version = repository.create(owned_vpn.clone()).unwrap();
    let reused_version = repository.create(reused_vpn.clone()).unwrap();
    let profile_version = repository.create(profile.clone()).unwrap();
    let mut vault = FakeVault::default();
    let owned_config = vpn_config(&owned_vpn, "owned-private-config");
    let reused_config = vpn_config(&reused_vpn, "reused-private-config");
    let owned_config_entry = vault.insert(&owned_config);
    let mut reused_config_entry = vault.insert(&reused_config);
    reused_config_entry.created = false;
    write_journal(
        destination.path(),
        "vpn-owned-run",
        vec![owned_config_entry, reused_config_entry],
        vec![
            entity_entry(&owned_vpn, owned_version.revision, true),
            entity_entry(&reused_vpn, reused_version.revision, false),
        ],
        vec![profile_entry(&profile, profile_version.revision, true)],
    );

    let receipt = rollback_activation_with_vault(
        destination.path(),
        "vpn-owned-run",
        confirm_rollback("ROLLBACK_MIGRATION_RUN").unwrap(),
        &mut vault,
    )
    .unwrap();

    assert_eq!(receipt.quarantined_profiles.len(), 1);
    assert_eq!(receipt.quarantined_vpns.len(), 1);
    assert_eq!(receipt.removed_vpn_config_ids, vec!["config-owned"]);
    assert!(
        repository
            .get::<Profile>("profile-owned")
            .unwrap()
            .is_none()
    );
    assert!(repository.get::<Vpn>("vpn-owned").unwrap().is_none());
    assert!(repository.get::<Vpn>("vpn-reused").unwrap().is_some());
    assert!(
        !vault
            .values
            .contains_key(&("vpn-configs".into(), "config-owned".into()))
    );
    assert!(
        vault
            .values
            .contains_key(&("vpn-configs".into(), "config-reused".into()))
    );
    let journal = fs::read_to_string(
        destination
            .path()
            .join("migration-activations/vpn-owned-run.json"),
    )
    .unwrap();
    assert!(journal.contains("\"status\": \"rolled_back\""));
    assert!(journal.contains("\"vpnConfigs\""));
    assert!(journal.contains("\"vpns\""));
}

#[test]
fn rollback_refuses_vpn_config_drift_before_mutation() {
    let destination = Temp::new("drift");
    let repository = LocalRepository::open(destination.path()).unwrap();
    let vpn = vpn("vpn-drift", "config-drift");
    let profile = profile("profile-drift", Some(vpn.id.clone()));
    let vpn_version = repository.create(vpn.clone()).unwrap();
    let profile_version = repository.create(profile.clone()).unwrap();
    let mut vault = FakeVault::default();
    let config = vpn_config(&vpn, "original-private-config");
    let config_entry = vault.insert(&config);
    write_journal(
        destination.path(),
        "vpn-drift-run",
        vec![config_entry],
        vec![entity_entry(&vpn, vpn_version.revision, true)],
        vec![profile_entry(&profile, profile_version.revision, true)],
    );
    vault.values.insert(
        ("vpn-configs".into(), "config-drift".into()),
        (
            SecretValue::new(b"tampered".to_vec()).unwrap(),
            SecretVersion::new(1).unwrap(),
        ),
    );

    let error = rollback_activation_with_vault(
        destination.path(),
        "vpn-drift-run",
        confirm_rollback("ROLLBACK_MIGRATION_RUN").unwrap(),
        &mut vault,
    )
    .unwrap_err();

    assert_eq!(error.code, "migration_activation_entity_changed");
    assert!(
        repository
            .get::<Profile>("profile-drift")
            .unwrap()
            .is_some()
    );
    assert!(repository.get::<Vpn>("vpn-drift").unwrap().is_some());
}

#[test]
fn reconciliation_proves_vpn_vault_entity_and_profile_reference() {
    let destination = Temp::new("reconcile");
    let repository = LocalRepository::open(destination.path()).unwrap();
    let vpn = vpn("vpn-reconcile", "config-reconcile");
    let profile = profile("profile-reconcile", Some(vpn.id.clone()));
    let vpn_version = repository.create(vpn.clone()).unwrap();
    let profile_version = repository.create(profile.clone()).unwrap();
    let mut vault = FakeVault::default();
    let config_entry = vault.insert(&vpn_config(&vpn, "reconcile-private-config"));
    write_journal(
        destination.path(),
        "vpn-reconcile-run",
        vec![config_entry],
        vec![entity_entry(&vpn, vpn_version.revision, true)],
        vec![profile_entry(&profile, profile_version.revision, true)],
    );
    let report = inspect_report();

    let passed =
        reconcile_activation_with_vault(&report, destination.path(), "vpn-reconcile-run", &vault)
            .unwrap();
    assert_eq!(passed.status, ReconciliationStatus::Passed);
    assert_eq!(passed.activated_vpn_configs, 1);
    assert_eq!(passed.activated_vpns, 1);

    repository
        .quarantine::<Vpn>(vpn_version.revision, vpn.id.as_str())
        .unwrap();
    let blocked =
        reconcile_activation_with_vault(&report, destination.path(), "vpn-reconcile-run", &vault)
            .unwrap();
    assert_eq!(blocked.status, ReconciliationStatus::Blocked);
    assert_eq!(blocked.mismatched_vpn_ids, vec!["vpn-reconcile"]);
    assert_eq!(
        blocked.profile_vpn_reference_mismatches,
        vec!["profile-reconcile"]
    );
    assert_eq!(blocked.mismatched_vpn_config_ids, vec!["config-reconcile"]);
}

fn vpn(id: &str, config_ref: &str) -> Vpn {
    Vpn {
        id: VpnId::new(id).unwrap(),
        name: id.into(),
        kind: VpnKind::Wireguard,
        config_ref: CredentialId::new(config_ref).unwrap(),
        last_used_at: None,
        sync_mode: None,
        created_at: "2026-07-21T00:00:00Z".into(),
        updated_at: "2026-07-21T00:00:00Z".into(),
    }
}

fn profile(id: &str, vpn_id: Option<VpnId>) -> Profile {
    Profile {
        id: ProfileId::new(id).unwrap(),
        name: id.into(),
        runtime: Runtime::Wayfern,
        runtime_version: None,
        runtime_config: None,
        state: ProfileState::Created,
        archived: false,
        fingerprint: None,
        note: None,
        proxy_id: None,
        vpn_id,
        group_ids: Vec::new(),
        tags: Vec::new(),
        extension_group_id: None,
        sync_mode: None,
        created_at: "2026-07-21T00:00:00Z".into(),
        updated_at: "2026-07-21T00:00:00Z".into(),
    }
}

fn vpn_config(vpn: &Vpn, config_data: &str) -> VpnConfigSecret {
    VpnConfigSecret {
        schema: "buglogin.vpn-config-secret/1".into(),
        config_ref: vpn.config_ref.as_str().into(),
        vpn_id: vpn.id.as_str().into(),
        kind: vpn.kind.clone(),
        config_sha256: buglogin_storage::blob::checksum(config_data.as_bytes()),
        config_data: config_data.into(),
    }
}

fn entity_entry<T: LocalEntity + Serialize>(
    entity: &T,
    revision: u64,
    created: bool,
) -> EntityActivationJournalEntry {
    EntityActivationJournalEntry {
        entity_id: entity.entity_id().into(),
        revision,
        sha256: buglogin_storage::blob::checksum(&serde_json::to_vec(entity).unwrap()),
        created,
    }
}

fn profile_entry(profile: &Profile, revision: u64, created: bool) -> ActivationJournalEntry {
    ActivationJournalEntry {
        profile_id: profile.id.as_str().into(),
        revision,
        sha256: buglogin_storage::blob::checksum(&serde_json::to_vec(profile).unwrap()),
        created,
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ActivationJournalV3 {
    #[serde(flatten)]
    journal: ActivationJournal,
    vpn_configs: Vec<VpnConfigActivationJournalEntry>,
    vpns: Vec<EntityActivationJournalEntry>,
}

fn write_journal(
    root: &Path,
    run_id: &str,
    vpn_configs: Vec<VpnConfigActivationJournalEntry>,
    vpns: Vec<EntityActivationJournalEntry>,
    profiles: Vec<ActivationJournalEntry>,
) {
    let directory = root.join("migration-activations");
    fs::create_dir_all(&directory).unwrap();
    let journal = ActivationJournalV3 {
        journal: ActivationJournal {
            schema: "buglogin.activation/1".into(),
            run_id: run_id.into(),
            status: ActivationStatus::Active,
            input_sha256: "test-input".into(),
            credentials: Vec::new(),
            extension_artifacts: Vec::new(),
            extensions: Vec::new(),
            extension_groups: Vec::new(),
            groups: Vec::new(),
            proxies: Vec::new(),
            profiles,
        },
        vpn_configs,
        vpns,
    };
    fs::write(
        directory.join(format!("{run_id}.json")),
        serde_json::to_vec_pretty(&journal).unwrap(),
    )
    .unwrap();
}

fn inspect_report() -> InspectReport {
    InspectReport {
        schema: "buglogin.inspect/1".into(),
        source: SourceIdentity {
            system: "buglogin-legacy".into(),
            version: "1.1.84".into(),
            instance_id: "sha256:vpn-test".into(),
            os: "windows".into(),
            architecture: "x86_64".into(),
            root: ".".into(),
        },
        entity_counts: BTreeMap::from([("profiles".into(), 1), ("vpns".into(), 1)]),
        total_bytes: 0,
        runtime_findings: Vec::new(),
        secret_scan: SecretScan::default(),
        invalid_records: Vec::new(),
        warnings: Vec::new(),
        dangling_references: Vec::new(),
        source_tree_sha256: "test-source".into(),
        cutover_eligibility: CutoverEligibility::Eligible,
    }
}
