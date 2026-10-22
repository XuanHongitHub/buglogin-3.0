use serde::{Deserialize, Deserializer, Serialize, Serializer};
use uuid::Uuid;

macro_rules! id_type {
    ($name:ident) => {
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, IdError> {
                let value = value.into();
                validate(&value)?;
                Ok(Self(value))
            }

            pub fn from_str(value: &str) -> Result<Self, IdError> {
                Self::new(value)
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                Self::new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
            }
        }
    };
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IdError {
    Empty,
    TooShort,
    TooLong,
    InvalidStart,
    InvalidCharacter,
}

impl std::fmt::Display for IdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid canonical ID: {self:?}")
    }
}
impl std::error::Error for IdError {}

fn validate(value: &str) -> Result<(), IdError> {
    if value.is_empty() {
        return Err(IdError::Empty);
    }
    if value.len() > 128 {
        return Err(IdError::TooLong);
    }
    if !value.as_bytes()[0].is_ascii_alphanumeric() {
        return Err(IdError::InvalidStart);
    }
    if !value
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'-'))
    {
        return Err(IdError::InvalidCharacter);
    }
    Ok(())
}

id_type!(ProfileId);
id_type!(GroupId);
id_type!(ProxyId);
id_type!(RuntimeInstallId);
id_type!(VpnId);
id_type!(CredentialId);
id_type!(ExtensionId);
id_type!(ExtensionGroupId);
id_type!(ArtifactId);
id_type!(AccountId);
id_type!(OrganizationId);
id_type!(WorkspaceId);
id_type!(TeamId);
id_type!(MemberId);
id_type!(MemberInvitationId);
id_type!(ResourceId);
id_type!(GrantId);

/// Stable device identifiers accepted by authentication and encryption-key APIs.
/// Keep this syntax aligned with `DeviceIdentifier` in the REST contract.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct DeviceId(String);

impl DeviceId {
    pub fn new(value: impl Into<String>) -> Result<Self, IdError> {
        let value = value.into();
        if value.is_empty() {
            return Err(IdError::Empty);
        }
        if value.len() < 8 {
            return Err(IdError::TooShort);
        }
        if value.len() > 128 {
            return Err(IdError::TooLong);
        }
        if !value.as_bytes()[0].is_ascii_alphanumeric() {
            return Err(IdError::InvalidStart);
        }
        if !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        {
            return Err(IdError::InvalidCharacter);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod device_id_tests {
    use super::{DeviceId, IdError};

    #[test]
    fn accepts_desktop_generated_and_supported_device_ids() {
        assert_eq!(
            DeviceId::new("dev_0123456789abcdef0123456789abcdef")
                .unwrap()
                .as_str(),
            "dev_0123456789abcdef0123456789abcdef"
        );
        assert!(DeviceId::new("desktop-device-01").is_ok());
    }

    #[test]
    fn rejects_short_whitespace_control_and_non_ascii_device_ids() {
        assert_eq!(DeviceId::new("").unwrap_err(), IdError::Empty);
        assert_eq!(DeviceId::new("device1").unwrap_err(), IdError::TooShort);
        assert!(DeviceId::new("device id").is_err());
        assert!(DeviceId::new("device\n01").is_err());
        assert!(DeviceId::new("device-é1").is_err());
    }

    #[test]
    fn accepts_exact_limit_and_rejects_overlong_device_ids() {
        assert!(DeviceId::new(format!("d{}", "a".repeat(127))).is_ok());
        assert_eq!(
            DeviceId::new(format!("d{}", "a".repeat(128))).unwrap_err(),
            IdError::TooLong
        );
    }
}

pub const MIGRATION_UUID_NAMESPACE: Uuid = Uuid::from_u128(0xf5d2d691b71548b1bc162855ef99586b);

pub fn migration_uuid(
    source_system: &str,
    source_instance_id: &str,
    entity_type: &str,
    source_id: &str,
) -> Uuid {
    let name = format!("{source_system}:{source_instance_id}:{entity_type}:{source_id}");
    Uuid::new_v5(&MIGRATION_UUID_NAMESPACE, name.as_bytes())
}
