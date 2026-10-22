use buglogin_domain::{
    entities::{Group, Profile, ProfileState, Proxy, Runtime, SyncMode},
    ids::{ProfileId, migration_uuid},
};
use buglogin_migration::{
    activate::{
        activate_profiles, activate_records, activate_records_with_vault, plan_profiles,
        plan_records, plan_records_with_vault,
    },
    encrypted_secrets::ProxyCredentialSecret,
    legacy::{NeutralGroupData, NeutralGroupRecord, NeutralProxyData, NeutralProxyRecord},
    rollback::{confirm_rollback, rollback_activation, rollback_activation_with_vault},
    transform::{
        NeutralProfileData, NeutralProfileRecord, ProfileDataClassification,
        ProfileDataDisposition, ProfileDataObject, ProfileSourceRef,
    },
};
use buglogin_storage::{
    local::LocalRepository,
    secrets::{OsVault, RotationReceipt, SecretError, SecretKey, SecretValue, SecretVersion},
};
use serde_json::{Map, json};
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

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

static SEQ: AtomicU64 = AtomicU64::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "buglogin-activation-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn record(source_id: &str, name: &str) -> NeutralProfileRecord {
    NeutralProfileRecord {
        schema: "buglogin.profile/1".into(),
        source_ref: ProfileSourceRef {
            system: "buglogin-legacy".into(),
            instance_id: "local".into(),
            id: source_id.into(),
        },
        id: migration_uuid("buglogin-legacy", "local", "profile", source_id).to_string(),
        revision: 1,
        created_at: Some("2026-07-20T00:00:00Z".into()),
        updated_at: Some("2026-07-20T00:01:00Z".into()),
        data: NeutralProfileData {
            name: name.into(),
            runtime: "wayfern".into(),
            runtime_version: None,
            release_type: "stable".into(),
            ephemeral: false,
            source_data_object: ProfileDataObject {
                path: "profiles/x/profile".into(),
                logical_type: "profile-directory".into(),
                disposition: ProfileDataDisposition {
                    classification: ProfileDataClassification::Excluded,
                    reason_code: "redacted".into(),
                },
            },
            migration_class: "direct".into(),
            group_id: None,
            proxy_id: None,
            vpn_id: None,
            extension_group_id: None,
            tags: vec!["migrated".into()],
            note: None,
            legacy_runtime_config: None,
        },
        extensions: Map::new(),
    }
}
fn foreign(id: &str) -> Profile {
    Profile {
        id: ProfileId::new(id).unwrap(),
        name: "Foreign".into(),
        runtime: Runtime::Wayfern,
        runtime_version: None,
        runtime_config: None,
        state: ProfileState::Ready,
        archived: false,
        fingerprint: None,
        note: None,
        proxy_id: None,
        vpn_id: None,
        group_ids: vec![],
        tags: vec![],
        extension_group_id: None,
        sync_mode: Some(SyncMode::Local),
        created_at: "2026-07-20T00:00:00Z".into(),
        updated_at: "2026-07-20T00:00:00Z".into(),
    }
}

fn group_record(source_id: &str) -> NeutralGroupRecord {
    NeutralGroupRecord {
        schema: "buglogin.group/1".into(),
        source_ref: ProfileSourceRef {
            system: "buglogin-legacy".into(),
            instance_id: "local".into(),
            id: source_id.into(),
        },
        id: migration_uuid("buglogin-legacy", "local", "group", source_id).to_string(),
        revision: 1,
        created_at: Some("2026-07-20T00:00:00Z".into()),
        updated_at: Some("2026-07-20T00:01:00Z".into()),
        data: NeutralGroupData {
            name: "Sales".into(),
            appearance: None,
            workspace_id: None,
        },
        extensions: Map::new(),
    }
}

