use buglogin_domain::errors::Problem;
use buglogin_storage::blob::checksum;
use buglogin_sync::crypto::{AuthenticatedChunk, ClientKey};
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, ZeroizeOnDrop};

const EXPORT_SCHEMA: &str = "buglogin.legacy-account-credentials/1";
const PACKAGE_SCHEMA: &str = "buglogin.encrypted-account-passwords/1";
const EXPORT_AAD: &[u8] = b"buglogin/legacy-account-credentials/1";
const PACKAGE_AAD: &[u8] = b"buglogin/encrypted-account-passwords/1";
pub const PACKAGE_PATH: &str = "encrypted/account-passwords.json";

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize, Zeroize, ZeroizeOnDrop)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LegacyScryptCredential {
    pub source_user_id: String,
    pub salt: String,
    pub hash: String,
}

impl std::fmt::Debug for LegacyScryptCredential {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LegacyScryptCredential")
            .field("source_user_id", &self.source_user_id)
            .field("salt", &"<redacted>")
            .field("hash", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EncryptedLegacyCredentials {
    pub schema: String,
    pub record_count: usize,
    pub bytes: u64,
    pub ciphertext_sha256: String,
    pub ciphertext: Vec<u8>,
}

impl std::fmt::Debug for EncryptedLegacyCredentials {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EncryptedLegacyCredentials")
            .field("schema", &self.schema)
            .field("record_count", &self.record_count)
            .field("bytes", &self.bytes)
            .field("ciphertext_sha256", &self.ciphertext_sha256)
            .field("ciphertext", &"<redacted>")
            .finish()
    }
}

pub fn encrypt_source_credentials(
    credentials: &[LegacyScryptCredential],
    key: &ClientKey,
) -> Result<EncryptedLegacyCredentials, Problem> {
    validate_credentials(credentials)?;
    let mut plaintext = serde_json::to_vec(credentials).map_err(|_| invalid())?;
    let encrypted = AuthenticatedChunk::encrypt(key, &plaintext, EXPORT_AAD);
    plaintext.zeroize();
    let ciphertext = encrypted?.encoded();
    Ok(EncryptedLegacyCredentials {
        schema: EXPORT_SCHEMA.into(),
        record_count: credentials.len(),
        bytes: ciphertext.len() as u64,
        ciphertext_sha256: checksum(&ciphertext),
        ciphertext,
    })
}

pub fn decrypt_source_credentials(
    encrypted: &EncryptedLegacyCredentials,
    key: &ClientKey,
) -> Result<Vec<LegacyScryptCredential>, Problem> {
    decrypt(encrypted, EXPORT_SCHEMA, EXPORT_AAD, key)
}

pub fn encrypt_package_credentials(
    credentials: &[LegacyScryptCredential],
    key: &ClientKey,
) -> Result<EncryptedLegacyCredentials, Problem> {
    validate_credentials(credentials)?;
    let mut plaintext = serde_json::to_vec(credentials).map_err(|_| invalid())?;
    let encrypted = AuthenticatedChunk::encrypt(key, &plaintext, PACKAGE_AAD);
    plaintext.zeroize();
    let ciphertext = encrypted?.encoded();
    Ok(EncryptedLegacyCredentials {
        schema: PACKAGE_SCHEMA.into(),
        record_count: credentials.len(),
        bytes: ciphertext.len() as u64,
        ciphertext_sha256: checksum(&ciphertext),
        ciphertext,
    })
}

pub fn decrypt_package_credentials(
    encrypted: &EncryptedLegacyCredentials,
    key: &ClientKey,
) -> Result<Vec<LegacyScryptCredential>, Problem> {
    decrypt(encrypted, PACKAGE_SCHEMA, PACKAGE_AAD, key)
}

fn decrypt(
    encrypted: &EncryptedLegacyCredentials,
    expected_schema: &str,
    aad: &[u8],
    key: &ClientKey,
) -> Result<Vec<LegacyScryptCredential>, Problem> {
    if encrypted.schema != expected_schema
        || encrypted.bytes != encrypted.ciphertext.len() as u64
        || encrypted.ciphertext.is_empty()
        || encrypted.ciphertext_sha256 != checksum(&encrypted.ciphertext)
    {
        return Err(invalid());
    }
    let mut plaintext = AuthenticatedChunk::decode(&encrypted.ciphertext)?.decrypt(key, aad)?;
    let credentials_result = serde_json::from_slice(&plaintext).map_err(|_| invalid());
    plaintext.zeroize();
    let credentials: Vec<LegacyScryptCredential> = credentials_result?;
    if credentials.len() != encrypted.record_count {
        return Err(invalid());
    }
    validate_credentials(&credentials)?;
    Ok(credentials)
}

fn validate_credentials(credentials: &[LegacyScryptCredential]) -> Result<(), Problem> {
    let mut user_ids = std::collections::BTreeSet::new();
    for credential in credentials {
        if credential.source_user_id.trim().is_empty()
            || credential.source_user_id.len() > 256
            || !user_ids.insert(&credential.source_user_id)
            || credential.salt.len() != 32
            || !credential.salt.bytes().all(|byte| byte.is_ascii_hexdigit())
            || credential.hash.len() != 128
            || !credential.hash.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(invalid());
        }
    }
    Ok(())
}

fn invalid() -> Problem {
    Problem::new(
        "migration_legacy_credentials_invalid",
        "Encrypted legacy credentials are invalid",
        422,
        "migration",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_and_package_credentials_round_trip_only_as_ciphertext() {
        let key = ClientKey::from_bytes([7; 32]);
        let credentials = vec![LegacyScryptCredential {
            source_user_id: "legacy-user-1".into(),
            salt: "0123456789abcdef0123456789abcdef".into(),
            hash: "ab".repeat(64),
        }];
        let source = encrypt_source_credentials(&credentials, &key).unwrap();
        let source_json = serde_json::to_vec(&source).unwrap();
        assert!(!String::from_utf8_lossy(&source_json).contains(&credentials[0].hash));
        assert_eq!(
            decrypt_source_credentials(&source, &key).unwrap(),
            credentials
        );

        let package = encrypt_package_credentials(&credentials, &key).unwrap();
        let package_json = serde_json::to_vec(&package).unwrap();
        assert!(!String::from_utf8_lossy(&package_json).contains(&credentials[0].salt));
        assert_eq!(
            decrypt_package_credentials(&package, &key).unwrap(),
            credentials
        );

        let mut tampered = package;
        tampered.ciphertext[0] ^= 1;
        assert!(decrypt_package_credentials(&tampered, &key).is_err());
    }
}
