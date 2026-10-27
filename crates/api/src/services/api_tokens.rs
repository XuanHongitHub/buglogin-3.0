use std::collections::{BTreeMap, BTreeSet};

use buglogin_domain::{errors::Problem, ids::AccountId};
use buglogin_storage::control::{
    ControlAuditEntry, ControlPut, ExecuteControlCommand, HttpResponse,
};
use getrandom::fill;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, Zeroizing};

use super::{
    account_persistence::AccountPersistence, control_persistence::ControlPersistence,
    credentials::CredentialService,
};

const TENANT: &str = "identity";
const RECORD_TYPE: &str = "api_token";
const PREFIX: &str = "blapi_";
const MAX_TTL: u64 = 365 * 24 * 60 * 60;

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ApiTokenRecord {
    account_id: AccountId,
    name: String,
    scopes: BTreeSet<String>,
    #[serde(default)]
    credential_version: Option<u32>,
    created_at: u64,
    expires_at: u64,
    revoked_at: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiTokenSummary {
    pub id: String,
    pub name: String,
    pub scopes: BTreeSet<String>,
    pub created_at: u64,
    pub expires_at: u64,
    pub revoked_at: Option<u64>,
}

#[derive(Clone, Eq, PartialEq)]
pub struct IssuedApiToken {
    pub token: String,
    pub summary: ApiTokenSummary,
}

impl std::fmt::Debug for IssuedApiToken {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("IssuedApiToken")
            .field("token", &"[REDACTED]")
            .field("summary", &self.summary)
            .finish()
    }
}

#[derive(Clone)]
pub struct ApiTokenService {
    store: ControlPersistence,
    credentials: CredentialService,
}

impl ApiTokenService {
    pub fn new(store: ControlPersistence, credentials: CredentialService) -> Self {
        Self { store, credentials }
    }

    pub fn issue(
        &self,
        account_id: AccountId,
        expected_credential_version: u32,
        name: &str,
        scopes: BTreeSet<String>,
        expires_at: u64,
        now: u64,
    ) -> Result<IssuedApiToken, Problem> {
        self.issue_with_account_revision(
            account_id,
            expected_credential_version,
            name,
            scopes,
            expires_at,
            now,
            None,
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "explicit fields preserve API token issuance policy inputs"
    )]
    pub fn issue_with_account_revision(
        &self,
        account_id: AccountId,
        expected_credential_version: u32,
        name: &str,
        scopes: BTreeSet<String>,
        expires_at: u64,
        now: u64,
        account_revision: Option<u64>,
    ) -> Result<IssuedApiToken, Problem> {
        validate_name(name)?;
        validate_scopes(&scopes)?;
        let maximum_expiry = crate::clock::checked_add_seconds(now, MAX_TTL, "api-token")?;
        if expires_at <= now || expires_at > maximum_expiry {
            return Err(invalid_input(
                "api_token_expiry_invalid",
                "API token expiry is invalid",
            ));
        }
        let Some((credential_version, credential_revision)) =
            self.credentials.password_versioned(&account_id)?
        else {
            return Err(invalid_token());
        };
        if credential_version != expected_credential_version {
            return Err(invalid_token());
        }
        let mut token = Zeroizing::new(random_token()?);
        let hash = sha256(token.as_bytes());
        let id = hex(&hash);
        let record = ApiTokenRecord {
            account_id: account_id.clone(),
            name: name.trim().to_owned(),
            scopes: scopes.clone(),
            credential_version: Some(credential_version),
            created_at: now,
            expires_at,
            revoked_at: None,
        };
        let mut expected_revisions = BTreeMap::from([(
            buglogin_storage::control::ControlRecordKey {
                tenant_id: TENANT.into(),
                record_type: "account_password".into(),
                record_id: account_id.as_str().into(),
            },
            credential_revision,
        )]);
        let account_put;
        if let Some(revision) = account_revision {
            expected_revisions.insert(
                buglogin_storage::control::ControlRecordKey {
                    tenant_id: "control".into(),
                    record_type: "account".into(),
                    record_id: account_id.as_str().into(),
                },
                revision,
            );
            account_put = Some(
                AccountPersistence::new(self.store.clone())
                    .prepare_credential_issuance_put(&account_id, revision)?,
            );
        } else {
            account_put = None;
        }
        let mut puts = vec![ControlPut {
            tenant_id: TENANT.into(),
            record_type: RECORD_TYPE.into(),
            record_id: id.clone(),
            expected_revision: Some(0),
            value: serde_json::to_value(&record).map_err(|_| corrupt())?,
        }];
        if let Some(account_put) = account_put {
            puts.push(account_put);
        }
        self.store.execute(ExecuteControlCommand {
            tenant_id: TENANT.into(),
            command_id: "auth.api_token.create".into(),
            idempotency_key: format!("api-token-create-{id}"),
            expected_revisions,
            puts,
            deletes: vec![],
            audit: audit(
                "auth.api_token.create",
                account_id.as_str(),
                json!({"tokenId": id}),
            ),
            response: HttpResponse {
                status: 201,
                headers: Default::default(),
                body: Value::Null,
            },
        })?;
        Ok(IssuedApiToken {
            token: std::mem::take(&mut *token),
            summary: summary(&id, &record),
        })
    }

