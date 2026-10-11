use std::sync::Mutex;

use buglogin_domain::{
    errors::Problem,
    ids::{ResourceId, WorkspaceId},
};
use buglogin_storage::{
    secrets::{OsVault, SecretError, SecretKey, SecretValue},
    vault::NativeVault,
};
use buglogin_sync::crypto::{ClientKey, RecoveryKey};
use sha2::{Digest, Sha256};

const NAMESPACE: &str = "buglogin-sync";
const KEY_BYTES: usize = 32;

pub struct ResourceKeys {
    pub client: ClientKey,
    pub recovery: RecoveryKey,
}

pub struct SyncKeyVault {
    vault: Mutex<NativeVault>,
}

impl SyncKeyVault {
    pub const fn native() -> Self {
        Self {
            vault: Mutex::new(NativeVault::new()),
        }
    }

    pub fn load_or_create(
        &self,
        workspace_id: &WorkspaceId,
        resource_id: &ResourceId,
        correlation_id: &str,
    ) -> Result<ResourceKeys, Problem> {
        let key = resource_secret_key(workspace_id, resource_id, correlation_id)?;
        let mut vault = self.vault.lock().map_err(|_| unavailable(correlation_id))?;
        match vault.read(&key) {
            Ok((value, _)) => decode_keys(value.as_bytes(), correlation_id),
            Err(SecretError::NotFound) => {
                let client = ClientKey::generate().map_err(|_| unavailable(correlation_id))?;
                let recovery = RecoveryKey::generate().map_err(|_| unavailable(correlation_id))?;
                let mut bytes = Vec::with_capacity(KEY_BYTES * 2);
                bytes.extend_from_slice(client.as_bytes());
                bytes.extend_from_slice(recovery.as_bytes());
                let value = SecretValue::new(bytes).map_err(|_| unavailable(correlation_id))?;
                match vault.create(&key, value) {
                    Ok(_) => Ok(ResourceKeys { client, recovery }),
                    Err(SecretError::VersionConflict) => {
                        let (value, _) = vault
                            .read(&key)
                            .map_err(|error| vault_problem(error, correlation_id))?;
                        decode_keys(value.as_bytes(), correlation_id)
                    }
                    Err(error) => Err(vault_problem(error, correlation_id)),
                }
            }
            Err(error) => Err(vault_problem(error, correlation_id)),
        }
    }

    pub fn load(
        &self,
        workspace_id: &WorkspaceId,
        resource_id: &ResourceId,
        correlation_id: &str,
    ) -> Result<Option<ResourceKeys>, Problem> {
        let key = resource_secret_key(workspace_id, resource_id, correlation_id)?;
        let vault = self.vault.lock().map_err(|_| unavailable(correlation_id))?;
        match vault.read(&key) {
            Ok((value, _)) => decode_keys(value.as_bytes(), correlation_id).map(Some),
            Err(SecretError::NotFound) => Ok(None),
            Err(error) => Err(vault_problem(error, correlation_id)),
        }
    }

    pub fn store_recovered(
        &self,
        workspace_id: &WorkspaceId,
        resource_id: &ResourceId,
        client: ClientKey,
        recovery: RecoveryKey,
        correlation_id: &str,
    ) -> Result<(), Problem> {
        let key = resource_secret_key(workspace_id, resource_id, correlation_id)?;
        let mut vault = self.vault.lock().map_err(|_| unavailable(correlation_id))?;
        let mut bytes = Vec::with_capacity(KEY_BYTES * 2);
        bytes.extend_from_slice(client.as_bytes());
        bytes.extend_from_slice(recovery.as_bytes());
        let value = SecretValue::new(bytes).map_err(|_| unavailable(correlation_id))?;
        match vault.create(&key, value) {
            Ok(_) => Ok(()),
            Err(SecretError::VersionConflict) => {
                let (stored, _) = vault
                    .read(&key)
                    .map_err(|error| vault_problem(error, correlation_id))?;
                let existing = decode_keys(stored.as_bytes(), correlation_id)?;
                if existing.client == client && existing.recovery.as_bytes() == recovery.as_bytes()
                {
                    Ok(())
                } else {
                    Err(Problem::new(
                        "sync_key_vault_conflict",
                        "Different profile keys are already stored on this device",
                        409,
                        correlation_id,
                    ))
                }
            }
            Err(error) => Err(vault_problem(error, correlation_id)),
        }
    }
}

fn decode_keys(bytes: &[u8], correlation_id: &str) -> Result<ResourceKeys, Problem> {
    if bytes.len() != KEY_BYTES * 2 {
        return Err(Problem::new(
            "sync_key_vault_corrupt",
            "Stored sync key material is invalid",
            500,
            correlation_id,
        ));
    }
    let mut client = [0_u8; KEY_BYTES];
    client.copy_from_slice(&bytes[..KEY_BYTES]);
    let mut recovery = [0_u8; KEY_BYTES];
    recovery.copy_from_slice(&bytes[KEY_BYTES..]);
    Ok(ResourceKeys {
        client: ClientKey::from_bytes(client),
        recovery: RecoveryKey::from_bytes(recovery),
    })
}

fn resource_secret_key(
    workspace_id: &WorkspaceId,
    resource_id: &ResourceId,
    correlation_id: &str,
) -> Result<SecretKey, Problem> {
    let scoped_id = format!("{}\0{}", workspace_id.as_str(), resource_id.as_str());
    let digest = Sha256::digest(scoped_id.as_bytes());
    SecretKey::new(NAMESPACE, format!("resource-{:x}", digest))
        .map_err(|error| vault_problem(error, correlation_id))
}

fn unavailable(correlation_id: &str) -> Problem {
    Problem::new(
        "sync_key_vault_unavailable",
        "Unable to access the operating system sync key vault",
        503,
        correlation_id,
    )
}

fn vault_problem(error: SecretError, correlation_id: &str) -> Problem {
    let status = match error {
        SecretError::InvalidKey | SecretError::EmptyValue | SecretError::TooLarge => 400,
        SecretError::NotFound => 404,
        SecretError::AccessDenied => 403,
        SecretError::VersionConflict => 409,
        SecretError::Unavailable => 503,
        SecretError::OperationFailed => 500,
    };
    Problem::new(
        error.code(),
        "Unable to access the operating system sync key vault",
        status,
        correlation_id,
    )
}
