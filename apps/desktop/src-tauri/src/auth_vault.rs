use buglogin_domain::{errors::Problem, ids::DeviceId};
use buglogin_storage::{
    secrets::{OsVault, RotationReceipt, SecretError, SecretKey, SecretValue, SecretVersion},
    vault::NativeVault,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, Zeroizing};

const NAMESPACE: &str = "buglogin-auth";

pub struct StoredBearer {
    bearer: String,
    version: SecretVersion,
}

pub struct StoredAuthSession {
    access_token: String,
    refresh_token: Option<String>,
    expires_at: u64,
    refresh_pending: bool,
    version: SecretVersion,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AuthSessionValue {
    access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    refresh_token: Option<String>,
    expires_at: u64,
    #[serde(default)]
    refresh_pending: bool,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PendingSsoValue {
    state: String,
    expires_at: u64,
}

impl Drop for PendingSsoValue {
    fn drop(&mut self) {
        self.state.zeroize();
    }
}

impl Drop for AuthSessionValue {
    fn drop(&mut self) {
        self.access_token.zeroize();
        if let Some(refresh_token) = &mut self.refresh_token {
            refresh_token.zeroize();
        }
    }
}

impl Drop for StoredAuthSession {
    fn drop(&mut self) {
        self.access_token.zeroize();
        if let Some(refresh_token) = &mut self.refresh_token {
            refresh_token.zeroize();
        }
    }
}

impl Drop for StoredBearer {
    fn drop(&mut self) {
        self.bearer.zeroize();
    }
}

impl StoredAuthSession {
    pub fn access_token(&self) -> &str {
        &self.access_token
    }

    pub fn access_token_for_resume(&self) -> Option<&str> {
        (!self.refresh_pending).then_some(self.access_token.as_str())
    }

    pub fn refresh_token(&self) -> Option<&str> {
        self.refresh_token.as_deref()
    }

    pub const fn expires_at(&self) -> u64 {
        self.expires_at
    }

    pub const fn refresh_pending(&self) -> bool {
        self.refresh_pending
    }

    pub const fn version(&self) -> SecretVersion {
        self.version
    }
}

impl std::fmt::Debug for StoredAuthSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StoredAuthSession")
            .field("access_token", &"[REDACTED]")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "[REDACTED]"),
            )
            .field("expires_at", &self.expires_at)
            .field("refresh_pending", &self.refresh_pending)
            .field("version", &self.version)
            .finish()
    }
}

impl StoredBearer {
    pub fn bearer(&self) -> &str {
        &self.bearer
    }

    pub const fn version(&self) -> SecretVersion {
        self.version
    }
}

impl std::fmt::Debug for StoredBearer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StoredBearer")
            .field("bearer", &"[REDACTED]")
            .field("version", &self.version)
            .finish()
    }
}

pub struct AuthVault<V = NativeVault> {
    vault: V,
}

impl AuthVault<NativeVault> {
    pub const fn native() -> Self {
        Self {
            vault: NativeVault::new(),
        }
    }
}

impl<V: OsVault> AuthVault<V> {
    pub const fn new(vault: V) -> Self {
        Self { vault }
    }

    pub fn load_session(
        &self,
        device_id: &str,
        correlation_id: &str,
    ) -> Result<Option<StoredAuthSession>, Problem> {
        let key = session_key(device_id, correlation_id)?;
        match self.vault.read(&key) {
            Ok((value, version)) => {
                let bytes = value.as_bytes();
                let text = std::str::from_utf8(bytes).map_err(|_| {
                    vault_problem(
                        SecretError::OperationFailed,
                        "Stored session is invalid",
                        correlation_id,
                    )
                })?;
                if let Ok(mut session) = serde_json::from_str::<AuthSessionValue>(text) {
                    if session.access_token.is_empty()
                        || session.refresh_token.as_ref().is_some_and(String::is_empty)
                        || (session.refresh_token.is_some() && session.expires_at == 0)
                    {
                        return Err(vault_problem(
                            SecretError::OperationFailed,
                            "Stored session is invalid",
                            correlation_id,
                        ));
                    }
                    let access_token = std::mem::take(&mut session.access_token);
                    let refresh_token = session.refresh_token.take();
                    return Ok(Some(StoredAuthSession {
                        access_token,
                        refresh_token,
                        expires_at: session.expires_at,
                        refresh_pending: session.refresh_pending,
                        version,
                    }));
                }
                if text.starts_with('{') {
                    return Err(vault_problem(
                        SecretError::OperationFailed,
                        "Stored session is invalid",
                        correlation_id,
                    ));
                }
                Ok(Some(StoredAuthSession {
                    access_token: text.to_owned(),
                    refresh_token: None,
                    expires_at: 0,
                    refresh_pending: false,
                    version,
                }))
            }
            Err(SecretError::NotFound) => Ok(None),
            Err(error) => Err(vault_problem(
                error,
                "Unable to load session",
                correlation_id,
            )),
        }
    }

