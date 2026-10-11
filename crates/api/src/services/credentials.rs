use argon2::{
    Algorithm, Argon2, Params, Version,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
};
use buglogin_domain::{
    account::{canonical_email, canonical_username},
    errors::Problem,
    ids::AccountId,
};
use buglogin_storage::control::{
    ControlAuditEntry, ControlPut, ExecuteControlCommand, HttpResponse,
};
use getrandom::fill;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{control_persistence::ControlPersistence, identity_persistence::IdentityPersistence};

const TENANT: &str = "identity";
const RECORD_TYPE: &str = "account_password";
const MAX_FAILURES: u32 = 5;
const LOCK_SECONDS: u64 = 15 * 60;

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct PasswordRecord {
    account_id: AccountId,
    password_phc: String,
    password_version: u32,
    failed_attempts: u32,
    locked_until: Option<u64>,
    changed_at: u64,
    updated_at: u64,
}

#[derive(Clone)]
pub struct CredentialService {
    store: ControlPersistence,
    identities: IdentityPersistence,
    dummy_phc: String,
}

impl CredentialService {
    pub fn new(store: ControlPersistence) -> Result<Self, Problem> {
        let dummy_phc = hash_password("buglogin-invalid-credential-work-factor")?;
        Ok(Self {
            identities: IdentityPersistence::new(store.clone()),
            store,
            dummy_phc,
        })
    }

    pub fn set_initial_password(
        &self,
        account_id: &AccountId,
        password: &str,
        now: u64,
    ) -> Result<(), Problem> {
        if self.identities.load(account_id)?.is_none() {
            return Err(Problem::new(
                "account_identity_not_found",
                "Account identity was not found",
                404,
                "identity",
            ));
        }
        validate_password(password)?;
        let record = PasswordRecord {
            account_id: account_id.clone(),
            password_phc: hash_password(password)?,
            password_version: 1,
            failed_attempts: 0,
            locked_until: None,
            changed_at: now,
            updated_at: now,
        };
        self.persist(&record, 0, "credential.create", "created")?;
        Ok(())
    }

    pub fn authenticate(
        &self,
        identifier: &str,
        password: &str,
        now: u64,
    ) -> Result<AccountId, Problem> {
        self.authenticate_versioned(identifier, password, now)
            .map(|(account_id, _)| account_id)
    }

    pub fn authenticate_versioned(
        &self,
        identifier: &str,
        password: &str,
        now: u64,
    ) -> Result<(AccountId, u32), Problem> {
        if password.chars().count() > 256 {
            let _ = verify_password(&self.dummy_phc, password);
            return Err(invalid_credentials());
        }
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
        let Some(identity) = identity else {
            let _ = verify_password(&self.dummy_phc, password);
            return Err(invalid_credentials());
        };
        let Some((record, revision)) = self.store.load_versioned::<PasswordRecord>(
            TENANT,
            RECORD_TYPE,
            identity.account_id.as_str(),
        )?
        else {
            let _ = verify_password(&self.dummy_phc, password);
            return Err(invalid_credentials());
        };
        let verified = verify_password(&record.password_phc, password);
        if record.locked_until.is_some_and(|until| now < until) {
            return Err(invalid_credentials());
        }
        if !verified {
            self.record_failure(&record, revision, now)?;
            return Err(invalid_credentials());
        }

        self.record_success(&record, revision, now)?;
        Ok((record.account_id, record.password_version))
    }

    pub(crate) fn password_version(&self, account_id: &AccountId) -> Result<Option<u32>, Problem> {
        self.store
            .load_versioned::<PasswordRecord>(TENANT, RECORD_TYPE, account_id.as_str())
            .map(|record| record.map(|(record, _)| record.password_version))
    }

    pub(crate) fn password_reset_put(
        &self,
        account_id: &AccountId,
        password: &str,
        now: u64,
    ) -> Result<ControlPut, Problem> {
        validate_password(password)?;
        let Some((current, revision)) = self.store.load_versioned::<PasswordRecord>(
            TENANT,
            RECORD_TYPE,
            account_id.as_str(),
        )?
        else {
            return Err(unavailable());
        };
        let record = PasswordRecord {
            account_id: account_id.clone(),
            password_phc: hash_password(password)?,
            password_version: current.password_version.saturating_add(1),
            failed_attempts: 0,
            locked_until: None,
            changed_at: now,
            updated_at: now,
        };
        Ok(ControlPut {
            tenant_id: TENANT.into(),
            record_type: RECORD_TYPE.into(),
            record_id: account_id.as_str().into(),
            expected_revision: Some(revision),
            value: serde_json::to_value(record).map_err(|_| corrupt())?,
        })
    }

