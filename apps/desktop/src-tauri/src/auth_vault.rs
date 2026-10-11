use buglogin_domain::errors::Problem;
use buglogin_storage::{
    secrets::{OsVault, RotationReceipt, SecretError, SecretKey, SecretValue, SecretVersion},
    vault::NativeVault,
};
use sha2::{Digest, Sha256};

const NAMESPACE: &str = "buglogin-auth";

pub struct StoredBearer {
    bearer: String,
    version: SecretVersion,
}

impl StoredBearer {
    pub fn bearer(&self) -> &str {
        &self.bearer
    }

    pub const fn version(&self) -> SecretVersion {
        self.version
    }
}

impl std::fmt::Debug for StoredBearer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StoredBearer")
            .field("bearer", &"[REDACTED]")
            .field("version", &self.version)
            .finish()
    }
}

pub struct AuthVault<V = NativeVault> {
    vault: V,
}

impl AuthVault<NativeVault> {
    pub const fn native() -> Self {
        Self {
            vault: NativeVault::new(),
        }
    }
}

impl<V: OsVault> AuthVault<V> {
    pub const fn new(vault: V) -> Self {
        Self { vault }
    }

    pub fn load_bearer(
        &self,
        device_id: &str,
        correlation_id: &str,
    ) -> Result<Option<StoredBearer>, Problem> {
        let key = session_key(device_id, correlation_id)?;
        match self.vault.read(&key) {
            Ok((value, version)) => {
                let bearer = String::from_utf8(value.as_bytes().to_vec()).map_err(|_| {
                    vault_problem(
                        SecretError::OperationFailed,
                        "Stored bearer is invalid",
                        correlation_id,
                    )
                })?;
                Ok(Some(StoredBearer { bearer, version }))
            }
            Err(SecretError::NotFound) => Ok(None),
            Err(error) => Err(vault_problem(
                error,
                "Unable to load bearer",
                correlation_id,
            )),
        }
    }

    pub fn store_bearer(
        &mut self,
        device_id: &str,
        bearer: &str,
        correlation_id: &str,
    ) -> Result<SecretVersion, Problem> {
        let key = session_key(device_id, correlation_id)?;
        let value = secret_value(bearer, correlation_id)?;
        self.vault
            .create(&key, value)
            .map_err(|error| vault_problem(error, "Unable to store bearer", correlation_id))
    }

    pub fn replace_bearer(
        &mut self,
        device_id: &str,
        expected_version: SecretVersion,
        bearer: &str,
        correlation_id: &str,
    ) -> Result<SecretVersion, Problem> {
        let key = session_key(device_id, correlation_id)?;
        let value = secret_value(bearer, correlation_id)?;
        let RotationReceipt { current, .. } = self
            .vault
            .rotate(&key, expected_version, value)
            .map_err(|error| vault_problem(error, "Unable to replace bearer", correlation_id))?;
        Ok(current)
    }

    pub fn remove_bearer(
        &mut self,
        device_id: &str,
        expected_version: SecretVersion,
        correlation_id: &str,
    ) -> Result<(), Problem> {
        let key = session_key(device_id, correlation_id)?;
        self.vault
            .remove(&key, expected_version)
            .map_err(|error| vault_problem(error, "Unable to remove bearer", correlation_id))
    }
}

fn session_key(device_id: &str, correlation_id: &str) -> Result<SecretKey, Problem> {
    let device_id = device_id.trim();
    if device_id.is_empty() {
        return Err(vault_problem(
            SecretError::InvalidKey,
            "Device ID is required",
            correlation_id,
        ));
    }
    let digest = Sha256::digest(device_id.as_bytes());
    SecretKey::new(NAMESPACE, format!("device-{:x}", digest))
        .map_err(|error| vault_problem(error, "Unable to build session key", correlation_id))
}

fn secret_value(bearer: &str, correlation_id: &str) -> Result<SecretValue, Problem> {
    SecretValue::new(bearer.as_bytes().to_vec())
        .map_err(|error| vault_problem(error, "Bearer is invalid", correlation_id))
}

