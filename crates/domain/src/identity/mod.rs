use getrandom::fill;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use zeroize::Zeroize;

use crate::{
    errors::Problem,
    ids::{AccountId, DeviceId},
};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRecord {
    pub account_id: AccountId,
    pub token_hash: [u8; 32],
    pub expires_at: u64,
    pub revoked_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub csrf_hash: Option<[u8; 32]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_version: Option<u32>,
    /// Short-lived, client-keyed encrypted response for an idempotent refresh
    /// retry. The ciphertext is not useful without possession of the old token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_replay: Option<SessionRefreshReplay>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionRefreshReplay {
    pub idempotency_key_hash: [u8; 32],
    pub expires_at: u64,
    pub ciphertext: Vec<u8>,
}

pub struct IssuedBrowserSession {
    pub raw_session: String,
    pub raw_csrf: String,
    pub record: SessionRecord,
}

impl std::fmt::Debug for IssuedBrowserSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IssuedBrowserSession")
            .field("raw_session", &"<redacted>")
            .field("raw_csrf", &"<redacted>")
            .field("record", &self.record)
            .finish()
    }
}

impl Drop for IssuedBrowserSession {
    fn drop(&mut self) {
        self.raw_session.zeroize();
        self.raw_csrf.zeroize();
    }
}

pub struct IssuedSession {
    pub raw_token: String,
    pub record: SessionRecord,
}
impl std::fmt::Debug for IssuedSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IssuedSession")
            .field("raw_token", &"<redacted>")
            .field("record", &self.record)
            .finish()
    }
}

impl Drop for IssuedSession {
    fn drop(&mut self) {
        self.raw_token.zeroize();
    }
}

pub fn issue(account_id: AccountId, expires_at: u64) -> Result<IssuedSession, Problem> {
    issue_for_device(account_id, expires_at, None)
}

pub fn issue_for_device(
    account_id: AccountId,
    expires_at: u64,
    device_id: Option<String>,
) -> Result<IssuedSession, Problem> {
    if device_id
        .as_deref()
        .is_some_and(|value| DeviceId::new(value).is_err())
    {
        return Err(Problem::new(
            "device_id_invalid",
            "Device ID is invalid",
            400,
            "identity",
        ));
    }
    let mut bytes = [0u8; 32];
    if fill(&mut bytes).is_err() {
        bytes.zeroize();
        return Err(Problem::new(
            "session_entropy_unavailable",
            "Session unavailable",
            503,
            "identity",
        ));
    }
    let raw_token = hex(&bytes);
    bytes.zeroize();
    Ok(IssuedSession {
        record: SessionRecord {
            account_id,
            token_hash: hash(raw_token.as_bytes()),
            expires_at,
            revoked_at: None,
            device_id,
            family_id: Some(random_hex()?),
            csrf_hash: None,
            created_at: None,
            last_seen_at: None,
            credential_version: None,
            refresh_replay: None,
        },
        raw_token,
    })
}

pub fn issue_browser(
    account_id: AccountId,
    expires_at: u64,
    created_at: u64,
) -> Result<IssuedBrowserSession, Problem> {
    let raw_session = random_hex()?;
    let raw_csrf = random_hex()?;
    let family_id = random_hex()?;
    Ok(IssuedBrowserSession {
        record: SessionRecord {
            account_id,
            token_hash: hash(raw_session.as_bytes()),
            expires_at,
            revoked_at: None,
            device_id: None,
            family_id: Some(family_id),
            csrf_hash: Some(hash(raw_csrf.as_bytes())),
            created_at: Some(created_at),
            last_seen_at: Some(created_at),
            credential_version: None,
            refresh_replay: None,
        },
        raw_session,
        raw_csrf,
    })
}

pub fn refresh(record: &SessionRecord, expires_at: u64) -> Result<IssuedSession, Problem> {
    let mut issued = issue_for_device(
        record.account_id.clone(),
        expires_at,
        record.device_id.clone(),
    )?;
    issued.record.family_id = record.family_id.clone();
    issued.record.csrf_hash = record.csrf_hash;
    issued.record.created_at = record.created_at;
    issued.record.last_seen_at = record.last_seen_at;
    issued.record.credential_version = record.credential_version;
    Ok(issued)
}

pub fn verify(record: &SessionRecord, raw_token: &str, now: u64) -> Result<(), Problem> {
    if record.revoked_at.is_some() {
        return Err(Problem::new(
            "session_revoked",
            "Session revoked",
            401,
            "identity",
        ));
    }
    if now >= record.expires_at {
        return Err(Problem::new(
            "session_expired",
            "Session expired",
            401,
            "identity",
        ));
    }
    if !bool::from(record.token_hash.ct_eq(&hash(raw_token.as_bytes()))) {
        return Err(Problem::new(
            "invalid_session",
            "Invalid session",
            401,
            "identity",
        ));
    }
    Ok(())
}

