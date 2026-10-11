use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};

use buglogin_domain::errors::Problem;
use serde::{Deserialize, Serialize};

use crate::package::{NeutralPackage, NeutralPackageManifest};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifiedPackage {
    pub package_id: String,
    pub checksums_sha256: String,
    pub files: Vec<VerifiedPackageFile>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifiedPackageFile {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
}

pub fn verify_package(package: &NeutralPackage) -> Result<VerifiedPackage, Problem> {
    let checksum_bytes = package.file("checksums.sha256").ok_or_else(|| {
        problem(
            "migration_checksums_missing",
            "Migration checksums are missing",
        )
    })?;
    let expected = parse_checksums(checksum_bytes)?;
    let actual_paths = package
        .files()
        .keys()
        .filter(|path| path.as_str() != "checksums.sha256" && path.as_str() != "signature.json")
        .cloned()
        .collect::<BTreeSet<_>>();
    let expected_paths = expected.keys().cloned().collect::<BTreeSet<_>>();
    if actual_paths != expected_paths {
        return Err(problem(
            "migration_checksum_coverage_mismatch",
            "Migration checksum coverage is incomplete",
        ));
    }

    let mut files = Vec::with_capacity(expected.len());
    for (path, expected_sha256) in expected {
        validate_relative_path(&path)?;
        let bytes = package.file(&path).ok_or_else(|| {
            problem(
                "migration_package_file_missing",
                "Migration package file is missing",
            )
        })?;
        let actual_sha256 = buglogin_storage::blob::checksum(bytes);
        if actual_sha256 != expected_sha256 {
            return Err(checksum_problem());
        }
        files.push(VerifiedPackageFile {
            path,
            bytes: bytes.len() as u64,
            sha256: actual_sha256,
        });
    }

    let manifest: NeutralPackageManifest =
        serde_json::from_slice(package.file("manifest.json").ok_or_else(|| {
            problem(
                "migration_manifest_missing",
                "Migration manifest is missing",
            )
        })?)
        .map_err(|_| {
            problem(
                "migration_manifest_invalid",
                "Migration manifest is invalid",
            )
        })?;
    if manifest.schema != "buglogin.export/1" || manifest.package_id.trim().is_empty() {
        return Err(problem(
            "migration_manifest_invalid",
            "Migration manifest is invalid",
        ));
    }
    let encrypted_paths = manifest
        .contents
        .iter()
        .any(|content| content.path.starts_with("encrypted/"));
    let encrypted_secret_paths = manifest.contents.iter().any(|content| {
        content.path.starts_with("encrypted/proxy-credentials/")
            || content.path.starts_with("encrypted/vpn-configs/")
    });
    let supported_encryption =
        manifest.security.encryption.as_deref() == Some("aes-256-gcm-chunks/1");
    if manifest.security.contains_cookie_values
        || manifest.security.contains_secrets != encrypted_secret_paths
        || manifest.security.encrypted != manifest.security.encryption.is_some()
        || manifest.security.encrypted != encrypted_paths
        || (manifest.security.encrypted && !supported_encryption)
        || manifest.security.redaction_policy != "buglogin.redaction/1"
    {
        return Err(problem(
            "migration_package_secret_policy_failed",
            "Migration package violates secret policy",
        ));
    }

    let verified_by_path = files
        .iter()
        .map(|file| (file.path.as_str(), file))
        .collect::<BTreeMap<_, _>>();
    let mut content_paths = BTreeSet::new();
    for content in &manifest.contents {
        validate_relative_path(&content.path)?;
        if !content_paths.insert(content.path.as_str()) {
            return Err(problem(
                "migration_manifest_duplicate_content",
                "Migration manifest contains duplicate content",
            ));
        }
        let file = verified_by_path.get(content.path.as_str()).ok_or_else(|| {
            problem(
                "migration_manifest_content_missing",
                "Migration manifest content is missing",
            )
        })?;
        if file.bytes != content.bytes || file.sha256 != content.sha256 {
            return Err(checksum_problem());
        }
    }

    Ok(VerifiedPackage {
        package_id: manifest.package_id,
        checksums_sha256: buglogin_storage::blob::checksum(checksum_bytes),
        files,
    })
}

fn parse_checksums(bytes: &[u8]) -> Result<BTreeMap<String, String>, Problem> {
    let text = std::str::from_utf8(bytes).map_err(|_| {
        problem(
            "migration_checksums_invalid",
            "Migration checksums are invalid",
        )
    })?;
    let mut checksums = BTreeMap::new();
    for line in text.lines() {
        let (sha256, path) = line.split_once("  ").ok_or_else(|| {
            problem(
                "migration_checksums_invalid",
                "Migration checksums are invalid",
            )
        })?;
        if !is_sha256(sha256)
            || path.is_empty()
            || checksums
                .insert(path.to_owned(), sha256.to_owned())
                .is_some()
        {
            return Err(problem(
                "migration_checksums_invalid",
                "Migration checksums are invalid",
            ));
        }
    }
    if checksums.is_empty() {
        return Err(problem(
            "migration_checksums_invalid",
            "Migration checksums are invalid",
        ));
    }
    Ok(checksums)
}

pub fn validate_relative_path(path: &str) -> Result<(), Problem> {
    let invalid_text = path.is_empty()
        || path.contains(['\\', '\0'])
        || path.starts_with('/')
        || path.ends_with('/')
        || path.contains("//")
        || path
            .split('/')
            .any(|component| component.is_empty() || matches!(component, "." | ".."))
        || path
            .split('/')
            .next()
            .is_some_and(|component| component.ends_with(':'));
    let native = Path::new(path);
    if invalid_text
        || native.is_absolute()
        || native
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(problem(
            "migration_package_path_invalid",
            "Migration package path is invalid",
        ));
    }
    validate_path_length(path, "migration_package_path_too_long")?;
    Ok(())
}

pub fn validate_path_length(path: &str, code: &'static str) -> Result<(), Problem> {
    if path.len() > 1024 || path.split('/').any(|component| component.len() > 255) {
        return Err(problem(
            code,
            "Migration path exceeds the portable path limit",
        ));
    }
    Ok(())
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn checksum_problem() -> Problem {
    problem("migration_checksum_mismatch", "Migration checksum mismatch")
}

fn problem(code: &str, title: &str) -> Problem {
    Problem::new(code, title, 409, "migration")
}
