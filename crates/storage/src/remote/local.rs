//! Durable, filesystem-backed object storage for a single local API instance.
//!
//! Object keys are mapped to flat SHA-256 filenames. The original key never
//! becomes part of a filesystem path, so separators, drive syntax, and NTFS
//! alternate data stream syntax cannot affect path resolution.

use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use buglogin_domain::errors::Problem;
use sha2::{Digest, Sha256};

use super::{
    ConcurrentRemoteChunkStore, MAX_REMOTE_OBJECT_BYTES, RemoteChunk, checksum_problem,
    validate_object_key, validate_remote_object_size,
};
use crate::blob::checksum;

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);
const LOCK_STRIPES: usize = 257;
const OBJECT_DIR: &str = "objects";

/// Filesystem-backed chunk store intended for a private, OS-local application
/// data directory. It is not a multi-process coordination mechanism: callers
/// must ensure there is only one API process writing this store at a time.
pub struct LocalRemoteChunkStore {
    root: PathBuf,
    objects: PathBuf,
    locks: Vec<Mutex<()>>,
}

impl LocalRemoteChunkStore {
    /// Opens or creates the store under `root`. The caller must provide an
    /// OS-local data path (for example `%LOCALAPPDATA%\\BugLogin\\api`).
    pub fn new(root: impl AsRef<Path>) -> Result<Self, Problem> {
        let requested_root = root.as_ref();
        fs::create_dir_all(requested_root).map_err(|_| storage_unavailable())?;
        reject_symlink(requested_root)?;
        let root = fs::canonicalize(requested_root).map_err(|_| storage_unavailable())?;
        reject_symlink(&root)?;

        let objects = root.join(OBJECT_DIR);
        match fs::symlink_metadata(&objects) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    return Err(storage_unavailable());
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&objects).map_err(|_| storage_unavailable())?;
            }
            Err(_) => return Err(storage_unavailable()),
        }
        let canonical_objects = fs::canonicalize(&objects).map_err(|_| storage_unavailable())?;
        if canonical_objects.parent() != Some(root.as_path()) {
            return Err(storage_unavailable());
        }

        Ok(Self {
            root,
            objects: canonical_objects,
            locks: (0..LOCK_STRIPES).map(|_| Mutex::new(())).collect(),
        })
    }

    fn lock_for(&self, object_key: &str) -> Result<std::sync::MutexGuard<'_, ()>, Problem> {
        let digest = Sha256::digest(object_key.as_bytes());
        let index = u64::from_be_bytes(digest[..8].try_into().expect("fixed digest width"))
            as usize
            % self.locks.len();
        self.locks[index].lock().map_err(|_| storage_unavailable())
    }

    fn object_path(&self, object_key: &str) -> Result<PathBuf, Problem> {
        validate_object_key(object_key)?;
        self.ensure_store_directories()?;
        let filename = format!("{:x}.chunk", Sha256::digest(object_key.as_bytes()));
        Ok(self.objects.join(filename))
    }

    fn ensure_store_directories(&self) -> Result<(), Problem> {
        reject_symlink(&self.root)?;
        reject_symlink(&self.objects)?;
        let root = fs::canonicalize(&self.root).map_err(|_| storage_unavailable())?;
        let objects = fs::canonicalize(&self.objects).map_err(|_| storage_unavailable())?;
        if root != self.root || objects != self.objects || objects.parent() != Some(root.as_path())
        {
            return Err(storage_unavailable());
        }
        Ok(())
    }

    fn read_object(&self, path: &Path) -> Result<Option<Vec<u8>>, Problem> {
        match fs::symlink_metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(storage_unavailable()),
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                return Err(storage_unavailable());
            }
            Ok(_) => {}
        }

        // O_NOFOLLOW / FILE_FLAG_OPEN_REPARSE_POINT closes the check/open race
        // for a leaf replaced with a symbolic link or reparse point.
        let mut options = OpenOptions::new();
        options.read(true);
        no_follow(&mut options);
        let file = match options.open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(storage_unavailable()),
        };
        let metadata = file.metadata().map_err(|_| storage_unavailable())?;
        if !metadata.is_file() || metadata.len() > MAX_REMOTE_OBJECT_BYTES {
            return Err(storage_unavailable());
        }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        file.take(MAX_REMOTE_OBJECT_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| storage_unavailable())?;
        validate_remote_object_size(bytes.len() as u64)?;
        Ok(Some(bytes))
    }
}

impl ConcurrentRemoteChunkStore for LocalRemoteChunkStore {
    fn readiness(&self) -> Result<(), Problem> {
        self.ensure_store_directories()
    }

    fn checksum(&self, object_key: &str) -> Result<Option<String>, Problem> {
        let _guard = self.lock_for(object_key)?;
        let path = self.object_path(object_key)?;
        Ok(self.read_object(&path)?.map(|bytes| checksum(&bytes)))
    }