    fn record_success(
        &self,
        current: &PasswordRecord,
        current_revision: u64,
        now: u64,
    ) -> Result<(), Problem> {
        let verified_phc = current.password_phc.clone();
        let mut record = current.clone();
        let mut revision = current_revision;
        for _ in 0..3 {
            if record.password_phc != verified_phc {
                return Err(invalid_credentials());
            }
            record.failed_attempts = 0;
            record.locked_until = None;
            record.updated_at = now;
            match self.persist(&record, revision, "credential.login", "authenticated") {
                Ok(_) => return Ok(()),
                Err(error) if error.code == "storage_revision_conflict" => {
                    let Some((latest, latest_revision)) = self.store.load_versioned(
                        TENANT,
                        RECORD_TYPE,
                        record.account_id.as_str(),
                    )?
                    else {
                        return Err(unavailable());
                    };
                    record = latest;
                    revision = latest_revision;
                }
                Err(error) => return Err(error),
            }
        }
        Err(unavailable())
    }

    fn record_failure(
        &self,
        current: &PasswordRecord,
        current_revision: u64,
        now: u64,
    ) -> Result<(), Problem> {
        let mut record = current.clone();
        let mut revision = current_revision;
        for _ in 0..3 {
            record.failed_attempts = record.failed_attempts.saturating_add(1);
            if record.failed_attempts >= MAX_FAILURES {
                record.locked_until = Some(now.saturating_add(LOCK_SECONDS));
            }
            record.updated_at = now;
            match self.persist(&record, revision, "credential.login", "rejected") {
                Ok(_) => return Ok(()),
                Err(error) if error.code == "storage_revision_conflict" => {
                    let Some((latest, latest_revision)) = self.store.load_versioned(
                        TENANT,
                        RECORD_TYPE,
                        record.account_id.as_str(),
                    )?
                    else {
                        return Err(unavailable());
                    };
                    record = latest;
                    revision = latest_revision;
                }
                Err(error) => return Err(error),
            }
        }
        Err(unavailable())
    }

    fn persist(
        &self,
        record: &PasswordRecord,
        expected_revision: u64,
        command_id: &str,
        state: &str,
    ) -> Result<u64, Problem> {
        let result = self.store.execute(ExecuteControlCommand {
            tenant_id: TENANT.into(),
            command_id: command_id.into(),
            idempotency_key: format!(
                "{command_id}-{}-{}-{expected_revision}",
                record.account_id.as_str(),
                record.updated_at
            ),
            expected_revisions: Default::default(),
            puts: vec![ControlPut {
                tenant_id: TENANT.into(),
                record_type: RECORD_TYPE.into(),
                record_id: record.account_id.as_str().into(),
                expected_revision: Some(expected_revision),
                value: serde_json::to_value(record).map_err(|_| corrupt())?,
            }],
            deletes: vec![],
            audit: ControlAuditEntry {
                tenant_id: TENANT.into(),
                command_id: command_id.into(),
                actor_id: record.account_id.as_str().into(),
                allowed: true,
                detail: json!({"state": state}),
            },
            response: HttpResponse {
                status: 204,
                headers: Default::default(),
                body: Value::Null,
            },
        })?;
        result
            .records
            .first()
            .map(|stored| stored.revision)
            .ok_or_else(unavailable)
    }
}

fn argon2() -> Result<Argon2<'static>, Problem> {
    Ok(Argon2::new(
        Algorithm::Argon2id,
        Version::V0x13,
        Params::new(19_456, 2, 1, None).map_err(|_| unavailable())?,
    ))
}

fn hash_password(password: &str) -> Result<String, Problem> {
    let mut salt_bytes = [0u8; 16];
    fill(&mut salt_bytes).map_err(|_| unavailable())?;
    let salt = SaltString::encode_b64(&salt_bytes).map_err(|_| unavailable())?;
    argon2()?
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|_| unavailable())
}

