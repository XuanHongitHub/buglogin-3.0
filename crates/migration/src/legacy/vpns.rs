use std::{collections::BTreeSet, fmt, fs, path::Path};

use buglogin_domain::{errors::Problem, ids::migration_uuid};
use buglogin_sync::crypto::{AuthenticatedChunk, ClientKey};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use super::ProfileSourceRef;

const STORAGE_VERSION: u32 = 1;
const KEY_BYTES: usize = 32;
const NONCE_BYTES: usize = 12;
const TAG_BYTES: usize = 16;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct NeutralVpnData {
    pub name: String,
    pub kind: String,
    pub config_ref: String,
    pub last_used_at: Option<String>,
    pub sync_mode: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct NeutralVpnRecord {
    pub schema: String,
    pub source_ref: ProfileSourceRef,
    pub id: String,
    pub revision: u32,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub data: NeutralVpnData,
    pub extensions: Map<String, Value>,
}

#[derive(Clone, Eq, PartialEq)]
pub struct VpnConfigSecret {
    pub config_ref: String,
    pub vpn_id: String,
    pub kind: String,
    pub config_sha256: String,
    config_data: String,
}

impl VpnConfigSecret {
    pub fn config_data(&self) -> &str {
        &self.config_data
    }
}

impl fmt::Debug for VpnConfigSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VpnConfigSecret")
            .field("config_ref", &self.config_ref)
            .field("vpn_id", &self.vpn_id)
            .field("kind", &self.kind)
            .field("config_sha256", &self.config_sha256)
            .field("config_data", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LegacyVpnExport {
    pub records: Vec<NeutralVpnRecord>,
    pub configs: Vec<VpnConfigSecret>,
}

#[derive(Deserialize)]
struct LegacyVpnStorage {
    version: u32,
    configs: Vec<StoredVpnConfig>,
}

#[derive(Deserialize)]
struct StoredVpnConfig {
    id: String,
    name: String,
    vpn_type: LegacyVpnType,
    encrypted_data: String,
    nonce: String,
    created_at: i64,
    last_used: Option<i64>,
    #[serde(default)]
    sync_enabled: bool,
    #[serde(default)]
    last_sync: Option<u64>,
}

#[derive(Deserialize)]
enum LegacyVpnType {
    WireGuard,
    OpenVPN,
}

pub fn map_legacy_vpns(
    source_root: &Path,
    source_instance_id: &str,
) -> Result<LegacyVpnExport, Problem> {
    if source_instance_id.trim().is_empty() {
        return Err(invalid("source instance ID"));
    }
    let vpn_root = source_root.join("vpn");
    let storage_path = vpn_root.join("vpn_configs.json");
    if !storage_path.exists() {
        return Ok(LegacyVpnExport {
            records: Vec::new(),
            configs: Vec::new(),
        });
    }
    reject_symlink(&vpn_root)?;
    reject_symlink(&storage_path)?;
    let storage: LegacyVpnStorage = serde_json::from_slice(
        &fs::read(&storage_path).map_err(|error| io_problem("vpn/vpn_configs.json", error))?,
    )
    .map_err(|_| invalid("VPN storage JSON"))?;
    if storage.version != STORAGE_VERSION {
        return Err(invalid("VPN storage version"));
    }
    if storage.configs.is_empty() {
        return Ok(LegacyVpnExport {
            records: Vec::new(),
            configs: Vec::new(),
        });
    }

    let key_path = vpn_root.join(".vpn_key");
    reject_symlink(&key_path)?;
    let key_bytes = fs::read(&key_path).map_err(|error| io_problem("vpn/.vpn_key", error))?;
    let key: [u8; KEY_BYTES] = key_bytes
        .try_into()
        .map_err(|_| invalid("VPN encryption key"))?;
    let key = ClientKey::from_bytes(key);

    let mut source_ids = BTreeSet::new();
    let mut nonces = BTreeSet::new();
    let mut records = Vec::with_capacity(storage.configs.len());
    let mut configs = Vec::with_capacity(storage.configs.len());
    for stored in storage.configs {
        let source_id = required(&stored.id, "VPN ID")?;
        if !source_ids.insert(source_id.to_owned()) {
            return Err(invalid("duplicate VPN ID"));
        }
        let name = required(&stored.name, "VPN name")?.to_owned();
        let nonce = decode_base64(&stored.nonce, "VPN nonce")?;
        let nonce: [u8; NONCE_BYTES] = nonce.try_into().map_err(|_| invalid("VPN nonce length"))?;
        if !nonces.insert(nonce) {
            return Err(invalid("duplicate VPN nonce"));
        }
        let encrypted = decode_base64(&stored.encrypted_data, "VPN ciphertext")?;
        if encrypted.len() <= TAG_BYTES {
            return Err(invalid("VPN ciphertext"));
        }
        let tag_offset = encrypted.len() - TAG_BYTES;
        let mut tag = [0_u8; TAG_BYTES];
        tag.copy_from_slice(&encrypted[tag_offset..]);
        let plaintext = AuthenticatedChunk {
            version: 1,
            nonce,
            ciphertext: encrypted[..tag_offset].to_vec(),
            tag,
        }
        .decrypt(&key, &[])
        .map_err(|_| invalid("VPN ciphertext authentication"))?;
        let config_data = String::from_utf8(plaintext).map_err(|_| invalid("VPN config UTF-8"))?;
        required(&config_data, "VPN config")?;

        let kind = match stored.vpn_type {
            LegacyVpnType::WireGuard => {
                validate_wireguard(&config_data)?;
                "wireguard"
            }
            LegacyVpnType::OpenVPN => {
                validate_openvpn(&config_data)?;
                "openvpn"
            }
        };
        let vpn_id = mapped_id(source_instance_id, "vpn", source_id);
        let config_ref = mapped_id(source_instance_id, "vpn-config", source_id);
        let created_at = unix_timestamp(stored.created_at, "VPN created timestamp")?;
        let last_used_at = stored
            .last_used
            .map(|value| unix_timestamp(value, "VPN last-used timestamp"))
            .transpose()?;
        let mut extensions = Map::new();
        extensions.insert("legacyStorageVersion".into(), Value::from(STORAGE_VERSION));
        if let Some(last_sync) = stored.last_sync {
            extensions.insert("legacyLastSync".into(), Value::from(last_sync));
        }
        records.push(NeutralVpnRecord {
            schema: "buglogin.vpn/1".into(),
            source_ref: source_ref(source_instance_id, source_id),
            id: vpn_id.clone(),
            revision: 1,
            created_at: Some(created_at.clone()),
            updated_at: Some(created_at),
            data: NeutralVpnData {
                name,
                kind: kind.into(),
                config_ref: config_ref.clone(),
                last_used_at,
                sync_mode: if stored.sync_enabled {
                    "managed".into()
                } else {
                    "local".into()
                },
            },
            extensions,
        });
        configs.push(VpnConfigSecret {
            config_ref,
            vpn_id,
            kind: kind.into(),
            config_sha256: buglogin_storage::blob::checksum(config_data.as_bytes()),
            config_data,
        });
    }
    records.sort_by(|left, right| left.source_ref.id.cmp(&right.source_ref.id));
    configs.sort_by(|left, right| left.vpn_id.cmp(&right.vpn_id));
    Ok(LegacyVpnExport { records, configs })
}

fn validate_wireguard(config: &str) -> Result<(), Problem> {
    let mut section = "";
    let mut interface = BTreeSet::new();
    let mut peer = BTreeSet::new();
    for line in config.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if line.eq_ignore_ascii_case("[Interface]") {
            section = "interface";
            continue;
        }
        if line.eq_ignore_ascii_case("[Peer]") {
            section = "peer";
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(invalid("WireGuard config"));
        };
        if value.trim().is_empty() {
            return Err(invalid("WireGuard config value"));
        }
        match section {
            "interface" => {
                interface.insert(key.trim().to_ascii_lowercase());
            }
            "peer" => {
                peer.insert(key.trim().to_ascii_lowercase());
            }
            _ => return Err(invalid("WireGuard section")),
        }
    }
    if !["privatekey", "address"]
        .into_iter()
        .all(|field| interface.contains(field))
        || !["publickey", "endpoint"]
            .into_iter()
            .all(|field| peer.contains(field))
    {
        return Err(invalid("WireGuard required fields"));
    }
    Ok(())
}

fn validate_openvpn(config: &str) -> Result<(), Problem> {
    let mut has_remote = false;
    let mut inline_block: Option<String> = None;
    for line in config.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(name) = line
            .strip_prefix('<')
            .and_then(|line| line.strip_suffix('>'))
        {
            if let Some(closing) = name.strip_prefix('/') {
                let closing = closing.to_ascii_lowercase();
                if inline_block.as_deref() != Some(closing.as_str()) {
                    return Err(invalid("OpenVPN inline block"));
                }
                inline_block = None;
            } else if inline_block.is_some() {
                return Err(invalid("OpenVPN inline block"));
            } else {
                inline_block = Some(name.to_ascii_lowercase());
            }
            continue;
        }
        if inline_block.is_some() {
            continue;
        }
        let mut parts = line.split_whitespace();
        let directive = parts.next().unwrap_or_default().to_ascii_lowercase();
        match directive.as_str() {
            "remote" => {
                if parts.next().is_none() {
                    return Err(invalid("OpenVPN remote"));
                }
                has_remote = true;
            }
            "askpass" | "auth-user-pass" | "ca" | "cert" | "config" | "crl-verify" | "dh"
            | "extra-certs" | "key" | "pkcs12" | "secret" | "tls-auth" | "tls-crypt"
            | "tls-crypt-v2" => {
                return Err(invalid("OpenVPN external dependency"));
            }
            _ => {}
        }
    }
    if inline_block.is_some() {
        return Err(invalid("OpenVPN inline block"));
    }
    if !has_remote {
        return Err(invalid("OpenVPN remote"));
    }
    Ok(())
}

