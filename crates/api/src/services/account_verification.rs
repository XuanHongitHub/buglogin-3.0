//! One-time email-verification challenges for pending account identities.
//!
//! Delivery is intentionally outside this service. Callers must send the returned secret only
//! to `destination`, never return it to an administrator or include it in an HTTP receipt/log.

use std::collections::BTreeMap;

use buglogin_domain::{
    account::{Account, AccountIdentity, AccountStatus, IdentityVerification},
    errors::Problem,
    ids::AccountId,
};
use buglogin_storage::control::{
    ControlAuditEntry, ControlDelete, ControlPut, ControlRecordKey, ExecuteControlCommand,
    HttpResponse,
};
use getrandom::fill;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, Zeroizing};

use super::{control_persistence::ControlPersistence, credentials::CredentialService};

const TENANT: &str = "identity";
const CONTROL_TENANT: &str = "control";
const CHALLENGE_RECORD: &str = "account_verification_challenge";
const TOKEN_INDEX_RECORD: &str = "account_verification_token";
const TOKEN_TTL_SECONDS: u64 = 24 * 60 * 60;
const TOKEN_BYTES: usize = 32;

/// Secret-bearing handoff to a configured email delivery implementation.
///
/// This type deliberately does not implement `Debug`, `Serialize`, or `Deserialize`.
pub struct AccountVerificationDelivery {
    pub account_id: AccountId,
    pub destination: String,
    pub token: Zeroizing<String>,
    pub expires_at: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct VerificationChallenge {
    account_id: AccountId,
    token_hash: String,
    created_at: u64,
    expires_at: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct VerificationTokenIndex {
    account_id: AccountId,
}

#[derive(Clone)]
pub struct AccountVerificationService {
    store: ControlPersistence,
    credentials: CredentialService,
}

impl AccountVerificationService {
    pub fn new(store: ControlPersistence, credentials: CredentialService) -> Self {
        Self { store, credentials }
    }

    /// Create or replace the account's current verification challenge.
    ///
    /// The token is persisted only as a SHA-256 digest. The caller must deliver it over a
    /// separately configured, trusted email channel and must not return it to the administrator.
    pub fn issue(
        &self,
        account_id: &AccountId,
        actor_id: &AccountId,
        now: u64,
    ) -> Result<AccountVerificationDelivery, Problem> {
        let expires_at = now
            .checked_add(TOKEN_TTL_SECONDS)
            .ok_or_else(invalid_time)?;
        let (account, account_revision) = self
            .store
            .load_versioned::<Account>(CONTROL_TENANT, "account", account_id.as_str())?
            .ok_or_else(identity_not_found)?;
        if account.id != *account_id || account.status != AccountStatus::Active {
            return Err(account_unavailable());
        }
        let (identity, identity_revision) = self
            .store
            .load_versioned::<AccountIdentity>(TENANT, "account_identity", account_id.as_str())?
            .ok_or_else(identity_not_found)?;
        if identity.account_id != *account_id || identity.revision != identity_revision {
            return Err(corrupt());
        }
        if identity.verification == IdentityVerification::Verified {
            return Err(identity_already_verified());
        }

        let current = self.store.load_versioned::<VerificationChallenge>(
            TENANT,
            CHALLENGE_RECORD,
            account_id.as_str(),
        )?;
        let expected_challenge_revision = current.as_ref().map_or(0, |(_, revision)| *revision);
        let mut deletes = Vec::new();
        if let Some((challenge, _)) = current.as_ref() {
            if challenge.account_id != *account_id {
                return Err(corrupt());
            }
            let (index, index_revision) = self
                .store
                .load_versioned::<VerificationTokenIndex>(
                    TENANT,
                    TOKEN_INDEX_RECORD,
                    &challenge.token_hash,
                )?
                .ok_or_else(corrupt)?;
            if index.account_id != *account_id {
                return Err(corrupt());
            }
            deletes.push(ControlDelete {
                tenant_id: TENANT.into(),
                record_type: TOKEN_INDEX_RECORD.into(),
                record_id: challenge.token_hash.clone(),
                expected_revision: Some(index_revision),
            });
        }

        let mut bytes = Zeroizing::new([0_u8; TOKEN_BYTES]);
        fill(bytes.as_mut()).map_err(|_| unavailable())?;
        let token = Zeroizing::new(hex(bytes.as_ref()));
        bytes.zeroize();
        let token_hash = digest_hex(token.as_bytes());
        let challenge = VerificationChallenge {
            account_id: account_id.clone(),
            token_hash: token_hash.clone(),
            created_at: now,
            expires_at,
        };
        let index = VerificationTokenIndex {
            account_id: account_id.clone(),
        };
        let mut expected_revisions = BTreeMap::new();
        expected_revisions.insert(
            record_key("account_identity", account_id.as_str()),
            identity_revision,
        );
        expected_revisions.insert(
            record_key_for(CONTROL_TENANT, "account", account_id.as_str()),
            account_revision,
        );
        expected_revisions.insert(
            record_key(CHALLENGE_RECORD, account_id.as_str()),
            expected_challenge_revision,
        );
        let idempotency_key = format!("account-verification-issue-{token_hash}");
        self.store.execute(ExecuteControlCommand {
            tenant_id: TENANT.into(),
            command_id: "account.verification.issue".into(),
            idempotency_key,
            expected_revisions,
            puts: vec![
                put(
                    CHALLENGE_RECORD,
                    account_id.as_str(),
                    Some(expected_challenge_revision),
                    &challenge,
                )?,
                put(TOKEN_INDEX_RECORD, &token_hash, Some(0), &index)?,
            ],
            deletes,
            audit: ControlAuditEntry {
                tenant_id: TENANT.into(),
                command_id: "account.verification.issue".into(),
                actor_id: actor_id.as_str().into(),
                allowed: true,
                detail: json!({"accountId": account_id, "state": "issued", "expiresAt": expires_at}),
            },
            response: HttpResponse {
                status: 202,
                headers: Default::default(),
                body: json!({"accountId": account_id, "expiresAt": expires_at}),
            },
        })?;

        Ok(AccountVerificationDelivery {
            account_id: account_id.clone(),
            destination: identity.email,
            token,
            expires_at,
        })
    }

    /// Verify the recipient and create their first password in one durable control command.
    pub fn complete(
        &self,
        token: &str,
        password: &str,
        now: u64,
        updated_at: &str,
    ) -> Result<AccountId, Problem> {
        if token.len() != TOKEN_BYTES * 2
            || !token
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(invalid_token());
        }
        let token_hash = digest_hex(token.as_bytes());
        let (index, index_revision) = self
            .store
            .load_versioned::<VerificationTokenIndex>(TENANT, TOKEN_INDEX_RECORD, &token_hash)?
            .ok_or_else(invalid_token)?;
        let (challenge, challenge_revision) = self
            .store
            .load_versioned::<VerificationChallenge>(
                TENANT,
                CHALLENGE_RECORD,
                index.account_id.as_str(),
            )?
            .ok_or_else(invalid_token)?;
        if challenge.account_id != index.account_id || challenge.token_hash != token_hash {
            return Err(invalid_token());
        }
        if now >= challenge.expires_at {
            return Err(invalid_token());
        }
        let (account, account_revision) = self
            .store
            .load_versioned::<Account>(CONTROL_TENANT, "account", index.account_id.as_str())?
            .ok_or_else(identity_not_found)?;
        if account.id != index.account_id || account.status != AccountStatus::Active {
            return Err(account_unavailable());
        }
        let (identity, identity_revision) = self
            .store
            .load_versioned::<AccountIdentity>(
                TENANT,
                "account_identity",
                index.account_id.as_str(),
            )?
            .ok_or_else(identity_not_found)?;
        if identity.account_id != index.account_id || identity.revision != identity_revision {
            return Err(corrupt());
        }
        if !matches!(
            identity.verification,
            IdentityVerification::Pending | IdentityVerification::AdminProvisioned
        ) {
            return Err(invalid_token());
        }

        let verified = identity.verify(identity.revision, updated_at)?;
        let identity_put = put(
            "account_identity",
            index.account_id.as_str(),
            Some(identity_revision),
            &verified,
        )?;
        let password_put = match identity.verification {
            IdentityVerification::Pending => {
                self.credentials
                    .prepare_initial_password_put(&index.account_id, password, now)?
            }
            IdentityVerification::AdminProvisioned => {
                self.credentials
                    .password_reset_put(&index.account_id, password, now)?
            }
            IdentityVerification::Verified => return Err(invalid_token()),
        };
        let mut expected_revisions = BTreeMap::new();
        expected_revisions.insert(
            record_key("account_identity", index.account_id.as_str()),
            identity_revision,
        );
        expected_revisions.insert(
            record_key_for(CONTROL_TENANT, "account", index.account_id.as_str()),
            account_revision,
        );
        expected_revisions.insert(
            record_key(CHALLENGE_RECORD, index.account_id.as_str()),
            challenge_revision,
        );
        expected_revisions.insert(record_key(TOKEN_INDEX_RECORD, &token_hash), index_revision);
        let command_key = format!("account-verification-complete-{token_hash}");
        self.store.execute(ExecuteControlCommand {
            tenant_id: TENANT.into(),
            command_id: "account.verification.complete".into(),
            idempotency_key: command_key,
            expected_revisions,
            puts: vec![identity_put, password_put],
            deletes: vec![
                ControlDelete {
                    tenant_id: TENANT.into(),
                    record_type: CHALLENGE_RECORD.into(),
                    record_id: index.account_id.as_str().into(),
                    expected_revision: Some(challenge_revision),
                },
                ControlDelete {
                    tenant_id: TENANT.into(),
                    record_type: TOKEN_INDEX_RECORD.into(),
                    record_id: token_hash,
                    expected_revision: Some(index_revision),
                },
            ],
            audit: ControlAuditEntry {
                tenant_id: TENANT.into(),
                command_id: "account.verification.complete".into(),
                actor_id: "anonymous".into(),
                allowed: true,
                detail: json!({"accountId": index.account_id, "state": "verified_and_credentialed"}),
            },
            response: HttpResponse {
                status: 204,
                headers: Default::default(),
                body: json!({"accountId": index.account_id, "verified": true}),
            },
        })?;

        Ok(index.account_id)
    }

    /// Remove a challenge when its delivery provider definitively reports failure.
    pub fn revoke_delivery(
        &self,
        account_id: &AccountId,
        token: &str,
        actor_id: &AccountId,
        now: u64,
    ) -> Result<(), Problem> {
        if token.len() != TOKEN_BYTES * 2
            || !token
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(invalid_token());
        }
        let token_hash = digest_hex(token.as_bytes());
        let (index, index_revision) = self
            .store
            .load_versioned::<VerificationTokenIndex>(TENANT, TOKEN_INDEX_RECORD, &token_hash)?
            .ok_or_else(invalid_token)?;
        let (challenge, challenge_revision) = self
            .store
            .load_versioned::<VerificationChallenge>(TENANT, CHALLENGE_RECORD, account_id.as_str())?
            .ok_or_else(invalid_token)?;
        if index.account_id != *account_id || challenge.token_hash != token_hash {
            return Err(invalid_token());
        }
        self.store.execute(ExecuteControlCommand {
            tenant_id: TENANT.into(),
            command_id: "account.verification.delivery_failed".into(),
            idempotency_key: format!("account-verification-delivery-failed-{token_hash}"),
            expected_revisions: BTreeMap::from([
                (
                    record_key(CHALLENGE_RECORD, account_id.as_str()),
                    challenge_revision,
                ),
                (record_key(TOKEN_INDEX_RECORD, &token_hash), index_revision),
            ]),
            puts: vec![],
            deletes: vec![
                ControlDelete {
                    tenant_id: TENANT.into(),
                    record_type: CHALLENGE_RECORD.into(),
                    record_id: account_id.as_str().into(),
                    expected_revision: Some(challenge_revision),
                },
                ControlDelete {
                    tenant_id: TENANT.into(),
                    record_type: TOKEN_INDEX_RECORD.into(),
                    record_id: token_hash,
                    expected_revision: Some(index_revision),
                },
            ],
            audit: ControlAuditEntry {
                tenant_id: TENANT.into(),
                command_id: "account.verification.delivery_failed".into(),
                actor_id: actor_id.as_str().into(),
                allowed: true,
                detail: json!({"accountId": account_id, "state": "challenge_revoked", "at": now}),
            },
            response: HttpResponse {
                status: 204,
                headers: Default::default(),
                body: Value::Null,
            },
        })?;
        Ok(())
    }
}

