use buglogin_domain::errors::Problem;
use buglogin_storage::{blob::checksum, remote::RemoteChunkStore};

use crate::crypto::{AuthenticatedChunk, ClientKey};

pub const DEFAULT_CHUNK_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EncryptedChunk {
    pub index: u64,
    pub object_key: String,
    pub plaintext_checksum: String,
    pub ciphertext_checksum: String,
    pub plaintext_size: u64,
    pub bytes: Vec<u8>,
}

impl EncryptedChunk {
    pub fn decrypt(&self, key: &ClientKey) -> Result<Vec<u8>, Problem> {
        if checksum(&self.bytes) != self.ciphertext_checksum {
            return Err(chunk_problem("encrypted_chunk_checksum_mismatch", false));
        }
        let authenticated = AuthenticatedChunk::decode(&self.bytes)?;
        let plaintext = authenticated.decrypt(key, self.object_key.as_bytes())?;
        if plaintext.len() as u64 != self.plaintext_size
            || checksum(&plaintext) != self.plaintext_checksum
        {
            return Err(chunk_problem("plaintext_chunk_checksum_mismatch", false));
        }
        Ok(plaintext)
    }
}

pub fn encrypt_chunks(
    key: &ClientKey,
    object_prefix: &str,
    plaintext: &[u8],
    chunk_bytes: usize,
) -> Result<Vec<EncryptedChunk>, Problem> {
    validate_prefix(object_prefix)?;
    if chunk_bytes == 0 {
        return Err(chunk_problem("invalid_chunk_size", false));
    }
    plaintext
        .chunks(chunk_bytes)
        .enumerate()
        .map(|(index, chunk)| {
            let plaintext_checksum = checksum(chunk);
            let object_key = format!("{object_prefix}/{index:016x}-{plaintext_checksum}");
            let authenticated = AuthenticatedChunk::encrypt(key, chunk, object_key.as_bytes())?;
            let bytes = authenticated.encoded();
            Ok(EncryptedChunk {
                index: index as u64,
                object_key,
                plaintext_checksum,
                ciphertext_checksum: checksum(&bytes),
                plaintext_size: chunk.len() as u64,
                bytes,
            })
        })
        .collect()
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct UploadReceipt {
    pub uploaded: usize,
    pub resumed: usize,
    pub retries: usize,
}

pub fn upload_resumable(
    remote: &mut impl RemoteChunkStore,
    chunks: &[EncryptedChunk],
    max_retries: usize,
) -> Result<UploadReceipt, Problem> {
    let mut receipt = UploadReceipt::default();
    for chunk in chunks {
        match remote.checksum(&chunk.object_key)? {
            Some(existing) if existing == chunk.ciphertext_checksum => {
                receipt.resumed += 1;
                continue;
            }
            Some(_) => return Err(chunk_problem("remote_chunk_conflict", false)),
            None => {}
        }

        let mut attempt = 0;
        loop {
            match remote.put(&chunk.object_key, &chunk.bytes, &chunk.ciphertext_checksum) {
                Ok(()) => {
                    receipt.uploaded += 1;
                    break;
                }
                Err(error) if error.retryable && attempt < max_retries => {
                    attempt += 1;
                    receipt.retries += 1;
                }
                Err(error) => return Err(error),
            }
        }
    }
    Ok(receipt)
}

pub fn download_and_decrypt(
    remote: &impl RemoteChunkStore,
    key: &ClientKey,
    chunks: &[EncryptedChunk],
) -> Result<Vec<u8>, Problem> {
    let mut ordered = chunks.to_vec();
    ordered.sort_by_key(|chunk| chunk.index);
    let capacity = ordered
        .iter()
        .map(|chunk| chunk.plaintext_size as usize)
        .sum();
    let mut plaintext = Vec::with_capacity(capacity);
    for chunk in ordered {
        let remote_chunk = remote.get(&chunk.object_key, &chunk.ciphertext_checksum)?;
        let mut downloaded = chunk;
        downloaded.bytes = remote_chunk.bytes;
        plaintext.extend_from_slice(&downloaded.decrypt(key)?);
    }
    Ok(plaintext)
}

fn validate_prefix(prefix: &str) -> Result<(), Problem> {
    if prefix.is_empty()
        || prefix.starts_with('/')
        || prefix.contains("..")
        || prefix.contains('\\')
    {
        return Err(chunk_problem("invalid_chunk_prefix", false));
    }
    Ok(())
}

fn chunk_problem(code: &str, retryable: bool) -> Problem {
    Problem::new(code, "Encrypted chunk transfer failed", 409, "sync").retryable(retryable)
}
