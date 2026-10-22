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

    pub fn stage_rotation(
        &self,
        workspace_id: &WorkspaceId,
        resource_id: &ResourceId,
        correlation_id: &str,
    ) -> Result<ResourceKeys, Problem> {
        let key = rotation_secret_key(workspace_id, resource_id, correlation_id)?;
        let mut vault = self.vault.lock().map_err(|_| unavailable(correlation_id))?;
        match vault.read(&key) {
            Ok((value, _)) => decode_keys(value.as_bytes(), correlation_id),
            Err(SecretError::NotFound) => {
                let keys = ResourceKeys {
                    client: ClientKey::generate().map_err(|_| unavailable(correlation_id))?,
                    recovery: RecoveryKey::generate().map_err(|_| unavailable(correlation_id))?,
                };
                let value = encode_keys(&keys, correlation_id)?;
                match vault.create(&key, value) {
                    Ok(_) => Ok(keys),
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

    pub fn activate_staged_rotation(
        &self,
        workspace_id: &WorkspaceId,
        resource_id: &ResourceId,
        correlation_id: &str,
    ) -> Result<ResourceKeys, Problem> {
        let active_key = resource_secret_key(workspace_id, resource_id, correlation_id)?;
        let pending_key = rotation_secret_key(workspace_id, resource_id, correlation_id)?;
        let mut vault = self.vault.lock().map_err(|_| unavailable(correlation_id))?;
        let (pending_value, pending_version) = vault
            .read(&pending_key)
            .map_err(|error| vault_problem(error, correlation_id))?;
        let pending = decode_keys(pending_value.as_bytes(), correlation_id)?;
        let replacement = encode_keys(&pending, correlation_id)?;
        match vault.read(&active_key) {
            Ok((active_value, active_version)) => {
                let active = decode_keys(active_value.as_bytes(), correlation_id)?;
                if active.client != pending.client
                    || active.recovery.as_bytes() != pending.recovery.as_bytes()
                {
                    vault
                        .rotate(&active_key, active_version, replacement)
                        .map_err(|error| vault_problem(error, correlation_id))?;
                }
            }
            Err(SecretError::NotFound) => {
                vault
                    .create(&active_key, replacement)
                    .map_err(|error| vault_problem(error, correlation_id))?;
            }
            Err(error) => return Err(vault_problem(error, correlation_id)),
        }
        vault
            .remove(&pending_key, pending_version)
            .map_err(|error| vault_problem(error, correlation_id))?;
        Ok(pending)
    }

    pub fn load_for_manifest(
        &self,
        workspace_id: &WorkspaceId,
        resource_id: &ResourceId,
        expected_key_version: &str,
        correlation_id: &str,
    ) -> Result<Option<ResourceKeys>, Problem> {
        let active_key = resource_secret_key(workspace_id, resource_id, correlation_id)?;
        let pending_key = rotation_secret_key(workspace_id, resource_id, correlation_id)?;
        let mut vault = self.vault.lock().map_err(|_| unavailable(correlation_id))?;
        let active = match vault.read(&active_key) {
            Ok((value, version)) => Some((decode_keys(value.as_bytes(), correlation_id)?, version)),
            Err(SecretError::NotFound) => None,
            Err(error) => return Err(vault_problem(error, correlation_id)),
        };
        let pending = match vault.read(&pending_key) {
            Ok((value, version)) => Some((decode_keys(value.as_bytes(), correlation_id)?, version)),
            Err(SecretError::NotFound) => None,
            Err(error) => return Err(vault_problem(error, correlation_id)),
        };
        if let Some((pending, pending_version)) = pending {
            if key_version(&pending.client) == expected_key_version {
                let replacement = encode_keys(&pending, correlation_id)?;
                match active {
                    Some((active, active_version)) => {
                        if active.client != pending.client
                            || active.recovery.as_bytes() != pending.recovery.as_bytes()
                        {
                            vault
                                .rotate(&active_key, active_version, replacement)
                                .map_err(|error| vault_problem(error, correlation_id))?;
                        }
                    }
                    None => {
                        vault
                            .create(&active_key, replacement)
                            .map_err(|error| vault_problem(error, correlation_id))?;
                    }
                }
                vault
                    .remove(&pending_key, pending_version)
                    .map_err(|error| vault_problem(error, correlation_id))?;
                return Ok(Some(pending));
            }
        }
        if let Some((active, _)) = active {
            if key_version(&active.client) == expected_key_version {
                return Ok(Some(active));
            }
            return Err(Problem::new(
                "profile_key_version_stale",
                "Stored profile keys do not match the active sync manifest",
                409,
                correlation_id,
            ));
        }
        Ok(None)
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

    pub fn replace_recovered_for_manifest(
        &self,
        workspace_id: &WorkspaceId,
        resource_id: &ResourceId,
        client: ClientKey,
        recovery: RecoveryKey,
        expected_key_version: &str,
        correlation_id: &str,
    ) -> Result<(), Problem> {
        if key_version(&client) != expected_key_version {
            return Err(Problem::new(
                "profile_key_version_mismatch",
                "Recovered profile keys do not match the active sync manifest",
                409,
                correlation_id,
            ));
        }
        let key = resource_secret_key(workspace_id, resource_id, correlation_id)?;
        let mut vault = self.vault.lock().map_err(|_| unavailable(correlation_id))?;
        let replacement = encode_keys(&ResourceKeys { client, recovery }, correlation_id)?;
        match vault.read(&key) {
            Ok((stored, version)) => {
                let existing = decode_keys(stored.as_bytes(), correlation_id)?;
                if key_version(&existing.client) == expected_key_version {
                    if existing.client.as_bytes() == &replacement.as_bytes()[..KEY_BYTES]
                        && existing.recovery.as_bytes() == &replacement.as_bytes()[KEY_BYTES..]
                    {
                        return Ok(());
                    }
                    return Err(Problem::new(
                        "sync_key_vault_conflict",
                        "Different profile keys are already stored for this manifest version",
                        409,
                        correlation_id,
                    ));
                }
                vault
                    .rotate(&key, version, replacement)
                    .map(|_| ())
                    .map_err(|error| vault_problem(error, correlation_id))
            }
            Err(SecretError::NotFound) => vault
                .create(&key, replacement)
                .map(|_| ())
                .map_err(|error| vault_problem(error, correlation_id)),
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

fn encode_keys(keys: &ResourceKeys, correlation_id: &str) -> Result<SecretValue, Problem> {
    let mut bytes = Vec::with_capacity(KEY_BYTES * 2);
    bytes.extend_from_slice(keys.client.as_bytes());
    bytes.extend_from_slice(keys.recovery.as_bytes());
    SecretValue::new(bytes).map_err(|_| unavailable(correlation_id))
}

fn key_version(client: &ClientKey) -> String {
    format!("{:x}", Sha256::digest(client.as_bytes()))
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

fn rotation_secret_key(
    workspace_id: &WorkspaceId,
    resource_id: &ResourceId,
    correlation_id: &str,
) -> Result<SecretKey, Problem> {
    let scoped_id = format!("{}\0{}", workspace_id.as_str(), resource_id.as_str());
    let digest = Sha256::digest(scoped_id.as_bytes());
    SecretKey::new(NAMESPACE, format!("resource-{:x}-rotation", digest))
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
