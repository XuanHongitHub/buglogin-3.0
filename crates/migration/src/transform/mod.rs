use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use buglogin_domain::errors::Problem;
use serde::{Deserialize, Serialize};

use crate::package::NeutralPackage;
use crate::verify::{VerifiedPackageFile, validate_relative_path, verify_package};

pub use crate::legacy::{
    LegacyProfileMapping, NeutralProfileData, NeutralProfileRecord, ProfileDataClassification,
    ProfileDataDisposition, ProfileDataObject, ProfileSourceRef, map_legacy_profiles,
    map_legacy_profiles_for_encrypted_package,
};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StagedMigration {
    pub run_id: String,
    pub package_id: String,
    pub staging_root: PathBuf,
    pub checksums_sha256: String,
    pub files: Vec<VerifiedPackageFile>,
}

const STAGING_JOURNAL: &str = ".buglogin-staging.json";
const STAGING_FREE_SPACE_RESERVE: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct StagingJournal {
    schema: String,
    run_id: String,
    package_id: String,
    checksums_sha256: String,
    files: Vec<VerifiedPackageFile>,
}

pub fn stage_package(
    package: &NeutralPackage,
    staging_parent: &Path,
    run_id: &str,
) -> Result<StagedMigration, Problem> {
    verify_package(package)?;
    ensure_directory(staging_parent)?;
    reject_symlink(staging_parent)?;
    let available_bytes = fs2::available_space(staging_parent)
        .map_err(|error| io_problem("read migration staging capacity", &error))?;
    stage_package_with_available_bytes(package, staging_parent, run_id, available_bytes)
}

pub fn stage_package_with_available_bytes(
    package: &NeutralPackage,
    staging_parent: &Path,
    run_id: &str,
    available_bytes: u64,
) -> Result<StagedMigration, Problem> {
    validate_run_id(run_id)?;
    let verified = verify_package(package)?;
    let staged_files = package_staged_files(package, &verified.files);
    ensure_directory(staging_parent)?;
    reject_symlink(staging_parent)?;
    let staging_root = staging_parent.join(run_id);
    let created_root = if staging_root.exists() {
        reject_symlink(&staging_root)?;
        false
    } else {
        fs::create_dir(&staging_root)
            .map_err(|error| io_problem("create migration staging", &error))?;
        true
    };

    let journal = StagingJournal {
        schema: "buglogin.staging/1".to_owned(),
        run_id: run_id.to_owned(),
        package_id: verified.package_id.clone(),
        checksums_sha256: verified.checksums_sha256.clone(),
        files: staged_files.clone(),
    };
    let required_bytes = if created_root {
        staged_files
            .iter()
            .try_fold(
                staging_journal_bytes(&journal)?.len() as u64,
                |total, file| {
                    total
                        .checked_add(file.bytes)
                        .ok_or_else(staging_capacity_overflow)
                },
            )?
            .checked_add(package.file("checksums.sha256").unwrap().len() as u64)
            .ok_or_else(staging_capacity_overflow)?
    } else {
        verify_staging_journal(&staging_root, &journal)?;
        missing_staging_bytes(package, &staging_root, &staged_files)?
    };
    if required_bytes > 0 {
        let required_with_reserve = required_bytes
            .checked_add(STAGING_FREE_SPACE_RESERVE)
            .ok_or_else(staging_capacity_overflow)?;
        if available_bytes < required_with_reserve {
            if created_root {
                let _ = fs::remove_dir(&staging_root);
            }
            return Err(problem(
                "migration_staging_capacity_insufficient",
                "Migration staging capacity is insufficient",
                507,
            )
            .with_field("requiredBytes", required_with_reserve.to_string())
            .with_field("availableBytes", available_bytes.to_string()));
        }
    }
    if created_root {
        if let Err(error) = write_staging_journal(&staging_root, &journal) {
            let _ = fs::remove_dir_all(&staging_root);
            return Err(error);
        }
    }

    let result = (|| {
        for file in &staged_files {
            let bytes = package.file(&file.path).ok_or_else(|| {
                problem(
                    "migration_package_file_missing",
                    "Migration package file is missing",
                    409,
                )
            })?;
            let target = staging_root.join(&file.path);
            if target.exists() {
                reject_symlink(&target)?;
                let existing = fs::read(&target)
                    .map_err(|error| io_problem("read staged migration file", &error))?;
                if existing.len() as u64 != file.bytes
                    || buglogin_storage::blob::checksum(&existing) != file.sha256
                {
                    return Err(problem(
                        "migration_staging_file_mismatch",
                        "Existing migration staging file does not match the package",
                        409,
                    ));
                }
            } else {
                write_created_file(&staging_root, &file.path, bytes)?;
            }
        }
        let checksum_bytes = package.file("checksums.sha256").unwrap();
        let checksum_path = staging_root.join("checksums.sha256");
        if checksum_path.exists() {
            reject_symlink(&checksum_path)?;
            let existing = fs::read(&checksum_path)
                .map_err(|error| io_problem("read staged checksums", &error))?;
            if existing != checksum_bytes {
                return Err(problem(
                    "migration_staging_file_mismatch",
                    "Existing migration checksums do not match the package",
                    409,
                ));
            }
        } else {
            write_created_file(&staging_root, "checksums.sha256", checksum_bytes)?;
        }
        Ok(StagedMigration {
            run_id: run_id.to_owned(),
            package_id: verified.package_id,
            staging_root: staging_root.clone(),
            checksums_sha256: verified.checksums_sha256,
            files: staged_files,
        })
    })();
    if result.is_err() && created_root {
        let _ = fs::remove_dir_all(&staging_root);
    }
    result
}

