use std::collections::{BTreeMap, BTreeSet};

use buglogin_domain::errors::Problem;
use buglogin_storage::blob::checksum;
use buglogin_sync::crypto::{AuthenticatedChunk, ClientKey};
use serde::{Deserialize, Serialize};

use crate::package::NeutralPackage;

const INDEX_PATH: &str = "encrypted/proxy-credentials/index.json";
const OBJECT_PREFIX: &str = "encrypted/proxy-credentials";
const INDEX_SCHEMA: &str = "buglogin.encrypted-proxy-credentials/2";

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyCredentialSecret {
    pub credential_id: String,
    pub username: Option<String>,
    pub password: Option<String>,
}

impl std::fmt::Debug for ProxyCredentialSecret {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProxyCredentialSecret")
            .field("credential_id", &self.credential_id)
            .field("username", &self.username.as_ref().map(|_| "<redacted>"))
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EncryptedProxyCredentialIndex {
    pub schema: String,
    pub credentials: Vec<EncryptedProxyCredential>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EncryptedProxyCredential {
    pub credential_id: String,
    pub object_key: String,
    pub ciphertext_sha256: String,
    pub bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EncryptedProxyCredentials {
    pub index: EncryptedProxyCredentialIndex,
    pub objects: BTreeMap<String, Vec<u8>>,
}

pub fn encrypt_proxy_credentials(
    credentials: &[ProxyCredentialSecret],
    key: &ClientKey,
) -> Result<EncryptedProxyCredentials, Problem> {
    validate_credentials(credentials)?;
    let mut index = Vec::with_capacity(credentials.len());
    let mut objects = BTreeMap::new();
    for credential in credentials {
        let plaintext = serde_json::to_vec(credential).map_err(|_| {
            secret_problem(
                "migration_proxy_credential_serialization_failed",
                "Proxy credential serialization failed",
            )
        })?;
        let object_key = object_key(&credential.credential_id);
        let bytes = AuthenticatedChunk::encrypt(key, &plaintext, object_key.as_bytes())?.encoded();
        index.push(EncryptedProxyCredential {
            credential_id: credential.credential_id.clone(),
            object_key: object_key.clone(),
            ciphertext_sha256: checksum(&bytes),
            bytes: bytes.len() as u64,
        });
        objects.insert(object_key, bytes);
    }
    Ok(EncryptedProxyCredentials {
        index: EncryptedProxyCredentialIndex {
            schema: INDEX_SCHEMA.into(),
            credentials: index,
        },
        objects,
    })
}

pub fn decrypt_packaged_proxy_credentials(
    package: &NeutralPackage,
    key: &ClientKey,
) -> Result<Vec<ProxyCredentialSecret>, Problem> {
    let Some(index_bytes) = package.file(INDEX_PATH) else {
        if package
            .files()
            .keys()
            .any(|path| path.starts_with(&format!("{OBJECT_PREFIX}/")))
        {
            return Err(secret_problem(
                "migration_proxy_credential_index_missing",
                "Proxy credential index is missing",
            ));
        }
        return Ok(Vec::new());
    };
    let index: EncryptedProxyCredentialIndex =
        serde_json::from_slice(index_bytes).map_err(|_| {
            secret_problem(
                "migration_proxy_credential_index_invalid",
                "Proxy credential index is invalid",
            )
        })?;
    validate_index(package, &index)?;

    let mut credentials = Vec::with_capacity(index.credentials.len());
    for entry in index.credentials {
        let ciphertext = package.file(&entry.object_key).ok_or_else(|| {
            secret_problem(
                "migration_proxy_credential_object_missing",
                "Proxy credential object is missing",
            )
        })?;
        if ciphertext.len() as u64 != entry.bytes || checksum(ciphertext) != entry.ciphertext_sha256
        {
            return Err(secret_problem(
                "migration_proxy_credential_object_tampered",
                "Proxy credential object failed integrity verification",
            ));
        }
        let plaintext =
            AuthenticatedChunk::decode(ciphertext)?.decrypt(key, entry.object_key.as_bytes())?;
        let credential: ProxyCredentialSecret =
            serde_json::from_slice(&plaintext).map_err(|_| {
                secret_problem(
                    "migration_proxy_credential_invalid",
                    "Proxy credential is invalid",
                )
            })?;
        if credential.credential_id != entry.credential_id {
            return Err(secret_problem(
                "migration_proxy_credential_id_mismatch",
                "Proxy credential ID does not match its index",
            ));
        }
        credentials.push(credential);
    }
    validate_credentials(&credentials)?;
    Ok(credentials)
}

pub(crate) const fn index_path() -> &'static str {
    INDEX_PATH
}

fn validate_index(
    package: &NeutralPackage,
    index: &EncryptedProxyCredentialIndex,
) -> Result<(), Problem> {
    if index.schema != INDEX_SCHEMA {
        return Err(secret_problem(
            "migration_proxy_credential_index_invalid",
            "Proxy credential index is invalid",
        ));
    }
    let mut credential_ids = BTreeSet::new();
    let mut expected_objects = BTreeSet::new();
    for entry in &index.credentials {
        if buglogin_domain::ids::CredentialId::new(entry.credential_id.clone()).is_err()
            || !credential_ids.insert(entry.credential_id.as_str())
            || entry.object_key != object_key(&entry.credential_id)
            || !is_sha256(&entry.ciphertext_sha256)
            || entry.bytes == 0
            || !expected_objects.insert(entry.object_key.as_str())
        {
            return Err(secret_problem(
                "migration_proxy_credential_index_invalid",
                "Proxy credential index is invalid",
            ));
        }
    }
    let actual_objects = package
        .files()
        .keys()
        .filter(|path| {
            path.starts_with(&format!("{OBJECT_PREFIX}/")) && path.as_str() != INDEX_PATH
        })
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if actual_objects != expected_objects {
        return Err(secret_problem(
            "migration_proxy_credential_object_set_mismatch",
            "Proxy credential objects do not match the index",
        ));
    }
    Ok(())
}

fn validate_credentials(credentials: &[ProxyCredentialSecret]) -> Result<(), Problem> {
    let mut ids = BTreeSet::new();
    for credential in credentials {
        if buglogin_domain::ids::CredentialId::new(credential.credential_id.clone()).is_err()
            || !ids.insert(credential.credential_id.as_str())
            || credential
                .username
                .as_ref()
                .is_some_and(|value| value.is_empty() || value.len() > 4096)
            || credential
                .password
                .as_ref()
                .is_some_and(|value| value.is_empty() || value.len() > 4096)
            || (credential.username.is_none() && credential.password.is_none())
        {
            return Err(secret_problem(
                "migration_proxy_credential_invalid",
                "Proxy credential is invalid",
            ));
        }
    }
    Ok(())
}

fn object_key(credential_id: &str) -> String {
    format!("{OBJECT_PREFIX}/{credential_id}/0")
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn secret_problem(code: &'static str, title: &'static str) -> Problem {
    Problem::new(code, title, 422, "migration")
}
