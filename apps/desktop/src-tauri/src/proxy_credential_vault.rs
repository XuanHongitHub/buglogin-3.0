use std::{
    fs::{self, File, OpenOptions},
    sync::Mutex,
};

use buglogin_domain::{
    errors::Problem,
    ids::{ProxyId, WorkspaceId},
};
use buglogin_storage::{
    secrets::{OsVault, SecretError, SecretKey, SecretValue},
    vault::NativeVault,
};
use fs2::FileExt;
use sha2::{Digest, Sha256};

const NAMESPACE: &str = "buglogin-proxy";
const PENDING_NAMESPACE: &str = "buglogin-proxy-mutation";

pub struct ProxyCredentialVault {
    vault: Mutex<Box<dyn OsVault + Send>>,
}

impl ProxyCredentialVault {
    pub fn native() -> Self {
        Self::with_vault(Box::new(NativeVault::new()))
    }

    pub fn with_vault(vault: Box<dyn OsVault + Send>) -> Self {
        Self {
            vault: Mutex::new(vault),
        }
    }

    pub fn load(
        &self,
        workspace_id: &WorkspaceId,
        proxy_id: &ProxyId,
        revision: u64,
        correlation_id: &str,
    ) -> Result<Option<SecretValue>, Problem> {
        let key = credential_key(workspace_id, proxy_id, revision, correlation_id)?;
        match self
            .vault
            .lock()
            .map_err(|_| unavailable(correlation_id))?
            .read(&key)
        {
            Ok((value, _)) => Ok(Some(value)),
            Err(SecretError::NotFound) => Ok(None),
            Err(error) => Err(vault_problem(error, correlation_id)),
        }
    }

    pub fn store(
        &self,
        workspace_id: &WorkspaceId,
        proxy_id: &ProxyId,
        revision: u64,
        value: SecretValue,
        correlation_id: &str,
    ) -> Result<(), Problem> {
        let key = credential_key(workspace_id, proxy_id, revision, correlation_id)?;
        let mut vault = self.vault.lock().map_err(|_| unavailable(correlation_id))?;
        match vault.read(&key) {
            Ok((_, version)) => vault
                .rotate(&key, version, value)
                .map(|_| ())
                .map_err(|error| vault_problem(error, correlation_id)),
            Err(SecretError::NotFound) => match vault.create(&key, value.clone()) {
                Ok(_) => Ok(()),
                Err(SecretError::VersionConflict) => {
                    let (_, version) = vault
                        .read(&key)
                        .map_err(|error| vault_problem(error, correlation_id))?;
                    vault
                        .rotate(&key, version, value)
                        .map(|_| ())
                        .map_err(|error| vault_problem(error, correlation_id))
                }
                Err(error) => Err(vault_problem(error, correlation_id)),
            },
            Err(error) => Err(vault_problem(error, correlation_id)),
        }
    }

    pub fn remove(
        &self,
        workspace_id: &WorkspaceId,
        proxy_id: &ProxyId,
        revision: u64,
        correlation_id: &str,
    ) -> Result<(), Problem> {
        let key = credential_key(workspace_id, proxy_id, revision, correlation_id)?;
        let mut vault = self.vault.lock().map_err(|_| unavailable(correlation_id))?;
        match vault.read(&key) {
            Ok((_, version)) => vault
                .remove(&key, version)
                .map_err(|error| vault_problem(error, correlation_id)),
            Err(SecretError::NotFound) => Ok(()),
            Err(error) => Err(vault_problem(error, correlation_id)),
        }
    }

    pub fn load_pending(
        &self,
        workspace_id: &WorkspaceId,
        proxy_id: &ProxyId,
        correlation_id: &str,
    ) -> Result<Option<SecretValue>, Problem> {
        let key = pending_key(workspace_id, proxy_id, correlation_id)?;
        match self
            .vault
            .lock()
            .map_err(|_| unavailable(correlation_id))?
            .read(&key)
        {
            Ok((value, _)) => Ok(Some(value)),
            Err(SecretError::NotFound) => Ok(None),
            Err(error) => Err(vault_problem(error, correlation_id)),
        }
    }

