use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
    mpsc,
};
use std::thread;

use buglogin_domain::errors::Problem;
use native_tls::{Certificate, TlsConnector};
use postgres::{
    Client, Config as PgConfig, GenericClient, Transaction,
    config::{Host, SslMode},
};
use postgres_native_tls::MakeTlsConnector;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::blob::checksum;

mod sqlite;

pub use sqlite::SqliteControlRecordStore;

const CORRELATION: &str = "storage";
const PG_WORKER_QUEUE_CAPACITY: usize = 64;

#[derive(Clone)]
pub struct PostgresConfig {
    config: PgConfig,
    tls_root_certificate: Option<PathBuf>,
}
impl PostgresConfig {
    pub fn new(database_url: impl Into<String>) -> Result<Self, Problem> {
        Self::new_with_tls_root_certificate(database_url, None)
    }
    pub fn new_with_tls_root_certificate(
        database_url: impl Into<String>,
        tls_root_certificate: Option<PathBuf>,
    ) -> Result<Self, Problem> {
        let database_url = database_url.into();
        if database_url.trim().is_empty() {
            return Err(problem(
                "invalid_postgres_config",
                "PostgreSQL configuration is required",
                400,
            ));
        }
        let config = PgConfig::from_str(&database_url)
            .map_err(|_| problem("invalid_postgres_config", "PostgreSQL URL is invalid", 400))?;
        if config.get_ssl_mode() != SslMode::Require {
            return Err(problem(
                "invalid_postgres_config",
                "PostgreSQL TLS is required",
                400,
            ));
        }
        Ok(Self {
            config,
            tls_root_certificate,
        })
    }
    pub fn from_env() -> Result<Self, Problem> {
        Self::new_with_tls_root_certificate(
            env::var("BUGLOGIN_POSTGRES_URL").map_err(|_| {
                problem(
                    "missing_postgres_config",
                    "PostgreSQL configuration is missing",
                    500,
                )
            })?,
            env::var_os("BUGLOGIN_POSTGRES_TLS_ROOT_CERT").map(PathBuf::from),
        )
    }

