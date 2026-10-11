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
id_type!(ResourceId);
id_type!(GrantId);

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