    pub fn store_pending(
        &self,
        workspace_id: &WorkspaceId,
        proxy_id: &ProxyId,
        bytes: Vec<u8>,
        correlation_id: &str,
    ) -> Result<(), Problem> {
        let key = pending_key(workspace_id, proxy_id, correlation_id)?;
        let value =
            SecretValue::new(bytes).map_err(|error| vault_problem(error, correlation_id))?;
        let mut vault = self.vault.lock().map_err(|_| unavailable(correlation_id))?;
        match vault.read(&key) {
            Ok(_) => Err(Problem::new(
                "proxy_mutation_pending",
                "Another proxy change must be recovered before this change can be saved",
                409,
                correlation_id,
            )),
            Err(SecretError::NotFound) => vault.create(&key, value).map(|_| ()).map_err(|error| {
                match error {
                    SecretError::VersionConflict => Problem::new(
                        "proxy_mutation_pending",
                        "Another proxy change must be recovered before this change can be saved",
                        409,
                        correlation_id,
                    ),
                    error => vault_problem(error, correlation_id),
                }
            }),
            Err(error) => Err(vault_problem(error, correlation_id)),
        }
    }

    pub fn remove_pending(
        &self,
        workspace_id: &WorkspaceId,
        proxy_id: &ProxyId,
        correlation_id: &str,
    ) -> Result<(), Problem> {
        let key = pending_key(workspace_id, proxy_id, correlation_id)?;
        let mut vault = self.vault.lock().map_err(|_| unavailable(correlation_id))?;
        match vault.read(&key) {
            Ok((_, version)) => vault
                .remove(&key, version)
                .map_err(|error| vault_problem(error, correlation_id)),
            Err(SecretError::NotFound) => Ok(()),
            Err(error) => Err(vault_problem(error, correlation_id)),
        }
    }

    pub fn acquire_mutation_lock(
        &self,
        workspace_id: &WorkspaceId,
        proxy_id: &ProxyId,
        correlation_id: &str,
    ) -> Result<File, Problem> {
        let root = std::env::temp_dir().join("buglogin-proxy-mutation-locks");
        fs::create_dir_all(&root).map_err(|_| unavailable(correlation_id))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700))
                .map_err(|_| unavailable(correlation_id))?;
        }
        let scoped_id = format!("{}\0{}", workspace_id.as_str(), proxy_id.as_str());
        let digest = Sha256::digest(scoped_id.as_bytes());
        let lock_path = root.join(format!("proxy-{:x}.lock", digest));
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path)
            .map_err(|_| unavailable(correlation_id))?;
        file.try_lock_exclusive().map_err(|error| {
            if error.kind() == std::io::ErrorKind::WouldBlock {
                Problem::new(
                    "proxy_mutation_in_progress",
                    "Another BugLogin process is saving or recovering this proxy",
                    409,
                    correlation_id,
                )
                .retryable(true)
            } else {
                unavailable(correlation_id)
            }
        })?;
        Ok(file)
    }
}

fn pending_key(
    workspace_id: &WorkspaceId,
    proxy_id: &ProxyId,
    correlation_id: &str,
) -> Result<SecretKey, Problem> {
    let scoped_id = format!("{}\0{}", workspace_id.as_str(), proxy_id.as_str());
    let digest = Sha256::digest(scoped_id.as_bytes());
    SecretKey::new(PENDING_NAMESPACE, format!("proxy-mutation-{:x}", digest))
        .map_err(|error| vault_problem(error, correlation_id))
}

