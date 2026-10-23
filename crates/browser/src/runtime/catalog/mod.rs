use std::{
    collections::{BTreeMap, HashSet},
    path::Path,
};

use buglogin_domain::{
    entities::{Runtime, RuntimeArchitecture, RuntimePlatform},
    errors::Problem,
};
use serde::{Deserialize, Serialize};
use url::Url;

const MAX_RUNTIME_PACKAGE_BYTES: u64 = 4 * 1024 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeRelease {
    pub runtime: Runtime,
    pub version: String,
    pub platform: RuntimePlatform,
    pub architecture: RuntimeArchitecture,
    pub relative_path: String,
    #[serde(default)]
    pub package_format: RuntimePackageFormat,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive_prefix: Option<String>,
    pub source: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub key_id: String,
    pub signature: String,
}

impl RuntimeRelease {
    pub fn signing_payload(&self) -> Vec<u8> {
        self.signing_payload_for_schema(1)
    }

    pub fn signing_payload_v2(&self) -> Vec<u8> {
        self.signing_payload_for_schema(2)
    }

    fn signing_payload_for_schema(&self, schema_version: u32) -> Vec<u8> {
        let mut payload = match schema_version {
            1 => b"buglogin.runtime-release.v1\0".to_vec(),
            2 => b"buglogin.runtime-release.v2\0".to_vec(),
            _ => return Vec::new(),
        };
        let fields = [
            runtime_name(&self.runtime),
            self.version.clone(),
            platform_name(&self.platform),
            architecture_name(&self.architecture),
            self.relative_path.clone(),
        ];
        for field in fields {
            append_field(&mut payload, &field);
        }
        if schema_version == 2 {
            append_field(&mut payload, self.package_format.as_str());
            append_field(&mut payload, self.archive_prefix.as_deref().unwrap_or(""));
        }
        let fields = [
            self.source.clone(),
            self.sha256.to_ascii_lowercase(),
            self.key_id.clone(),
        ];
        for field in fields {
            append_field(&mut payload, &field);
        }
        payload.extend_from_slice(&self.size_bytes.to_be_bytes());
        payload
    }
}

fn append_field(payload: &mut Vec<u8>, field: &str) {
    payload.extend_from_slice(&(field.len() as u64).to_be_bytes());
    payload.extend_from_slice(field.as_bytes());
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RuntimePackageFormat {
    #[default]
    Raw,
    Zip,
    Dmg,
}

impl RuntimePackageFormat {
    fn as_str(self) -> &'static str {
        match self {
            Self::Raw => "raw",
            Self::Zip => "zip",
            Self::Dmg => "dmg",
        }
    }
}

pub trait RuntimeSignatureVerifier {
    fn verify(&self, key_id: &str, payload: &[u8], signature: &str) -> bool;
}

#[derive(Clone, Debug)]
pub struct Ed25519RuntimeSignatureVerifier {
    trusted_keys: BTreeMap<String, [u8; 32]>,
}

impl Ed25519RuntimeSignatureVerifier {
    pub fn new(trusted_keys: BTreeMap<String, [u8; 32]>) -> Result<Self, Problem> {
        if trusted_keys.is_empty() || trusted_keys.keys().any(|key| key.trim().is_empty()) {
            return Err(problem(
                "runtime_trust_keys_invalid",
                "Runtime trust keys are invalid",
            ));
        }
        Ok(Self { trusted_keys })
    }
}

