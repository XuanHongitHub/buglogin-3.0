use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use buglogin_domain::errors::Problem;
use serde::{Deserialize, Serialize};

use crate::blob::checksum;
use crate::paths::confined_path;

const MANIFEST_VERSION: u32 = 1;
const OVERWRITE_CONFIRMATION: &str = "RESTORE_AND_OVERWRITE";
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct SnapshotFile {
    path: String,
    bytes: u64,
    sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct SnapshotManifest {
    version: u32,
    files: Vec<SnapshotFile>,
}

#[derive(Clone, Debug)]
pub struct VerifiedSnapshot {
    root: PathBuf,
    manifest: SnapshotManifest,
    manifest_sha256: String,
}

impl VerifiedSnapshot {
    pub fn manifest_sha256(&self) -> &str {
        &self.manifest_sha256
    }
    pub fn file_count(&self) -> usize {
        self.manifest.files.len()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct OverwriteConfirmation(());

pub fn confirm_overwrite(value: &str) -> Result<OverwriteConfirmation, Problem> {
    if value == OVERWRITE_CONFIRMATION {
        Ok(OverwriteConfirmation(()))
    } else {
        Err(problem(
            "backup_overwrite_not_confirmed",
            "Restore overwrite not confirmed",
            409,
        ))
    }
}

pub enum RestoreBoundary<'a> {
    CreateOnly,
    ReplaceExisting {
        confirmation: OverwriteConfirmation,
        safety_backups_root: &'a Path,
        safety_snapshot_id: &'a str,
    },
}

pub fn create_snapshot(
    source: &Path,
    backups_root: &Path,
    snapshot_id: &str,
) -> Result<VerifiedSnapshot, Problem> {
    validate_snapshot_id(snapshot_id)?;
    reject_symlink(source)?;
    let source = source
        .canonicalize()
        .map_err(|e| io_problem("open snapshot source", &e))?;
    if !source.is_dir() {
        return Err(problem(
            "backup_source_invalid",
            "Backup source must be a directory",
            400,
        ));
    }
    fs::create_dir_all(backups_root).map_err(|e| io_problem("create backup root", &e))?;
    let snapshot_root = confined_path(backups_root, Path::new(snapshot_id))?;
    if snapshot_root.starts_with(&source) {
        return Err(problem(
            "backup_destination_inside_source",
            "Backup destination cannot be inside source",
            400,
        ));
    }
    fs::create_dir(&snapshot_root).map_err(|e| {
        if e.kind() == std::io::ErrorKind::AlreadyExists {
            problem(
                "backup_snapshot_exists",
                "Backup snapshot already exists",
                409,
            )
        } else {
            io_problem("create snapshot", &e)
        }
    })?;
    let result = write_snapshot(&source, &snapshot_root)
        .and_then(|hash| verify_snapshot_root(&snapshot_root, &hash));
    if result.is_err() {
        let _ = make_writable(&snapshot_root);
        let _ = fs::remove_dir_all(&snapshot_root);
    }
    result
}

pub fn verify_snapshot(
    backups_root: &Path,
    snapshot_id: &str,
    expected_manifest_sha256: &str,
) -> Result<VerifiedSnapshot, Problem> {
    validate_snapshot_id(snapshot_id)?;
    validate_sha256(expected_manifest_sha256)?;
    let root = confined_path(backups_root, Path::new(snapshot_id))?;
    verify_snapshot_root(&root, &expected_manifest_sha256.to_ascii_lowercase())
}

pub fn restore_snapshot(
    snapshot: &VerifiedSnapshot,
    destination: &Path,
    boundary: RestoreBoundary<'_>,
) -> Result<(), Problem> {
    verify_snapshot_root(&snapshot.root, &snapshot.manifest_sha256)?;
    let parent = destination.parent().ok_or_else(|| {
        problem(
            "backup_restore_destination_invalid",
            "Restore destination is invalid",
            400,
        )
    })?;
    fs::create_dir_all(parent).map_err(|e| io_problem("create restore parent", &e))?;
    match boundary {
        RestoreBoundary::CreateOnly => {
            if destination.exists() {
                return Err(problem(
                    "backup_restore_destination_exists",
                    "Restore destination already exists",
                    409,
                ));
            }
            fs::create_dir(destination)
                .map_err(|e| io_problem("create restore destination", &e))?;
            if let Err(e) = copy_verified_files(snapshot, destination) {
                let _ = fs::remove_dir_all(destination);
                return Err(e);
            }
            Ok(())
        }
        RestoreBoundary::ReplaceExisting {
            confirmation,
            safety_backups_root,
            safety_snapshot_id,
        } => {
            let _ = confirmation;
            restore_replacing(
                snapshot,
                destination,
                safety_backups_root,
                safety_snapshot_id,
            )
        }
    }
}

fn write_snapshot(source: &Path, root: &Path) -> Result<String, Problem> {
    let files_root = root.join("files");
    fs::create_dir(&files_root).map_err(|e| io_problem("create snapshot files", &e))?;
    let mut paths = Vec::new();
    collect_files(source, Path::new(""), &mut paths)?;
    paths.sort();
    let mut files = Vec::with_capacity(paths.len());
    for relative in paths {
        let bytes = fs::read(source.join(&relative))
            .map_err(|e| io_problem("read snapshot source file", &e))?;
        let sha256 = checksum(&bytes);
        write_new(&files_root.join(&relative), &bytes)?;
        files.push(SnapshotFile {
            path: portable_path(&relative)?,
            bytes: bytes.len() as u64,
            sha256,
        });
    }
    let manifest = SnapshotManifest {
        version: MANIFEST_VERSION,
        files,
    };
    let bytes = serde_json::to_vec(&manifest)
        .map_err(|e| serialization_problem("serialize snapshot manifest", &e))?;
    let hash = checksum(&bytes);
    write_new(&root.join("manifest.json"), &bytes)?;
    set_readonly(root)?;
    Ok(hash)
}

fn verify_snapshot_root(root: &Path, expected: &str) -> Result<VerifiedSnapshot, Problem> {
    validate_sha256(expected)?;
    reject_symlink(root)?;
    let root = root
        .canonicalize()
        .map_err(|e| io_problem("open snapshot", &e))?;
    reject_symlink(&root.join("manifest.json"))?;
    reject_symlink(&root.join("files"))?;
    let bytes = fs::read(root.join("manifest.json"))
        .map_err(|e| io_problem("read snapshot manifest", &e))?;
    if checksum(&bytes) != expected.to_ascii_lowercase() {
        return Err(checksum_problem());
    }
    let manifest: SnapshotManifest = serde_json::from_slice(&bytes)
        .map_err(|e| serialization_problem("deserialize snapshot manifest", &e))?;
    if manifest.version != MANIFEST_VERSION {
        return Err(problem(
            "backup_manifest_version_unsupported",
            "Backup manifest version unsupported",
            409,
        ));
    }
    let mut listed = BTreeSet::new();
    let files_root = root.join("files");
    for file in &manifest.files {
        validate_sha256(&file.sha256)?;
        let relative = parse_portable_path(&file.path)?;
        if !listed.insert(file.path.clone()) {
            return Err(invalid_manifest());
        }
        let data = fs::read(files_root.join(relative))
            .map_err(|e| io_problem("read snapshot file", &e))?;
        if data.len() as u64 != file.bytes || checksum(&data) != file.sha256 {
            return Err(checksum_problem());
        }
    }
    let mut paths = Vec::new();
    collect_files(&files_root, Path::new(""), &mut paths)?;
    let actual = paths
        .iter()
        .map(|p| portable_path(p))
        .collect::<Result<BTreeSet<_>, _>>()?;
    if actual != listed {
        return Err(invalid_manifest());
    }
    Ok(VerifiedSnapshot {
        root,
        manifest,
        manifest_sha256: expected.to_ascii_lowercase(),
    })
}

fn restore_replacing(
    snapshot: &VerifiedSnapshot,
    destination: &Path,
    safety_root: &Path,
    safety_id: &str,
) -> Result<(), Problem> {
    if !destination.exists() {
        return restore_snapshot(snapshot, destination, RestoreBoundary::CreateOnly);
    }
    reject_symlink(destination)?;
    if !destination.is_dir() {
        return Err(problem(
            "backup_restore_destination_invalid",
            "Restore destination must be a directory",
            400,
        ));
    }
    create_snapshot(destination, safety_root, safety_id)?;
    let staging = sibling_temp(destination, "restore")?;
    fs::create_dir(&staging).map_err(|e| io_problem("create restore staging", &e))?;
    if let Err(e) = copy_verified_files(snapshot, &staging) {
        let _ = fs::remove_dir_all(&staging);
        return Err(e);
    }
    let displaced = sibling_temp(destination, "displaced")?;
    fs::rename(destination, &displaced)
        .map_err(|e| io_problem("preserve restore destination", &e))?;
    if let Err(e) = fs::rename(&staging, destination) {
        let _ = fs::rename(&displaced, destination);
        let _ = fs::remove_dir_all(&staging);
        return Err(io_problem("publish restored data", &e));
    }
    fs::remove_dir_all(&displaced).map_err(|e| io_problem("remove displaced restore data", &e))
}

fn copy_verified_files(snapshot: &VerifiedSnapshot, destination: &Path) -> Result<(), Problem> {
    for file in &snapshot.manifest.files {
        let relative = parse_portable_path(&file.path)?;
        let bytes = fs::read(snapshot.root.join("files").join(&relative))
            .map_err(|e| io_problem("read snapshot file", &e))?;
        if bytes.len() as u64 != file.bytes || checksum(&bytes) != file.sha256 {
            return Err(checksum_problem());
        }
        write_new(&destination.join(relative), &bytes)?;
    }
    Ok(())
}

fn collect_files(root: &Path, relative: &Path, files: &mut Vec<PathBuf>) -> Result<(), Problem> {
    let mut entries = fs::read_dir(root.join(relative))
        .map_err(|e| io_problem("read backup directory", &e))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| io_problem("read backup entry", &e))?;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let kind = entry
            .file_type()
            .map_err(|e| io_problem("inspect backup entry", &e))?;
        if kind.is_symlink() {
            return Err(problem(
                "backup_symlink_rejected",
                "Backup symlinks are not allowed",
                400,
            ));
        }
        let child = relative.join(entry.file_name());
        if kind.is_dir() {
            collect_files(root, &child, files)?;
        } else if kind.is_file() {
            files.push(child);
        } else {
            return Err(problem(
                "backup_file_type_unsupported",
                "Backup file type unsupported",
                400,
            ));
        }
    }
    Ok(())
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), Problem> {
    let parent = path
        .parent()
        .ok_or_else(|| problem("backup_path_invalid", "Backup path is invalid", 400))?;
    fs::create_dir_all(parent).map_err(|e| io_problem("create backup directory", &e))?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| io_problem("create backup file", &e))?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| io_problem("write backup file", &e))
}