fn credential_key(
    workspace_id: &WorkspaceId,
    proxy_id: &ProxyId,
    revision: u64,
    correlation_id: &str,
) -> Result<SecretKey, Problem> {
    let scoped_id = format!(
        "{}\0{}\0{}",
        workspace_id.as_str(),
        proxy_id.as_str(),
        revision
    );
    let digest = Sha256::digest(scoped_id.as_bytes());
    SecretKey::new(NAMESPACE, format!("proxy-{:x}", digest))
        .map_err(|error| vault_problem(error, correlation_id))
}

fn unavailable(correlation_id: &str) -> Problem {
    Problem::new(
        "proxy_credential_vault_unavailable",
        "Unable to access the operating system proxy credential vault",
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
        "Unable to access the operating system proxy credential vault",
        status,
        correlation_id,
    )
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use buglogin_storage::secrets::{RotationReceipt, SecretVersion};

    #[derive(Default)]
    struct MemoryVault(BTreeMap<(String, String), (SecretValue, SecretVersion)>);

    impl OsVault for MemoryVault {
        fn read(&self, key: &SecretKey) -> Result<(SecretValue, SecretVersion), SecretError> {
            self.0
                .get(&(key.namespace().to_owned(), key.name().to_owned()))
                .cloned()
                .ok_or(SecretError::NotFound)
        }

        fn create(
            &mut self,
            key: &SecretKey,
            value: SecretValue,
        ) -> Result<SecretVersion, SecretError> {
            let map_key = (key.namespace().to_owned(), key.name().to_owned());
            if self.0.contains_key(&map_key) {
                return Err(SecretError::VersionConflict);
            }
            let version = SecretVersion::new(1).unwrap();
            self.0.insert(map_key, (value, version));
            Ok(version)
        }

        fn rotate(
            &mut self,
            key: &SecretKey,
            expected: SecretVersion,
            replacement: SecretValue,
        ) -> Result<RotationReceipt, SecretError> {
            let map_key = (key.namespace().to_owned(), key.name().to_owned());
            let current = self.0.get(&map_key).map(|(_, version)| *version);
            if current != Some(expected) {
                return Err(SecretError::VersionConflict);
            }
            let next = SecretVersion::new(expected.generation() + 1).unwrap();
            self.0.insert(map_key, (replacement, next));
            Ok(RotationReceipt {
                previous: expected,
                current: next,
            })
        }

        fn remove(&mut self, key: &SecretKey, expected: SecretVersion) -> Result<(), SecretError> {
            let map_key = (key.namespace().to_owned(), key.name().to_owned());
            if self.0.get(&map_key).map(|(_, version)| *version) != Some(expected) {
                return Err(SecretError::VersionConflict);
            }
            self.0.remove(&map_key);
            Ok(())
        }
    }

    #[test]
    fn credential_lifecycle_is_scoped_to_workspace_and_proxy() {
        let vault = ProxyCredentialVault::with_vault(Box::<MemoryVault>::default());
        let workspace = WorkspaceId::new("ws_one").unwrap();
        let other_workspace = WorkspaceId::new("ws_two").unwrap();
        let proxy = ProxyId::new("proxy_one").unwrap();
        let first = SecretValue::new(b"first".to_vec()).unwrap();
        vault
            .store(&workspace, &proxy, 1, first.clone(), "test")
            .unwrap();

        assert_eq!(
            vault.load(&workspace, &proxy, 1, "test").unwrap(),
            Some(first)
        );
        assert_eq!(vault.load(&workspace, &proxy, 2, "test").unwrap(), None);
        assert_eq!(
            vault.load(&other_workspace, &proxy, 1, "test").unwrap(),
            None
        );

        let replacement = SecretValue::new(b"replacement".to_vec()).unwrap();
        vault
            .store(&workspace, &proxy, 1, replacement.clone(), "test")
            .unwrap();
        assert_eq!(
            vault.load(&workspace, &proxy, 1, "test").unwrap(),
            Some(replacement)
        );

        vault.remove(&workspace, &proxy, 1, "test").unwrap();
        assert_eq!(vault.load(&workspace, &proxy, 1, "test").unwrap(), None);
    }
}