    pub fn authenticate(
        &self,
        token: &str,
        now: u64,
    ) -> Result<(AccountId, BTreeSet<String>), Problem> {
        if !token.starts_with(PREFIX) {
            return Err(invalid_token());
        }
        let hash = sha256(token.as_bytes());
        let id = hex(&hash);
        let Some(record) = self
            .store
            .load::<ApiTokenRecord>(TENANT, RECORD_TYPE, &id)?
        else {
            return Err(invalid_token());
        };
        let credential_is_current = match record.credential_version {
            Some(issued) => self.credentials.password_version(&record.account_id)? == Some(issued),
            None => true,
        };
        if record.revoked_at.is_some() || now >= record.expires_at || !credential_is_current {
            return Err(invalid_token());
        }
        Ok((record.account_id, record.scopes))
    }

    pub fn list(&self, account_id: &AccountId) -> Result<Vec<ApiTokenSummary>, Problem> {
        let mut values = self
            .store
            .list_versioned_for_account::<ApiTokenRecord>(TENANT, RECORD_TYPE, account_id.as_str())?
            .into_iter()
            .map(|(id, record, _)| {
                if &record.account_id != account_id {
                    return Err(corrupt());
                }
                validate_record_id(&id)?;
                Ok(summary(&id, &record))
            })
            .collect::<Result<Vec<_>, Problem>>()?;
        values.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then(left.id.cmp(&right.id))
        });
        Ok(values)
    }

    pub fn account_revocation_puts(
        &self,
        account_id: &AccountId,
        now: u64,
    ) -> Result<Vec<ControlPut>, Problem> {
        self.store
            .list_versioned_for_account::<ApiTokenRecord>(TENANT, RECORD_TYPE, account_id.as_str())?
            .into_iter()
            .map(|(id, mut record, revision)| {
                if &record.account_id != account_id {
                    return Err(corrupt());
                }
                validate_record_id(&id)?;
                if record.revoked_at.is_some() {
                    return Ok(None);
                }
                record.revoked_at = Some(now);
                Ok(Some(ControlPut {
                    tenant_id: TENANT.into(),
                    record_type: RECORD_TYPE.into(),
                    record_id: id,
                    expected_revision: Some(revision),
                    value: serde_json::to_value(record).map_err(|_| corrupt())?,
                }))
            })
            .collect::<Result<Vec<_>, Problem>>()
            .map(|puts| puts.into_iter().flatten().collect())
    }

    pub fn revoke_account(&self, account_id: &AccountId, now: u64) -> Result<usize, Problem> {
        let puts = self.account_revocation_puts(account_id, now)?;
        if puts.is_empty() {
            return Ok(0);
        }
        let count = puts.len();
        self.store.execute(ExecuteControlCommand {
            tenant_id: TENANT.into(),
            command_id: "auth.api_token.revoke_account".into(),
            idempotency_key: format!("api-token-revoke-account-{}-{now}", account_id.as_str()),
            expected_revisions: Default::default(),
            puts,
            deletes: vec![],
            audit: audit(
                "auth.api_token.revoke_account",
                account_id.as_str(),
                json!({"recordCount":count}),
            ),
            response: HttpResponse {
                status: 200,
                headers: Default::default(),
                body: json!({"recordCount":count}),
            },
        })?;
        Ok(count)
    }

    pub fn revoke(
        &self,
        account_id: &AccountId,
        token_id: &str,
        idempotency_key: &str,
        now: u64,
    ) -> Result<(), Problem> {
        let Some((mut record, revision)) =
            self.store
                .load_versioned::<ApiTokenRecord>(TENANT, RECORD_TYPE, token_id)?
        else {
            return Err(not_found());
        };
        if &record.account_id != account_id {
            return Err(not_found());
        }
        validate_record_id(token_id)?;
        let scope = format!(
            "{}:auth.api.token.revoke:{idempotency_key}",
            account_id.as_str()
        );
        let command_key = format!("api-token-revoke-{}", hex(&sha256(scope.as_bytes())));
        let request_hash = hex(&sha256(token_id.as_bytes()));
        if let Some(replay) = self.store.replay(TENANT, &command_key)? {
            return if replay.response.status == 204
                && replay
                    .response
                    .headers
                    .get("x-buglogin-input-sha256")
                    .is_some_and(|stored| stored == &request_hash)
            {
                Ok(())
            } else if replay.response.status == 204 {
                Err(idempotency_conflict())
            } else {
                Err(corrupt())
            };
        }
        let puts = if record.revoked_at.is_some() {
            vec![]
        } else {
            record.revoked_at = Some(now);
            vec![ControlPut {
                tenant_id: TENANT.into(),
                record_type: RECORD_TYPE.into(),
                record_id: token_id.into(),
                expected_revision: Some(revision),
                value: serde_json::to_value(record).map_err(|_| corrupt())?,
            }]
        };
        let result = self.store.execute(ExecuteControlCommand {
            tenant_id: TENANT.into(),
            command_id: "auth.api_token.revoke".into(),
            idempotency_key: command_key.clone(),
            expected_revisions: Default::default(),
            puts,
            deletes: vec![],
            audit: audit(
                "auth.api_token.revoke",
                account_id.as_str(),
                json!({"tokenId": token_id}),
            ),
            response: HttpResponse {
                status: 204,
                headers: [("x-buglogin-input-sha256".into(), request_hash.clone())]
                    .into_iter()
                    .collect(),
                body: Value::Null,
            },
        });
        match result {
            Ok(_) => Ok(()),
            Err(error) => match self.store.replay(TENANT, &command_key)? {
                Some(replay)
                    if replay.response.status == 204
                        && replay
                            .response
                            .headers
                            .get("x-buglogin-input-sha256")
                            .is_some_and(|stored| stored == &request_hash) =>
                {
                    Ok(())
                }
                Some(replay) if replay.response.status == 204 => Err(idempotency_conflict()),
                _ => Err(error),
            },
        }
    }
}