fn set_readonly(root: &Path) -> Result<(), Problem> {
    let mut files = Vec::new();
    collect_files(root, Path::new(""), &mut files)?;
    for relative in files {
        let path = root.join(relative);
        let mut permissions = fs::metadata(&path)
            .map_err(|e| io_problem("inspect snapshot permissions", &e))?
            .permissions();
        permissions.set_readonly(true);
        fs::set_permissions(path, permissions).map_err(|e| io_problem("lock snapshot file", &e))?;
    }
    Ok(())
}

fn make_writable(root: &Path) -> Result<(), Problem> {
    if !root.exists() {
        return Ok(());
    }
    let mut files = Vec::new();
    collect_files(root, Path::new(""), &mut files)?;
    for relative in files {
        let path = root.join(relative);
        let mut permissions = fs::metadata(&path)
            .map_err(|e| io_problem("inspect snapshot permissions", &e))?
            .permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            permissions.set_mode(permissions.mode() | 0o200);
        }
        #[cfg(windows)]
        {
            // Windows uses the read-only file attribute; it has no Unix mode bits.
            #[allow(clippy::permissions_set_readonly_false)]
            permissions.set_readonly(false);
        }
        #[cfg(not(any(unix, windows)))]
        permissions.set_readonly(false);
        fs::set_permissions(path, permissions)
            .map_err(|e| io_problem("unlock snapshot file", &e))?;
    }
    Ok(())
}

