use std::fmt;

use aes_gcm::{
    Aes256Gcm, Nonce, Tag,
    aead::{AeadInPlace, KeyInit},
};
use buglogin_domain::{errors::Problem, ids::WorkspaceId};
use serde::{Deserialize, Serialize};

const KEY_BYTES: usize = 32;
const NONCE_BYTES: usize = 12;
const TAG_BYTES: usize = 16;
const ENCRYPTION_VERSION: u8 = 1;
pub const AUTHENTICATED_CHUNK_VERSION: u8 = ENCRYPTION_VERSION;

#[derive(Clone, Eq, PartialEq)]
pub struct ClientKey([u8; KEY_BYTES]);

impl ClientKey {
    pub fn generate() -> Result<Self, Problem> {
        let mut bytes = [0_u8; KEY_BYTES];
        if getrandom::fill(&mut bytes).is_err() {
            bytes.fill(0);
            return Err(crypto_problem("key_generation_failed"));
        }
        Ok(Self(bytes))
    }

    pub fn from_bytes(bytes: [u8; KEY_BYTES]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; KEY_BYTES] {
        &self.0
    }
}

/// A user-held secret used to recover a workspace key on another device.
/// The caller must display the recovery code once and must not persist or log it.
pub struct RecoveryKey([u8; KEY_BYTES]);

impl RecoveryKey {
    pub fn generate() -> Result<Self, Problem> {
        let mut bytes = [0_u8; KEY_BYTES];
        if getrandom::fill(&mut bytes).is_err() {
            bytes.fill(0);
            return Err(crypto_problem("key_generation_failed"));
        }
        Ok(Self(bytes))
    }

    pub fn from_bytes(bytes: [u8; KEY_BYTES]) -> Self {
        Self(bytes)
    }

    pub fn parse_code(code: &str) -> Result<Self, Problem> {
        if code.len() != KEY_BYTES * 2 {
            return Err(crypto_problem("invalid_recovery_code"));
        }
        let mut key = Self([0_u8; KEY_BYTES]);
        let encoded = code.as_bytes();
        for (index, byte) in key.0.iter_mut().enumerate() {
            let start = index * 2;
            let Some(high) = hex_digit(encoded[start]) else {
                return Err(crypto_problem("invalid_recovery_code"));
            };
            let Some(low) = hex_digit(encoded[start + 1]) else {
                return Err(crypto_problem("invalid_recovery_code"));
            };
            *byte = (high << 4) | low;
        }
        Ok(key)
    }

    pub fn recovery_code(&self) -> String {
        let mut code = String::with_capacity(KEY_BYTES * 2);
        for byte in &self.0 {
            use fmt::Write as _;
            let _ = write!(code, "{byte:02x}");
        }
        code
    }
}

impl fmt::Debug for RecoveryKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RecoveryKey(<redacted>)")
    }
}

impl Drop for RecoveryKey {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}

/// Server-storable envelope for a workspace key, encrypted with a client-held
/// recovery key and authenticated against the workspace identity.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct WorkspaceKeyEnvelope {
    pub schema_version: u8,
    pub workspace_id: WorkspaceId,
    pub encrypted_key: AuthenticatedChunk,
}

impl WorkspaceKeyEnvelope {
    pub fn wrap(
        workspace_id: WorkspaceId,
        client_key: &ClientKey,
        recovery_key: &RecoveryKey,
    ) -> Result<Self, Problem> {
        let recovery_key = ClientKey::from_bytes(recovery_key.0);
        let encrypted_key = AuthenticatedChunk::encrypt(
            &recovery_key,
            client_key.as_bytes(),
            &workspace_key_aad(&workspace_id),
        )?;
        Ok(Self {
            schema_version: 1,
            workspace_id,
            encrypted_key,
        })
    }

    pub fn unwrap(
        &self,
        expected_workspace_id: &WorkspaceId,
        recovery_key: &RecoveryKey,
    ) -> Result<ClientKey, Problem> {
        if self.schema_version != 1 || &self.workspace_id != expected_workspace_id {
            return Err(crypto_problem("workspace_key_envelope_scope_mismatch"));
        }
        let recovery_key = ClientKey::from_bytes(recovery_key.0);
        let mut bytes = self
            .encrypted_key
            .decrypt(&recovery_key, &workspace_key_aad(expected_workspace_id))?;
        if bytes.len() != KEY_BYTES {
            bytes.fill(0);
            return Err(crypto_problem("invalid_workspace_key_envelope"));
        }
        let mut key = [0_u8; KEY_BYTES];
        key.copy_from_slice(&bytes);
        bytes.fill(0);
        Ok(ClientKey::from_bytes(key))
    }
}

fn workspace_key_aad(workspace_id: &WorkspaceId) -> Vec<u8> {
    format!("buglogin/workspace-key/v1/{}", workspace_id.as_str()).into_bytes()
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
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

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
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
