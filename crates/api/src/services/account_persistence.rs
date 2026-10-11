use buglogin_domain::{account::Account, errors::Problem, ids::AccountId};
use buglogin_storage::control::{
    ControlAuditEntry, ControlPut, ControlRecord, ExecuteControlCommand, HttpResponse,
};
use serde_json::json;

use super::control_persistence::ControlPersistence;

const TENANT: &str = "control";
const RECORD_TYPE: &str = "account";

#[derive(Clone)]
pub struct AccountPersistence {
    store: ControlPersistence,
}

impl AccountPersistence {
    pub fn new(store: ControlPersistence) -> Self {
        Self { store }
    }

    pub fn load(&self, account_id: &AccountId) -> Result<Option<Account>, Problem> {
        self.store
            .load::<Account>(TENANT, RECORD_TYPE, account_id.as_str())
    }

    pub(crate) fn load_versioned(
        &self,
        account_id: &AccountId,
    ) -> Result<Option<(Account, u64)>, Problem> {
        self.store
            .load_versioned(TENANT, RECORD_TYPE, account_id.as_str())
    }

    pub(crate) fn prepare_import_put(
        &self,
        account: &Account,
        expected_revision: u64,
    ) -> Result<ControlPut, Problem> {
        Ok(ControlPut {
            tenant_id: TENANT.into(),
            record_type: RECORD_TYPE.into(),
            record_id: account.id.as_str().into(),
            expected_revision: Some(expected_revision),
            value: serde_json::to_value(account).map_err(|_| corrupt())?,
        })
    }

    pub fn provision_default(
        &self,
        account_id: AccountId,
        created_at: String,
    ) -> Result<Account, Problem> {
        if let Some(account) = self.load(&account_id)? {
            return Ok(account);
        }
        let account = Account::new(account_id.clone(), created_at);
        let value = serde_json::to_value(&account).map_err(|_| corrupt())?;
        let command = ExecuteControlCommand {
            tenant_id: TENANT.into(),
            command_id: "account.provision".into(),
            idempotency_key: format!("account-provision-{}", account_id.as_str()),
            expected_revisions: Default::default(),
            puts: vec![ControlPut {
                tenant_id: TENANT.into(),
                record_type: RECORD_TYPE.into(),
                record_id: account_id.as_str().into(),
                expected_revision: Some(0),
                value: value.clone(),
            }],
            deletes: vec![],
            audit: ControlAuditEntry {
                tenant_id: TENANT.into(),
                command_id: "account.provision".into(),
                actor_id: account_id.as_str().into(),
                allowed: true,
                detail: json!({"source": "explicit_local_provisioning"}),
            },
            response: HttpResponse {
                status: 201,
                headers: Default::default(),
                body: value,
            },
        };
        self.store
            .execute(command)?
            .records
            .into_iter()
            .next()
            .map(decode)
            .transpose()?
            .ok_or_else(unavailable)
    }
}

fn decode(record: ControlRecord) -> Result<Account, Problem> {
    serde_json::from_value(record.value).map_err(|_| corrupt())
}

fn unavailable() -> Problem {
    Problem::new(
        "account_storage_unavailable",
        "Account storage is unavailable",
        503,
        "api",
    )
    .retryable(true)
}

fn corrupt() -> Problem {
    Problem::new(
        "account_record_corrupt",
        "Account record is corrupt",
        500,
        "api",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use buglogin_storage::control::MemoryControlRecordStore;

    #[test]
    fn provisions_once_and_reloads_from_store() {
        let persistence =
            AccountPersistence::new(ControlPersistence::new(MemoryControlRecordStore::default()));
        let id = AccountId::new("acct_persist").unwrap();
        let created = persistence
            .provision_default(id.clone(), "2026-07-21T00:00:00Z".into())
            .unwrap();
        assert_eq!(persistence.load(&id).unwrap(), Some(created.clone()));
        assert_eq!(
            persistence
                .provision_default(id, "different".into())
                .unwrap(),
            created
        );
    }
}