fn reject_symlink(path: &Path) -> Result<(), Problem> {
    if path
        .symlink_metadata()
        .map_err(|e| io_problem("inspect backup path", &e))?
        .file_type()
        .is_symlink()
    {
        Err(problem(
            "backup_symlink_rejected",
            "Backup symlinks are not allowed",
            400,
        ))
    } else {
        Ok(())
    }
}

fn portable_path(path: &Path) -> Result<String, Problem> {
    let mut parts = Vec::new();
    for component in path.components() {
        let std::path::Component::Normal(part) = component else {
            return Err(invalid_manifest_path());
        };
        let part = part.to_str().ok_or_else(invalid_manifest_path)?;
        if part.is_empty() || part.contains(['/', '\\']) {
            return Err(invalid_manifest_path());
        }
        parts.push(part);
    }
    if parts.is_empty() {
        return Err(invalid_manifest_path());
    }
    Ok(parts.join("/"))
}

fn parse_portable_path(path: &str) -> Result<PathBuf, Problem> {
    if path.is_empty() || path.contains('\\') {
        return Err(invalid_manifest_path());
    }
    let mut result = PathBuf::new();
    for part in path.split('/') {
        if part.is_empty() || matches!(part, "." | "..") || part.contains(':') {
            return Err(invalid_manifest_path());
        }
        result.push(part);
    }
    Ok(result)
}