fn summary(id: &str, record: &ApiTokenRecord) -> ApiTokenSummary {
    ApiTokenSummary {
        id: id.into(),
        name: record.name.clone(),
        scopes: record.scopes.clone(),
        created_at: record.created_at,
        expires_at: record.expires_at,
        revoked_at: record.revoked_at,
    }
}

fn validate_record_id(value: &str) -> Result<(), Problem> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(corrupt());
    }
    Ok(())
}

fn validate_name(name: &str) -> Result<(), Problem> {
    if name.trim().is_empty()
        || name.trim().chars().count() > 64
        || name.chars().any(|c| c.is_control())
    {
        return Err(invalid_input(
            "api_token_name_invalid",
            "API token name is invalid",
        ));
    }
    Ok(())
}

fn validate_scopes(scopes: &BTreeSet<String>) -> Result<(), Problem> {
    if scopes.is_empty() || scopes.len() > 32 {
        return Err(invalid_input(
            "api_token_scopes_invalid",
            "API token scopes are invalid",
        ));
    }
    for scope in scopes {
        let Some((capability, action)) = scope.split_once(':') else {
            return Err(invalid_input(
                "api_token_scopes_invalid",
                "API token scopes are invalid",
            ));
        };
        if capability.is_empty()
            || capability.len() > 128
            || action.is_empty()
            || action != "*"
                && !matches!(
                    action,
                    "read" | "create" | "update" | "delete" | "execute" | "share" | "manage"
                )
        {
            return Err(invalid_input(
                "api_token_scopes_invalid",
                "API token scopes are invalid",
            ));
        }
    }
    Ok(())
}