fn proxy_record(source_id: &str) -> NeutralProxyRecord {
    NeutralProxyRecord {
        schema: "buglogin.proxy/1".into(),
        source_ref: ProfileSourceRef {
            system: "buglogin-legacy".into(),
            instance_id: "local".into(),
            id: source_id.into(),
        },
        id: migration_uuid("buglogin-legacy", "local", "proxy", source_id).to_string(),
        revision: 1,
        created_at: Some("2026-07-20T00:00:00Z".into()),
        updated_at: Some("2026-07-20T00:01:00Z".into()),
        data: NeutralProxyData {
            name: Some("Migration proxy".into()),
            protocol: "socks5".into(),
            host: "127.0.0.2".into(),
            port: 1080,
            credential_ref: None,
            rotation_url: None,
            bypass_rules: vec!["localhost".into()],
        },
        extensions: Map::new(),
    }
}

#[test]
fn activation_rerun_keeps_revision_one() {
    let t = Temp::new();
    let r = record("00000000-0000-0000-0000-000000000001", "Migrated");
    let first = activate_profiles(&t.0, "run", &[r.clone()]).unwrap();
    let second = activate_profiles(&t.0, "run", &[r]).unwrap();
    assert_eq!(first.created_profiles, 1);
    assert_eq!(second.created_profiles, 0);
    assert_eq!(
        LocalRepository::open(&t.0)
            .unwrap()
            .list::<Profile>()
            .unwrap()[0]
            .revision,
        1
    );
}

#[test]
fn activation_preserves_runtime_version_config_fingerprint_and_note() {
    let t = Temp::new();
    let mut r = record("00000000-0000-0000-0000-000000000011", "Migrated config");
    r.data.runtime_version = Some("146.0.7680.177.4".into());
    r.data.note = Some("legacy note".into());
    r.data.legacy_runtime_config = Some(json!({
        "schema_key": "chromium.runtime-config.v1",
        "schema_version": 1,
        "payload": {
            "fingerprint": r#"{"language":"en-US","timezone":"America/New_York"}"#,
            "runtime_context": {"country_code": "US"}
        }
    }));

    activate_profiles(&t.0, "run-config", &[r]).unwrap();
    let profile = LocalRepository::open(&t.0)
        .unwrap()
        .list::<Profile>()
        .unwrap()
        .remove(0)
        .value;

    assert_eq!(profile.runtime_version.as_deref(), Some("146.0.7680.177.4"));
    assert_eq!(profile.note.as_deref(), Some("legacy note"));
    assert_eq!(profile.fingerprint.as_ref().unwrap()["language"], "en-US");
    assert_eq!(
        profile.runtime_config.as_ref().unwrap()["schema_key"],
        "chromium.runtime-config.v1"
    );
}

#[test]
fn rollback_quarantines_owned_only() {
    let t = Temp::new();
    let repo = LocalRepository::open(&t.0).unwrap();
    repo.create(foreign("prf_foreign")).unwrap();
    let r = record("00000000-0000-0000-0000-000000000002", "Migrated");
    let id = r.id.clone();
    activate_profiles(&t.0, "run", &[r]).unwrap();
    let receipt = rollback_activation(
        &t.0,
        "run",
        confirm_rollback("ROLLBACK_MIGRATION_RUN").unwrap(),
    )
    .unwrap();
    assert_eq!(receipt.quarantined_profiles.len(), 1);
    assert!(repo.get::<Profile>(&id).unwrap().is_none());
    assert!(repo.get::<Profile>("prf_foreign").unwrap().is_some());
}

