use buglogin_domain::{account::AccountIdentity, errors::Problem, ids::AccountId};
use buglogin_storage::control::{
    ControlAuditEntry, ControlPut, ExecuteControlCommand, HttpResponse,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::control_persistence::ControlPersistence;

const TENANT: &str = "identity";
const IDENTITY: &str = "account_identity";
const USERNAME_INDEX: &str = "account_identity_username";
const EMAIL_INDEX: &str = "account_identity_email";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct IdentityIndex {
    account_id: AccountId,
}

#[derive(Clone)]
pub struct IdentityPersistence {
    store: ControlPersistence,
}

impl IdentityPersistence {
    pub fn new(store: ControlPersistence) -> Self {
        Self { store }
    }

    pub fn load(&self, account_id: &AccountId) -> Result<Option<AccountIdentity>, Problem> {
        self.store
            .load(TENANT, IDENTITY, account_id.as_str())
            .map_err(map_corrupt)
    }

    pub fn find_by_username(&self, canonical: &str) -> Result<Option<AccountIdentity>, Problem> {
        self.find(USERNAME_INDEX, canonical)
    }

    pub fn find_by_email(&self, canonical: &str) -> Result<Option<AccountIdentity>, Problem> {
        self.find(EMAIL_INDEX, canonical)
    }

    pub fn create(&self, identity: &AccountIdentity) -> Result<AccountIdentity, Problem> {
        if let Some(existing) = self.load(&identity.account_id)? {
            return if existing == *identity {
                Ok(existing)
            } else {
                Err(identity_exists())
            };
        }
        self.assert_available(
            USERNAME_INDEX,
            &identity.canonical_username,
            "username_taken",
        )?;
        self.assert_available(EMAIL_INDEX, &identity.canonical_email, "email_taken")?;

        let result = self.store.execute(ExecuteControlCommand {
            tenant_id: TENANT.into(),
            command_id: "identity.create".into(),
            idempotency_key: format!("identity.create-{}", identity.account_id.as_str()),
            expected_revisions: Default::default(),
            puts: vec![
                put(IDENTITY, identity.account_id.as_str(), identity)?,
                put(
                    USERNAME_INDEX,
                    &index_id(&identity.canonical_username),
                    &IdentityIndex {
                        account_id: identity.account_id.clone(),
                    },
                )?,
                put(
                    EMAIL_INDEX,
                    &index_id(&identity.canonical_email),
                    &IdentityIndex {
                        account_id: identity.account_id.clone(),
                    },
                )?,
            ],
            deletes: vec![],
            audit: ControlAuditEntry {
                tenant_id: TENANT.into(),
                command_id: "identity.create".into(),
                actor_id: identity.account_id.as_str().into(),
                allowed: true,
                detail: json!({"state": "created"}),
            },
            response: HttpResponse {
                status: 201,
                headers: Default::default(),
                body: Value::Null,
            },
        });

        match result {
            Ok(_) => self.load(&identity.account_id)?.ok_or_else(unavailable),
            Err(error) if error.code == "storage_revision_conflict" => {
                if let Some(existing) = self.load(&identity.account_id)? {
                    return if existing == *identity {
                        Ok(existing)
                    } else {
                        Err(identity_exists())
                    };
                }
                self.assert_available(
                    USERNAME_INDEX,
                    &identity.canonical_username,
                    "username_taken",
                )?;
                self.assert_available(EMAIL_INDEX, &identity.canonical_email, "email_taken")?;
                Err(error)
            }
            Err(error) => Err(error),
        }
    }

    fn find(&self, record_type: &str, canonical: &str) -> Result<Option<AccountIdentity>, Problem> {
        let Some(index) = self
            .store
            .load::<IdentityIndex>(TENANT, record_type, &index_id(canonical))
            .map_err(map_corrupt)?
        else {
            return Ok(None);
        };
        self.load(&index.account_id)?.map(Some).ok_or_else(corrupt)
    }

    fn assert_available(
        &self,
        record_type: &str,
        canonical: &str,
        code: &'static str,
    ) -> Result<(), Problem> {
        if self
            .store
            .load::<IdentityIndex>(TENANT, record_type, &index_id(canonical))
            .map_err(map_corrupt)?
            .is_some()
        {
            return Err(Problem::new(
                code,
                "Identity is already in use",
                409,
                "identity",
            ));
        }
        Ok(())
    }
}

fn put<T: Serialize>(record_type: &str, record_id: &str, value: &T) -> Result<ControlPut, Problem> {
    Ok(ControlPut {
        tenant_id: TENANT.into(),
        record_type: record_type.into(),
        record_id: record_id.into(),
        expected_revision: Some(0),
        value: serde_json::to_value(value).map_err(|_| corrupt())?,
    })
}

fn index_id(canonical: &str) -> String {
    Sha256::digest(canonical.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn map_corrupt(error: Problem) -> Problem {
    if error.code == "control_record_corrupt" {
        corrupt()
    } else {
        error
    }
}

fn identity_exists() -> Problem {
    Problem::new(
        "account_identity_exists",
        "Account identity already exists",
        409,
        "identity",
    )
}

fn unavailable() -> Problem {
    Problem::new(
        "identity_storage_unavailable",
        "Identity storage is unavailable",
        503,
        "identity",
    )
    .retryable(true)
}

fn corrupt() -> Problem {
    Problem::new(
        "identity_record_corrupt",
        "Identity record is corrupt",
        500,
        "identity",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use buglogin_storage::control::MemoryControlRecordStore;

    fn identity(account: &str, username: &str, email: &str) -> AccountIdentity {
        AccountIdentity::new(
            AccountId::new(account).unwrap(),
            username,
            email,
            "2026-07-22T00:00:00Z",
        )
        .unwrap()
    }

    #[test]
    fn creates_once_and_resolves_both_canonical_indexes() {
        let persistence =
            IdentityPersistence::new(ControlPersistence::new(MemoryControlRecordStore::default()));
        let created = persistence
            .create(&identity("acct_identity", "Owner", "Owner@Example.com"))
            .unwrap();

        assert_eq!(
            persistence.find_by_username("owner").unwrap(),
            Some(created.clone())
        );
        assert_eq!(
            persistence.find_by_email("owner@example.com").unwrap(),
            Some(created.clone())
        );
        assert_eq!(persistence.create(&created).unwrap(), created);
    }

    #[test]
    fn rejects_username_and_email_collisions_without_partial_identity() {
        let persistence =
            IdentityPersistence::new(ControlPersistence::new(MemoryControlRecordStore::default()));
        persistence
            .create(&identity("acct_one", "owner", "one@example.com"))
            .unwrap();

        assert_eq!(
            persistence
                .create(&identity("acct_two", "OWNER", "two@example.com"))
                .unwrap_err()
                .code,
            "username_taken"
        );
        assert_eq!(
            persistence
                .create(&identity("acct_three", "other", "ONE@example.com"))
                .unwrap_err()
                .code,
            "email_taken"
        );
        assert!(
            persistence
                .load(&AccountId::new("acct_two").unwrap())
                .unwrap()
                .is_none()
        );
        assert!(
            persistence
                .load(&AccountId::new("acct_three").unwrap())
                .unwrap()
                .is_none()
        );
    }
}