    pub fn store_session(
        &mut self,
        device_id: &str,
        access_token: &str,
        refresh_token: &str,
        expires_at: u64,
        correlation_id: &str,
    ) -> Result<SecretVersion, Problem> {
        let key = session_key(device_id, correlation_id)?;
        let value = session_value_optional(
            access_token,
            Some(refresh_token),
            expires_at,
            false,
            correlation_id,
        )?;
        self.vault
            .create(&key, value)
            .map_err(|error| vault_problem(error, "Unable to store session", correlation_id))
    }

    pub fn replace_session(
        &mut self,
        device_id: &str,
        expected_version: SecretVersion,
        access_token: &str,
        refresh_token: &str,
        expires_at: u64,
        correlation_id: &str,
    ) -> Result<SecretVersion, Problem> {
        let key = session_key(device_id, correlation_id)?;
        let value = session_value_optional(
            access_token,
            Some(refresh_token),
            expires_at,
            false,
            correlation_id,
        )?;
        let RotationReceipt { current, .. } = self
            .vault
            .rotate(&key, expected_version, value)
            .map_err(|error| vault_problem(error, "Unable to replace session", correlation_id))?;
        Ok(current)
    }

    pub fn store_legacy_session(
        &mut self,
        device_id: &str,
        access_token: &str,
        expires_at: u64,
        correlation_id: &str,
    ) -> Result<SecretVersion, Problem> {
        let key = session_key(device_id, correlation_id)?;
        let value = session_value_optional(access_token, None, expires_at, false, correlation_id)?;
        self.vault
            .create(&key, value)
            .map_err(|error| vault_problem(error, "Unable to store session", correlation_id))
    }

    pub fn replace_legacy_session(
        &mut self,
        device_id: &str,
        expected_version: SecretVersion,
        access_token: &str,
        expires_at: u64,
        correlation_id: &str,
    ) -> Result<SecretVersion, Problem> {
        let key = session_key(device_id, correlation_id)?;
        let value = session_value_optional(access_token, None, expires_at, false, correlation_id)?;
        let RotationReceipt { current, .. } = self
            .vault
            .rotate(&key, expected_version, value)
            .map_err(|error| vault_problem(error, "Unable to replace session", correlation_id))?;
        Ok(current)
    }

    pub fn mark_refresh_pending(
        &mut self,
        device_id: &str,
        expected_version: SecretVersion,
        correlation_id: &str,
    ) -> Result<SecretVersion, Problem> {
        self.set_refresh_pending(device_id, expected_version, true, correlation_id)
    }

    pub fn clear_refresh_pending(
        &mut self,
        device_id: &str,
        expected_version: SecretVersion,
        correlation_id: &str,
    ) -> Result<SecretVersion, Problem> {
        self.set_refresh_pending(device_id, expected_version, false, correlation_id)
    }

    fn set_refresh_pending(
        &mut self,
        device_id: &str,
        expected_version: SecretVersion,
        refresh_pending: bool,
        correlation_id: &str,
    ) -> Result<SecretVersion, Problem> {
        let key = session_key(device_id, correlation_id)?;
        let (current, _) = self
            .vault
            .read(&key)
            .map_err(|error| vault_problem(error, "Unable to read session", correlation_id))?;
        let text = std::str::from_utf8(current.as_bytes()).map_err(|_| {
            vault_problem(
                SecretError::OperationFailed,
                "Stored session is invalid",
                correlation_id,
            )
        })?;
        let mut session = match serde_json::from_str::<AuthSessionValue>(text) {
            Ok(session) => session,
            Err(_) if !text.starts_with('{') => AuthSessionValue {
                access_token: text.to_owned(),
                refresh_token: None,
                expires_at: 0,
                refresh_pending: false,
            },
            Err(_) => {
                return Err(vault_problem(
                    SecretError::OperationFailed,
                    "Stored session is invalid",
                    correlation_id,
                ));
            }
        };
        session.refresh_pending = refresh_pending;
        let value = session_value_optional(
            &session.access_token,
            session.refresh_token.as_deref(),
            session.expires_at,
            refresh_pending,
            correlation_id,
        )?;
        let RotationReceipt { current, .. } = self
            .vault
            .rotate(&key, expected_version, value)
            .map_err(|error| {
                vault_problem(error, "Unable to stage session refresh", correlation_id)
            })?;
        Ok(current)
    }