#[test]
fn rollback_cas_refuses_changed_entity() {
    let t = Temp::new();
    let repo = LocalRepository::open(&t.0).unwrap();
    let r = record("00000000-0000-0000-0000-000000000003", "Migrated");
    let id = r.id.clone();
    activate_profiles(&t.0, "run", &[r]).unwrap();
    let mut p = repo.get::<Profile>(&id).unwrap().unwrap().value;
    p.name = "Changed".into();
    repo.update(1, p).unwrap();
    let e = rollback_activation(
        &t.0,
        "run",
        confirm_rollback("ROLLBACK_MIGRATION_RUN").unwrap(),
    )
    .unwrap_err();
    assert_eq!(e.code, "migration_activation_entity_changed");
    assert_eq!(repo.get::<Profile>(&id).unwrap().unwrap().revision, 2);
}
#[test]
fn activation_plan_is_read_only_and_detects_reuse_and_collision() {
    let temp = Temp::new();
    let destination = temp.0.join("planned-destination");
    let record = record("00000000-0000-0000-0000-000000000004", "Migrated");
    let id = record.id.clone();

    let initial = plan_profiles(&destination, &[record.clone()]).unwrap();
    assert_eq!(initial.create_profile_ids, vec![id.clone()]);
    assert!(initial.reuse_profile_ids.is_empty());
    assert!(!destination.exists());

    activate_profiles(&destination, "planned-run", &[record.clone()]).unwrap();
    let reuse = plan_profiles(&destination, &[record.clone()]).unwrap();
    assert!(reuse.create_profile_ids.is_empty());
    assert_eq!(reuse.reuse_profile_ids, vec![id]);

    let mut changed = record;
    changed.data.name = "Changed".into();
    assert_eq!(
        plan_profiles(&destination, &[changed]).unwrap_err().code,
        "migration_activation_collision"
    );
}
#[test]
fn activation_rejects_unresolved_entity_references_before_write() {
    let temp = Temp::new();
    let destination = temp.0.join("referenced-destination");
    let mut referenced = record("00000000-0000-0000-0000-000000000005", "Referenced");
    referenced.data.proxy_id = Some("00000000-0000-0000-0000-000000000099".into());
    assert_eq!(
        plan_profiles(&destination, &[referenced]).unwrap_err().code,
        "migration_reference_missing"
    );
    assert!(!destination.exists());
}

#[test]
fn activates_reuses_and_rolls_back_groups_proxies_and_profiles_atomically() {
    let temp = Temp::new();
    let destination = temp.0.join("entity-destination");
    let group = group_record("group-a");
    let proxy = proxy_record("proxy-a");
    let mut profile = record("00000000-0000-0000-0000-000000000006", "Referenced");
    profile.data.group_id = Some(group.id.clone());
    profile.data.proxy_id = Some(proxy.id.clone());

    let plan = plan_records(
        &destination,
        &[profile.clone()],
        &[group.clone()],
        &[proxy.clone()],
    )
    .unwrap();
    assert_eq!(plan.create_group_ids, vec![group.id.clone()]);
    assert_eq!(plan.create_proxy_ids, vec![proxy.id.clone()]);
    assert_eq!(plan.create_profile_ids, vec![profile.id.clone()]);
    assert!(!destination.exists());

    let first = activate_records(
        &destination,
        "entity-run",
        &[profile.clone()],
        &[group.clone()],
        &[proxy.clone()],
    )
    .unwrap();
    assert_eq!(first.created_groups, 1);
    assert_eq!(first.created_proxies, 1);
    assert_eq!(first.created_profiles, 1);

    let second = activate_records(
        &destination,
        "entity-run",
        &[profile.clone()],
        &[group.clone()],
        &[proxy.clone()],
    )
    .unwrap();
    assert_eq!(second.reused_groups, 1);
    assert_eq!(second.reused_proxies, 1);
    assert_eq!(second.reused_profiles, 1);

    let repository = LocalRepository::open(&destination).unwrap();
    assert!(repository.get::<Group>(&group.id).unwrap().is_some());
    assert!(repository.get::<Proxy>(&proxy.id).unwrap().is_some());
    assert!(repository.get::<Profile>(&profile.id).unwrap().is_some());

    let rollback = rollback_activation(
        &destination,
        "entity-run",
        confirm_rollback("ROLLBACK_MIGRATION_RUN").unwrap(),
    )
    .unwrap();
    assert_eq!(rollback.quarantined_groups.len(), 1);
    assert_eq!(rollback.quarantined_proxies.len(), 1);
    assert_eq!(rollback.quarantined_profiles.len(), 1);
    assert!(repository.get::<Group>(&group.id).unwrap().is_none());
    assert!(repository.get::<Proxy>(&proxy.id).unwrap().is_none());
    assert!(repository.get::<Profile>(&profile.id).unwrap().is_none());
}

