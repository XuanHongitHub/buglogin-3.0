use std::fmt;

use aes_gcm::{
    Aes256Gcm, Nonce, Tag,
    aead::{AeadInPlace, KeyInit},
};
pub use buglogin_domain::profile::ProfileKeyShareEnvelope;
use buglogin_domain::{
    errors::Problem,
    ids::{DeviceId, ProxyId, ResourceId, WorkspaceId},
};
use hpke::{
    Deserializable, Kem, OpModeR, OpModeS, Serializable, aead::ChaCha20Poly1305, kdf::HkdfSha256,
    kem::X25519HkdfSha256,
};
use serde::{Deserialize, Serialize};

const KEY_BYTES: usize = 32;
const NONCE_BYTES: usize = 12;
const TAG_BYTES: usize = 16;
const ENCRYPTION_VERSION: u8 = 1;
const PROFILE_KEY_SHARE_VERSION: u8 = 2;
const PROFILE_KEY_SHARE_INFO: &[u8] = b"buglogin/profile-key-share/v2";
pub const AUTHENTICATED_CHUNK_OVERHEAD_BYTES: usize = 1 + NONCE_BYTES + TAG_BYTES;
pub const AUTHENTICATED_CHUNK_VERSION: u8 = ENCRYPTION_VERSION;

type ProfileKeyShareKem = X25519HkdfSha256;
type ProfileKeyShareAead = ChaCha20Poly1305;
type ProfileKeyShareKdf = HkdfSha256;

/// Device-held private key for receiving profile resource keys. The raw key is
/// kept by the caller in the platform secret vault and is never serializable.
pub struct DevicePrivateKey([u8; 32]);

impl DevicePrivateKey {
    pub fn generate() -> Result<Self, Problem> {
        let mut raw = [0_u8; 32];
        if getrandom::fill(&mut raw).is_err() {
            raw.fill(0);
            return Err(crypto_problem("device_key_generation_failed"));
        }
        Ok(Self(raw))
    }

    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn public_key(&self) -> Result<DevicePublicKey, Problem> {
        let private_key = <ProfileKeyShareKem as Kem>::PrivateKey::from_bytes(&self.0)
            .map_err(|_| crypto_problem("invalid_device_private_key"))?;
        let public_key = ProfileKeyShareKem::sk_to_pk(&private_key).to_bytes();
        let mut bytes = [0_u8; 32];
        bytes.copy_from_slice(public_key.as_slice());
        Ok(DevicePublicKey(bytes))
    }
}

impl fmt::Debug for DevicePrivateKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DevicePrivateKey(<redacted>)")
    }
}

impl Drop for DevicePrivateKey {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}

/// Public HPKE key for a device that is eligible to receive profile access.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DevicePublicKey([u8; 32]);

impl DevicePublicKey {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// Keys needed by an authorized recipient to read and continue syncing a
/// shared encrypted profile. Debug output deliberately omits both secrets.
pub struct SharedProfileKeys {
    pub client: ClientKey,
    pub recovery: RecoveryKey,
}

impl fmt::Debug for SharedProfileKeys {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SharedProfileKeys(<redacted>)")
    }
}

/// Client-generated HPKE envelope that gives one recipient device the keys
/// needed to read and continue syncing a profile without exposing them to the
/// API or object store.
pub fn seal_profile_key_share(
    workspace_id: WorkspaceId,
    resource_id: ResourceId,
    recipient_device_id: String,
    key_version: String,
    recipient_public_key: &DevicePublicKey,
    client_key: &ClientKey,
    recovery_key: &RecoveryKey,
) -> Result<ProfileKeyShareEnvelope, Problem> {
    validate_share_scope(&recipient_device_id, &key_version)?;
    let public_key =
        <ProfileKeyShareKem as Kem>::PublicKey::from_bytes(recipient_public_key.as_bytes())
            .map_err(|_| crypto_problem("invalid_device_public_key"))?;
    let aad = profile_key_share_aad(
        &workspace_id,
        &resource_id,
        &recipient_device_id,
        &key_version,
    );
    let mut plaintext = [0_u8; KEY_BYTES * 2];
    plaintext[..KEY_BYTES].copy_from_slice(client_key.as_bytes());
    plaintext[KEY_BYTES..].copy_from_slice(recovery_key.as_bytes());
    let seal_result =
        hpke::single_shot_seal::<ProfileKeyShareAead, ProfileKeyShareKdf, ProfileKeyShareKem>(
            &OpModeS::Base,
            &public_key,
            PROFILE_KEY_SHARE_INFO,
            &plaintext,
            &aad,
        );
    plaintext.fill(0);
    let (encapped_key, ciphertext) =
        seal_result.map_err(|_| crypto_problem("profile_key_share_seal_failed"))?;
    Ok(ProfileKeyShareEnvelope {
        schema_version: PROFILE_KEY_SHARE_VERSION,
        workspace_id,
        resource_id,
        recipient_device_id,
        key_version,
        encapped_key: encapped_key.to_bytes().as_slice().to_vec(),
        ciphertext,
    })
}

