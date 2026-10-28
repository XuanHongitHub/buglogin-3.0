use std::sync::Mutex;

use buglogin_domain::errors::Problem;
use buglogin_storage::{
    secrets::{OsVault, SecretError, SecretKey, SecretValue},
    vault::NativeVault,
};
use getrandom::fill;

#[cfg(test)]
const NAMESPACE: &str = "buglogin-device-identity";
const RECORD_NAME: &str = "stable-id";

pub struct DeviceIdentityVault<V = NativeVault> {
    vault: Mutex<V>,
}

impl DeviceIdentityVault<NativeVault> {
    pub const fn native() -> Self {
        Self {
            vault: Mutex::new(NativeVault::new()),
        }
    }
}

impl<V: OsVault> DeviceIdentityVault<V> {
    #[cfg(test)]
    pub const fn new(vault: V) -> Self {
        Self {
            vault: Mutex::new(vault),
        }
    }

    #[cfg(test)]
    pub fn load_or_create(&self, correlation_id: &str) -> Result<String, Problem> {
        self.load_or_create_in_namespace(NAMESPACE, correlation_id)
    }

    pub fn load_or_create_in_namespace(
        &self,
        namespace: &str,
        correlation_id: &str,
    ) -> Result<String, Problem> {
        let key = SecretKey::new(namespace, RECORD_NAME)
            .map_err(|error| vault_error(error, correlation_id))?;
        let mut vault = self
            .vault
            .lock()
            .map_err(|_| identity_problem(correlation_id))?;
        match vault.read(&key) {
            Ok((value, _)) => decode_id(value.as_bytes(), correlation_id),
            Err(SecretError::NotFound) => {
                let id = generate_id(correlation_id)?;
                let value = SecretValue::new(id.as_bytes().to_vec())
                    .map_err(|_| identity_problem(correlation_id))?;
                match vault.create(&key, value) {
                    Ok(_) => Ok(id),
                    Err(SecretError::VersionConflict) => {
                        let (value, _) = vault
                            .read(&key)
                            .map_err(|error| vault_error(error, correlation_id))?;
                        decode_id(value.as_bytes(), correlation_id)
                    }
                    Err(error) => Err(vault_error(error, correlation_id)),
                }
            }
            Err(error) => Err(vault_error(error, correlation_id)),
        }
    }
}

fn generate_id(correlation_id: &str) -> Result<String, Problem> {
    let mut random = [0_u8; 16];
    fill(&mut random).map_err(|_| identity_problem(correlation_id))?;
    let mut id = String::with_capacity(36);
    id.push_str("dev_");
    for byte in random {
        id.push(hex_digit(byte >> 4));
        id.push(hex_digit(byte & 0x0f));
    }
    Ok(id)
}

fn decode_id(bytes: &[u8], correlation_id: &str) -> Result<String, Problem> {
    let id = std::str::from_utf8(bytes).map_err(|_| identity_corrupt(correlation_id))?;
    if id.len() != 36
        || !id.starts_with("dev_")
        || !id[4..].bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(identity_corrupt(correlation_id));
    }
    Ok(id.to_ascii_lowercase())
}

fn hex_digit(value: u8) -> char {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    DIGITS[value as usize] as char
}

fn identity_problem(correlation_id: &str) -> Problem {
    Problem::new(
        "device_identity_unavailable",
        "Unable to create or access this device's stable BugLogin identity",
        503,
        correlation_id,
    )
}

fn identity_corrupt(correlation_id: &str) -> Problem {
    Problem::new(
        "device_identity_corrupt",
        "Stored device identity is invalid",
        500,
        correlation_id,
    )
}

