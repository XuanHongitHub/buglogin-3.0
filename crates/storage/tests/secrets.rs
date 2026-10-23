#[path = "../src/secrets/mod.rs"]
mod secrets;

use std::collections::HashMap;

use secrets::{
    OsVault, REDACTED, RotationReceipt, SecretError, SecretKey, SecretValue, SecretVersion,
    UnavailableVault,
};

#[derive(Default)]
struct FakeVault {
    entries: HashMap<SecretKey, (SecretValue, SecretVersion)>,
    next_generation: u64,
}

impl OsVault for FakeVault {
    fn read(&self, key: &SecretKey) -> Result<(SecretValue, SecretVersion), SecretError> {
        self.entries.get(key).cloned().ok_or(SecretError::NotFound)
    }

    fn create(
        &mut self,
        key: &SecretKey,
        value: SecretValue,
    ) -> Result<SecretVersion, SecretError> {
        if self.entries.contains_key(key) {
            return Err(SecretError::VersionConflict);
        }
        let version = self.next_version();
        self.entries.insert(key.clone(), (value, version));
        Ok(version)
    }

    fn rotate(
        &mut self,
        key: &SecretKey,
        expected: SecretVersion,
        replacement: SecretValue,
    ) -> Result<RotationReceipt, SecretError> {
        let current = self
            .entries
            .get(key)
            .map(|(_, version)| *version)
            .ok_or(SecretError::NotFound)?;
        if current != expected {
            return Err(SecretError::VersionConflict);
        }
        let next = self.next_version();
        self.entries.insert(key.clone(), (replacement, next));
        Ok(RotationReceipt {
            previous: current,
            current: next,
        })
    }

    fn remove(&mut self, key: &SecretKey, expected: SecretVersion) -> Result<(), SecretError> {
        let current = self
            .entries
            .get(key)
            .map(|(_, version)| *version)
            .ok_or(SecretError::NotFound)?;
        if current != expected {
            return Err(SecretError::VersionConflict);
        }
        self.entries.remove(key);
        Ok(())
    }
}

impl FakeVault {
    fn next_version(&mut self) -> SecretVersion {
        self.next_generation += 1;
        SecretVersion::new(self.next_generation).unwrap()
    }
}

fn key() -> SecretKey {
    SecretKey::new("proxy", "profile-1").unwrap()
}

#[test]
fn secret_value_redacts_debug_and_display() {
    let value = SecretValue::new(b"do-not-leak".to_vec()).unwrap();
    assert_eq!(format!("{value:?}"), "SecretValue(\"<redacted>\")");
    assert_eq!(value.to_string(), REDACTED);
}

#[test]
fn vault_requires_os_backend_and_fails_closed() {
    let mut vault = UnavailableVault;
    let error = vault
        .create(&key(), SecretValue::new(b"secret".to_vec()).unwrap())
        .unwrap_err();
    assert_eq!(error, SecretError::Unavailable);
    assert_eq!(error.to_string(), "secret_vault_unavailable");
}

#[test]
fn rotation_requires_current_version_and_returns_metadata_only() {
    let mut vault = FakeVault::default();
    let version = vault
        .create(&key(), SecretValue::new(b"old".to_vec()).unwrap())
        .unwrap();
    let receipt = vault
        .rotate(&key(), version, SecretValue::new(b"new".to_vec()).unwrap())
        .unwrap();
    assert_eq!(receipt.previous, version);
    assert_ne!(receipt.current, version);
    assert_eq!(vault.read(&key()).unwrap().0.as_bytes(), b"new");
    assert_eq!(
        vault
            .rotate(
                &key(),
                version,
                SecretValue::new(b"stale".to_vec()).unwrap()
            )
            .unwrap_err(),
        SecretError::VersionConflict
    );
}

#[test]
fn keys_reject_path_like_components() {
    let valid = key();
    assert_eq!(valid.namespace(), "proxy");
    assert_eq!(valid.name(), "profile-1");
    assert_eq!(
        SecretKey::new("proxy", "../profile").unwrap_err(),
        SecretError::InvalidKey
    );
}

#[test]
fn versions_and_deletion_are_explicit() {
    let mut vault = FakeVault::default();
    let version = vault
        .create(&key(), SecretValue::new(b"secret".to_vec()).unwrap())
        .unwrap();
    assert_eq!(version.generation(), 1);
    assert!(!vault.read(&key()).unwrap().0.is_empty());
    assert_eq!(
        vault
            .remove(&key(), SecretVersion::new(2).unwrap())
            .unwrap_err(),
        SecretError::VersionConflict
    );
    vault.remove(&key(), version).unwrap();
    assert_eq!(vault.read(&key()).unwrap_err(), SecretError::NotFound);
    assert_eq!(SecretError::AccessDenied.code(), "secret_access_denied");
    assert_eq!(
        SecretError::OperationFailed.code(),
        "secret_operation_failed"
    );
}
