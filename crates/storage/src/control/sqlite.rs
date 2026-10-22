//! Durable SQLite implementation of the control-record contract.
//!
//! This module is intentionally not wired into `control/mod.rs` yet. The
//! storage crate must first declare its `rusqlite` dependency and export this
//! module in a separately reviewed integration change.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use buglogin_domain::errors::Problem;
use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, params,
};
use serde_json::Value;

use super::*;

/// A durable local store. One SQLite transaction atomically commits records,
/// audit entries and idempotency outcomes, matching the Postgres adapter.
pub struct SqliteControlRecordStore {
    connection: Connection,
    database_path: PathBuf,
    advisory_locks: BTreeMap<i64, Connection>,
}

impl SqliteControlRecordStore {
    /// Opens (or creates) a local control database and initializes its schema.
    /// The caller is responsible for choosing a private, OS-local data path.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, Problem> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent).map_err(|_| unavailable())?;
        }
        let connection = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_FULL_MUTEX,
        )
        .map_err(|_| unavailable())?;
        connection
            .busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|_| unavailable())?;
        connection
            .execute_batch(
                "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=FULL;
             PRAGMA foreign_keys=ON;
             CREATE TABLE IF NOT EXISTS buglogin_control_records (
               tenant_id TEXT NOT NULL,
               record_type TEXT NOT NULL,
               record_id TEXT NOT NULL,
               revision INTEGER NOT NULL CHECK (revision > 0),
               checksum TEXT NOT NULL CHECK (length(checksum) = 64),
               value TEXT NOT NULL CHECK (json_valid(value)),
               PRIMARY KEY (tenant_id, record_type, record_id)
             );
             CREATE INDEX IF NOT EXISTS buglogin_control_records_scope_idx
               ON buglogin_control_records (tenant_id, record_type, record_id);
             CREATE TABLE IF NOT EXISTS buglogin_control_idempotency (
               tenant_id TEXT NOT NULL,
               idempotency_key TEXT NOT NULL,
               request_checksum TEXT NOT NULL CHECK (length(request_checksum) = 64),
               response TEXT NOT NULL CHECK (json_valid(response)),
               PRIMARY KEY (tenant_id, idempotency_key)
             );
             CREATE TABLE IF NOT EXISTS buglogin_control_audit (
               sequence INTEGER PRIMARY KEY AUTOINCREMENT,
               tenant_id TEXT NOT NULL,
               command_id TEXT NOT NULL,
               actor_id TEXT NOT NULL,
               allowed INTEGER NOT NULL CHECK (allowed IN (0, 1)),
               detail TEXT NOT NULL CHECK (json_valid(detail))
             );
             CREATE INDEX IF NOT EXISTS buglogin_control_audit_tenant_idx
               ON buglogin_control_audit (tenant_id, sequence);",
            )
            .map_err(|_| unavailable())?;
        Ok(Self {
            connection,
            database_path: path,
            advisory_locks: BTreeMap::new(),
        })
    }

    fn lock_path(&self, lock_id: i64) -> Result<PathBuf, Problem> {
        let parent = self
            .database_path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let name = self
            .database_path
            .file_name()
            .ok_or_else(unavailable)?
            .to_string_lossy();
        Ok(parent.join(format!(".{name}.advisory-{lock_id}.sqlite")))
    }

    fn execute_tx(
        &mut self,
        command: ExecuteControlCommand,
    ) -> Result<ControlCommandResult, Problem> {
        validate_command(&command)?;
        let request_checksum = command_checksum(&command)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| unavailable())?;
        if let Some((stored_checksum, response)) = tx.query_row(
            "SELECT request_checksum, response FROM buglogin_control_idempotency WHERE tenant_id=?1 AND idempotency_key=?2",
            params![command.tenant_id, command.idempotency_key],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        ).optional().map_err(|_| unavailable())? {
            if stored_checksum != request_checksum { return Err(idempotency_conflict()); }
            let mut result = decode_completed_response(decode_json(&response)?)?.ok_or_else(idempotency_in_progress)?;
            result.replayed = true;
            return Ok(result);
        }

        insert_audit(&tx, &command.audit)?;
        let records = if command.audit.allowed {
            write_sqlite_command(&tx, &command)?
        } else {
            vec![]
        };
        let result = ControlCommandResult {
            records,
            response: command.response.clone(),
            replayed: false,
        };
        let serialized = serde_json::to_string(&StoredIdempotencyResponse::Complete {
            completion_checksum: request_checksum.clone(),
            result: result.clone(),
        })
        .map_err(|_| corrupt_record())?;
        tx.execute(
            "INSERT INTO buglogin_control_idempotency (tenant_id,idempotency_key,request_checksum,response) VALUES (?1,?2,?3,?4)",
            params![command.tenant_id, command.idempotency_key, request_checksum, serialized],
        ).map_err(|_| unavailable())?;
        tx.commit().map_err(|_| unavailable())?;
        Ok(result)
    }
}

