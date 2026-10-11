use std::collections::BTreeSet;

use buglogin_domain::{errors::Problem, ids::AccountId};
use buglogin_storage::control::{
    ControlAuditEntry, ControlPut, ExecuteControlCommand, HttpResponse,
};
use getrandom::fill;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::control_persistence::ControlPersistence;

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

#[derive(Clone)]
pub struct ApiTokenService {
    store: ControlPersistence,
}

impl ApiTokenService {
    pub fn new(store: ControlPersistence) -> Self {
        Self { store }
    }

    pub fn issue(
        &self,
        account_id: AccountId,
        name: &str,
        scopes: BTreeSet<String>,
        expires_at: u64,
        now: u64,
    ) -> Result<IssuedApiToken, Problem> {
        validate_name(name)?;
        validate_scopes(&scopes)?;
        if expires_at <= now || expires_at > now.saturating_add(MAX_TTL) {
            return Err(invalid_input(
                "api_token_expiry_invalid",
                "API token expiry is invalid",
            ));
        }
        let token = random_token()?;
        let hash = sha256(token.as_bytes());
        let id = hex(&hash);
        let record = ApiTokenRecord {
            account_id: account_id.clone(),
            name: name.trim().to_owned(),
            scopes: scopes.clone(),
            created_at: now,
            expires_at,
            revoked_at: None,
        };
        self.store.execute(ExecuteControlCommand {
            tenant_id: TENANT.into(),
            command_id: "auth.api_token.create".into(),
            idempotency_key: format!("api-token-create-{id}"),
            expected_revisions: Default::default(),
            puts: vec![ControlPut {
                tenant_id: TENANT.into(),
                record_type: RECORD_TYPE.into(),
                record_id: id.clone(),
                expected_revision: Some(0),
                value: serde_json::to_value(&record).map_err(|_| corrupt())?,
            }],
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
            token,
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
        if record.revoked_at.is_some() || now >= record.expires_at {
            return Err(invalid_token());
        }
        Ok((record.account_id, record.scopes))
    }

    pub fn list(&self, account_id: &AccountId) -> Result<Vec<ApiTokenSummary>, Problem> {
        let mut values = self
            .store
            .list_versioned::<ApiTokenRecord>(TENANT, RECORD_TYPE)?
            .into_iter()
            .filter(|(_, record, _)| &record.account_id == account_id)
            .map(|(id, record, _)| summary(&id, &record))
            .collect::<Vec<_>>();
        values.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then(left.id.cmp(&right.id))
        });
        Ok(values)
    }

    pub fn revoke(&self, account_id: &AccountId, token_id: &str, now: u64) -> Result<(), Problem> {
        let Some((mut record, revision)) =
            self.store
                .load_versioned::<ApiTokenRecord>(TENANT, RECORD_TYPE, token_id)?
        else {
            return Err(not_found());
        };
        if &record.account_id != account_id {
            return Err(not_found());
        }
        if record.revoked_at.is_some() {
            return Ok(());
        }
        record.revoked_at = Some(now);
        self.store.execute(ExecuteControlCommand {
            tenant_id: TENANT.into(),
            command_id: "auth.api_token.revoke".into(),
            idempotency_key: format!("api-token-revoke-{token_id}"),
            expected_revisions: Default::default(),
            puts: vec![ControlPut {
                tenant_id: TENANT.into(),
                record_type: RECORD_TYPE.into(),
                record_id: token_id.into(),
                expected_revision: Some(revision),
                value: serde_json::to_value(record).map_err(|_| corrupt())?,
            }],
            deletes: vec![],
            audit: audit(
                "auth.api_token.revoke",
                account_id.as_str(),
                json!({"tokenId": token_id}),
            ),
            response: HttpResponse {
                status: 204,
                headers: Default::default(),
                body: Value::Null,
            },
        })?;
        Ok(())
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

fn validate_name(name: &str) -> Result<(), Problem> {
    if name.trim().is_empty() || name.trim().len() > 64 || name.chars().any(|c| c.is_control()) {
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
    fill(&mut bytes).map_err(|_| unavailable())?;
    Ok(format!("{PREFIX}{}", hex(&bytes)))
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

#[cfg(test)]
mod tests {
    use super::*;
    use buglogin_storage::control::MemoryControlRecordStore;

    #[test]
    fn issues_hash_only_scoped_token_and_revokes_it() {
        let store = ControlPersistence::new(MemoryControlRecordStore::default());
        let service = ApiTokenService::new(store.clone());
        let account = AccountId::new("acct_api_token").unwrap();
        let scopes = BTreeSet::from(["profile.read:read".into(), "proxy.parse:read".into()]);
        let issued = service
            .issue(account.clone(), "automation", scopes.clone(), 500, 100)
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
        service.revoke(&account, &issued.summary.id, 300).unwrap();
        assert_eq!(
            service.authenticate(&issued.token, 301).unwrap_err().code,
            "invalid_api_token"
        );
    }

    #[test]
    fn expiry_and_owner_scope_are_enforced() {
        let store = ControlPersistence::new(MemoryControlRecordStore::default());
        let service = ApiTokenService::new(store);
        let account = AccountId::new("acct_api_token_owner").unwrap();
        let other = AccountId::new("acct_api_token_other").unwrap();
        let issued = service
            .issue(
                account.clone(),
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
                .revoke(&other, &issued.summary.id, 150)
                .unwrap_err()
                .code,
            "api_token_not_found"
        );
    }
}
