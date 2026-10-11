use buglogin_domain::{errors::Problem, identity::SessionRecord};
use buglogin_storage::control::{
    ControlAuditEntry, ControlPut, ExecuteControlCommand, HttpResponse,
};
use serde_json::{Value, json};

use super::control_persistence::ControlPersistence;

const TENANT: &str = "identity";
const RECORD_TYPE: &str = "session";

#[derive(Clone)]
pub struct SessionPersistence {
    store: ControlPersistence,
}

impl SessionPersistence {
    pub fn new(store: ControlPersistence) -> Self {
        Self { store }
    }

    pub fn list(&self) -> Result<Vec<(SessionRecord, u64)>, Problem> {
        self.store
            .list_versioned::<SessionRecord>(TENANT, RECORD_TYPE)?
            .into_iter()
            .map(|(record_id, record, revision)| {
                if record_id != session_id(&record.token_hash) {
                    return Err(corrupt());
                }
                Ok((record, revision))
            })
            .collect()
    }

    pub fn get(&self, token_hash: &[u8; 32]) -> Result<Option<(SessionRecord, u64)>, Problem> {
        let record_id = session_id(token_hash);
        let Some((record, revision)) =
            self.store
                .load_versioned::<SessionRecord>(TENANT, RECORD_TYPE, &record_id)?
        else {
            return Ok(None);
        };
        if record.token_hash != *token_hash {
            return Err(corrupt());
        }
        Ok(Some((record, revision)))
    }

    pub fn create(&self, record: &SessionRecord) -> Result<u64, Problem> {
        self.persist(record, 0, "session.issue", "issued")
    }

    pub fn revoke(&self, record: &SessionRecord, expected_revision: u64) -> Result<u64, Problem> {
        self.persist(record, expected_revision, "session.revoke", "revoked")
    }

    pub fn rotate(
        &self,
        revoked: &SessionRecord,
        expected_revision: u64,
        replacement: &SessionRecord,
    ) -> Result<(u64, u64), Problem> {
        let revoked_id = session_id(&revoked.token_hash);
        let replacement_id = session_id(&replacement.token_hash);
        let result = self.store.execute(ExecuteControlCommand {
            tenant_id: TENANT.into(),
            command_id: "session.refresh".into(),
            idempotency_key: format!(
                "session.refresh-{}-{}-{}",
                &revoked_id[..16],
                &replacement_id[..16],
                revoked.revoked_at.unwrap_or(0)
            ),
            expected_revisions: Default::default(),
            puts: vec![
                ControlPut {
                    tenant_id: TENANT.into(),
                    record_type: RECORD_TYPE.into(),
                    record_id: revoked_id.clone(),
                    expected_revision: Some(expected_revision),
                    value: serde_json::to_value(revoked).map_err(|_| corrupt())?,
                },
                ControlPut {
                    tenant_id: TENANT.into(),
                    record_type: RECORD_TYPE.into(),
                    record_id: replacement_id.clone(),
                    expected_revision: Some(0),
                    value: serde_json::to_value(replacement).map_err(|_| corrupt())?,
                },
            ],
            deletes: vec![],
            audit: ControlAuditEntry {
                tenant_id: TENANT.into(),
                command_id: "session.refresh".into(),
                actor_id: revoked.account_id.as_str().into(),
                allowed: true,
                detail: json!({
                    "revokedSessionId": revoked_id,
                    "replacementSessionId": replacement_id,
                    "state": "rotated"
                }),
            },
            response: HttpResponse {
                status: 200,
                headers: Default::default(),
                body: Value::Null,
            },
        })?;
        let revoked_revision = result
            .records
            .iter()
            .find(|record| record.record_id == revoked_id)
            .map(|record| record.revision)
            .ok_or_else(unavailable)?;
        let replacement_revision = result
            .records
            .iter()
            .find(|record| record.record_id == replacement_id)
            .map(|record| record.revision)
            .ok_or_else(unavailable)?;
        Ok((revoked_revision, replacement_revision))
    }

    fn persist(
        &self,
        record: &SessionRecord,
        expected_revision: u64,
        command_id: &str,
        state: &str,
    ) -> Result<u64, Problem> {
        let record_id = session_id(&record.token_hash);
        let value = serde_json::to_value(record).map_err(|_| corrupt())?;
        let result = self.store.execute(ExecuteControlCommand {
            tenant_id: TENANT.into(),
            command_id: command_id.into(),
            idempotency_key: format!(
                "{command_id}-{record_id}-{}",
                record.revoked_at.unwrap_or(0)
            ),
            expected_revisions: Default::default(),
            puts: vec![ControlPut {
                tenant_id: TENANT.into(),
                record_type: RECORD_TYPE.into(),
                record_id: record_id.clone(),
                expected_revision: Some(expected_revision),
                value,
            }],
            deletes: vec![],
            audit: ControlAuditEntry {
                tenant_id: TENANT.into(),
                command_id: command_id.into(),
                actor_id: record.account_id.as_str().into(),
                allowed: true,
                detail: json!({ "sessionId": record_id, "state": state }),
            },
            response: HttpResponse {
                status: 200,
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

fn session_id(hash: &[u8; 32]) -> String {
    let mut value = String::with_capacity(64);
    for byte in hash {
        use std::fmt::Write;
        let _ = write!(value, "{byte:02x}");
    }
    value
}

fn unavailable() -> Problem {
    Problem::new(
        "session_storage_unavailable",
        "Session storage is unavailable",
        503,
        "identity",
    )
    .retryable(true)
}

fn corrupt() -> Problem {
    Problem::new(
        "session_record_corrupt",
        "Session record is corrupt",
        500,
        "identity",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use buglogin_domain::{identity::issue, ids::AccountId};
    use buglogin_storage::control::MemoryControlRecordStore;

    #[test]
    fn rotation_is_atomic_when_replacement_conflicts() {
        let persistence =
            SessionPersistence::new(ControlPersistence::new(MemoryControlRecordStore::default()));
        let mut current = issue(AccountId::new("acct_rotation_atomic").unwrap(), 100)
            .unwrap()
            .record;
        assert_eq!(persistence.create(&current).unwrap(), 1);
        let replacement = issue(current.account_id.clone(), 200).unwrap().record;
        assert_eq!(persistence.create(&replacement).unwrap(), 1);
        current.revoked_at = Some(50);

        assert!(persistence.rotate(&current, 1, &replacement).is_err());
        let loaded = persistence.list().unwrap();
        let stored_current = loaded
            .iter()
            .find(|(record, _)| record.token_hash == current.token_hash)
            .unwrap();
        assert_eq!(stored_current.0.revoked_at, None);
        assert_eq!(stored_current.1, 1);
    }

    #[test]
    fn persists_only_hashed_session_and_revision() {
        let persistence =
            SessionPersistence::new(ControlPersistence::new(MemoryControlRecordStore::default()));
        let issued = issue(AccountId::new("acct_session").unwrap(), 100).unwrap();
        let raw = issued.raw_token.clone();
        assert_eq!(persistence.create(&issued.record).unwrap(), 1);
        let loaded = persistence.list().unwrap();
        assert_eq!(loaded, vec![(issued.record, 1)]);
        assert!(!serde_json::to_string(&loaded).unwrap().contains(&raw));
    }
}
