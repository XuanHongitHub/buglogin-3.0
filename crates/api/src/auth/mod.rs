use std::collections::{BTreeSet, HashMap};

use buglogin_domain::{
    errors::Problem,
    identity::{
        IssuedBrowserSession, SessionRecord, SessionRefreshReplay, issue, issue_browser,
        issue_for_device, refresh as refresh_session, verify, verify_csrf,
    },
    ids::{AccountId, DeviceId},
};
use buglogin_sync::crypto::{AuthenticatedChunk, ClientKey};
use getrandom::fill;
use hkdf::Hkdf;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, Zeroizing};

use crate::services::{
    credentials::CredentialService,
    device_authorization::DeviceAuthorizationConsumption,
    session_persistence::{DeviceRefreshRecord, SessionPersistence},
};
use buglogin_storage::control::{ControlDelete, ControlPut};

const REFRESH_REPLAY_SECONDS: u64 = 5 * 60;
const REFRESH_REPLAY_KDF_INFO: &[u8] = b"buglogin/auth-refresh-response/v1";
const REFRESH_REPLAY_AAD_PREFIX: &str = "buglogin/auth-refresh-response/v1";

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RefreshReplayPayload {
    access_token: String,
    refresh_token: Option<String>,
    expires_at: u64,
}

impl Drop for RefreshReplayPayload {
    fn drop(&mut self) {
        self.access_token.zeroize();
        if let Some(refresh_token) = &mut self.refresh_token {
            refresh_token.zeroize();
        }
    }
}

#[derive(Clone)]
struct StoredSession {
    record: SessionRecord,
    revision: u64,
}

#[derive(Clone)]
struct StoredDeviceRefresh {
    record: DeviceRefreshRecord,
    revision: u64,
}

pub struct DeviceSessionTokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: u64,
}

impl std::fmt::Debug for DeviceSessionTokens {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DeviceSessionTokens")
            .field("access_token", &"[REDACTED]")
            .field("refresh_token", &"[REDACTED]")
            .finish()
    }
}

