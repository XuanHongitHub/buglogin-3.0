use std::collections::HashMap;

use buglogin_domain::{
    errors::Problem,
    identity::{
        IssuedBrowserSession, IssuedSession, SessionRecord, issue, issue_browser, issue_for_device,
        refresh as refresh_session, verify, verify_csrf,
    },
    ids::AccountId,
};
use serde::Serialize;

use crate::services::{credentials::CredentialService, session_persistence::SessionPersistence};

#[derive(Clone)]
struct StoredSession {
    record: SessionRecord,
    revision: u64,
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
    persistence: Option<SessionPersistence>,
    credentials: Option<CredentialService>,
}

impl AuthService {
    pub fn persistent(persistence: SessionPersistence) -> Result<Self, Problem> {
        Ok(Self {
            sessions: HashMap::new(),
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

    pub fn issue(&mut self, account_id: AccountId, expires_at: u64) -> Result<String, Problem> {
        let IssuedSession { raw_token, record } = issue(account_id, expires_at)?;
        self.store_issued(record)?;
        Ok(raw_token)
    }

    pub fn issue_with_credential_version(
        &mut self,
        account_id: AccountId,
        expires_at: u64,
        credential_version: u32,
    ) -> Result<String, Problem> {
        let IssuedSession {
            raw_token,
            mut record,
        } = issue(account_id, expires_at)?;
        record.credential_version = Some(credential_version);
        self.store_issued(record)?;
        Ok(raw_token)
    }

    pub fn issue_for_device(
        &mut self,
        account_id: AccountId,
        expires_at: u64,
        device_id: String,
    ) -> Result<String, Problem> {
        let IssuedSession { raw_token, record } =
            issue_for_device(account_id, expires_at, Some(device_id))?;
        self.store_issued(record)?;
        Ok(raw_token)
    }

    pub fn issue_for_device_with_credential_version(
        &mut self,
        account_id: AccountId,
        expires_at: u64,
        device_id: String,
        credential_version: u32,
    ) -> Result<String, Problem> {
        let IssuedSession {
            raw_token,
            mut record,
        } = issue_for_device(account_id, expires_at, Some(device_id))?;
        record.credential_version = Some(credential_version);
        self.store_issued(record)?;
        Ok(raw_token)
    }

    pub fn issue_for_device_at(
        &mut self,
        account_id: AccountId,
        expires_at: u64,
        device_id: String,
        created_at: u64,
    ) -> Result<String, Problem> {
        let IssuedSession {
            raw_token,
            mut record,
        } = issue_for_device(account_id, expires_at, Some(device_id))?;
        record.created_at = Some(created_at);
        record.last_seen_at = Some(created_at);
        self.store_issued(record)?;
        Ok(raw_token)
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
        self.verify_session(&stored.record, raw_token, now)?;
        let IssuedSession {
            raw_token: replacement,
            record: replacement_record,
        } = refresh_session(&stored.record, expires_at)?;
        let mut revoked_record = stored.record;
        revoked_record.revoked_at = Some(now);

        // ponytail: single-session rotation; add session-family/device binding with its schema.
        let (revoked_revision, replacement_revision) = match &self.persistence {
            Some(persistence) => {
                persistence.rotate(&revoked_record, stored.revision, &replacement_record)?
            }
            None => (stored.revision, 0),
        };
        self.sessions.insert(
            hash,
            StoredSession {
                record: revoked_record,
                revision: revoked_revision,
            },
        );
        self.sessions.insert(
            replacement_record.token_hash,
            StoredSession {
                record: replacement_record,
                revision: replacement_revision,
            },
        );
        Ok(replacement)
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

    pub fn revoke(&mut self, raw_token: &str, now: u64) -> Result<(), Problem> {
        let hash = sha256(raw_token.as_bytes());
        let stored = self.load_stored(&hash)?.ok_or_else(invalid_session)?;
        if stored.record.revoked_at.is_some() {
            return Ok(());
        }
        let mut record = stored.record;
        record.revoked_at = Some(now);
        let revision = match &self.persistence {
            Some(persistence) => persistence.revoke(&record, stored.revision)?,
            None => stored.revision,
        };
        self.sessions
            .insert(hash, StoredSession { record, revision });
        Ok(())
    }

    pub fn list_sessions(
        &self,
        account_id: &AccountId,
        current_raw_session: Option<&str>,
    ) -> Result<Vec<SessionSummary>, Problem> {
        let current_hash = current_raw_session.map(|value| sha256(value.as_bytes()));
        let records: Vec<StoredSession> = match &self.persistence {
            Some(persistence) => persistence
                .list()?
                .into_iter()
                .map(|(record, revision)| StoredSession { record, revision })
                .collect(),
            None => self.sessions.values().cloned().collect(),
        };
        let mut sessions: Vec<_> = records
            .into_iter()
            .filter(|stored| &stored.record.account_id == account_id)
            .map(|stored| SessionSummary {
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
                revoked_at: stored.record.revoked_at,
                current: current_hash == Some(stored.record.token_hash),
            })
            .collect();
        sessions.sort_by_key(|session| (!session.current, std::cmp::Reverse(session.created_at)));
        Ok(sessions)
    }

    pub fn revoke_session(
        &mut self,
        account_id: &AccountId,
        session_id: &str,
        now: u64,
    ) -> Result<(), Problem> {
        let hash = decode_hash(session_id)?;
        let stored = self
            .load_stored(&hash)?
            .filter(|stored| &stored.record.account_id == account_id)
            .ok_or_else(session_not_found)?;
        if stored.record.revoked_at.is_some() {
            return Ok(());
        }
        let mut record = stored.record;
        record.revoked_at = Some(now);
        let revision = match &self.persistence {
            Some(persistence) => persistence.revoke(&record, stored.revision)?,
            None => stored.revision,
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
                revoked += 1;
            }
        }
        revoked
    }

    fn store_issued(&mut self, record: SessionRecord) -> Result<(), Problem> {
        let revision = match &self.persistence {
            Some(persistence) => persistence.create(&record)?,
            None => 0,
        };
        self.sessions
            .insert(record.token_hash, StoredSession { record, revision });
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

    fn verify_session(
        &self,
        record: &SessionRecord,
        raw_token: &str,
        now: u64,
    ) -> Result<(), Problem> {
        verify(record, raw_token, now)?;
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

fn sha256(value: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    Sha256::digest(value).into()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::control_persistence::ControlPersistence;
    use buglogin_storage::control::MemoryControlRecordStore;

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
}