fn verify_password(phc: &str, password: &str) -> bool {
    PasswordHash::new(phc).ok().is_some_and(|hash| {
        argon2().is_ok_and(|argon| argon.verify_password(password.as_bytes(), &hash).is_ok())
    })
}

fn validate_password(password: &str) -> Result<(), Problem> {
    let length = password.chars().count();
    if !(12..=256).contains(&length) {
        return Err(Problem::new(
            "invalid_password",
            "Password must contain 12 to 256 characters",
            400,
            "identity",
        ));
    }
    Ok(())
}

fn invalid_credentials() -> Problem {
    Problem::new(
        "invalid_credentials",
        "Invalid credentials",
        401,
        "identity",
    )
}

fn unavailable() -> Problem {
    Problem::new(
        "credential_service_unavailable",
        "Credential service is unavailable",
        503,
        "identity",
    )
    .retryable(true)
}

fn corrupt() -> Problem {
    Problem::new(
        "credential_record_corrupt",
        "Credential record is corrupt",
        500,
        "identity",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use buglogin_domain::account::AccountIdentity;
    use buglogin_storage::control::MemoryControlRecordStore;

    fn service() -> (IdentityPersistence, CredentialService) {
        let store = ControlPersistence::new(MemoryControlRecordStore::default());
        (
            IdentityPersistence::new(store.clone()),
            CredentialService::new(store).unwrap(),
        )
    }

    fn seed(identities: &IdentityPersistence, credentials: &CredentialService) -> AccountId {
        let account_id = AccountId::new("acct_login").unwrap();
        identities
            .create(
                &AccountIdentity::new(
                    account_id.clone(),
                    "Owner",
                    "owner@example.com",
                    "2026-07-22T00:00:00Z",
                )
                .unwrap(),
            )
            .unwrap();
        credentials
            .set_initial_password(&account_id, "correct horse battery staple", 100)
            .unwrap();
        account_id
    }

    #[test]
    fn authenticates_username_or_email_without_persisting_raw_password() {
        let (identities, credentials) = service();
        let account_id = seed(&identities, &credentials);
        let revision_before = credentials
            .store
            .load_versioned::<PasswordRecord>(TENANT, RECORD_TYPE, account_id.as_str())
            .unwrap()
            .unwrap()
            .1;
        assert_eq!(
            credentials
                .authenticate("OWNER", "correct horse battery staple", 101)
                .unwrap(),
            account_id
        );
        let revision_after_first_login = credentials
            .store
            .load_versioned::<PasswordRecord>(TENANT, RECORD_TYPE, account_id.as_str())
            .unwrap()
            .unwrap()
            .1;
        assert_eq!(revision_after_first_login, revision_before + 1);
        assert_eq!(
            credentials
                .authenticate("Owner@Example.COM", "correct horse battery staple", 102)
                .unwrap(),
            account_id
        );
        let persisted = serde_json::to_string(
            &credentials
                .store
                .list_versioned::<PasswordRecord>(TENANT, RECORD_TYPE)
                .unwrap(),
        )
        .unwrap();
        assert!(!persisted.contains("correct horse battery staple"));
        assert!(persisted.contains("$argon2id$"));
    }

    #[test]
    fn locks_after_repeated_failures_and_recovers_after_window() {
        let (identities, credentials) = service();
        let account_id = seed(&identities, &credentials);
        for now in 101..106 {
            assert_eq!(
                credentials
                    .authenticate("owner", "wrong password value", now)
                    .unwrap_err()
                    .code,
                "invalid_credentials"
            );
        }
        assert_eq!(
            credentials
                .authenticate("owner", "correct horse battery staple", 106)
                .unwrap_err()
                .code,
            "invalid_credentials"
        );
        assert_eq!(
            credentials
                .authenticate("owner", "correct horse battery staple", 105 + LOCK_SECONDS)
                .unwrap(),
            account_id
        );
    }

    #[test]
    fn unknown_identity_returns_same_public_error() {
        let (_, credentials) = service();
        assert_eq!(
            credentials
                .authenticate("nobody", "wrong password value", 100)
                .unwrap_err()
                .code,
            "invalid_credentials"
        );
    }
}
