use std::{env, sync::Arc};

use buglogin_domain::{
    account::{AccountIdentity, canonical_email, canonical_username},
    errors::Problem,
    identity::SessionRecord,
    ids::AccountId,
};
use buglogin_storage::control::{
    ControlAuditEntry, ControlPut, ExecuteControlCommand, HttpResponse,
};
use getrandom::fill;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{
    control_persistence::ControlPersistence, credentials::CredentialService,
    identity_persistence::IdentityPersistence,
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

pub struct WebhookRecoveryDelivery {
    url: String,
    secret: String,
    client: reqwest::blocking::Client,
}

impl WebhookRecoveryDelivery {
    pub fn from_env(production: bool) -> Result<Option<Arc<dyn RecoveryDelivery>>, Problem> {
        let url = env::var("BUGLOGIN_RECOVERY_WEBHOOK_URL").ok();
        let secret = env::var("BUGLOGIN_RECOVERY_WEBHOOK_SECRET").ok();
        if url.is_none() && secret.is_none() {
            return Ok(None);
        }
        let url =
            url.ok_or_else(|| delivery_config("BUGLOGIN_RECOVERY_WEBHOOK_URL is required"))?;
        let secret = secret
            .ok_or_else(|| delivery_config("BUGLOGIN_RECOVERY_WEBHOOK_SECRET is required"))?;
        if production && !url.starts_with("https://") {
            return Err(delivery_config(
                "BUGLOGIN_RECOVERY_WEBHOOK_URL must use HTTPS in production",
            ));
        }
        if secret.len() < 32 || secret.len() > 4096 {
            return Err(delivery_config(
                "BUGLOGIN_RECOVERY_WEBHOOK_SECRET length is invalid",
            ));
        }
        let client = reqwest::blocking::Client::builder()
            .build()
            .map_err(|_| delivery_config("Recovery delivery client could not initialize"))?;
        Ok(Some(Arc::new(Self {
            url,
            secret,
            client,
        })))
    }
}

impl RecoveryDelivery for WebhookRecoveryDelivery {
    fn available(&self) -> bool {
        true
    }

    fn deliver(&self, message: RecoveryDeliveryMessage) -> Result<(), Problem> {
        let response = self
            .client
            .post(&self.url)
            .bearer_auth(&self.secret)
            .json(&json!({
                "accountId": message.account_id,
                "destination": message.destination,
                "token": message.raw_token,
                "expiresAt": message.expires_at,
            }))
            .send()
            .map_err(|_| delivery_unavailable())?;
        if !response.status().is_success() {
            return Err(delivery_unavailable());
        }
        Ok(())
    }
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
    delivery: Arc<dyn RecoveryDelivery>,
}

impl RecoveryService {
    pub fn new(store: ControlPersistence, credentials: CredentialService) -> Self {
        Self {
            identities: IdentityPersistence::new(store.clone()),
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
        let rate_id = hex(&sha256(identifier.trim().to_lowercase().as_bytes()));
        let request_nonce = random_token()?;
        let identity = self.find_identity(identifier)?;
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
                let raw_token = random_token()?;
                let token_hash = sha256(raw_token.as_bytes());
                let record = RecoveryTokenRecord {
                    account_id: identity.account_id.clone(),
                    token_hash,
                    created_at: now,
                    expires_at: now.saturating_add(TOKEN_SECONDS),
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
                    if let Some((identity, record, raw_token)) = issued {
                        if let Err(error) = self.delivery.deliver(RecoveryDeliveryMessage {
                            account_id: identity.account_id,
                            destination: identity.email,
                            raw_token,
                            expires_at: record.expires_at,
                        }) {
                            self.invalidate(&record, now)?;
                            let _ = error;
                        }
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
        if raw_token.len() != 64 || !raw_token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
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
        let Some((mut record, revision)) = self.store.load_versioned::<RecoveryTokenRecord>(
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
        record.consumed_at = Some(now);
        let mut puts = vec![
            password_put,
            ControlPut {
                tenant_id: TENANT.into(),
                record_type: TOKEN_RECORD_TYPE.into(),
                record_id: token_id,
                expected_revision: Some(revision),
                value: serde_json::to_value(&record).map_err(|_| corrupt())?,
            },
        ];
        let mut revoked_sessions = 0;
        for (session_id, mut session, session_revision) in self
            .store
            .list_versioned::<SessionRecord>(TENANT, "session")?
        {
            if session.account_id != record.account_id || session.revoked_at.is_some() {
                continue;
            }
            session.revoked_at = Some(now);
            puts.push(ControlPut {
                tenant_id: TENANT.into(),
                record_type: "session".into(),
                record_id: session_id,
                expected_revision: Some(session_revision),
                value: serde_json::to_value(session).map_err(|_| corrupt())?,
            });
            revoked_sessions += 1;
        }
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
                detail: json!({"state": "consumed", "sessionsRevoked": revoked_sessions}),
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
        if identifier.contains('@') {
            return Ok(match canonical_email(identifier) {
                Ok(value) => self.identities.find_by_email(&value)?,
                Err(_) => None,
            });
        }
        Ok(match canonical_username(identifier) {
            Ok(value) => self.identities.find_by_username(&value)?,
            Err(_) => None,
        })
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
    fill(&mut bytes).map_err(|_| delivery_unavailable())?;
    Ok(hex(&bytes))
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

fn delivery_config(detail: &'static str) -> Problem {
    Problem::new("recovery_delivery_config_invalid", detail, 500, "identity")
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
    use std::sync::Mutex;

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
        let (_, login_version) = credentials
            .authenticate_versioned("owner", "correct horse battery staple", 199)
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
            credentials
                .authenticate("owner", "correct horse battery staple", 202)
                .unwrap_err()
                .code,
            "invalid_credentials"
        );
        assert_eq!(
            credentials
                .authenticate("owner", "new correct horse battery staple", 203)
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
    fn unknown_identity_and_rate_limit_have_same_success_shape() {
        let (recovery, _, delivery, _, _) = setup();
        recovery.request("missing@example.com", 200).unwrap();
        assert!(delivery.messages.lock().unwrap().is_empty());

        for second in 201..=204 {
            recovery.request("owner@example.com", second).unwrap();
        }
        assert_eq!(delivery.messages.lock().unwrap().len(), 3);
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
        let rate_id = hex(&sha256(b"owner@example.com"));
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
}