fn validate_snapshot_id(value: &str) -> Result<(), Problem> {
    let valid = !value.is_empty()
        && value.len() <= 128
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    if valid {
        Ok(())
    } else {
        Err(problem(
            "backup_snapshot_id_invalid",
            "Backup snapshot ID is invalid",
            400,
        ))
    }
}

fn validate_sha256(value: &str) -> Result<(), Problem> {
    if value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(problem(
            "backup_checksum_invalid",
            "Backup checksum is invalid",
            400,
        ))
    }
}

fn sibling_temp(path: &Path, label: &str) -> Result<PathBuf, Problem> {
    let parent = path.parent().ok_or_else(|| {
        problem(
            "backup_restore_destination_invalid",
            "Restore destination is invalid",
            400,
        )
    })?;
    let name = path.file_name().and_then(|n| n.to_str()).ok_or_else(|| {
        problem(
            "backup_restore_destination_invalid",
            "Restore destination is invalid",
            400,
        )
    })?;
    Ok(parent.join(format!(
        ".{name}.{label}.{}-{}",
        std::process::id(),
        TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )))
}

fn invalid_manifest_path() -> Problem {
    problem(
        "backup_manifest_path_invalid",
        "Backup manifest path is invalid",
        409,
    )
}
fn invalid_manifest() -> Problem {
    problem("backup_manifest_invalid", "Backup manifest is invalid", 409)
}
fn checksum_problem() -> Problem {
    problem("backup_checksum_mismatch", "Backup checksum mismatch", 409)
}
fn problem(code: &str, title: &str, status: u16) -> Problem {
    Problem::new(code, title, status, "storage")
}
fn io_problem(operation: &str, error: &std::io::Error) -> Problem {
    Problem::new("backup_io_error", "Backup operation failed", 500, "storage")
        .with_detail(format!("Failed to {operation}: {error}"))
        .retryable(true)
}
fn serialization_problem(operation: &str, error: &dyn std::fmt::Display) -> Problem {
    Problem::new(
        "backup_manifest_invalid",
        "Backup manifest is invalid",
        409,
        "storage",
    )
    .with_detail(format!("Failed to {operation}: {error}"))
}