pub fn open_profile_key_share(
    envelope: &ProfileKeyShareEnvelope,
    expected_workspace_id: &WorkspaceId,
    expected_resource_id: &ResourceId,
    expected_device_id: &str,
    expected_key_version: &str,
    private_key: &DevicePrivateKey,
) -> Result<SharedProfileKeys, Problem> {
    if envelope.schema_version != PROFILE_KEY_SHARE_VERSION
        || &envelope.workspace_id != expected_workspace_id
        || &envelope.resource_id != expected_resource_id
        || envelope.recipient_device_id != expected_device_id
        || envelope.key_version != expected_key_version
    {
        return Err(crypto_problem("profile_key_share_scope_mismatch"));
    }
    validate_share_scope(&envelope.recipient_device_id, &envelope.key_version)?;
    let private_key = <ProfileKeyShareKem as Kem>::PrivateKey::from_bytes(private_key.as_bytes())
        .map_err(|_| crypto_problem("invalid_device_private_key"))?;
    let encapped_key = <ProfileKeyShareKem as Kem>::EncappedKey::from_bytes(&envelope.encapped_key)
        .map_err(|_| crypto_problem("invalid_profile_key_share_envelope"))?;
    let aad = profile_key_share_aad(
        expected_workspace_id,
        expected_resource_id,
        expected_device_id,
        expected_key_version,
    );
    let mut plaintext =
        hpke::single_shot_open::<ProfileKeyShareAead, ProfileKeyShareKdf, ProfileKeyShareKem>(
            &OpModeR::Base,
            &private_key,
            &encapped_key,
            PROFILE_KEY_SHARE_INFO,
            &envelope.ciphertext,
            &aad,
        )
        .map_err(|_| crypto_problem("profile_key_share_open_failed"))?;
    if plaintext.len() != KEY_BYTES * 2 {
        plaintext.fill(0);
        return Err(crypto_problem("invalid_profile_key_share_envelope"));
    }
    let mut client = [0_u8; KEY_BYTES];
    let mut recovery = [0_u8; KEY_BYTES];
    client.copy_from_slice(&plaintext[..KEY_BYTES]);
    recovery.copy_from_slice(&plaintext[KEY_BYTES..]);
    plaintext.fill(0);
    let client_key = ClientKey::from_bytes(client);
    client.fill(0);
    let recovery_key = RecoveryKey::from_bytes(recovery);
    recovery.fill(0);
    Ok(SharedProfileKeys {
        client: client_key,
        recovery: recovery_key,
    })
}

fn validate_share_scope(device_id: &str, key_version: &str) -> Result<(), Problem> {
    if DeviceId::new(device_id).is_err()
        || key_version.len() != 64
        || !key_version.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(crypto_problem("invalid_profile_key_share_scope"));
    }
    Ok(())
}

fn profile_key_share_aad(
    workspace_id: &WorkspaceId,
    resource_id: &ResourceId,
    recipient_device_id: &str,
    key_version: &str,
) -> Vec<u8> {
    format!(
        "buglogin/profile-key-share/v2/{}/{}/{}/{}",
        workspace_id.as_str(),
        resource_id.as_str(),
        recipient_device_id,
        key_version
    )
    .into_bytes()
}

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

/// A user-held secret used to recover one resource key on another device.
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

    pub fn as_bytes(&self) -> &[u8; KEY_BYTES] {
        &self.0
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

/// Server-storable envelope for one resource key, encrypted with a client-held
/// recovery key and authenticated against workspace and resource.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct ResourceKeyEnvelope {
    pub schema_version: u8,
    pub workspace_id: WorkspaceId,
    pub resource_id: ResourceId,
    pub encrypted_key: AuthenticatedChunk,
}

impl ResourceKeyEnvelope {
    pub fn wrap(
        workspace_id: WorkspaceId,
        resource_id: ResourceId,
        client_key: &ClientKey,
        recovery_key: &RecoveryKey,
    ) -> Result<Self, Problem> {
        let recovery_key = ClientKey::from_bytes(recovery_key.0);
        let encrypted_key = AuthenticatedChunk::encrypt(
            &recovery_key,
            client_key.as_bytes(),
            &resource_key_aad(&workspace_id, &resource_id),
        )?;
        Ok(Self {
            schema_version: 1,
            workspace_id,
            resource_id,
            encrypted_key,
        })
    }

    pub fn unwrap(
        &self,
        expected_workspace_id: &WorkspaceId,
        expected_resource_id: &ResourceId,
        recovery_key: &RecoveryKey,
    ) -> Result<ClientKey, Problem> {
        if self.schema_version != 1
            || &self.workspace_id != expected_workspace_id
            || &self.resource_id != expected_resource_id
        {
            return Err(crypto_problem("resource_key_envelope_scope_mismatch"));
        }
        let recovery_key = ClientKey::from_bytes(recovery_key.0);
        let mut bytes = self.encrypted_key.decrypt(
            &recovery_key,
            &resource_key_aad(expected_workspace_id, expected_resource_id),
        )?;
        if bytes.len() != KEY_BYTES {
            bytes.fill(0);
            return Err(crypto_problem("invalid_resource_key_envelope"));
        }
        let mut key = [0_u8; KEY_BYTES];
        key.copy_from_slice(&bytes);
        bytes.fill(0);
        Ok(ClientKey::from_bytes(key))
    }
}

