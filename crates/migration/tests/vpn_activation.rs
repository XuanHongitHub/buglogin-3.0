use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use buglogin_domain::{
    entities::{Profile, Vpn, VpnKind},
    ids::{CredentialId, VpnId, migration_uuid},
};
use buglogin_migration::{
    activate::{activate_records_v3_with_vault, plan_records_v3_with_vault},
    encrypted_vpn_configs::VpnConfigSecret,
    legacy::{NeutralVpnData, NeutralVpnRecord},
    transform::{
        NeutralProfileData, NeutralProfileRecord, ProfileDataClassification,
        ProfileDataDisposition, ProfileDataObject, ProfileSourceRef,
    },
};
use buglogin_storage::{
    local::LocalRepository,
    secrets::{OsVault, RotationReceipt, SecretError, SecretKey, SecretValue, SecretVersion},
};
use serde_json::Map;

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct Temp(PathBuf);

impl Temp {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "buglogin-vpn-activation-{label}-{}-{}",
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
fn vpn_plan_activation_and_rerun_are_vault_first_and_idempotent() {
    let destination = Temp::new("success");
    fs::remove_dir(destination.path()).unwrap();
    let vpn = vpn_record("vpn-a", "VPN A");
    let config = vpn_config(&vpn, "[Interface]\nPrivateKey = secret-private-key\n");
    let profile = profile_record("profile-a", Some(vpn.id.clone()), None);
    let mut vault = FakeVault::default();

    let plan = plan_records_v3_with_vault(
        destination.path(),
        &[profile.clone()],
        &[],
        &[],
        &[],
        &[],
        &[],
        &[],
        &[vpn.clone()],
        &[config.clone()],
        &vault,
    )
    .unwrap();
    assert!(!destination.path().exists());
    assert_eq!(plan.create_vpn_config_ids, vec![config.config_ref.clone()]);
    assert_eq!(plan.create_vpn_ids, vec![vpn.id.clone()]);
    assert_eq!(plan.create_profile_ids, vec![profile.id.clone()]);

    let first = activate_records_v3_with_vault(
        destination.path(),
        "vpn-success",
        &[profile.clone()],
        &[],
        &[],
        &[],
        &[],
        &[],
        &[],
        &[vpn.clone()],
        &[config.clone()],
        &mut vault,
    )
    .unwrap();
    assert_eq!(first.created_vpn_configs, 1);
    assert_eq!(first.created_vpns, 1);
    assert_eq!(first.created_profiles, 1);
    assert!(
        vault
            .values
            .contains_key(&("vpn-configs".into(), config.config_ref.clone()))
    );
    let repository = LocalRepository::open(destination.path()).unwrap();
    assert!(repository.get::<Vpn>(&vpn.id).unwrap().is_some());
    assert!(repository.get::<Profile>(&profile.id).unwrap().is_some());
    let journal = fs::read_to_string(first.journal_path).unwrap();
    assert!(journal.contains("\"vpnConfigs\""));
    assert!(journal.contains("\"vpns\""));
    assert!(!journal.contains("secret-private-key"));

    let rerun = activate_records_v3_with_vault(
        destination.path(),
        "vpn-success",
        &[profile],
        &[],
        &[],
        &[],
        &[],
        &[],
        &[],
        &[vpn],
        &[config],
        &mut vault,
    )
    .unwrap();
    assert_eq!(rerun.reused_vpn_configs, 1);
    assert_eq!(rerun.reused_vpns, 1);
    assert_eq!(rerun.reused_profiles, 1);
    assert_eq!(
        repository
            .get::<Vpn>(&plan.create_vpn_ids[0])
            .unwrap()
            .unwrap()
            .revision,
        1
    );
}

#[test]
fn proxy_and_vpn_reference_conflict_fails_before_writes() {
    let destination = Temp::new("network-conflict");
    fs::remove_dir(destination.path()).unwrap();
    let vpn = vpn_record("vpn-conflict", "VPN Conflict");
    let config = vpn_config(&vpn, "client\nremote vpn.example.test 1194\n");
    let proxy_id = migration_uuid("buglogin-legacy", "local", "proxy", "proxy-a").to_string();
    let profile = profile_record("profile-conflict", Some(vpn.id.clone()), Some(proxy_id));
    let vault = FakeVault::default();

    let error = plan_records_v3_with_vault(
        destination.path(),
        &[profile],
        &[],
        &[],
        &[],
        &[],
        &[],
        &[],
        &[vpn],
        &[config],
        &vault,
    )
    .unwrap_err();
    assert_eq!(error.code, "migration_profile_network_conflict");
    assert!(!destination.path().exists());
    assert!(vault.values.is_empty());
}

#[test]
fn vpn_entity_collision_removes_new_vault_config() {
    let destination = Temp::new("cleanup");
    let vpn = vpn_record("vpn-collision", "Imported VPN");
    let config = vpn_config(&vpn, "[Interface]\nPrivateKey = cleanup-secret\n");
    let repository = LocalRepository::open(destination.path()).unwrap();
    repository
        .create(Vpn {
            id: VpnId::new(vpn.id.clone()).unwrap(),
            name: "Existing VPN".into(),
            kind: VpnKind::Wireguard,
            config_ref: CredentialId::new(vpn.data.config_ref.clone()).unwrap(),
            last_used_at: None,
            sync_mode: None,
            created_at: "2026-07-21T00:00:00Z".into(),
            updated_at: "2026-07-21T00:00:00Z".into(),
        })
        .unwrap();
    let mut vault = FakeVault::default();

    let error = activate_records_v3_with_vault(
        destination.path(),
        "vpn-cleanup",
        &[],
        &[],
        &[],
        &[],
        &[],
        &[],
        &[],
        &[vpn],
        &[config.clone()],
        &mut vault,
    )
    .unwrap_err();
    assert_eq!(error.code, "migration_activation_collision");
    assert!(
        !vault
            .values
            .contains_key(&("vpn-configs".into(), config.config_ref))
    );
    assert!(
        !destination
            .path()
            .join("migration-activations/vpn-cleanup.json")
            .exists()
    );
}

fn vpn_record(source_id: &str, name: &str) -> NeutralVpnRecord {
    NeutralVpnRecord {
        schema: "buglogin.vpn/1".into(),
        source_ref: source_ref(source_id),
        id: migration_uuid("buglogin-legacy", "local", "vpn", source_id).to_string(),
        revision: 1,
        created_at: Some("2026-07-21T00:00:00Z".into()),
        updated_at: Some("2026-07-21T00:00:00Z".into()),
        data: NeutralVpnData {
            name: name.into(),
            kind: "wireguard".into(),
            config_ref: migration_uuid("buglogin-legacy", "local", "vpn-config", source_id)
                .to_string(),
            last_used_at: None,
            sync_mode: "local".into(),
        },
        extensions: Map::new(),
    }
}

fn vpn_config(vpn: &NeutralVpnRecord, config_data: &str) -> VpnConfigSecret {
    VpnConfigSecret {
        schema: "buglogin.vpn-config-secret/1".into(),
        config_ref: vpn.data.config_ref.clone(),
        vpn_id: vpn.id.clone(),
        kind: VpnKind::Wireguard,
        config_sha256: buglogin_storage::blob::checksum(config_data.as_bytes()),
        config_data: config_data.into(),
    }
}

fn profile_record(
    source_id: &str,
    vpn_id: Option<String>,
    proxy_id: Option<String>,
) -> NeutralProfileRecord {
    NeutralProfileRecord {
        schema: "buglogin.profile/1".into(),
        source_ref: source_ref(source_id),
        id: migration_uuid("buglogin-legacy", "local", "profile", source_id).to_string(),
        revision: 1,
        created_at: Some("2026-07-21T00:00:00Z".into()),
        updated_at: Some("2026-07-21T00:00:00Z".into()),
        data: NeutralProfileData {
            name: source_id.into(),
            runtime: "wayfern".into(),
            runtime_version: None,
            release_type: "stable".into(),
            ephemeral: false,
            source_data_object: ProfileDataObject {
                path: format!("profiles/{source_id}/profile"),
                logical_type: "profile-directory".into(),
                disposition: ProfileDataDisposition {
                    classification: ProfileDataClassification::Excluded,
                    reason_code: "test".into(),
                },
            },
            migration_class: "direct".into(),
            group_id: None,
            proxy_id,
            vpn_id,
            extension_group_id: None,
            tags: Vec::new(),
            note: None,
            legacy_runtime_config: None,
        },
        extensions: Map::new(),
    }
}

fn source_ref(source_id: &str) -> ProfileSourceRef {
    ProfileSourceRef {
        system: "buglogin-legacy".into(),
        instance_id: "local".into(),
        id: source_id.into(),
    }
}