impl Drop for DeviceSessionTokens {
    fn drop(&mut self) {
        self.access_token.zeroize();
        self.refresh_token.zeroize();
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionKind {
    Browser,
    Desktop,
    Api,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub id: String,
    pub kind: SessionKind,
    pub device_id: Option<String>,
    pub created_at: Option<u64>,
    pub expires_at: u64,
    pub revoked_at: Option<u64>,
    pub current: bool,
}

#[derive(Default)]
pub struct AuthService {
    sessions: HashMap<[u8; 32], StoredSession>,
    device_refresh: HashMap<[u8; 32], StoredDeviceRefresh>,
    revoked_families: HashMap<String, u64>,
    persistence: Option<SessionPersistence>,
    credentials: Option<CredentialService>,
}

impl AuthService {
    pub fn persistent(persistence: SessionPersistence) -> Result<Self, Problem> {
        Ok(Self {
            sessions: HashMap::new(),
            device_refresh: HashMap::new(),
            revoked_families: HashMap::new(),
            persistence: Some(persistence),
            credentials: None,
        })
    }

    pub fn persistent_with_credentials(
        persistence: SessionPersistence,
        credentials: CredentialService,
    ) -> Result<Self, Problem> {
        let mut auth = Self::persistent(persistence)?;
        auth.credentials = Some(credentials);
        Ok(auth)
    }

    pub fn purge_expired_sessions(&self, now: u64) -> Result<usize, Problem> {
        self.persistence
            .as_ref()
            .map_or(Ok(0), |persistence| persistence.purge_expired(now))
    }

    pub fn issue(&mut self, account_id: AccountId, expires_at: u64) -> Result<String, Problem> {
        let mut issued = issue(account_id, expires_at)?;
        self.store_issued(issued.record.clone())?;
        Ok(std::mem::take(&mut issued.raw_token))
    }

    pub fn issue_with_credential_version(
        &mut self,
        account_id: AccountId,
        expires_at: u64,
        credential_version: u32,
    ) -> Result<String, Problem> {
        let mut issued = issue(account_id, expires_at)?;
        issued.record.credential_version = Some(credential_version);
        self.store_issued(issued.record.clone())?;
        Ok(std::mem::take(&mut issued.raw_token))
    }

    pub fn issue_with_account_revision(
        &mut self,
        account_id: AccountId,
        expires_at: u64,
        account_revision: u64,
    ) -> Result<String, Problem> {
        let mut issued = issue(account_id, expires_at)?;
        self.store_issued_with_account_revision(issued.record.clone(), Some(account_revision))?;
        Ok(std::mem::take(&mut issued.raw_token))
    }

    pub fn issue_with_credential_version_and_account_revision(
        &mut self,
        account_id: AccountId,
        expires_at: u64,
        credential_version: u32,
        account_revision: u64,
    ) -> Result<String, Problem> {
        let mut issued = issue(account_id, expires_at)?;
        issued.record.credential_version = Some(credential_version);
        self.store_issued_with_account_revision(issued.record.clone(), Some(account_revision))?;
        Ok(std::mem::take(&mut issued.raw_token))
    }

    pub fn issue_for_device(
        &mut self,
        account_id: AccountId,
        expires_at: u64,
        device_id: String,
    ) -> Result<String, Problem> {
        let mut issued = issue_for_device(account_id, expires_at, Some(device_id))?;
        self.store_issued(issued.record.clone())?;
        Ok(std::mem::take(&mut issued.raw_token))
    }

    pub fn issue_for_device_with_credential_version(
        &mut self,
        account_id: AccountId,
        expires_at: u64,
        device_id: String,
        credential_version: u32,
    ) -> Result<String, Problem> {
        let mut issued = issue_for_device(account_id, expires_at, Some(device_id))?;
        issued.record.credential_version = Some(credential_version);
        self.store_issued(issued.record.clone())?;
        Ok(std::mem::take(&mut issued.raw_token))
    }

    pub fn issue_for_device_at(
        &mut self,
        account_id: AccountId,
        expires_at: u64,
        device_id: String,
        created_at: u64,
    ) -> Result<String, Problem> {
        let mut issued = issue_for_device(account_id, expires_at, Some(device_id))?;
        issued.record.created_at = Some(created_at);
        issued.record.last_seen_at = Some(created_at);
        self.store_issued(issued.record.clone())?;
        Ok(std::mem::take(&mut issued.raw_token))
    }

    pub fn issue_device_session(
        &mut self,
        account_id: AccountId,
        access_expires_at: u64,
        refresh_expires_at: u64,
        device_id: String,
        credential_version: Option<u32>,
    ) -> Result<DeviceSessionTokens, Problem> {
        self.issue_device_session_with_account_revision(
            account_id,
            access_expires_at,
            refresh_expires_at,
            device_id,
            credential_version,
            None,
        )
    }

    pub fn issue_device_session_with_account_revision(
        &mut self,
        account_id: AccountId,
        access_expires_at: u64,
        refresh_expires_at: u64,
        device_id: String,
        credential_version: Option<u32>,
        account_revision: Option<u64>,
    ) -> Result<DeviceSessionTokens, Problem> {
        self.issue_device_session_inner(
            account_id,
            access_expires_at,
            refresh_expires_at,
            device_id,
            credential_version,
            account_revision,
            Vec::new(),
        )
    }

    pub(crate) fn issue_device_session_with_account_revision_and_consumption(
        &mut self,
        account_id: AccountId,
        access_expires_at: u64,
        refresh_expires_at: u64,
        device_id: String,
        credential_version: Option<u32>,
        account_revision: Option<u64>,
        consumption: DeviceAuthorizationConsumption,
    ) -> Result<DeviceSessionTokens, Problem> {
        if self.persistence.is_none() {
            return Err(Problem::new(
                "auth_unavailable",
                "Persistent authentication storage is required for device-code exchange",
                503,
                "identity",
            ));
        }
        self.issue_device_session_inner(
            account_id,
            access_expires_at,
            refresh_expires_at,
            device_id,
            credential_version,
            account_revision,
            consumption.into_deletes(),
        )
    }

    fn issue_device_session_inner(
        &mut self,
        account_id: AccountId,
        access_expires_at: u64,
        refresh_expires_at: u64,
        device_id: String,
        credential_version: Option<u32>,
        account_revision: Option<u64>,
        deletes: Vec<ControlDelete>,
    ) -> Result<DeviceSessionTokens, Problem> {
        if access_expires_at == 0 || refresh_expires_at <= access_expires_at {
            return Err(invalid_device_session_expiry());
        }
        if DeviceId::new(device_id.clone()).is_err() {
            return Err(Problem::new(
                "device_session_required",
                "A connected device session is required",
                400,
                "identity",
            ));
        }
        let mut issued = issue_for_device(account_id, access_expires_at, Some(device_id.clone()))?;
        issued.record.credential_version = credential_version;
        let record = &issued.record;
        let mut refresh_token = Zeroizing::new(random_refresh_token()?);
        let refresh_record = DeviceRefreshRecord {
            account_id: record.account_id.clone(),
            token_hash: sha256(refresh_token.as_bytes()),
            access_token_hash: record.token_hash,
            device_id,
            family_id: record.family_id.clone().ok_or_else(invalid_session)?,
            expires_at: refresh_expires_at,
            consumed_at: None,
            credential_version,
            refresh_replay: None,
        };
        let revision = match &self.persistence {
            Some(persistence) => {
                persistence.create_device_session_with_account_revision_and_deletes(
                    record,
                    &refresh_record,
                    account_revision,
                    deletes,
                )?;
                1
            }
            None => {
                self.sessions.insert(
                    record.token_hash,
                    StoredSession {
                        record: (*record).clone(),
                        revision: 0,
                    },
                );
                0
            }
        };
        if self.persistence.is_none() {
            self.device_refresh.insert(
                refresh_record.token_hash,
                StoredDeviceRefresh {
                    record: refresh_record,
                    revision,
                },
            );
        }
        Ok(DeviceSessionTokens {
            access_token: std::mem::take(&mut issued.raw_token),
            refresh_token: std::mem::take(&mut *refresh_token),
            expires_at: access_expires_at,
        })
    }

    pub fn refresh_device_session(
        &mut self,
        raw_refresh_token: &str,
        device_id: &str,
        now: u64,
        access_expires_at: u64,
    ) -> Result<DeviceSessionTokens, Problem> {
        self.refresh_device_session_with_idempotency(
            raw_refresh_token,
            device_id,
            None,
            now,
            access_expires_at,
        )
    }

    pub fn refresh_device_session_with_idempotency(
        &mut self,
        raw_refresh_token: &str,
        device_id: &str,
        idempotency_key: Option<&str>,
        now: u64,
        access_expires_at: u64,
    ) -> Result<DeviceSessionTokens, Problem> {
        self.refresh_device_session_with_idempotency_and_account_revision(
            raw_refresh_token,
            device_id,
            idempotency_key,
            now,
            access_expires_at,
            None,
        )
    }

    pub fn refresh_device_session_with_idempotency_and_account_revision(
        &mut self,
        raw_refresh_token: &str,
        device_id: &str,
        idempotency_key: Option<&str>,
        now: u64,
        access_expires_at: u64,
        account_revision: Option<u64>,
    ) -> Result<DeviceSessionTokens, Problem> {
        if access_expires_at <= now {
            return Err(invalid_device_session_expiry());
        }
        validate_refresh_idempotency_key(idempotency_key)?;
        let old_refresh_hash = sha256(raw_refresh_token.as_bytes());
        let mut old_refresh = self
            .load_device_refresh(&old_refresh_hash)?
            .ok_or_else(invalid_session)?;
        if old_refresh.record.consumed_at.is_some() {
            return self.replay_or_revoke_device_refresh(
                &old_refresh.record,
                raw_refresh_token,
                device_id,
                idempotency_key,
                now,
            );
        }
        if old_refresh.record.expires_at <= now {
            return Err(session_expired());
        }
        if old_refresh.record.device_id != device_id || DeviceId::new(device_id).is_err() {
            return Err(session_revoked());
        }
        let family_revision = self.verify_refresh_family(&old_refresh.record, now)?;
        let mut old_session = self
            .load_stored(&old_refresh.record.access_token_hash)?
            .ok_or_else(invalid_session)?;
        if old_session.record.revoked_at.is_some()
            || old_session.record.account_id != old_refresh.record.account_id
            || old_session.record.device_id.as_deref() != Some(device_id)
            || old_session.record.family_id.as_deref()
                != Some(old_refresh.record.family_id.as_str())
            || old_session.record.credential_version != old_refresh.record.credential_version
        {
            self.revoke_family(
                &old_refresh.record.family_id,
                &old_refresh.record.account_id,
                now,
            )?;
            return Err(session_revoked());
        }
        if let Some(version) = old_refresh.record.credential_version {
            let credentials = self.credentials.as_ref().ok_or_else(|| {
                Problem::new(
                    "session_authority_unavailable",
                    "Session authority is unavailable",
                    503,
                    "identity",
                )
            })?;
            if credentials.password_version(&old_refresh.record.account_id)? != Some(version) {
                self.revoke_family(
                    &old_refresh.record.family_id,
                    &old_refresh.record.account_id,
                    now,
                )?;
                return Err(session_revoked());
            }
        }

        let mut issued = refresh_session(&old_session.record, access_expires_at)?;
        issued.record.last_seen_at = Some(now);
        let new_session = &issued.record;
        let mut refresh_token = Zeroizing::new(random_refresh_token()?);
        let new_refresh = DeviceRefreshRecord {
            account_id: old_refresh.record.account_id.clone(),
            token_hash: sha256(refresh_token.as_bytes()),
            access_token_hash: new_session.token_hash,
            device_id: old_refresh.record.device_id.clone(),
            family_id: old_refresh.record.family_id.clone(),
            expires_at: old_refresh.record.expires_at,
            consumed_at: None,
            credential_version: old_refresh.record.credential_version,
            refresh_replay: None,
        };
        old_session.record.revoked_at = Some(now);
        old_refresh.record.consumed_at = Some(now);
        if let Some(key) = idempotency_key {
            let payload = RefreshReplayPayload {
                access_token: issued.raw_token.clone(),
                refresh_token: Some(refresh_token.as_str().to_owned()),
                expires_at: access_expires_at,
            };
            old_refresh.record.refresh_replay = Some(seal_refresh_replay(
                raw_refresh_token,
                &old_refresh_hash,
                key,
                "device",
                Some(device_id),
                now,
                &payload,
            )?);
        }
        let (session_revision, new_session_revision, refresh_revision, new_refresh_revision) =
            match self.persistence.clone() {
                Some(persistence) => {
                    let next_session_revision = old_session
                        .revision
                        .checked_add(1)
                        .ok_or_else(revision_exhausted)?;
                    let next_refresh_revision = old_refresh
                        .revision
                        .checked_add(1)
                        .ok_or_else(revision_exhausted)?;
                    if let Err(error) = persistence.rotate_device_session(
                        &old_session.record,
                        old_session.revision,
                        &old_refresh.record,
                        old_refresh.revision,
                        family_revision,
                        new_session,
                        &new_refresh,
                        account_revision,
                    ) {
                        if let Some((latest, _)) =
                            persistence.get_device_refresh(&old_refresh_hash)?
                            && latest.consumed_at.is_some()
                        {
                            return self.replay_or_revoke_device_refresh(
                                &latest,
                                raw_refresh_token,
                                device_id,
                                idempotency_key,
                                now,
                            );
                        }
                        if account_revision.is_some() && error.code == "storage_revision_conflict" {
                            return Err(session_refresh_conflict());
                        }
                        return Err(error);
                    }
                    (next_session_revision, 1, next_refresh_revision, 1)
                }
                None => (0, 0, 0, 0),
            };
        if self.persistence.is_none() {
            self.sessions.insert(
                old_session.record.token_hash,
                StoredSession {
                    record: old_session.record,
                    revision: session_revision,
                },
            );
            self.sessions.insert(
                new_session.token_hash,
                StoredSession {
                    record: (*new_session).clone(),
                    revision: new_session_revision,
                },
            );
            self.device_refresh.insert(
                old_refresh_hash,
                StoredDeviceRefresh {
                    record: old_refresh.record,
                    revision: refresh_revision,
                },
            );
            self.device_refresh.insert(
                new_refresh.token_hash,
                StoredDeviceRefresh {
                    record: new_refresh,
                    revision: new_refresh_revision,
                },
            );
        }
        Ok(DeviceSessionTokens {
            access_token: std::mem::take(&mut issued.raw_token),
            refresh_token: std::mem::take(&mut *refresh_token),
            expires_at: access_expires_at,
        })
    }

    pub fn device_refresh_account_id(&self, raw_refresh_token: &str) -> Result<AccountId, Problem> {
        if !buglogin_domain::security::is_lower_hex_token_64(raw_refresh_token) {
            return Err(invalid_session());
        }
        let refresh_hash = sha256(raw_refresh_token.as_bytes());
        self.load_device_refresh(&refresh_hash)?
            .map(|stored| stored.record.account_id)
            .ok_or_else(invalid_session)
    }

    fn replay_or_revoke_device_refresh(
        &mut self,
        record: &DeviceRefreshRecord,
        raw_refresh_token: &str,
        device_id: &str,
        idempotency_key: Option<&str>,
        now: u64,
    ) -> Result<DeviceSessionTokens, Problem> {
        if record.device_id != device_id || DeviceId::new(device_id).is_err() {
            self.revoke_family(&record.family_id, &record.account_id, now)?;
            return Err(session_revoked());
        }
        if let (Some(key), Some(replay)) = (idempotency_key, record.refresh_replay.as_ref()) {
            if replay.idempotency_key_hash == sha256(key.as_bytes()) {
                if replay.expires_at <= now {
                    return Err(refresh_replay_expired());
                }
                self.verify_refresh_family(record, now)?;
                if let Some(version) = record.credential_version {
                    let credentials = self.credentials.as_ref().ok_or_else(|| {
                        Problem::new(
                            "session_authority_unavailable",
                            "Session authority is unavailable",
                            503,
                            "identity",
                        )
                    })?;
                    if credentials.password_version(&record.account_id)? != Some(version) {
                        return Err(session_revoked());
                    }
                }
                let mut payload = open_refresh_replay(
                    raw_refresh_token,
                    &record.token_hash,
                    key,
                    "device",
                    Some(device_id),
                    replay,
                )?;
                let refresh_token = payload
                    .refresh_token
                    .take()
                    .ok_or_else(refresh_replay_corrupt)?;
                return Ok(DeviceSessionTokens {
                    access_token: std::mem::take(&mut payload.access_token),
                    refresh_token,
                    expires_at: payload.expires_at,
                });
            }
        }
        self.revoke_family(&record.family_id, &record.account_id, now)?;
        Err(session_revoked())
    }

    pub fn issue_browser(
        &mut self,
        account_id: AccountId,
        expires_at: u64,
        created_at: u64,
    ) -> Result<IssuedBrowserSession, Problem> {
        let issued = issue_browser(account_id, expires_at, created_at)?;
        self.store_issued(issued.record.clone())?;
        Ok(issued)
    }

    pub fn issue_browser_with_credential_version(
        &mut self,
        account_id: AccountId,
        expires_at: u64,
        created_at: u64,
        credential_version: u32,
    ) -> Result<IssuedBrowserSession, Problem> {
        let mut issued = issue_browser(account_id, expires_at, created_at)?;
        issued.record.credential_version = Some(credential_version);
        self.store_issued(issued.record.clone())?;
        Ok(issued)
    }

    pub fn issue_browser_with_account_revision(
        &mut self,
        account_id: AccountId,
        expires_at: u64,
        created_at: u64,
        credential_version: u32,
        account_revision: u64,
    ) -> Result<IssuedBrowserSession, Problem> {
        let mut issued = issue_browser(account_id, expires_at, created_at)?;
        issued.record.credential_version = Some(credential_version);
        self.store_issued_with_account_revision(issued.record.clone(), Some(account_revision))?;
        Ok(issued)
    }

    pub fn authenticate_browser(
        &self,
        raw_session: &str,
        raw_csrf: &str,
        now: u64,
    ) -> Result<AccountId, Problem> {
        self.authenticate_browser_with_credential_version(raw_session, raw_csrf, now)
            .map(|(account_id, _)| account_id)
    }

    pub fn authenticate_browser_with_credential_version(
        &self,
        raw_session: &str,
        raw_csrf: &str,
        now: u64,
    ) -> Result<(AccountId, Option<u32>), Problem> {
        let hash = sha256(raw_session.as_bytes());
        let stored = self.load_stored(&hash)?.ok_or_else(invalid_session)?;
        self.verify_session(&stored.record, raw_session, now)?;
        verify_csrf(&stored.record, raw_csrf)?;
        Ok((stored.record.account_id, stored.record.credential_version))
    }

    pub fn browser_session(
        &self,
        raw_session: &str,
        raw_csrf: &str,
        now: u64,
    ) -> Result<(AccountId, u64), Problem> {
        let hash = sha256(raw_session.as_bytes());
        let stored = self.load_stored(&hash)?.ok_or_else(invalid_session)?;
        self.verify_session(&stored.record, raw_session, now)?;
        verify_csrf(&stored.record, raw_csrf)?;
        Ok((stored.record.account_id, stored.record.expires_at))
    }

    pub fn refresh(
        &mut self,
        raw_token: &str,
        now: u64,
        expires_at: u64,
    ) -> Result<String, Problem> {
        self.refresh_with_idempotency(raw_token, None, now, expires_at)
            .map(|(token, _)| token)
    }

    pub fn refresh_with_idempotency(
        &mut self,
        raw_token: &str,
        idempotency_key: Option<&str>,
        now: u64,
        expires_at: u64,
    ) -> Result<(String, u64), Problem> {
        self.refresh_with_idempotency_and_account_revision(
            raw_token,
            idempotency_key,
            now,
            expires_at,
            None,
        )
    }

    pub fn refresh_with_idempotency_and_account_revision(
        &mut self,
        raw_token: &str,
        idempotency_key: Option<&str>,
        now: u64,
        expires_at: u64,
        account_revision: Option<u64>,
    ) -> Result<(String, u64), Problem> {
        validate_refresh_idempotency_key(idempotency_key)?;
        if expires_at <= now {
            return Err(Problem::new(
                "invalid_session_expiry",
                "Session expiry must be in the future",
                400,
                "identity",
            ));
        }

        let hash = sha256(raw_token.as_bytes());
        let stored = self.load_stored(&hash)?.ok_or_else(invalid_session)?;
        if stored.record.revoked_at.is_some() {
            return self.replay_or_revoke_session(&stored.record, raw_token, idempotency_key, now);
        }
        self.verify_session(&stored.record, raw_token, now)?;
        let mut issued = refresh_session(&stored.record, expires_at)?;
        let replacement_record = &mut issued.record;
        replacement_record.family_id = stored.record.family_id.clone();
        replacement_record.csrf_hash = stored.record.csrf_hash;
        replacement_record.created_at = stored.record.created_at;
        replacement_record.last_seen_at = Some(now);
        let mut revoked_record = stored.record;
        revoked_record.revoked_at = Some(now);
        if let Some(key) = idempotency_key {
            let payload = RefreshReplayPayload {
                access_token: issued.raw_token.clone(),
                refresh_token: None,
                expires_at,
            };
            revoked_record.refresh_replay = Some(seal_refresh_replay(
                raw_token, &hash, key, "session", None, now, &payload,
            )?);
        }

        // The store rotates the token pair atomically, preserving the family binding.
        let (revoked_revision, replacement_revision) = match self.persistence.clone() {
            Some(persistence) => {
                match persistence.rotate_with_account_revision(
                    &revoked_record,
                    stored.revision,
                    &issued.record,
                    account_revision,
                ) {
                    Ok(revisions) => revisions,
                    Err(error) => {
                        if let Some((latest, _)) = persistence.get(&hash)?
                            && latest.revoked_at.is_some()
                        {
                            return self.replay_or_revoke_session(
                                &latest,
                                raw_token,
                                idempotency_key,
                                now,
                            );
                        }
                        return Err(error);
                    }
                }
            }
            None => (stored.revision, 0),
        };
        if self.persistence.is_none() {
            self.sessions.insert(
                hash,
                StoredSession {
                    record: revoked_record,
                    revision: revoked_revision,
                },
            );
            self.sessions.insert(
                issued.record.token_hash,
                StoredSession {
                    record: issued.record.clone(),
                    revision: replacement_revision,
                },
            );
        }
        Ok((std::mem::take(&mut issued.raw_token), expires_at))
    }

    pub fn refresh_account_id(&self, raw_token: &str) -> Result<AccountId, Problem> {
        let hash = sha256(raw_token.as_bytes());
        self.load_stored(&hash)?
            .map(|stored| stored.record.account_id)
            .ok_or_else(invalid_session)
    }

    fn replay_or_revoke_session(
        &mut self,
        record: &SessionRecord,
        raw_token: &str,
        idempotency_key: Option<&str>,
        now: u64,
    ) -> Result<(String, u64), Problem> {
        if let (Some(key), Some(replay)) = (idempotency_key, record.refresh_replay.as_ref()) {
            if replay.idempotency_key_hash == sha256(key.as_bytes()) {
                if replay.expires_at <= now {
                    return Err(refresh_replay_expired());
                }
                self.verify_replay_family(record)?;
                self.verify_replay_credential_version(record)?;
                let mut payload = open_refresh_replay(
                    raw_token,
                    &record.token_hash,
                    key,
                    "session",
                    None,
                    replay,
                )?;
                if payload.refresh_token.is_some() {
                    return Err(refresh_replay_corrupt());
                }
                return Ok((
                    std::mem::take(&mut payload.access_token),
                    payload.expires_at,
                ));
            }
        }
        if let Some(family_id) = record.family_id.as_deref() {
            match &self.persistence {
                Some(persistence) => {
                    persistence.revoke_family(family_id, &record.account_id, now)?
                }
                None => {
                    self.revoked_families.insert(family_id.to_owned(), now);
                }
            }
        }
        Err(session_revoked())
    }

    fn verify_replay_family(&self, record: &SessionRecord) -> Result<(), Problem> {
        let Some(family_id) = record.family_id.as_deref() else {
            return Ok(());
        };
        let revoked = match &self.persistence {
            Some(persistence) => matches!(
                persistence.family_revoked_at(family_id, &record.account_id)?,
                Some(Some(_))
            ),
            None => self.revoked_families.contains_key(family_id),
        };
        if revoked {
            Err(session_revoked())
        } else {
            Ok(())
        }
    }

    fn verify_replay_credential_version(&self, record: &SessionRecord) -> Result<(), Problem> {
        let Some(version) = record.credential_version else {
            return Ok(());
        };
        let credentials = self.credentials.as_ref().ok_or_else(|| {
            Problem::new(
                "session_authority_unavailable",
                "Session authority is unavailable",
                503,
                "identity",
            )
        })?;
        if credentials.password_version(&record.account_id)? == Some(version) {
            Ok(())
        } else {
            Err(session_revoked())
        }
    }

    pub fn authenticate(&self, raw_token: &str, now: u64) -> Result<AccountId, Problem> {
        self.authenticate_with_credential_version(raw_token, now)
            .map(|(account_id, _)| account_id)
    }

    pub fn authenticate_with_credential_version(
        &self,
        raw_token: &str,
        now: u64,
    ) -> Result<(AccountId, Option<u32>), Problem> {
        let hash = sha256(raw_token.as_bytes());
        let stored = self.load_stored(&hash)?.ok_or_else(invalid_session)?;
        self.verify_session(&stored.record, raw_token, now)?;
        Ok((stored.record.account_id, stored.record.credential_version))
    }

    pub fn authenticate_device(
        &self,
        raw_token: &str,
        now: u64,
    ) -> Result<(AccountId, String), Problem> {
        let hash = sha256(raw_token.as_bytes());
        let stored = self.load_stored(&hash)?.ok_or_else(invalid_session)?;
        self.verify_session(&stored.record, raw_token, now)?;
        let device_id = stored.record.device_id.clone().ok_or_else(|| {
            Problem::new(
                "device_session_required",
                "A connected device session is required",
                403,
                "identity",
            )
        })?;
        Ok((stored.record.account_id, device_id))
    }

    pub fn active_device_ids(
        &self,
        account_id: &AccountId,
        now: u64,
    ) -> Result<BTreeSet<String>, Problem> {
        if let Some(persistence) = &self.persistence {
            return persistence.active_device_ids(account_id, now);
        }
        Ok(self
            .device_refresh
            .values()
            .filter(|stored| {
                &stored.record.account_id == account_id
                    && stored.record.consumed_at.is_none()
                    && stored.record.expires_at > now
                    && !self.revoked_families.contains_key(&stored.record.family_id)
            })
            .map(|stored| stored.record.device_id.clone())
            .collect())
    }

    pub fn revoke(&mut self, raw_token: &str, now: u64) -> Result<(), Problem> {
        let hash = sha256(raw_token.as_bytes());
        self.revoke_with_idempotency(raw_token, &format!("auth-logout-{}", hex(&hash)), now)
    }

    pub fn revoke_with_idempotency(
        &mut self,
        raw_token: &str,
        idempotency_key: &str,
        now: u64,
    ) -> Result<(), Problem> {
        let hash = sha256(raw_token.as_bytes());
        let stored = self.load_stored(&hash)?.ok_or_else(invalid_session)?;
        self.revoke_session_with_idempotency(
            &stored.record.account_id,
            &hex(&hash),
            idempotency_key,
            now,
        )
    }

    pub fn revoke_browser_with_idempotency(
        &mut self,
        raw_session: &str,
        raw_csrf: &str,
        idempotency_key: &str,
        now: u64,
    ) -> Result<(), Problem> {
        let hash = sha256(raw_session.as_bytes());
        let stored = self.load_stored(&hash)?.ok_or_else(invalid_session)?;
        verify_csrf(&stored.record, raw_csrf)?;
        self.revoke_session_with_idempotency(
            &stored.record.account_id,
            &hex(&hash),
            idempotency_key,
            now,
        )
    }

    pub fn list_sessions(
        &self,
        account_id: &AccountId,
        current_raw_session: Option<&str>,
    ) -> Result<Vec<SessionSummary>, Problem> {
        let current_hash = current_raw_session.map(|value| sha256(value.as_bytes()));
        let records: Vec<StoredSession> = match &self.persistence {
            Some(persistence) => persistence
                .list_for_account(account_id)?
                .into_iter()
                .map(|(record, revision)| StoredSession { record, revision })
                .collect(),
            None => self
                .sessions
                .values()
                .filter(|stored| &stored.record.account_id == account_id)
                .cloned()
                .collect(),
        };
        let family_revocations = match &self.persistence {
            Some(persistence) => persistence.family_revocations(account_id)?,
            None => self.revoked_families.clone(),
        };
        let mut sessions_by_family = HashMap::<String, SessionSummary>::new();
        for stored in records
            .into_iter()
            .filter(|stored| &stored.record.account_id == account_id)
        {
            let family_revoked_at = stored
                .record
                .family_id
                .as_ref()
                .and_then(|family_id| family_revocations.get(family_id))
                .copied();
            let summary = SessionSummary {
                id: hex(&stored.record.token_hash),
                kind: if stored.record.device_id.is_some() {
                    SessionKind::Desktop
                } else if stored.record.csrf_hash.is_some() {
                    SessionKind::Browser
                } else {
                    SessionKind::Api
                },
                device_id: stored.record.device_id.clone(),
                created_at: stored.record.created_at,
                expires_at: stored.record.expires_at,
                revoked_at: stored.record.revoked_at.or(family_revoked_at),
                current: current_hash == Some(stored.record.token_hash),
            };
            // Refresh rotates the access token while keeping one session family.
            // Expose one row per login session, otherwise long-running clients
            // accumulate a visible row for every token refresh.
            let family_key = stored
                .record
                .family_id
                .unwrap_or_else(|| summary.id.clone());
            let replace = sessions_by_family.get(&family_key).is_none_or(|previous| {
                (summary.current, summary.expires_at) > (previous.current, previous.expires_at)
            });
            if replace {
                sessions_by_family.insert(family_key, summary);
            }
        }
        let mut sessions: Vec<_> = sessions_by_family.into_values().collect();
        sessions.sort_by_key(|session| (!session.current, std::cmp::Reverse(session.created_at)));
        Ok(sessions)
    }

    pub fn revoke_session(
        &mut self,
        account_id: &AccountId,
        session_id: &str,
        now: u64,
    ) -> Result<(), Problem> {
        self.revoke_session_with_idempotency(
            account_id,
            session_id,
            &format!("auth-session-revoke-{session_id}"),
            now,
        )
    }

    pub fn revoke_session_with_idempotency(
        &mut self,
        account_id: &AccountId,
        session_id: &str,
        idempotency_key: &str,
        now: u64,
    ) -> Result<(), Problem> {
        let hash = decode_hash(session_id)?;
        let stored = self
            .load_stored(&hash)?
            .filter(|stored| &stored.record.account_id == account_id)
            .ok_or_else(session_not_found)?;
        let mut record = stored.record;
        let was_active = record.revoked_at.is_none();
        if was_active {
            record.revoked_at = Some(now);
        }
        let revision = match &self.persistence {
            Some(persistence) => {
                persistence.revoke_session(&record, stored.revision, idempotency_key, was_active)?
            }
            None if was_active => {
                if let Some(family_id) = record.family_id.as_deref() {
                    self.revoked_families
                        .entry(family_id.to_owned())
                        .or_insert(now);
                }
                stored.revision
            }
            None => return Ok(()),
        };
        self.sessions
            .insert(hash, StoredSession { record, revision });
        Ok(())
    }

    pub(crate) fn mark_account_sessions_revoked(
        &mut self,
        account_id: &AccountId,
        now: u64,
    ) -> usize {
        let mut revoked = 0;
        for stored in self.sessions.values_mut() {
            if &stored.record.account_id == account_id && stored.record.revoked_at.is_none() {
                stored.record.revoked_at = Some(now);
                if let Some(family_id) = stored.record.family_id.as_ref() {
                    self.revoked_families
                        .entry(family_id.clone())
                        .or_insert(now);
                }
                revoked += 1;
            }
        }
        revoked
    }

    pub(crate) fn revoke_account_sessions(
        &mut self,
        account_id: &AccountId,
        now: u64,
    ) -> Result<usize, Problem> {
        let persisted = match &self.persistence {
            Some(persistence) => persistence.revoke_account_sessions(account_id, now)?,
            None => 0,
        };
        Ok(self
            .mark_account_sessions_revoked(account_id, now)
            .max(persisted))
    }

    pub(crate) fn account_session_revocation_puts(
        &self,
        account_id: &AccountId,
        now: u64,
    ) -> Result<Vec<ControlPut>, Problem> {
        match &self.persistence {
            Some(persistence) => persistence.account_session_revocation_puts(account_id, now),
            None => Ok(Vec::new()),
        }
    }

    fn store_issued(&mut self, record: SessionRecord) -> Result<(), Problem> {
        self.store_issued_with_account_revision(record, None)
    }

    fn store_issued_with_account_revision(
        &mut self,
        record: SessionRecord,
        account_revision: Option<u64>,
    ) -> Result<(), Problem> {
        match &self.persistence {
            Some(persistence) => {
                persistence.create_with_account_revision(&record, account_revision)?;
            }
            None => {
                self.sessions.insert(
                    record.token_hash,
                    StoredSession {
                        record,
                        revision: 0,
                    },
                );
            }
        }
        Ok(())
    }

    fn load_stored(&self, token_hash: &[u8; 32]) -> Result<Option<StoredSession>, Problem> {
        match &self.persistence {
            Some(persistence) => Ok(persistence
                .get(token_hash)?
                .map(|(record, revision)| StoredSession { record, revision })),
            None => Ok(self.sessions.get(token_hash).cloned()),
        }
    }

    fn load_device_refresh(
        &self,
        token_hash: &[u8; 32],
    ) -> Result<Option<StoredDeviceRefresh>, Problem> {
        match &self.persistence {
            Some(persistence) => Ok(persistence
                .get_device_refresh(token_hash)?
                .map(|(record, revision)| StoredDeviceRefresh { record, revision })),
            None => Ok(self.device_refresh.get(token_hash).cloned()),
        }
    }

    fn verify_refresh_family(
        &self,
        record: &DeviceRefreshRecord,
        _now: u64,
    ) -> Result<u64, Problem> {
        let revision = match &self.persistence {
            Some(persistence) => {
                let (revoked_at, revision) = persistence
                    .device_refresh_family_state(&record.family_id, &record.account_id)?;
                if revoked_at.is_some() {
                    return Err(session_revoked());
                }
                revision
            }
            None if self.revoked_families.contains_key(&record.family_id) => {
                return Err(session_revoked());
            }
            None => 0,
        };
        Ok(revision)
    }

    fn revoke_family(
        &mut self,
        family_id: &str,
        account_id: &AccountId,
        now: u64,
    ) -> Result<(), Problem> {
        match &self.persistence {
            Some(persistence) => persistence.revoke_family(family_id, account_id, now),
            None => {
                self.revoked_families
                    .entry(family_id.to_owned())
                    .or_insert(now);
                Ok(())
            }
        }
    }

    fn verify_session(
        &self,
        record: &SessionRecord,
        raw_token: &str,
        now: u64,
    ) -> Result<(), Problem> {
        verify(record, raw_token, now)?;
        if let Some(family_id) = record.family_id.as_deref() {
            let revoked = match &self.persistence {
                Some(persistence) => matches!(
                    persistence.family_revoked_at(family_id, &record.account_id)?,
                    Some(Some(_))
                ),
                None => self.revoked_families.contains_key(family_id),
            };
            if revoked {
                return Err(session_revoked());
            }
        }
        let Some(issued_version) = record.credential_version else {
            return Ok(());
        };
        let credentials = self.credentials.as_ref().ok_or_else(|| {
            Problem::new(
                "session_authority_unavailable",
                "Session authority is unavailable",
                503,
                "identity",
            )
        })?;
        if credentials.password_version(&record.account_id)? == Some(issued_version) {
            return Ok(());
        }
        Err(Problem::new(
            "session_revoked",
            "Session revoked",
            401,
            "identity",
        ))
    }
}

fn validate_refresh_idempotency_key(key: Option<&str>) -> Result<(), Problem> {
    if key.is_some_and(|key| !key.is_ascii() || !(16..=128).contains(&key.len())) {
        return Err(Problem::new(
            "invalid_idempotency_key",
            "Refresh idempotency key is invalid",
            400,
            "identity",
        ));
    }
    Ok(())
}

fn seal_refresh_replay(
    raw_token: &str,
    token_hash: &[u8; 32],
    idempotency_key: &str,
    flow: &str,
    device_id: Option<&str>,
    now: u64,
    payload: &RefreshReplayPayload,
) -> Result<SessionRefreshReplay, Problem> {
    let key_hash = sha256(idempotency_key.as_bytes());
    let key = refresh_replay_key(raw_token, token_hash)?;
    let plaintext =
        Zeroizing::new(serde_json::to_vec(payload).map_err(|_| refresh_replay_corrupt())?);
    let aad = refresh_replay_aad(flow, token_hash, &key_hash, device_id);
    let ciphertext = AuthenticatedChunk::encrypt(&key, plaintext.as_slice(), &aad)
        .map_err(|_| refresh_replay_crypto_unavailable())?
        .encoded();
    Ok(SessionRefreshReplay {
        idempotency_key_hash: key_hash,
        expires_at: now.checked_add(REFRESH_REPLAY_SECONDS).ok_or_else(|| {
            Problem::new(
                "clock_unavailable",
                "System clock cannot provide a valid refresh retry deadline",
                503,
                "identity",
            )
            .retryable(true)
        })?,
        ciphertext,
    })
}

fn open_refresh_replay(
    raw_token: &str,
    token_hash: &[u8; 32],
    idempotency_key: &str,
    flow: &str,
    device_id: Option<&str>,
    replay: &SessionRefreshReplay,
) -> Result<RefreshReplayPayload, Problem> {
    let key_hash = sha256(idempotency_key.as_bytes());
    if replay.idempotency_key_hash != key_hash {
        return Err(session_revoked());
    }
    let key = refresh_replay_key(raw_token, token_hash)?;
    let aad = refresh_replay_aad(flow, token_hash, &key_hash, device_id);
    let chunk =
        AuthenticatedChunk::decode(&replay.ciphertext).map_err(|_| refresh_replay_corrupt())?;
    let plaintext = Zeroizing::new(
        chunk
            .decrypt(&key, &aad)
            .map_err(|_| refresh_replay_corrupt())?,
    );
    serde_json::from_slice(plaintext.as_slice()).map_err(|_| refresh_replay_corrupt())
}

fn refresh_replay_key(raw_token: &str, token_hash: &[u8; 32]) -> Result<ClientKey, Problem> {
    let hkdf = Hkdf::<Sha256>::new(Some(token_hash), raw_token.as_bytes());
    let mut key_bytes = [0_u8; 32];
    hkdf.expand(REFRESH_REPLAY_KDF_INFO, &mut key_bytes)
        .map_err(|_| refresh_replay_crypto_unavailable())?;
    let key = ClientKey::from_bytes(key_bytes);
    key_bytes.zeroize();
    Ok(key)
}

fn refresh_replay_aad(
    flow: &str,
    token_hash: &[u8; 32],
    idempotency_key_hash: &[u8; 32],
    device_id: Option<&str>,
) -> Vec<u8> {
    format!(
        "{REFRESH_REPLAY_AAD_PREFIX}/{flow}/{}/{}/{}",
        hex(token_hash),
        hex(idempotency_key_hash),
        device_id.unwrap_or("-")
    )
    .into_bytes()
}

fn sha256(value: &[u8]) -> [u8; 32] {
    Sha256::digest(value).into()
}

fn random_refresh_token() -> Result<String, Problem> {
    let mut bytes = [0_u8; 32];
    if fill(&mut bytes).is_err() {
        bytes.zeroize();
        return Err(Problem::new(
            "session_entropy_unavailable",
            "Session unavailable",
            503,
            "identity",
        ));
    }
    let token = hex(&bytes);
    bytes.zeroize();
    Ok(token)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn decode_hash(value: &str) -> Result<[u8; 32], Problem> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(session_not_found());
    }
    let mut decoded = [0u8; 32];
    for (index, chunk) in value.as_bytes().chunks_exact(2).enumerate() {
        decoded[index] = u8::from_str_radix(std::str::from_utf8(chunk).unwrap_or(""), 16)
            .map_err(|_| session_not_found())?;
    }
    Ok(decoded)
}

fn session_not_found() -> Problem {
    Problem::new(
        "session_not_found",
        "Session was not found",
        404,
        "identity",
    )
}

fn invalid_session() -> Problem {
    Problem::new("invalid_session", "Invalid session", 401, "identity")
}

fn session_expired() -> Problem {
    Problem::new("session_expired", "Session expired", 401, "identity")
}

fn revision_exhausted() -> Problem {
    Problem::new("revision_exhausted", "Revision exhausted", 409, "identity")
}

fn session_refresh_conflict() -> Problem {
    Problem::new(
        "session_refresh_conflict",
        "Session state changed during refresh; retry with the same request key",
        409,
        "identity",
    )
    .retryable(true)
}

fn invalid_device_session_expiry() -> Problem {
    Problem::new(
        "invalid_session_expiry",
        "Device session expiry is invalid",
        400,
        "identity",
    )
}

fn session_revoked() -> Problem {
    Problem::new("session_revoked", "Session revoked", 401, "identity")
}

fn refresh_replay_expired() -> Problem {
    Problem::new(
        "refresh_replay_expired",
        "The refresh retry window has expired; authorize this device again",
        409,
        "identity",
    )
}

fn refresh_replay_corrupt() -> Problem {
    Problem::new(
        "refresh_replay_corrupt",
        "Stored refresh retry response is invalid",
        500,
        "identity",
    )
}

fn refresh_replay_crypto_unavailable() -> Problem {
    Problem::new(
        "refresh_replay_crypto_unavailable",
        "Refresh retry protection is unavailable",
        503,
        "identity",
    )
    .retryable(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::control_persistence::ControlPersistence;
    use buglogin_storage::control::MemoryControlRecordStore;

    #[test]
    fn durable_refresh_cas_conflict_is_retryable() {
        let error = session_refresh_conflict();
        assert_eq!(error.code, "session_refresh_conflict");
        assert_eq!(error.status, 409);
        assert!(error.retryable);
    }

    #[test]
    fn persistent_auth_does_not_retain_session_or_refresh_rows_in_memory() {
        let persistence =
            SessionPersistence::new(ControlPersistence::new(MemoryControlRecordStore::default()));
        let account_id = AccountId::new("acct_persistent_cache").unwrap();
        let mut auth = AuthService::persistent(persistence).unwrap();

        let browser_token = auth.issue(account_id.clone(), 100).unwrap();
        assert!(auth.sessions.is_empty());
        let replacement = auth.refresh(&browser_token, 50, 200).unwrap();
        assert!(auth.sessions.is_empty());
        assert_eq!(auth.authenticate(&replacement, 60).unwrap(), account_id);

        let device_tokens = auth
            .issue_device_session(account_id, 300, 1_000, "desktop-device-01".into(), None)
            .unwrap();
        assert!(auth.sessions.is_empty());
        assert!(auth.device_refresh.is_empty());
        auth.refresh_device_session(&device_tokens.refresh_token, "desktop-device-01", 100, 400)
            .unwrap();
        assert!(auth.sessions.is_empty());
        assert!(auth.device_refresh.is_empty());
    }

    #[test]
    fn session_survives_restart_and_revocation_survives_second_restart() {
        let control = ControlPersistence::new(MemoryControlRecordStore::default());
        let persistence = SessionPersistence::new(control);
        let account_id = AccountId::new("acct_restart").unwrap();
        let token = AuthService::persistent(persistence.clone())
            .unwrap()
            .issue(account_id.clone(), 100)
            .unwrap();

        let mut reopened = AuthService::persistent(persistence.clone()).unwrap();
        assert_eq!(reopened.authenticate(&token, 50).unwrap(), account_id);
        reopened.revoke(&token, 60).unwrap();

        let reopened = AuthService::persistent(persistence).unwrap();
        assert_eq!(
            reopened.authenticate(&token, 70).unwrap_err().code,
            "session_revoked"
        );
    }

    #[test]
    fn account_session_revocation_survives_restart() {
        let persistence =
            SessionPersistence::new(ControlPersistence::new(MemoryControlRecordStore::default()));
        let account_id = AccountId::new("acct_revoke_all_sessions").unwrap();
        let mut issuer = AuthService::persistent(persistence.clone()).unwrap();
        let issued = issuer.issue_browser(account_id.clone(), 100, 1).unwrap();

        assert_eq!(issuer.revoke_account_sessions(&account_id, 20).unwrap(), 1);
        let reopened = AuthService::persistent(persistence.clone()).unwrap();
        assert_eq!(
            reopened
                .authenticate_browser(&issued.raw_session, &issued.raw_csrf, 30)
                .unwrap_err()
                .code,
            "session_revoked"
        );
        assert_eq!(
            reopened.list_sessions(&account_id, None).unwrap()[0].revoked_at,
            Some(20)
        );
    }

    #[test]
    fn persistent_auth_instances_observe_new_and_revoked_sessions() {
        let persistence =
            SessionPersistence::new(ControlPersistence::new(MemoryControlRecordStore::default()));
        let account_id = AccountId::new("acct_shared_sessions").unwrap();
        let mut issuer = AuthService::persistent(persistence.clone()).unwrap();
        let verifier = AuthService::persistent(persistence).unwrap();

        let token = issuer.issue(account_id.clone(), 100).unwrap();
        assert_eq!(verifier.authenticate(&token, 50).unwrap(), account_id);
        let sessions = verifier.list_sessions(&account_id, Some(&token)).unwrap();
        assert_eq!(sessions.len(), 1);
        assert!(sessions[0].current);
        assert_eq!(sessions[0].revoked_at, None);

        issuer.revoke(&token, 60).unwrap();
        assert_eq!(
            verifier.authenticate(&token, 70).unwrap_err().code,
            "session_revoked"
        );
        assert_eq!(
            verifier.list_sessions(&account_id, Some(&token)).unwrap()[0].revoked_at,
            Some(60)
        );
    }

    #[test]
    fn refresh_rotates_token_and_persists_revocation() {
        let control = ControlPersistence::new(MemoryControlRecordStore::default());
        let persistence = SessionPersistence::new(control);
        let account_id = AccountId::new("acct_refresh").unwrap();
        let mut auth = AuthService::persistent(persistence.clone()).unwrap();
        let token = auth.issue(account_id.clone(), 100).unwrap();

        let replacement = auth.refresh(&token, 50, 200).unwrap();
        assert_ne!(replacement, token);
        assert_eq!(
            auth.authenticate(&token, 50).unwrap_err().code,
            "session_revoked"
        );
        assert_eq!(auth.authenticate(&replacement, 50).unwrap(), account_id);
        let persisted = serde_json::to_string(&persistence.list().unwrap()).unwrap();
        assert!(!persisted.contains(&token));
        assert!(!persisted.contains(&replacement));

        let reopened = AuthService::persistent(persistence).unwrap();
        assert_eq!(
            reopened.authenticate(&token, 50).unwrap_err().code,
            "session_revoked"
        );
        assert_eq!(reopened.authenticate(&replacement, 50).unwrap(), account_id);
    }

    #[test]
    fn refresh_token_reuse_revokes_the_persisted_session_family() {
        let persistence =
            SessionPersistence::new(ControlPersistence::new(MemoryControlRecordStore::default()));
        let account_id = AccountId::new("acct_refresh_reuse").unwrap();
        let mut issuer = AuthService::persistent(persistence.clone()).unwrap();
        let original = issuer.issue(account_id, 500).unwrap();
        let replacement = issuer.refresh(&original, 100, 600).unwrap();

        let mut verifier = AuthService::persistent(persistence).unwrap();
        assert_eq!(
            verifier.refresh(&original, 150, 700).unwrap_err().code,
            "session_revoked"
        );
        assert_eq!(
            verifier.authenticate(&replacement, 160).unwrap_err().code,
            "session_revoked"
        );
        let listed = verifier
            .list_sessions(&AccountId::new("acct_refresh_reuse").unwrap(), None)
            .unwrap();
        assert_eq!(listed.len(), 1);
        assert!(listed[0].revoked_at.is_some());
    }

    #[test]
    fn refresh_rejects_invalid_expiry_without_revoking_current_token() {
        let mut auth = AuthService::default();
        let account_id = AccountId::new("acct_refresh_expiry").unwrap();
        let token = auth.issue(account_id.clone(), 100).unwrap();

        assert_eq!(
            auth.refresh(&token, 50, 50).unwrap_err().code,
            "invalid_session_expiry"
        );
        assert_eq!(auth.authenticate(&token, 50).unwrap(), account_id);
    }

    #[test]
    fn refresh_rejects_expired_and_unknown_tokens_as_unauthorized() {
        let mut auth = AuthService::default();
        let token = auth
            .issue(AccountId::new("acct_refresh_rejected").unwrap(), 100)
            .unwrap();

        assert_eq!(
            auth.refresh(&token, 100, 200).unwrap_err().code,
            "session_expired"
        );
        assert_eq!(
            auth.refresh("unknown", 50, 200).unwrap_err().code,
            "invalid_session"
        );
    }

    #[test]
    fn device_refresh_survives_access_expiry_rotates_atomically_and_revokes_replay() {
        let persistence =
            SessionPersistence::new(ControlPersistence::new(MemoryControlRecordStore::default()));
        let account_id = AccountId::new("acct_device_refresh").unwrap();
        let mut issuer = AuthService::persistent(persistence.clone()).unwrap();
        let initial = issuer
            .issue_device_session(account_id.clone(), 100, 500, "device-windows".into(), None)
            .unwrap();
        assert_eq!(
            issuer
                .authenticate(&initial.access_token, 100)
                .unwrap_err()
                .code,
            "session_expired"
        );

        let replacement = issuer
            .refresh_device_session(&initial.refresh_token, "device-windows", 101, 200)
            .unwrap();
        assert_eq!(
            issuer
                .authenticate(&initial.access_token, 101)
                .unwrap_err()
                .code,
            "session_revoked"
        );
        assert_eq!(
            issuer.authenticate(&replacement.access_token, 102).unwrap(),
            account_id
        );

        let serialized = serde_json::to_string(&persistence.list().unwrap()).unwrap();
        assert!(!serialized.contains(&initial.access_token));
        assert!(!serialized.contains(&initial.refresh_token));
        let refresh_record = persistence
            .get_device_refresh(&sha256(replacement.refresh_token.as_bytes()))
            .unwrap()
            .unwrap()
            .0;
        assert!(
            !serde_json::to_string(&refresh_record)
                .unwrap()
                .contains(&replacement.refresh_token)
        );

        let mut reopened = AuthService::persistent(persistence).unwrap();
        assert_eq!(
            reopened
                .refresh_device_session(&initial.refresh_token, "device-windows", 103, 300)
                .unwrap_err()
                .code,
            "session_revoked"
        );
        assert_eq!(
            reopened
                .authenticate(&replacement.access_token, 104)
                .unwrap_err()
                .code,
            "session_revoked"
        );
    }

    #[test]
    fn device_refresh_is_bound_to_device_and_absolute_expiry() {
        let mut auth = AuthService::default();
        let pair = auth
            .issue_device_session(
                AccountId::new("acct_device_binding").unwrap(),
                100,
                200,
                "device-one".into(),
                None,
            )
            .unwrap();
        assert_eq!(
            auth.refresh_device_session(&pair.refresh_token, "device-two", 101, 200)
                .unwrap_err()
                .code,
            "session_revoked"
        );
        assert_eq!(
            auth.refresh_device_session(&pair.refresh_token, "device-one", 200, 300)
                .unwrap_err()
                .code,
            "session_expired"
        );
    }

    #[test]
    fn device_refresh_is_invalidated_by_logout_and_owner_session_revoke() {
        let mut auth = AuthService::default();
        let account_id = AccountId::new("acct_device_refresh_revoke").unwrap();
        let logout_pair = auth
            .issue_device_session(account_id.clone(), 500, 2_000, "device-logout".into(), None)
            .unwrap();
        auth.revoke(&logout_pair.access_token, 100).unwrap();
        assert_eq!(
            auth.refresh_device_session(&logout_pair.refresh_token, "device-logout", 101, 600)
                .unwrap_err()
                .code,
            "session_revoked"
        );

        let managed_pair = auth
            .issue_device_session(
                account_id.clone(),
                500,
                2_000,
                "device-managed".into(),
                None,
            )
            .unwrap();
        let session_id = auth
            .list_sessions(&account_id, Some(&managed_pair.access_token))
            .unwrap()
            .into_iter()
            .find(|session| session.current)
            .unwrap()
            .id;
        auth.revoke_session(&account_id, &session_id, 102).unwrap();
        assert_eq!(
            auth.refresh_device_session(&managed_pair.refresh_token, "device-managed", 103, 600)
                .unwrap_err()
                .code,
            "session_revoked"
        );
    }

    #[test]
    fn lists_safe_session_metadata_and_owner_scoped_revoke_survives_restart() {
        let control = ControlPersistence::new(MemoryControlRecordStore::default());
        let persistence = SessionPersistence::new(control);
        let account_id = AccountId::new("acct_sessions").unwrap();
        let other_id = AccountId::new("acct_other").unwrap();
        let mut auth = AuthService::persistent(persistence.clone()).unwrap();
        let browser = auth.issue_browser(account_id.clone(), 200, 100).unwrap();
        let desktop = auth
            .issue_for_device_at(account_id.clone(), 300, "device-win".into(), 101)
            .unwrap();
        let other = auth.issue(other_id.clone(), 300).unwrap();

        let sessions = auth
            .list_sessions(&account_id, Some(&browser.raw_session))
            .unwrap();
        assert_eq!(sessions.len(), 2);
        assert!(sessions[0].current);
        assert_eq!(sessions[0].kind, SessionKind::Browser);
        let desktop_session = sessions
            .iter()
            .find(|session| session.kind == SessionKind::Desktop)
            .unwrap();
        assert_eq!(desktop_session.device_id.as_deref(), Some("device-win"));
        assert!(!serde_json::to_string(&sessions).unwrap().contains(&desktop));

        assert_eq!(
            auth.revoke_session(&other_id, &desktop_session.id, 120)
                .unwrap_err()
                .code,
            "session_not_found"
        );
        auth.revoke_session(&account_id, &desktop_session.id, 120)
            .unwrap();
        assert_eq!(
            auth.authenticate(&desktop, 121).unwrap_err().code,
            "session_revoked"
        );
        assert_eq!(auth.authenticate(&other, 121).unwrap(), other_id);

        let reopened = AuthService::persistent(persistence).unwrap();
        assert_eq!(
            reopened.authenticate(&desktop, 121).unwrap_err().code,
            "session_revoked"
        );
    }

    #[test]
    fn session_list_collapses_access_token_rotations_into_one_login_session() {
        let persistence =
            SessionPersistence::new(ControlPersistence::new(MemoryControlRecordStore::default()));
        let account_id = AccountId::new("acct_session_rotation_list").unwrap();
        let mut auth = AuthService::persistent(persistence).unwrap();
        let first = auth
            .issue_device_session(account_id.clone(), 100, 1_000, "device-win".into(), None)
            .unwrap();
        let second = auth
            .refresh_device_session(&first.refresh_token, "device-win", 101, 200)
            .unwrap();
        let third = auth
            .refresh_device_session(&second.refresh_token, "device-win", 201, 300)
            .unwrap();

        let sessions = auth
            .list_sessions(&account_id, Some(&third.access_token))
            .unwrap();
        assert_eq!(sessions.len(), 1);
        assert!(sessions[0].current);
        assert_eq!(sessions[0].kind, SessionKind::Desktop);
        assert_eq!(sessions[0].device_id.as_deref(), Some("device-win"));

        auth.revoke_session(&account_id, &sessions[0].id, 301)
            .unwrap();
        assert_eq!(
            auth.authenticate(&third.access_token, 302)
                .unwrap_err()
                .code,
            "session_revoked"
        );
    }
}