pub fn discard_staging(
    package: &NeutralPackage,
    staging_parent: &Path,
    run_id: &str,
) -> Result<(), Problem> {
    validate_run_id(run_id)?;
    let verified = verify_package(package)?;
    ensure_directory(staging_parent)?;
    reject_symlink(staging_parent)?;
    let staging_root = staging_parent.join(run_id);
    if !staging_root.exists() {
        return Err(problem(
            "migration_staging_not_found",
            "Migration staging does not exist",
            404,
        ));
    }
    reject_symlink(&staging_root)?;
    let journal = StagingJournal {
        schema: "buglogin.staging/1".to_owned(),
        run_id: run_id.to_owned(),
        package_id: verified.package_id,
        checksums_sha256: verified.checksums_sha256,
        files: package_staged_files(package, &verified.files),
    };
    verify_staging_journal(&staging_root, &journal)?;
    fs::remove_dir_all(staging_root)
        .map_err(|error| io_problem("discard migration staging", &error))
}

fn write_staging_journal(root: &Path, journal: &StagingJournal) -> Result<(), Problem> {
    write_created_file(root, STAGING_JOURNAL, &staging_journal_bytes(journal)?)
}

fn staging_journal_bytes(journal: &StagingJournal) -> Result<Vec<u8>, Problem> {
    let mut bytes = serde_json::to_vec(journal).map_err(|_| {
        problem(
            "migration_staging_journal_invalid",
            "Migration staging journal is invalid",
            500,
        )
    })?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn missing_staging_bytes(
    package: &NeutralPackage,
    root: &Path,
    files: &[VerifiedPackageFile],
) -> Result<u64, Problem> {
    let mut missing = 0_u64;
    for file in files {
        let target = root.join(&file.path);
        if target.exists() {
            reject_symlink(&target)?;
            let existing = fs::read(&target)
                .map_err(|error| io_problem("read staged migration file", &error))?;
            if existing.len() as u64 != file.bytes
                || buglogin_storage::blob::checksum(&existing) != file.sha256
            {
                return Err(problem(
                    "migration_staging_file_mismatch",
                    "Existing migration staging file does not match the package",
                    409,
                ));
            }
        } else {
            missing = missing
                .checked_add(file.bytes)
                .ok_or_else(staging_capacity_overflow)?;
        }
    }
    let checksums = package.file("checksums.sha256").unwrap();
    let checksum_path = root.join("checksums.sha256");
    if checksum_path.exists() {
        reject_symlink(&checksum_path)?;
        let existing = fs::read(&checksum_path)
            .map_err(|error| io_problem("read staged checksums", &error))?;
        if existing != checksums {
            return Err(problem(
                "migration_staging_file_mismatch",
                "Existing migration checksums do not match the package",
                409,
            ));
        }
    } else {
        missing = missing
            .checked_add(checksums.len() as u64)
            .ok_or_else(staging_capacity_overflow)?;
    }
    Ok(missing)
}

fn staging_capacity_overflow() -> Problem {
    problem(
        "migration_staging_capacity_overflow",
        "Migration staging capacity calculation overflowed",
        422,
    )
}

fn verify_staging_journal(root: &Path, expected: &StagingJournal) -> Result<(), Problem> {
    let bytes = fs::read(root.join(STAGING_JOURNAL))
        .map_err(|error| io_problem("read migration staging journal", &error))?;
    let actual: StagingJournal = serde_json::from_slice(&bytes).map_err(|_| {
        problem(
            "migration_staging_journal_invalid",
            "Migration staging journal is invalid",
            409,
        )
    })?;
    if actual != *expected {
        return Err(problem(
            "migration_staging_replay_mismatch",
            "Migration staging replay does not match the package",
            409,
        ));
    }
    let expected_paths = expected
        .files
        .iter()
        .map(|file| file.path.clone())
        .chain(std::iter::once("checksums.sha256".to_owned()))
        .chain(std::iter::once(STAGING_JOURNAL.to_owned()))
        .collect::<BTreeSet<_>>();
    let mut actual_paths = BTreeSet::new();
    collect_staging_files(root, root, &mut actual_paths)?;
    if actual_paths
        .iter()
        .any(|path| !expected_paths.contains(path))
    {
        return Err(problem(
            "migration_staging_replay_mismatch",
            "Migration staging contains unexpected files",
            409,
        ));
    }
    Ok(())
}

fn collect_staging_files(
    root: &Path,
    current: &Path,
    files: &mut BTreeSet<String>,
) -> Result<(), Problem> {
    for entry in
        fs::read_dir(current).map_err(|error| io_problem("read migration staging", &error))?
    {
        let entry = entry.map_err(|error| io_problem("read migration staging entry", &error))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| io_problem("inspect migration staging entry", &error))?;
        if file_type.is_symlink() {
            return Err(problem(
                "migration_staging_symlink_rejected",
                "Migration staging symlink is not allowed",
                409,
            ));
        }
        if file_type.is_dir() {
            collect_staging_files(root, &path, files)?;
        } else if file_type.is_file() {
            let relative = path
                .strip_prefix(root)
                .map_err(|_| {
                    problem(
                        "migration_staging_replay_mismatch",
                        "Migration staging path is invalid",
                        409,
                    )
                })?
                .to_string_lossy()
                .replace('\\', "/");
            files.insert(relative);
        } else {
            return Err(problem(
                "migration_staging_entry_invalid",
                "Migration staging entry is invalid",
                409,
            ));
        }
    }
    Ok(())
}

