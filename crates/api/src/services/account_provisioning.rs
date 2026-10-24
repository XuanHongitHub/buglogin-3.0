//! Safe account onboarding for administrator-driven provisioning.
//!
//! This creates an active account record with a `Pending` identity only. It deliberately
//! accepts no password, issues no session/token, and does not claim the account can log in.
//! Credential authentication rejects pending identities until a supported verification flow
//! marks the identity verified.

use buglogin_domain::{
    account::{Account, AccountIdentity, AccountStatus, IdentityVerification},
    errors::Problem,
    ids::{AccountId, MemberId, WorkspaceId},
    member::MemberRole,
};
use buglogin_storage::control::{ControlAuditEntry, ExecuteControlCommand, HttpResponse};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{control_persistence::ControlPersistence, identity_persistence::IdentityPersistence};

const CONTROL_TENANT: &str = "control";

/// Safe projection returned by pending account provisioning.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingAccountProvisioning {
    pub account: Account,
    pub identity: AccountIdentity,
    /// True when the request reused a previously committed idempotency key.
    pub replayed: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct StoredPendingAccountProvisioning {
    account: Account,
    identity: AccountIdentity,
    #[serde(default)]
    workspace_id: Option<WorkspaceId>,
    #[serde(default)]
    requested_role: Option<MemberRole>,
}

#[derive(Clone)]
pub struct AccountProvisioningService {
    store: ControlPersistence,
    identities: IdentityPersistence,
}

impl AccountProvisioningService {
    pub fn new(store: ControlPersistence) -> Self {
        Self {
            identities: IdentityPersistence::new(store.clone()),
            store,
        }
    }

    /// Atomically create a pending account and its canonical identity indexes.
    ///
    /// The caller must authorize the actor before invoking this method. `idempotency_key` is
    /// scoped by `actor_id`; `created_at` is accepted as an operation input so callers can
    /// preserve it when retrying an interrupted request. The password and session lifecycle
    /// are intentionally separate from this operation.
    pub fn create_pending(
        &self,
        account_id: AccountId,
        username: &str,
        email: &str,
        actor_id: &AccountId,
        idempotency_key: &str,
        created_at: &str,
    ) -> Result<PendingAccountProvisioning, Problem> {
        self.create_pending_in_scope(
            account_id,
            username,
            email,
            actor_id,
            idempotency_key,
            created_at,
            None,
        )
    }