impl ControlRecordStore for SqliteControlRecordStore {
    fn readiness(&mut self) -> Result<(), Problem> {
        self.connection
            .query_row("SELECT 1", [], |_| Ok(()))
            .map_err(|_| unavailable())
    }

    fn get(
        &mut self,
        tenant_id: &str,
        record_type: &str,
        record_id: &str,
    ) -> Result<Option<ControlRecord>, Problem> {
        let record_key = key(tenant_id, record_type, record_id)?;
        load_sqlite_record(&self.connection, &record_key)
    }

    fn revision(
        &mut self,
        tenant_id: &str,
        record_type: &str,
        record_id: &str,
    ) -> Result<Option<u64>, Problem> {
        key(tenant_id, record_type, record_id)?;
        let revision: Option<i64> = self.connection.query_row(
            "SELECT revision FROM buglogin_control_records WHERE tenant_id=?1 AND record_type=?2 AND record_id=?3",
            params![tenant_id, record_type, record_id], |row| row.get(0),
        ).optional().map_err(|_| unavailable())?;
        revision
            .map(|value| {
                if value > 0 {
                    u64::try_from(value).map_err(|_| corrupt_record())
                } else {
                    Err(corrupt_record())
                }
            })
            .transpose()
    }

    fn list(&mut self, tenant_id: &str, record_type: &str) -> Result<Vec<ControlRecord>, Problem> {
        validate_scope(tenant_id, record_type, "list")?;
        load_sqlite_list(
            &self.connection,
            "tenant_id=?1 AND record_type=?2",
            params![tenant_id, record_type],
        )
    }

    fn list_for_tenants(
        &mut self,
        tenant_ids: &[String],
        record_type: &str,
    ) -> Result<Vec<ControlRecord>, Problem> {
        for tenant_id in tenant_ids {
            validate_scope(tenant_id, record_type, "list")?;
        }
        let mut records = Vec::new();
        for tenant_id in tenant_ids {
            records.extend(self.list(tenant_id, record_type)?);
        }
        records.sort_by(|a, b| (&a.tenant_id, &a.record_id).cmp(&(&b.tenant_id, &b.record_id)));
        Ok(records)
    }

    fn list_for_account(
        &mut self,
        tenant_id: &str,
        record_type: &str,
        account_id: &str,
    ) -> Result<Vec<ControlRecord>, Problem> {
        validate_scope(tenant_id, record_type, account_id)?;
        Ok(self
            .list(tenant_id, record_type)?
            .into_iter()
            .filter(|record| {
                record.value.get("accountId").and_then(Value::as_str) == Some(account_id)
            })
            .collect())
    }

    fn replay(
        &mut self,
        tenant_id: &str,
        idempotency_key: &str,
    ) -> Result<Option<ControlCommandResult>, Problem> {
        validate_scope(tenant_id, idempotency_key, "idempotency")?;
        let response: Option<String> = self.connection.query_row(
            "SELECT response FROM buglogin_control_idempotency WHERE tenant_id=?1 AND idempotency_key=?2",
            params![tenant_id, idempotency_key], |row| row.get(0),
        ).optional().map_err(|_| unavailable())?;
        match response {
            Some(serialized) => Ok(decode_completed_response(decode_json(&serialized)?)?.map(
                |mut result| {
                    result.replayed = true;
                    result
                },
            )),
            None => Ok(None),
        }
    }