impl RuntimeSignatureVerifier for Ed25519RuntimeSignatureVerifier {
    fn verify(&self, key_id: &str, payload: &[u8], signature: &str) -> bool {
        let Some(public_key) = self.trusted_keys.get(key_id) else {
            return false;
        };
        let Ok(signature) = decode_hex(signature) else {
            return false;
        };
        if signature.len() != 64 {
            return false;
        }
        ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, public_key)
            .verify(payload, &signature)
            .is_ok()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeCatalog {
    pub schema_version: u32,
    pub releases: Vec<RuntimeRelease>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedRuntimeRelease(RuntimeRelease);

impl VerifiedRuntimeRelease {
    pub fn release(&self) -> &RuntimeRelease {
        &self.0
    }

    pub fn verify_artifact(&self, bytes: &[u8]) -> Result<(), Problem> {
        let release = self.release();
        if bytes.len() as u64 != release.size_bytes {
            return Err(problem("runtime_size_mismatch", "Runtime size mismatch"));
        }
        if buglogin_storage::blob::checksum(bytes) != release.sha256.to_ascii_lowercase() {
            return Err(problem(
                "runtime_checksum_mismatch",
                "Runtime checksum mismatch",
            ));
        }
        Ok(())
    }
}

impl RuntimeCatalog {
    pub fn verify(
        &self,
        verifier: &impl RuntimeSignatureVerifier,
    ) -> Result<Vec<VerifiedRuntimeRelease>, Problem> {
        if !matches!(self.schema_version, 1 | 2) {
            return Err(problem(
                "runtime_catalog_version_unsupported",
                "Runtime catalog version unsupported",
            ));
        }
        if self.releases.is_empty() {
            return Err(problem(
                "runtime_catalog_empty",
                "Runtime catalog must contain at least one release",
            ));
        }
        let mut identities = HashSet::new();
        let mut verified: Vec<_> = self
            .releases
            .iter()
            .map(|release| {
                validate_release(release)?;
                let identity = release_identity(release);
                if !identities.insert(identity) {
                    return Err(problem(
                        "runtime_release_duplicate",
                        "Runtime release duplicated",
                    ));
                }
                if self.schema_version == 1
                    && (release.package_format != RuntimePackageFormat::Raw
                        || release.archive_prefix.is_some())
                {
                    return Err(problem(
                        "runtime_release_invalid",
                        "Runtime release invalid",
                    ));
                }
                if !verifier.verify(
                    &release.key_id,
                    &release.signing_payload_for_schema(self.schema_version),
                    &release.signature,
                ) {
                    return Err(problem(
                        "runtime_signature_invalid",
                        "Runtime signature invalid",
                    ));
                }
                Ok(VerifiedRuntimeRelease(release.clone()))
            })
            .collect::<Result<_, _>>()?;
        verified.sort_by(|left, right| {
            release_identity(left.release()).cmp(&release_identity(right.release()))
        });
        Ok(verified)
    }
}

fn release_identity(release: &RuntimeRelease) -> Vec<u8> {
    let fields = [
        runtime_name(&release.runtime),
        release.version.clone(),
        platform_name(&release.platform),
        architecture_name(&release.architecture),
    ];
    let mut identity = Vec::new();
    for field in fields {
        identity.extend_from_slice(&(field.len() as u64).to_be_bytes());
        identity.extend_from_slice(field.as_bytes());
    }
    identity
}

fn runtime_name(runtime: &Runtime) -> String {
    match runtime {
        Runtime::Wayfern => "wayfern",
        Runtime::Bugox => "bugox",
        Runtime::Bugium => "bugium",
        Runtime::Bugmini => "bugmini",
        Runtime::Bugthor => "bugthor",
        Runtime::Bughelium => "bughelium",
        Runtime::Preserved => "preserved",
    }
    .to_owned()
}

fn platform_name(platform: &RuntimePlatform) -> String {
    match platform {
        RuntimePlatform::Windows => "windows",
        RuntimePlatform::Macos => "macos",
        RuntimePlatform::Linux => "linux",
    }
    .to_owned()
}

fn architecture_name(architecture: &RuntimeArchitecture) -> String {
    match architecture {
        RuntimeArchitecture::X64 => "x64",
        RuntimeArchitecture::Arm64 => "arm64",
    }
    .to_owned()
}

fn decode_hex(value: &str) -> Result<Vec<u8>, ()> {
    if value.len() % 2 != 0 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(());
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = hex_digit(pair[0]).ok_or(())?;
            let low = hex_digit(pair[1]).ok_or(())?;
            Ok((high << 4) | low)
        })
        .collect()
}

