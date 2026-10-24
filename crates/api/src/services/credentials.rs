use argon2::{
    Algorithm, Argon2, Params, Version,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
};
use buglogin_domain::{
    account::{IdentityVerification, canonical_email, canonical_username},
    errors::Problem,
    ids::AccountId,
};
use buglogin_storage::control::{
    ControlAuditEntry, ControlPut, ExecuteControlCommand, HttpResponse,
};
use getrandom::fill;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use subtle::ConstantTimeEq;
use tokio::sync::{Semaphore, SemaphorePermit};
use zeroize::{Zeroize, ZeroizeOnDrop};

use super::{control_persistence::ControlPersistence, identity_persistence::IdentityPersistence};

const TENANT: &str = "identity";
const RECORD_TYPE: &str = "account_password";
const MAX_FAILURES: u32 = 5;
const LOCK_SECONDS: u64 = 15 * 60;
const MAX_CONCURRENT_PASSWORD_VERIFICATIONS: usize = 4;
const MAX_PASSWORD_PHC_BYTES: usize = 256;
const PASSWORD_ARGON2_MEMORY_KIB: u32 = 19_456;
const PASSWORD_ARGON2_ITERATIONS: u32 = 2;
const PASSWORD_ARGON2_PARALLELISM: u32 = 1;
const PASSWORD_ARGON2_OUTPUT_BYTES: usize = 32;
static PASSWORD_VERIFICATION_SLOTS: Semaphore =
    Semaphore::const_new(MAX_CONCURRENT_PASSWORD_VERIFICATIONS);

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize, Zeroize, ZeroizeOnDrop)]
#[serde(rename_all = "camelCase")]
struct PasswordRecord {
    #[zeroize(skip)]
    account_id: AccountId,
    password_phc: Option<String>,
    #[serde(default)]
    legacy_scrypt: Option<LegacyScryptVerifier>,
    #[zeroize(skip)]
    password_version: u32,
    #[zeroize(skip)]
    failed_attempts: u32,
    #[zeroize(skip)]
    locked_until: Option<u64>,
    #[zeroize(skip)]
    changed_at: u64,
    #[zeroize(skip)]
    updated_at: u64,
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize, Zeroize, ZeroizeOnDrop)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LegacyScryptVerifier {
    salt: String,
    hash: String,
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
            password_phc: Some(hash_password(password)?),
            legacy_scrypt: None,
            password_version: 1,
            failed_attempts: 0,
            locked_until: None,
            changed_at: now,
            updated_at: now,
        };
        let operation_id = new_operation_id()?;
        self.persist(&record, 0, "credential.create", "created", &operation_id)?;
        Ok(())
    }

    pub(crate) fn prepare_initial_password_put(
        &self,
        account_id: &AccountId,
        password: &str,
        now: u64,
    ) -> Result<ControlPut, Problem> {
        validate_password(password)?;
        let record = PasswordRecord {
            account_id: account_id.clone(),
            password_phc: Some(hash_password(password)?),
            legacy_scrypt: None,
            password_version: 1,
            failed_attempts: 0,
            locked_until: None,
            changed_at: now,
            updated_at: now,
        };
        Ok(ControlPut {
            tenant_id: TENANT.into(),
            record_type: RECORD_TYPE.into(),
            record_id: account_id.as_str().into(),
            expected_revision: Some(0),
            value: serde_json::to_value(record).map_err(|_| corrupt())?,
        })
    }

    pub(crate) fn validate_initial_password(password: &str) -> Result<(), Problem> {
        validate_password(password)
    }

    /// Compare a candidate credential to the stored verifier without recording a login
    /// success/failure, changing lockout state, or advancing credential versions. This is
    /// reserved for validating an idempotent account-provision replay.
    pub(crate) fn matches_password_without_recording_attempt(
        &self,
        account_id: &AccountId,
        password: &str,
    ) -> Result<bool, Problem> {
        validate_password(password)?;
        let _verification_slot = acquire_password_verification_slot(&PASSWORD_VERIFICATION_SLOTS)?;
        let Some((record, _revision)) = self.store.load_versioned::<PasswordRecord>(
            TENANT,
            RECORD_TYPE,
            account_id.as_str(),
        )?
        else {
            return Ok(false);
        };
        validate_password_record_owner(&record, account_id)?;
        Ok(match &record.legacy_scrypt {
            Some(legacy) => verify_legacy_scrypt(password, legacy),
            None => record
                .password_phc
                .as_deref()
                .is_some_and(|phc| verify_password(phc, password)),
        })
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
        let verification_slot = acquire_password_verification_slot(&PASSWORD_VERIFICATION_SLOTS)?;
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
        validate_password_record_owner(&record, &identity.account_id)?;
        let operation_id = new_operation_id()?;
        let verified = match &record.legacy_scrypt {
            Some(legacy) => verify_legacy_scrypt(password, legacy),
            None => record
                .password_phc
                .as_deref()
                .is_some_and(|phc| verify_password(phc, password)),
        };
        drop(verification_slot);
        if record.locked_until.is_some_and(|until| now < until) {
            return Err(invalid_credentials());
        }
        if !verified {
            self.record_failure(&record, revision, now, &operation_id)?;
            return Err(invalid_credentials());
        }
        if !matches!(
            identity.verification,
            IdentityVerification::Verified | IdentityVerification::AdminProvisioned
        ) {
            return Err(invalid_credentials());
        }

        let password_version =
            self.record_success(&record, revision, password, now, &operation_id)?;
        Ok((record.account_id.clone(), password_version))
    }

    pub(crate) fn password_version(&self, account_id: &AccountId) -> Result<Option<u32>, Problem> {
        Ok(self
            .password_versioned(account_id)?
            .map(|(version, _)| version))
    }

    pub(crate) fn password_versioned(
        &self,
        account_id: &AccountId,
    ) -> Result<Option<(u32, u64)>, Problem> {
        let Some((record, revision)) = self.store.load_versioned::<PasswordRecord>(
            TENANT,
            RECORD_TYPE,
            account_id.as_str(),
        )?
        else {
            return Ok(None);
        };
        validate_password_record_owner(&record, account_id)?;
        Ok(Some((record.password_version, revision)))
    }

    pub(crate) fn password_reset_put(
        &self,
        account_id: &AccountId,
        password: &str,
        now: u64,
    ) -> Result<ControlPut, Problem> {
        self.password_reset_put_with_minimum(account_id, password, now, 12)
    }

    pub(crate) fn local_dev_admin_password_reset_put(
        &self,
        account_id: &AccountId,
        password: &str,
        now: u64,
    ) -> Result<ControlPut, Problem> {
        self.password_reset_put_with_minimum(account_id, password, now, 8)
    }

    fn password_reset_put_with_minimum(
        &self,
        account_id: &AccountId,
        password: &str,
        now: u64,
        minimum_length: usize,
    ) -> Result<ControlPut, Problem> {
        validate_password_with_minimum(password, minimum_length)?;
        let Some((current, revision)) = self.store.load_versioned::<PasswordRecord>(
            TENANT,
            RECORD_TYPE,
            account_id.as_str(),
        )?
        else {
            return Err(unavailable());
        };
        validate_password_record_owner(&current, account_id)?;
        let record = PasswordRecord {
            account_id: account_id.clone(),
            password_phc: Some(hash_password(password)?),
            legacy_scrypt: None,
            password_version: current.password_version.checked_add(1).ok_or_else(|| {
                Problem::new(
                    "credential_version_exhausted",
                    "Password credential version cannot be advanced",
                    409,
                    "identity",
                )
            })?,
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
        password: &str,
        now: u64,
        operation_id: &str,
    ) -> Result<u32, Problem> {
        let verified_phc = current.password_phc.clone();
        let verified_legacy = current.legacy_scrypt.clone();
        let mut record = current.clone();
        let mut revision = current_revision;
        for _ in 0..3 {
            // A concurrent failed-login request may have locked the account
            // after authenticate_versioned checked the original record. Do
            // not let this CAS retry clear that newer lockout state.
            if record.locked_until.is_some_and(|until| now < until) {
                return Err(invalid_credentials());
            }
            if record.password_phc != verified_phc || record.legacy_scrypt != verified_legacy {
                if verified_legacy.is_some()
                    && record.legacy_scrypt.is_none()
                    && record
                        .password_phc
                        .as_deref()
                        .is_some_and(|phc| verify_password(phc, password))
                {
                    return Ok(record.password_version);
                }
                return Err(invalid_credentials());
            }
            if record.legacy_scrypt.is_some() {
                record.password_phc = Some(hash_password(password)?);
                record.legacy_scrypt = None;
            }
            record.failed_attempts = 0;
            record.locked_until = None;
            record.updated_at = now;
            match self.persist(
                &record,
                revision,
                "credential.login",
                "authenticated",
                operation_id,
            ) {
                Ok(_) => return Ok(record.password_version),
                Err(error) if error.code == "storage_revision_conflict" => {
                    let Some((latest, latest_revision)) = self.store.load_versioned(
                        TENANT,
                        RECORD_TYPE,
                        record.account_id.as_str(),
                    )?
                    else {
                        return Err(unavailable());
                    };
                    validate_password_record_owner(&latest, &record.account_id)?;
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
        operation_id: &str,
    ) -> Result<(), Problem> {
        let mut record = current.clone();
        let mut revision = current_revision;
        for _ in 0..3 {
            if record.locked_until.is_some_and(|until| until <= now) {
                record.failed_attempts = 0;
                record.locked_until = None;
            }
            record.failed_attempts = record.failed_attempts.saturating_add(1);
            if record.failed_attempts >= MAX_FAILURES {
                let next_lock =
                    crate::clock::checked_add_seconds(now, LOCK_SECONDS, "credentials")?;
                record.locked_until = Some(
                    record
                        .locked_until
                        .map_or(next_lock, |existing| existing.max(next_lock)),
                );
            }
            record.updated_at = now;
            match self.persist(
                &record,
                revision,
                "credential.login",
                "rejected",
                operation_id,
            ) {
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
                    validate_password_record_owner(&latest, &record.account_id)?;
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
        operation_id: &str,
    ) -> Result<u64, Problem> {
        let result = self.store.execute(ExecuteControlCommand {
            tenant_id: TENANT.into(),
            command_id: command_id.into(),
            idempotency_key: credential_idempotency_key(
                command_id,
                record.account_id.as_str(),
                operation_id,
                expected_revision,
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

fn new_operation_id() -> Result<String, Problem> {
    let mut bytes = [0_u8; 16];
    fill(&mut bytes).map_err(|_| unavailable())?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn credential_idempotency_key(
    command_id: &str,
    account_id: &str,
    operation_id: &str,
    expected_revision: u64,
) -> String {
    format!("{command_id}-{account_id}-{operation_id}-{expected_revision}")
}

impl CredentialService {
    pub(crate) fn prepare_legacy_scrypt_put(
        &self,
        account_id: &AccountId,
        salt: &str,
        hash: &str,
        now: u64,
    ) -> Result<ControlPut, Problem> {
        if salt.len() != 32
            || !salt.bytes().all(|byte| byte.is_ascii_hexdigit())
            || hash.len() != 128
            || !hash.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(corrupt());
        }
        let record = PasswordRecord {
            account_id: account_id.clone(),
            password_phc: None,
            legacy_scrypt: Some(LegacyScryptVerifier {
                salt: salt.to_owned(),
                hash: hash.to_ascii_lowercase(),
            }),
            password_version: 1,
            failed_attempts: 0,
            locked_until: None,
            changed_at: now,
            updated_at: now,
        };
        Ok(ControlPut {
            tenant_id: TENANT.into(),
            record_type: RECORD_TYPE.into(),
            record_id: account_id.as_str().into(),
            expected_revision: Some(0),
            value: serde_json::to_value(record).map_err(|_| corrupt())?,
        })
    }
}

fn acquire_password_verification_slot(limiter: &Semaphore) -> Result<SemaphorePermit<'_>, Problem> {
    limiter.try_acquire().map_err(|_| {
        Problem::new(
            "credential_capacity_exhausted",
            "Password verification is temporarily busy",
            503,
            "identity",
        )
        .retryable(true)
    })
}

fn argon2() -> Result<Argon2<'static>, Problem> {
    Ok(Argon2::new(
        Algorithm::Argon2id,
        Version::V0x13,
        Params::new(
            PASSWORD_ARGON2_MEMORY_KIB,
            PASSWORD_ARGON2_ITERATIONS,
            PASSWORD_ARGON2_PARALLELISM,
            None,
        )
        .map_err(|_| unavailable())?,
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

fn validate_password_record_owner(
    record: &PasswordRecord,
    expected_account_id: &AccountId,
) -> Result<(), Problem> {
    if record.account_id != *expected_account_id
        || record.password_version == 0
        || record.password_phc.is_some() == record.legacy_scrypt.is_some()
        || record
            .password_phc
            .as_deref()
            .is_some_and(|phc| supported_password_hash(phc).is_none())
        || record
            .legacy_scrypt
            .as_ref()
            .is_some_and(|legacy| !valid_legacy_scrypt(legacy))
    {
        return Err(corrupt());
    }
    Ok(())
}

fn verify_password(phc: &str, password: &str) -> bool {
    let Some(hash) = supported_password_hash(phc) else {
        return false;
    };
    argon2().is_ok_and(|argon| argon.verify_password(password.as_bytes(), &hash).is_ok())
}

fn supported_password_hash(phc: &str) -> Option<PasswordHash<'_>> {
    if phc.len() > MAX_PASSWORD_PHC_BYTES {
        return None;
    }
    let hash = PasswordHash::new(phc).ok()?;
    let params = Params::try_from(&hash).ok()?;
    if hash.algorithm != Algorithm::Argon2id.ident()
        || hash.version != Some(u32::from(Version::V0x13))
        || params.m_cost() != PASSWORD_ARGON2_MEMORY_KIB
        || params.t_cost() != PASSWORD_ARGON2_ITERATIONS
        || params.p_cost() != PASSWORD_ARGON2_PARALLELISM
        || !params.keyid().is_empty()
        || !params.data().is_empty()
        || params.output_len() != Some(PASSWORD_ARGON2_OUTPUT_BYTES)
    {
        return None;
    }
    Some(hash)
}

fn valid_legacy_scrypt(legacy: &LegacyScryptVerifier) -> bool {
    legacy.salt.len() == 32
        && legacy.salt.bytes().all(|byte| byte.is_ascii_hexdigit())
        && legacy.hash.len() == 128
        && legacy.hash.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn verify_legacy_scrypt(password: &str, legacy: &LegacyScryptVerifier) -> bool {
    let Ok(mut expected) = decode_hex(&legacy.hash) else {
        return false;
    };
    if expected.len() != 64 {
        expected.zeroize();
        return false;
    }
    let Ok(params) = scrypt::Params::new(14, 8, 1, 64) else {
        return false;
    };
    let mut derived = [0u8; 64];
    if scrypt::scrypt(
        password.as_bytes(),
        legacy.salt.as_bytes(),
        &params,
        &mut derived,
    )
    .is_err()
    {
        derived.zeroize();
        expected.zeroize();
        return false;
    }
    let verified = bool::from(derived.as_slice().ct_eq(expected.as_slice()));
    derived.zeroize();
    expected.zeroize();
    verified
}

fn decode_hex(value: &str) -> Result<Vec<u8>, ()> {
    if value.len() % 2 != 0 {
        return Err(());
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = hex_digit(pair[0]).ok_or(())?;
            let low = hex_digit(pair[1]).ok_or(())?;
            Ok((high << 4) | low)
        })
        .collect()
}

fn hex_digit(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn validate_password(password: &str) -> Result<(), Problem> {
    validate_password_with_minimum(password, 12)
}

fn validate_password_with_minimum(password: &str, minimum_length: usize) -> Result<(), Problem> {
    let length = password.chars().count();
    if !(minimum_length..=256).contains(&length) {
        return Err(Problem::new(
            "invalid_password",
            format!("Password must contain {minimum_length} to 256 characters"),
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
    use std::time::{Duration, Instant};

    #[test]
    fn eight_character_minimum_is_only_for_local_admin_recovery() {
        assert_eq!(
            validate_password("admin123").unwrap_err().code,
            "invalid_password"
        );
        assert!(validate_password_with_minimum("admin123", 8).is_ok());
        assert!(validate_password("password1234").is_ok());
    }

    fn retry_if_password_capacity_is_busy<T>(
        mut authenticate: impl FnMut() -> Result<T, Problem>,
    ) -> Result<T, Problem> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match authenticate() {
                Err(error)
                    if error.code == "credential_capacity_exhausted"
                        && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(1));
                }
                result => return result,
            }
        }
    }

    fn service() -> (IdentityPersistence, CredentialService) {
        let store = ControlPersistence::new(MemoryControlRecordStore::default());
        (
            IdentityPersistence::new(store.clone()),
            CredentialService::new(store).unwrap(),
        )
    }

    fn seed(identities: &IdentityPersistence, credentials: &CredentialService) -> AccountId {
        let account_id = AccountId::new("acct_login").unwrap();
        let identity = AccountIdentity::new(
            account_id.clone(),
            "Owner",
            "owner@example.com",
            "2026-07-22T00:00:00Z",
        )
        .unwrap()
        .verify(1, "2026-07-22T00:00:01Z")
        .unwrap();
        identities.create(&identity).unwrap();
        credentials
            .set_initial_password(&account_id, "correct horse battery staple", 100)
            .unwrap();
        account_id
    }

    #[test]
    fn pending_identity_cannot_authenticate_and_keeps_generic_failure_shape() {
        let (identities, credentials) = service();
        let account_id = AccountId::new("acct_pending").unwrap();
        identities
            .create(
                &AccountIdentity::new(
                    account_id.clone(),
                    "PendingOwner",
                    "pending@example.com",
                    "2026-07-22T00:00:00Z",
                )
                .unwrap(),
            )
            .unwrap();
        credentials
            .set_initial_password(&account_id, "correct horse battery staple", 100)
            .unwrap();

        let pending_error = retry_if_password_capacity_is_busy(|| {
            credentials.authenticate_versioned(
                "pending@example.com",
                "correct horse battery staple",
                101,
            )
        })
        .unwrap_err();
        let unknown_error = retry_if_password_capacity_is_busy(|| {
            credentials.authenticate_versioned(
                "missing@example.com",
                "correct horse battery staple",
                101,
            )
        })
        .unwrap_err();

        assert_eq!(pending_error.code, "invalid_credentials");
        assert_eq!(pending_error.status, 401);
        assert_eq!(pending_error.code, unknown_error.code);
        assert_eq!(pending_error.title, unknown_error.title);
        assert_eq!(
            credentials
                .store
                .load_versioned::<PasswordRecord>(TENANT, RECORD_TYPE, account_id.as_str())
                .unwrap()
                .unwrap()
                .1,
            1,
            "a rejected pending identity must not record successful login or upgrade credentials"
        );
    }

    #[test]
    fn admin_provisioned_identity_can_login_without_claiming_email_verified() {
        let (identities, credentials) = service();
        let account_id = AccountId::new("acct_admin_provisioned").unwrap();
        let identity = AccountIdentity::admin_provisioned(
            account_id.clone(),
            "provisioned",
            "provisioned@example.com",
            "2026-10-24T00:00:00Z",
        )
        .unwrap();
        identities.create(&identity).unwrap();
        credentials
            .set_initial_password(&account_id, "provisioned-password-123", 100)
            .unwrap();

        assert_eq!(
            retry_if_password_capacity_is_busy(|| {
                credentials.authenticate("provisioned@example.com", "provisioned-password-123", 101)
            })
            .unwrap(),
            account_id
        );
        assert_eq!(
            identities.load(&account_id).unwrap().unwrap().verification,
            IdentityVerification::AdminProvisioned
        );
        assert_eq!(
            identity
                .verify(1, "2026-10-24T00:01:00Z")
                .unwrap()
                .verification,
            IdentityVerification::Verified
        );
        assert_eq!(
            credentials
                .authenticate("provisioned", "wrong-password-123", 102)
                .unwrap_err()
                .code,
            "invalid_credentials"
        );
    }

    #[test]
    fn concurrent_password_attempts_get_distinct_persistence_idempotency_keys() {
        let first = credential_idempotency_key("credential.login", "acct_login", "attempt-0001", 7);
        let second =
            credential_idempotency_key("credential.login", "acct_login", "attempt-0002", 7);
        let retry = credential_idempotency_key("credential.login", "acct_login", "attempt-0001", 7);

        assert_ne!(first, second);
        assert_eq!(first, retry);
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
            retry_if_password_capacity_is_busy(|| {
                credentials.authenticate("OWNER", "correct horse battery staple", 101)
            })
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
            retry_if_password_capacity_is_busy(|| {
                credentials.authenticate("Owner@Example.COM", "correct horse battery staple", 102)
            })
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
    fn rejects_argon2_phc_with_untrusted_work_factor_before_verification() {
        let phc = "$argon2id$v=19$m=4294967295,t=2,p=1$c2FsdHNhbHRzYWx0c2FsdA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
        assert!(!verify_password(phc, "not the password"));
        let phc_with_unissued_data = "$argon2id$v=19$m=19456,t=2,p=1,data=YQ$c2FsdHNhbHRzYWx0c2FsdA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
        assert!(!verify_password(phc_with_unissued_data, "not the password"));
        assert!(!verify_password(
            &"x".repeat(MAX_PASSWORD_PHC_BYTES + 1),
            "not the password"
        ));
    }

    #[test]
    fn rejects_password_record_bound_to_another_account() {
        let (identities, credentials) = service();
        let account_id = seed(&identities, &credentials);
        let (mut record, _) = credentials
            .store
            .load_versioned::<PasswordRecord>(TENANT, RECORD_TYPE, account_id.as_str())
            .unwrap()
            .unwrap();
        record.account_id = AccountId::new("acct_other_owner").unwrap();

        assert_eq!(
            validate_password_record_owner(&record, &account_id)
                .unwrap_err()
                .code,
            "credential_record_corrupt"
        );
    }

    #[test]
    fn imported_node_scrypt_credential_upgrades_to_argon2id_on_login() {
        let (identities, credentials) = service();
        let account_id = seed(&identities, &credentials);
        let password = "legacy password with enough length";
        let salt = "0123456789abcdef0123456789abcdef";
        // Fixed vector generated by Node crypto.scryptSync(password, salt, 64),
        // matching the 1.0 control-plane implementation.
        let hash = "7608d2f3bd3ac7a04908f236fa2fc691443ab546284b4989e65f2941124bcb9bcd8da5efba4e322495a59f5397af72b19b5cad7bcfae76b36aa786984fc9fd7e";
        let imported = credentials
            .prepare_legacy_scrypt_put(&account_id, salt, &hash, 100)
            .unwrap();
        credentials
            .persist(
                &serde_json::from_value(imported.value).unwrap(),
                1,
                "migration.test.import",
                "imported",
                "migration-test-operation",
            )
            .unwrap();

        assert_eq!(
            retry_if_password_capacity_is_busy(|| {
                credentials.authenticate_versioned("owner@example.com", password, 101)
            })
            .unwrap(),
            (account_id.clone(), 1)
        );
        let (record, _) = credentials
            .store
            .load_versioned::<PasswordRecord>(TENANT, RECORD_TYPE, account_id.as_str())
            .unwrap()
            .unwrap();
        assert!(record.legacy_scrypt.is_none());
        assert!(
            record
                .password_phc
                .as_deref()
                .unwrap()
                .starts_with("$argon2id$")
        );
        assert_eq!(record.password_version, 1);
        assert_eq!(
            retry_if_password_capacity_is_busy(|| {
                credentials.authenticate("owner@example.com", password, 102)
            })
            .unwrap(),
            account_id
        );
    }

    #[test]
    fn locks_after_repeated_failures_and_recovers_after_window() {
        let (identities, credentials) = service();
        let account_id = seed(&identities, &credentials);
        for now in 101..106 {
            assert_eq!(
                retry_if_password_capacity_is_busy(|| {
                    credentials.authenticate("owner", "wrong password value", now)
                })
                .unwrap_err()
                .code,
                "invalid_credentials"
            );
        }
        assert_eq!(
            retry_if_password_capacity_is_busy(|| {
                credentials.authenticate("owner", "correct horse battery staple", 106)
            })
            .unwrap_err()
            .code,
            "invalid_credentials"
        );
        assert_eq!(
            retry_if_password_capacity_is_busy(|| {
                credentials.authenticate(
                    "owner",
                    "correct horse battery staple",
                    105 + LOCK_SECONDS,
                )
            })
            .unwrap(),
            account_id
        );
    }

    #[test]
    fn expired_lock_starts_a_fresh_failure_window() {
        let (identities, credentials) = service();
        let account_id = seed(&identities, &credentials);
        for now in 101..106 {
            let _ = retry_if_password_capacity_is_busy(|| {
                credentials.authenticate("owner", "wrong password value", now)
            });
        }
        let after_lock = 105 + LOCK_SECONDS;
        assert_eq!(
            retry_if_password_capacity_is_busy(|| {
                credentials.authenticate("owner", "wrong password value", after_lock)
            })
            .unwrap_err()
            .code,
            "invalid_credentials"
        );
        assert_eq!(
            retry_if_password_capacity_is_busy(|| {
                credentials.authenticate("owner", "correct horse battery staple", after_lock + 1)
            })
            .unwrap(),
            account_id
        );
    }

    #[test]
    fn unknown_identity_returns_same_public_error() {
        let (_, credentials) = service();
        assert_eq!(
            retry_if_password_capacity_is_busy(|| {
                credentials.authenticate("nobody", "wrong password value", 100)
            })
            .unwrap_err()
            .code,
            "invalid_credentials"
        );
    }

    #[test]
    fn password_verification_admission_fails_fast_when_saturated() {
        let limiter = Semaphore::new(1);
        let occupied = limiter.try_acquire().unwrap();
        let error = acquire_password_verification_slot(&limiter).unwrap_err();
        assert_eq!(error.code, "credential_capacity_exhausted");
        assert_eq!(error.status, 503);
        assert!(error.retryable);
        drop(occupied);
        assert!(acquire_password_verification_slot(&limiter).is_ok());
    }
}
