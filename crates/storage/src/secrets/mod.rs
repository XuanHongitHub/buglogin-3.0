use std::fmt;

pub const REDACTED: &str = "<redacted>";
pub const MAX_SECRET_BYTES: usize = 1024 * 1024;

#[derive(Clone, Eq, Hash, PartialEq)]
pub struct SecretKey {
    namespace: String,
    name: String,
}

impl SecretKey {
    pub fn new(namespace: impl Into<String>, name: impl Into<String>) -> Result<Self, SecretError> {
        let namespace = namespace.into();
        let name = name.into();
        validate_component(&namespace)?;
        validate_component(&name)?;
        Ok(Self { namespace, name })
    }

    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

impl fmt::Debug for SecretKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SecretKey")
            .field("namespace", &self.namespace)
            .field("name", &self.name)
            .finish()
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct SecretValue(Vec<u8>);

impl SecretValue {
    pub fn new(value: impl Into<Vec<u8>>) -> Result<Self, SecretError> {
        let value = value.into();
        if value.is_empty() {
            return Err(SecretError::EmptyValue);
        }
        if value.len() > MAX_SECRET_BYTES {
            return Err(SecretError::TooLarge);
        }
        Ok(Self(value))
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for SecretValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("SecretValue")
            .field(&REDACTED)
            .finish()
    }
}

impl fmt::Display for SecretValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(REDACTED)
    }
}

impl Drop for SecretValue {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SecretVersion(u64);

impl SecretVersion {
    pub const fn new(generation: u64) -> Option<Self> {
        if generation == 0 {
            None
        } else {
            Some(Self(generation))
        }
    }

    pub const fn generation(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RotationReceipt {
    pub previous: SecretVersion,
    pub current: SecretVersion,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SecretError {
    InvalidKey,
    EmptyValue,
    TooLarge,
    Unavailable,
    NotFound,
    AccessDenied,
    VersionConflict,
    OperationFailed,
}

impl SecretError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::InvalidKey => "invalid_secret_key",
            Self::EmptyValue => "empty_secret",
            Self::TooLarge => "secret_too_large",
            Self::Unavailable => "secret_vault_unavailable",
            Self::NotFound => "secret_not_found",
            Self::AccessDenied => "secret_access_denied",
            Self::VersionConflict => "secret_version_conflict",
            Self::OperationFailed => "secret_operation_failed",
        }
    }
}

impl fmt::Display for SecretError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

fn validate_component(value: &str) -> Result<(), SecretError> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        Err(SecretError::InvalidKey)
    } else {
        Ok(())
    }
}

pub trait OsVault {
    fn read(&self, key: &SecretKey) -> Result<(SecretValue, SecretVersion), SecretError>;

    fn create(&mut self, key: &SecretKey, value: SecretValue)
    -> Result<SecretVersion, SecretError>;

    fn rotate(
        &mut self,
        key: &SecretKey,
        expected: SecretVersion,
        replacement: SecretValue,
    ) -> Result<RotationReceipt, SecretError>;

    fn remove(&mut self, key: &SecretKey, expected: SecretVersion) -> Result<(), SecretError>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct UnavailableVault;

impl OsVault for UnavailableVault {
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