    fn put(&self, object_key: &str, bytes: &[u8], expected_checksum: &str) -> Result<(), Problem> {
        validate_object_key(object_key)?;
        validate_remote_object_size(bytes.len() as u64)?;
        let actual_checksum = checksum(bytes);
        if actual_checksum != expected_checksum {
            return Err(checksum_problem());
        }
        let _guard = self.lock_for(object_key)?;
        let target = self.object_path(object_key)?;
        if let Some(existing) = self.read_object(&target)? {
            return if checksum(&existing) == expected_checksum {
                Ok(())
            } else {
                Err(conflict_problem())
            };
        }

        let (temp_path, mut file) = self.create_temp_file()?;
        let write_result = file.write_all(bytes).and_then(|_| file.sync_all());
        drop(file);
        if write_result.is_err() {
            let _ = fs::remove_file(&temp_path);
            return Err(storage_unavailable());
        }

        // Hard-link publication is an atomic create-if-absent operation on the
        // same filesystem, including Windows; unlike rename it cannot replace
        // an object another process created after our initial existence check.
        let result = match fs::hard_link(&temp_path, &target) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                match self.read_object(&target) {
                    Ok(Some(existing)) if checksum(&existing) == expected_checksum => Ok(()),
                    Ok(Some(_)) => Err(conflict_problem()),
                    Ok(None) | Err(_) => Err(storage_unavailable()),
                }
            }
            Err(_) => Err(storage_unavailable()),
        };
        let _ = fs::remove_file(&temp_path);
        if result.is_ok() {
            sync_directory(&self.objects);
        }
        result
    }

    fn get(&self, object_key: &str, expected_checksum: &str) -> Result<RemoteChunk, Problem> {
        let _guard = self.lock_for(object_key)?;
        let path = self.object_path(object_key)?;
        let bytes = self.read_object(&path)?.ok_or_else(not_found_problem)?;
        let actual_checksum = checksum(&bytes);
        if actual_checksum != expected_checksum {
            return Err(checksum_problem());
        }
        Ok(RemoteChunk {
            bytes,
            checksum: actual_checksum,
        })
    }

    fn delete_if_checksum(
        &self,
        object_key: &str,
        expected_checksum: &str,
    ) -> Result<bool, Problem> {
        let _guard = self.lock_for(object_key)?;
        let path = self.object_path(object_key)?;
        let Some(bytes) = self.read_object(&path)? else {
            return Ok(false);
        };
        if checksum(&bytes) != expected_checksum {
            return Err(conflict_problem());
        }
        fs::remove_file(&path).map_err(|_| storage_unavailable())?;
        sync_directory(&self.objects);
        Ok(true)
    }
}

impl LocalRemoteChunkStore {
    fn create_temp_file(&self) -> Result<(PathBuf, File), Problem> {
        for _ in 0..32 {
            let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = self
                .objects
                .join(format!(".tmp-{}-{sequence}", std::process::id()));
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            no_follow(&mut options);
            match options.open(&path) {
                Ok(file) => return Ok((path, file)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(_) => return Err(storage_unavailable()),
            }
        }
        Err(storage_unavailable())
    }
}

#[cfg(unix)]
fn no_follow(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;
    // Numeric constants are scoped to the platforms where their values are
    // known. Other Unix targets retain the canonical-root and symlink checks.
    #[cfg(target_os = "macos")]
    const O_NOFOLLOW: i32 = 0x100;
    #[cfg(target_os = "linux")]
    const O_NOFOLLOW: i32 = 0x20000;
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    options.custom_flags(O_NOFOLLOW);
}

#[cfg(windows)]
fn no_follow(options: &mut OpenOptions) {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
}

#[cfg(not(any(unix, windows)))]
fn no_follow(_options: &mut OpenOptions) {}

fn reject_symlink(path: &Path) -> Result<(), Problem> {
    let metadata = fs::symlink_metadata(path).map_err(|_| storage_unavailable())?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(storage_unavailable());
    }
    Ok(())
}

fn sync_directory(path: &Path) {
    // Directory handles can be synced on Unix. Windows does not consistently
    // support opening/syncing directories through std; file data itself was
    // flushed before publication there.
    #[cfg(unix)]
    if let Ok(directory) = File::open(path) {
        let _ = directory.sync_all();
    }
    #[cfg(not(unix))]
    let _ = path;
}

fn not_found_problem() -> Problem {
    Problem::new(
        "remote_chunk_not_found",
        "Remote chunk not found",
        404,
        "storage",
    )
}

fn conflict_problem() -> Problem {
    Problem::new(
        "remote_chunk_conflict",
        "Remote chunk conflict",
        409,
        "storage",
    )
}

fn storage_unavailable() -> Problem {
    Problem::new(
        "storage_unavailable",
        "Remote storage unavailable",
        503,
        "storage",
    )
}