    fn claim_idempotency(
        &mut self,
        command: ClaimControlCommand,
    ) -> Result<ControlIdempotencyClaim, Problem> {
        validate_claim(&command)?;
        let pending = serde_json::to_string(&StoredIdempotencyResponse::Pending {
            claim_token: command.claim_token.clone(),
            operation_id: command.operation_id.clone(),
        })
        .map_err(|_| corrupt_record())?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| unavailable())?;
        let inserted = tx.execute(
            "INSERT INTO buglogin_control_idempotency (tenant_id,idempotency_key,request_checksum,response) VALUES (?1,?2,?3,?4) ON CONFLICT (tenant_id,idempotency_key) DO NOTHING",
            params![command.tenant_id, command.idempotency_key, command.request_checksum, pending],
        ).map_err(|_| unavailable())?;
        if inserted == 1 {
            tx.commit().map_err(|_| unavailable())?;
            return Ok(ControlIdempotencyClaim::Claimed {
                claim: ControlIdempotencyClaimToken {
                    tenant_id: command.tenant_id,
                    idempotency_key: command.idempotency_key,
                    request_checksum: command.request_checksum,
                    claim_token: command.claim_token,
                    operation_id: command.operation_id,
                },
            });
        }
        let (stored_checksum, response): (String, String) = tx.query_row(
            "SELECT request_checksum,response FROM buglogin_control_idempotency WHERE tenant_id=?1 AND idempotency_key=?2",
            params![command.tenant_id, command.idempotency_key], |row| Ok((row.get(0)?,row.get(1)?)),
        ).map_err(|_| unavailable())?;
        if stored_checksum != command.request_checksum {
            return Err(idempotency_conflict());
        }
        let stored = decode_stored_response(decode_json(&response)?)?;
        tx.commit().map_err(|_| unavailable())?;
        Ok(match stored {
            StoredIdempotencyResponse::Pending { operation_id, .. } => {
                ControlIdempotencyClaim::Pending { operation_id }
            }
            StoredIdempotencyResponse::Complete { mut result, .. } => {
                result.replayed = true;
                ControlIdempotencyClaim::Replay { result }
            }
        })
    }

    fn abandon_idempotency(&mut self, claim: &ControlIdempotencyClaimToken) -> Result<(), Problem> {
        validate_scope(&claim.tenant_id, &claim.idempotency_key, "idempotency")?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| unavailable())?;
        let stored: Option<(String,String)> = tx.query_row(
            "SELECT request_checksum,response FROM buglogin_control_idempotency WHERE tenant_id=?1 AND idempotency_key=?2",
            params![claim.tenant_id, claim.idempotency_key], |row| Ok((row.get(0)?,row.get(1)?)),
        ).optional().map_err(|_| unavailable())?;
        let (checksum, response) = stored.ok_or_else(idempotency_claim_missing)?;
        if checksum != claim.request_checksum {
            return Err(idempotency_conflict());
        }
        match decode_stored_response(decode_json(&response)?)? {
            StoredIdempotencyResponse::Pending { claim_token, .. }
                if claim_token == claim.claim_token => {}
            StoredIdempotencyResponse::Pending { .. } => return Err(idempotency_claim_conflict()),
            StoredIdempotencyResponse::Complete { .. } => return Err(idempotency_conflict()),
        }
        let deleted = tx.execute(
            "DELETE FROM buglogin_control_idempotency WHERE tenant_id=?1 AND idempotency_key=?2 AND request_checksum=?3",
            params![claim.tenant_id, claim.idempotency_key, claim.request_checksum],
        ).map_err(|_| unavailable())?;
        if deleted != 1 {
            return Err(idempotency_claim_conflict());
        }
        tx.commit().map_err(|_| unavailable())
    }

    fn complete_idempotency(
        &mut self,
        command: CompleteControlCommand,
    ) -> Result<ControlCommandResult, Problem> {
        validate_complete(&command)?;
        let completion_checksum = complete_checksum(&command)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| unavailable())?;
        let stored: Option<(String,String)> = tx.query_row(
            "SELECT request_checksum,response FROM buglogin_control_idempotency WHERE tenant_id=?1 AND idempotency_key=?2",
            params![command.claim.tenant_id, command.claim.idempotency_key], |row| Ok((row.get(0)?,row.get(1)?)),
        ).optional().map_err(|_| unavailable())?;
        let (request_checksum, response) = stored.ok_or_else(idempotency_claim_missing)?;
        if request_checksum != command.claim.request_checksum {
            return Err(idempotency_conflict());
        }
        match decode_stored_response(decode_json(&response)?)? {
            StoredIdempotencyResponse::Complete {
                completion_checksum: saved,
                mut result,
            } => {
                if saved != completion_checksum {
                    return Err(idempotency_conflict());
                }
                result.replayed = true;
                return Ok(result);
            }
            StoredIdempotencyResponse::Pending { claim_token, .. }
                if claim_token != command.claim.claim_token =>
            {
                return Err(idempotency_claim_conflict());
            }
            StoredIdempotencyResponse::Pending { .. } => {}
        }
        insert_audit(&tx, &command.audit)?;
        let records = if command.apply_mutations {
            write_sqlite_complete(&tx, &command)?
        } else {
            vec![]
        };
        let result = ControlCommandResult {
            records,
            response: command.response.clone(),
            replayed: false,
        };
        let complete = serde_json::to_string(&StoredIdempotencyResponse::Complete {
            completion_checksum,
            result: result.clone(),
        })
        .map_err(|_| corrupt_record())?;
        tx.execute("UPDATE buglogin_control_idempotency SET response=?3 WHERE tenant_id=?1 AND idempotency_key=?2", params![command.claim.tenant_id, command.claim.idempotency_key, complete]).map_err(|_| unavailable())?;
        tx.commit().map_err(|_| unavailable())?;
        Ok(result)
    }

    fn execute(&mut self, command: ExecuteControlCommand) -> Result<ControlCommandResult, Problem> {
        self.execute_tx(command)
    }

    fn audit(&mut self, tenant_id: &str) -> Result<Vec<ControlAuditEntry>, Problem> {
        validate_scope(tenant_id, "audit", "list")?;
        let mut statement = self.connection.prepare("SELECT command_id,actor_id,allowed,detail FROM buglogin_control_audit WHERE tenant_id=?1 ORDER BY sequence").map_err(|_| unavailable())?;
        let rows = statement
            .query_map(params![tenant_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .map_err(|_| unavailable())?;
        rows.map(|row| {
            let (command_id, actor_id, allowed, detail) = row.map_err(|_| unavailable())?;
            Ok(ControlAuditEntry {
                tenant_id: tenant_id.into(),
                command_id,
                actor_id,
                allowed: allowed != 0,
                detail: decode_json(&detail)?,
            })
        })
        .collect()
    }

    fn try_advisory_lock(&mut self, lock_id: i64) -> Result<bool, Problem> {
        if self.advisory_locks.contains_key(&lock_id) {
            return Ok(false);
        }
        let path = self.lock_path(lock_id)?;
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_FULL_MUTEX,
        )
        .map_err(|_| unavailable())?;
        connection
            .busy_timeout(std::time::Duration::ZERO)
            .map_err(|_| unavailable())?;
        match connection.execute_batch("BEGIN EXCLUSIVE") {
            Ok(()) => {
                self.advisory_locks.insert(lock_id, connection);
                Ok(true)
            }
            Err(error)
                if matches!(
                    error.sqlite_error_code(),
                    Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked)
                ) =>
            {
                Ok(false)
            }
            Err(_) => Err(unavailable()),
        }
    }

    fn release_advisory_lock(&mut self, lock_id: i64) -> Result<(), Problem> {
        let connection = self
            .advisory_locks
            .remove(&lock_id)
            .ok_or_else(unavailable)?;
        connection
            .execute_batch("ROLLBACK")
            .map_err(|_| unavailable())
    }
}

