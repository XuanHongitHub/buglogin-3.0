use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::Read;
use std::path::{Component, Path};

use buglogin_domain::errors::Problem;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use unicode_normalization::UnicodeNormalization;

use crate::verify::validate_path_length;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotEntry {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceSnapshot {
    #[serde(skip)]
    pub source_root: std::path::PathBuf,
    #[serde(skip)]
    integrity_tree_sha256: String,
    pub snapshot_id: String,
    pub source_tree_sha256: String,
    pub total_bytes: u64,
    pub files: Vec<SnapshotEntry>,
}

impl fmt::Debug for SourceSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SourceSnapshot")
            .field("source_root", &self.source_root)
            .field("snapshot_id", &self.snapshot_id)
            .field("source_tree_sha256", &self.source_tree_sha256)
            .field("total_bytes", &self.total_bytes)
            .field("files", &self.files)
            .finish()
    }
}

impl SourceSnapshot {
    pub fn has_same_integrity(&self, other: &Self) -> bool {
        !self.integrity_tree_sha256.is_empty()
            && !other.integrity_tree_sha256.is_empty()
            && self.integrity_tree_sha256 == other.integrity_tree_sha256
    }
}

struct CollectedFile {
    entry: SnapshotEntry,
    integrity_sha256: String,
}

pub fn create_snapshot(source_root: &Path) -> Result<SourceSnapshot, Problem> {
    validate_source_path(source_root)?;
    let metadata = fs::symlink_metadata(source_root).map_err(|_| source_unavailable())?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(source_unavailable());
    }

    let mut collected = Vec::new();
    collect_files(source_root, source_root, &mut collected)?;
    collected.sort_by(|left, right| left.entry.path.cmp(&right.entry.path));
    reject_canonical_path_collisions(&collected)?;

    let total_bytes = collected.iter().try_fold(0_u64, |total, file| {
        total.checked_add(file.entry.bytes).ok_or_else(|| {
            Problem::new(
                "migration_source_too_large",
                "Migration source is too large",
                413,
                "migration",
            )
        })
    })?;
    let mut tree = Sha256::new();
    let mut integrity_tree = Sha256::new();
    for file in &collected {
        hash_field(&mut tree, file.entry.path.as_bytes());
        hash_field(&mut tree, &file.entry.bytes.to_be_bytes());
        hash_field(&mut tree, file.entry.sha256.as_bytes());
        hash_field(&mut integrity_tree, file.entry.path.as_bytes());
        hash_field(&mut integrity_tree, &file.entry.bytes.to_be_bytes());
        hash_field(&mut integrity_tree, file.integrity_sha256.as_bytes());
    }
    let source_tree_sha256 = format!("{:x}", tree.finalize());
    let integrity_tree_sha256 = format!("{:x}", integrity_tree.finalize());

    Ok(SourceSnapshot {
        source_root: source_root.to_path_buf(),
        integrity_tree_sha256,
        snapshot_id: format!("sha256:{source_tree_sha256}"),
        source_tree_sha256,
        total_bytes,
        files: collected.into_iter().map(|file| file.entry).collect(),
    })
}

pub fn validate_source_path(path: &Path) -> Result<(), Problem> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|_| source_unavailable())?
            .join(path)
    };
    for ancestor in absolute.ancestors() {
        if ancestor.as_os_str().is_empty() {
            continue;
        }
        let metadata = fs::symlink_metadata(ancestor).map_err(|_| source_unavailable())?;
        if metadata.file_type().is_symlink() && !is_system_managed_link(&metadata) {
            return Err(source_symlink_unsupported());
        }
    }
    Ok(())
}

/// Root-owned links such as macOS `/var -> /private/var` belong to the OS, not
/// to whoever supplied the source path, so they cannot redirect a migration.
#[cfg(unix)]
fn is_system_managed_link(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    metadata.uid() == 0
}

#[cfg(not(unix))]
fn is_system_managed_link(_metadata: &fs::Metadata) -> bool {
    false
}

const CHROMIUM_RUNTIME_LINKS: [&str; 4] = [
    "SingletonLock",
    "SingletonSocket",
    "SingletonCookie",
    "RunningChromeVersion",
];

/// Chromium writes these lock links into every running (or crashed) profile;
/// they are machine-local state, never user data.
pub fn is_chromium_runtime_link(file_type: fs::FileType, name: &std::ffi::OsStr) -> bool {
    file_type.is_symlink()
        && name
            .to_str()
            .is_some_and(|name| CHROMIUM_RUNTIME_LINKS.contains(&name))
}

/// Legacy data roots carry downloaded browser binaries (with app-bundle links)
/// and Chromium lock links; migration skips them instead of failing closed.
pub fn is_legacy_runtime_artifact(relative: &Path, file_type: fs::FileType) -> bool {
    relative == Path::new("binaries")
        || relative
            .file_name()
            .is_some_and(|name| is_chromium_runtime_link(file_type, name))
}

fn reject_canonical_path_collisions(files: &[CollectedFile]) -> Result<(), Problem> {
    let mut paths = BTreeMap::new();
    for file in files {
        let canonical = file
            .entry
            .path
            .nfc()
            .flat_map(char::to_lowercase)
            .collect::<String>();
        if let Some(existing) = paths.insert(canonical.clone(), file.entry.path.as_str()) {
            if existing != file.entry.path {
                return Err(Problem::new(
                    "migration_source_path_collision",
                    "Migration source contains case or Unicode-equivalent paths",
                    422,
                    "migration",
                )
                .with_field("canonicalPath", canonical)
                .with_field("firstPath", existing)
                .with_field("secondPath", file.entry.path.clone()));
            }
        }
    }
    Ok(())
}

