use std::fmt;

use aes_gcm::{
    Aes256Gcm, Nonce, Tag,
    aead::{AeadInPlace, KeyInit},
};
use buglogin_domain::errors::Problem;

const KEY_BYTES: usize = 32;
const NONCE_BYTES: usize = 12;
const TAG_BYTES: usize = 16;
const ENCRYPTION_VERSION: u8 = 1;

#[derive(Clone, Eq, PartialEq)]
pub struct ClientKey([u8; KEY_BYTES]);

impl ClientKey {
    pub fn from_bytes(bytes: [u8; KEY_BYTES]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; KEY_BYTES] {
        &self.0
    }
}

impl fmt::Debug for ClientKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ClientKey(<redacted>)")
    }
}

impl Drop for ClientKey {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthenticatedChunk {
    pub version: u8,
    pub nonce: [u8; NONCE_BYTES],
    pub ciphertext: Vec<u8>,
    pub tag: [u8; TAG_BYTES],
}

impl AuthenticatedChunk {
    pub fn encrypt(
        key: &ClientKey,
        plaintext: &[u8],
        associated_data: &[u8],
    ) -> Result<Self, Problem> {
        let cipher = Aes256Gcm::new_from_slice(key.as_bytes())
            .map_err(|_| crypto_problem("invalid_encryption_key"))?;
        let mut nonce = [0_u8; NONCE_BYTES];
        getrandom::fill(&mut nonce).map_err(|_| crypto_problem("nonce_generation_failed"))?;
        let mut ciphertext = plaintext.to_vec();
        let tag = cipher
            .encrypt_in_place_detached(Nonce::from_slice(&nonce), associated_data, &mut ciphertext)
            .map_err(|_| crypto_problem("chunk_encryption_failed"))?;
        Ok(Self {
            version: ENCRYPTION_VERSION,
            nonce,
            ciphertext,
            tag: tag.into(),
        })
    }

    pub fn decrypt(&self, key: &ClientKey, associated_data: &[u8]) -> Result<Vec<u8>, Problem> {
        if self.version != ENCRYPTION_VERSION {
            return Err(crypto_problem("unsupported_chunk_encryption"));
        }
        let cipher = Aes256Gcm::new_from_slice(key.as_bytes())
            .map_err(|_| crypto_problem("invalid_encryption_key"))?;
        let mut plaintext = self.ciphertext.clone();
        cipher
            .decrypt_in_place_detached(
                Nonce::from_slice(&self.nonce),
                associated_data,
                &mut plaintext,
                Tag::from_slice(&self.tag),
            )
            .map_err(|_| crypto_problem("chunk_authentication_failed"))?;
        Ok(plaintext)
    }

    pub fn encoded(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(1 + NONCE_BYTES + TAG_BYTES + self.ciphertext.len());
        bytes.push(self.version);
        bytes.extend_from_slice(&self.nonce);
        bytes.extend_from_slice(&self.tag);
        bytes.extend_from_slice(&self.ciphertext);
        bytes
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Problem> {
        let header = 1 + NONCE_BYTES + TAG_BYTES;
        if bytes.len() < header {
            return Err(crypto_problem("invalid_encrypted_chunk"));
        }
        let mut nonce = [0_u8; NONCE_BYTES];
        nonce.copy_from_slice(&bytes[1..1 + NONCE_BYTES]);
        let mut tag = [0_u8; TAG_BYTES];
        tag.copy_from_slice(&bytes[1 + NONCE_BYTES..header]);
        Ok(Self {
            version: bytes[0],
            nonce,
            tag,
            ciphertext: bytes[header..].to_vec(),
        })
    }
}

fn crypto_problem(code: &'static str) -> Problem {
    Problem::new(code, "Encrypted chunk operation failed", 409, "sync-crypto")
}