fn decode_json(serialized: &str) -> Result<Value, Problem> {
    serde_json::from_str(serialized).map_err(|_| corrupt_record())
}

fn load_sqlite_record(
    connection: &Connection,
    key: &ControlRecordKey,
) -> Result<Option<ControlRecord>, Problem> {
    let row: Option<(i64,String,String)> = connection.query_row(
        "SELECT revision,checksum,value FROM buglogin_control_records WHERE tenant_id=?1 AND record_type=?2 AND record_id=?3",
        params![key.tenant_id,key.record_type,key.record_id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
    ).optional().map_err(|_| unavailable())?;
    row.map(|(revision, checksum, serialized)| {
        decode_sqlite_record(key, revision, checksum, &serialized)
    })
    .transpose()
}

fn load_sqlite_list<P: rusqlite::Params>(
    connection: &Connection,
    predicate: &str,
    values: P,
) -> Result<Vec<ControlRecord>, Problem> {
    let sql = format!(
        "SELECT tenant_id,record_type,record_id,revision,checksum,value FROM buglogin_control_records WHERE {predicate} ORDER BY tenant_id,record_type,record_id"
    );
    let mut statement = connection.prepare(&sql).map_err(|_| unavailable())?;
    let rows = statement
        .query_map(values, |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        })
        .map_err(|_| unavailable())?;
    rows.map(|row| {
        let (tenant, kind, id, revision, checksum, value) = row.map_err(|_| unavailable())?;
        decode_sqlite_record(
            &ControlRecordKey {
                tenant_id: tenant,
                record_type: kind,
                record_id: id,
            },
            revision,
            checksum,
            &value,
        )
    })
    .collect()
}