fn hex_digit(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn validate_release(release: &RuntimeRelease) -> Result<(), Problem> {
    if !matches!(&release.runtime, &Runtime::Wayfern | &Runtime::Bugmini) {
        return Err(problem(
            "runtime_not_supported",
            "Runtime release has no approved desktop adapter",
        ));
    }
    if release.version.trim().is_empty()
        || release.key_id.trim().is_empty()
        || release.signature.trim().is_empty()
    {
        return Err(problem(
            "runtime_release_invalid",
            "Runtime release invalid",
        ));
    }
    match (release.package_format, release.archive_prefix.as_deref()) {
        (RuntimePackageFormat::Raw, None) | (RuntimePackageFormat::Zip, None) => {}
        (RuntimePackageFormat::Zip, Some(prefix)) if safe_relative_path(prefix) => {}
        (RuntimePackageFormat::Dmg, None)
            if release.runtime == Runtime::Wayfern
                && release.platform == RuntimePlatform::Macos
                && release
                    .relative_path
                    .split('/')
                    .next()
                    .is_some_and(|part| part.ends_with(".app") && part.len() > ".app".len()) => {}
        _ => {
            return Err(problem(
                "runtime_archive_prefix_invalid",
                "Runtime archive prefix invalid",
            ));
        }
    }
    if release.size_bytes == 0 || release.size_bytes > MAX_RUNTIME_PACKAGE_BYTES {
        return Err(problem(
            "runtime_package_size_invalid",
            "Runtime package size is outside the supported limit",
        ));
    }
    if release.sha256.len() != 64 || !release.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(problem(
            "runtime_checksum_invalid",
            "Runtime checksum invalid",
        ));
    }
    let source = Url::parse(&release.source)
        .map_err(|_| problem("runtime_source_invalid", "Runtime source URL is invalid"))?;
    if release
        .source
        .bytes()
        .any(|byte| byte.is_ascii_whitespace())
    {
        return Err(problem(
            "runtime_source_invalid",
            "Runtime source URL is invalid",
        ));
    }
    if source.scheme() != "https" {
        return Err(problem(
            "runtime_source_insecure",
            "Runtime source must use HTTPS",
        ));
    }
    let authority_has_userinfo = release
        .source
        .split_once("://")
        .and_then(|(_, remainder)| remainder.split(['/', '?', '#']).next())
        .is_some_and(|authority| authority.contains('@'));
    if source.host_str().is_none()
        || !source.username().is_empty()
        || source.password().is_some()
        || authority_has_userinfo
        || source.fragment().is_some()
    {
        return Err(problem(
            "runtime_source_invalid",
            "Runtime source URL is invalid",
        ));
    }
    if !safe_relative_path(&release.relative_path) {
        return Err(problem("runtime_path_invalid", "Runtime path invalid"));
    }
    Ok(())
}

fn safe_relative_path(value: &str) -> bool {
    let path = Path::new(value);
    !value.contains('\\')
        && !value.contains(':')
        && value
            .split('/')
            .all(|component| !matches!(component, "" | "." | ".."))
        && !path.as_os_str().is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
}