    pub fn load_bearer(
        &self,
        device_id: &str,
        correlation_id: &str,
    ) -> Result<Option<StoredBearer>, Problem> {
        let key = session_key(device_id, correlation_id)?;
        match self.vault.read(&key) {
            Ok((value, version)) => {
                let bearer = String::from_utf8(value.as_bytes().to_vec()).map_err(|_| {
                    vault_problem(
                        SecretError::OperationFailed,
                        "Stored bearer is invalid",
                        correlation_id,
                    )
                })?;
                Ok(Some(StoredBearer { bearer, version }))
            }
            Err(SecretError::NotFound) => Ok(None),
            Err(error) => Err(vault_problem(
                error,
                "Unable to load bearer",
                correlation_id,
            )),
        }
    }

    pub fn store_bearer(
        &mut self,
        device_id: &str,
        bearer: &str,
        correlation_id: &str,
    ) -> Result<SecretVersion, Problem> {
        let key = session_key(device_id, correlation_id)?;
        let value = secret_value(bearer, correlation_id)?;
        self.vault
            .create(&key, value)
            .map_err(|error| vault_problem(error, "Unable to store bearer", correlation_id))
    }

    pub fn replace_bearer(
        &mut self,
        device_id: &str,
        expected_version: SecretVersion,
        bearer: &str,
        correlation_id: &str,
    ) -> Result<SecretVersion, Problem> {
        let key = session_key(device_id, correlation_id)?;
        let value = secret_value(bearer, correlation_id)?;
        let RotationReceipt { current, .. } = self
            .vault
            .rotate(&key, expected_version, value)
            .map_err(|error| vault_problem(error, "Unable to replace bearer", correlation_id))?;
        Ok(current)
    }

    pub fn remove_bearer(
        &mut self,
        device_id: &str,
        expected_version: SecretVersion,
        correlation_id: &str,
    ) -> Result<(), Problem> {
        let key = session_key(device_id, correlation_id)?;
        self.vault
            .remove(&key, expected_version)
            .map_err(|error| vault_problem(error, "Unable to remove bearer", correlation_id))
    }

    pub fn load_pending_sso(
        &self,
        device_id: &str,
        correlation_id: &str,
    ) -> Result<Option<(Zeroizing<String>, u64)>, Problem> {
        let key = pending_sso_key(device_id, correlation_id)?;
        let (value, _) = match self.vault.read(&key) {
            Ok(stored) => stored,
            Err(SecretError::NotFound) => return Ok(None),
            Err(error) => {
                return Err(vault_problem(
                    error,
                    "Unable to load pending sign-in",
                    correlation_id,
                ));
            }
        };
        let mut pending: PendingSsoValue =
            serde_json::from_slice(value.as_bytes()).map_err(|_| {
                vault_problem(
                    SecretError::OperationFailed,
                    "Pending sign-in is invalid",
                    correlation_id,
                )
            })?;
        if pending.state.is_empty() || pending.expires_at == 0 {
            return Err(vault_problem(
                SecretError::OperationFailed,
                "Pending sign-in is invalid",
                correlation_id,
            ));
        }
        Ok(Some((
            Zeroizing::new(std::mem::take(&mut pending.state)),
            pending.expires_at,
        )))
    }