fn decode_sqlite_record(
    key: &ControlRecordKey,
    revision: i64,
    stored_checksum: String,
    serialized: &str,
) -> Result<ControlRecord, Problem> {
    let value: Value = serde_json::from_str(serialized).map_err(|_| corrupt_record())?;
    let bytes = serde_json::to_vec(&value).map_err(|_| corrupt_record())?;
    if revision <= 0 || checksum(&bytes) != stored_checksum {
        return Err(corrupt_record());
    }
    Ok(ControlRecord {
        tenant_id: key.tenant_id.clone(),
        record_type: key.record_type.clone(),
        record_id: key.record_id.clone(),
        revision: u64::try_from(revision).map_err(|_| corrupt_record())?,
        checksum: stored_checksum,
        value,
    })
}

fn insert_audit(tx: &Transaction<'_>, audit: &ControlAuditEntry) -> Result<(), Problem> {
    let detail = serde_json::to_string(&audit.detail).map_err(|_| corrupt_record())?;
    tx.execute("INSERT INTO buglogin_control_audit (tenant_id,command_id,actor_id,allowed,detail) VALUES (?1,?2,?3,?4,?5)", params![audit.tenant_id,audit.command_id,audit.actor_id,audit.allowed as i64,detail]).map_err(|_| unavailable())?;
    Ok(())
}

fn write_sqlite_command(
    tx: &Transaction<'_>,
    command: &ExecuteControlCommand,
) -> Result<Vec<ControlRecord>, Problem> {
    for (expected_key, expected_revision) in &command.expected_revisions {
        let actual = load_sqlite_record(tx, expected_key)?.map_or(0, |record| record.revision);
        check_revision(&command.expected_revisions, expected_key, actual)?;
        debug_assert_eq!(actual, *expected_revision);
    }
    let mut records = Vec::new();
    for put in &command.puts {
        let record_key = key(&put.tenant_id, &put.record_type, &put.record_id)?;
        let actual = load_sqlite_record(tx, &record_key)?.map_or(0, |record| record.revision);
        check_revision(&command.expected_revisions, &record_key, actual)?;
        check_revision_option(put.expected_revision, actual)?;
        let record = make_record(&record_key, actual, &put.value)?;
        let revision = i64::try_from(record.revision).map_err(|_| corrupt_record())?;
        let serialized = serde_json::to_string(&record.value).map_err(|_| corrupt_record())?;
        let affected = if actual == 0 {
            tx.execute("INSERT INTO buglogin_control_records (tenant_id,record_type,record_id,revision,checksum,value) VALUES (?1,?2,?3,?4,?5,?6) ON CONFLICT (tenant_id,record_type,record_id) DO NOTHING", params![record.tenant_id,record.record_type,record.record_id,revision,record.checksum,serialized]).map_err(|_| unavailable())?
        } else {
            tx.execute("UPDATE buglogin_control_records SET revision=?4,checksum=?5,value=?6 WHERE tenant_id=?1 AND record_type=?2 AND record_id=?3 AND revision=?7", params![record.tenant_id,record.record_type,record.record_id,revision,record.checksum,serialized,i64::try_from(actual).map_err(|_| corrupt_record())?]).map_err(|_| unavailable())?
        };
        require_sqlite_write(tx, &record_key, actual, affected)?;
        records.push(record);
    }
    for delete in &command.deletes {
        let record_key = key(&delete.tenant_id, &delete.record_type, &delete.record_id)?;
        let actual = load_sqlite_record(tx, &record_key)?.map_or(0, |record| record.revision);
        check_revision(&command.expected_revisions, &record_key, actual)?;
        check_revision_option(delete.expected_revision, actual)?;
        let affected = tx.execute("DELETE FROM buglogin_control_records WHERE tenant_id=?1 AND record_type=?2 AND record_id=?3 AND revision=?4", params![record_key.tenant_id,record_key.record_type,record_key.record_id,i64::try_from(actual).map_err(|_| corrupt_record())?]).map_err(|_| unavailable())?;
        if delete.expected_revision.is_some() && actual > 0 {
            require_sqlite_write(tx, &record_key, actual, affected)?;
        }
    }
    Ok(records)
}

