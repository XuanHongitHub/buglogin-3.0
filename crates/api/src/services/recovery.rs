use std::sync::Arc;

use buglogin_domain::{
    account::{AccountIdentity, canonical_email, canonical_username},
    errors::Problem,
    ids::AccountId,
};
use buglogin_storage::control::{
    ControlAuditEntry, ControlPut, ExecuteControlCommand, HttpResponse,
};
use getrandom::fill;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, Zeroizing};

use super::{
    api_tokens::ApiTokenService, control_persistence::ControlPersistence,
    credentials::CredentialService, identity_persistence::IdentityPersistence,
    session_persistence::SessionPersistence,
};

const TENANT: &str = "identity";
const TOKEN_RECORD_TYPE: &str = "recovery_token";
const RATE_RECORD_TYPE: &str = "recovery_rate";
const TOKEN_SECONDS: u64 = 15 * 60;
const RATE_WINDOW_SECONDS: u64 = 15 * 60;
const MAX_REQUESTS_PER_WINDOW: u32 = 3;
const MAX_RATE_CAS_RETRIES: usize = 32;

#[derive(Clone, Eq, PartialEq)]
pub struct RecoveryDeliveryMessage {
    pub account_id: AccountId,
    pub destination: String,
    pub raw_token: String,
    pub expires_at: u64,
}

pub trait RecoveryDelivery: Send + Sync {
    fn available(&self) -> bool;
    fn deliver(&self, message: RecoveryDeliveryMessage) -> Result<(), Problem>;
}

#[derive(Default)]
pub struct UnavailableRecoveryDelivery;

impl RecoveryDelivery for UnavailableRecoveryDelivery {
    fn available(&self) -> bool {
        false
    }

