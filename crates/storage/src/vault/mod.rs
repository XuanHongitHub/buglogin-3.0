use crate::secrets::{
    OsVault, RotationReceipt, SecretError, SecretKey, SecretValue, SecretVersion,
};

#[derive(Clone, Copy, Debug, Default)]
pub struct NativeVault;

impl NativeVault {
    pub const fn new() -> Self {
        Self
    }
}

#[cfg(any(windows, target_os = "macos"))]
mod supported {
    use super::*;

    const SERVICE: &str = "BugLogin";
    const MAGIC: &[u8; 4] = b"BLV1";
    const HEADER_LEN: usize = 12;

    impl NativeVault {
        fn entry(key: &SecretKey) -> Result<keyring::Entry, SecretError> {
            keyring::Entry::new(SERVICE, &format!("{}.{}", key.namespace(), key.name()))
                .map_err(map_error)
        }
    }

    impl OsVault for NativeVault {
        fn read(&self, key: &SecretKey) -> Result<(SecretValue, SecretVersion), SecretError> {
            decode(&Self::entry(key)?.get_secret().map_err(map_error)?)
        }

        fn create(
            &mut self,
            key: &SecretKey,
            value: SecretValue,
        ) -> Result<SecretVersion, SecretError> {
            let entry = Self::entry(key)?;
            match entry.get_secret() {
                Ok(_) => return Err(SecretError::VersionConflict),
                Err(keyring::Error::NoEntry) => {}
                Err(error) => return Err(map_error(error)),
            }
            let version = SecretVersion::new(1).ok_or(SecretError::OperationFailed)?;
            entry
                .set_secret(&encode(version, &value))
                .map_err(map_error)?;
            Ok(version)
        }

        fn rotate(
            &mut self,
            key: &SecretKey,
            expected: SecretVersion,
            replacement: SecretValue,
        ) -> Result<RotationReceipt, SecretError> {
            let entry = Self::entry(key)?;
            let (_, current) = decode(&entry.get_secret().map_err(map_error)?)?;
            if current != expected {
                return Err(SecretError::VersionConflict);
            }
            let next = SecretVersion::new(
                current
                    .generation()
                    .checked_add(1)
                    .ok_or(SecretError::OperationFailed)?,
            )
            .ok_or(SecretError::OperationFailed)?;
            entry
                .set_secret(&encode(next, &replacement))
                .map_err(map_error)?;
            Ok(RotationReceipt {
                previous: current,
                current: next,
            })
        }

        fn remove(&mut self, key: &SecretKey, expected: SecretVersion) -> Result<(), SecretError> {
            let entry = Self::entry(key)?;
            let (_, current) = decode(&entry.get_secret().map_err(map_error)?)?;
            if current != expected {
                return Err(SecretError::VersionConflict);
            }
            entry.delete_credential().map_err(map_error)
        }
    }

    fn encode(version: SecretVersion, value: &SecretValue) -> Vec<u8> {
        let mut encoded = Vec::with_capacity(HEADER_LEN + value.as_bytes().len());
        encoded.extend_from_slice(MAGIC);
        encoded.extend_from_slice(&version.generation().to_be_bytes());
        encoded.extend_from_slice(value.as_bytes());
        encoded
    }

    fn decode(bytes: &[u8]) -> Result<(SecretValue, SecretVersion), SecretError> {
        if bytes.len() < HEADER_LEN || &bytes[..MAGIC.len()] != MAGIC {
            return Err(SecretError::OperationFailed);
        }
        let generation = u64::from_be_bytes(
            bytes[4..12]
                .try_into()
                .map_err(|_| SecretError::OperationFailed)?,
        );
        let version = SecretVersion::new(generation).ok_or(SecretError::OperationFailed)?;
        Ok((SecretValue::new(bytes[HEADER_LEN..].to_vec())?, version))
    }

    fn map_error(error: keyring::Error) -> SecretError {
        match error {
            keyring::Error::NoEntry => SecretError::NotFound,
            keyring::Error::NoStorageAccess(_) => SecretError::AccessDenied,
            keyring::Error::PlatformFailure(_)
            | keyring::Error::NoDefaultStore
            | keyring::Error::NotSupportedByStore(_) => SecretError::Unavailable,
            _ => SecretError::OperationFailed,
        }
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
impl OsVault for NativeVault {
    fn read(&self, _key: &SecretKey) -> Result<(SecretValue, SecretVersion), SecretError> {
        Err(SecretError::Unavailable)
    }

    fn create(
        &mut self,
        _key: &SecretKey,
        _value: SecretValue,
    ) -> Result<SecretVersion, SecretError> {
        Err(SecretError::Unavailable)
    }

    fn rotate(
        &mut self,
        _key: &SecretKey,
        _expected: SecretVersion,
        _replacement: SecretValue,
    ) -> Result<RotationReceipt, SecretError> {
        Err(SecretError::Unavailable)
    }

    fn remove(&mut self, _key: &SecretKey, _expected: SecretVersion) -> Result<(), SecretError> {
        Err(SecretError::Unavailable)
    }
}
