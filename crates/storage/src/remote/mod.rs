use std::{
    collections::BTreeMap,
    sync::{Mutex, RwLock},
};

use buglogin_domain::errors::Problem;
use sha2::{Digest, Sha256};

use crate::blob::checksum;

pub const MAX_REMOTE_OBJECT_BYTES: u64 = 64 * 1024 * 1024;

pub(crate) fn validate_remote_object_size(size: u64) -> Result<(), Problem> {
    if size > MAX_REMOTE_OBJECT_BYTES {
        return Err(Problem::new(
            "remote_chunk_too_large",
            "Remote object exceeds the configured size limit",
            413,
            "storage",
        ));
    }
    Ok(())
}

#[cfg(feature = "production-adapters")]
mod s3;

mod local;

pub use local::LocalRemoteChunkStore;

#[cfg(feature = "production-adapters")]
pub use s3::{S3Config, S3RemoteChunkStore};

pub fn sha256_bytes(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteChunk {
    pub bytes: Vec<u8>,
    pub checksum: String,
}

pub trait RemoteChunkStore: Send {
    fn readiness(&self) -> Result<(), Problem>;

    fn checksum(&self, object_key: &str) -> Result<Option<String>, Problem>;

    fn put(
        &mut self,
        object_key: &str,
        bytes: &[u8],
        expected_checksum: &str,
    ) -> Result<(), Problem>;

    fn get(&self, object_key: &str, expected_checksum: &str) -> Result<RemoteChunk, Problem>;

    fn delete_if_checksum(
        &mut self,
        object_key: &str,
        expected_checksum: &str,
    ) -> Result<bool, Problem>;
}

/// Shared adapter surface for servers that can run object operations concurrently.
/// Legacy `RemoteChunkStore` implementations remain usable through
/// `SerializedRemoteChunkStore` without changing their public method signatures.
pub trait ConcurrentRemoteChunkStore: Send + Sync {
    fn readiness(&self) -> Result<(), Problem>;

    fn checksum(&self, object_key: &str) -> Result<Option<String>, Problem>;

    fn put(&self, object_key: &str, bytes: &[u8], expected_checksum: &str) -> Result<(), Problem>;

    fn get(&self, object_key: &str, expected_checksum: &str) -> Result<RemoteChunk, Problem>;

    fn delete_if_checksum(
        &self,
        object_key: &str,
        expected_checksum: &str,
    ) -> Result<bool, Problem>;
}

pub struct SerializedRemoteChunkStore {
    inner: Mutex<Box<dyn RemoteChunkStore>>,
}

impl SerializedRemoteChunkStore {
    pub fn new(inner: Box<dyn RemoteChunkStore>) -> Self {
        Self {
            inner: Mutex::new(inner),
        }
    }

    fn with<T>(
        &self,
        operation: impl FnOnce(&mut dyn RemoteChunkStore) -> Result<T, Problem>,
    ) -> Result<T, Problem> {
        let mut inner = self.inner.lock().map_err(|_| storage_unavailable())?;
        operation(inner.as_mut())
    }
}

impl ConcurrentRemoteChunkStore for SerializedRemoteChunkStore {
    fn readiness(&self) -> Result<(), Problem> {
        self.with(|inner| inner.readiness())
    }

    fn checksum(&self, object_key: &str) -> Result<Option<String>, Problem> {
        self.with(|inner| inner.checksum(object_key))
    }

    fn put(&self, object_key: &str, bytes: &[u8], expected_checksum: &str) -> Result<(), Problem> {
        self.with(|inner| inner.put(object_key, bytes, expected_checksum))
    }

    fn get(&self, object_key: &str, expected_checksum: &str) -> Result<RemoteChunk, Problem> {
        self.with(|inner| inner.get(object_key, expected_checksum))
    }

    fn delete_if_checksum(
        &self,
        object_key: &str,
        expected_checksum: &str,
    ) -> Result<bool, Problem> {
        self.with(|inner| inner.delete_if_checksum(object_key, expected_checksum))
    }
}

#[derive(Clone, Debug, Default)]
pub struct MemoryRemoteChunkStore {
    chunks: std::sync::Arc<RwLock<BTreeMap<String, RemoteChunk>>>,
}

impl RemoteChunkStore for MemoryRemoteChunkStore {
    fn readiness(&self) -> Result<(), Problem> {
        Ok(())
    }

    fn checksum(&self, object_key: &str) -> Result<Option<String>, Problem> {
        validate_object_key(object_key)?;
        let chunks = self.chunks.read().map_err(|_| storage_unavailable())?;
        Ok(chunks.get(object_key).map(|chunk| chunk.checksum.clone()))
    }

    fn put(
        &mut self,
        object_key: &str,
        bytes: &[u8],
        expected_checksum: &str,
    ) -> Result<(), Problem> {
        validate_object_key(object_key)?;
        validate_remote_object_size(bytes.len() as u64)?;
        let actual_checksum = checksum(bytes);
        if actual_checksum != expected_checksum {
            return Err(checksum_problem());
        }
        let mut chunks = self.chunks.write().map_err(|_| storage_unavailable())?;
        if let Some(existing) = chunks.get(object_key) {
            if existing.checksum == expected_checksum {
                return Ok(());
            }
            return Err(Problem::new(
                "remote_chunk_conflict",
                "Remote chunk conflict",
                409,
                "storage",
            ));
        }
        chunks.insert(
            object_key.to_owned(),
            RemoteChunk {
                bytes: bytes.to_vec(),
                checksum: actual_checksum,
            },
        );
        Ok(())
    }

    fn get(&self, object_key: &str, expected_checksum: &str) -> Result<RemoteChunk, Problem> {
        validate_object_key(object_key)?;
        let chunks = self.chunks.read().map_err(|_| storage_unavailable())?;
        let chunk = chunks.get(object_key).ok_or_else(|| {
            Problem::new(
                "remote_chunk_not_found",
                "Remote chunk not found",
                404,
                "storage",
            )
        })?;
        if chunk.checksum != expected_checksum || checksum(&chunk.bytes) != expected_checksum {
            return Err(checksum_problem());
        }
        Ok(chunk.clone())
    }

    fn delete_if_checksum(
        &mut self,
        object_key: &str,
        expected_checksum: &str,
    ) -> Result<bool, Problem> {
        validate_object_key(object_key)?;
        let mut chunks = self.chunks.write().map_err(|_| storage_unavailable())?;
        let Some(existing) = chunks.get(object_key) else {
            return Ok(false);
        };
        if existing.checksum != expected_checksum {
            return Err(Problem::new(
                "remote_chunk_conflict",
                "Remote chunk conflict",
                409,
                "storage",
            ));
        }
        chunks.remove(object_key);
        Ok(true)
    }
}

impl ConcurrentRemoteChunkStore for MemoryRemoteChunkStore {
    fn readiness(&self) -> Result<(), Problem> {
        Ok(())
    }

    fn checksum(&self, object_key: &str) -> Result<Option<String>, Problem> {
        RemoteChunkStore::checksum(self, object_key)
    }

    fn put(&self, object_key: &str, bytes: &[u8], expected_checksum: &str) -> Result<(), Problem> {
        let mut adapter = self.clone();
        RemoteChunkStore::put(&mut adapter, object_key, bytes, expected_checksum)
    }

    fn get(&self, object_key: &str, expected_checksum: &str) -> Result<RemoteChunk, Problem> {
        RemoteChunkStore::get(self, object_key, expected_checksum)
    }

    fn delete_if_checksum(
        &self,
        object_key: &str,
        expected_checksum: &str,
    ) -> Result<bool, Problem> {
        let mut adapter = self.clone();
        RemoteChunkStore::delete_if_checksum(&mut adapter, object_key, expected_checksum)
    }
}

pub(crate) fn validate_object_key(object_key: &str) -> Result<(), Problem> {
    if object_key.is_empty()
        || object_key.len() > 512
        || object_key.starts_with('/')
        || object_key.contains("..")
        || object_key.contains('\\')
    {
        return Err(Problem::new(
            "invalid_remote_chunk_key",
            "Invalid remote chunk key",
            400,
            "storage",
        ));
    }
    Ok(())
}

pub(crate) fn checksum_problem() -> Problem {
    Problem::new(
        "remote_chunk_checksum_mismatch",
        "Remote chunk checksum mismatch",
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

#[cfg(test)]
mod tests {
    use super::{MAX_REMOTE_OBJECT_BYTES, validate_remote_object_size};

    #[test]
    fn remote_object_size_limit_is_inclusive_and_bounded() {
        assert!(validate_remote_object_size(MAX_REMOTE_OBJECT_BYTES).is_ok());
        let error = validate_remote_object_size(MAX_REMOTE_OBJECT_BYTES + 1).unwrap_err();
        assert_eq!(error.code, "remote_chunk_too_large");
        assert_eq!(error.status, 413);
    }
}