fn vault_error(error: SecretError, correlation_id: &str) -> Problem {
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
        "Unable to access the device identity vault",
        status,
        correlation_id,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use buglogin_storage::secrets::{RotationReceipt, SecretVersion};
    use std::{collections::HashMap, sync::Arc};

    #[derive(Clone, Default)]
    struct FakeVault(Arc<Mutex<HashMap<SecretKey, (SecretValue, SecretVersion)>>>);

    impl OsVault for FakeVault {
        fn read(&self, key: &SecretKey) -> Result<(SecretValue, SecretVersion), SecretError> {
            self.0
                .lock()
                .map_err(|_| SecretError::Unavailable)?
                .get(key)
                .cloned()
                .ok_or(SecretError::NotFound)
        }

        fn create(
            &mut self,
            key: &SecretKey,
            value: SecretValue,
        ) -> Result<SecretVersion, SecretError> {
            let mut values = self.0.lock().map_err(|_| SecretError::Unavailable)?;
            if values.contains_key(key) {
                return Err(SecretError::VersionConflict);
            }
            let version = SecretVersion::new(1).unwrap();
            values.insert(key.clone(), (value, version));
            Ok(version)
        }

        fn rotate(
            &mut self,
            key: &SecretKey,
            expected: SecretVersion,
            replacement: SecretValue,
        ) -> Result<RotationReceipt, SecretError> {
            let mut values = self.0.lock().map_err(|_| SecretError::Unavailable)?;
            let (_, current) = values.get(key).ok_or(SecretError::NotFound)?;
            if *current != expected {
                return Err(SecretError::VersionConflict);
            }
            let current = *current;
            let next = SecretVersion::new(current.generation() + 1).unwrap();
            values.insert(key.clone(), (replacement, next));
            Ok(RotationReceipt {
                previous: current,
                current: next,
            })
        }

        fn remove(&mut self, key: &SecretKey, expected: SecretVersion) -> Result<(), SecretError> {
            let mut values = self.0.lock().map_err(|_| SecretError::Unavailable)?;
            let (_, current) = values.get(key).ok_or(SecretError::NotFound)?;
            if *current != expected {
                return Err(SecretError::VersionConflict);
            }
            values.remove(key);
            Ok(())
        }
    }

    #[test]
    fn identity_is_generated_once_and_survives_vault_reconstruction() {
        let vault = FakeVault::default();
        let first = DeviceIdentityVault::new(vault.clone());
        let id = first.load_or_create("device-id-test").unwrap();
        assert_eq!(first.load_or_create("device-id-test").unwrap(), id);

        let after_restart = DeviceIdentityVault::new(vault);
        assert_eq!(after_restart.load_or_create("device-id-test").unwrap(), id);
        assert_eq!(id.len(), 36);
        assert!(id.starts_with("dev_"));
        assert!(id[4..].bytes().all(|byte| byte.is_ascii_hexdigit()));
    }

    #[test]
    fn local_api_identity_does_not_reuse_production_device_identity() {
        let vault = FakeVault::default();
        let production = DeviceIdentityVault::new(vault.clone());
        let local_api = DeviceIdentityVault::new(vault);

        let production_id = production
            .load_or_create("production-identity-test")
            .unwrap();
        let local_id = local_api
            .load_or_create_in_namespace(
                "buglogin-device-identity-local-api",
                "local-api-identity-test",
            )
            .unwrap();

        assert_ne!(local_id, production_id);
        assert_eq!(
            production
                .load_or_create("production-identity-test")
                .unwrap(),
            production_id
        );
        assert_eq!(
            local_api
                .load_or_create_in_namespace(
                    "buglogin-device-identity-local-api",
                    "local-api-identity-test",
                )
                .unwrap(),
            local_id
        );
    }

    #[test]
    fn qa_instance_namespaces_create_distinct_stable_device_ids() {
        let vault = FakeVault::default();
        let first = DeviceIdentityVault::new(vault.clone());
        let second = DeviceIdentityVault::new(vault);

        let first_id = first
            .load_or_create_in_namespace("buglogin-device-identity-instance-qa-a", "qa-a")
            .unwrap();
        let second_id = second
            .load_or_create_in_namespace("buglogin-device-identity-instance-qa-b", "qa-b")
            .unwrap();

        assert_ne!(first_id, second_id);
        assert_eq!(
            first
                .load_or_create_in_namespace(
                    "buglogin-device-identity-instance-qa-a",
                    "qa-a-reload",
                )
                .unwrap(),
            first_id
        );
        assert_eq!(
            second
                .load_or_create_in_namespace(
                    "buglogin-device-identity-instance-qa-b",
                    "qa-b-reload",
                )
                .unwrap(),
            second_id
        );
    }

    #[test]
    fn concurrent_first_start_converges_on_one_device_identity() {
        let vault = FakeVault::default();
        let first = DeviceIdentityVault::new(vault.clone());
        let second = DeviceIdentityVault::new(vault);
        let (left, right) = std::thread::scope(|scope| {
            let left = scope.spawn(|| first.load_or_create("parallel-bootstrap"));
            let right = scope.spawn(|| second.load_or_create("parallel-bootstrap"));
            (left.join().unwrap(), right.join().unwrap())
        });
        assert_eq!(left.unwrap(), right.unwrap());
    }

    #[test]
    fn identity_vault_failure_does_not_fall_back_to_an_ephemeral_id() {
        let identity = DeviceIdentityVault::new(buglogin_storage::secrets::UnavailableVault);
        assert_eq!(
            identity.load_or_create("device-id-test").unwrap_err().code,
            "secret_vault_unavailable"
        );
    }
}
