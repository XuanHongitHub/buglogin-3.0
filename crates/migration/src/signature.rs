use buglogin_domain::errors::Problem;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::package::NeutralPackage;
use crate::verify::{VerifiedPackage, verify_package};

const ALGORITHM: &str = "hmac-sha256";
const CONTEXT: &[u8] = b"buglogin.migration.signature/1\0";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct SignatureMetadata {
    pub key_id: String,
    pub signer: String,
    pub signed_at: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct PackageSignature {
    pub schema: String,
    pub algorithm: String,
    pub key_id: String,
    pub signer: String,
    pub signed_at: String,
    pub checksums_sha256: String,
    pub signature: String,
}

pub fn sign_package(
    package: &NeutralPackage,
    key: &[u8],
    metadata: SignatureMetadata,
) -> Result<NeutralPackage, Problem> {
    validate_key(key)?;
    validate_metadata(&metadata)?;
    verify_package(package)?;
    if package.file("signature.json").is_some() {
        return Err(problem(
            "migration_signature_exists",
            "Migration package is already signed",
        ));
    }
    let checksum_bytes = package.file("checksums.sha256").ok_or_else(|| {
        problem(
            "migration_checksums_missing",
            "Migration checksums are missing",
        )
    })?;
    let checksums_sha256 = checksum(checksum_bytes);
    let signature = PackageSignature {
        schema: "buglogin.signature/1".into(),
        algorithm: ALGORITHM.into(),
        key_id: metadata.key_id,
        signer: metadata.signer,
        signed_at: metadata.signed_at,
        signature: hex(&hmac(key, &message(&checksums_sha256))),
        checksums_sha256,
    };
    let mut files = package.files().clone();
    let mut bytes = serde_json::to_vec(&signature).map_err(|_| {
        problem(
            "migration_signature_invalid",
            "Migration package signature is invalid",
        )
    })?;
    bytes.push(b'\n');
    files.insert("signature.json".into(), bytes);
    Ok(NeutralPackage::from_files(files))
}

pub fn verify_signed_package(
    package: &NeutralPackage,
    key: &[u8],
) -> Result<VerifiedPackage, Problem> {
    validate_key(key)?;
    let verified = verify_package(package)?;
    let signature: PackageSignature =
        serde_json::from_slice(package.file("signature.json").ok_or_else(|| {
            problem(
                "migration_signature_missing",
                "Migration package signature is missing",
            )
        })?)
        .map_err(|_| {
            problem(
                "migration_signature_invalid",
                "Migration package signature is invalid",
            )
        })?;
    if signature.schema != "buglogin.signature/1"
        || signature.algorithm != ALGORITHM
        || signature.key_id.trim().is_empty()
        || signature.signer.trim().is_empty()
        || !signature.signed_at.ends_with('Z')
        || signature.checksums_sha256 != verified.checksums_sha256
    {
        return Err(problem(
            "migration_signature_invalid",
            "Migration package signature is invalid",
        ));
    }
    let expected = hex(&hmac(key, &message(&signature.checksums_sha256)));
    if !constant_time_equal(expected.as_bytes(), signature.signature.as_bytes()) {
        return Err(problem(
            "migration_signature_mismatch",
            "Migration package signature verification failed",
        ));
    }
    Ok(verified)
}

fn validate_key(key: &[u8]) -> Result<(), Problem> {
    if key.len() < 32 {
        Err(problem(
            "migration_signing_key_invalid",
            "Migration signing key must be at least 32 bytes",
        ))
    } else {
        Ok(())
    }
}

fn validate_metadata(metadata: &SignatureMetadata) -> Result<(), Problem> {
    if metadata.key_id.trim().is_empty()
        || metadata.signer.trim().is_empty()
        || !metadata.signed_at.contains('T')
        || !metadata.signed_at.ends_with('Z')
    {
        Err(problem(
            "migration_signature_metadata_invalid",
            "Migration signature metadata is invalid",
        ))
    } else {
        Ok(())
    }
}

fn message(checksums_sha256: &str) -> Vec<u8> {
    let mut value = Vec::with_capacity(CONTEXT.len() + checksums_sha256.len());
    value.extend_from_slice(CONTEXT);
    value.extend_from_slice(checksums_sha256.as_bytes());
    value
}

fn hmac(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut normalized = [0_u8; 64];
    if key.len() > normalized.len() {
        normalized[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        normalized[..key.len()].copy_from_slice(key);
    }
    let mut inner_pad = [0x36_u8; 64];
    let mut outer_pad = [0x5c_u8; 64];
    for index in 0..64 {
        inner_pad[index] ^= normalized[index];
        outer_pad[index] ^= normalized[index];
    }
    let inner = Sha256::new()
        .chain_update(inner_pad)
        .chain_update(message)
        .finalize();
    Sha256::new()
        .chain_update(outer_pad)
        .chain_update(inner)
        .finalize()
        .into()
}

fn checksum(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

fn problem(code: &str, title: &str) -> Problem {
    Problem::new(code, title, 409, "migration")
}