fn package_staged_files(
    package: &NeutralPackage,
    verified_files: &[VerifiedPackageFile],
) -> Vec<VerifiedPackageFile> {
    let mut files = verified_files.to_vec();
    if let Some(signature) = package.file("signature.json") {
        files.push(VerifiedPackageFile {
            path: "signature.json".to_owned(),
            bytes: signature.len() as u64,
            sha256: buglogin_storage::blob::checksum(signature),
        });
    }
    files
}

pub(crate) fn validate_run_id(run_id: &str) -> Result<(), Problem> {
    if run_id.is_empty()
        || run_id.len() > 128
        || !run_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(problem(
            "migration_run_id_invalid",
            "Migration run ID is invalid",
            400,
        ));
    }
    Ok(())
}

pub(crate) fn ensure_directory(path: &Path) -> Result<(), Problem> {
    fs::create_dir_all(path).map_err(|error| io_problem("create migration directory", &error))?;
    if !path.is_dir() {
        return Err(problem(
            "migration_directory_invalid",
            "Migration directory is invalid",
            400,
        ));
    }
    Ok(())
}

pub(crate) fn reject_symlink(path: &Path) -> Result<(), Problem> {
    if path
        .symlink_metadata()
        .map_err(|error| io_problem("inspect migration directory", &error))?
        .file_type()
        .is_symlink()
    {
        return Err(problem(
            "migration_symlink_rejected",
            "Migration symlink is not allowed",
            400,
        ));
    }
    Ok(())
}

pub(crate) fn write_created_file(root: &Path, relative: &str, bytes: &[u8]) -> Result<(), Problem> {
    validate_relative_path(relative)?;
    let target = root.join(relative);
    let parent = target.parent().ok_or_else(|| {
        problem(
            "migration_package_path_invalid",
            "Migration package path is invalid",
            400,
        )
    })?;
    fs::create_dir_all(parent).map_err(|error| io_problem("create staged directory", &error))?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&target)
        .map_err(|error| io_problem("create staged file", &error))?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|error| io_problem("write staged file", &error))
}

pub(crate) fn problem(code: &str, title: &str, status: u16) -> Problem {
    Problem::new(code, title, status, "migration")
}

pub(crate) fn io_problem(operation: &str, error: &std::io::Error) -> Problem {
    Problem::new(
        "migration_io_error",
        "Migration filesystem operation failed",
        500,
        "migration",
    )
    .with_detail(format!("Failed to {operation}: {error}"))
    .retryable(true)
}