fn put<T: Serialize>(
    record_type: &str,
    record_id: &str,
    expected_revision: Option<u64>,
    value: &T,
) -> Result<ControlPut, Problem> {
    Ok(ControlPut {
        tenant_id: TENANT.into(),
        record_type: record_type.into(),
        record_id: record_id.into(),
        expected_revision,
        value: serde_json::to_value(value).map_err(|_| corrupt())?,
    })
}

fn record_key(record_type: &str, record_id: &str) -> ControlRecordKey {
    record_key_for(TENANT, record_type, record_id)
}

fn record_key_for(tenant_id: &str, record_type: &str, record_id: &str) -> ControlRecordKey {
    ControlRecordKey {
        tenant_id: tenant_id.into(),
        record_type: record_type.into(),
        record_id: record_id.into(),
    }
}

fn digest_hex(value: &[u8]) -> String {
    hex(&Sha256::digest(value))
}

fn hex(value: &[u8]) -> String {
    value.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn identity_not_found() -> Problem {
    Problem::new(
        "account_identity_not_found",
        "Account identity was not found",
        404,
        "identity",
    )
}

fn identity_already_verified() -> Problem {
    Problem::new(
        "account_identity_already_verified",
        "Account identity is already verified",
        409,
        "identity",
    )
}

fn account_unavailable() -> Problem {
    Problem::new(
        "account_unavailable",
        "Account is unavailable",
        409,
        "identity",
    )
}

fn invalid_token() -> Problem {
    Problem::new(
        "invalid_account_verification_token",
        "Verification token is invalid or expired",
        400,
        "identity",
    )
}

fn invalid_time() -> Problem {
    Problem::new(
        "account_verification_time_invalid",
        "Verification expiry cannot be represented",
        400,
        "identity",
    )
}

fn unavailable() -> Problem {
    Problem::new(
        "account_verification_unavailable",
        "Account verification is unavailable",
        503,
        "identity",
    )
    .retryable(true)
}

fn corrupt() -> Problem {
    Problem::new(
        "account_verification_record_corrupt",
        "Account verification record is corrupt",
        500,
        "identity",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::{
        account_provisioning::AccountProvisioningService, credentials::CredentialService,
    };
    use buglogin_storage::control::MemoryControlRecordStore;

    fn retry_if_password_capacity_is_busy<T>(
        mut authenticate: impl FnMut() -> Result<T, Problem>,
    ) -> Result<T, Problem> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            match authenticate() {
                Err(error)
                    if error.code == "credential_capacity_exhausted"
                        && std::time::Instant::now() < deadline =>
                {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                result => return result,
            }
        }
    }

    fn fixture() -> (
        ControlPersistence,
        AccountVerificationService,
        AccountId,
        AccountId,
    ) {
        let store = ControlPersistence::new(MemoryControlRecordStore::default());
        let actor = AccountId::new("acct_platform_admin").unwrap();
        let account_id = AccountId::new("acct_verify_user").unwrap();
        AccountProvisioningService::new(store.clone())
            .create_pending(
                account_id.clone(),
                "verifyuser",
                "verify-user@example.com",
                &actor,
                "account-create-verify-01",
                "2026-10-23T00:00:00Z",
            )
            .unwrap();
        let credentials = CredentialService::new(store.clone()).unwrap();
        (
            store.clone(),
            AccountVerificationService::new(store, credentials),
            account_id,
            actor,
        )
    }

    #[test]
    fn pending_identity_cannot_login_until_email_verification_and_first_password_commit() {
        let (store, verification, account_id, actor) = fixture();
        let credentials = CredentialService::new(store.clone()).unwrap();
        assert_eq!(
            retry_if_password_capacity_is_busy(|| {
                credentials.authenticate(
                    "verify-user@example.com",
                    "correct horse battery staple",
                    10,
                )
            })
            .unwrap_err()
            .code,
            "invalid_credentials"
        );

        let delivery = verification.issue(&account_id, &actor, 100).unwrap();
        assert_eq!(delivery.destination, "verify-user@example.com");
        assert_eq!(delivery.expires_at, 100 + TOKEN_TTL_SECONDS);
        verification
            .complete(
                &delivery.token,
                "correct horse battery staple",
                101,
                "2026-10-23T00:01:41Z",
            )
            .unwrap();

        assert_eq!(
            retry_if_password_capacity_is_busy(|| {
                credentials.authenticate(
                    "verify-user@example.com",
                    "correct horse battery staple",
                    102,
                )
            })
            .unwrap(),
            account_id
        );
        let identity = store
            .load::<AccountIdentity>(TENANT, "account_identity", account_id.as_str())
            .unwrap()
            .unwrap();
        assert_eq!(identity.verification, IdentityVerification::Verified);
        assert!(
            store
                .load::<Value>(TENANT, CHALLENGE_RECORD, account_id.as_str())
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn admin_provisioned_identity_can_verify_email_and_replace_initial_password() {
        let (store, verification, account_id, actor) = fixture();
        let (mut identity, revision) = store
            .load_versioned::<AccountIdentity>(TENANT, "account_identity", account_id.as_str())
            .unwrap()
            .unwrap();
        identity.verification = IdentityVerification::AdminProvisioned;
        identity.revision = identity.revision.checked_add(1).unwrap();
        store
            .execute(ExecuteControlCommand {
                tenant_id: TENANT.into(),
                command_id: "test.account.admin-provisioned".into(),
                idempotency_key: "test-account-admin-provisioned".into(),
                expected_revisions: BTreeMap::new(),
                puts: vec![
                    put(
                        "account_identity",
                        account_id.as_str(),
                        Some(revision),
                        &identity,
                    )
                    .unwrap(),
                ],
                deletes: vec![],
                audit: ControlAuditEntry {
                    tenant_id: TENANT.into(),
                    command_id: "test.account.admin-provisioned".into(),
                    actor_id: actor.as_str().into(),
                    allowed: true,
                    detail: json!({}),
                },
                response: HttpResponse {
                    status: 200,
                    headers: Default::default(),
                    body: Value::Null,
                },
            })
            .unwrap();
        let credentials = CredentialService::new(store.clone()).unwrap();
        credentials
            .set_initial_password(&account_id, "admin-provided-password-123", 100)
            .unwrap();

        let challenge = verification.issue(&account_id, &actor, 101).unwrap();
        verification
            .complete(
                &challenge.token,
                "user-chosen-password-456",
                102,
                "2026-10-23T00:01:42Z",
            )
            .unwrap();
        assert_eq!(
            retry_if_password_capacity_is_busy(|| {
                credentials.authenticate("verify-user@example.com", "user-chosen-password-456", 103)
            })
            .unwrap(),
            account_id
        );
        assert_eq!(
            retry_if_password_capacity_is_busy(|| {
                credentials.authenticate(
                    "verify-user@example.com",
                    "admin-provided-password-123",
                    104,
                )
            })
            .unwrap_err()
            .code,
            "invalid_credentials"
        );
        assert_eq!(
            store
                .load::<AccountIdentity>(TENANT, "account_identity", account_id.as_str())
                .unwrap()
                .unwrap()
                .verification,
            IdentityVerification::Verified
        );
    }

    #[test]
    fn expired_verification_token_does_not_verify_or_create_credentials() {
        let (store, verification, account_id, actor) = fixture();
        let delivery = verification.issue(&account_id, &actor, 100).unwrap();
        let error = verification
            .complete(
                &delivery.token,
                "correct horse battery staple",
                100 + TOKEN_TTL_SECONDS,
                "2026-10-24T00:00:00Z",
            )
            .unwrap_err();
        assert_eq!(error.code, "invalid_account_verification_token");
        assert_eq!(
            store
                .load::<AccountIdentity>(TENANT, "account_identity", account_id.as_str())
                .unwrap()
                .unwrap()
                .verification,
            IdentityVerification::Pending
        );
        assert!(
            store
                .load::<Value>(TENANT, "account_password", account_id.as_str())
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn verification_token_is_single_use_and_reissued_token_invalidates_previous_token() {
        let (_store, verification, account_id, actor) = fixture();
        let first = verification.issue(&account_id, &actor, 100).unwrap();
        let second = verification.issue(&account_id, &actor, 101).unwrap();
        assert_ne!(*first.token, *second.token);
        assert_eq!(
            verification
                .complete(
                    &first.token,
                    "correct horse battery staple",
                    102,
                    "2026-10-23T00:01:42Z"
                )
                .unwrap_err()
                .code,
            "invalid_account_verification_token"
        );
        verification
            .complete(
                &second.token,
                "correct horse battery staple",
                103,
                "2026-10-23T00:01:43Z",
            )
            .unwrap();
        assert_eq!(
            verification
                .complete(
                    &second.token,
                    "different password phrase",
                    104,
                    "2026-10-23T00:01:44Z"
                )
                .unwrap_err()
                .code,
            "invalid_account_verification_token"
        );
    }

    #[test]
    fn invalid_password_leaves_verification_pending_and_token_usable() {
        let (store, verification, account_id, actor) = fixture();
        let delivery = verification.issue(&account_id, &actor, 100).unwrap();
        assert_eq!(
            verification
                .complete(&delivery.token, "abc", 101, "2026-10-23T00:01:41Z")
                .unwrap_err()
                .code,
            "invalid_password"
        );
        assert_eq!(
            store
                .load::<AccountIdentity>(TENANT, "account_identity", account_id.as_str())
                .unwrap()
                .unwrap()
                .verification,
            IdentityVerification::Pending
        );
        verification
            .complete(
                &delivery.token,
                "correct horse battery staple",
                102,
                "2026-10-23T00:01:42Z",
            )
            .unwrap();
    }

    #[test]
    fn credential_conflict_rolls_back_identity_verification_and_challenge_consumption() {
        let (store, verification, account_id, actor) = fixture();
        let credentials = CredentialService::new(store.clone()).unwrap();
        credentials
            .set_initial_password(&account_id, "existing password phrase", 99)
            .unwrap();
        let delivery = verification.issue(&account_id, &actor, 100).unwrap();

        assert_eq!(
            verification
                .complete(
                    &delivery.token,
                    "new password phrase",
                    101,
                    "2026-10-23T00:01:41Z"
                )
                .unwrap_err()
                .code,
            "storage_revision_conflict"
        );
        assert_eq!(
            store
                .load::<AccountIdentity>(TENANT, "account_identity", account_id.as_str())
                .unwrap()
                .unwrap()
                .verification,
            IdentityVerification::Pending
        );
        assert!(
            store
                .load::<Value>(TENANT, CHALLENGE_RECORD, account_id.as_str())
                .unwrap()
                .is_some()
        );
        assert!(
            verification
                .complete(
                    &delivery.token,
                    "new password phrase",
                    102,
                    "2026-10-23T00:01:42Z"
                )
                .is_err()
        );
        assert_eq!(
            retry_if_password_capacity_is_busy(|| {
                credentials.authenticate("verify-user@example.com", "existing password phrase", 103)
            })
            .unwrap_err()
            .code,
            "invalid_credentials"
        );
    }
}
