use std::fs;
use std::path::{Path, PathBuf};

use buglogin_domain::errors::Problem;
use serde::{Deserialize, Serialize};

use crate::transform::{
    StagedMigration, ensure_directory, io_problem, problem, reject_symlink, validate_run_id,
    write_created_file,
};

const CUTOVER_CONFIRMATION: &str = "CUTOVER_AND_PROMOTE";
pub(crate) const IMPORTS_DIRECTORY: &str = "migration-imports";
pub(crate) const JOURNAL_FILE: &str = ".buglogin-cutover.json";
pub(crate) const JOURNAL_CHECKSUM_FILE: &str = ".buglogin-cutover.sha256";

#[derive(Clone, Copy, Debug)]
pub struct CutoverConfirmation(());

pub fn confirm_cutover(value: &str) -> Result<CutoverConfirmation, Problem> {
    if value == CUTOVER_CONFIRMATION {
        Ok(CutoverConfirmation(()))
    } else {
        Err(problem(
            "migration_cutover_not_confirmed",
            "Migration cutover is not confirmed",
            409,
        ))
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CutoverArtifact {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CutoverJournal {
    pub schema: String,
    pub run_id: String,
    pub package_id: String,
    pub checksums_sha256: String,
    pub destination_created: bool,
    pub imports_directory_created: bool,
    pub artifacts: Vec<CutoverArtifact>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CutoverReceipt {
    pub run_id: String,
    pub package_id: String,
    pub destination_root: PathBuf,
    pub run_root: PathBuf,
    pub artifacts: usize,
}

pub fn promote_staged(
    staged: &StagedMigration,
    destination_root: &Path,
    confirmation: CutoverConfirmation,
) -> Result<CutoverReceipt, Problem> {
    let _ = confirmation;
    validate_run_id(&staged.run_id)?;
    reject_symlink(&staged.staging_root)?;
    let destination_created = !destination_root.exists();
    ensure_directory(destination_root)?;
    reject_symlink(destination_root)?;
    let imports_root = destination_root.join(IMPORTS_DIRECTORY);
    let imports_directory_created = !imports_root.exists();
    ensure_directory(&imports_root)?;
    reject_symlink(&imports_root)?;

    let pending_root = imports_root.join(format!(".{}.pending", staged.run_id));
    let run_root = imports_root.join(&staged.run_id);
    if run_root.exists() {
        return replay_existing_cutover(staged, destination_root, run_root);
    }
    if pending_root.exists() {
        let pending_receipt =
            replay_existing_cutover(&staged, destination_root, pending_root.clone())?;
        fs::rename(&pending_root, &run_root)
            .map_err(|error| io_problem("resume migration staging promotion", &error))?;
        return Ok(CutoverReceipt {
            run_id: staged.run_id.clone(),
            package_id: staged.package_id.clone(),
            destination_root: destination_root.to_path_buf(),
            run_root,
            artifacts: pending_receipt.artifacts,
        });
    }
    fs::create_dir(&pending_root)
        .map_err(|error| io_problem("create cutover pending directory", &error))?;

    let result = (|| {
        let mut artifacts = Vec::with_capacity(staged.files.len() + 1);
        for file in &staged.files {
            let source = staged.staging_root.join(&file.path);
            let bytes = fs::read(&source)
                .map_err(|error| io_problem("read staged migration file", &error))?;
            if bytes.len() as u64 != file.bytes
                || buglogin_storage::blob::checksum(&bytes) != file.sha256
            {
                return Err(problem(
                    "migration_staging_checksum_mismatch",
                    "Migration staging checksum mismatch",
                    409,
                ));
            }
            write_created_file(&pending_root, &file.path, &bytes)?;
            artifacts.push(CutoverArtifact {
                path: file.path.clone(),
                bytes: file.bytes,
                sha256: file.sha256.clone(),
            });
        }
        let checksum_bytes = fs::read(staged.staging_root.join("checksums.sha256"))
            .map_err(|error| io_problem("read staged checksums", &error))?;
        if buglogin_storage::blob::checksum(&checksum_bytes) != staged.checksums_sha256 {
            return Err(problem(
                "migration_staging_checksum_mismatch",
                "Migration staging checksum mismatch",
                409,
            ));
        }
        write_created_file(&pending_root, "checksums.sha256", &checksum_bytes)?;
        artifacts.push(CutoverArtifact {
            path: "checksums.sha256".to_owned(),
            bytes: checksum_bytes.len() as u64,
            sha256: staged.checksums_sha256.clone(),
        });

        let journal = CutoverJournal {
            schema: "buglogin.cutover/1".to_owned(),
            run_id: staged.run_id.clone(),
            package_id: staged.package_id.clone(),
            checksums_sha256: staged.checksums_sha256.clone(),
            destination_created,
            imports_directory_created,
            artifacts,
        };
        let mut journal_bytes = serde_json::to_vec(&journal).map_err(|_| {
            problem(
                "migration_cutover_journal_invalid",
                "Migration cutover journal is invalid",
                500,
            )
        })?;
        journal_bytes.push(b'\n');
        write_created_file(&pending_root, JOURNAL_FILE, &journal_bytes)?;
        write_created_file(
            &pending_root,
            JOURNAL_CHECKSUM_FILE,
            buglogin_storage::blob::checksum(&journal_bytes).as_bytes(),
        )?;
        fs::rename(&pending_root, &run_root)
            .map_err(|error| io_problem("promote migration staging", &error))?;
        Ok(CutoverReceipt {
            run_id: staged.run_id.clone(),
            package_id: staged.package_id.clone(),
            destination_root: destination_root.to_path_buf(),
            run_root,
            artifacts: journal.artifacts.len(),
        })
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&pending_root);
        if imports_directory_created {
            let _ = fs::remove_dir(&imports_root);
        }
        if destination_created {
            let _ = fs::remove_dir(destination_root);
        }
    }
    result
}

fn replay_existing_cutover(
    staged: &StagedMigration,
    destination_root: &Path,
    run_root: PathBuf,
) -> Result<CutoverReceipt, Problem> {
    reject_symlink(&run_root)?;
    let journal_bytes = fs::read(run_root.join(JOURNAL_FILE))
        .map_err(|error| io_problem("read cutover journal", &error))?;
    let journal_checksum = fs::read_to_string(run_root.join(JOURNAL_CHECKSUM_FILE))
        .map_err(|error| io_problem("read cutover journal checksum", &error))?;
    if buglogin_storage::blob::checksum(&journal_bytes) != journal_checksum.trim() {
        return Err(problem(
            "migration_cutover_journal_checksum_mismatch",
            "Migration cutover journal checksum mismatch",
            409,
        ));
    }
    let journal: CutoverJournal = serde_json::from_slice(&journal_bytes).map_err(|_| {
        problem(
            "migration_cutover_journal_invalid",
            "Migration cutover journal is invalid",
            409,
        )
    })?;
    if journal.schema != "buglogin.cutover/1"
        || journal.run_id != staged.run_id
        || journal.package_id != staged.package_id
        || journal.checksums_sha256 != staged.checksums_sha256
    {
        return Err(problem(
            "migration_cutover_replay_mismatch",
            "Migration cutover replay does not match the staged package",
            409,
        ));
    }
    let mut expected = staged
        .files
        .iter()
        .map(|file| (file.path.as_str(), (file.bytes, file.sha256.as_str())))
        .collect::<std::collections::BTreeMap<_, _>>();
    expected.insert("checksums.sha256", (0, staged.checksums_sha256.as_str()));
    if journal.artifacts.len() != expected.len()
        || journal.artifacts.iter().any(|artifact| {
            let Some((bytes, sha256)) = expected.remove(artifact.path.as_str()) else {
                return true;
            };
            let Ok(contents) = fs::read(run_root.join(&artifact.path)) else {
                return true;
            };
            let size_matches =
                artifact.path == "checksums.sha256" || contents.len() as u64 == bytes;
            !size_matches
                || artifact.bytes != contents.len() as u64
                || artifact.sha256 != sha256
                || buglogin_storage::blob::checksum(&contents) != sha256
        })
        || !expected.is_empty()
    {
        return Err(problem(
            "migration_cutover_replay_mismatch",
            "Migration cutover files do not match the staged package",
            409,
        ));
    }
    Ok(CutoverReceipt {
        run_id: staged.run_id.clone(),
        package_id: staged.package_id.clone(),
        destination_root: destination_root.to_path_buf(),
        run_root,
        artifacts: journal.artifacts.len(),
    })
}