    fn deliver(&self, _message: RecoveryDeliveryMessage) -> Result<(), Problem> {
        Err(delivery_unavailable())
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct RecoveryTokenRecord {
    account_id: AccountId,
    token_hash: [u8; 32],
    created_at: u64,
    expires_at: u64,
    consumed_at: Option<u64>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct RecoveryRateRecord {
    window_started_at: u64,
    attempts: u32,
}

#[derive(Clone)]
pub struct RecoveryService {
    store: ControlPersistence,
    identities: IdentityPersistence,
    credentials: CredentialService,
    api_tokens: ApiTokenService,
    sessions: SessionPersistence,
    delivery: Arc<dyn RecoveryDelivery>,
}

impl RecoveryService {
    pub fn new(store: ControlPersistence, credentials: CredentialService) -> Self {
        Self {
            identities: IdentityPersistence::new(store.clone()),
            api_tokens: ApiTokenService::new(store.clone(), credentials.clone()),
            sessions: SessionPersistence::new(store.clone()),
            store,
            credentials,
            delivery: Arc::new(UnavailableRecoveryDelivery),
        }
    }

    pub fn with_delivery(mut self, delivery: Arc<dyn RecoveryDelivery>) -> Self {
        self.delivery = delivery;
        self
    }

    pub fn request(&self, identifier: &str, now: u64) -> Result<(), Problem> {
        if !self.delivery.available() {
            return Err(delivery_unavailable());
        }
        let request_nonce = random_token()?;
        let identity = self.find_identity(identifier)?;
        let rate_subject = identity.as_ref().map_or_else(
            || format!("identifier:{}", identifier.trim().to_lowercase()),
            |identity| format!("account:{}", identity.account_id.as_str()),
        );
        let rate_id = hex(&sha256(rate_subject.as_bytes()));
        for _ in 0..MAX_RATE_CAS_RETRIES {
            let (rate, rate_revision) = self
                .store
                .load_versioned::<RecoveryRateRecord>(TENANT, RATE_RECORD_TYPE, &rate_id)?
                .unwrap_or((
                    RecoveryRateRecord {
                        window_started_at: now,
                        attempts: 0,
                    },
                    0,
                ));
            let mut next_rate = if now.saturating_sub(rate.window_started_at) >= RATE_WINDOW_SECONDS
            {
                RecoveryRateRecord {
                    window_started_at: now,
                    attempts: 0,
                }
            } else {
                rate
            };
            next_rate.attempts = next_rate.attempts.saturating_add(1);
            let suppressed = next_rate.attempts > MAX_REQUESTS_PER_WINDOW || identity.is_none();
            let mut puts = vec![ControlPut {
                tenant_id: TENANT.into(),
                record_type: RATE_RECORD_TYPE.into(),
                record_id: rate_id.clone(),
                expected_revision: Some(rate_revision),
                value: serde_json::to_value(&next_rate).map_err(|_| corrupt())?,
            }];
            let issued = if let Some(identity) = identity.clone().filter(|_| !suppressed) {
                let raw_token = Zeroizing::new(random_token()?);
                let token_hash = sha256(raw_token.as_bytes());
                let record = RecoveryTokenRecord {
                    account_id: identity.account_id.clone(),
                    token_hash,
                    created_at: now,
                    expires_at: crate::clock::checked_add_seconds(now, TOKEN_SECONDS, "recovery")?,
                    consumed_at: None,
                };
                puts.push(ControlPut {
                    tenant_id: TENANT.into(),
                    record_type: TOKEN_RECORD_TYPE.into(),
                    record_id: hex(&token_hash),
                    expected_revision: Some(0),
                    value: serde_json::to_value(&record).map_err(|_| corrupt())?,
                });
                Some((identity, record, raw_token))
            } else {
                None
            };
            let result = self.store.execute(ExecuteControlCommand {
                tenant_id: TENANT.into(),
                command_id: "auth.recovery.request".into(),
                idempotency_key: format!(
                    "recovery-request-{rate_id}-{}-{}",
                    &request_nonce[..32],
                    next_rate.attempts
                ),
                expected_revisions: Default::default(),
                puts,
                deletes: vec![],
                audit: ControlAuditEntry {
                    tenant_id: TENANT.into(),
                    command_id: "auth.recovery.request".into(),
                    actor_id: "anonymous".into(),
                    allowed: true,
                    detail: json!({"state": "accepted"}),
                },
                response: HttpResponse {
                    status: 202,
                    headers: Default::default(),
                    body: Value::Null,
                },
            });
            match result {
                Ok(_) => {
                    if let Some((identity, record, mut raw_token)) = issued
                        && let Err(error) = self.delivery.deliver(RecoveryDeliveryMessage {
                            account_id: identity.account_id,
                            destination: identity.email,
                            raw_token: std::mem::take(&mut *raw_token),
                            expires_at: record.expires_at,
                        })
                    {
                        self.invalidate(&record, now)?;
                        let _ = error;
                    }
                    return Ok(());
                }
                Err(error) if error.code == "storage_revision_conflict" => continue,
                Err(error) => return Err(error),
            }
        }
        Err(Problem::new(
            "recovery_rate_limit_busy",
            "Recovery request could not be recorded; retry shortly",
            503,
            "identity",
        )
        .retryable(true))
    }

    pub fn complete(
        &self,
        raw_token: &str,
        new_password: &str,
        idempotency_key: &str,
        now: u64,
    ) -> Result<AccountId, Problem> {
        if !self.delivery.available() {
            return Err(delivery_unavailable());
        }
        if !buglogin_domain::security::is_lower_hex_token_64(raw_token) {
            return Err(invalid_token());
        }
        let token_hash = sha256(raw_token.as_bytes());
        let token_id = hex(&token_hash);
        let command_key = format!("recovery-complete-{idempotency_key}-{token_id}");
        if let Some(replay) = self.store.replay(TENANT, &command_key)? {
            let account_id = replay.response.body["accountId"]
                .as_str()
                .ok_or_else(corrupt)?;
            return AccountId::new(account_id.to_owned()).map_err(|_| corrupt());
        }
        let Some((record, _revision)) = self.store.load_versioned::<RecoveryTokenRecord>(
            TENANT,
            TOKEN_RECORD_TYPE,
            &token_id,
        )?
        else {
            return Err(invalid_token());
        };
        if record.token_hash != token_hash
            || record.consumed_at.is_some()
            || now >= record.expires_at
        {
            return Err(invalid_token());
        }
        let password_put =
            self.credentials
                .password_reset_put(&record.account_id, new_password, now)?;
        let mut recovery_token_puts = Vec::new();
        let mut current_token_found = false;
        let pending_tokens = self
            .store
            .list_versioned_for_account::<RecoveryTokenRecord>(
                TENANT,
                TOKEN_RECORD_TYPE,
                record.account_id.as_str(),
            )?;
        for (record_id, mut pending, token_revision) in pending_tokens {
            if pending.account_id != record.account_id || record_id != hex(&pending.token_hash) {
                return Err(corrupt());
            }
            if record_id == token_id {
                current_token_found = true;
                if pending.consumed_at.is_some() || now >= pending.expires_at {
                    return Err(invalid_token());
                }
            }
            if pending.consumed_at.is_none() {
                pending.consumed_at = Some(now);
                recovery_token_puts.push(ControlPut {
                    tenant_id: TENANT.into(),
                    record_type: TOKEN_RECORD_TYPE.into(),
                    record_id,
                    expected_revision: Some(token_revision),
                    value: serde_json::to_value(pending).map_err(|_| corrupt())?,
                });
            }
        }
        if !current_token_found {
            return Err(corrupt());
        }
        let mut puts = Vec::with_capacity(1 + recovery_token_puts.len());
        puts.push(password_put);
        puts.extend(recovery_token_puts);
        let session_revocations = self
            .sessions
            .account_session_revocation_puts(&record.account_id, now)?;
        let revoked_sessions = session_revocations
            .iter()
            .filter(|put| matches!(put.record_type.as_str(), "session" | "session_family"))
            .count();
        puts.extend(session_revocations);
        let revoked_api_tokens = self
            .api_tokens
            .account_revocation_puts(&record.account_id, now)?;
        let revoked_api_token_count = revoked_api_tokens.len();
        puts.extend(revoked_api_tokens);
        self.store.execute(ExecuteControlCommand {
            tenant_id: TENANT.into(),
            command_id: "auth.recovery.complete".into(),
            idempotency_key: command_key,
            expected_revisions: Default::default(),
            puts,
            deletes: vec![],
            audit: ControlAuditEntry {
                tenant_id: TENANT.into(),
                command_id: "auth.recovery.complete".into(),
                actor_id: record.account_id.as_str().into(),
                allowed: true,
                detail: json!({
                    "state": "consumed",
                    "sessionsRevoked": revoked_sessions,
                    "apiTokensRevoked": revoked_api_token_count,
                }),
            },
            response: HttpResponse {
                status: 204,
                headers: Default::default(),
                body: json!({"accountId": record.account_id}),
            },
        })?;
        Ok(record.account_id)
    }

    fn find_identity(&self, identifier: &str) -> Result<Option<AccountIdentity>, Problem> {
        let identity = if identifier.contains('@') {
            match canonical_email(identifier) {
                Ok(value) => self.identities.find_by_email(&value)?,
                Err(_) => None,
            }
        } else {
            match canonical_username(identifier) {
                Ok(value) => self.identities.find_by_username(&value)?,
                Err(_) => None,
            }
        };
        // Do not send recovery links to an address that has not been verified. Admin
        // provisioning enables the initial credential only; it does not prove email control.
        Ok(identity.filter(|identity| {
            identity.verification == buglogin_domain::account::IdentityVerification::Verified
        }))
    }

    fn invalidate(&self, record: &RecoveryTokenRecord, now: u64) -> Result<(), Problem> {
        let token_id = hex(&record.token_hash);
        let Some((mut current, revision)) = self.store.load_versioned::<RecoveryTokenRecord>(
            TENANT,
            TOKEN_RECORD_TYPE,
            &token_id,
        )?
        else {
            return Ok(());
        };
        current.consumed_at = Some(now);
        self.store.execute(ExecuteControlCommand {
            tenant_id: TENANT.into(),
            command_id: "auth.recovery.delivery_failed".into(),
            idempotency_key: format!("recovery-delivery-failed-{token_id}"),
            expected_revisions: Default::default(),
            puts: vec![ControlPut {
                tenant_id: TENANT.into(),
                record_type: TOKEN_RECORD_TYPE.into(),
                record_id: token_id,
                expected_revision: Some(revision),
                value: serde_json::to_value(current).map_err(|_| corrupt())?,
            }],
            deletes: vec![],
            audit: ControlAuditEntry {
                tenant_id: TENANT.into(),
                command_id: "auth.recovery.delivery_failed".into(),
                actor_id: record.account_id.as_str().into(),
                allowed: false,
                detail: json!({"state": "invalidated"}),
            },
            response: HttpResponse {
                status: 503,
                headers: Default::default(),
                body: Value::Null,
            },
        })?;
        Ok(())
    }
}

fn random_token() -> Result<String, Problem> {
    let mut bytes = [0u8; 32];
    if fill(&mut bytes).is_err() {
        bytes.zeroize();
        return Err(delivery_unavailable());
    }
    let token = hex(&bytes);
    bytes.zeroize();
    Ok(token)
}

fn sha256(value: &[u8]) -> [u8; 32] {
    Sha256::digest(value).into()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn invalid_token() -> Problem {
    Problem::new(
        "invalid_recovery_token",
        "Recovery token is invalid or expired",
        400,
        "identity",
    )
}

fn delivery_unavailable() -> Problem {
    Problem::new(
        "recovery_delivery_unavailable",
        "Recovery delivery is unavailable",
        503,
        "identity",
    )
    .retryable(true)
}

fn corrupt() -> Problem {
    Problem::new(
        "recovery_record_corrupt",
        "Recovery record is corrupt",
        500,
        "identity",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::Mutex,
        thread,
        time::{Duration, Instant},
    };

    use crate::{auth::AuthService, services::session_persistence::SessionPersistence};
    use buglogin_domain::account::AccountIdentity;
    use buglogin_storage::control::MemoryControlRecordStore;

    #[derive(Default)]
    struct RecordingDelivery {
        messages: Mutex<Vec<RecoveryDeliveryMessage>>,
    }

    impl RecoveryDelivery for RecordingDelivery {
        fn available(&self) -> bool {
            true
        }

        fn deliver(&self, message: RecoveryDeliveryMessage) -> Result<(), Problem> {
            self.messages.lock().unwrap().push(message);
            Ok(())
        }
    }

    fn retry_if_credential_capacity_is_busy<T>(
        mut authenticate: impl FnMut() -> Result<T, Problem>,
    ) -> Result<T, Problem> {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            match authenticate() {
                Err(error)
                    if error.code == "credential_capacity_exhausted"
                        && Instant::now() < deadline =>
                {
                    thread::sleep(Duration::from_millis(1));
                }
                result => return result,
            }
        }
    }

    fn setup() -> (
        RecoveryService,
        CredentialService,
        Arc<RecordingDelivery>,
        ControlPersistence,
        AccountId,
    ) {
        let store = ControlPersistence::new(MemoryControlRecordStore::default());
        let identities = IdentityPersistence::new(store.clone());
        let account_id = AccountId::new("acct_recovery").unwrap();
        identities
            .create(
                &AccountIdentity::new(
                    account_id.clone(),
                    "owner",
                    "owner@example.com",
                    "2026-07-23T00:00:00Z",
                )
                .unwrap()
                .verify(1, "2026-07-23T00:00:01Z")
                .unwrap(),
            )
            .unwrap();
        let credentials = CredentialService::new(store.clone()).unwrap();
        credentials
            .set_initial_password(&account_id, "correct horse battery staple", 100)
            .unwrap();
        let delivery = Arc::new(RecordingDelivery::default());
        let recovery = RecoveryService::new(store.clone(), credentials.clone())
            .with_delivery(delivery.clone());
        (recovery, credentials, delivery, store, account_id)
    }

    #[test]
    fn token_is_hashed_single_use_and_resets_password() {
        let (recovery, credentials, delivery, store, account_id) = setup();
        let mut auth = AuthService::persistent_with_credentials(
            SessionPersistence::new(store.clone()),
            credentials.clone(),
        )
        .unwrap();
        let (_, login_version) = retry_if_credential_capacity_is_busy(|| {
            credentials.authenticate_versioned("owner", "correct horse battery staple", 199)
        })
        .unwrap();
        let session = auth
            .issue_browser_with_credential_version(account_id.clone(), 10_000, 199, login_version)
            .unwrap();
        recovery.request("OWNER@example.com", 200).unwrap();
        let message = delivery.messages.lock().unwrap().pop().unwrap();
        assert_eq!(message.account_id, account_id);
        assert_eq!(message.destination, "owner@example.com");
        let records = store
            .list_versioned::<RecoveryTokenRecord>(TENANT, TOKEN_RECORD_TYPE)
            .unwrap();
        assert_eq!(records.len(), 1);
        assert!(
            !serde_json::to_string(&records)
                .unwrap()
                .contains(&message.raw_token)
        );

        assert_eq!(
            recovery
                .complete(
                    &message.raw_token,
                    "new correct horse battery staple",
                    "recovery-complete-test-0001",
                    201,
                )
                .unwrap(),
            account_id
        );
        assert_eq!(
            retry_if_credential_capacity_is_busy(|| {
                credentials.authenticate("owner", "correct horse battery staple", 202)
            })
            .unwrap_err()
            .code,
            "invalid_credentials"
        );
        assert_eq!(
            retry_if_credential_capacity_is_busy(|| {
                credentials.authenticate("owner", "new correct horse battery staple", 203)
            })
            .unwrap(),
            account_id
        );
        assert_eq!(
            AuthService::persistent(SessionPersistence::new(store.clone()))
                .unwrap()
                .authenticate_browser(&session.raw_session, &session.raw_csrf, 203)
                .unwrap_err()
                .code,
            "session_revoked"
        );
        let late_session = AuthService::persistent_with_credentials(
            SessionPersistence::new(store.clone()),
            credentials.clone(),
        )
        .unwrap()
        .issue_browser_with_credential_version(account_id.clone(), 10_000, 202, login_version)
        .unwrap();
        assert_eq!(
            AuthService::persistent_with_credentials(
                SessionPersistence::new(store.clone()),
                credentials.clone(),
            )
            .unwrap()
            .authenticate_browser(&late_session.raw_session, &late_session.raw_csrf, 203)
            .unwrap_err()
            .code,
            "session_revoked"
        );
        assert_eq!(
            recovery
                .complete(
                    &message.raw_token,
                    "another correct horse battery staple",
                    "recovery-complete-test-0002",
                    204,
                )
                .unwrap_err()
                .code,
            "invalid_recovery_token"
        );
    }

    #[test]
    fn completing_recovery_consumes_every_other_outstanding_token() {
        let (recovery, _, delivery, _, account_id) = setup();
        recovery.request("owner@example.com", 200).unwrap();
        recovery.request("owner@example.com", 201).unwrap();
        let messages = std::mem::take(&mut *delivery.messages.lock().unwrap());
        assert_eq!(messages.len(), 2);

        assert_eq!(
            recovery
                .complete(
                    &messages[0].raw_token,
                    "new correct horse battery staple",
                    "recovery-complete-current-token",
                    202,
                )
                .unwrap(),
            account_id
        );
        assert_eq!(
            recovery
                .complete(
                    &messages[1].raw_token,
                    "another correct horse battery staple",
                    "recovery-complete-stale-token",
                    203,
                )
                .unwrap_err()
                .code,
            "invalid_recovery_token"
        );
    }

    #[test]
    fn unknown_identity_and_rate_limit_have_same_success_shape() {
        let (recovery, _, delivery, _, _) = setup();
        recovery.request("missing@example.com", 200).unwrap();
        assert!(delivery.messages.lock().unwrap().is_empty());

        for second in 201..=203 {
            recovery.request("owner@example.com", second).unwrap();
        }
        recovery.request("owner", 204).unwrap();
        assert_eq!(delivery.messages.lock().unwrap().len(), 3);
    }

    #[test]
    fn admin_provisioned_email_is_not_eligible_for_password_recovery() {
        let store = ControlPersistence::new(MemoryControlRecordStore::default());
        let account_id = AccountId::new("acct_unverified_recovery").unwrap();
        IdentityPersistence::new(store.clone())
            .create(
                &AccountIdentity::admin_provisioned(
                    account_id.clone(),
                    "unverified",
                    "unverified@example.com",
                    "2026-10-24T00:00:00Z",
                )
                .unwrap(),
            )
            .unwrap();
        let credentials = CredentialService::new(store.clone()).unwrap();
        credentials
            .set_initial_password(&account_id, "initial-password-123", 100)
            .unwrap();
        let delivery = Arc::new(RecordingDelivery::default());
        let recovery =
            RecoveryService::new(store.clone(), credentials).with_delivery(delivery.clone());

        recovery.request("unverified@example.com", 200).unwrap();
        assert!(delivery.messages.lock().unwrap().is_empty());
        assert!(
            store
                .list::<RecoveryTokenRecord>(TENANT, TOKEN_RECORD_TYPE)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn concurrent_recovery_requests_retry_rate_revision_conflicts_without_over_delivery() {
        let (recovery, _, delivery, store, _) = setup();
        let barrier = Arc::new(std::sync::Barrier::new(12));
        let requests = (0..12)
            .map(|_| {
                let recovery = recovery.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    recovery.request("owner@example.com", 300)
                })
            })
            .collect::<Vec<_>>();

        for request in requests {
            request
                .join()
                .expect("recovery request thread should finish")
                .expect("CAS conflict should retry without exposing account state");
        }

        assert_eq!(delivery.messages.lock().unwrap().len(), 3);
        let rate_id = hex(&sha256(b"account:acct_recovery"));
        let rate = store
            .load::<RecoveryRateRecord>(TENANT, RATE_RECORD_TYPE, &rate_id)
            .unwrap()
            .expect("all request attempts should be counted");
        assert_eq!(rate.attempts, 12);
    }

    #[test]
    fn unavailable_delivery_fails_before_identity_lookup() {
        let store = ControlPersistence::new(MemoryControlRecordStore::default());
        let credentials = CredentialService::new(store.clone()).unwrap();
        let recovery = RecoveryService::new(store, credentials);
        assert_eq!(
            recovery
                .request("anyone@example.com", 200)
                .unwrap_err()
                .code,
            "recovery_delivery_unavailable"
        );
    }

    #[test]
    fn unavailable_delivery_cannot_consume_a_recovery_token() {
        let store = ControlPersistence::new(MemoryControlRecordStore::default());
        let credentials = CredentialService::new(store.clone()).unwrap();
        let recovery = RecoveryService::new(store.clone(), credentials);
        let token = "a".repeat(64);

        assert_eq!(
            recovery
                .complete(&token, "A-long-enough-new-password", "recovery-test", 200)
                .unwrap_err()
                .code,
            "recovery_delivery_unavailable"
        );
        assert!(
            store
                .load::<RecoveryTokenRecord>(
                    TENANT,
                    TOKEN_RECORD_TYPE,
                    &hex(&sha256(token.as_bytes()))
                )
                .unwrap()
                .is_none()
        );
    }
}