    pub fn store_pending_sso(
        &mut self,
        device_id: &str,
        state: &str,
        expires_at: u64,
        correlation_id: &str,
    ) -> Result<(), Problem> {
        if state.is_empty() || state.len() > 512 || expires_at == 0 {
            return Err(Problem::new(
                "invalid_sso_state",
                "Pending sign-in is invalid",
                400,
                correlation_id,
            ));
        }
        let key = pending_sso_key(device_id, correlation_id)?;
        let mut pending = PendingSsoValue {
            state: state.to_owned(),
            expires_at,
        };
        let mut bytes = serde_json::to_vec(&pending).map_err(|_| {
            Problem::new(
                "sso_state_store_failed",
                "Unable to save pending sign-in",
                500,
                correlation_id,
            )
        })?;
        let value = SecretValue::new(bytes.clone()).map_err(|error| {
            vault_problem(error, "Unable to save pending sign-in", correlation_id)
        })?;
        bytes.zeroize();
        pending.state.zeroize();
        match self.vault.read(&key) {
            Ok((_, version)) => {
                self.vault
                    .rotate(&key, version, value)
                    .map(|_| ())
                    .map_err(|error| {
                        vault_problem(error, "Unable to save pending sign-in", correlation_id)
                    })
            }
            Err(SecretError::NotFound) => {
                self.vault.create(&key, value).map(|_| ()).map_err(|error| {
                    vault_problem(error, "Unable to save pending sign-in", correlation_id)
                })
            }
            Err(error) => Err(vault_problem(
                error,
                "Unable to save pending sign-in",
                correlation_id,
            )),
        }
    }

    pub fn remove_pending_sso(
        &mut self,
        device_id: &str,
        correlation_id: &str,
    ) -> Result<(), Problem> {
        let key = pending_sso_key(device_id, correlation_id)?;
        match self.vault.read(&key) {
            Ok((_, version)) => self.vault.remove(&key, version).map_err(|error| {
                vault_problem(error, "Unable to clear pending sign-in", correlation_id)
            }),
            Err(SecretError::NotFound) => Ok(()),
            Err(error) => Err(vault_problem(
                error,
                "Unable to clear pending sign-in",
                correlation_id,
            )),
        }
    }
}

fn session_key(device_id: &str, correlation_id: &str) -> Result<SecretKey, Problem> {
    DeviceId::new(device_id).map_err(|_| {
        Problem::new(
            "device_id_invalid",
            "Device ID is invalid",
            400,
            correlation_id,
        )
    })?;
    let digest = Sha256::digest(device_id.as_bytes());
    SecretKey::new(NAMESPACE, format!("device-{:x}", digest))
        .map_err(|error| vault_problem(error, "Unable to build session key", correlation_id))
}

fn pending_sso_key(device_id: &str, correlation_id: &str) -> Result<SecretKey, Problem> {
    DeviceId::new(device_id).map_err(|_| {
        Problem::new(
            "device_id_invalid",
            "Device ID is invalid",
            400,
            correlation_id,
        )
    })?;
    let digest = Sha256::digest(device_id.as_bytes());
    SecretKey::new(NAMESPACE, format!("sso-device-{:x}", digest)).map_err(|error| {
        vault_problem(error, "Unable to build pending sign-in key", correlation_id)
    })
}

pub(crate) fn pending_sso_matches(
    expected_state: &str,
    expires_at: u64,
    callback_state: &str,
    now: u64,
) -> bool {
    !expected_state.is_empty() && expires_at > now && expected_state == callback_state
}

fn secret_value(bearer: &str, correlation_id: &str) -> Result<SecretValue, Problem> {
    SecretValue::new(bearer.as_bytes().to_vec())
        .map_err(|error| vault_problem(error, "Bearer is invalid", correlation_id))
}

fn session_value_optional(
    access_token: &str,
    refresh_token: Option<&str>,
    expires_at: u64,
    refresh_pending: bool,
    correlation_id: &str,
) -> Result<SecretValue, Problem> {
    if access_token.is_empty()
        || refresh_token.is_some_and(str::is_empty)
        || (refresh_token.is_some() && expires_at == 0)
    {
        return Err(vault_problem(
            SecretError::EmptyValue,
            "Session is invalid",
            correlation_id,
        ));
    }
    let bytes = serde_json::to_vec(&AuthSessionValue {
        access_token: access_token.to_owned(),
        refresh_token: refresh_token.map(str::to_owned),
        expires_at,
        refresh_pending,
    })
    .map_err(|_| {
        vault_problem(
            SecretError::OperationFailed,
            "Unable to encode session",
            correlation_id,
        )
    })?;
    SecretValue::new(bytes)
        .map_err(|error| vault_problem(error, "Session is invalid", correlation_id))
}