fn random_token() -> Result<String, Problem> {
    let mut bytes = [0u8; 32];
    if fill(&mut bytes).is_err() {
        bytes.zeroize();
        return Err(unavailable());
    }
    let token = format!("{PREFIX}{}", hex(&bytes));
    bytes.zeroize();
    Ok(token)
}

fn sha256(value: &[u8]) -> [u8; 32] {
    Sha256::digest(value).into()
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
fn audit(command: &str, actor: &str, detail: Value) -> ControlAuditEntry {
    ControlAuditEntry {
        tenant_id: TENANT.into(),
        command_id: command.into(),
        actor_id: actor.into(),
        allowed: true,
        detail,
    }
}
fn invalid_token() -> Problem {
    Problem::new(
        "invalid_api_token",
        "API token is invalid or expired",
        401,
        "identity",
    )
}
fn not_found() -> Problem {
    Problem::new(
        "api_token_not_found",
        "API token was not found",
        404,
        "identity",
    )
}
fn invalid_input(code: &'static str, title: &'static str) -> Problem {
    Problem::new(code, title, 400, "identity")
}
fn unavailable() -> Problem {
    Problem::new(
        "api_token_storage_unavailable",
        "API token storage is unavailable",
        503,
        "identity",
    )
    .retryable(true)
}
fn corrupt() -> Problem {
    Problem::new(
        "api_token_record_corrupt",
        "API token record is corrupt",
        500,
        "identity",
    )
}
fn idempotency_conflict() -> Problem {
    Problem::new(
        "idempotency_conflict",
        "Idempotency key was reused with different input",
        409,
        "identity",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::{
        credentials::CredentialService, identity_persistence::IdentityPersistence,
    };
    use buglogin_domain::account::{Account, AccountIdentity, AccountStatus};
    use buglogin_storage::control::MemoryControlRecordStore;

    fn seeded_service(account_ids: &[&str]) -> (ControlPersistence, ApiTokenService) {
        let store = ControlPersistence::new(MemoryControlRecordStore::default());
        let credentials = CredentialService::new(store.clone()).unwrap();
        let identities = IdentityPersistence::new(store.clone());
        for (index, value) in account_ids.iter().enumerate() {
            let account = AccountId::new(*value).unwrap();
            let identity = AccountIdentity::new(
                account.clone(),
                format!("token-user-{index}"),
                format!("token-user-{index}@example.com"),
                "100",
            )
            .unwrap();
            identities.create(&identity).unwrap();
            credentials
                .set_initial_password(&account, "correct horse battery staple", 100)
                .unwrap();
        }
        let service = ApiTokenService::new(store.clone(), credentials);
        (store, service)
    }

    #[test]
    fn token_name_limit_matches_openapi_character_length() {
        let account = AccountId::new("acct_api_token_unicode").unwrap();
        let (_, service) = seeded_service(&[account.as_str()]);
        let scopes = BTreeSet::from(["profile.read:read".into()]);
        assert!(
            service
                .issue(
                    account.clone(),
                    1,
                    &"é".repeat(64),
                    scopes.clone(),
                    500,
                    100,
                )
                .is_ok()
        );
        let error = service
            .issue(account, 1, &"é".repeat(65), scopes, 500, 100)
            .expect_err("65-character token name must be rejected");
        assert_eq!(error.code, "api_token_name_invalid");
    }

    #[test]
    fn issues_hash_only_scoped_token_and_revokes_it() {
        let account = AccountId::new("acct_api_token").unwrap();
        let other_account = AccountId::new("acct_api_token_other").unwrap();
        let (store, service) = seeded_service(&[account.as_str(), other_account.as_str()]);
        let scopes = BTreeSet::from(["profile.read:read".into(), "proxy.parse:read".into()]);
        let issued = service
            .issue(account.clone(), 1, "automation", scopes.clone(), 500, 100)
            .unwrap();
        let other_issued = service
            .issue(
                other_account.clone(),
                1,
                "other automation",
                scopes.clone(),
                500,
                100,
            )
            .unwrap();
        assert!(issued.token.starts_with(PREFIX));
        assert_eq!(
            service.authenticate(&issued.token, 200).unwrap(),
            (account.clone(), scopes)
        );
        let stored = store.list::<ApiTokenRecord>(TENANT, RECORD_TYPE).unwrap();
        assert!(
            !serde_json::to_string(&stored)
                .unwrap()
                .contains(&issued.token)
        );
        assert_eq!(service.list(&account).unwrap().len(), 1);
        assert_eq!(service.list(&other_account).unwrap().len(), 1);
        let revocations = service.account_revocation_puts(&account, 250).unwrap();
        assert_eq!(revocations.len(), 1);
        let revoked_record: ApiTokenRecord =
            serde_json::from_value(revocations[0].value.clone()).unwrap();
        assert_eq!(revoked_record.account_id, account);
        service
            .revoke(
                &account,
                &issued.summary.id,
                "api-token-revoke-owner-0001",
                300,
            )
            .unwrap();
        assert_eq!(
            service.authenticate(&issued.token, 301).unwrap_err().code,
            "invalid_api_token"
        );
        assert_eq!(
            service.authenticate(&other_issued.token, 301).unwrap().0,
            other_account
        );
    }

    #[test]
    fn account_token_operations_fail_closed_on_malformed_record_ids() {
        let account = AccountId::new("acct_api_token_malformed_id").unwrap();
        let (store, service) = seeded_service(&[account.as_str()]);
        store
            .execute(ExecuteControlCommand {
                tenant_id: TENANT.into(),
                command_id: "test.api_token.seed_corrupt".into(),
                idempotency_key: "test-api-token-malformed-record-id".into(),
                expected_revisions: Default::default(),
                puts: vec![ControlPut {
                    tenant_id: TENANT.into(),
                    record_type: RECORD_TYPE.into(),
                    record_id: "malformed-id".into(),
                    expected_revision: Some(0),
                    value: json!({
                        "accountId": account,
                        "name": "corrupt",
                        "scopes": ["profile.read:read"],
                        "credentialVersion": 1,
                        "createdAt": 100,
                        "expiresAt": 500,
                        "revokedAt": null
                    }),
                }],
                deletes: vec![],
                audit: audit("test.api_token.seed_corrupt", "test", Value::Null),
                response: HttpResponse {
                    status: 200,
                    headers: Default::default(),
                    body: Value::Null,
                },
            })
            .unwrap();

        assert_eq!(
            service.list(&account).unwrap_err().code,
            "api_token_record_corrupt"
        );
        assert_eq!(
            service
                .account_revocation_puts(&account, 200)
                .unwrap_err()
                .code,
            "api_token_record_corrupt"
        );
    }

    #[test]
    fn token_issue_is_fenced_by_durable_account_status_revision() {
        let account_id = AccountId::new("acct_api_token_account_fence").unwrap();
        let (store, service) = seeded_service(&[account_id.as_str()]);
        let account = Account::new(account_id.clone(), "2026-10-18T00:00:00Z");
        store
            .execute(ExecuteControlCommand {
                tenant_id: "control".into(),
                command_id: "account.provision".into(),
                idempotency_key: "account-api-token-fence-seed".into(),
                expected_revisions: BTreeMap::new(),
                puts: vec![ControlPut {
                    tenant_id: "control".into(),
                    record_type: "account".into(),
                    record_id: account_id.as_str().into(),
                    expected_revision: Some(0),
                    value: serde_json::to_value(&account).unwrap(),
                }],
                deletes: vec![],
                audit: ControlAuditEntry {
                    tenant_id: "control".into(),
                    command_id: "account.provision".into(),
                    actor_id: "service".into(),
                    allowed: true,
                    detail: Value::Null,
                },
                response: HttpResponse {
                    status: 201,
                    headers: Default::default(),
                    body: Value::Null,
                },
            })
            .unwrap();

        let scopes = BTreeSet::from(["profile.read:read".into()]);
        service
            .issue_with_account_revision(
                account_id.clone(),
                1,
                "before-suspension",
                scopes.clone(),
                500,
                100,
                Some(1),
            )
            .unwrap();
        let (current_account, current_storage_revision) = store
            .load_versioned::<Account>("control", "account", account_id.as_str())
            .unwrap()
            .unwrap();
        let suspended = current_account
            .transition_status(
                current_account.revision,
                AccountStatus::Suspended,
                "2026-10-18T00:01:00Z",
            )
            .unwrap();
        let account_key = buglogin_storage::control::ControlRecordKey {
            tenant_id: "control".into(),
            record_type: "account".into(),
            record_id: account_id.as_str().into(),
        };
        store
            .execute(ExecuteControlCommand {
                tenant_id: "control".into(),
                command_id: "account.status.change".into(),
                idempotency_key: "account-api-token-fence-suspend".into(),
                expected_revisions: BTreeMap::from([(account_key, current_storage_revision)]),
                puts: vec![ControlPut {
                    tenant_id: "control".into(),
                    record_type: "account".into(),
                    record_id: account_id.as_str().into(),
                    expected_revision: Some(current_storage_revision),
                    value: serde_json::to_value(suspended).unwrap(),
                }],
                deletes: vec![],
                audit: ControlAuditEntry {
                    tenant_id: "control".into(),
                    command_id: "account.status.change".into(),
                    actor_id: "operator".into(),
                    allowed: true,
                    detail: Value::Null,
                },
                response: HttpResponse {
                    status: 200,
                    headers: Default::default(),
                    body: Value::Null,
                },
            })
            .unwrap();

        assert_eq!(
            service
                .issue_with_account_revision(
                    account_id.clone(),
                    1,
                    "after-suspension",
                    scopes,
                    500,
                    101,
                    Some(1),
                )
                .unwrap_err()
                .code,
            "storage_revision_conflict"
        );
        assert_eq!(service.list(&account_id).unwrap().len(), 1);
    }

    #[test]
    fn expiry_and_owner_scope_are_enforced() {
        let account = AccountId::new("acct_api_token_owner").unwrap();
        let other = AccountId::new("acct_api_token_other").unwrap();
        let (_, service) = seeded_service(&[account.as_str(), other.as_str()]);
        let issued = service
            .issue(
                account.clone(),
                1,
                "automation",
                BTreeSet::from(["profile.read:read".into()]),
                200,
                100,
            )
            .unwrap();
        assert_eq!(
            service.authenticate(&issued.token, 200).unwrap_err().code,
            "invalid_api_token"
        );
        assert_eq!(
            service
                .revoke(
                    &other,
                    &issued.summary.id,
                    "api-token-revoke-other-0001",
                    150
                )
                .unwrap_err()
                .code,
            "api_token_not_found"
        );
    }

    #[test]
    fn account_revocation_invalidates_all_persisted_tokens() {
        let account = AccountId::new("acct_api_token_revoke_all").unwrap();
        let (_, service) = seeded_service(&[account.as_str()]);
        let first = service
            .issue(
                account.clone(),
                1,
                "first",
                BTreeSet::from(["profile.read:read".into()]),
                500,
                100,
            )
            .unwrap();
        let second = service
            .issue(
                account.clone(),
                1,
                "second",
                BTreeSet::from(["proxy.list:read".into()]),
                500,
                101,
            )
            .unwrap();

        assert_eq!(service.revoke_account(&account, 200).unwrap(), 2);
        let reopened = service.clone();
        assert_eq!(
            reopened.authenticate(&first.token, 201).unwrap_err().code,
            "invalid_api_token"
        );
        assert_eq!(
            reopened.authenticate(&second.token, 201).unwrap_err().code,
            "invalid_api_token"
        );
    }
}