#[test]
fn proxy_credentials_use_vault_cas_and_rollback_without_plaintext_metadata() {
    let temp = Temp::new();
    let destination = temp.0.join("credential-destination");
    let mut proxy = proxy_record("proxy-secret");
    let credential_id = migration_uuid(
        "buglogin-legacy",
        "local",
        "proxy-credential",
        "proxy-secret",
    )
    .to_string();
    proxy.data.credential_ref = Some(credential_id.clone());
    let credential = ProxyCredentialSecret {
        credential_id: credential_id.clone(),
        username: Some("secret-user".into()),
        password: Some("secret-password".into()),
    };
    let mut profile = record("00000000-0000-0000-0000-000000000007", "Secret Proxy");
    profile.data.proxy_id = Some(proxy.id.clone());
    let mut vault = FakeVault::default();

    let plan = plan_records_with_vault(
        &destination,
        &[profile.clone()],
        &[],
        &[proxy.clone()],
        std::slice::from_ref(&credential),
        &vault,
    )
    .unwrap();
    assert_eq!(plan.create_credential_ids, vec![credential_id.clone()]);
    assert!(!destination.exists());

    let first = activate_records_with_vault(
        &destination,
        "credential-run",
        &[profile.clone()],
        &[],
        &[proxy.clone()],
        std::slice::from_ref(&credential),
        &mut vault,
    )
    .unwrap();
    assert_eq!(first.created_credentials, 1);
    let key = SecretKey::new("proxy-credentials", &credential_id).unwrap();
    let (stored, version) = vault.read(&key).unwrap();
    assert_eq!(version.generation(), 1);
    assert!(!String::from_utf8_lossy(stored.as_bytes()).is_empty());

    let second = activate_records_with_vault(
        &destination,
        "credential-run",
        &[profile],
        &[],
        &[proxy],
        std::slice::from_ref(&credential),
        &mut vault,
    )
    .unwrap();
    assert_eq!(second.reused_credentials, 1);

    let rollback = rollback_activation_with_vault(
        &destination,
        "credential-run",
        confirm_rollback("ROLLBACK_MIGRATION_RUN").unwrap(),
        &mut vault,
    )
    .unwrap();
    assert_eq!(rollback.removed_credentials, vec![credential_id]);
    assert_eq!(vault.read(&key).unwrap_err(), SecretError::NotFound);
}

#[test]
fn malformed_activation_pending_journal_fails_closed() {
    let temp = Temp::new();
    let destination = temp.0.join("pending-destination");
    fs::create_dir_all(destination.join("migration-activations")).unwrap();
    fs::write(
        destination.join("migration-activations/.pending-run.pending.json"),
        b"{}",
    )
    .unwrap();

    let error = activate_profiles(
        &destination,
        "pending-run",
        &[record("00000000-0000-0000-0000-000000000099", "Pending")],
    )
    .unwrap_err();
    assert_eq!(error.code, "migration_activation_journal_invalid");
}

#[test]
fn matching_activation_pending_journal_resumes() {
    let temp = Temp::new();
    let destination = temp.0.join("pending-resume-destination");
    let profile = record("00000000-0000-0000-0000-000000000100", "Resume");

    activate_profiles(
        &destination,
        "pending-resume-run",
        std::slice::from_ref(&profile),
    )
    .unwrap();
    let journal = destination.join("migration-activations/pending-resume-run.json");
    let pending = destination.join("migration-activations/.pending-resume-run.pending.json");
    fs::copy(&journal, &pending).unwrap();
    fs::remove_file(&journal).unwrap();

    let receipt = activate_profiles(
        &destination,
        "pending-resume-run",
        std::slice::from_ref(&profile),
    )
    .unwrap();
    assert_eq!(receipt.created_profiles, 0);
    assert!(journal.exists());
    assert!(!pending.exists());
}