fn decode_base64(value: &str, field: &str) -> Result<Vec<u8>, Problem> {
    if value.is_empty() || value.len() % 4 != 0 || !value.is_ascii() {
        return Err(invalid(field));
    }
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(value.len() / 4 * 3);
    for (index, chunk) in bytes.chunks_exact(4).enumerate() {
        let last = index + 1 == bytes.len() / 4;
        let padding = usize::from(chunk[3] == b'=') + usize::from(chunk[2] == b'=');
        if padding > 0 && !last || chunk[2] == b'=' && chunk[3] != b'=' {
            return Err(invalid(field));
        }
        let a = base64_value(chunk[0]).ok_or_else(|| invalid(field))?;
        let b = base64_value(chunk[1]).ok_or_else(|| invalid(field))?;
        let c = if chunk[2] == b'=' {
            0
        } else {
            base64_value(chunk[2]).ok_or_else(|| invalid(field))?
        };
        let d = if chunk[3] == b'=' {
            0
        } else {
            base64_value(chunk[3]).ok_or_else(|| invalid(field))?
        };
        if padding == 2 && b & 0x0f != 0 || padding == 1 && c & 0x03 != 0 {
            return Err(invalid(field));
        }
        decoded.push(a << 2 | b >> 4);
        if padding < 2 {
            decoded.push(b << 4 | c >> 2);
        }
        if padding == 0 {
            decoded.push(c << 6 | d);
        }
    }
    Ok(decoded)
}