fn vault_problem(error: SecretError, title: &str, correlation_id: &str) -> Problem {
    let status = match error {
        SecretError::InvalidKey | SecretError::EmptyValue | SecretError::TooLarge => 400,
        SecretError::NotFound => 404,
        SecretError::AccessDenied => 403,
        SecretError::VersionConflict => 409,
        SecretError::Unavailable => 503,
        SecretError::OperationFailed => 500,
    };
    let mut problem = Problem::new(error.code(), title, status, correlation_id);
    if error == SecretError::VersionConflict || error == SecretError::Unavailable {
        problem.retryable = true;
    }
    problem
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[derive(Default)]
    struct FakeVault {
        values: BTreeMap<(String, String), (SecretValue, SecretVersion)>,
    }

    impl OsVault for FakeVault {
        fn read(&self, key: &SecretKey) -> Result<(SecretValue, SecretVersion), SecretError> {
            self.values
                .get(&(key.namespace().to_owned(), key.name().to_owned()))
                .cloned()
                .ok_or(SecretError::NotFound)
        }

        fn create(
            &mut self,
            key: &SecretKey,
            value: SecretValue,
        ) -> Result<SecretVersion, SecretError> {
            let map_key = (key.namespace().to_owned(), key.name().to_owned());
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
            let map_key = (key.namespace().to_owned(), key.name().to_owned());
            let current = self
                .values
                .get(&map_key)
                .map(|(_, version)| *version)
                .ok_or(SecretError::NotFound)?;
            if current != expected {
                return Err(SecretError::VersionConflict);
            }
            let next = SecretVersion::new(current.generation() + 1).unwrap();
            self.values.insert(map_key, (replacement, next));
            Ok(RotationReceipt {
                previous: current,
                current: next,
            })
        }

        fn remove(&mut self, key: &SecretKey, expected: SecretVersion) -> Result<(), SecretError> {
            let map_key = (key.namespace().to_owned(), key.name().to_owned());
            let (_, current) = self.values.get(&map_key).ok_or(SecretError::NotFound)?;
            if *current != expected {
                return Err(SecretError::VersionConflict);
            }
            self.values.remove(&map_key);
            Ok(())
        }
    }

    #[test]
    fn bearer_round_trip_replacement_and_removal_use_versions() {
        let mut vault = AuthVault::new(FakeVault::default());
        let first = vault
            .store_bearer("device-a", "first-token", "cor-store")
            .unwrap();
        assert_eq!(
            vault
                .load_bearer("device-a", "cor-load")
                .unwrap()
                .unwrap()
                .bearer(),
            "first-token"
        );
        let second = vault
            .replace_bearer("device-a", first, "second-token", "cor-replace")
            .unwrap();
        assert_eq!(second.generation(), 2);
        assert_eq!(
            vault
                .load_bearer("device-a", "cor-load")
                .unwrap()
                .unwrap()
                .bearer(),
            "second-token"
        );
        vault
            .remove_bearer("device-a", second, "cor-remove")
            .unwrap();
        assert!(vault.load_bearer("device-a", "cor-load").unwrap().is_none());
    }

    #[test]
    fn missing_bearer_maps_to_none_and_device_key_is_stable() {
        let vault = AuthVault::new(FakeVault::default());
        assert!(vault.load_bearer("device-a", "cor-load").unwrap().is_none());
        let first = session_key(" device-a ", "cor-key").unwrap();
        let second = session_key("device-a", "cor-key").unwrap();
        assert_eq!(first, second);
        assert_ne!(
            first.name(),
            session_key("device-b", "cor-key").unwrap().name()
        );
    }

    #[test]
    fn errors_are_typed_and_never_contain_bearer() {
        let mut vault = AuthVault::new(FakeVault::default());
        let error = vault.store_bearer("device-a", "", "cor-empty").unwrap_err();
        assert_eq!(error.code, "empty_secret");
        assert!(!format!("{error:?}").contains("first-token"));
        assert!(vault.load_bearer("device-a", "cor-load").unwrap().is_none());
    }
}
