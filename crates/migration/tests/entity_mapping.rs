use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use buglogin_domain::{
    entities::{Group, Proxy},
    ids::migration_uuid,
};
use buglogin_migration::encrypted_secrets::decrypt_packaged_proxy_credentials;
use buglogin_migration::inspect::inspect_source;
use buglogin_migration::legacy::{
    map_legacy_groups, map_legacy_proxies, map_legacy_proxies_for_encrypted_package,
    map_legacy_proxies_for_ids,
};
use buglogin_migration::package::{PackageMetadata, build_encrypted_profile_package};
use buglogin_migration::reconcile::{ReconciliationStatus, reconcile_activation_with_vault};
use buglogin_migration::rollback::{confirm_rollback, rollback_activation_with_vault};
use buglogin_migration::{
    MigrationKey, activate_package, activate_package_with_vault, plan_package_activation,
    plan_package_activation_with_vault,
};
use buglogin_storage::local::LocalRepository;
use buglogin_storage::secrets::{
    OsVault, RotationReceipt, SecretError, SecretKey, SecretValue, SecretVersion,
};
use std::collections::{BTreeMap, BTreeSet};

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
        let version = SecretVersion::new(1).unwrap();
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

static SEQUENCE: AtomicU64 = AtomicU64::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "buglogin-entity-mapping-{}-{}",
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
fn maps_groups_and_both_credential_free_proxy_shapes_into_package() {
    let source = Temp::new();
    source.write("version.json", br#"{"system":"buglogin-legacy","version":"1.1.84","instanceId":"entities","os":"windows","architecture":"x86_64"}"#);
    source.write(
        "groups.json",
        br##"{"groups":[{"id":"group-a","name":"Sales","color":"#112233"}]}"##,
    );
    source.write("proxies/proxy-a.json", br#"{"id":"proxy-a","proxy_settings":{"proxy_type":"socks5","host":"127.0.0.2","port":1080},"bypass_rules":["localhost"]}"#);
    source.write(
        "proxies/proxy-b.json",
        br#"{"id":"proxy-b","upstream_url":"https://proxy.example.test:8443"}"#,
    );

    let groups = map_legacy_groups(source.path(), "entities").unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(
        groups[0].id,
        migration_uuid("buglogin-legacy", "entities", "group", "group-a").to_string()
    );
    assert_eq!(groups[0].data.name, "Sales");

    let proxies = map_legacy_proxies(source.path(), "entities").unwrap();
    assert_eq!(proxies.len(), 2);
    assert_eq!(proxies[0].data.protocol, "socks5");
    assert_eq!(proxies[0].data.bypass_rules, vec!["localhost"]);
    assert_eq!(proxies[1].data.protocol, "https");

    let (report, snapshot) = inspect_source(source.path()).unwrap();
    assert_eq!(report.entity_counts["groups"], 1);
    assert_eq!(report.entity_counts["proxies"], 2);
    let key = MigrationKey::from_bytes([51; 32]);
    let package = build_encrypted_profile_package(
        source.path(),
        &report,
        &snapshot,
        &PackageMetadata {
            created_at: "2026-07-21T00:00:00Z".into(),
            tool_version: "0.1.0".into(),
            tool_commit: "0123456789abcdef0123456789abcdef01234567".into(),
        },
        &key,
    )
    .unwrap();
    assert!(package.file("entities/groups.jsonl").is_some());
    assert!(package.file("entities/proxies.jsonl").is_some());

    let destination = Temp::new();
    let plan = plan_package_activation(destination.path(), &package, &key).unwrap();
    assert_eq!(plan.create_group_ids.len(), 1);
    assert_eq!(plan.create_proxy_ids.len(), 2);
    let receipt =
        activate_package(destination.path(), "entity-package", &package, Some(&key)).unwrap();
    assert_eq!(receipt.created_groups, 1);
    assert_eq!(receipt.created_proxies, 2);
    let repository = LocalRepository::open(destination.path()).unwrap();
    assert_eq!(repository.list::<Group>().unwrap().len(), 1);
    assert_eq!(repository.list::<Proxy>().unwrap().len(), 2);
}

#[test]
fn proxy_credentials_fail_closed_without_leaking_values() {
    let source = Temp::new();
    source.write("version.json", br#"{"system":"buglogin-legacy","version":"1.1.84","instanceId":"entities","os":"windows","architecture":"x86_64"}"#);
    source.write("proxies/secret.json", br#"{"id":"secret","proxy_settings":{"proxy_type":"http","host":"proxy.example.test","port":8080,"username":"secret-user","password":"secret-password"}}"#);
    let error = map_legacy_proxies(source.path(), "entities").unwrap_err();
    assert_eq!(
        error.code,
        "migration_proxy_credentials_require_secret_import"
    );
    let debug = format!("{error:?}");
    assert!(!debug.contains("secret-user"));
    assert!(!debug.contains("secret-password"));

    let mapped = map_legacy_proxies_for_encrypted_package(source.path(), "entities").unwrap();
    assert_eq!(mapped.records.len(), 1);
    assert_eq!(mapped.credentials.len(), 1);
    assert_eq!(
        mapped.records[0].data.credential_ref,
        Some(
            migration_uuid("buglogin-legacy", "entities", "proxy-credential", "secret").to_string()
        )
    );
    let (report, snapshot) = inspect_source(source.path()).unwrap();
    let key = MigrationKey::from_bytes([71; 32]);
    let package = build_encrypted_profile_package(
        source.path(),
        &report,
        &snapshot,
        &PackageMetadata {
            created_at: "2026-07-21T00:00:00Z".into(),
            tool_version: "0.1.0".into(),
            tool_commit: "0123456789abcdef0123456789abcdef01234567".into(),
        },
        &key,
    )
    .unwrap();
    let package_bytes = package
        .files()
        .values()
        .flatten()
        .copied()
        .collect::<Vec<_>>();
    assert!(
        !package_bytes
            .windows(b"secret-user".len())
            .any(|value| value == b"secret-user")
    );
    assert!(
        !package_bytes
            .windows(b"secret-password".len())
            .any(|value| value == b"secret-password")
    );
    let credentials = decrypt_packaged_proxy_credentials(&package, &key).unwrap();
    assert_eq!(credentials[0].username.as_deref(), Some("secret-user"));
    assert_eq!(credentials[0].password.as_deref(), Some("secret-password"));

    let destination = Temp::new();
    let mut vault = FakeVault::default();
    let plan =
        plan_package_activation_with_vault(destination.path(), &package, &key, &vault).unwrap();
    assert_eq!(plan.create_credential_ids.len(), 1);
    assert_eq!(plan.create_proxy_ids.len(), 1);
    let activation = activate_package_with_vault(
        destination.path(),
        "credential-package",
        &package,
        &key,
        &mut vault,
    )
    .unwrap();
    assert_eq!(activation.created_credentials, 1);
    assert_eq!(activation.created_proxies, 1);
    let reconciliation =
        reconcile_activation_with_vault(&report, destination.path(), "credential-package", &vault)
            .unwrap();
    assert_eq!(reconciliation.status, ReconciliationStatus::Passed);
    assert_eq!(reconciliation.activated_credentials, 1);
    let rollback = rollback_activation_with_vault(
        destination.path(),
        "credential-package",
        confirm_rollback("ROLLBACK_MIGRATION_RUN").unwrap(),
        &mut vault,
    )
    .unwrap();
    assert_eq!(rollback.removed_credentials.len(), 1);
    assert!(vault.values.is_empty());
}

#[cfg(unix)]
#[test]
fn referenced_proxy_mapping_rejects_symlinked_proxy_directory() {
    let source = Temp::new();
    let outside = Temp::new();
    outside.write(
        "proxy-a.json",
        br#"{"id":"proxy-a","proxy_settings":{"proxy_type":"http","host":"proxy.example.test","port":8080}}"#,
    );
    std::os::unix::fs::symlink(outside.path(), source.path().join("proxies")).unwrap();
    let ids = BTreeSet::from(["proxy-a".to_owned()]);

    let error = match map_legacy_proxies_for_ids(source.path(), "entities", &ids) {
        Ok(_) => panic!("symlinked proxy directory must be rejected"),
        Err(error) => error,
    };
    assert_eq!(error.code, "migration_entity_invalid");
}
