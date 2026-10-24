use buglogin_domain::{
    errors::Problem,
    ids::{AccountId, ProfileId},
};
use buglogin_storage::control::{
    ControlAuditEntry, ControlDelete, ControlPut, ExecuteControlCommand, HttpResponse,
};
use getrandom::fill;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt::Write;
use zeroize::{Zeroize, Zeroizing};

use super::control_persistence::ControlPersistence;

const TENANT: &str = "profile-viewer";
const TICKET_TYPE: &str = "ticket";
const SESSION_TYPE: &str = "session";
const TICKET_TTL_SECONDS: u64 = 120;
pub const SESSION_TTL_SECONDS: u64 = 8 * 60 * 60;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ViewerTicketRecord {
    account_id: AccountId,
    profile_id: ProfileId,
    expires_at: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ViewerSessionRecord {
    account_id: AccountId,
    profile_id: ProfileId,
    expires_at: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IssuedProfileViewerTicket {
    pub ticket: String,
    pub expires_in: u64,
}

impl std::fmt::Debug for IssuedProfileViewerTicket {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("IssuedProfileViewerTicket")
            .field("ticket", &"[REDACTED]")
            .field("expires_in", &self.expires_in)
            .finish()
    }
}

impl Drop for IssuedProfileViewerTicket {
    fn drop(&mut self) {
        self.ticket.zeroize();
    }
}

#[derive(Clone)]
pub struct ProfileViewerService {
    persistence: ControlPersistence,
}

impl ProfileViewerService {
    pub fn new(persistence: ControlPersistence) -> Self {
        Self { persistence }
    }

    pub fn issue(
        &self,
        account_id: &AccountId,
        profile_id: &ProfileId,
        now: u64,
    ) -> Result<IssuedProfileViewerTicket, Problem> {
        self.purge_expired(now)?;
        let mut ticket = Zeroizing::new(random_token()?);
        let ticket_id = token_id(&ticket);
        let record = ViewerTicketRecord {
            account_id: account_id.clone(),
            profile_id: profile_id.clone(),
            expires_at: checked_add(now, TICKET_TTL_SECONDS)?,
        };
        self.persistence.execute(ExecuteControlCommand {
            tenant_id: TENANT.into(),
            command_id: "profile.viewer.ticket.issue".into(),
            idempotency_key: format!("profile.viewer.ticket.issue-{ticket_id}"),
            expected_revisions: Default::default(),
            puts: vec![ControlPut {
                tenant_id: TENANT.into(),
                record_type: TICKET_TYPE.into(),
                record_id: ticket_id,
                expected_revision: Some(0),
                value: serde_json::to_value(record).map_err(|_| corrupt())?,
            }],
            deletes: vec![],
            audit: audit(
                "profile.viewer.ticket.issue",
                account_id.as_str(),
                serde_json::json!({"profileId": profile_id, "expiresIn": TICKET_TTL_SECONDS}),
            ),
            response: response(),
        })?;
        Ok(IssuedProfileViewerTicket {
            ticket: std::mem::take(&mut *ticket),
            expires_in: TICKET_TTL_SECONDS,
        })
    }

    pub fn redeem(
        &self,
        ticket: &str,
        profile_id: &ProfileId,
        now: u64,
    ) -> Result<(Zeroizing<String>, AccountId), Problem> {
        if ticket.len() != 64 || !ticket.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(invalid_ticket());
        }
        let ticket_id = token_id(ticket);
        let (record, revision) = self
            .persistence
            .load_versioned::<ViewerTicketRecord>(TENANT, TICKET_TYPE, &ticket_id)?
            .ok_or_else(invalid_ticket)?;
        if record.expires_at <= now || &record.profile_id != profile_id {
            return Err(invalid_ticket());
        }
        let session_token = Zeroizing::new(random_token()?);
        let session_id = token_id(&session_token);
        let session_record = ViewerSessionRecord {
            account_id: record.account_id.clone(),
            profile_id: profile_id.clone(),
            expires_at: checked_add(now, SESSION_TTL_SECONDS)?,
        };
        self.persistence.execute(ExecuteControlCommand {
            tenant_id: TENANT.into(),
            command_id: "profile.viewer.ticket.redeem".into(),
            idempotency_key: format!("profile.viewer.ticket.redeem-{ticket_id}"),
            expected_revisions: Default::default(),
            puts: vec![ControlPut {
                tenant_id: TENANT.into(),
                record_type: SESSION_TYPE.into(),
                record_id: session_id,
                expected_revision: Some(0),
                value: serde_json::to_value(session_record).map_err(|_| corrupt())?,
            }],
            deletes: vec![ControlDelete {
                tenant_id: TENANT.into(),
                record_type: TICKET_TYPE.into(),
                record_id: ticket_id,
                expected_revision: Some(revision),
            }],
            audit: audit(
                "profile.viewer.ticket.redeem",
                record.account_id.as_str(),
                serde_json::json!({"profileId": profile_id, "state": "redeemed"}),
            ),
            response: response(),
        })?;
        Ok((session_token, record.account_id))
    }

    pub fn authenticate(
        &self,
        raw_session: &str,
        profile_id: &ProfileId,
        now: u64,
    ) -> Result<AccountId, Problem> {
        if raw_session.len() != 64 || !raw_session.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(invalid_ticket());
        }
        let session = self
            .persistence
            .load::<ViewerSessionRecord>(TENANT, SESSION_TYPE, &token_id(raw_session))?
            .filter(|session| session.expires_at > now && &session.profile_id == profile_id)
            .ok_or_else(invalid_ticket)?;
        Ok(session.account_id)
    }

    fn purge_expired(&self, now: u64) -> Result<(), Problem> {
        for record_type in [TICKET_TYPE, SESSION_TYPE] {
            let records = if record_type == TICKET_TYPE {
                self.persistence
                    .list_versioned::<ViewerTicketRecord>(TENANT, record_type)?
                    .into_iter()
                    .filter(|(_, record, _)| record.expires_at <= now)
                    .take(128)
                    .map(|(id, _, revision)| (id, revision))
                    .collect::<Vec<_>>()
            } else {
                self.persistence
                    .list_versioned::<ViewerSessionRecord>(TENANT, record_type)?
                    .into_iter()
                    .filter(|(_, record, _)| record.expires_at <= now)
                    .take(128)
                    .map(|(id, _, revision)| (id, revision))
                    .collect::<Vec<_>>()
            };
            if records.is_empty() {
                continue;
            }
            self.persistence.execute(ExecuteControlCommand {
                tenant_id: TENANT.into(),
                command_id: "profile.viewer.expire".into(),
                idempotency_key: format!("profile.viewer.expire-{record_type}-{}", records[0].0),
                expected_revisions: Default::default(),
                puts: vec![],
                deletes: records
                    .into_iter()
                    .map(|(record_id, revision)| ControlDelete {
                        tenant_id: TENANT.into(),
                        record_type: record_type.into(),
                        record_id,
                        expected_revision: Some(revision),
                    })
                    .collect(),
                audit: audit(
                    "profile.viewer.expire",
                    "system",
                    serde_json::json!({"recordType": record_type}),
                ),
                response: response(),
            })?;
        }
        Ok(())
    }
}

