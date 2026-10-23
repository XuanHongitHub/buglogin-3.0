use std::collections::{BTreeMap, BTreeSet};

use buglogin_domain::{
    entities::VpnKind,
    errors::Problem,
    ids::{CredentialId, VpnId},
};
use buglogin_storage::{blob::checksum, secrets::MAX_SECRET_BYTES};
use buglogin_sync::crypto::{AuthenticatedChunk, ClientKey};
use serde::{Deserialize, Serialize};

use crate::package::NeutralPackage;

const INDEX_PATH: &str = "encrypted/vpn-configs/index.json";
const OBJECT_PREFIX: &str = "encrypted/vpn-configs";
const INDEX_SCHEMA: &str = "buglogin.encrypted-vpn-configs/2";
const SECRET_SCHEMA: &str = "buglogin.vpn-config-secret/1";

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VpnConfigSecret {
    pub schema: String,
    pub config_ref: String,
    pub vpn_id: String,
    pub kind: VpnKind,
    pub config_sha256: String,
    pub config_data: String,
}

impl std::fmt::Debug for VpnConfigSecret {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VpnConfigSecret")
            .field("schema", &self.schema)
            .field("config_ref", &self.config_ref)
            .field("vpn_id", &self.vpn_id)
            .field("kind", &self.kind)
            .field("config_sha256", &self.config_sha256)
            .field("config_data", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EncryptedVpnConfigIndex {
    pub schema: String,
    pub configs: Vec<EncryptedVpnConfig>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EncryptedVpnConfig {
    pub config_ref: String,
    pub vpn_id: String,
    pub kind: VpnKind,
    pub object_key: String,
    pub ciphertext_sha256: String,
    pub bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EncryptedVpnConfigs {
    pub index: EncryptedVpnConfigIndex,
    pub objects: BTreeMap<String, Vec<u8>>,
}

pub fn encrypt_vpn_configs(
    configs: &[VpnConfigSecret],
    key: &ClientKey,
) -> Result<EncryptedVpnConfigs, Problem> {
    validate_configs(configs)?;
    let mut index = Vec::with_capacity(configs.len());
    let mut objects = BTreeMap::new();
    for config in configs {
        let plaintext = serde_json::to_vec(config).map_err(|_| {
            vpn_problem(
                "migration_vpn_config_serialization_failed",
                "VPN config serialization failed",
            )
        })?;
        if plaintext.len() > MAX_SECRET_BYTES {
            return Err(vpn_problem(
                "migration_vpn_config_too_large",
                "VPN config exceeds the native vault limit",
            ));
        }
        let object_key = object_key(&config.config_ref);
        let authenticated = AuthenticatedChunk::encrypt(key, &plaintext, object_key.as_bytes())?;
        let bytes = authenticated.encoded();
        index.push(EncryptedVpnConfig {
            config_ref: config.config_ref.clone(),
            vpn_id: config.vpn_id.clone(),
            kind: config.kind.clone(),
            object_key: object_key.clone(),
            ciphertext_sha256: checksum(&bytes),
            bytes: bytes.len() as u64,
        });
        objects.insert(object_key, bytes);
    }
    Ok(EncryptedVpnConfigs {
        index: EncryptedVpnConfigIndex {
            schema: INDEX_SCHEMA.into(),
            configs: index,
        },
        objects,
    })
}

pub fn decrypt_packaged_vpn_configs(
    package: &NeutralPackage,
    key: &ClientKey,
) -> Result<Vec<VpnConfigSecret>, Problem> {
    let Some(index_bytes) = package.file(INDEX_PATH) else {
        if package
            .files()
            .keys()
            .any(|path| path.starts_with(&format!("{OBJECT_PREFIX}/")))
        {
            return Err(vpn_problem(
                "migration_vpn_config_index_missing",
                "VPN config index is missing",
            ));
        }
        return Ok(Vec::new());
    };
    let index: EncryptedVpnConfigIndex = serde_json::from_slice(index_bytes).map_err(|_| {
        vpn_problem(
            "migration_vpn_config_index_invalid",
            "VPN config index is invalid",
        )
    })?;
    validate_index(package, &index)?;

    let mut configs = Vec::with_capacity(index.configs.len());
    for entry in index.configs {
        let ciphertext = package.file(&entry.object_key).ok_or_else(|| {
            vpn_problem(
                "migration_vpn_config_object_missing",
                "VPN config object is missing",
            )
        })?;
        if ciphertext.len() as u64 != entry.bytes || checksum(ciphertext) != entry.ciphertext_sha256
        {
            return Err(vpn_problem(
                "migration_vpn_config_object_tampered",
                "VPN config object failed integrity verification",
            ));
        }
        let authenticated = AuthenticatedChunk::decode(ciphertext)?;
        let plaintext = authenticated.decrypt(key, entry.object_key.as_bytes())?;
        if plaintext.len() > MAX_SECRET_BYTES {
            return Err(vpn_problem(
                "migration_vpn_config_plaintext_invalid",
                "VPN config plaintext is invalid",
            ));
        }
        let config: VpnConfigSecret = serde_json::from_slice(&plaintext)
            .map_err(|_| vpn_problem("migration_vpn_config_invalid", "VPN config is invalid"))?;
        if config.config_ref != entry.config_ref
            || config.vpn_id != entry.vpn_id
            || config.kind != entry.kind
        {
            return Err(vpn_problem(
                "migration_vpn_config_index_mismatch",
                "VPN config does not match its index",
            ));
        }
        configs.push(config);
    }
    validate_configs(&configs)?;
    Ok(configs)
}

pub(crate) const fn index_path() -> &'static str {
    INDEX_PATH
}

fn validate_index(
    package: &NeutralPackage,
    index: &EncryptedVpnConfigIndex,
) -> Result<(), Problem> {
    if index.schema != INDEX_SCHEMA {
        return Err(vpn_problem(
            "migration_vpn_config_index_invalid",
            "VPN config index is invalid",
        ));
    }
    let mut refs = BTreeSet::new();
    let mut vpn_ids = BTreeSet::new();
    let mut expected_objects = BTreeSet::new();
    for entry in &index.configs {
        if CredentialId::new(entry.config_ref.clone()).is_err()
            || VpnId::new(entry.vpn_id.clone()).is_err()
            || !refs.insert(entry.config_ref.as_str())
            || !vpn_ids.insert(entry.vpn_id.as_str())
            || entry.object_key != object_key(&entry.config_ref)
            || !is_sha256(&entry.ciphertext_sha256)
            || entry.bytes == 0
            || !expected_objects.insert(entry.object_key.as_str())
        {
            return Err(vpn_problem(
                "migration_vpn_config_index_invalid",
                "VPN config index is invalid",
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
        return Err(vpn_problem(
            "migration_vpn_config_object_set_mismatch",
            "VPN config objects do not match the index",
        ));
    }
    Ok(())
}

fn validate_configs(configs: &[VpnConfigSecret]) -> Result<(), Problem> {
    let mut refs = BTreeSet::new();
    let mut vpn_ids = BTreeSet::new();
    for config in configs {
        if config.schema != SECRET_SCHEMA
            || CredentialId::new(config.config_ref.clone()).is_err()
            || VpnId::new(config.vpn_id.clone()).is_err()
            || !refs.insert(config.config_ref.as_str())
            || !vpn_ids.insert(config.vpn_id.as_str())
            || config.config_data.is_empty()
            || !is_sha256(&config.config_sha256)
            || checksum(config.config_data.as_bytes()) != config.config_sha256
        {
            return Err(vpn_problem(
                "migration_vpn_config_invalid",
                "VPN config is invalid",
            ));
        }
    }
    Ok(())
}

fn object_key(config_ref: &str) -> String {
    format!("{OBJECT_PREFIX}/{config_ref}/0")
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn vpn_problem(code: &'static str, title: &'static str) -> Problem {
    Problem::new(code, title, 422, "migration")
}