/// Encrypted proxy credentials scoped to the profile resource that references
/// them. The server may store this envelope as an opaque manifest object, but
/// cannot move it across workspaces, profile resources, proxies, or revisions.
#[derive(Clone, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProxyCredentialEnvelope {
    pub schema_version: u8,
    pub workspace_id: WorkspaceId,
    pub resource_id: ResourceId,
    pub proxy_id: ProxyId,
    pub proxy_revision: u64,
    pub encrypted_secret: AuthenticatedChunk,
}

impl ProxyCredentialEnvelope {
    pub fn seal(
        workspace_id: WorkspaceId,
        resource_id: ResourceId,
        proxy_id: ProxyId,
        proxy_revision: u64,
        client_key: &ClientKey,
        secret: &[u8],
    ) -> Result<Self, Problem> {
        if proxy_revision == 0 || secret.is_empty() {
            return Err(crypto_problem("invalid_proxy_credential_envelope"));
        }
        let encrypted_secret = AuthenticatedChunk::encrypt(
            client_key,
            secret,
            &proxy_credential_aad(&workspace_id, &resource_id, &proxy_id, proxy_revision),
        )?;
        Ok(Self {
            schema_version: 1,
            workspace_id,
            resource_id,
            proxy_id,
            proxy_revision,
            encrypted_secret,
        })
    }

    pub fn open(
        &self,
        workspace_id: &WorkspaceId,
        resource_id: &ResourceId,
        proxy_id: &ProxyId,
        proxy_revision: u64,
        client_key: &ClientKey,
    ) -> Result<Vec<u8>, Problem> {
        if self.schema_version != 1
            || &self.workspace_id != workspace_id
            || &self.resource_id != resource_id
            || &self.proxy_id != proxy_id
            || self.proxy_revision != proxy_revision
            || proxy_revision == 0
        {
            return Err(crypto_problem("proxy_credential_envelope_scope_mismatch"));
        }
        self.encrypted_secret.decrypt(
            client_key,
            &proxy_credential_aad(workspace_id, resource_id, proxy_id, proxy_revision),
        )
    }
}

fn proxy_credential_aad(
    workspace_id: &WorkspaceId,
    resource_id: &ResourceId,
    proxy_id: &ProxyId,
    proxy_revision: u64,
) -> Vec<u8> {
    format!(
        "buglogin/proxy-credential/v1/{}/{}/{}/{}",
        workspace_id.as_str(),
        resource_id.as_str(),
        proxy_id.as_str(),
        proxy_revision
    )
    .into_bytes()
}

fn resource_key_aad(workspace_id: &WorkspaceId, resource_id: &ResourceId) -> Vec<u8> {
    format!(
        "buglogin/resource-key/v1/{}/{}",
        workspace_id.as_str(),
        resource_id.as_str()
    )
    .into_bytes()
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
        let mut bytes =
            Vec::with_capacity(AUTHENTICATED_CHUNK_OVERHEAD_BYTES + self.ciphertext.len());
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
    let (status, title, retryable) =
        if code.ends_with("_generation_failed") || code == "nonce_generation_failed" {
            (503, "Cryptographic key generation is unavailable", true)
        } else if code.ends_with("_scope_mismatch") {
            (
                409,
                "Encrypted data belongs to a different resource scope",
                false,
            )
        } else if code.ends_with("_authentication_failed") || code.ends_with("_open_failed") {
            (422, "Encrypted data could not be authenticated", false)
        } else if code.starts_with("invalid_") || code.starts_with("unsupported_") {
            (400, "Encrypted sync input is invalid", false)
        } else {
            (500, "Cryptographic operation failed", false)
        };
    Problem::new(code, title, status, "sync-crypto").retryable(retryable)
}

#[cfg(test)]
mod error_contract_tests {
    use super::crypto_problem;

    #[test]
    fn crypto_problem_uses_status_for_failure_class() {
        let invalid = crypto_problem("invalid_encrypted_chunk");
        assert_eq!((invalid.status, invalid.retryable), (400, false));

        let scope = crypto_problem("resource_key_envelope_scope_mismatch");
        assert_eq!((scope.status, scope.retryable), (409, false));

        let authentication = crypto_problem("chunk_authentication_failed");
        assert_eq!(
            (authentication.status, authentication.retryable),
            (422, false)
        );

        let unavailable = crypto_problem("key_generation_failed");
        assert_eq!((unavailable.status, unavailable.retryable), (503, true));
    }
}
