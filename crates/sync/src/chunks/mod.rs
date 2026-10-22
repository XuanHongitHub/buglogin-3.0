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
        validate_chunk_metadata(self)?;
        let authenticated = AuthenticatedChunk::decode(&self.bytes)?;
        let associated_data = logical_chunk_key(&self.object_key, &self.ciphertext_checksum)?;
        let plaintext = authenticated.decrypt(key, associated_data.as_bytes())?;
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
    if chunk_bytes == 0 || chunk_bytes > DEFAULT_CHUNK_BYTES {
        return Err(chunk_problem("invalid_chunk_size", false));
    }
    plaintext
        .chunks(chunk_bytes)
        .enumerate()
        .map(|(index, chunk)| {
            let plaintext_checksum = checksum(chunk);
            let logical_key = format!("{object_prefix}/{index:016x}-{plaintext_checksum}");
            let authenticated = AuthenticatedChunk::encrypt(key, chunk, logical_key.as_bytes())?;
            let bytes = authenticated.encoded();
            let ciphertext_checksum = checksum(&bytes);
            // AES-GCM uses a random nonce, so identical plaintext can produce different
            // ciphertext. Address the immutable blob by its ciphertext digest while keeping
            // authenticated data stable for the logical chunk.
            let object_key = format!("{logical_key}/{ciphertext_checksum}");
            Ok(EncryptedChunk {
                index: index as u64,
                object_key,
                plaintext_checksum,
                ciphertext_checksum,
                plaintext_size: chunk.len() as u64,
                bytes,
            })
        })
        .collect()
}

fn logical_chunk_key(object_key: &str, ciphertext_checksum: &str) -> Result<String, Problem> {
    let suffix = format!("/{ciphertext_checksum}");
    // Older manifests used the logical key itself as the remote object key. Keep those
    // chunks readable while new writes use ciphertext-addressed object keys.
    let logical_key = object_key.strip_suffix(&suffix).unwrap_or(object_key);
    validate_prefix(logical_key)?;
    Ok(logical_key.to_owned())
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
    validate_chunk_set(chunks)?;
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
                    match remote.checksum(&chunk.object_key)? {
                        Some(existing) if existing == chunk.ciphertext_checksum => {
                            receipt.resumed += 1;
                            break;
                        }
                        Some(_) => return Err(chunk_problem("remote_chunk_conflict", false)),
                        None => {}
                    }
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
    validate_chunk_set(chunks)?;
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

fn validate_chunk_set(chunks: &[EncryptedChunk]) -> Result<(), Problem> {
    let mut expected_prefix: Option<String> = None;
    for (expected_index, chunk) in chunks.iter().enumerate() {
        validate_chunk_metadata(chunk)?;
        if chunk.index != expected_index as u64 {
            return Err(chunk_problem("invalid_chunk_sequence", false));
        }
        let logical_key = logical_chunk_key(&chunk.object_key, &chunk.ciphertext_checksum)?;
        let (prefix, _) = logical_key
            .rsplit_once('/')
            .ok_or_else(|| chunk_problem("invalid_chunk_sequence", false))?;
        match &expected_prefix {
            Some(expected) if expected != prefix => {
                return Err(chunk_problem("invalid_chunk_sequence", false));
            }
            None => expected_prefix = Some(prefix.to_owned()),
            _ => {}
        }
    }
    Ok(())
}

fn validate_chunk_metadata(chunk: &EncryptedChunk) -> Result<(), Problem> {
    let expected_ciphertext_size = usize::try_from(chunk.plaintext_size)
        .ok()
        .filter(|size| (1..=DEFAULT_CHUNK_BYTES).contains(size))
        .and_then(|size| size.checked_add(crate::crypto::AUTHENTICATED_CHUNK_OVERHEAD_BYTES));
    if expected_ciphertext_size != Some(chunk.bytes.len()) {
        return Err(chunk_problem("invalid_chunk_size", false));
    }
    if checksum(&chunk.bytes) != chunk.ciphertext_checksum {
        return Err(chunk_problem("encrypted_chunk_checksum_mismatch", false));
    }
    let logical_key = logical_chunk_key(&chunk.object_key, &chunk.ciphertext_checksum)?;
    let (_, leaf) = logical_key
        .rsplit_once('/')
        .ok_or_else(|| chunk_problem("invalid_chunk_sequence", false))?;
    let (index, plaintext_checksum) = leaf
        .split_once('-')
        .ok_or_else(|| chunk_problem("invalid_chunk_sequence", false))?;
    let encoded_index = format!("{:016x}", chunk.index);
    if index != encoded_index || plaintext_checksum != chunk.plaintext_checksum {
        return Err(chunk_problem("invalid_chunk_sequence", false));
    }
    Ok(())
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
    let (status, title) = match code {
        "invalid_chunk_size" | "invalid_chunk_sequence" | "invalid_chunk_prefix" => {
            (400, "Encrypted chunk request is invalid")
        }
        "plaintext_chunk_checksum_mismatch" | "encrypted_chunk_checksum_mismatch" => {
            (422, "Encrypted chunk integrity check failed")
        }
        "remote_chunk_conflict" => (409, "Remote chunk conflicts with existing content"),
        _ => (500, "Encrypted chunk transfer failed"),
    };
    Problem::new(code, title, status, "sync").retryable(retryable)
}

#[cfg(test)]
mod error_contract_tests {
    use super::chunk_problem;

    #[test]
    fn chunk_problem_uses_status_for_failure_class() {
        assert_eq!(chunk_problem("invalid_chunk_sequence", false).status, 400);
        assert_eq!(
            chunk_problem("encrypted_chunk_checksum_mismatch", false).status,
            422
        );
        assert_eq!(chunk_problem("remote_chunk_conflict", false).status, 409);
    }
}
