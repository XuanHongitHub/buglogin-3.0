use buglogin_domain::{
    account::{AccountIdentity, canonical_email, canonical_username},
    errors::Problem,
    ids::AccountId,
};
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
        let identity: Option<AccountIdentity> = self
            .store
            .load(TENANT, IDENTITY, account_id.as_str())
            .map_err(map_corrupt)?;
        let Some(identity) = identity else {
            return Ok(None);
        };
        if identity.account_id != *account_id
            || identity.revision == 0
            || canonical_username(&identity.username)
                .map(|canonical| canonical != identity.canonical_username)
                .unwrap_or(true)
            || canonical_email(&identity.email)
                .map(|canonical| canonical != identity.canonical_email)
                .unwrap_or(true)
        {
            return Err(corrupt());
        }
        Ok(Some(identity))
    }

    pub fn find_by_username(&self, canonical: &str) -> Result<Option<AccountIdentity>, Problem> {
        self.find(USERNAME_INDEX, canonical)
    }

    pub fn find_by_email(&self, canonical: &str) -> Result<Option<AccountIdentity>, Problem> {
        self.find(EMAIL_INDEX, canonical)
    }

    pub fn create(&self, identity: &AccountIdentity) -> Result<AccountIdentity, Problem> {
        validate_identity_for_write(identity)?;
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

    pub(crate) fn prepare_create_puts(
        &self,
        identity: &AccountIdentity,
    ) -> Result<Vec<ControlPut>, Problem> {
        validate_identity_for_write(identity)?;
        if self.load(&identity.account_id)?.is_some() {
            return Err(identity_exists());
        }
        self.assert_available(
            USERNAME_INDEX,
            &identity.canonical_username,
            "username_taken",
        )?;
        self.assert_available(EMAIL_INDEX, &identity.canonical_email, "email_taken")?;
        Ok(vec![
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
        ])
    }

    fn find(&self, record_type: &str, canonical: &str) -> Result<Option<AccountIdentity>, Problem> {
        let Some(index) = self
            .store
            .load::<IdentityIndex>(TENANT, record_type, &index_id(canonical))
            .map_err(map_corrupt)?
        else {
            return Ok(None);
        };
        let identity = self.load(&index.account_id)?.ok_or_else(corrupt)?;
        let indexed_value = match record_type {
            USERNAME_INDEX => &identity.canonical_username,
            EMAIL_INDEX => &identity.canonical_email,
            _ => return Err(corrupt()),
        };
        if indexed_value != canonical {
            return Err(corrupt());
        }
        Ok(Some(identity))
    }

    fn assert_available(
        &self,
        record_type: &str,
        canonical: &str,
        code: &'static str,
    ) -> Result<(), Problem> {
        if self.find(record_type, canonical)?.is_some() {
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

fn validate_identity_for_write(identity: &AccountIdentity) -> Result<(), Problem> {
    if identity.revision == 0
        || identity.username.trim() != identity.username.as_str()
        || identity.email.trim() != identity.email.as_str()
        || canonical_username(&identity.username)? != identity.canonical_username
        || canonical_email(&identity.email)? != identity.canonical_email
    {
        return Err(Problem::new(
            "account_identity_invalid",
            "Account identity fields are inconsistent",
            400,
            "identity",
        ));
    }
    Ok(())
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

    #[test]
    fn rejects_index_that_points_to_identity_with_different_canonical_value() {
        let store = ControlPersistence::new(MemoryControlRecordStore::default());
        let persisted = identity("acct_owner", "real-owner", "owner@example.com");
        store
            .execute(ExecuteControlCommand {
                tenant_id: TENANT.into(),
                command_id: "identity.test.seed-corrupt-index".into(),
                idempotency_key: "identity.test.seed-corrupt-index".into(),
                expected_revisions: Default::default(),
                puts: vec![
                    put(IDENTITY, persisted.account_id.as_str(), &persisted).unwrap(),
                    put(
                        USERNAME_INDEX,
                        &index_id("owner"),
                        &IdentityIndex {
                            account_id: persisted.account_id.clone(),
                        },
                    )
                    .unwrap(),
                ],
                deletes: vec![],
                audit: ControlAuditEntry {
                    tenant_id: TENANT.into(),
                    command_id: "identity.test.seed-corrupt-index".into(),
                    actor_id: "test".into(),
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

        let persistence = IdentityPersistence::new(store);
        assert_eq!(
            persistence.find_by_username("owner").unwrap_err().code,
            "identity_record_corrupt"
        );
    }

    #[test]
    fn rejects_inconsistent_identity_before_writing_any_records() {
        let persistence =
            IdentityPersistence::new(ControlPersistence::new(MemoryControlRecordStore::default()));
        let mut invalid = identity("acct_invalid_identity", "owner", "owner@example.com");
        invalid.canonical_email = "attacker@example.com".into();

        assert_eq!(
            persistence.create(&invalid).unwrap_err().code,
            "account_identity_invalid"
        );
        assert!(persistence.load(&invalid.account_id).unwrap().is_none());
        assert!(
            persistence
                .find_by_email("attacker@example.com")
                .unwrap()
                .is_none()
        );
    }
}