    pub fn create_pending_scoped(
        &self,
        account_id: AccountId,
        username: &str,
        email: &str,
        actor_id: &AccountId,
        idempotency_key: &str,
        created_at: &str,
        workspace_id: &WorkspaceId,
        requested_role: &MemberRole,
    ) -> Result<PendingAccountProvisioning, Problem> {
        self.create_pending_in_scope(
            account_id,
            username,
            email,
            actor_id,
            idempotency_key,
            created_at,
            Some((workspace_id, requested_role)),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn create_pending_in_scope(
        &self,
        account_id: AccountId,
        username: &str,
        email: &str,
        actor_id: &AccountId,
        idempotency_key: &str,
        created_at: &str,
        scope: Option<(&WorkspaceId, &MemberRole)>,
    ) -> Result<PendingAccountProvisioning, Problem> {
        validate_idempotency_key(idempotency_key)?;
        let identity = AccountIdentity::new(account_id.clone(), username, email, created_at)?;
        let account = Account::new(account_id.clone(), created_at);
        let stored_key = scoped_idempotency_key(actor_id, idempotency_key);
        let workspace_id = scope.map(|(workspace_id, _)| workspace_id.clone());
        let requested_role = scope.map(|(_, role)| role.clone());

        if let Some(replay) = self.store.replay(CONTROL_TENANT, &stored_key)? {
            return replay_pending(
                replay.response.body,
                &account,
                &identity,
                &workspace_id,
                &requested_role,
                true,
            );
        }

        if self
            .store
            .load::<Account>(CONTROL_TENANT, "account", account_id.as_str())?
            .is_some()
        {
            return Err(account_exists());
        }
        let mut puts = self.identities.prepare_create_puts(&identity)?;
        puts.push(buglogin_storage::control::ControlPut {
            tenant_id: CONTROL_TENANT.into(),
            record_type: "account".into(),
            record_id: account_id.as_str().into(),
            expected_revision: Some(0),
            value: serde_json::to_value(&account).map_err(|_| corrupt())?,
        });

        let response_body = serde_json::to_value(StoredPendingAccountProvisioning {
            account: account.clone(),
            identity: identity.clone(),
            workspace_id: workspace_id.clone(),
            requested_role: requested_role.clone(),
        })
        .map_err(|_| corrupt())?;
        let command = ExecuteControlCommand {
            tenant_id: CONTROL_TENANT.into(),
            command_id: "account.provision.pending".into(),
            idempotency_key: stored_key.clone(),
            expected_revisions: Default::default(),
            puts,
            deletes: vec![],
            audit: ControlAuditEntry {
                tenant_id: CONTROL_TENANT.into(),
                command_id: "account.provision.pending".into(),
                actor_id: actor_id.as_str().into(),
                allowed: true,
                detail: json!({
                    "accountId": account_id,
                    "identityVerification": "pending",
                    "credentialIssued": false,
                    "sessionIssued": false,
                    "workspaceId": workspace_id,
                    "requestedRole": requested_role,
                }),
            },
            response: HttpResponse {
                status: 201,
                headers: Default::default(),
                body: response_body,
            },
        };

        match self.store.execute(command) {
            Ok(result) => replay_pending(
                result.response.body,
                &account,
                &identity,
                &workspace_id,
                &requested_role,
                result.replayed,
            ),
            Err(error) => {
                // A concurrent retry can reach the record CAS before the idempotency replay
                // becomes visible. Only return the winner when its persisted request matches.
                if let Some(replay) = self.store.replay(CONTROL_TENANT, &stored_key)? {
                    return replay_pending(
                        replay.response.body,
                        &account,
                        &identity,
                        &workspace_id,
                        &requested_role,
                        true,
                    );
                }
                Err(error)
            }
        }
    }
}

/// Stable identifiers let a retry finish the membership step if identity creation
/// succeeded but the API response was lost.
pub fn scoped_account_idempotency_key(
    workspace_id: &WorkspaceId,
    account_id: &AccountId,
) -> String {
    let digest = Sha256::digest(format!("{}:{}", workspace_id.as_str(), account_id.as_str()));
    format!("scoped-account-{}", hex_digest(&digest))
}

pub fn scoped_member_id(
    workspace_id: &WorkspaceId,
    account_id: &AccountId,
) -> Result<MemberId, Problem> {
    let digest = Sha256::digest(format!("{}:{}", workspace_id.as_str(), account_id.as_str()));
    MemberId::new(format!("mem_{}", hex_digest(&digest[..16]))).map_err(|_| {
        Problem::new(
            "account_provisioning_member_id_invalid",
            "Account provisioning member ID could not be generated",
            500,
            "identity",
        )
    })
}

fn hex_digest(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn replay_pending(
    body: Value,
    expected_account: &Account,
    expected_identity: &AccountIdentity,
    expected_workspace_id: &Option<WorkspaceId>,
    expected_role: &Option<MemberRole>,
    replayed: bool,
) -> Result<PendingAccountProvisioning, Problem> {
    let stored: StoredPendingAccountProvisioning =
        serde_json::from_value(body).map_err(|_| corrupt())?;
    // Ignore timestamps when comparing a retry; the server-generated creation time can differ
    // between attempts. All caller-controlled account and identity fields must match.
    let same_identity = stored.identity.account_id == expected_identity.account_id
        && stored.identity.username == expected_identity.username
        && stored.identity.canonical_username == expected_identity.canonical_username
        && stored.identity.email == expected_identity.email
        && stored.identity.canonical_email == expected_identity.canonical_email;
    if stored.workspace_id != *expected_workspace_id
        || stored.requested_role != *expected_role
        || stored.account.id != expected_account.id
        || stored.account.status != AccountStatus::Active
        || stored.identity.verification != IdentityVerification::Pending
        || !same_identity
    {
        return Err(idempotency_conflict());
    }
    Ok(PendingAccountProvisioning {
        account: stored.account,
        identity: stored.identity,
        replayed,
    })
}

fn validate_idempotency_key(value: &str) -> Result<(), Problem> {
    if !(8..=128).contains(&value.len())
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
    {
        return Err(Problem::new(
            "invalid_idempotency_key",
            "Idempotency key is invalid",
            400,
            "identity",
        ));
    }
    Ok(())
}

fn scoped_idempotency_key(actor_id: &AccountId, key: &str) -> String {
    let digest = Sha256::digest(format!("{}:{key}", actor_id.as_str()).as_bytes());
    let digest = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("account-pending-{digest}")
}

fn account_exists() -> Problem {
    Problem::new("account_exists", "Account already exists", 409, "identity")
}

fn idempotency_conflict() -> Problem {
    Problem::new(
        "idempotency_conflict",
        "Idempotency key was already used for a different account request",
        409,
        "identity",
    )
}

fn corrupt() -> Problem {
    Problem::new(
        "account_provisioning_record_corrupt",
        "Account provisioning record is corrupt",
        500,
        "identity",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::credentials::CredentialService;
    use buglogin_domain::account::IdentityVerification;
    use buglogin_storage::control::MemoryControlRecordStore;

    fn fixture() -> (ControlPersistence, AccountProvisioningService, AccountId) {
        let store = ControlPersistence::new(MemoryControlRecordStore::default());
        let service = AccountProvisioningService::new(store.clone());
        let actor = AccountId::new("acct_platform_admin").unwrap();
        (store, service, actor)
    }

    #[test]
    fn creates_pending_identity_without_credential_or_session_material() {
        let (store, service, actor) = fixture();
        let account_id = AccountId::new("acct_pending_user").unwrap();
        let created = service
            .create_pending(
                account_id.clone(),
                "newuser",
                "new-user@example.com",
                &actor,
                "account-create-0001",
                "2026-10-23T00:00:00Z",
            )
            .unwrap();

        assert_eq!(created.account.id, account_id);
        assert_eq!(created.identity.verification, IdentityVerification::Pending);
        assert!(!created.replayed);
        assert!(
            store
                .load::<Value>("identity", "account_password", created.account.id.as_str())
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .load::<Value>("identity", "web_session", created.account.id.as_str())
                .unwrap()
                .is_none()
        );

        let credentials = CredentialService::new(store).unwrap();
        let error = credentials
            .authenticate_versioned("new-user@example.com", "some password", 100)
            .unwrap_err();
        assert_eq!(error.code, "invalid_credentials");
        assert_eq!(error.status, 401);
    }

    #[test]
    fn retries_are_atomic_idempotent_and_do_not_issue_a_second_identity() {
        let (_store, service, actor) = fixture();
        let account_id = AccountId::new("acct_pending_user").unwrap();
        let first = service
            .create_pending(
                account_id.clone(),
                "newuser",
                "new-user@example.com",
                &actor,
                "account-create-0001",
                "2026-10-23T00:00:00Z",
            )
            .unwrap();
        let retry = service
            .create_pending(
                account_id,
                "newuser",
                "new-user@example.com",
                &actor,
                "account-create-0001",
                "2026-10-23T00:00:01Z",
            )
            .unwrap();

        assert_eq!(retry.account, first.account);
        assert_eq!(retry.identity, first.identity);
        assert!(retry.replayed);
    }

    #[test]
    fn rejects_reusing_idempotency_key_for_different_identity() {
        let (_store, service, actor) = fixture();
        service
            .create_pending(
                AccountId::new("acct_pending_user").unwrap(),
                "newuser",
                "new-user@example.com",
                &actor,
                "account-create-0001",
                "2026-10-23T00:00:00Z",
            )
            .unwrap();
        let error = service
            .create_pending(
                AccountId::new("acct_different_user").unwrap(),
                "differentuser",
                "different@example.com",
                &actor,
                "account-create-0001",
                "2026-10-23T00:00:01Z",
            )
            .unwrap_err();
        assert_eq!(error.code, "idempotency_conflict");
    }
}