fn write_sqlite_complete(
    tx: &Transaction<'_>,
    command: &CompleteControlCommand,
) -> Result<Vec<ControlRecord>, Problem> {
    write_sqlite_command(
        tx,
        &ExecuteControlCommand {
            tenant_id: command.claim.tenant_id.clone(),
            command_id: command.command_id.clone(),
            idempotency_key: command.claim.idempotency_key.clone(),
            expected_revisions: command.expected_revisions.clone(),
            puts: command.puts.clone(),
            deletes: command.deletes.clone(),
            audit: command.audit.clone(),
            response: command.response.clone(),
        },
    )
}

fn require_sqlite_write(
    tx: &Transaction<'_>,
    key: &ControlRecordKey,
    expected_revision: u64,
    affected: usize,
) -> Result<(), Problem> {
    match affected {
        1 => Ok(()),
        0 => {
            let actual = load_sqlite_record(tx, key)?.map_or(0, |record| record.revision);
            Err(revision_conflict(Some(expected_revision), actual))
        }
        _ => Err(corrupt_record()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(1);

    fn database_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "buglogin-sqlite-{label}-{}-{}.db",
            std::process::id(),
            TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn remove_database(path: &Path) {
        let _ = fs::remove_file(path);
        let _ = fs::remove_file(path.with_extension("db-wal"));
        let _ = fs::remove_file(path.with_extension("db-shm"));
        for lock in fs::read_dir(path.parent().unwrap_or_else(|| Path::new(".")))
            .into_iter()
            .flatten()
            .flatten()
        {
            if lock.file_name().to_string_lossy().starts_with(&format!(
                ".{}.advisory-",
                path.file_name().unwrap().to_string_lossy()
            )) {
                let _ = fs::remove_file(lock.path());
            }
        }
    }

    fn command(idempotency_key: &str, puts: Vec<ControlPut>) -> ExecuteControlCommand {
        ExecuteControlCommand {
            tenant_id: "tenant".into(),
            command_id: "record.write".into(),
            idempotency_key: idempotency_key.into(),
            expected_revisions: BTreeMap::new(),
            puts,
            deletes: vec![],
            audit: ControlAuditEntry {
                tenant_id: "tenant".into(),
                command_id: "record.write".into(),
                actor_id: "actor".into(),
                allowed: true,
                detail: Value::Null,
            },
            response: HttpResponse {
                status: 200,
                headers: BTreeMap::new(),
                body: Value::Null,
            },
        }
    }

    fn put(record_id: &str, expected_revision: Option<u64>, value: Value) -> ControlPut {
        ControlPut {
            tenant_id: "tenant".into(),
            record_type: "profile".into(),
            record_id: record_id.into(),
            expected_revision,
            value,
        }
    }

    #[test]
    fn records_and_idempotent_receipts_survive_reopen() {
        let path = database_path("reopen");
        let mut store = SqliteControlRecordStore::open(&path).unwrap();
        let request = command(
            "sqlite-reopen-request-0001",
            vec![put("one", Some(0), serde_json::json!({"v": 1}))],
        );
        let created = store.execute(request.clone()).unwrap();
        assert_eq!(created.records[0].revision, 1);
        drop(store);

        let mut reopened = SqliteControlRecordStore::open(&path).unwrap();
        let record = reopened.get("tenant", "profile", "one").unwrap().unwrap();
        assert_eq!(record.value, serde_json::json!({"v": 1}));
        let replay = reopened.execute(request).unwrap();
        assert!(replay.replayed);
        assert_eq!(reopened.audit("tenant").unwrap().len(), 1);
        drop(reopened);
        remove_database(&path);
    }

    #[test]
    fn revision_conflict_rolls_back_all_records_audit_and_receipt() {
        let path = database_path("rollback");
        let mut store = SqliteControlRecordStore::open(&path).unwrap();
        store
            .execute(command(
                "sqlite-rollback-seed-0001",
                vec![put("two", Some(0), serde_json::json!({"v": 2}))],
            ))
            .unwrap();
        let audit_before = store.audit("tenant").unwrap().len();
        let failing = command(
            "sqlite-rollback-fail-0001",
            vec![
                put("one", Some(0), serde_json::json!({"v": 1})),
                put("two", Some(0), serde_json::json!({"v": 3})),
            ],
        );
        assert_eq!(
            store.execute(failing.clone()).unwrap_err().code,
            "storage_revision_conflict"
        );
        assert!(store.get("tenant", "profile", "one").unwrap().is_none());
        assert_eq!(store.audit("tenant").unwrap().len(), audit_before);
        store
            .execute(command(
                "sqlite-rollback-fail-0001",
                vec![
                    put("one", Some(0), serde_json::json!({"v": 1})),
                    put("two", Some(1), serde_json::json!({"v": 3})),
                ],
            ))
            .unwrap();
        drop(store);
        remove_database(&path);
    }

    #[test]
    fn pending_idempotency_claim_survives_reopen_and_fences_owner() {
        let path = database_path("claim");
        let claim_command = ClaimControlCommand {
            tenant_id: "tenant".into(),
            idempotency_key: "sqlite-pending-request-0001".into(),
            request_checksum: "a".repeat(64),
            claim_token: "sqlite-claim-token-0001".into(),
            operation_id: "sqlite-operation-0001".into(),
        };
        let mut store = SqliteControlRecordStore::open(&path).unwrap();
        assert!(matches!(
            store.claim_idempotency(claim_command.clone()).unwrap(),
            ControlIdempotencyClaim::Claimed { .. }
        ));
        drop(store);

        let mut reopened = SqliteControlRecordStore::open(&path).unwrap();
        assert!(matches!(
            reopened.claim_idempotency(claim_command.clone()).unwrap(),
            ControlIdempotencyClaim::Pending { operation_id } if operation_id == "sqlite-operation-0001"
        ));
        let wrong_owner = ControlIdempotencyClaimToken {
            tenant_id: claim_command.tenant_id.clone(),
            idempotency_key: claim_command.idempotency_key.clone(),
            request_checksum: claim_command.request_checksum.clone(),
            claim_token: "sqlite-claim-token-wrong1".into(),
            operation_id: claim_command.operation_id.clone(),
        };
        assert_eq!(
            reopened.abandon_idempotency(&wrong_owner).unwrap_err().code,
            "idempotency_claim_conflict"
        );
        drop(reopened);
        remove_database(&path);
    }

    #[test]
    fn advisory_locks_are_shared_between_connections_and_release_cleanly() {
        let path = database_path("lock");
        let mut first = SqliteControlRecordStore::open(&path).unwrap();
        let mut second = SqliteControlRecordStore::open(&path).unwrap();
        assert!(first.try_advisory_lock(42).unwrap());
        assert!(!second.try_advisory_lock(42).unwrap());
        first.release_advisory_lock(42).unwrap();
        assert!(second.try_advisory_lock(42).unwrap());
        second.release_advisory_lock(42).unwrap();
        drop(first);
        drop(second);
        remove_database(&path);
    }
}