fn random_token() -> Result<String, Problem> {
    let mut bytes = [0_u8; 32];
    fill(&mut bytes).map_err(|_| unavailable())?;
    let mut output = String::with_capacity(64);
    for byte in bytes {
        write!(output, "{byte:02x}").map_err(|_| unavailable())?;
    }
    bytes.zeroize();
    Ok(output)
}

fn token_id(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    digest
        .iter()
        .fold(String::with_capacity(64), |mut output, byte| {
            let _ = write!(output, "{byte:02x}");
            output
        })
}

fn checked_add(now: u64, seconds: u64) -> Result<u64, Problem> {
    now.checked_add(seconds).ok_or_else(unavailable)
}

fn audit(command_id: &str, actor_id: &str, detail: serde_json::Value) -> ControlAuditEntry {
    ControlAuditEntry {
        tenant_id: TENANT.into(),
        command_id: command_id.into(),
        actor_id: actor_id.into(),
        allowed: true,
        detail,
    }
}

fn response() -> HttpResponse {
    HttpResponse {
        status: 200,
        headers: Default::default(),
        body: serde_json::Value::Null,
    }
}

fn invalid_ticket() -> Problem {
    Problem::new(
        "profile_viewer_ticket_invalid",
        "Profile viewer link is invalid or expired",
        401,
        "profile-viewer",
    )
}

fn corrupt() -> Problem {
    Problem::new(
        "profile_viewer_state_corrupt",
        "Profile viewer state is corrupt",
        500,
        "profile-viewer",
    )
}

fn unavailable() -> Problem {
    Problem::new(
        "profile_viewer_unavailable",
        "Profile viewer handoff is unavailable",
        503,
        "profile-viewer",
    )
    .retryable(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use buglogin_storage::control::MemoryControlRecordStore;

    fn service() -> ProfileViewerService {
        ProfileViewerService::new(ControlPersistence::new(MemoryControlRecordStore::default()))
    }

    #[test]
    fn ticket_is_single_use_profile_bound_and_expires() {
        let service = service();
        let account = AccountId::from_str("act_profile_viewer_test").unwrap();
        let profile = ProfileId::from_str("prf_profile_viewer_test").unwrap();
        let other = ProfileId::from_str("prf_profile_viewer_other").unwrap();
        let issued = service.issue(&account, &profile, 100).unwrap();

        assert!(service.redeem(&issued.ticket, &other, 101).is_err());
        let (session, redeemed_account) = service.redeem(&issued.ticket, &profile, 101).unwrap();
        assert_eq!(redeemed_account, account);
        assert_eq!(
            service.authenticate(&session, &profile, 102).unwrap(),
            account
        );
        assert!(service.authenticate(&session, &other, 102).is_err());
        assert!(service.redeem(&issued.ticket, &profile, 102).is_err());
        assert!(
            service
                .authenticate(&session, &profile, 101 + SESSION_TTL_SECONDS)
                .is_err()
        );
    }

    #[test]
    fn expired_ticket_cannot_be_redeemed() {
        let service = service();
        let account = AccountId::from_str("act_profile_viewer_expired").unwrap();
        let profile = ProfileId::from_str("prf_profile_viewer_expired").unwrap();
        let issued = service.issue(&account, &profile, 100).unwrap();
        assert!(
            service
                .redeem(&issued.ticket, &profile, 100 + TICKET_TTL_SECONDS)
                .is_err()
        );
    }
}
