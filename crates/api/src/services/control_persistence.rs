use std::sync::{Arc, Mutex};

use buglogin_domain::errors::Problem;
use buglogin_storage::control::{
    ClaimControlCommand, CompleteControlCommand, ControlCommandResult, ControlIdempotencyClaim,
    ControlRecordStore, ExecuteControlCommand,
};
use serde::de::DeserializeOwned;

#[derive(Clone)]
pub struct ControlPersistence {
    store: Arc<Mutex<Box<dyn ControlRecordStore + Send>>>,
}

impl ControlPersistence {
    pub fn new<S>(store: S) -> Self
    where
        S: ControlRecordStore + Send + 'static,
    {
        Self {
            store: Arc::new(Mutex::new(Box::new(store))),
        }
    }

    pub fn load<T>(
        &self,
        tenant_id: &str,
        record_type: &str,
        record_id: &str,
    ) -> Result<Option<T>, Problem>
    where
        T: DeserializeOwned,
    {
        Ok(self
            .load_versioned(tenant_id, record_type, record_id)?
            .map(|(value, _)| value))
    }

    pub fn load_versioned<T>(
        &self,
        tenant_id: &str,
        record_type: &str,
        record_id: &str,
    ) -> Result<Option<(T, u64)>, Problem>
    where
        T: DeserializeOwned,
    {
        let record =
            self.store
                .lock()
                .map_err(|_| unavailable())?
                .get(tenant_id, record_type, record_id)?;
        record
            .map(|record| {
                let revision = record.revision;
                decode(record).map(|value| (value, revision))
            })
            .transpose()
    }

    pub fn list_versioned<T>(
        &self,
        tenant_id: &str,
        record_type: &str,
    ) -> Result<Vec<(String, T, u64)>, Problem>
    where
        T: DeserializeOwned,
    {
        self.store
            .lock()
            .map_err(|_| unavailable())?
            .list(tenant_id, record_type)?
            .into_iter()
            .map(|record| {
                let id = record.record_id.clone();
                let revision = record.revision;
                decode(record).map(|value| (id, value, revision))
            })
            .collect()
    }

    #[allow(dead_code)]
    pub fn list<T>(&self, tenant_id: &str, record_type: &str) -> Result<Vec<T>, Problem>
    where
        T: DeserializeOwned,
    {
        self.store
            .lock()
            .map_err(|_| unavailable())?
            .list(tenant_id, record_type)?
            .into_iter()
            .map(decode)
            .collect()
    }

    pub fn replay(
        &self,
        tenant_id: &str,
        idempotency_key: &str,
    ) -> Result<Option<ControlCommandResult>, Problem> {
        self.store
            .lock()
            .map_err(|_| unavailable())?
            .replay(tenant_id, idempotency_key)
    }
    pub fn claim_idempotency(
        &self,
        command: ClaimControlCommand,
    ) -> Result<ControlIdempotencyClaim, Problem> {
        self.store
            .lock()
            .map_err(|_| unavailable())?
            .claim_idempotency(command)
    }
    pub fn complete_idempotency(
        &self,
        command: CompleteControlCommand,
    ) -> Result<ControlCommandResult, Problem> {
        self.store
            .lock()
            .map_err(|_| unavailable())?
            .complete_idempotency(command)
    }
    pub fn execute(&self, command: ExecuteControlCommand) -> Result<ControlCommandResult, Problem> {
        self.store
            .lock()
            .map_err(|_| unavailable())?
            .execute(command)
    }
}

fn decode<T>(record: buglogin_storage::control::ControlRecord) -> Result<T, Problem>
where
    T: DeserializeOwned,
{
    serde_json::from_value(record.value).map_err(|_| corrupt())
}

fn unavailable() -> Problem {
    Problem::new(
        "control_persistence_unavailable",
        "Control persistence is unavailable",
        503,
        "api",
    )
    .retryable(true)
}

fn corrupt() -> Problem {
    Problem::new(
        "control_record_corrupt",
        "Control record is corrupt",
        500,
        "api",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use buglogin_storage::control::{
        ControlAuditEntry, ControlDelete, ControlPut, HttpResponse, MemoryControlRecordStore,
    };
    use serde::{Deserialize, Serialize};
    use serde_json::json;

    #[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
    struct Entry {
        value: String,
    }

    fn command(idempotency_key: &str) -> ExecuteControlCommand {
        ExecuteControlCommand {
            tenant_id: "tenant".into(),
            command_id: "entry.write".into(),
            idempotency_key: idempotency_key.into(),
            expected_revisions: Default::default(),
            puts: vec![ControlPut {
                tenant_id: "tenant".into(),
                record_type: "entry".into(),
                record_id: "one".into(),
                expected_revision: Some(0),
                value: json!({"value": "stored"}),
            }],
            deletes: vec![],
            audit: ControlAuditEntry {
                tenant_id: "tenant".into(),
                command_id: "entry.write".into(),
                actor_id: "actor".into(),
                allowed: true,
                detail: json!({"reason": "test"}),
            },
            response: HttpResponse {
                status: 201,
                headers: Default::default(),
                body: json!({"ok": true}),
            },
        }
    }

    #[test]
    fn clones_share_typed_records_and_idempotent_response() {
        let persistence = ControlPersistence::new(MemoryControlRecordStore::default());
        let clone = persistence.clone();
        let first = persistence.execute(command("entry-write-000001")).unwrap();
        let replay = clone.execute(command("entry-write-000001")).unwrap();

        assert!(!first.replayed);
        assert!(replay.replayed);
        assert_eq!(replay.response, first.response);
        assert_eq!(
            clone.load::<Entry>("tenant", "entry", "one").unwrap(),
            Some(Entry {
                value: "stored".into()
            })
        );
        assert_eq!(
            persistence.list::<Entry>("tenant", "entry").unwrap().len(),
            1
        );
    }

    #[test]
    fn execute_applies_delete_atomically_with_audit() {
        let persistence = ControlPersistence::new(MemoryControlRecordStore::default());
        persistence.execute(command("entry-write-000002")).unwrap();
        let mut delete = command("entry-delete-000001");
        delete.command_id = "entry.delete".into();
        delete.audit.command_id = "entry.delete".into();
        delete.puts.clear();
        delete.deletes = vec![ControlDelete {
            tenant_id: "tenant".into(),
            record_type: "entry".into(),
            record_id: "one".into(),
            expected_revision: Some(1),
        }];

        let result = persistence.execute(delete).unwrap();
        assert_eq!(result.response.status, 201);
        assert!(
            persistence
                .load::<Entry>("tenant", "entry", "one")
                .unwrap()
                .is_none()
        );
    }
}
