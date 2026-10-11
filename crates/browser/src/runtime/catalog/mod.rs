use std::{collections::HashSet, path::Path};

use buglogin_domain::{
    entities::{Runtime, RuntimeArchitecture, RuntimePlatform},
    errors::Problem,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeRelease {
    pub runtime: Runtime,
    pub version: String,
    pub platform: RuntimePlatform,
    pub architecture: RuntimeArchitecture,
    pub relative_path: String,
    pub source: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub key_id: String,
    pub signature: String,
}

impl RuntimeRelease {
    pub fn signing_payload(&self) -> Vec<u8> {
        format!(
            "{:?}\n{}\n{:?}\n{:?}\n{}\n{}\n{}\n{}\n{}",
            self.runtime,
            self.version,
            self.platform,
            self.architecture,
            self.relative_path,
            self.source,
            self.sha256.to_ascii_lowercase(),
            self.size_bytes,
            self.key_id
        )
        .into_bytes()
    }
}

pub trait RuntimeSignatureVerifier {
    fn verify(&self, key_id: &str, payload: &[u8], signature: &str) -> bool;
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
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
        if self.schema_version != 1 {
            return Err(problem(
                "runtime_catalog_version_unsupported",
                "Runtime catalog version unsupported",
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
                if !verifier.verify(
                    &release.key_id,
                    &release.signing_payload(),
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

fn release_identity(release: &RuntimeRelease) -> String {
    format!(
        "{:?}\0{}\0{:?}\0{:?}",
        release.runtime, release.version, release.platform, release.architecture
    )
}

fn validate_release(release: &RuntimeRelease) -> Result<(), Problem> {
    if release.version.trim().is_empty()
        || release.key_id.trim().is_empty()
        || release.signature.trim().is_empty()
    {
        return Err(problem(
            "runtime_release_invalid",
            "Runtime release invalid",
        ));
    }
    if release.size_bytes == 0
        || release.sha256.len() != 64
        || !release.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(problem(
            "runtime_checksum_invalid",
            "Runtime checksum invalid",
        ));
    }
    if !release.source.starts_with("https://") {
        return Err(problem(
            "runtime_source_insecure",
            "Runtime source must use HTTPS",
        ));
    }
    let path = Path::new(&release.relative_path);
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(problem("runtime_path_invalid", "Runtime path invalid"));
    }
    Ok(())
}

fn problem(code: &'static str, title: &'static str) -> Problem {
    Problem::new(code, title, 400, "runtime-catalog")
}
