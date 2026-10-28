use std::sync::Mutex;

use buglogin_domain::{errors::Problem, ids::DeviceId};
use buglogin_storage::{
    secrets::{OsVault, SecretError, SecretKey, SecretValue},
    vault::NativeVault,
};
use buglogin_sync::crypto::DevicePrivateKey;
use sha2::{Digest, Sha256};

const NAMESPACE: &str = "buglogin-device-keys";
const PRIVATE_KEY_BYTES: usize = 32;

pub struct DeviceKeyVault<V = NativeVault> {
    vault: Mutex<V>,
}

impl DeviceKeyVault<NativeVault> {
    pub const fn native() -> Self {
        Self {
            vault: Mutex::new(NativeVault::new()),
        }
    }
}

impl<V: OsVault> DeviceKeyVault<V> {
    #[cfg(test)]
    pub const fn new(vault: V) -> Self {
        Self {
            vault: Mutex::new(vault),
        }
    }

    pub fn load_or_create(
        &self,
        device_id: &str,
        correlation_id: &str,
    ) -> Result<DevicePrivateKey, Problem> {
        let secret_key = device_secret_key(device_id, correlation_id)?;
        let mut vault = self.vault.lock().map_err(|_| unavailable(correlation_id))?;
        match vault.read(&secret_key) {
            Ok((value, _)) => decode_private_key(value.as_bytes(), correlation_id),
            Err(SecretError::NotFound) => {
                let private_key =
                    DevicePrivateKey::generate().map_err(|_| unavailable(correlation_id))?;
                let secret = SecretValue::new(private_key.as_bytes().to_vec())
                    .map_err(|_| unavailable(correlation_id))?;
                match vault.create(&secret_key, secret) {
                    Ok(_) => Ok(private_key),
                    Err(SecretError::VersionConflict) => {
                        let (value, _) = vault
                            .read(&secret_key)
                            .map_err(|error| vault_problem(error, correlation_id))?;
                        decode_private_key(value.as_bytes(), correlation_id)
                    }
                    Err(error) => Err(vault_problem(error, correlation_id)),
                }
            }
            Err(error) => Err(vault_problem(error, correlation_id)),
        }
    }
}

fn decode_private_key(bytes: &[u8], correlation_id: &str) -> Result<DevicePrivateKey, Problem> {
    if bytes.len() != PRIVATE_KEY_BYTES {
        return Err(Problem::new(
            "device_key_vault_corrupt",
            "Stored device encryption key is invalid",
            500,
            correlation_id,
        ));
    }
    let mut key = [0_u8; PRIVATE_KEY_BYTES];
    key.copy_from_slice(bytes);
    Ok(DevicePrivateKey::from_bytes(key))
}

fn device_secret_key(device_id: &str, correlation_id: &str) -> Result<SecretKey, Problem> {
    if DeviceId::new(device_id).is_err() {
        return Err(Problem::new(
            "device_id_invalid",
            "Device ID is invalid",
            400,
            correlation_id,
        ));
    }
    let digest = Sha256::digest(device_id.as_bytes());
    SecretKey::new(NAMESPACE, format!("device-{:x}", digest))
        .map_err(|error| vault_problem(error, correlation_id))
}

fn unavailable(correlation_id: &str) -> Problem {
    Problem::new(
        "device_key_vault_unavailable",
        "Unable to access the operating system device-key vault",
        503,
        correlation_id,
    )
}

fn vault_problem(error: SecretError, correlation_id: &str) -> Problem {
    let status = match error {
        SecretError::InvalidKey | SecretError::EmptyValue | SecretError::TooLarge => 400,
        SecretError::NotFound => 404,
        SecretError::AccessDenied => 403,
        SecretError::VersionConflict => 409,
        SecretError::Unavailable => 503,
        SecretError::OperationFailed => 500,
    };
    Problem::new(
        error.code(),
        "Unable to access the operating system device-key vault",
        status,
        correlation_id,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use buglogin_storage::secrets::{RotationReceipt, SecretVersion};
    use std::collections::BTreeMap;

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
            let storage_key = (key.namespace().to_owned(), key.name().to_owned());
            if self.values.contains_key(&storage_key) {
                return Err(SecretError::VersionConflict);
            }
            let version = SecretVersion::new(1).unwrap();
            self.values.insert(storage_key, (value, version));
            Ok(version)
        }

        fn rotate(
            &mut self,
            key: &SecretKey,
            expected: SecretVersion,
            replacement: SecretValue,
        ) -> Result<RotationReceipt, SecretError> {
            let storage_key = (key.namespace().to_owned(), key.name().to_owned());
            let current = self
                .values
                .get(&storage_key)
                .map(|(_, version)| *version)
                .ok_or(SecretError::NotFound)?;
            if current != expected {
                return Err(SecretError::VersionConflict);
            }
            let next = SecretVersion::new(current.generation() + 1).unwrap();
            self.values.insert(storage_key, (replacement, next));
            Ok(RotationReceipt {
                previous: current,
                current: next,
            })
        }

        fn remove(&mut self, key: &SecretKey, expected: SecretVersion) -> Result<(), SecretError> {
            let storage_key = (key.namespace().to_owned(), key.name().to_owned());
            let (_, current) = self.values.get(&storage_key).ok_or(SecretError::NotFound)?;
            if *current != expected {
                return Err(SecretError::VersionConflict);
            }
            self.values.remove(&storage_key);
            Ok(())
        }
    }

    #[test]
    fn private_key_is_stable_per_device_and_scoped_between_devices() {
        let vault = DeviceKeyVault::new(FakeVault::default());
        let first = vault.load_or_create("device-a1", "test").unwrap();
        let first_public_key = first.public_key().unwrap();
        let reloaded = vault.load_or_create("device-a1", "test").unwrap();
        let other_device = vault.load_or_create("device-b1", "test").unwrap();

        assert_eq!(reloaded.public_key().unwrap(), first_public_key);
        assert_ne!(other_device.public_key().unwrap(), first_public_key);
        assert_eq!(format!("{first:?}"), "DevicePrivateKey(<redacted>)");
    }

    #[test]
    fn malformed_device_id_is_rejected_before_vault_access() {
        let vault = DeviceKeyVault::new(FakeVault::default());
        assert_eq!(
            vault.load_or_create("  ", "test").unwrap_err().code,
            "device_id_invalid"
        );
    }
}