    /// Creates a PostgreSQL config for an explicitly local development database.
    /// TLS is mandatory; the URL is accepted only for one explicit loopback TCP host.
    pub fn new_local_development(
        database_url: impl Into<String>,
        tls_root_certificate: Option<PathBuf>,
    ) -> Result<Self, Problem> {
        let database_url = database_url.into();
        if database_url.trim().is_empty() {
            return Err(problem(
                "invalid_postgres_config",
                "PostgreSQL configuration is required",
                400,
            ));
        }
        let config = PgConfig::from_str(&database_url)
            .map_err(|_| problem("invalid_postgres_config", "PostgreSQL URL is invalid", 400))?;
        let local_tcp_host = matches!(
            config.get_hosts(),
            [Host::Tcp(host)] if matches!(host.as_str(), "localhost" | "127.0.0.1" | "::1")
        );
        if !local_tcp_host
            || !config.get_hostaddrs().is_empty()
            || config.get_ssl_mode() != SslMode::Require
        {
            return Err(problem(
                "invalid_local_postgres_config",
                "Local PostgreSQL must use one explicit loopback host and sslmode=require",
                400,
            ));
        }
        Ok(Self {
            config,
            tls_root_certificate,
        })
    }
}
impl fmt::Debug for PostgresConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PostgresConfig")
            .field("database_url", &"<redacted>")
            .field(
                "tls_root_certificate_configured",
                &self.tls_root_certificate.is_some(),
            )
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
    /// When present, deletion succeeds only if this exact revision still exists.
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
    /// Transactional revision preconditions. Keys may be read-only and need
    /// not also appear in `puts` or `deletes`.
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
    /// Transactional revision preconditions. Keys may be read-only and need
    /// not also appear in `puts` or `deletes`.
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
    fn readiness(&mut self) -> Result<(), Problem>;

    fn get(
        &mut self,
        tenant_id: &str,
        record_type: &str,
        record_id: &str,
    ) -> Result<Option<ControlRecord>, Problem>;
    fn get_many(
        &mut self,
        tenant_id: &str,
        record_type: &str,
        record_ids: &[String],
    ) -> Result<Vec<ControlRecord>, Problem> {
        validate_scope(tenant_id, record_type, "list")?;
        let mut records = Vec::with_capacity(record_ids.len());
        for record_id in record_ids {
            if let Some(record) = self.get(tenant_id, record_type, record_id)? {
                records.push(record);
            }
        }
        Ok(records)
    }
    fn revision(
        &mut self,
        tenant_id: &str,
        record_type: &str,
        record_id: &str,
    ) -> Result<Option<u64>, Problem> {
        Ok(self
            .get(tenant_id, record_type, record_id)?
            .map(|record| record.revision))
    }
    fn list(&mut self, tenant_id: &str, record_type: &str) -> Result<Vec<ControlRecord>, Problem>;
    fn list_for_tenants(
        &mut self,
        tenant_ids: &[String],
        record_type: &str,
    ) -> Result<Vec<ControlRecord>, Problem> {
        let mut records = Vec::new();
        for tenant_id in tenant_ids {
            records.extend(self.list(tenant_id, record_type)?);
        }
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
    ) -> Result<Option<ControlCommandResult>, Problem>;
    fn claim_idempotency(
        &mut self,
        command: ClaimControlCommand,
    ) -> Result<ControlIdempotencyClaim, Problem>;
    fn abandon_idempotency(&mut self, claim: &ControlIdempotencyClaimToken) -> Result<(), Problem>;
    fn complete_idempotency(
        &mut self,
        command: CompleteControlCommand,
    ) -> Result<ControlCommandResult, Problem>;
    fn execute(&mut self, command: ExecuteControlCommand) -> Result<ControlCommandResult, Problem>;
    fn audit(&mut self, tenant_id: &str) -> Result<Vec<ControlAuditEntry>, Problem>;
    /// Acquire a non-reentrant exclusive lock for this store's lock namespace.
    /// Re-acquiring a lock ID already held through this store returns `false`.
    fn try_advisory_lock(&mut self, lock_id: i64) -> Result<bool, Problem>;
    /// Release a lock previously acquired through this store.
    fn release_advisory_lock(&mut self, lock_id: i64) -> Result<(), Problem>;
    /// Acquire a shared lock for concurrent object writers. Implementations
    /// without shared-lock support fall back to the safe exclusive behavior.
    fn try_shared_advisory_lock(&mut self, lock_id: i64) -> Result<bool, Problem> {
        self.try_advisory_lock(lock_id)
    }
    /// Release one shared lock previously acquired through this store.
    fn release_shared_advisory_lock(&mut self, lock_id: i64) -> Result<(), Problem> {
        self.release_advisory_lock(lock_id)
    }
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
    advisory_locks: BTreeSet<i64>,
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
    fn readiness(&mut self) -> Result<(), Problem> {
        Ok(())
    }

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
    fn list_for_tenants(
        &mut self,
        tenant_ids: &[String],
        record_type: &str,
    ) -> Result<Vec<ControlRecord>, Problem> {
        for tenant_id in tenant_ids {
            validate_scope(tenant_id, record_type, "list")?;
        }
        let tenants = tenant_ids
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        Ok(self
            .state
            .lock()
            .map_err(|_| unavailable())?
            .records
            .values()
            .filter(|record| {
                tenants.contains(record.tenant_id.as_str()) && record.record_type == record_type
            })
            .cloned()
            .collect())
    }
    fn list_for_account(
        &mut self,
        tenant_id: &str,
        record_type: &str,
        account_id: &str,
    ) -> Result<Vec<ControlRecord>, Problem> {
        validate_scope(tenant_id, record_type, account_id)?;
        Ok(self
            .state
            .lock()
            .map_err(|_| unavailable())?
            .records
            .values()
            .filter(|record| {
                record.tenant_id == tenant_id
                    && record.record_type == record_type
                    && record.value.get("accountId").and_then(Value::as_str) == Some(account_id)
            })
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
    fn abandon_idempotency(&mut self, claim: &ControlIdempotencyClaimToken) -> Result<(), Problem> {
        validate_scope(&claim.tenant_id, &claim.idempotency_key, "idempotency")?;
        let mut state = self.state.lock().map_err(|_| unavailable())?;
        let key = (claim.tenant_id.clone(), claim.idempotency_key.clone());
        let Some((stored_checksum, stored)) = state.idempotency.get(&key) else {
            return Err(idempotency_claim_missing());
        };
        if stored_checksum != &claim.request_checksum {
            return Err(idempotency_conflict());
        }
        match stored {
            StoredIdempotencyResponse::Pending { claim_token, .. }
                if claim_token == &claim.claim_token => {}
            StoredIdempotencyResponse::Pending { .. } => {
                return Err(idempotency_claim_conflict());
            }
            StoredIdempotencyResponse::Complete { .. } => {
                return Err(idempotency_conflict());
            }
        }
        state.idempotency.remove(&key);
        Ok(())
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

    fn try_advisory_lock(&mut self, lock_id: i64) -> Result<bool, Problem> {
        Ok(self
            .state
            .lock()
            .map_err(|_| unavailable())?
            .advisory_locks
            .insert(lock_id))
    }

    fn release_advisory_lock(&mut self, lock_id: i64) -> Result<(), Problem> {
        self.state
            .lock()
            .map_err(|_| unavailable())?
            .advisory_locks
            .remove(&lock_id);
        Ok(())
    }
}

pub struct PostgresControlRecordStore {
    worker: mpsc::SyncSender<PgOperation>,
    queued: Arc<AtomicUsize>,
}

type PgOperation = Box<dyn FnOnce(&mut PostgresControlRecordWorker) + Send + 'static>;

struct PostgresControlRecordWorker {
    client: Client,
    config: PostgresConfig,
    advisory_locks: BTreeMap<i64, Client>,
    shared_advisory_locks: BTreeMap<i64, Vec<Client>>,
}

impl PostgresControlRecordStore {
    pub fn connect(config: &PostgresConfig) -> Result<Self, Problem> {
        let config = config.clone();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let (worker_tx, worker_rx) = mpsc::sync_channel::<PgOperation>(PG_WORKER_QUEUE_CAPACITY);
        let queued = Arc::new(AtomicUsize::new(0));
        thread::Builder::new()
            .name("buglogin-postgres".into())
            .spawn(
                move || match PostgresControlRecordWorker::connect(&config) {
                    Ok(mut worker) => {
                        if ready_tx.send(Ok(())).is_ok() {
                            while let Ok(operation) = worker_rx.recv() {
                                operation(&mut worker);
                            }
                        }
                    }
                    Err(error) => {
                        let _ = ready_tx.send(Err(error));
                    }
                },
            )
            .map_err(|_| unavailable())?;
        ready_rx.recv().map_err(|_| unavailable())??;
        Ok(Self {
            worker: worker_tx,
            queued,
        })
    }

    fn call<T, F>(&self, operation: F) -> Result<T, Problem>
    where
        T: Send + 'static,
        F: FnOnce(&mut PostgresControlRecordWorker) -> Result<T, Problem> + Send + 'static,
    {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        let queued = Arc::clone(&self.queued);
        let enqueued_at = std::time::Instant::now();
        queued.fetch_add(1, Ordering::Relaxed);
        if let Err(error) = self.worker.try_send(Box::new(move |worker| {
            let pending = queued.fetch_sub(1, Ordering::Relaxed).saturating_sub(1);
            let queue_wait = enqueued_at.elapsed();
            let started_at = std::time::Instant::now();
            let result = operation(worker);
            let service_time = started_at.elapsed();
            if queue_wait >= std::time::Duration::from_millis(100)
                || service_time >= std::time::Duration::from_millis(500)
                || pending >= PG_WORKER_QUEUE_CAPACITY * 3 / 4
            {
                eprintln!(
                    "control_pg_worker queue_wait_ms={} service_ms={} pending={pending}",
                    queue_wait.as_millis(),
                    service_time.as_millis()
                );
            }
            let _ = reply_tx.send(result);
        })) {
            self.queued.fetch_sub(1, Ordering::Relaxed);
            return Err(match error {
                mpsc::TrySendError::Full(_) => {
                    eprintln!("control_pg_worker queue_full capacity={PG_WORKER_QUEUE_CAPACITY}");
                    overloaded()
                }
                mpsc::TrySendError::Disconnected(_) => unavailable(),
            });
        }
        let reply_wait_started = std::time::Instant::now();
        let result = reply_rx.recv().map_err(|_| unavailable())?;
        let reply_wait = reply_wait_started.elapsed();
        if reply_wait >= std::time::Duration::from_millis(250) {
            eprintln!(
                "control_pg_worker reply_wait_ms={} pending={}",
                reply_wait.as_millis(),
                self.queued.load(Ordering::Relaxed)
            );
        }
        result
    }

    pub fn ensure_schema(&mut self) -> Result<(), Problem> {
        self.call(PostgresControlRecordWorker::ensure_schema)
    }
}

impl PostgresControlRecordWorker {
    fn connect(config: &PostgresConfig) -> Result<Self, Problem> {
        let client = Self::connect_client(config)?;
        Ok(Self {
            client,
            config: config.clone(),
            advisory_locks: BTreeMap::new(),
            shared_advisory_locks: BTreeMap::new(),
        })
    }

    fn connect_client(config: &PostgresConfig) -> Result<Client, Problem> {
        let mut connector_builder = TlsConnector::builder();
        if let Some(path) = &config.tls_root_certificate {
            let pem = std::fs::read(path).map_err(|_| unavailable())?;
            let certificate = Certificate::from_pem(&pem).map_err(|_| unavailable())?;
            connector_builder.add_root_certificate(certificate);
        }
        let connector = connector_builder.build().map_err(|_| unavailable())?;
        let client = config
            .config
            .connect(MakeTlsConnector::new(connector))
            .map_err(|_| unavailable())?;
        Ok(client)
    }
    fn ensure_schema(&mut self) -> Result<(), Problem> {
        self.client.batch_execute("CREATE TABLE IF NOT EXISTS buglogin_control_records (tenant_id TEXT NOT NULL, record_type TEXT NOT NULL, record_id TEXT NOT NULL, revision BIGINT NOT NULL CHECK (revision > 0), checksum CHAR(64) NOT NULL, value JSONB NOT NULL, PRIMARY KEY (tenant_id, record_type, record_id)); CREATE INDEX IF NOT EXISTS buglogin_control_records_account_idx ON buglogin_control_records (tenant_id, record_type, (value->>'accountId'), record_id); CREATE TABLE IF NOT EXISTS buglogin_control_idempotency (tenant_id TEXT NOT NULL, idempotency_key TEXT NOT NULL, request_checksum CHAR(64) NOT NULL, response JSONB NOT NULL, created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(), PRIMARY KEY (tenant_id, idempotency_key)); CREATE TABLE IF NOT EXISTS buglogin_control_audit (sequence BIGSERIAL PRIMARY KEY, tenant_id TEXT NOT NULL, command_id TEXT NOT NULL, actor_id TEXT NOT NULL, allowed BOOLEAN NOT NULL, detail JSONB NOT NULL, created_at TIMESTAMPTZ NOT NULL DEFAULT NOW());").map_err(|_| unavailable())
    }
}
impl ControlRecordStore for PostgresControlRecordWorker {
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
    fn get_many(
        &mut self,
        tenant_id: &str,
        record_type: &str,
        record_ids: &[String],
    ) -> Result<Vec<ControlRecord>, Problem> {
        validate_scope(tenant_id, record_type, "list")?;
        if record_ids.is_empty() {
            return Ok(Vec::new());
        }
        for record_id in record_ids {
            key(tenant_id, record_type, record_id)?;
        }
        self.client
            .query(
                "SELECT record_id, revision, checksum, value FROM buglogin_control_records WHERE tenant_id = $1 AND record_type = $2 AND record_id = ANY($3) ORDER BY record_id",
                &[&tenant_id, &record_type, &record_ids],
            )
            .map_err(|_| unavailable())?
            .into_iter()
            .map(|row| {
                decode_record(
                    tenant_id,
                    record_type,
                    row.get(0),
                    row.get(1),
                    row.get(2),
                    row.get(3),
                )
            })
            .collect()
    }
    fn revision(
        &mut self,
        tenant_id: &str,
        record_type: &str,
        record_id: &str,
    ) -> Result<Option<u64>, Problem> {
        key(tenant_id, record_type, record_id)?;
        self.client
            .query_opt(
                "SELECT revision FROM buglogin_control_records WHERE tenant_id = $1 AND record_type = $2 AND record_id = $3",
                &[&tenant_id, &record_type, &record_id],
            )
            .map_err(|_| unavailable())?
            .map(|row| {
                let revision: i64 = row.get(0);
                u64::try_from(revision).map_err(|_| corrupt_record())
            })
            .transpose()
    }
    fn list(&mut self, tenant_id: &str, record_type: &str) -> Result<Vec<ControlRecord>, Problem> {
        validate_scope(tenant_id, record_type, "list")?;
        self.client.query("SELECT record_id, revision, checksum, value FROM buglogin_control_records WHERE tenant_id = $1 AND record_type = $2 ORDER BY record_id", &[&tenant_id, &record_type]).map_err(|_| unavailable())?.into_iter().map(|row| decode_record(tenant_id, record_type, row.get(0), row.get(1), row.get(2), row.get(3))).collect()
    }
    fn list_for_tenants(
        &mut self,
        tenant_ids: &[String],
        record_type: &str,
    ) -> Result<Vec<ControlRecord>, Problem> {
        for tenant_id in tenant_ids {
            validate_scope(tenant_id, record_type, "list")?;
        }
        if tenant_ids.is_empty() {
            return Ok(Vec::new());
        }
        let tenant_ids = tenant_ids.to_vec();
        self.client
            .query(
                "SELECT tenant_id, record_id, revision, checksum, value FROM buglogin_control_records WHERE tenant_id = ANY($1) AND record_type = $2 ORDER BY tenant_id, record_id",
                &[&tenant_ids, &record_type],
            )
            .map_err(|_| unavailable())?
            .into_iter()
            .map(|row| {
                let tenant_id: String = row.get(0);
                let record_id: String = row.get(1);
                decode_record(
                    &tenant_id,
                    record_type,
                    &record_id,
                    row.get(2),
                    row.get(3),
                    row.get(4),
                )
            })
            .collect()
    }
    fn list_for_account(
        &mut self,
        tenant_id: &str,
        record_type: &str,
        account_id: &str,
    ) -> Result<Vec<ControlRecord>, Problem> {
        validate_scope(tenant_id, record_type, account_id)?;
        self.client
            .query(
                "SELECT record_id, revision, checksum, value FROM buglogin_control_records WHERE tenant_id = $1 AND record_type = $2 AND value->>'accountId' = $3 ORDER BY record_id",
                &[&tenant_id, &record_type, &account_id],
            )
            .map_err(|_| unavailable())?
            .into_iter()
            .map(|row| {
                decode_record(
                    tenant_id,
                    record_type,
                    row.get(0),
                    row.get(1),
                    row.get(2),
                    row.get(3),
                )
            })
            .collect()
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
    fn abandon_idempotency(&mut self, claim: &ControlIdempotencyClaimToken) -> Result<(), Problem> {
        validate_scope(&claim.tenant_id, &claim.idempotency_key, "idempotency")?;
        let mut tx = self.client.transaction().map_err(|_| unavailable())?;
        let row = tx
            .query_opt(
                "SELECT request_checksum, response FROM buglogin_control_idempotency WHERE tenant_id = $1 AND idempotency_key = $2 FOR UPDATE",
                &[&claim.tenant_id, &claim.idempotency_key],
            )
            .map_err(|_| unavailable())?
            .ok_or_else(idempotency_claim_missing)?;
        let stored_checksum: String = row.get(0);
        if stored_checksum != claim.request_checksum {
            return Err(idempotency_conflict());
        }
        match decode_stored_response(row.get(1))? {
            StoredIdempotencyResponse::Pending { claim_token, .. }
                if claim_token == claim.claim_token => {}
            StoredIdempotencyResponse::Pending { .. } => {
                return Err(idempotency_claim_conflict());
            }
            StoredIdempotencyResponse::Complete { .. } => {
                return Err(idempotency_conflict());
            }
        }
        let deleted = tx.execute(
            "DELETE FROM buglogin_control_idempotency WHERE tenant_id = $1 AND idempotency_key = $2 AND request_checksum = $3",
            &[&claim.tenant_id, &claim.idempotency_key, &claim.request_checksum],
        )
        .map_err(|_| unavailable())?;
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

    fn try_advisory_lock(&mut self, lock_id: i64) -> Result<bool, Problem> {
        if self.advisory_locks.contains_key(&lock_id) {
            return Ok(false);
        }
        let mut lock_client = Self::connect_client(&self.config)?;
        let acquired = lock_client
            .query_one("SELECT pg_try_advisory_lock($1)", &[&lock_id])
            .map(|row| row.get(0))
            .map_err(|_| unavailable())?;
        if acquired {
            self.advisory_locks.insert(lock_id, lock_client);
        }
        Ok(acquired)
    }

    fn release_advisory_lock(&mut self, lock_id: i64) -> Result<(), Problem> {
        let result = self
            .advisory_locks
            .get_mut(&lock_id)
            .ok_or_else(unavailable)?
            .query_one("SELECT pg_advisory_unlock($1)", &[&lock_id])
            .map(|row| row.get::<_, bool>(0));
        match result {
            Ok(true) => {
                self.advisory_locks.remove(&lock_id);
                Ok(())
            }
            Ok(false) | Err(_) => {
                // The connection's lock state is now uncertain. Drop this
                // dedicated session so PostgreSQL releases any remaining
                // session lock, and don't retain a dead client in the worker.
                self.advisory_locks.remove(&lock_id);
                Err(unavailable())
            }
        }
    }

    fn try_shared_advisory_lock(&mut self, lock_id: i64) -> Result<bool, Problem> {
        let mut lock_client = Self::connect_client(&self.config)?;
        let acquired: bool = lock_client
            .query_one("SELECT pg_try_advisory_lock_shared($1)", &[&lock_id])
            .map(|row| row.get(0))
            .map_err(|_| unavailable())?;
        if acquired {
            self.shared_advisory_locks
                .entry(lock_id)
                .or_default()
                .push(lock_client);
        }
        Ok(acquired)
    }

    fn release_shared_advisory_lock(&mut self, lock_id: i64) -> Result<(), Problem> {
        let result = self
            .shared_advisory_locks
            .get_mut(&lock_id)
            .and_then(|clients| clients.last_mut())
            .ok_or_else(unavailable)?
            .query_one("SELECT pg_advisory_unlock_shared($1)", &[&lock_id])
            .map(|row| row.get::<_, bool>(0));
        match result {
            Ok(true) => {
                discard_latest_shared_lock_client(&mut self.shared_advisory_locks, lock_id);
                Ok(())
            }
            Ok(false) | Err(_) => {
                // A failed/ambiguous unlock must not leave a possibly broken
                // session cached. Dropping its dedicated connection makes
                // PostgreSQL release any lock still owned by that session.
                discard_latest_shared_lock_client(&mut self.shared_advisory_locks, lock_id);
                Err(unavailable())
            }
        }
    }
}

fn discard_latest_shared_lock_client<T>(locks: &mut BTreeMap<i64, Vec<T>>, lock_id: i64) {
    if let Some(clients) = locks.get_mut(&lock_id) {
        clients.pop();
        if clients.is_empty() {
            locks.remove(&lock_id);
        }
    }
}

impl ControlRecordStore for PostgresControlRecordStore {
    fn readiness(&mut self) -> Result<(), Problem> {
        self.call(|worker| worker.readiness())
    }

    fn get(
        &mut self,
        tenant_id: &str,
        record_type: &str,
        record_id: &str,
    ) -> Result<Option<ControlRecord>, Problem> {
        let (tenant_id, record_type, record_id) = (
            tenant_id.to_owned(),
            record_type.to_owned(),
            record_id.to_owned(),
        );
        self.call(move |worker| worker.get(&tenant_id, &record_type, &record_id))
    }

    fn get_many(
        &mut self,
        tenant_id: &str,
        record_type: &str,
        record_ids: &[String],
    ) -> Result<Vec<ControlRecord>, Problem> {
        let (tenant_id, record_type, record_ids) = (
            tenant_id.to_owned(),
            record_type.to_owned(),
            record_ids.to_vec(),
        );
        self.call(move |worker| worker.get_many(&tenant_id, &record_type, &record_ids))
    }

    fn revision(
        &mut self,
        tenant_id: &str,
        record_type: &str,
        record_id: &str,
    ) -> Result<Option<u64>, Problem> {
        let (tenant_id, record_type, record_id) = (
            tenant_id.to_owned(),
            record_type.to_owned(),
            record_id.to_owned(),
        );
        self.call(move |worker| worker.revision(&tenant_id, &record_type, &record_id))
    }

    fn list(&mut self, tenant_id: &str, record_type: &str) -> Result<Vec<ControlRecord>, Problem> {
        let (tenant_id, record_type) = (tenant_id.to_owned(), record_type.to_owned());
        self.call(move |worker| worker.list(&tenant_id, &record_type))
    }

    fn list_for_tenants(
        &mut self,
        tenant_ids: &[String],
        record_type: &str,
    ) -> Result<Vec<ControlRecord>, Problem> {
        let tenant_ids = tenant_ids.to_vec();
        let record_type = record_type.to_owned();
        self.call(move |worker| worker.list_for_tenants(&tenant_ids, &record_type))
    }

    fn list_for_account(
        &mut self,
        tenant_id: &str,
        record_type: &str,
        account_id: &str,
    ) -> Result<Vec<ControlRecord>, Problem> {
        let (tenant_id, record_type, account_id) = (
            tenant_id.to_owned(),
            record_type.to_owned(),
            account_id.to_owned(),
        );
        self.call(move |worker| worker.list_for_account(&tenant_id, &record_type, &account_id))
    }

    fn replay(
        &mut self,
        tenant_id: &str,
        idempotency_key: &str,
    ) -> Result<Option<ControlCommandResult>, Problem> {
        let (tenant_id, idempotency_key) = (tenant_id.to_owned(), idempotency_key.to_owned());
        self.call(move |worker| worker.replay(&tenant_id, &idempotency_key))
    }

    fn claim_idempotency(
        &mut self,
        command: ClaimControlCommand,
    ) -> Result<ControlIdempotencyClaim, Problem> {
        self.call(move |worker| worker.claim_idempotency(command))
    }

    fn complete_idempotency(
        &mut self,
        command: CompleteControlCommand,
    ) -> Result<ControlCommandResult, Problem> {
        self.call(move |worker| worker.complete_idempotency(command))
    }

    fn abandon_idempotency(&mut self, claim: &ControlIdempotencyClaimToken) -> Result<(), Problem> {
        let claim = claim.clone();
        self.call(move |worker| worker.abandon_idempotency(&claim))
    }

    fn try_advisory_lock(&mut self, lock_id: i64) -> Result<bool, Problem> {
        self.call(move |worker| worker.try_advisory_lock(lock_id))
    }

    fn release_advisory_lock(&mut self, lock_id: i64) -> Result<(), Problem> {
        self.call(move |worker| worker.release_advisory_lock(lock_id))
    }

    fn try_shared_advisory_lock(&mut self, lock_id: i64) -> Result<bool, Problem> {
        self.call(move |worker| worker.try_shared_advisory_lock(lock_id))
    }

    fn release_shared_advisory_lock(&mut self, lock_id: i64) -> Result<(), Problem> {
        self.call(move |worker| worker.release_shared_advisory_lock(lock_id))
    }

    fn execute(&mut self, command: ExecuteControlCommand) -> Result<ControlCommandResult, Problem> {
        self.call(move |worker| worker.execute(command))
    }

    fn audit(&mut self, tenant_id: &str) -> Result<Vec<ControlAuditEntry>, Problem> {
        let tenant_id = tenant_id.to_owned();
        self.call(move |worker| worker.audit(&tenant_id))
    }
}

fn write_postgres_command(
    tx: &mut Transaction<'_>,
    command: &ExecuteControlCommand,
) -> Result<Vec<ControlRecord>, Problem> {
    for (expected_key, expected_revision) in &command.expected_revisions {
        let stored_revision = tx
            .query_opt(
                "SELECT revision FROM buglogin_control_records WHERE tenant_id = $1 AND record_type = $2 AND record_id = $3 FOR UPDATE",
                &[&expected_key.tenant_id, &expected_key.record_type, &expected_key.record_id],
            )
            .map_err(|_| unavailable())?
            .map(|row| row.get::<_, i64>(0));
        let actual = match stored_revision {
            Some(revision) if revision > 0 => revision as u64,
            Some(_) => return Err(corrupt_record()),
            None => 0,
        };
        check_revision(&command.expected_revisions, expected_key, actual)?;
        debug_assert_eq!(actual, *expected_revision);
    }
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
        let affected = tx.execute("DELETE FROM buglogin_control_records WHERE tenant_id = $1 AND record_type = $2 AND record_id = $3 AND revision = $4", &[&key.tenant_id, &key.record_type, &key.record_id, &(actual as i64)]).map_err(|_| unavailable())?;
        if item.expected_revision.is_some() && actual > 0 {
            require_single_record_write(tx, &key, actual, affected)?;
        }
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
    for expected_key in command.expected_revisions.keys() {
        let actual = state
            .records
            .get(expected_key)
            .map_or(0, |record| record.revision);
        check_revision(&command.expected_revisions, expected_key, actual)?;
    }
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
    let expected_revisions = expected_revision_checksum_entries(
        &command.expected_revisions,
        &command.puts,
        &command.deletes,
    );
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
    let expected_revisions = expected_revision_checksum_entries(
        &command.expected_revisions,
        &command.puts,
        &command.deletes,
    );
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

fn expected_revision_checksum_entries<'a>(
    expected: &'a BTreeMap<ControlRecordKey, u64>,
    puts: &[ControlPut],
    deletes: &[ControlDelete],
) -> Vec<(&'a ControlRecordKey, &'a u64)> {
    expected
        .iter()
        .filter(|(key, _)| {
            puts.iter().any(|put| {
                put.tenant_id == key.tenant_id
                    && put.record_type == key.record_type
                    && put.record_id == key.record_id
            }) || deletes.iter().any(|delete| {
                delete.tenant_id == key.tenant_id
                    && delete.record_type == key.record_type
                    && delete.record_id == key.record_id
            })
        })
        .collect()
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
fn overloaded() -> Problem {
    problem(
        "control_storage_overloaded",
        "Control storage is busy; retry shortly",
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

    #[test]
    fn postgres_control_store_rejects_work_when_bounded_queue_is_full() {
        let (worker, _receiver) = mpsc::sync_channel::<PgOperation>(PG_WORKER_QUEUE_CAPACITY);
        for _ in 0..PG_WORKER_QUEUE_CAPACITY {
            assert!(worker.try_send(Box::new(|_| {})).is_ok());
        }
        let store = PostgresControlRecordStore {
            worker,
            queued: Arc::new(AtomicUsize::new(0)),
        };

        let error = store.call(|_| Ok(())).unwrap_err();
        assert_eq!(error.code, "control_storage_overloaded");
        assert_eq!(error.status, 503);
        assert!(error.retryable);
    }

    #[test]
    fn dropping_uncertain_shared_lock_client_preserves_other_lock_owners() {
        let mut locks = BTreeMap::from([(7, vec!["first", "uncertain"]), (9, vec!["other"])]);

        discard_latest_shared_lock_client(&mut locks, 7);

        assert_eq!(locks.get(&7), Some(&vec!["first"]));
        assert_eq!(locks.get(&9), Some(&vec!["other"]));
        discard_latest_shared_lock_client(&mut locks, 7);
        assert!(!locks.contains_key(&7));
    }

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
    fn account_scoped_list_only_returns_matching_identity_records() {
        let mut store = MemoryControlRecordStore::default();
        for (record_id, account_id, record_type) in [
            ("refresh-a", "account-a", "device_refresh"),
            ("refresh-b", "account-b", "device_refresh"),
            ("session-a", "account-a", "session"),
        ] {
            let mut write = command(&format!("account-scope-{record_id}"), true);
            write.puts[0].record_type = record_type.into();
            write.puts[0].record_id = record_id.into();
            write.puts[0].value = serde_json::json!({"accountId": account_id});
            store.execute(write).unwrap();
        }

        let rows = store
            .list_for_account("tenant", "device_refresh", "account-a")
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].record_id, "refresh-a");
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
    fn expected_revision_can_fence_a_read_only_authorization_record() {
        let mut store = MemoryControlRecordStore::default();
        let mut seed = command("authorization-fence-seed-0001", true);
        seed.tenant_id = "control".into();
        seed.audit.tenant_id = "control".into();
        seed.puts[0].tenant_id = "control".into();
        seed.puts[0].record_type = "control_state".into();
        seed.puts[0].record_id = "primary".into();
        seed.puts[0].expected_revision = Some(0);
        store.execute(seed).unwrap();

        let mut revoke = command("authorization-fence-revoke-0001", true);
        revoke.tenant_id = "control".into();
        revoke.audit.tenant_id = "control".into();
        revoke.puts[0].tenant_id = "control".into();
        revoke.puts[0].record_type = "control_state".into();
        revoke.puts[0].record_id = "primary".into();
        revoke.puts[0].expected_revision = Some(1);
        revoke.puts[0].value = serde_json::json!({"authorization": "revoked"});
        store.execute(revoke).unwrap();

        let mut stale_job = command("authorization-fence-stale-job-0001", true);
        stale_job.expected_revisions.insert(
            ControlRecordKey {
                tenant_id: "control".into(),
                record_type: "control_state".into(),
                record_id: "primary".into(),
            },
            1,
        );
        stale_job.puts[0].record_type = "connected_cookie_job".into();
        assert_eq!(
            store.execute(stale_job).unwrap_err().code,
            "storage_revision_conflict"
        );
        assert!(
            store
                .get("tenant", "connected_cookie_job", "one")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn revision_checked_delete_conflicts_without_removing_record() {
        let mut store = MemoryControlRecordStore::default();
        store
            .execute(command("request-key-delete-seed", true))
            .unwrap();

        let mut stale_delete = command("request-key-delete-stale", true);
        stale_delete.puts.clear();
        stale_delete.deletes.push(ControlDelete {
            tenant_id: "tenant".into(),
            record_type: "profile".into(),
            record_id: "one".into(),
            expected_revision: Some(0),
        });
        assert_eq!(
            store.execute(stale_delete).unwrap_err().code,
            "storage_revision_conflict"
        );
        assert!(store.get("tenant", "profile", "one").unwrap().is_some());

        let mut current_delete = command("request-key-delete-current", true);
        current_delete.puts.clear();
        current_delete.deletes.push(ControlDelete {
            tenant_id: "tenant".into(),
            record_type: "profile".into(),
            record_id: "one".into(),
            expected_revision: Some(1),
        });
        store.execute(current_delete).unwrap();
        assert!(store.get("tenant", "profile", "one").unwrap().is_none());
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
    fn read_only_preconditions_do_not_change_idempotent_request_identity() {
        let mut store = MemoryControlRecordStore::default();
        let control_key = ControlRecordKey {
            tenant_id: "control".into(),
            record_type: "control_state".into(),
            record_id: "primary".into(),
        };
        let mut first = command("request-key-auth-fence-0001", true);
        first.expected_revisions.insert(control_key.clone(), 0);
        store.execute(first.clone()).unwrap();

        first.expected_revisions.insert(control_key, 1);
        let replay = store.execute(first).unwrap();
        assert!(replay.replayed);
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
    fn abandoning_owned_pending_claim_allows_a_fresh_claim() {
        let mut store = MemoryControlRecordStore::default();
        let checksum = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let claim = match store
            .claim_idempotency(ClaimControlCommand {
                tenant_id: "tenant".into(),
                idempotency_key: "mcp-key-abandon-0001".into(),
                request_checksum: checksum.into(),
                claim_token: "claim-token-owner-0001".into(),
                operation_id: "operation-owner-0001".into(),
            })
            .unwrap()
        {
            ControlIdempotencyClaim::Claimed { claim } => claim,
            other => panic!("expected a new claim, got {other:?}"),
        };

        store.abandon_idempotency(&claim).unwrap();
        assert!(
            store
                .replay("tenant", "mcp-key-abandon-0001")
                .unwrap()
                .is_none()
        );

        assert!(matches!(
            store
                .claim_idempotency(ClaimControlCommand {
                    tenant_id: "tenant".into(),
                    idempotency_key: "mcp-key-abandon-0001".into(),
                    request_checksum: checksum.into(),
                    claim_token: "claim-token-owner-0002".into(),
                    operation_id: "operation-owner-0002".into(),
                })
                .unwrap(),
            ControlIdempotencyClaim::Claimed { .. }
        ));
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
        assert!(
            PostgresConfig::new(
                "postgres://db/app?sslmode=disable&application_name=sslmode=require"
            )
            .is_err()
        );
        assert!(
            PostgresConfig::new("postgres://db/app?sslmode=prefer&application_name=verify-full")
                .is_err()
        );
        assert!(PostgresConfig::new("postgres://db/app?sslmode=verify-full").is_err());
        let config = PostgresConfig::new("postgres://user:secret@db/app?sslmode=require").unwrap();
        let debug = format!("{config:?}");
        assert!(!debug.contains("secret"));
        assert!(debug.contains("<redacted>"));
    }

    #[test]
    fn local_development_postgres_requires_explicit_loopback_and_tls() {
        assert!(
            PostgresConfig::new_local_development(
                "postgres://user:secret@127.0.0.1/app?sslmode=require",
                None
            )
            .is_ok()
        );
        assert!(
            PostgresConfig::new_local_development(
                "postgres://user:secret@localhost/app?sslmode=require",
                None
            )
            .is_ok()
        );
        assert!(
            PostgresConfig::new_local_development(
                "postgres://user:secret@db.example.com/app?sslmode=require",
                None
            )
            .is_err()
        );
        assert!(
            PostgresConfig::new_local_development(
                "postgres://user:secret@127.0.0.1,db.example.com/app?sslmode=require",
                None
            )
            .is_err()
        );
        assert!(
            PostgresConfig::new_local_development(
                "postgres://user:secret@127.0.0.1/app?sslmode=disable",
                None
            )
            .is_err()
        );
    }

    #[test]
    #[ignore = "requires BUGLOGIN_TEST_POSTGRES_URL for a local PostgreSQL instance"]
    fn postgres_shared_writers_can_coexist_and_exclude_cleanup() {
        let database_url = env::var("BUGLOGIN_TEST_POSTGRES_URL")
            .expect("set BUGLOGIN_TEST_POSTGRES_URL to a local TLS PostgreSQL URL");
        let config = PostgresConfig::new_local_development(
            database_url,
            env::var_os("BUGLOGIN_POSTGRES_TLS_ROOT_CERT").map(PathBuf::from),
        )
        .expect("local PostgreSQL config must use loopback TLS");
        let mut writer_one = PostgresControlRecordStore::connect(&config).unwrap();
        let mut writer_two = PostgresControlRecordStore::connect(&config).unwrap();
        let mut cleanup = PostgresControlRecordStore::connect(&config).unwrap();
        let lock_id = i64::MIN
            + i64::from(std::process::id()) * 1_000_000
            + (std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis()
                % 1_000_000) as i64;

        assert!(writer_one.try_shared_advisory_lock(lock_id).unwrap());
        assert!(writer_two.try_shared_advisory_lock(lock_id).unwrap());
        assert!(!cleanup.try_advisory_lock(lock_id).unwrap());

        writer_one.release_shared_advisory_lock(lock_id).unwrap();
        assert!(!cleanup.try_advisory_lock(lock_id).unwrap());
        writer_two.release_shared_advisory_lock(lock_id).unwrap();
        assert!(cleanup.try_advisory_lock(lock_id).unwrap());
        cleanup.release_advisory_lock(lock_id).unwrap();
    }
}
