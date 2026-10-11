use std::collections::BTreeMap;

use buglogin_domain::errors::Problem;
use sha2::{Digest, Sha256};

use crate::blob::checksum;

#[cfg(feature = "production-adapters")]
mod s3;

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
    ) -> Result<bool, Problem> {
        let _ = (object_key, expected_checksum);
        Err(Problem::new(
            "remote_chunk_delete_unsupported",
            "Remote chunk delete is unsupported",
            501,
            "storage",
        ))
    }
}

#[derive(Clone, Debug, Default)]
pub struct MemoryRemoteChunkStore {
    chunks: BTreeMap<String, RemoteChunk>,
}

impl RemoteChunkStore for MemoryRemoteChunkStore {
    fn checksum(&self, object_key: &str) -> Result<Option<String>, Problem> {
        validate_object_key(object_key)?;
        Ok(self
            .chunks
            .get(object_key)
            .map(|chunk| chunk.checksum.clone()))
    }

    fn put(
        &mut self,
        object_key: &str,
        bytes: &[u8],
        expected_checksum: &str,
    ) -> Result<(), Problem> {
        validate_object_key(object_key)?;
        let actual_checksum = checksum(bytes);
        if actual_checksum != expected_checksum {
            return Err(checksum_problem());
        }
        if let Some(existing) = self.chunks.get(object_key) {
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
        self.chunks.insert(
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
        let chunk = self.chunks.get(object_key).ok_or_else(|| {
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
        let Some(existing) = self.chunks.get(object_key) else {
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
        self.chunks.remove(object_key);
        Ok(true)
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