fn problem(code: &'static str, title: &'static str) -> Problem {
    Problem::new(code, title, 400, "runtime-catalog")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use ring::signature::{Ed25519KeyPair, KeyPair};

    use super::{
        Ed25519RuntimeSignatureVerifier, RuntimeCatalog, RuntimeRelease, RuntimeSignatureVerifier,
    };
    use buglogin_domain::entities::{Runtime, RuntimeArchitecture, RuntimePlatform};

    fn signed_release(key_pair: &Ed25519KeyPair) -> RuntimeRelease {
        let mut release = RuntimeRelease {
            runtime: Runtime::Wayfern,
            version: "1.2.3".into(),
            platform: RuntimePlatform::Windows,
            architecture: RuntimeArchitecture::X64,
            relative_path: "wayfern/wayfern.exe".into(),
            package_format: super::RuntimePackageFormat::Raw,
            archive_prefix: None,
            source: "https://releases.buglogin.example/wayfern.exe".into(),
            sha256: "ab".repeat(32),
            size_bytes: 123,
            key_id: "runtime-key-1".into(),
            signature: String::new(),
        };
        release.signature = encode_hex(key_pair.sign(&release.signing_payload()).as_ref());
        release
    }

    fn encode_hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    #[test]
    fn ed25519_catalog_verification_binds_all_release_fields_and_key_id() {
        let key_pair = Ed25519KeyPair::from_seed_unchecked(&[7_u8; 32]).unwrap();
        let public_key: [u8; 32] = key_pair.public_key().as_ref().try_into().unwrap();
        let verifier = Ed25519RuntimeSignatureVerifier::new(BTreeMap::from([(
            "runtime-key-1".to_owned(),
            public_key,
        )]))
        .unwrap();
        let release = signed_release(&key_pair);
        assert!(verifier.verify(
            &release.key_id,
            &release.signing_payload(),
            &release.signature
        ));

        let mut tampered = release.clone();
        tampered.source.push_str("?changed=1");
        assert_eq!(
            RuntimeCatalog {
                schema_version: 1,
                releases: vec![tampered],
            }
            .verify(&verifier)
            .unwrap_err()
            .code,
            "runtime_signature_invalid"
        );
        assert!(!verifier.verify(
            "unknown-key",
            &release.signing_payload(),
            &release.signature
        ));
        assert!(!verifier.verify(&release.key_id, &release.signing_payload(), "not-hex"));

        let mut preserved = release;
        preserved.runtime = Runtime::Preserved;
        assert_eq!(
            RuntimeCatalog {
                schema_version: 1,
                releases: vec![preserved],
            }
            .verify(&verifier)
            .unwrap_err()
            .code,
            "runtime_not_supported"
        );
    }

    #[test]
    fn schema_v2_catalog_signature_matches_the_offline_release_signer() {
        let signing_key = Ed25519KeyPair::from_seed_unchecked(&[7_u8; 32]).unwrap();
        let public_key = [
            0xea, 0x4a, 0x6c, 0x63, 0xe2, 0x9c, 0x52, 0x0a, 0xbe, 0xf5, 0x50, 0x7b, 0x13, 0x2e,
            0xc5, 0xf9, 0x95, 0x47, 0x76, 0xae, 0xbe, 0xbe, 0x7b, 0x92, 0x42, 0x1e, 0xea, 0x69,
            0x14, 0x46, 0xd2, 0x2c,
        ];
        let verifier = Ed25519RuntimeSignatureVerifier::new(BTreeMap::from([(
            "runtime-test-2026".to_owned(),
            public_key,
        )]))
        .unwrap();
        let release = RuntimeRelease {
            runtime: Runtime::Wayfern,
            version: "154.0.8037.57".into(),
            platform: RuntimePlatform::Windows,
            architecture: RuntimeArchitecture::X64,
            relative_path: "chrome.exe".into(),
            package_format: super::RuntimePackageFormat::Zip,
            archive_prefix: Some("wayfern".into()),
            source: "https://download.example.test/wayfern.zip".into(),
            sha256: "08d8563f63df16581c60a922f97cda4797b1e5cf72c9f42d089d8fea52eb15f7".into(),
            size_bytes: 25,
            key_id: "runtime-test-2026".into(),
            signature: "2a3951c67c2c8e0bc38dd25b75a77711dac5bbfa78442257ddc7e94d3e056233342764be69ad55b5ecd44cefc98d40df30b8252585f6e261b8bd1525c1affd00".into(),
        };
        assert_eq!(
            encode_hex(signing_key.public_key().as_ref()),
            encode_hex(&public_key)
        );
        assert_eq!(
            encode_hex(&release.signing_payload_v2()),
            "6275676c6f67696e2e72756e74696d652d72656c656173652e76320000000000000000077761796665726e000000000000000d3135342e302e383033372e3537000000000000000777696e646f77730000000000000003783634000000000000000a6368726f6d652e65786500000000000000037a697000000000000000077761796665726e000000000000002968747470733a2f2f646f776e6c6f61642e6578616d706c652e746573742f7761796665726e2e7a6970000000000000004030386438353633663633646631363538316336306139323266393763646134373937623165356366373263396634326430383964386665613532656231356637000000000000001172756e74696d652d746573742d323032360000000000000019"
        );
        assert_eq!(
            encode_hex(signing_key.sign(&release.signing_payload_v2()).as_ref()),
            release.signature
        );
        let verified = RuntimeCatalog {
            schema_version: 2,
            releases: vec![release],
        }
        .verify(&verifier)
        .expect("the Rust verifier must accept a catalog emitted by the Node release signer");
        assert_eq!(verified.len(), 1);
    }

    #[test]
    fn runtime_trust_configuration_must_be_nonempty_and_named() {
        assert_eq!(
            Ed25519RuntimeSignatureVerifier::new(BTreeMap::new())
                .unwrap_err()
                .code,
            "runtime_trust_keys_invalid"
        );
        assert_eq!(
            Ed25519RuntimeSignatureVerifier::new(BTreeMap::from([(" ".to_owned(), [0_u8; 32],)]))
                .unwrap_err()
                .code,
            "runtime_trust_keys_invalid"
        );
    }
}
