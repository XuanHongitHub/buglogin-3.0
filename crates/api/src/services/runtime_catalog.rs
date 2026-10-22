use std::collections::BTreeMap;

use buglogin_browser::runtime::catalog::{Ed25519RuntimeSignatureVerifier, RuntimeCatalog};
use buglogin_domain::errors::Problem;
use serde::Deserialize;
use sha2::{Digest, Sha256};

const MAX_CATALOG_BYTES: usize = 2 * 1024 * 1024;
const MAX_TRUST_KEYS_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug)]
pub struct PublishedRuntimeCatalog {
    catalog: RuntimeCatalog,
    etag: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeTrustKey {
    pub key_id: String,
    pub public_key: String,
}

impl PublishedRuntimeCatalog {
    pub fn verify(catalog_bytes: &[u8], trust_keys_bytes: &[u8]) -> Result<Self, Problem> {
        if catalog_bytes.is_empty() || catalog_bytes.len() > MAX_CATALOG_BYTES {
            return Err(problem("runtime_catalog_size_invalid", 500));
        }
        if trust_keys_bytes.is_empty() || trust_keys_bytes.len() > MAX_TRUST_KEYS_BYTES {
            return Err(problem("runtime_trust_keys_size_invalid", 500));
        }
        let catalog: RuntimeCatalog = serde_json::from_slice(catalog_bytes)
            .map_err(|_| problem("runtime_catalog_invalid", 500))?;
        let trust_keys: Vec<RuntimeTrustKey> = serde_json::from_slice(trust_keys_bytes)
            .map_err(|_| problem("runtime_trust_keys_invalid", 500))?;
        let mut trusted_keys = BTreeMap::new();
        for key in trust_keys {
            if key.key_id.trim().is_empty() {
                return Err(problem("runtime_trust_keys_invalid", 500));
            }
            let public_key = decode_public_key(&key.public_key)?;
            if trusted_keys.insert(key.key_id, public_key).is_some() {
                return Err(problem("runtime_trust_keys_invalid", 500));
            }
        }
        let verifier = Ed25519RuntimeSignatureVerifier::new(trusted_keys)
            .map_err(|_| problem("runtime_trust_keys_invalid", 500))?;
        catalog
            .verify(&verifier)
            .map_err(|error| Problem::new(error.code, error.title, 500, "runtime-catalog"))?;
        let etag = format!("\"{:x}\"", Sha256::digest(catalog_bytes));
        Ok(Self { catalog, etag })
    }

    pub fn catalog(&self) -> &RuntimeCatalog {
        &self.catalog
    }

    pub fn etag(&self) -> &str {
        &self.etag
    }
}

fn decode_public_key(value: &str) -> Result<[u8; 32], Problem> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(problem("runtime_trust_keys_invalid", 500));
    }
    let mut key = [0_u8; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        key[index] = (hex_digit(pair[0])? << 4) | hex_digit(pair[1])?;
    }
    Ok(key)
}

fn hex_digit(value: u8) -> Result<u8, Problem> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        _ => Err(problem("runtime_trust_keys_invalid", 500)),
    }
}

fn problem(code: &'static str, status: u16) -> Problem {
    Problem::new(
        code,
        "Runtime catalog configuration is invalid",
        status,
        "runtime-catalog",
    )
}

#[cfg(test)]
mod tests {
    use super::PublishedRuntimeCatalog;

    #[test]
    fn catalog_publication_rejects_unsigned_or_untrusted_releases() {
        let catalog = br#"{"schemaVersion":1,"releases":[{"runtime":"wayfern","version":"1.2.3","platform":"windows","architecture":"x64","relativePath":"wayfern/wayfern.exe","source":"https://download.example/wayfern.exe","sha256":"abababababababababababababababababababababababababababababababab","sizeBytes":123,"keyId":"runtime-key-1","signature":"00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000"}]}"#;
        let trust_keys = br#"[{"keyId":"runtime-key-1","publicKey":"abababababababababababababababababababababababababababababababab"}]"#;

        let error = PublishedRuntimeCatalog::verify(catalog, trust_keys).unwrap_err();

        assert_eq!(error.code, "runtime_signature_invalid");
    }

    #[test]
    fn catalog_publication_rejects_empty_catalogs() {
        let catalog = br#"{"schemaVersion":1,"releases":[]}"#;
        let trust_keys = br#"[{"keyId":"runtime-key-1","publicKey":"abababababababababababababababababababababababababababababababab"}]"#;

        let error = PublishedRuntimeCatalog::verify(catalog, trust_keys).unwrap_err();

        assert_eq!(error.code, "runtime_catalog_empty");
    }

    #[test]
    fn catalog_publication_rejects_unknown_schema_fields() {
        let catalog = br#"{"schemaVersion":1,"releases":[],"debug":true}"#;
        let trust_keys = br#"[{"keyId":"runtime-key-1","publicKey":"abababababababababababababababababababababababababababababababab"}]"#;

        let error = PublishedRuntimeCatalog::verify(catalog, trust_keys).unwrap_err();

        assert_eq!(error.code, "runtime_catalog_invalid");
    }
}