fn hash(value: &[u8]) -> [u8; 32] {
    Sha256::digest(value).into()
}
pub fn verify_csrf(record: &SessionRecord, raw_csrf: &str) -> Result<(), Problem> {
    let Some(expected) = record.csrf_hash else {
        return Err(Problem::new(
            "csrf_required",
            "CSRF validation is required",
            403,
            "identity",
        ));
    };
    if !bool::from(expected.ct_eq(&hash(raw_csrf.as_bytes()))) {
        return Err(Problem::new(
            "csrf_invalid",
            "CSRF validation failed",
            403,
            "identity",
        ));
    }
    Ok(())
}

fn random_hex() -> Result<String, Problem> {
    let mut bytes = [0u8; 32];
    if fill(&mut bytes).is_err() {
        bytes.zeroize();
        return Err(Problem::new(
            "session_entropy_unavailable",
            "Session unavailable",
            503,
            "identity",
        ));
    }
    let value = hex(&bytes);
    bytes.zeroize();
    Ok(value)
}
fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        result.push(HEX[(byte >> 4) as usize] as char);
        result.push(HEX[(byte & 0x0f) as usize] as char);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn account() -> AccountId {
        AccountId::new("acct_device_session").unwrap()
    }

    #[test]
    fn legacy_session_json_defaults_optional_bindings() {
        let record: SessionRecord = serde_json::from_value(json!({
            "accountId": "acct_device_session",
            "tokenHash": vec![0u8; 32],
            "expiresAt": 100,
            "revokedAt": null
        }))
        .unwrap();

        assert_eq!(record.device_id, None);
        assert_eq!(record.family_id, None);
        assert_eq!(record.csrf_hash, None);
        assert_eq!(record.credential_version, None);
        assert!(
            serde_json::to_value(record)
                .unwrap()
                .get("deviceId")
                .is_none()
        );
    }

    #[test]
    fn issue_and_refresh_preserve_device_binding_without_serializing_raw_token() {
        let mut issued = issue_for_device(account(), 100, Some("device_windows_1".into())).unwrap();
        issued.record.credential_version = Some(4);
        let raw_token = issued.raw_token.clone();
        assert_eq!(issued.record.device_id.as_deref(), Some("device_windows_1"));
        assert!(issued.record.family_id.is_some());
        let serialized = serde_json::to_string(&issued.record).unwrap();
        assert!(serialized.contains("device_windows_1"));
        assert!(!serialized.contains(&raw_token));
        assert!(!format!("{issued:?}").contains(&raw_token));

        let refreshed = refresh(&issued.record, 200).unwrap();
        assert_eq!(refreshed.record.account_id, issued.record.account_id);
        assert_eq!(refreshed.record.device_id, issued.record.device_id);
        assert_eq!(refreshed.record.credential_version, Some(4));
        assert_eq!(refreshed.record.family_id, issued.record.family_id);
        assert_ne!(refreshed.raw_token, raw_token);
        assert!(
            !serde_json::to_string(&refreshed.record)
                .unwrap()
                .contains(&refreshed.raw_token)
        );
    }

    #[test]
    fn issue_for_device_rejects_noncanonical_device_ids() {
        assert_eq!(
            issue_for_device(account(), 100, Some("short".into()))
                .unwrap_err()
                .code,
            "device_id_invalid"
        );
    }

    #[test]
    fn issued_session_has_a_family_without_a_device_binding() {
        let issued = issue(account(), 100).unwrap();
        assert_eq!(issued.record.device_id, None);
        assert!(issued.record.family_id.is_some());
    }

    #[test]
    fn browser_issue_keeps_secrets_out_of_record_and_validates_csrf() {
        let issued = issue_browser(account(), 100, 50).unwrap();
        assert_ne!(issued.raw_session, issued.raw_csrf);
        assert!(verify_csrf(&issued.record, &issued.raw_csrf).is_ok());
        assert_eq!(
            verify_csrf(&issued.record, "wrong-csrf").unwrap_err().code,
            "csrf_invalid"
        );
        let serialized = serde_json::to_string(&issued.record).unwrap();
        assert!(!serialized.contains(&issued.raw_session));
        assert!(!serialized.contains(&issued.raw_csrf));
        assert!(!format!("{issued:?}").contains(&issued.raw_csrf));
    }
}
