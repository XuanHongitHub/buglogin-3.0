use std::collections::BTreeMap;
use std::env;
use std::fmt;
use std::sync::{Arc, Mutex};

use buglogin_domain::errors::Problem;
use native_tls::TlsConnector;
use postgres::{Client, GenericClient, Transaction};
use postgres_native_tls::MakeTlsConnector;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::blob::checksum;

const CORRELATION: &str = "storage";

#[derive(Clone)]
pub struct PostgresConfig {
    database_url: String,
}
impl PostgresConfig {
    pub fn new(database_url: impl Into<String>) -> Result<Self, Problem> {
        let database_url = database_url.into();
        let normalized = database_url.to_ascii_lowercase();
        if database_url.trim().is_empty()
            || ![
                "sslmode=require",
                "sslmode=verify-ca",
                "sslmode=verify-full",
            ]
            .iter()
            .any(|mode| normalized.contains(mode))
        {
            return Err(problem(
                "invalid_postgres_config",
                "PostgreSQL TLS configuration is required",
                400,
            ));
        }
        Ok(Self { database_url })
    }
    pub fn from_env() -> Result<Self, Problem> {
        Self::new(env::var("BUGLOGIN_POSTGRES_URL").map_err(|_| {
            problem(
                "missing_postgres_config",
                "PostgreSQL configuration is missing",
                500,
            )
        })?)
    }
}
impl fmt::Debug for PostgresConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PostgresConfig")
            .field("database_url", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlRecordKey {
    pub tenant_id: String,
    pub record_type: String,
    pub record_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlRecord {
    pub tenant_id: String,
    pub record_type: String,
    pub record_id: String,
    pub revision: u64,
    pub checksum: String,
    pub value: Value,
}

#[derive(Clone, Debug)]
pub struct PutControlRecord<'a> {
    pub tenant_id: &'a str,
    pub record_type: &'a str,
    pub record_id: &'a str,
    pub expected_revision: Option<u64>,
    pub idempotency_key: &'a str,
    pub value: &'a Value,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlPut {
    pub tenant_id: String,
    pub record_type: String,
    pub record_id: String,
    pub expected_revision: Option<u64>,
    pub value: Value,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlDelete {
    pub tenant_id: String,
    pub record_type: String,
    pub record_id: String,
    pub expected_revision: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpResponse {
    pub status: u16,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    pub body: Value,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlAuditEntry {
    pub tenant_id: String,
    pub command_id: String,
    pub actor_id: String,
    pub allowed: bool,
    pub detail: Value,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecuteControlCommand {
    pub tenant_id: String,
    pub command_id: String,
    pub idempotency_key: String,
    #[serde(default)]
    pub expected_revisions: BTreeMap<ControlRecordKey, u64>,
    #[serde(default)]
    pub puts: Vec<ControlPut>,
    #[serde(default)]
    pub deletes: Vec<ControlDelete>,
    pub audit: ControlAuditEntry,
    pub response: HttpResponse,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlCommandResult {
    pub records: Vec<ControlRecord>,
    pub response: HttpResponse,
    pub replayed: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaimControlCommand {
    pub tenant_id: String,
    pub idempotency_key: String,
    pub request_checksum: String,
    pub claim_token: String,
    pub operation_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlIdempotencyClaimToken {
    pub tenant_id: String,
    pub idempotency_key: String,
    pub request_checksum: String,
    pub claim_token: String,
    pub operation_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "state")]
pub enum ControlIdempotencyClaim {
    Claimed { claim: ControlIdempotencyClaimToken },
    Pending { operation_id: String },
    Replay { result: ControlCommandResult },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompleteControlCommand {
    pub claim: ControlIdempotencyClaimToken,
    pub command_id: String,
    #[serde(default)]
    pub expected_revisions: BTreeMap<ControlRecordKey, u64>,
    #[serde(default)]
    pub puts: Vec<ControlPut>,
    #[serde(default)]
    pub deletes: Vec<ControlDelete>,
    pub apply_mutations: bool,
    pub audit: ControlAuditEntry,
    pub response: HttpResponse,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "state")]
enum StoredIdempotencyResponse {
    Pending {
        claim_token: String,
        operation_id: String,
    },
    Complete {
        completion_checksum: String,
        result: ControlCommandResult,
    },
}

pub trait ControlRecordStore {
    fn readiness(&mut self) -> Result<(), Problem> {
        Ok(())
    }

    fn get(
        &mut self,
        tenant_id: &str,
        record_type: &str,
        record_id: &str,
    ) -> Result<Option<ControlRecord>, Problem>;
    fn list(&mut self, tenant_id: &str, record_type: &str) -> Result<Vec<ControlRecord>, Problem>;
    fn replay(
        &mut self,
        _tenant_id: &str,
        _idempotency_key: &str,
    ) -> Result<Option<ControlCommandResult>, Problem> {
        Ok(None)
    }
    fn claim_idempotency(
        &mut self,
        command: ClaimControlCommand,
    ) -> Result<ControlIdempotencyClaim, Problem>;
    fn complete_idempotency(
        &mut self,
        command: CompleteControlCommand,
    ) -> Result<ControlCommandResult, Problem>;
    fn execute(&mut self, command: ExecuteControlCommand) -> Result<ControlCommandResult, Problem>;
    fn audit(&mut self, tenant_id: &str) -> Result<Vec<ControlAuditEntry>, Problem>;
    fn put(&mut self, request: PutControlRecord<'_>) -> Result<ControlRecord, Problem> {
        let command = ExecuteControlCommand {
            tenant_id: request.tenant_id.to_owned(),
            command_id: "control.record.put".into(),
            idempotency_key: request.idempotency_key.into(),
            expected_revisions: BTreeMap::new(),
            puts: vec![ControlPut {
                tenant_id: request.tenant_id.into(),
                record_type: request.record_type.into(),
                record_id: request.record_id.into(),
                expected_revision: request.expected_revision,
                value: request.value.clone(),
            }],
            deletes: vec![],
            audit: ControlAuditEntry {
                tenant_id: request.tenant_id.into(),
                command_id: "control.record.put".into(),
                actor_id: "storage.compat".into(),
                allowed: true,
                detail: Value::Null,
            },
            response: HttpResponse {
                status: 200,
                headers: BTreeMap::new(),
                body: Value::Null,
            },
        };
        self.execute(command)?
            .records
            .into_iter()
            .next()
            .ok_or_else(unavailable)
    }
}

#[derive(Clone, Default)]
struct MemoryState {
    records: BTreeMap<ControlRecordKey, ControlRecord>,
    idempotency: BTreeMap<(String, String), (String, StoredIdempotencyResponse)>,
    audit: Vec<ControlAuditEntry>,
}
#[derive(Clone, Default)]
pub struct MemoryControlRecordStore {
    state: Arc<Mutex<MemoryState>>,
}
impl MemoryControlRecordStore {
    pub fn reopen(&self) -> Self {
        self.clone()
    }
}

impl ControlRecordStore for MemoryControlRecordStore {
    fn get(
        &mut self,
        tenant_id: &str,
        record_type: &str,
        record_id: &str,
    ) -> Result<Option<ControlRecord>, Problem> {
        let key = key(tenant_id, record_type, record_id)?;
        Ok(self
            .state
            .lock()
            .map_err(|_| unavailable())?
            .records
            .get(&key)
            .cloned())
    }
    fn list(&mut self, tenant_id: &str, record_type: &str) -> Result<Vec<ControlRecord>, Problem> {
        validate_scope(tenant_id, record_type, "list")?;
        Ok(self
            .state
            .lock()
            .map_err(|_| unavailable())?
            .records
            .values()
            .filter(|record| record.tenant_id == tenant_id && record.record_type == record_type)
            .cloned()
            .collect())
    }
    fn replay(
        &mut self,
        tenant_id: &str,
        idempotency_key: &str,
    ) -> Result<Option<ControlCommandResult>, Problem> {
        validate_scope(tenant_id, idempotency_key, "idempotency")?;
        Ok(self
            .state
            .lock()
            .map_err(|_| unavailable())?
            .idempotency
            .get(&(tenant_id.to_owned(), idempotency_key.to_owned()))
            .and_then(|(_, stored)| match stored {
                StoredIdempotencyResponse::Complete { result, .. } => {
                    let mut replay = result.clone();
                    replay.replayed = true;
                    Some(replay)
                }
                StoredIdempotencyResponse::Pending { .. } => None,
            }))
    }
    fn claim_idempotency(
        &mut self,
        command: ClaimControlCommand,
    ) -> Result<ControlIdempotencyClaim, Problem> {
        validate_claim(&command)?;
        let mut state = self.state.lock().map_err(|_| unavailable())?;
        let key = (command.tenant_id.clone(), command.idempotency_key.clone());
        if let Some((stored_checksum, stored)) = state.idempotency.get(&key) {
            if stored_checksum != &command.request_checksum {
                return Err(idempotency_conflict());
            }
            return Ok(match stored {
                StoredIdempotencyResponse::Pending { operation_id, .. } => {
                    ControlIdempotencyClaim::Pending {
                        operation_id: operation_id.clone(),
                    }
                }
                StoredIdempotencyResponse::Complete { result, .. } => {
                    let mut replay = result.clone();
                    replay.replayed = true;
                    ControlIdempotencyClaim::Replay { result: replay }
                }
            });
        }
        state.idempotency.insert(
            key,
            (
                command.request_checksum.clone(),
                StoredIdempotencyResponse::Pending {
                    claim_token: command.claim_token.clone(),
                    operation_id: command.operation_id.clone(),
                },
            ),
        );
        Ok(ControlIdempotencyClaim::Claimed {
            claim: ControlIdempotencyClaimToken {
                tenant_id: command.tenant_id,
                idempotency_key: command.idempotency_key,
                request_checksum: command.request_checksum,
                claim_token: command.claim_token,
                operation_id: command.operation_id,
            },
        })
    }
    fn complete_idempotency(
        &mut self,
        command: CompleteControlCommand,
    ) -> Result<ControlCommandResult, Problem> {
        validate_complete(&command)?;
        let completion_checksum = complete_checksum(&command)?;
        let mut state = self.state.lock().map_err(|_| unavailable())?;
        let key = (
            command.claim.tenant_id.clone(),
            command.claim.idempotency_key.clone(),
        );
        let Some((stored_checksum, stored)) = state.idempotency.get(&key).cloned() else {
            return Err(idempotency_claim_missing());
        };
        if stored_checksum != command.claim.request_checksum {
            return Err(idempotency_conflict());
        }
        if let StoredIdempotencyResponse::Complete {
            completion_checksum: stored_completion,
            result,
        } = stored
        {
            if stored_completion != completion_checksum {
                return Err(idempotency_conflict());
            }
            let mut replay = result;
            replay.replayed = true;
            return Ok(replay);
        }
        let StoredIdempotencyResponse::Pending { claim_token, .. } = stored else {
            return Err(idempotency_conflict());
        };
        if claim_token != command.claim.claim_token {
            return Err(idempotency_claim_conflict());
        }
        let mut next = state.clone();
        next.audit.push(command.audit.clone());
        let records = if command.apply_mutations {
            apply_complete_command(&mut next, &command)?
        } else {
            vec![]
        };
        let result = ControlCommandResult {
            records,
            response: command.response,
            replayed: false,
        };
        next.idempotency.insert(
            key,
            (
                command.claim.request_checksum,
                StoredIdempotencyResponse::Complete {
                    completion_checksum,
                    result: result.clone(),
                },
            ),
        );
        *state = next;
        Ok(result)
    }
    fn execute(&mut self, command: ExecuteControlCommand) -> Result<ControlCommandResult, Problem> {
        validate_command(&command)?;
        let request_checksum = command_checksum(&command)?;
        let mut state = self.state.lock().map_err(|_| unavailable())?;
        if let Some((stored, result)) = state
            .idempotency
            .get(&(command.tenant_id.clone(), command.idempotency_key.clone()))
        {
            if stored != &request_checksum {
                return Err(idempotency_conflict());
            }
            if let StoredIdempotencyResponse::Complete { result, .. } = result {
                let mut replay = result.clone();
                replay.replayed = true;
                return Ok(replay);
            }
            return Err(idempotency_in_progress());
        }
        let mut next = state.clone();
        next.audit.push(command.audit.clone());
        if !command.audit.allowed {
            let result = ControlCommandResult {
                records: vec![],
                response: command.response.clone(),
                replayed: false,
            };
            next.idempotency.insert(
                (command.tenant_id.clone(), command.idempotency_key.clone()),
                (
                    request_checksum,
                    StoredIdempotencyResponse::Complete {
                        completion_checksum: command_checksum(&command)?,
                        result: result.clone(),
                    },
                ),
            );
            *state = next;
            return Ok(result);
        }
        let result = ControlCommandResult {
            records: apply_command(&mut next, &command)?,
            response: command.response.clone(),
            replayed: false,
        };
        next.idempotency.insert(
            (command.tenant_id.clone(), command.idempotency_key.clone()),
            (
                request_checksum,
                StoredIdempotencyResponse::Complete {
                    completion_checksum: command_checksum(&command)?,
                    result: result.clone(),
                },
            ),
        );
        *state = next;
        Ok(result)
    }
    fn audit(&mut self, tenant_id: &str) -> Result<Vec<ControlAuditEntry>, Problem> {
        validate_scope(tenant_id, "audit", "list")?;
        Ok(self
            .state
            .lock()
            .map_err(|_| unavailable())?
            .audit
            .iter()
            .filter(|entry| entry.tenant_id == tenant_id)
            .cloned()
            .collect())
    }
}

pub struct PostgresControlRecordStore {
    client: Client,
}
impl PostgresControlRecordStore {
    pub fn connect(config: &PostgresConfig) -> Result<Self, Problem> {
        let connector = TlsConnector::builder().build().map_err(|_| unavailable())?;
        let client = Client::connect(&config.database_url, MakeTlsConnector::new(connector))
            .map_err(|_| unavailable())?;
        Ok(Self { client })
    }
    pub fn ensure_schema(&mut self) -> Result<(), Problem> {
        self.client.batch_execute("CREATE TABLE IF NOT EXISTS buglogin_control_records (tenant_id TEXT NOT NULL, record_type TEXT NOT NULL, record_id TEXT NOT NULL, revision BIGINT NOT NULL CHECK (revision > 0), checksum CHAR(64) NOT NULL, value JSONB NOT NULL, PRIMARY KEY (tenant_id, record_type, record_id)); CREATE TABLE IF NOT EXISTS buglogin_control_idempotency (tenant_id TEXT NOT NULL, idempotency_key TEXT NOT NULL, request_checksum CHAR(64) NOT NULL, response JSONB NOT NULL, created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(), PRIMARY KEY (tenant_id, idempotency_key)); CREATE TABLE IF NOT EXISTS buglogin_control_audit (sequence BIGSERIAL PRIMARY KEY, tenant_id TEXT NOT NULL, command_id TEXT NOT NULL, actor_id TEXT NOT NULL, allowed BOOLEAN NOT NULL, detail JSONB NOT NULL, created_at TIMESTAMPTZ NOT NULL DEFAULT NOW());").map_err(|_| unavailable())
    }
}
impl ControlRecordStore for PostgresControlRecordStore {
    fn readiness(&mut self) -> Result<(), Problem> {
        self.client
            .query_one("SELECT 1", &[])
            .map(|_| ())
            .map_err(|_| unavailable())
    }

    fn get(
        &mut self,
        tenant_id: &str,
        record_type: &str,
        record_id: &str,
    ) -> Result<Option<ControlRecord>, Problem> {
        let key = key(tenant_id, record_type, record_id)?;
        load_record(&mut self.client, &key)
    }
    fn list(&mut self, tenant_id: &str, record_type: &str) -> Result<Vec<ControlRecord>, Problem> {
        validate_scope(tenant_id, record_type, "list")?;
        self.client.query("SELECT record_id, revision, checksum, value FROM buglogin_control_records WHERE tenant_id = $1 AND record_type = $2 ORDER BY record_id", &[&tenant_id, &record_type]).map_err(|_| unavailable())?.into_iter().map(|row| decode_record(tenant_id, record_type, row.get(0), row.get(1), row.get(2), row.get(3))).collect()
    }
    fn replay(
        &mut self,
        tenant_id: &str,
        idempotency_key: &str,
    ) -> Result<Option<ControlCommandResult>, Problem> {
        validate_scope(tenant_id, idempotency_key, "idempotency")?;
        self.client
            .query_opt(
                "SELECT response FROM buglogin_control_idempotency WHERE tenant_id = $1 AND idempotency_key = $2",
                &[&tenant_id, &idempotency_key],
            )
            .map_err(|_| unavailable())?
            .map(|row| {
                let response: Value = row.get(0);
                Ok(decode_completed_response(response)?.map(|mut result| {
                    result.replayed = true;
                    result
                }))
            })
            .transpose()
            .map(Option::flatten)
    }
    fn claim_idempotency(
        &mut self,
        command: ClaimControlCommand,
    ) -> Result<ControlIdempotencyClaim, Problem> {
        validate_claim(&command)?;
        let pending = serde_json::to_value(StoredIdempotencyResponse::Pending {
            claim_token: command.claim_token.clone(),
            operation_id: command.operation_id.clone(),
        })
        .map_err(|_| corrupt_record())?;
        let mut tx = self.client.transaction().map_err(|_| unavailable())?;
        let inserted = tx.execute(
            "INSERT INTO buglogin_control_idempotency (tenant_id, idempotency_key, request_checksum, response) VALUES ($1, $2, $3, $4) ON CONFLICT (tenant_id, idempotency_key) DO NOTHING",
            &[&command.tenant_id, &command.idempotency_key, &command.request_checksum, &pending],
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
        let row = tx.query_one(
            "SELECT request_checksum, response FROM buglogin_control_idempotency WHERE tenant_id = $1 AND idempotency_key = $2",
            &[&command.tenant_id, &command.idempotency_key],
        ).map_err(|_| unavailable())?;
        let stored_checksum: String = row.get(0);
        if stored_checksum != command.request_checksum {
            return Err(idempotency_conflict());
        }
        let stored = decode_stored_response(row.get(1))?;
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
    fn complete_idempotency(
        &mut self,
        command: CompleteControlCommand,
    ) -> Result<ControlCommandResult, Problem> {
        validate_complete(&command)?;
        let completion_checksum = complete_checksum(&command)?;
        let mut tx = self.client.transaction().map_err(|_| unavailable())?;
        let row = tx.query_opt(
            "SELECT request_checksum, response FROM buglogin_control_idempotency WHERE tenant_id = $1 AND idempotency_key = $2 FOR UPDATE",
            &[&command.claim.tenant_id, &command.claim.idempotency_key],
        ).map_err(|_| unavailable())?.ok_or_else(idempotency_claim_missing)?;
        let stored_checksum: String = row.get(0);
        if stored_checksum != command.claim.request_checksum {
            return Err(idempotency_conflict());
        }
        match decode_stored_response(row.get(1))? {
            StoredIdempotencyResponse::Complete {
                completion_checksum: stored_completion,
                mut result,
            } => {
                if stored_completion != completion_checksum {
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
        tx.execute(
            "INSERT INTO buglogin_control_audit (tenant_id, command_id, actor_id, allowed, detail) VALUES ($1, $2, $3, $4, $5)",
            &[&command.audit.tenant_id, &command.audit.command_id, &command.audit.actor_id, &command.audit.allowed, &command.audit.detail],
        ).map_err(|_| unavailable())?;
        let records = if command.apply_mutations {
            write_postgres_complete(&mut tx, &command)?
        } else {
            vec![]
        };
        let result = ControlCommandResult {
            records,
            response: command.response.clone(),
            replayed: false,
        };
        let complete = serde_json::to_value(StoredIdempotencyResponse::Complete {
            completion_checksum,
            result: result.clone(),
        })
        .map_err(|_| corrupt_record())?;
        tx.execute(
            "UPDATE buglogin_control_idempotency SET response = $3 WHERE tenant_id = $1 AND idempotency_key = $2",
            &[&command.claim.tenant_id, &command.claim.idempotency_key, &complete],
        ).map_err(|_| unavailable())?;
        tx.commit().map_err(|_| unavailable())?;
        Ok(result)
    }
    fn execute(&mut self, command: ExecuteControlCommand) -> Result<ControlCommandResult, Problem> {
        validate_command(&command)?;
        let request_checksum = command_checksum(&command)?;
        let mut tx = self.client.transaction().map_err(|_| unavailable())?;
        if let Some(row) = tx.query_opt("SELECT request_checksum, response FROM buglogin_control_idempotency WHERE tenant_id = $1 AND idempotency_key = $2 FOR UPDATE", &[&command.tenant_id, &command.idempotency_key]).map_err(|_| unavailable())? { let stored: String = row.get(0); if stored != request_checksum { return Err(idempotency_conflict()); } let response: Value = row.get(1); let Some(mut result) = decode_completed_response(response)? else { return Err(idempotency_in_progress()); }; result.replayed = true; return Ok(result); }
        tx.execute("INSERT INTO buglogin_control_audit (tenant_id, command_id, actor_id, allowed, detail) VALUES ($1, $2, $3, $4, $5)", &[&command.audit.tenant_id, &command.audit.command_id, &command.audit.actor_id, &command.audit.allowed, &command.audit.detail]).map_err(|_| unavailable())?;
        let records = if command.audit.allowed {
            write_postgres_command(&mut tx, &command)?
        } else {
            vec![]
        };
        let result = ControlCommandResult {
            records,
            response: command.response.clone(),
            replayed: false,
        };
        let serialized = serde_json::to_value(StoredIdempotencyResponse::Complete {
            completion_checksum: request_checksum.clone(),
            result: result.clone(),
        })
        .map_err(|_| corrupt_record())?;
        tx.execute("INSERT INTO buglogin_control_idempotency (tenant_id, idempotency_key, request_checksum, response) VALUES ($1, $2, $3, $4)", &[&command.tenant_id, &command.idempotency_key, &request_checksum, &serialized]).map_err(|_| unavailable())?;
        tx.commit().map_err(|_| unavailable())?;
        Ok(result)
    }
    fn audit(&mut self, tenant_id: &str) -> Result<Vec<ControlAuditEntry>, Problem> {
        validate_scope(tenant_id, "audit", "list")?;
        self.client.query("SELECT command_id, actor_id, allowed, detail FROM buglogin_control_audit WHERE tenant_id = $1 ORDER BY sequence", &[&tenant_id]).map_err(|_| unavailable())?.into_iter().map(|row| Ok(ControlAuditEntry { tenant_id: tenant_id.into(), command_id: row.get(0), actor_id: row.get(1), allowed: row.get(2), detail: row.get(3) })).collect()
    }
}

fn write_postgres_command(
    tx: &mut Transaction<'_>,
    command: &ExecuteControlCommand,
) -> Result<Vec<ControlRecord>, Problem> {
    let mut records = vec![];
    for item in &command.puts {
        let key = key(&item.tenant_id, &item.record_type, &item.record_id)?;
        let current = load_record(tx, &key)?;
        let actual = current.as_ref().map_or(0, |record| record.revision);
        check_revision(&command.expected_revisions, &key, actual)?;
        check_revision_option(item.expected_revision, actual)?;
        let record = make_record(&key, actual, &item.value)?;
        let affected = if actual == 0 {
            tx.execute("INSERT INTO buglogin_control_records (tenant_id, record_type, record_id, revision, checksum, value) VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT (tenant_id, record_type, record_id) DO NOTHING", &[&record.tenant_id, &record.record_type, &record.record_id, &(record.revision as i64), &record.checksum, &record.value]).map_err(|_| unavailable())?
        } else {
            tx.execute("UPDATE buglogin_control_records SET revision = $4, checksum = $5, value = $6 WHERE tenant_id = $1 AND record_type = $2 AND record_id = $3 AND revision = $7", &[&record.tenant_id, &record.record_type, &record.record_id, &(record.revision as i64), &record.checksum, &record.value, &(actual as i64)]).map_err(|_| unavailable())?
        };
        require_single_record_write(tx, &key, actual, affected)?;
        records.push(record);
    }
    for item in &command.deletes {
        let key = key(&item.tenant_id, &item.record_type, &item.record_id)?;
        let actual = load_record(tx, &key)?
            .as_ref()
            .map_or(0, |record| record.revision);
        check_revision(&command.expected_revisions, &key, actual)?;
        check_revision_option(item.expected_revision, actual)?;
        tx.execute("DELETE FROM buglogin_control_records WHERE tenant_id = $1 AND record_type = $2 AND record_id = $3 AND revision = $4", &[&key.tenant_id, &key.record_type, &key.record_id, &(actual as i64)]).map_err(|_| unavailable())?;
    }
    Ok(records)
}
fn write_postgres_complete(
    tx: &mut Transaction<'_>,
    command: &CompleteControlCommand,
) -> Result<Vec<ControlRecord>, Problem> {
    write_postgres_command(
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
fn require_single_record_write(
    tx: &mut Transaction<'_>,
    key: &ControlRecordKey,
    expected_revision: u64,
    affected: u64,
) -> Result<(), Problem> {
    match affected {
        1 => Ok(()),
        0 => {
            let actual = load_record(tx, key)?
                .as_ref()
                .map_or(0, |record| record.revision);
            Err(revision_conflict(Some(expected_revision), actual))
        }
        _ => Err(corrupt_record()),
    }
}
fn apply_command(
    state: &mut MemoryState,
    command: &ExecuteControlCommand,
) -> Result<Vec<ControlRecord>, Problem> {
    let mut records = vec![];
    for item in &command.puts {
        let key = key(&item.tenant_id, &item.record_type, &item.record_id)?;
        let actual = state.records.get(&key).map_or(0, |record| record.revision);
        check_revision(&command.expected_revisions, &key, actual)?;
        check_revision_option(item.expected_revision, actual)?;
        let record = make_record(&key, actual, &item.value)?;
        state.records.insert(key, record.clone());
        records.push(record);
    }
    for item in &command.deletes {
        let key = key(&item.tenant_id, &item.record_type, &item.record_id)?;
        let actual = state.records.get(&key).map_or(0, |record| record.revision);
        check_revision(&command.expected_revisions, &key, actual)?;
        check_revision_option(item.expected_revision, actual)?;
        state.records.remove(&key);
    }
    Ok(records)
}
fn apply_complete_command(
    state: &mut MemoryState,
    command: &CompleteControlCommand,
) -> Result<Vec<ControlRecord>, Problem> {
    apply_command(
        state,
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
fn make_record(
    key: &ControlRecordKey,
    actual: u64,
    value: &Value,
) -> Result<ControlRecord, Problem> {
    let serialized = serde_json::to_vec(value).map_err(|_| corrupt_record())?;
    Ok(ControlRecord {
        tenant_id: key.tenant_id.clone(),
        record_type: key.record_type.clone(),
        record_id: key.record_id.clone(),
        revision: actual.checked_add(1).ok_or_else(corrupt_record)?,
        checksum: checksum(&serialized),
        value: value.clone(),
    })
}
fn load_record(
    client: &mut impl GenericClient,
    key: &ControlRecordKey,
) -> Result<Option<ControlRecord>, Problem> {
    client.query_opt("SELECT revision, checksum, value FROM buglogin_control_records WHERE tenant_id = $1 AND record_type = $2 AND record_id = $3", &[&key.tenant_id, &key.record_type, &key.record_id]).map_err(|_| unavailable())?.map(|row| decode_record(&key.tenant_id, &key.record_type, &key.record_id, row.get(0), row.get(1), row.get(2))).transpose()
}
fn decode_record(
    tenant_id: &str,
    record_type: &str,
    record_id: &str,
    revision: i64,
    stored_checksum: String,
    value: Value,
) -> Result<ControlRecord, Problem> {
    let serialized = serde_json::to_vec(&value).map_err(|_| corrupt_record())?;
    if revision <= 0 || checksum(&serialized) != stored_checksum {
        return Err(corrupt_record());
    }
    Ok(ControlRecord {
        tenant_id: tenant_id.into(),
        record_type: record_type.into(),
        record_id: record_id.into(),
        revision: revision as u64,
        checksum: stored_checksum,
        value,
    })
}
fn command_checksum(command: &ExecuteControlCommand) -> Result<String, Problem> {
    let expected_revisions: Vec<_> = command
        .expected_revisions
        .iter()
        .map(|(key, revision)| (key, revision))
        .collect();
    serde_json::to_vec(&(
        &command.tenant_id,
        &command.command_id,
        &command.idempotency_key,
        expected_revisions,
        &command.puts,
        &command.deletes,
        &command.audit,
        &command.response,
    ))
    .map(|bytes| checksum(&bytes))
    .map_err(|_| corrupt_record())
}
fn complete_checksum(command: &CompleteControlCommand) -> Result<String, Problem> {
    let expected_revisions: Vec<_> = command.expected_revisions.iter().collect();
    serde_json::to_vec(&(
        &command.claim.tenant_id,
        &command.claim.idempotency_key,
        &command.claim.request_checksum,
        &command.command_id,
        expected_revisions,
        &command.puts,
        &command.deletes,
        command.apply_mutations,
        &command.audit,
        &command.response,
    ))
    .map(|bytes| checksum(&bytes))
    .map_err(|_| corrupt_record())
}
fn decode_stored_response(value: Value) -> Result<StoredIdempotencyResponse, Problem> {
    serde_json::from_value::<StoredIdempotencyResponse>(value.clone())
        .or_else(|_| {
            serde_json::from_value::<ControlCommandResult>(value).map(|result| {
                StoredIdempotencyResponse::Complete {
                    completion_checksum: String::new(),
                    result,
                }
            })
        })
        .map_err(|_| corrupt_record())
}
fn decode_completed_response(value: Value) -> Result<Option<ControlCommandResult>, Problem> {
    Ok(match decode_stored_response(value)? {
        StoredIdempotencyResponse::Pending { .. } => None,
        StoredIdempotencyResponse::Complete { result, .. } => Some(result),
    })
}
fn validate_claim(command: &ClaimControlCommand) -> Result<(), Problem> {
    validate_scope(&command.tenant_id, &command.idempotency_key, "idempotency")?;
    validate_sha256(&command.request_checksum)?;
    validate_key(
        &command.claim_token,
        "invalid_claim_token",
        "Claim token is invalid",
    )?;
    validate_key(
        &command.operation_id,
        "invalid_operation_id",
        "Operation ID is invalid",
    )
}
fn validate_complete(command: &CompleteControlCommand) -> Result<(), Problem> {
    validate_scope(&command.claim.tenant_id, "command", &command.command_id)?;
    validate_scope(
        &command.claim.tenant_id,
        &command.claim.idempotency_key,
        "idempotency",
    )?;
    validate_sha256(&command.claim.request_checksum)?;
    validate_key(
        &command.claim.claim_token,
        "invalid_claim_token",
        "Claim token is invalid",
    )?;
    validate_key(
        &command.claim.operation_id,
        "invalid_operation_id",
        "Operation ID is invalid",
    )?;
    if command.audit.tenant_id != command.claim.tenant_id
        || command.audit.command_id != command.command_id
    {
        return Err(problem(
            "invalid_control_audit",
            "Audit scope does not match command",
            400,
        ));
    }
    for key in command.expected_revisions.keys() {
        validate_scope(&key.tenant_id, &key.record_type, &key.record_id)?;
    }
    for put in &command.puts {
        validate_scope(&put.tenant_id, &put.record_type, &put.record_id)?;
    }
    for delete in &command.deletes {
        validate_scope(&delete.tenant_id, &delete.record_type, &delete.record_id)?;
    }
    Ok(())
}
fn validate_sha256(value: &str) -> Result<(), Problem> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(problem(
            "invalid_request_checksum",
            "Request checksum is invalid",
            400,
        ));
    }
    Ok(())
}
fn validate_command(command: &ExecuteControlCommand) -> Result<(), Problem> {
    validate_scope(&command.tenant_id, "command", &command.command_id)?;
    validate_key(
        &command.idempotency_key,
        "invalid_idempotency_key",
        "Idempotency key is invalid",
    )?;
    if command.audit.tenant_id != command.tenant_id
        || command.audit.command_id != command.command_id
    {
        return Err(problem(
            "invalid_control_audit",
            "Audit scope does not match command",
            400,
        ));
    }
    for key in command.expected_revisions.keys() {
        validate_scope(&key.tenant_id, &key.record_type, &key.record_id)?;
    }
    for put in &command.puts {
        validate_scope(&put.tenant_id, &put.record_type, &put.record_id)?;
    }
    for delete in &command.deletes {
        validate_scope(&delete.tenant_id, &delete.record_type, &delete.record_id)?;
    }
    Ok(())
}
fn key(tenant_id: &str, record_type: &str, record_id: &str) -> Result<ControlRecordKey, Problem> {
    validate_scope(tenant_id, record_type, record_id)?;
    Ok(ControlRecordKey {
        tenant_id: tenant_id.into(),
        record_type: record_type.into(),
        record_id: record_id.into(),
    })
}
fn validate_scope(tenant_id: &str, record_type: &str, record_id: &str) -> Result<(), Problem> {
    if [tenant_id, record_type, record_id].iter().any(|value| {
        value.is_empty()
            || value.len() > 128
            || !value.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-')
            })
    }) {
        return Err(problem(
            "invalid_control_record_key",
            "Control record key is invalid",
            400,
        ));
    }
    Ok(())
}
fn validate_key(value: &str, code: &str, title: &str) -> Result<(), Problem> {
    if value.len() < 16
        || value.len() > 128
        || !value.is_ascii()
        || value.bytes().any(|byte| byte.is_ascii_whitespace())
    {
        return Err(problem(code, title, 400));
    }
    Ok(())
}
fn check_revision(
    expected: &BTreeMap<ControlRecordKey, u64>,
    key: &ControlRecordKey,
    actual: u64,
) -> Result<(), Problem> {
    if expected.get(key).copied().unwrap_or(actual) != actual {
        return Err(revision_conflict(expected.get(key).copied(), actual));
    }
    Ok(())
}
fn check_revision_option(expected: Option<u64>, actual: u64) -> Result<(), Problem> {
    if expected.unwrap_or(actual) != actual {
        return Err(revision_conflict(expected, actual));
    }
    Ok(())
}
fn revision_conflict(expected: Option<u64>, actual: u64) -> Problem {
    problem(
        "storage_revision_conflict",
        "Storage revision conflict",
        409,
    )
    .with_field("expectedRevision", expected)
    .with_field("actualRevision", actual)
}
fn idempotency_conflict() -> Problem {
    problem(
        "idempotency_conflict",
        "Idempotency key conflicts with an existing request",
        409,
    )
}
fn idempotency_in_progress() -> Problem {
    problem(
        "idempotency_in_progress",
        "Idempotent request is already running",
        409,
    )
    .retryable(true)
}
fn idempotency_claim_missing() -> Problem {
    problem(
        "idempotency_claim_missing",
        "Idempotency claim is missing",
        409,
    )
}
fn idempotency_claim_conflict() -> Problem {
    problem(
        "idempotency_claim_conflict",
        "Idempotency claim is owned by another operation",
        409,
    )
}
fn unavailable() -> Problem {
    problem(
        "control_storage_unavailable",
        "Control storage is unavailable",
        503,
    )
    .retryable(true)
}
fn corrupt_record() -> Problem {
    problem(
        "control_record_checksum_mismatch",
        "Control record checksum mismatch",
        500,
    )
}
fn problem(code: &str, title: &str, status: u16) -> Problem {
    Problem::new(code, title, status, CORRELATION)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn command(key: &str, allowed: bool) -> ExecuteControlCommand {
        ExecuteControlCommand {
            tenant_id: "tenant".into(),
            command_id: "record.write".into(),
            idempotency_key: key.into(),
            expected_revisions: BTreeMap::new(),
            puts: vec![ControlPut {
                tenant_id: "tenant".into(),
                record_type: "profile".into(),
                record_id: "one".into(),
                expected_revision: None,
                value: serde_json::json!({"v": 1}),
            }],
            deletes: vec![],
            audit: ControlAuditEntry {
                tenant_id: "tenant".into(),
                command_id: "record.write".into(),
                actor_id: "actor".into(),
                allowed,
                detail: Value::Null,
            },
            response: HttpResponse {
                status: if allowed { 200 } else { 403 },
                headers: BTreeMap::new(),
                body: Value::Null,
            },
        }
    }
    #[test]
    fn command_is_atomic_and_replays_after_reopen() {
        let mut store = MemoryControlRecordStore::default();
        let first = store.execute(command("request-key-000001", true)).unwrap();
        assert_eq!(first.records[0].revision, 1);
        let mut reopened = store.reopen();
        let replay = reopened
            .execute(command("request-key-000001", true))
            .unwrap();
        assert!(replay.replayed);
        assert_eq!(reopened.list("tenant", "profile").unwrap().len(), 1);
    }
    #[test]
    fn conflict_rolls_back_all_mutations() {
        let mut store = MemoryControlRecordStore::default();
        let mut seed = command("request-key-000002-seed", true);
        seed.puts[0].record_id = "two".into();
        seed.puts[0].expected_revision = Some(0);
        store.execute(seed).unwrap();

        let mut conflicting = command("request-key-000002", true);
        conflicting.puts[0].expected_revision = Some(0);
        conflicting.puts.push(ControlPut {
            tenant_id: "tenant".into(),
            record_type: "profile".into(),
            record_id: "two".into(),
            expected_revision: Some(0),
            value: serde_json::json!({"v": 2}),
        });
        assert_eq!(
            store.execute(conflicting).unwrap_err().code,
            "storage_revision_conflict"
        );
        assert!(store.get("tenant", "profile", "one").unwrap().is_none());
        let unchanged = store.get("tenant", "profile", "two").unwrap().unwrap();
        assert_eq!(unchanged.revision, 1);
        assert_eq!(unchanged.value, serde_json::json!({"v": 1}));
    }
    #[test]
    fn revision_map_participates_in_checksum() {
        let mut store = MemoryControlRecordStore::default();
        let first = store.execute(command("request-key-000004", true)).unwrap();
        let mut retry = command("request-key-000004", true);
        retry.expected_revisions.insert(
            ControlRecordKey {
                tenant_id: "tenant".into(),
                record_type: "profile".into(),
                record_id: "one".into(),
            },
            first.records[0].revision,
        );
        assert_eq!(
            store.execute(retry).unwrap_err().code,
            "idempotency_conflict"
        );
    }

    #[test]
    fn denied_command_writes_audit_only() {
        let mut store = MemoryControlRecordStore::default();
        let result = store.execute(command("request-key-000003", false)).unwrap();
        assert_eq!(result.response.status, 403);
        assert!(store.list("tenant", "profile").unwrap().is_empty());
        assert_eq!(store.audit("tenant").unwrap().len(), 1);
    }

    #[test]
    fn idempotency_claim_has_one_winner_and_replays_after_reopen() {
        use std::sync::{Arc, Barrier};
        use std::thread;

        let store = MemoryControlRecordStore::default();
        let barrier = Arc::new(Barrier::new(32));
        let mut workers = Vec::new();
        for index in 0..32 {
            let store = store.clone();
            let barrier = barrier.clone();
            workers.push(thread::spawn(move || {
                barrier.wait();
                let mut store = store;
                store
                    .claim_idempotency(ClaimControlCommand {
                        tenant_id: "tenant".into(),
                        idempotency_key: "mcp-key-000000000001".into(),
                        request_checksum:
                            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                                .into(),
                        claim_token: format!("claim-token-{index:016}"),
                        operation_id: format!("operation-{index:016}"),
                    })
                    .unwrap()
            }));
        }
        let claims: Vec<_> = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect();
        let winner = claims
            .iter()
            .find_map(|claim| match claim {
                ControlIdempotencyClaim::Claimed { claim } => Some(claim.clone()),
                _ => None,
            })
            .expect("one claim winner");
        assert_eq!(
            claims
                .iter()
                .filter(|claim| matches!(claim, ControlIdempotencyClaim::Claimed { .. }))
                .count(),
            1
        );
        assert_eq!(
            claims
                .iter()
                .filter(|claim| matches!(claim, ControlIdempotencyClaim::Pending { .. }))
                .count(),
            31
        );

        let result = store
            .clone()
            .complete_idempotency(CompleteControlCommand {
                claim: winner,
                command_id: "record.write".into(),
                expected_revisions: BTreeMap::new(),
                puts: vec![ControlPut {
                    tenant_id: "tenant".into(),
                    record_type: "profile".into(),
                    record_id: "one".into(),
                    expected_revision: Some(0),
                    value: serde_json::json!({"v": 2}),
                }],
                deletes: vec![],
                apply_mutations: true,
                audit: ControlAuditEntry {
                    tenant_id: "tenant".into(),
                    command_id: "record.write".into(),
                    actor_id: "actor".into(),
                    allowed: true,
                    detail: serde_json::json!({"outcome": "success"}),
                },
                response: HttpResponse {
                    status: 200,
                    headers: BTreeMap::new(),
                    body: serde_json::json!({"ok": true}),
                },
            })
            .unwrap();
        assert!(!result.replayed);

        let mut reopened = store.reopen();
        let replay = reopened
            .claim_idempotency(ClaimControlCommand {
                tenant_id: "tenant".into(),
                idempotency_key: "mcp-key-000000000001".into(),
                request_checksum:
                    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
                claim_token: "claim-token-retry-000000".into(),
                operation_id: "operation-retry-000000".into(),
            })
            .unwrap();
        match replay {
            ControlIdempotencyClaim::Replay { result } => {
                assert!(result.replayed);
                assert_eq!(result.response.status, 200);
            }
            other => panic!("expected replay, got {other:?}"),
        }
        assert_eq!(reopened.audit("tenant").unwrap().len(), 1);
        assert_eq!(reopened.list("tenant", "profile").unwrap().len(), 1);
    }

    #[test]
    fn pending_claim_rejects_different_input_and_stays_fail_closed() {
        let mut store = MemoryControlRecordStore::default();
        let claim = ClaimControlCommand {
            tenant_id: "tenant".into(),
            idempotency_key: "mcp-key-000000000002".into(),
            request_checksum: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                .into(),
            claim_token: "claim-token-pending-000".into(),
            operation_id: "operation-pending-000".into(),
        };
        assert!(matches!(
            store.claim_idempotency(claim.clone()).unwrap(),
            ControlIdempotencyClaim::Claimed { .. }
        ));
        assert_eq!(
            store
                .claim_idempotency(ClaimControlCommand {
                    request_checksum:
                        "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc".into(),
                    ..claim
                })
                .unwrap_err()
                .code,
            "idempotency_conflict"
        );
        assert!(
            store
                .replay("tenant", "mcp-key-000000000002")
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn config_requires_tls_and_redacts_secret() {
        assert!(PostgresConfig::new("postgres://user:secret@db/app?sslmode=disable").is_err());
        let config =
            PostgresConfig::new("postgres://user:secret@db/app?sslmode=verify-full").unwrap();
        let debug = format!("{config:?}");
        assert!(!debug.contains("secret"));
        assert!(debug.contains("<redacted>"));
    }
}