fn vault_problem(error: SecretError, title: &str, correlation_id: &str) -> Problem {
    let status = match error {
        SecretError::InvalidKey | SecretError::EmptyValue | SecretError::TooLarge => 400,
        SecretError::NotFound => 404,
        SecretError::AccessDenied => 403,
        SecretError::VersionConflict => 409,
        SecretError::Unavailable => 503,
        SecretError::OperationFailed => 500,
    };
    let mut problem = Problem::new(error.code(), title, status, correlation_id);
    if error == SecretError::VersionConflict || error == SecretError::Unavailable {
        problem.retryable = true;
    }
    problem
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[derive(Default)]
    struct FakeVault {
        values: BTreeMap<(String, String), (SecretValue, SecretVersion)>,
    }

    impl OsVault for FakeVault {
        fn read(&self, key: &SecretKey) -> Result<(SecretValue, SecretVersion), SecretError> {
            self.values
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
            if self.values.contains_key(&map_key) {
                return Err(SecretError::VersionConflict);
            }
            let version = SecretVersion::new(1).unwrap();
            self.values.insert(map_key, (value, version));
            Ok(version)
        }

        fn rotate(
            &mut self,
            key: &SecretKey,
            expected: SecretVersion,
            replacement: SecretValue,
        ) -> Result<RotationReceipt, SecretError> {
            let map_key = (key.namespace().to_owned(), key.name().to_owned());
            let current = self
                .values
                .get(&map_key)
                .map(|(_, version)| *version)
                .ok_or(SecretError::NotFound)?;
            if current != expected {
                return Err(SecretError::VersionConflict);
            }
            let next = SecretVersion::new(current.generation() + 1).unwrap();
            self.values.insert(map_key, (replacement, next));
            Ok(RotationReceipt {
                previous: current,
                current: next,
            })
        }

        fn remove(&mut self, key: &SecretKey, expected: SecretVersion) -> Result<(), SecretError> {
            let map_key = (key.namespace().to_owned(), key.name().to_owned());
            let (_, current) = self.values.get(&map_key).ok_or(SecretError::NotFound)?;
            if *current != expected {
                return Err(SecretError::VersionConflict);
            }
            self.values.remove(&map_key);
            Ok(())
        }
    }

    #[test]
    fn bearer_round_trip_replacement_and_removal_use_versions() {
        let mut vault = AuthVault::new(FakeVault::default());
        let first = vault
            .store_bearer("device-a1", "first-token", "cor-store")
            .unwrap();
        assert_eq!(
            vault
                .load_bearer("device-a1", "cor-load")
                .unwrap()
                .unwrap()
                .bearer(),
            "first-token"
        );
        let second = vault
            .replace_bearer("device-a1", first, "second-token", "cor-replace")
            .unwrap();
        assert_eq!(second.generation(), 2);
        assert_eq!(
            vault
                .load_bearer("device-a1", "cor-load")
                .unwrap()
                .unwrap()
                .bearer(),
            "second-token"
        );
        vault
            .remove_bearer("device-a1", second, "cor-remove")
            .unwrap();
        assert!(
            vault
                .load_bearer("device-a1", "cor-load")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn missing_bearer_maps_to_none_and_device_key_is_stable() {
        let vault = AuthVault::new(FakeVault::default());
        assert!(
            vault
                .load_bearer("device-a1", "cor-load")
                .unwrap()
                .is_none()
        );
        let first = session_key("device-a1", "cor-key").unwrap();
        let second = session_key("device-a1", "cor-key").unwrap();
        assert_eq!(first, second);
        assert_eq!(
            session_key(" device-a1 ", "cor-key").unwrap_err().code,
            "device_id_invalid"
        );
        assert_ne!(
            first.name(),
            session_key("device-b1", "cor-key").unwrap().name()
        );
    }

    #[test]
    fn errors_are_typed_and_never_contain_bearer() {
        let mut vault = AuthVault::new(FakeVault::default());
        let error = vault
            .store_bearer("device-a1", "", "cor-empty")
            .unwrap_err();
        assert_eq!(error.code, "empty_secret");
        assert!(!format!("{error:?}").contains("first-token"));
        assert!(
            vault
                .load_bearer("device-a1", "cor-load")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn session_pair_round_trips_and_rotates_as_one_vault_value() {
        let mut vault = AuthVault::new(FakeVault::default());
        let first = vault
            .store_session("device-a1", "access-one", "refresh-one", 100, "cor-store")
            .unwrap();
        let loaded = vault
            .load_session("device-a1", "cor-load")
            .unwrap()
            .unwrap();
        assert_eq!(loaded.access_token(), "access-one");
        assert_eq!(loaded.access_token_for_resume(), Some("access-one"));
        assert_eq!(loaded.refresh_token(), Some("refresh-one"));
        assert_eq!(loaded.expires_at(), 100);
        assert_eq!(loaded.version(), first);
        assert!(!format!("{loaded:?}").contains("refresh-one"));

        let staged_version = vault
            .mark_refresh_pending("device-a1", loaded.version(), "cor-stage")
            .unwrap();
        let staged = vault
            .load_session("device-a1", "cor-load-staged")
            .unwrap()
            .unwrap();
        assert!(staged.refresh_pending());
        assert_eq!(staged.access_token_for_resume(), None);
        assert_eq!(staged.version(), staged_version);

        let next = vault
            .replace_session(
                "device-a1",
                staged.version(),
                "access-two",
                "refresh-two",
                200,
                "cor-rotate",
            )
            .unwrap();
        let loaded = vault
            .load_session("device-a1", "cor-load-next")
            .unwrap()
            .unwrap();
        assert_eq!(loaded.version(), next);
        assert!(!loaded.refresh_pending());
        assert_eq!(loaded.access_token(), "access-two");
        assert_eq!(loaded.refresh_token(), Some("refresh-two"));
    }

    #[test]
    fn pending_sso_survives_a_new_auth_vault_instance_and_expires_by_timestamp() {
        let shared_vault = FakeVault::default();
        let mut first_process = AuthVault::new(shared_vault);
        first_process
            .store_pending_sso("device-a1", "one-time-state", 2_000, "cor-sso-start")
            .unwrap();

        // A separately constructed AuthVault models the app process that receives
        // the OS deep link after Windows launches another process.
        let shared_vault = first_process.vault;
        let second_process = AuthVault::new(shared_vault);
        let (state, expires_at) = second_process
            .load_pending_sso("device-a1", "cor-sso-callback")
            .unwrap()
            .unwrap();
        assert_eq!(state.as_str(), "one-time-state");
        assert_eq!(expires_at, 2_000);
        assert!(pending_sso_matches(
            state.as_str(),
            expires_at,
            "one-time-state",
            1_999
        ));
        assert!(!pending_sso_matches(
            state.as_str(),
            expires_at,
            "one-time-state",
            2_000
        ));
        assert!(!pending_sso_matches(
            state.as_str(),
            expires_at,
            "different-state",
            1_999
        ));
        assert!(
            second_process
                .load_pending_sso("device-b1", "cor-other-device")
                .unwrap()
                .is_none()
        );

        let mut second_process = second_process;
        second_process
            .remove_pending_sso("device-a1", "cor-sso-complete")
            .unwrap();
        assert!(
            second_process
                .load_pending_sso("device-a1", "cor-sso-after-complete")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn definitely_unsent_refresh_can_clear_pending_marker_without_rotating_tokens() {
        let mut vault = AuthVault::new(FakeVault::default());
        let stored = vault
            .store_session("device-a1", "access-one", "refresh-one", 100, "cor-store")
            .unwrap();
        let staged = vault
            .mark_refresh_pending("device-a1", stored, "cor-stage")
            .unwrap();
        let cleared = vault
            .clear_refresh_pending("device-a1", staged, "cor-clear")
            .unwrap();
        let loaded = vault
            .load_session("device-a1", "cor-load-cleared")
            .unwrap()
            .unwrap();

        assert!(!loaded.refresh_pending());
        assert_eq!(loaded.access_token_for_resume(), Some("access-one"));
        assert_eq!(loaded.refresh_token(), Some("refresh-one"));
        assert_eq!(loaded.version(), cleared);
    }

    #[test]
    fn legacy_bearer_is_readable_as_a_session_without_refresh_authority() {
        let mut vault = AuthVault::new(FakeVault::default());
        vault
            .store_bearer("device-legacy1", "legacy-access", "cor-legacy")
            .unwrap();
        let loaded = vault
            .load_session("device-legacy1", "cor-load-legacy")
            .unwrap()
            .unwrap();
        assert_eq!(loaded.access_token(), "legacy-access");
        assert_eq!(loaded.refresh_token(), None);
        assert_eq!(loaded.expires_at(), 0);
        let pending_version = vault
            .mark_refresh_pending("device-legacy1", loaded.version(), "cor-stage-legacy")
            .unwrap();
        let pending = vault
            .load_session("device-legacy1", "cor-load-pending-legacy")
            .unwrap()
            .unwrap();
        assert!(pending.refresh_pending());
        assert_eq!(pending.version(), pending_version);
    }
}