fn base64_value(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

fn unix_timestamp(seconds: i64, field: &str) -> Result<String, Problem> {
    OffsetDateTime::from_unix_timestamp(seconds)
        .map_err(|_| invalid(field))?
        .format(&Rfc3339)
        .map_err(|_| invalid(field))
}

fn required<'a>(value: &'a str, field: &str) -> Result<&'a str, Problem> {
    if value.trim().is_empty() {
        Err(invalid(field))
    } else {
        Ok(value)
    }
}

fn source_ref(instance_id: &str, source_id: &str) -> ProfileSourceRef {
    ProfileSourceRef {
        system: "buglogin-legacy".into(),
        instance_id: instance_id.into(),
        id: source_id.into(),
    }
}

fn mapped_id(instance_id: &str, entity: &str, source_id: &str) -> String {
    migration_uuid("buglogin-legacy", instance_id, entity, source_id).to_string()
}

fn reject_symlink(path: &Path) -> Result<(), Problem> {
    if fs::symlink_metadata(path)
        .map_err(|error| io_problem("VPN source", error))?
        .file_type()
        .is_symlink()
    {
        Err(invalid("VPN symlink"))
    } else {
        Ok(())
    }
}

fn invalid(field: &str) -> Problem {
    Problem::new(
        "migration_vpn_invalid",
        "Legacy VPN migration data is invalid",
        422,
        "migration",
    )
    .with_field("field", field)
}

fn io_problem(path: &str, error: std::io::Error) -> Problem {
    Problem::new(
        "migration_vpn_read_failed",
        "Legacy VPN migration data could not be read",
        500,
        "migration",
    )
    .with_field("path", path)
    .with_detail(error.to_string())
}