fn collect_files(
    root: &Path,
    directory: &Path,
    files: &mut Vec<CollectedFile>,
) -> Result<(), Problem> {
    let entries = fs::read_dir(directory).map_err(|_| source_unavailable())?;
    for entry in entries {
        let entry = entry.map_err(|_| source_unavailable())?;
        let file_type = entry.file_type().map_err(|_| source_unavailable())?;
        let path = entry.path();
        let relative = path.strip_prefix(root).map_err(|_| source_unavailable())?;
        if is_legacy_runtime_artifact(relative, file_type) {
            continue;
        }
        if file_type.is_symlink() {
            return Err(source_symlink_unsupported());
        }
        if file_type.is_dir() {
            collect_files(root, &path, files)?;
            continue;
        }
        if !file_type.is_file() {
            return Err(Problem::new(
                "migration_source_entry_unsupported",
                "Migration source contains an unsupported entry",
                422,
                "migration",
            ));
        }

        let mut file = OpenOptions::new()
            .read(true)
            .open(&path)
            .map_err(|error| source_read_error(&error))?;
        FileExt::try_lock_shared(&file).map_err(|_| source_locked())?;
        let before = file.metadata().map_err(|_| source_unavailable())?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .map_err(|error| source_read_error(&error))?;
        let after = file.metadata().map_err(|_| source_unavailable())?;
        if before.len() != after.len()
            || after.len() != bytes.len() as u64
            || before.modified().ok() != after.modified().ok()
        {
            return Err(Problem::new(
                "migration_source_changed",
                "Migration source changed during inspection",
                409,
                "migration",
            )
            .retryable(true));
        }
        let relative = relative_path(root, &path)?;
        let integrity_sha256 = buglogin_storage::blob::checksum(&bytes);
        files.push(CollectedFile {
            entry: SnapshotEntry {
                path: relative.clone(),
                bytes: bytes.len() as u64,
                sha256: if is_sensitive_file(&relative, &bytes) {
                    "redacted".to_owned()
                } else {
                    integrity_sha256.clone()
                },
            },
            integrity_sha256,
        });
    }
    Ok(())
}

fn source_symlink_unsupported() -> Problem {
    Problem::new(
        "migration_source_symlink_unsupported",
        "Migration source contains an unsupported symlink",
        422,
        "migration",
    )
}

fn relative_path(root: &Path, path: &Path) -> Result<String, Problem> {
    let relative = path.strip_prefix(root).map_err(|_| invalid_source_path())?;
    let mut parts = Vec::new();
    for component in relative.components() {
        let Component::Normal(part) = component else {
            return Err(invalid_source_path());
        };
        parts.push(part.to_str().ok_or_else(invalid_source_path)?);
    }
    let path = parts.join("/");
    validate_path_length(&path, "migration_source_path_too_long")?;
    Ok(path)
}

fn is_sensitive_file(path: &str, bytes: &[u8]) -> bool {
    let normalized_path = path.to_ascii_lowercase();
    let path_sensitive = normalized_path.split('/').any(|component| {
        component.contains("cookie")
            || component.contains("credential")
            || component.contains("password")
            || component.contains("secret")
            || component.contains("token")
            || component.contains("api_key")
            || component.contains("apikey")
            || component.contains("proxy")
            || component.contains("vpn")
    });
    path_sensitive
        || serde_json::from_slice::<Value>(bytes)
            .is_ok_and(|value| contains_sensitive_json_field(&value))
}

fn contains_sensitive_json_field(value: &Value) -> bool {
    match value {
        Value::Object(object) => object.iter().any(|(key, child)| {
            let normalized = key.to_ascii_lowercase().replace(['-', '_'], "");
            (matches!(
                normalized.as_str(),
                "password"
                    | "proxypassword"
                    | "token"
                    | "accesstoken"
                    | "refreshtoken"
                    | "apikey"
                    | "secret"
                    | "privatekey"
                    | "cookie"
                    | "cookies"
            ) && !child.is_null())
                || contains_sensitive_json_field(child)
        }),
        Value::Array(values) => values.iter().any(contains_sensitive_json_field),
        _ => false,
    }
}

fn hash_field(hasher: &mut Sha256, value: &[u8]) {
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value);
}

fn source_unavailable() -> Problem {
    Problem::new(
        "migration_source_unavailable",
        "Migration source is unavailable",
        404,
        "migration",
    )
}

fn source_read_error(error: &std::io::Error) -> Problem {
    #[cfg(windows)]
    if matches!(error.raw_os_error(), Some(32 | 33)) {
        return Problem::new(
            "migration_source_locked",
            "Migration source file is locked by another process",
            423,
            "migration",
        );
    }
    source_unavailable()
}

fn source_locked() -> Problem {
    Problem::new(
        "migration_source_locked",
        "Migration source file is locked by another process",
        423,
        "migration",
    )
}

fn invalid_source_path() -> Problem {
    Problem::new(
        "migration_source_path_invalid",
        "Migration source path is invalid",
        422,
        "migration",
    )
}
