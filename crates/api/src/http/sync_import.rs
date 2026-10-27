use std::{
    future::Future,
    io::{self, Read, Write},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use axum::{
    Json,
    extract::{Request, State},
    http::{HeaderMap, StatusCode, header::CONTENT_TYPE},
    response::{IntoResponse, Response},
};
use buglogin_domain::errors::Problem;
use buglogin_domain::{
    permissions::{Action, Capability, ResourceScope, is_allowed},
    profile::ProfileArchiveState,
};
use buglogin_storage::control::{
    ClaimControlCommand, ControlIdempotencyClaim, ControlIdempotencyClaimToken,
};
use buglogin_sync::{
    coordination::{
        CoordinationAuth, CoordinationBackend, CoordinationScope, lease_release_idempotency_key,
    },
    export::SyncExport,
    import::{
        SyncImportPlan, SyncStageReceipt, SyncStagedObject, plan_sync_import_manifest,
        sync_object_storage_key,
    },
};
use futures_util::TryStreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::{
    sync::{Semaphore, oneshot},
    task::JoinHandle,
    time::{interval, sleep, timeout},
};
use tokio_util::io::{StreamReader, SyncIoBridge};
use zeroize::Zeroizing;

use super::{
    ApiState, SyncMigrationJournal, SyncMigrationJournalLock, SyncMigrationJournalObject,
    SyncMigrationJournalState, authenticate, checked_now, checked_now_ms, grants, hex_encode,
    problem, request_now_ms, response,
};

const JOURNAL_SCHEMA: &str = "buglogin.migration-sync-journal/1";
pub(super) const MAX_SYNC_IMPORT_REQUEST_BYTES: usize = 320 * 1024 * 1024;
static SYNC_IMPORT_REQUEST_SLOT: OnceLock<Arc<Semaphore>> = OnceLock::new();
const IMPORT_LEASE_TTL: Duration = Duration::from_secs(300);
const IMPORT_LEASE_HEARTBEAT: Duration = Duration::from_secs(60);

struct LeaseHeartbeat {
    stop: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
    failure: Arc<Mutex<Option<Problem>>>,
    healthy: Arc<AtomicBool>,
}

impl LeaseHeartbeat {
    fn start(
        backend: Arc<dyn CoordinationBackend>,
        workspace_id: buglogin_domain::ids::WorkspaceId,
        resource_id: buglogin_domain::ids::ResourceId,
        auth: CoordinationAuth,
        owner_token: String,
        operation_id: String,
    ) -> Self {
        let mut heartbeat_sequence = 0_u64;
        spawn_lease_heartbeat(
            move || {
                let idempotency_key =
                    next_lease_heartbeat_sequence(&mut heartbeat_sequence).map(|sequence| {
                        lease_heartbeat_idempotency_key(&operation_id, &owner_token, &sequence)
                    });
                let backend = Arc::clone(&backend);
                let workspace_id = workspace_id.clone();
                let resource_id = resource_id.clone();
                let auth = auth.clone();
                let owner_token = owner_token.clone();
                async move {
                    let idempotency_key = idempotency_key?;
                    renew_lease_with_retry(
                        backend.as_ref(),
                        &workspace_id,
                        &resource_id,
                        &auth,
                        &owner_token,
                        IMPORT_LEASE_TTL,
                        &idempotency_key,
                    )
                    .await
                    .map(|_| ())
                }
            },
            IMPORT_LEASE_HEARTBEAT,
        )
    }

    async fn stop(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if timeout(Duration::from_secs(2), &mut self.task)
            .await
            .is_err()
        {
            self.task.abort();
        }
    }
}

async fn renew_lease_with_retry(
    backend: &dyn CoordinationBackend,
    workspace_id: &buglogin_domain::ids::WorkspaceId,
    resource_id: &buglogin_domain::ids::ResourceId,
    auth: &CoordinationAuth,
    owner_token: &str,
    ttl: Duration,
    idempotency_key: &str,
) -> Result<buglogin_sync::coordination::LeaseGrant, Problem> {
    let mut attempt = 0;
    loop {
        match backend
            .renew_lease_idempotent(
                CoordinationScope { workspace_id, auth },
                resource_id,
                owner_token,
                ttl,
                idempotency_key,
            )
            .await
        {
            Err(error) if error.retryable && attempt == 0 => {
                attempt += 1;
                sleep(Duration::from_millis(250)).await;
            }
            result => return result,
        }
    }
}

fn spawn_lease_heartbeat<F, Fut>(mut renew: F, period: Duration) -> LeaseHeartbeat
where
    F: FnMut() -> Fut + Send + 'static,
    Fut: Future<Output = Result<(), Problem>> + Send + 'static,
{
    let (stop, mut stop_rx) = oneshot::channel();
    let failure = Arc::new(Mutex::new(None));
    let healthy = Arc::new(AtomicBool::new(true));
    let task_failure = Arc::clone(&failure);
    let task_healthy = Arc::clone(&healthy);
    let task = tokio::spawn(async move {
        let mut ticks = interval(period);
        ticks.tick().await;
        loop {
            tokio::select! {
                _ = &mut stop_rx => break,
                _ = ticks.tick() => {
                    if let Err(error) = renew().await {
                        task_healthy.store(false, Ordering::SeqCst);
                        if let Ok(mut failure) = task_failure.lock() {
                            *failure = Some(error);
                        }
                        break;
                    }
                }
            }
        }
    });
    LeaseHeartbeat {
        stop: Some(stop),
        task,
        failure,
        healthy,
    }
}

impl Drop for LeaseHeartbeat {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct SyncImportInput {
    pub(super) export: SyncExport,
    #[serde(default)]
    pub(super) dry_run: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct SyncImportReceipt {
    pub(super) operation_id: String,
    pub(super) export_id: String,
    pub(super) workspace_id: String,
    pub(super) resource_id: String,
    pub(super) generation: u64,
    pub(super) manifest_sha256: String,
    pub(super) replayed: bool,
    pub(super) dry_run: bool,
    pub(super) created_blobs: usize,
    pub(super) resumed_blobs: usize,
    pub(super) state: String,
}

pub(super) async fn import_sync(
    State(state): State<ApiState>,
    request: Request,
) -> Result<Response, Response> {
    let (parts, body) = request.into_parts();
    let headers = parts.headers;
    let principal = authenticate(&state, &headers)?;
    let content_type = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(str::trim);
    if !content_type.is_some_and(|value| value.eq_ignore_ascii_case("application/json")) {
        return Err(response(problem(
            "unsupported_media_type",
            "Sync import requires application/json",
            415,
        )));
    }
    if super::declared_body_exceeds_limit(&headers, MAX_SYNC_IMPORT_REQUEST_BYTES)
        .map_err(response)?
    {
        return Err(response(
            problem(
                "sync_import_request_too_large",
                "Sync import request exceeds the configured size limit",
                413,
            )
            .with_field("maxBytes", MAX_SYNC_IMPORT_REQUEST_BYTES),
        ));
    }
    let _import_admission = sync_import_request_slot()?;
    let _transfer_admission = super::large_transfer_body_slot().await.map_err(response)?;
    let input = parse_sync_import_input(body, MAX_SYNC_IMPORT_REQUEST_BYTES).await?;
    let receipt = import_sync_authorized(&state, &headers, &principal, input).await?;
    Ok((StatusCode::OK, Json(receipt)).into_response())
}

fn sync_import_request_slot() -> Result<tokio::sync::OwnedSemaphorePermit, Response> {
    let import_permit =
        Arc::clone(SYNC_IMPORT_REQUEST_SLOT.get_or_init(|| Arc::new(Semaphore::new(1))))
            .try_acquire_owned()
            .map_err(|_| {
                response(
                    problem(
                        "sync_import_capacity_exhausted",
                        "Another direct sync import is already running",
                        503,
                    )
                    .retryable(true),
                )
            })?;
    Ok(import_permit)
}

async fn parse_sync_import_input(
    body: axum::body::Body,
    max_bytes: usize,
) -> Result<SyncImportInput, Response> {
    let stream = body
        .into_data_stream()
        .map_err(|error| io::Error::other(error.to_string()));
    let reader = StreamReader::new(stream);
    let parse_result = tokio::task::spawn_blocking(move || {
        let bridge = SyncIoBridge::new(reader);
        // Leave one byte of headroom so a complete JSON body over the configured cap is
        // distinguishable from a valid body exactly at the cap, without buffering the body.
        let limit = u64::try_from(max_bytes)
            .unwrap_or(u64::MAX)
            .saturating_add(1);
        let mut bounded = bridge.take(limit);
        // `from_reader` deserializes one value and can return without consuming trailing
        // bytes. Drive the deserializer to the end so chunked requests cannot bypass the
        // byte limit with a valid JSON prefix and so a second/trailing JSON value is rejected.
        let parsed = (|| {
            let mut deserializer = serde_json::Deserializer::from_reader(&mut bounded);
            let input = SyncImportInput::deserialize(&mut deserializer)?;
            deserializer.end().map(|()| input)
        })();
        if bounded.limit() == 0 {
            Err(true)
        } else {
            parsed.map_err(|_| false)
        }
    })
    .await
    .map_err(|_| {
        response(
            problem(
                "sync_import_parser_unavailable",
                "Sync import parser is temporarily unavailable",
                503,
            )
            .retryable(true),
        )
    })?;

    parse_result.map_err(|too_large| {
        if too_large {
            response(
                problem(
                    "sync_import_request_too_large",
                    "Sync import request exceeds the configured size limit",
                    413,
                )
                .with_field("maxBytes", max_bytes),
            )
        } else {
            response(problem(
                "sync_import_request_invalid",
                "Sync import request JSON is invalid",
                422,
            ))
        }
    })
}

pub(super) async fn import_sync_authorized(
    state: &ApiState,
    headers: &HeaderMap,
    principal: &buglogin_domain::permissions::Principal,
    mut input: SyncImportInput,
) -> Result<SyncImportReceipt, Response> {
    let actor_id = principal
        .account_id
        .as_ref()
        .ok_or_else(|| {
            response(problem(
                "authentication_required",
                "Authentication required",
                401,
            ))
        })?
        .as_str()
        .to_owned();
    input.export.validate().map_err(response)?;
    state.refresh_control_aggregate().map_err(response)?;
    let now = checked_now("sync-import-preflight").map_err(response)?;
    let principal = state
        .authenticate_token(&bearer_token(headers)?, now)
        .map_err(response)?;
    if principal
        .account_id
        .as_ref()
        .map(|account| account.as_str())
        != Some(actor_id.as_str())
    {
        return Err(response(problem(
            "authentication_required",
            "Authentication is no longer valid for this import",
            401,
        )));
    }
    authorize(&state, &principal, &input.export)?;
    verify_binding(&state, &input.export)?;
    let grants = state.grants().map_err(response)?;
    super::control::validate_profile_proxy_dependencies_problem(
        state,
        &principal,
        &grants,
        &input.export.workspace_id,
        &input.export.resource_id,
        &input.export.manifest,
        false,
        false,
        true,
    )
    .map_err(response)?;
    let idempotency_key = required_idempotency(&headers)?;
    let (request_hash, export_hash) = sync_import_hashes(&input)?;
    // Full export validation and request hashing are complete. Move, rather than clone, the
    // large payload buffer out of the metadata envelope used by authorization/finalization.
    let import_blobs = std::mem::take(&mut input.export.blobs);
    let canonical_key = sha256_parts(&[
        "migration-sync-v1",
        &actor_id,
        input.export.workspace_id.as_str(),
        input.export.resource_id.as_str(),
        idempotency_key,
    ]);
    let store_key = format!("migration-sync:{canonical_key}");
    let operation_id = format!("op_{}", &request_hash[..32]);
    let journal_id = format!("msi_{}", &canonical_key[..40]);

    if input.dry_run {
        let current = state
            .current_sync_manifest_fresh(&input.export.workspace_id, &input.export.resource_id)
            .map_err(response)?;
        let fencing_token = match current.as_ref() {
            Some(manifest) => manifest.fencing_token.checked_add(1).ok_or_else(|| {
                response(problem(
                    "sync_fencing_token_exhausted",
                    "Profile sync fencing token cannot be incremented",
                    409,
                ))
            })?,
            None => 1,
        };
        let plan = plan_sync_import_manifest(&input.export, current.as_ref(), fencing_token)
            .map_err(response)?;
        return Ok(receipt(
            &operation_id,
            &input.export,
            &plan,
            true,
            0,
            0,
            "planned",
        ));
    }

    // Recovery takes the same journal-scoped lock before fencing a stale staging or finalizing
    // record. Holding it through commit/release prevents age-based cleanup from racing a live
    // importer that is still renewing its resource lease.
    let _journal_lock = SyncMigrationJournalLock::acquire(state, &journal_id).map_err(response)?;

    // The in-process semaphore does not coordinate multiple API instances. A
    // PostgreSQL advisory lock prevents two retries with the same idempotency
    // key from both reconstructing the same pending claim and writing the same
    // journal concurrently. Different idempotency keys remain coordinated by
    // the resource lease below.
    let _idempotency_lock =
        SyncImportIdempotencyLock::acquire(state, &store_key).map_err(response)?;

    let claim_token = sha256_parts(&[
        "migration-sync-claim-v1",
        &store_key,
        &request_hash,
        &operation_id,
    ]);
    let claim = match state
        .control_persistence
        .claim_idempotency(ClaimControlCommand {
            tenant_id: "control".into(),
            idempotency_key: store_key.clone(),
            request_checksum: request_hash.clone(),
            claim_token: claim_token.clone(),
            operation_id: operation_id.clone(),
        }) {
        Ok(ControlIdempotencyClaim::Claimed { claim }) => claim,
        Ok(ControlIdempotencyClaim::Replay { result }) => {
            if result.response.status >= 400 {
                return Err(response(problem(
                    "migration_sync_replay_failed",
                    "Stored sync migration response is not successful",
                    result.response.status,
                )));
            }
            return serde_json::from_value(result.response.body).map_err(|_| {
                response(problem(
                    "migration_sync_replay_invalid",
                    "Stored sync migration receipt is invalid",
                    500,
                ))
            });
        }
        Ok(ControlIdempotencyClaim::Pending {
            operation_id: pending,
        }) if pending == operation_id => ControlIdempotencyClaimToken {
            tenant_id: "control".into(),
            idempotency_key: store_key.clone(),
            request_checksum: request_hash.clone(),
            claim_token,
            operation_id: operation_id.clone(),
        },
        Ok(ControlIdempotencyClaim::Pending { .. }) => {
            return Err(response(problem(
                "migration_sync_import_in_progress",
                "Sync migration import is already in progress",
                409,
            )));
        }
        Err(error) => return Err(response(error)),
    };

    let control_revision_before = state.control_revision().map_err(response)?;
    let (mut journal, mut journal_revision) = match state
        .load_sync_migration_journal(&journal_id)
        .map_err(response)?
    {
        Some((journal, revision)) => {
            if journal.export_sha256 != export_hash
                || journal.workspace_id != input.export.workspace_id.as_str()
                || journal.resource_id != input.export.resource_id.as_str()
                || matches!(
                    journal.state,
                    SyncMigrationJournalState::Active | SyncMigrationJournalState::CleanupPending
                )
            {
                return Err(response(problem(
                    "migration_sync_journal_conflict",
                    "Sync migration journal conflicts with request",
                    409,
                )));
            }
            if journal.state == SyncMigrationJournalState::Abandoned {
                let mut journal = journal;
                journal.state = SyncMigrationJournalState::Staging;
                journal.created_objects.clear();
                journal.resumed_objects.clear();
                journal.reserved_objects.clear();
                journal.destination_fencing_token = None;
                journal.control_revision_before = control_revision_before;
                journal.control_revision_after = None;
                journal.updated_at_ms = request_now_ms(headers)?;
                journal.error_code = None;
                let next_revision = state
                    .persist_sync_migration_journal(
                        &journal_id,
                        revision,
                        journal_write_key(&journal_id, "staging-reopen", revision),
                        actor_id.clone(),
                        &journal,
                    )
                    .map_err(response)?;
                (journal, next_revision)
            } else {
                (journal, revision)
            }
        }
        None => {
            let journal = SyncMigrationJournal {
                schema: JOURNAL_SCHEMA.into(),
                operation_id: operation_id.clone(),
                export_id: input.export.export_id.clone(),
                export_sha256: export_hash.clone(),
                manifest_sha256: input.export.manifest_sha256.clone(),
                blob_set_sha256: input.export.blob_set_sha256.clone(),
                workspace_id: input.export.workspace_id.as_str().into(),
                resource_id: input.export.resource_id.as_str().into(),
                base_generation: input.export.base_generation,
                target_generation: input.export.current_generation,
                state: SyncMigrationJournalState::Staging,
                created_objects: vec![],
                resumed_objects: vec![],
                reserved_objects: vec![],
                destination_fencing_token: None,
                control_revision_before,
                control_revision_after: None,
                updated_at_ms: request_now_ms(headers)?,
                error_code: None,
            };
            let revision = state
                .persist_sync_migration_journal(
                    &journal_id,
                    0,
                    journal_write_key(&journal_id, "staging", 0),
                    actor_id.clone(),
                    &journal,
                )
                .map_err(response)?;
            (journal, revision)
        }
    };

    let mut raw_bearer = bearer_token(&headers)?;
    let auth = CoordinationAuth::new(actor_id.clone(), std::mem::take(&mut *raw_bearer))
        .map_err(response)?;
    let backend = state.coordination().map_err(response)?;
    let lease_idempotency_key = lease_attempt_idempotency_key(&operation_id).map_err(response)?;
    let mut lease = backend
        .acquire_lease_idempotent(
            CoordinationScope {
                workspace_id: &input.export.workspace_id,
                auth: &auth,
            },
            &input.export.resource_id,
            IMPORT_LEASE_TTL,
            &lease_idempotency_key,
        )
        .await
        .map_err(|error| {
            let error = error.retryable(true);
            let error = persist_stage_failure(
                state,
                &journal_id,
                journal_revision,
                &actor_id,
                &mut journal,
                error,
            );
            response(error)
        })?;
    // The request may have waited behind authorization or a previous writer while acquiring the
    // lease. Refresh and revalidate the actor, ACL, binding, and proxy dependencies before any
    // object-store writes, then plan against the same refreshed control snapshot.
    let current = (|| {
        let _mutation = state
            .control_mutation()
            .lock()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?;
        state.refresh_control_aggregate_locked()?;
        let current_principal =
            state.authenticate_token(auth.bearer_token(), checked_now("sync-import-stage")?)?;
        if current_principal
            .account_id
            .as_ref()
            .map(|account| account.as_str())
            != Some(actor_id.as_str())
        {
            return Err(problem(
                "authentication_required",
                "Authentication is no longer valid for this import",
                401,
            ));
        }
        authorize_problem(state, &current_principal, &input.export)?;
        let current_grants = state.grants()?;
        let aggregate = state.control_aggregate()?;
        verify_binding_aggregate(&aggregate, &input.export)?;
        super::control::validate_profile_proxy_dependencies_problem(
            state,
            &current_principal,
            &current_grants,
            &input.export.workspace_id,
            &input.export.resource_id,
            &input.export.manifest,
            false,
            false,
            true,
        )?;
        state.current_sync_manifest(&input.export.workspace_id, &input.export.resource_id)
    })();
    let current = match current {
        Ok(current) => current,
        Err(error) => {
            let error = persist_stage_failure(
                state,
                &journal_id,
                journal_revision,
                &actor_id,
                &mut journal,
                error,
            );
            release(&*backend, &input.export, &auth, &lease.owner_token).await;
            return Err(response(error));
        }
    };
    let plan = match plan_sync_import_manifest(&input.export, current.as_ref(), lease.fencing_token)
    {
        Ok(mut plan) => {
            plan.blobs = import_blobs;
            plan
        }
        Err(error) => {
            let error = persist_stage_failure(
                state,
                &journal_id,
                journal_revision,
                &actor_id,
                &mut journal,
                error,
            );
            release(&*backend, &input.export, &auth, &lease.owner_token).await;
            return Err(response(error));
        }
    };
    let heartbeat = LeaseHeartbeat::start(
        Arc::clone(&backend),
        input.export.workspace_id.clone(),
        input.export.resource_id.clone(),
        auth.clone(),
        lease.owner_token.clone(),
        operation_id.clone(),
    );
    let stage_state = state.clone();
    let stage_plan = plan;
    let stage_journal_id = journal_id.clone();
    let stage_actor_id = actor_id.clone();
    let stage_journal = journal.clone();
    let stage_result = tokio::task::spawn_blocking(move || {
        let mut stage_journal = stage_journal;
        let result = stage_import_objects(
            &stage_state,
            &stage_plan,
            &stage_journal_id,
            journal_revision,
            &stage_actor_id,
            &mut stage_journal,
        )
        .and_then(|(receipt, revision)| {
            verify_staged(&stage_state, &receipt)
                .map(|()| (receipt, revision))
                .map_err(|error| (error, revision))
        });
        (result, stage_journal, stage_plan)
    })
    .await;
    let heartbeat_failure_state = Arc::clone(&heartbeat.failure);
    let heartbeat_health_state = Arc::clone(&heartbeat.healthy);
    heartbeat.stop().await;
    let heartbeat_failure = heartbeat_failure_state
        .lock()
        .ok()
        .and_then(|failure| failure.clone());
    let heartbeat_healthy = heartbeat_health_state.load(Ordering::SeqCst);
    let (stage_result, stage_journal, plan) = match stage_result {
        Ok(result) => result,
        Err(_) => {
            if let Ok(Some((durable_journal, durable_revision))) =
                state.load_sync_migration_journal(&journal_id)
            {
                journal = durable_journal;
                journal_revision = durable_revision;
            }
            let error = Problem::new(
                "sync_import_staging_task_failed",
                "Sync import staging task failed",
                503,
                "sync-import",
            )
            .retryable(true);
            let error = persist_stage_failure(
                state,
                &journal_id,
                journal_revision,
                &actor_id,
                &mut journal,
                error,
            );
            release(&*backend, &input.export, &auth, &lease.owner_token).await;
            return Err(response(error));
        }
    };
    journal = stage_journal;
    let (staged, next_journal_revision) = match stage_result {
        Ok(result) => result,
        Err((error, revision)) => {
            let error =
                persist_stage_failure(state, &journal_id, revision, &actor_id, &mut journal, error);
            release(&*backend, &input.export, &auth, &lease.owner_token).await;
            return Err(response(error));
        }
    };
    journal_revision = next_journal_revision;
    if !heartbeat_healthy {
        let error = heartbeat_failure
            .unwrap_or_else(|| {
                Problem::new(
                    "sync_import_lease_lost",
                    "Sync import lease was lost during staging",
                    503,
                    "sync-import",
                )
            })
            .retryable(true);
        let error = persist_stage_failure(
            state,
            &journal_id,
            journal_revision,
            &actor_id,
            &mut journal,
            error,
        );
        release(&*backend, &input.export, &auth, &lease.owner_token).await;
        return Err(response(error));
    }
    let finalize_fencing_token = lease.fencing_token.to_string();
    let finalize_lease_expiry = lease.expires_at_ms.to_string();
    let finalize_renewal_key = format!(
        "lease-renew-{}",
        sha256_parts(&[
            &operation_id,
            "finalize",
            &finalize_fencing_token,
            &finalize_lease_expiry,
        ])
    );
    lease = match renew_lease_with_retry(
        backend.as_ref(),
        &input.export.workspace_id,
        &input.export.resource_id,
        &auth,
        &lease.owner_token,
        Duration::from_secs(300),
        &finalize_renewal_key,
    )
    .await
    {
        Ok(lease) => lease,
        Err(error) => {
            let error = error.retryable(true);
            let error = persist_stage_failure(
                state,
                &journal_id,
                journal_revision,
                &actor_id,
                &mut journal,
                error,
            );
            release(&*backend, &input.export, &auth, &lease.owner_token).await;
            return Err(response(error));
        }
    };

    let (created_objects, resumed_objects) = match journal_objects(&plan, &staged) {
        Ok(objects) => objects,
        Err(error) => {
            let error = persist_stage_failure(
                state,
                &journal_id,
                journal_revision,
                &actor_id,
                &mut journal,
                error,
            );
            release(&*backend, &input.export, &auth, &lease.owner_token).await;
            return Err(response(error));
        }
    };
    journal.state = SyncMigrationJournalState::Finalizing;
    journal.created_objects = created_objects;
    journal.resumed_objects = resumed_objects;
    journal.destination_fencing_token = Some(lease.fencing_token);
    journal.updated_at_ms = request_now_ms(headers)?;
    journal.error_code = None;
    journal_revision = match state.persist_sync_migration_journal(
        &journal_id,
        journal_revision,
        journal_write_key(&journal_id, "finalizing", journal_revision),
        actor_id.clone(),
        &journal,
    ) {
        Ok(revision) => revision,
        Err(error) => {
            // The journal write may have committed before its acknowledgement was lost.
            // Keep immutable objects intact so a retry or recovery worker can reconcile it.
            release(&*backend, &input.export, &auth, &lease.owner_token).await;
            return Err(response(error));
        }
    };

    let snapshot = std::sync::Arc::clone(&state.control_snapshot_gate)
        .write_owned()
        .await;
    if let Err(error) = backend
        .validate_write_authority(
            CoordinationScope {
                workspace_id: &input.export.workspace_id,
                auth: &auth,
            },
            &input.export.resource_id,
            &lease.owner_token,
            lease.fencing_token,
        )
        .await
    {
        let error = error.retryable(true);
        let error = persist_stage_failure(
            state,
            &journal_id,
            journal_revision,
            &actor_id,
            &mut journal,
            error,
        );
        drop(snapshot);
        release(&*backend, &input.export, &auth, &lease.owner_token).await;
        return Err(response(error));
    }

    let finalize_result: Result<SyncImportReceipt, Problem> = (|| {
        let _snapshot = snapshot;
        // Refresh while holding the same mutation lock used by control writes.
        // Otherwise a local ACL or binding update can land between refresh and
        // lock acquisition, letting this commit authorize against stale state.
        let _mutation = state
            .control_mutation()
            .lock()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?;
        state.refresh_control_aggregate_locked()?;
        let token = bearer_token(headers)
            .map_err(|_| problem("authentication_required", "Authentication required", 401))?;
        let current_principal =
            state.authenticate_token(&token, checked_now("sync-import-commit")?)?;
        let current_actor_id = current_principal
            .account_id
            .as_ref()
            .map(|account| account.as_str());
        if current_actor_id != Some(actor_id.as_str()) {
            return Err(problem(
                "authentication_required",
                "Authentication is no longer valid for this import",
                401,
            ));
        }
        authorize_problem(state, &current_principal, &input.export)?;
        let grant_snapshot = state.grants()?;
        let mut target = state.control_aggregate()?;
        verify_binding_aggregate(&target, &input.export)?;
        let key = format!(
            "{}:{}",
            input.export.workspace_id.as_str(),
            input.export.resource_id.as_str()
        );
        let current = target.sync_manifests.get(&key).cloned();
        let final_plan =
            plan_sync_import_manifest(&input.export, current.as_ref(), lease.fencing_token)?;
        super::control::validate_profile_proxy_dependencies_problem(
            state,
            &current_principal,
            &grant_snapshot,
            &input.export.workspace_id,
            &input.export.resource_id,
            &final_plan.manifest,
            true,
            false,
            true,
        )?;
        super::validate_manifest_gc_claims(&final_plan.manifest, &target.sync_gc_claims)?;
        target
            .sync_manifests
            .insert(key, final_plan.manifest.clone());
        target.mark_migrated_profile_ready_after_sync(&final_plan.manifest)?;
        journal.state = SyncMigrationJournalState::Active;
        journal.control_revision_before = state.control_revision()?;
        journal.control_revision_after = Some(
            journal
                .control_revision_before
                .checked_add(1)
                .ok_or_else(|| {
                    Problem::new(
                        "control_revision_exhausted",
                        "Control revision cannot be incremented",
                        409,
                        "sync-import",
                    )
                })?,
        );
        journal.updated_at_ms = checked_now_ms("sync-import-commit")?;
        let created = staged
            .objects
            .iter()
            .filter(|object| object.created)
            .count();
        let resumed = staged.objects.len().saturating_sub(created);
        let receipt = receipt(
            &operation_id,
            &input.export,
            &final_plan,
            false,
            created,
            resumed,
            "active",
        );
        let body = serde_json::to_value(&receipt)
            .map_err(|_| problem("serialization_failed", "Response serialization failed", 500))?;
        state
            .finalize_sync_import(
                claim,
                target,
                &journal_id,
                journal_revision,
                &journal,
                &request_hash,
                actor_id.clone(),
                serde_json::json!({
                    "operationId": operation_id,
                    "journalId": journal_id,
                    "workspaceId": input.export.workspace_id,
                    "resourceId": input.export.resource_id,
                    "exportSha256": export_hash,
                    "manifestSha256": input.export.manifest_sha256,
                    "createdBlobs": created,
                    "resumedBlobs": resumed,
                }),
                body.clone(),
            )
            .map(|_| receipt.clone())
    })();

    release(&*backend, &input.export, &auth, &lease.owner_token).await;
    match finalize_result {
        Ok(receipt) => Ok(receipt),
        Err(error) => match state.replay_control_response(&store_key, &request_hash) {
            Ok(Some(stored)) => {
                state.reload_control_aggregate().map_err(response)?;
                if stored.status >= 400 {
                    return Err(response(problem(
                        "migration_sync_replay_failed",
                        "Stored sync migration response is not successful",
                        stored.status,
                    )));
                }
                serde_json::from_value(stored.body).map_err(|_| {
                    response(problem(
                        "migration_sync_replay_invalid",
                        "Stored sync migration receipt is invalid",
                        500,
                    ))
                })
            }
            _ => Err(response(
                problem(
                    "migration_sync_commit_outcome_unknown",
                    "Sync migration commit outcome requires reconciliation",
                    503,
                )
                .retryable(true)
                .with_field("cause", error.code),
            )),
        },
    }
}

fn authorize(
    state: &ApiState,
    principal: &buglogin_domain::permissions::Principal,
    export: &SyncExport,
) -> Result<(), Response> {
    let capability = Capability::new("migration.write")
        .map_err(|_| response(problem("invalid_capability", "Invalid capability", 500)))?;
    let scope = ResourceScope::resource(export.workspace_id.clone(), export.resource_id.clone());
    if is_allowed(
        principal,
        &capability,
        Action::Execute,
        &scope,
        &grants(state)?,
    ) {
        Ok(())
    } else {
        Err(response(problem(
            "migration_forbidden",
            "Migration access denied",
            403,
        )))
    }
}

fn authorize_problem(
    state: &ApiState,
    principal: &buglogin_domain::permissions::Principal,
    export: &SyncExport,
) -> Result<(), Problem> {
    let capability = Capability::new("migration.write")
        .map_err(|_| problem("invalid_capability", "Invalid capability", 500))?;
    let scope = ResourceScope::resource(export.workspace_id.clone(), export.resource_id.clone());
    if is_allowed(
        principal,
        &capability,
        Action::Execute,
        &scope,
        &state.grants()?,
    ) {
        Ok(())
    } else {
        Err(problem(
            "migration_forbidden",
            "Migration access denied",
            403,
        ))
    }
}

fn verify_binding(state: &ApiState, export: &SyncExport) -> Result<(), Response> {
    verify_binding_problem(state, export).map_err(response)
}

fn verify_binding_problem(state: &ApiState, export: &SyncExport) -> Result<(), Problem> {
    let resource_exists = state
        .resources
        .read()
        .map_err(|_| problem("resource_unavailable", "Resource state unavailable", 503))?
        .exists_in_workspace(&export.resource_id, &export.workspace_id);
    let binding_exists = state
        .profile_resource_for_resource(&export.workspace_id, &export.resource_id)?
        .is_some_and(|binding| binding.archive_state == ProfileArchiveState::Active);
    if resource_exists && binding_exists {
        Ok(())
    } else {
        Err(problem(
            "migration_sync_binding_not_found",
            "Active profile resource binding not found",
            404,
        ))
    }
}

fn verify_binding_aggregate(
    aggregate: &super::ControlAggregate,
    export: &SyncExport,
) -> Result<(), Problem> {
    let resource_exists = aggregate
        .resources
        .exists_in_workspace(&export.resource_id, &export.workspace_id);
    let binding_exists = aggregate.profile_resources.values().any(|binding| {
        binding.workspace_id == export.workspace_id
            && binding.resource_id == export.resource_id
            && binding.archive_state == ProfileArchiveState::Active
    });
    if resource_exists && binding_exists {
        Ok(())
    } else {
        Err(problem(
            "migration_sync_binding_not_found",
            "Active profile resource binding not found",
            404,
        ))
    }
}

fn stage_import_objects(
    state: &ApiState,
    plan: &SyncImportPlan,
    journal_id: &str,
    mut journal_revision: u64,
    actor_id: &str,
    journal: &mut SyncMigrationJournal,
) -> Result<(SyncStageReceipt, u64), (Problem, u64)> {
    let mut staged = Vec::with_capacity(plan.blobs.len());
    journal.state = SyncMigrationJournalState::Staging;
    journal.reserved_objects = plan
        .blobs
        .iter()
        .map(|blob| SyncMigrationJournalObject {
            storage_key: sync_object_storage_key(&plan.manifest, blob),
            checksum: blob.checksum.clone(),
            size: blob.size,
        })
        .collect();
    if let Err(error) =
        persist_staging_checkpoint(state, journal_id, &mut journal_revision, actor_id, journal)
    {
        return Err((error, journal_revision));
    }
    if let Err(error) = state.reserve_sync_blob_uploads(
        &plan.manifest.workspace_id,
        &plan.manifest.resource_id,
        &journal.reserved_objects,
        checked_now_ms("sync-import-stage").map_err(|error| (error, journal_revision))?,
        actor_id,
    ) {
        return Err((error, journal_revision));
    }
    for blob in &plan.blobs {
        let storage_key = sync_object_storage_key(&plan.manifest, blob);
        let already_created = journal
            .created_objects
            .iter()
            .any(|object| object.storage_key == storage_key && object.checksum == blob.checksum);
        let existing = match state.remote_chunk_checksum(&storage_key) {
            Ok(existing) => existing,
            Err(error) => return Err((error, journal_revision)),
        };
        match existing {
            Some(existing) if existing == blob.checksum => {
                let workspace_id = plan.manifest.workspace_id.as_str();
                let resource_id = plan.manifest.resource_id.as_str();
                match state.try_sync_object_write_lock(workspace_id, resource_id) {
                    Ok(true) => {}
                    Ok(false) => {
                        return Err((
                            Problem::new(
                                "sync_object_cleanup_in_progress",
                                "Sync object cleanup is in progress; retry the import",
                                503,
                                "sync-import",
                            )
                            .retryable(true),
                            journal_revision,
                        ));
                    }
                    Err(error) => return Err((error, journal_revision)),
                }
                let stage_result = (|| {
                    state.validate_sync_stage_write(journal_id, journal_revision, &storage_key)?;
                    let current = state.remote_chunk_checksum(&storage_key)?;
                    match current {
                        Some(current) if current == blob.checksum => {
                            if !already_created {
                                remember_journal_object(
                                    &mut journal.resumed_objects,
                                    &storage_key,
                                    &blob.checksum,
                                    blob.size,
                                );
                            }
                            journal.error_code = None;
                            persist_staging_checkpoint(
                                state,
                                journal_id,
                                &mut journal_revision,
                                actor_id,
                                journal,
                            )?;
                            Ok(())
                        }
                        Some(_) => Err(Problem::new(
                            "sync_import_blob_collision",
                            "Sync import blob collides with destination",
                            409,
                            "sync-import",
                        )),
                        None => Err(Problem::new(
                            "sync_import_blob_missing",
                            "Sync import blob disappeared during staging",
                            409,
                            "sync-import",
                        )
                        .retryable(true)),
                    }
                })();
                let release_result =
                    state.release_sync_object_write_lock(workspace_id, resource_id);
                match (stage_result, release_result) {
                    (Ok(()), Ok(())) => staged.push(SyncStagedObject {
                        storage_key,
                        checksum: blob.checksum.clone(),
                        created: already_created,
                    }),
                    (Err(error), _) => return Err((error, journal_revision)),
                    (Ok(()), Err(error)) => return Err((error, journal_revision)),
                }
            }
            Some(_) => {
                return Err((
                    Problem::new(
                        "sync_import_blob_collision",
                        "Sync import blob collides with destination",
                        409,
                        "sync-import",
                    ),
                    journal_revision,
                ));
            }
            None => {
                remember_journal_object(
                    &mut journal.created_objects,
                    &storage_key,
                    &blob.checksum,
                    blob.size,
                );
                journal
                    .resumed_objects
                    .retain(|object| object.storage_key != storage_key);
                journal.error_code = None;
                if let Err(error) = persist_staging_checkpoint(
                    state,
                    journal_id,
                    &mut journal_revision,
                    actor_id,
                    journal,
                ) {
                    return Err((error, journal_revision));
                }
                let workspace_id = plan.manifest.workspace_id.as_str();
                let resource_id = plan.manifest.resource_id.as_str();
                match state.try_sync_object_write_lock(workspace_id, resource_id) {
                    Ok(true) => {}
                    Ok(false) => {
                        return Err((
                            Problem::new(
                                "sync_object_cleanup_in_progress",
                                "Sync object cleanup is in progress; retry the import",
                                503,
                                "sync-import",
                            )
                            .retryable(true),
                            journal_revision,
                        ));
                    }
                    Err(error) => return Err((error, journal_revision)),
                }
                let stage_result = (|| {
                    state.validate_sync_stage_write(journal_id, journal_revision, &storage_key)?;
                    match state.remote_chunk_checksum(&storage_key)? {
                        Some(existing) if existing == blob.checksum => {
                            journal.created_objects.retain(|object| {
                                object.storage_key != storage_key
                                    || object.checksum != blob.checksum
                            });
                            remember_journal_object(
                                &mut journal.resumed_objects,
                                &storage_key,
                                &blob.checksum,
                                blob.size,
                            );
                            persist_staging_checkpoint(
                                state,
                                journal_id,
                                &mut journal_revision,
                                actor_id,
                                journal,
                            )?;
                            Ok(false)
                        }
                        Some(_) => Err(Problem::new(
                            "sync_import_blob_collision",
                            "Sync import blob collides with destination",
                            409,
                            "sync-import",
                        )),
                        None => {
                            state.put_remote_chunk(&storage_key, &blob.data, &blob.checksum)?;
                            persist_staging_checkpoint(
                                state,
                                journal_id,
                                &mut journal_revision,
                                actor_id,
                                journal,
                            )?;
                            Ok(true)
                        }
                    }
                })();
                let release_result =
                    state.release_sync_object_write_lock(workspace_id, resource_id);
                let created = match (stage_result, release_result) {
                    (Ok(created), Ok(())) => created,
                    (Err(error), _) => return Err((error, journal_revision)),
                    (Ok(_), Err(error)) => return Err((error, journal_revision)),
                };
                staged.push(SyncStagedObject {
                    storage_key,
                    checksum: blob.checksum.clone(),
                    created,
                });
                continue;
            }
        }
        journal.error_code = None;
        if let Err(error) =
            persist_staging_checkpoint(state, journal_id, &mut journal_revision, actor_id, journal)
        {
            return Err((error, journal_revision));
        }
    }
    Ok((SyncStageReceipt { objects: staged }, journal_revision))
}

fn remember_journal_object(
    objects: &mut Vec<SyncMigrationJournalObject>,
    storage_key: &str,
    checksum: &str,
    size: u64,
) {
    if let Some(object) = objects
        .iter_mut()
        .find(|object| object.storage_key == storage_key)
    {
        object.checksum = checksum.to_owned();
        object.size = size;
        return;
    }
    objects.push(SyncMigrationJournalObject {
        storage_key: storage_key.to_owned(),
        checksum: checksum.to_owned(),
        size,
    });
}

fn persist_staging_checkpoint(
    state: &ApiState,
    journal_id: &str,
    journal_revision: &mut u64,
    actor_id: &str,
    journal: &mut SyncMigrationJournal,
) -> Result<(), Problem> {
    journal.updated_at_ms = checked_now_ms("sync-import-stage")?;
    *journal_revision = state.persist_sync_migration_journal(
        journal_id,
        *journal_revision,
        journal_write_key(journal_id, "staging", *journal_revision),
        actor_id.to_owned(),
        journal,
    )?;
    Ok(())
}

fn persist_stage_failure(
    state: &ApiState,
    journal_id: &str,
    mut journal_revision: u64,
    actor_id: &str,
    journal: &mut SyncMigrationJournal,
    error: Problem,
) -> Problem {
    journal.state = if error.retryable {
        SyncMigrationJournalState::Staging
    } else {
        SyncMigrationJournalState::CleanupPending
    };
    journal.error_code = Some(error.code.clone());
    match persist_staging_checkpoint(state, journal_id, &mut journal_revision, actor_id, journal) {
        Ok(()) if journal.state == SyncMigrationJournalState::CleanupPending => {
            let objects = journal
                .reserved_objects
                .iter()
                .chain(&journal.created_objects)
                .chain(&journal.resumed_objects)
                .cloned()
                .collect::<Vec<_>>();
            match state.release_sync_blob_reservations_for_import(
                journal_id,
                &journal.workspace_id,
                &journal.resource_id,
                &objects,
                actor_id,
            ) {
                Ok(()) => error,
                Err(release_error) => Problem::new(
                    "sync_import_reservation_release_failed",
                    "Sync import failed and storage reservations could not be released",
                    503,
                    "sync-import",
                )
                .retryable(true)
                .with_field("stageErrorCode", error.code)
                .with_field("releaseErrorCode", release_error.code),
            }
        }
        Ok(()) => error,
        Err(checkpoint_error) => Problem::new(
            "sync_import_recovery_checkpoint_failed",
            "Sync import failed and its recovery checkpoint could not be saved",
            503,
            "sync-import",
        )
        .retryable(true)
        .with_field("stageErrorCode", error.code)
        .with_field("checkpointErrorCode", checkpoint_error.code),
    }
}

fn verify_staged(
    state: &ApiState,
    staged: &buglogin_sync::import::SyncStageReceipt,
) -> Result<(), buglogin_domain::errors::Problem> {
    for object in &staged.objects {
        state.get_remote_chunk(&object.storage_key, &object.checksum)?;
    }
    Ok(())
}

fn journal_objects(
    plan: &SyncImportPlan,
    staged: &buglogin_sync::import::SyncStageReceipt,
) -> Result<
    (
        Vec<SyncMigrationJournalObject>,
        Vec<SyncMigrationJournalObject>,
    ),
    Problem,
> {
    let sizes_by_storage_key = plan
        .blobs
        .iter()
        .map(|blob| (sync_object_storage_key(&plan.manifest, blob), blob.size))
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut created = vec![];
    let mut resumed = vec![];
    for object in &staged.objects {
        let size = sizes_by_storage_key
            .get(&object.storage_key)
            .copied()
            .ok_or_else(|| {
                Problem::new(
                    "sync_import_staged_object_missing",
                    "Staged sync object is missing from the import plan",
                    500,
                    "sync-import",
                )
            })?;
        let value = SyncMigrationJournalObject {
            storage_key: object.storage_key.clone(),
            checksum: object.checksum.clone(),
            size,
        };
        if object.created {
            created.push(value);
        } else {
            resumed.push(value);
        }
    }
    Ok((created, resumed))
}

fn required_idempotency(headers: &HeaderMap) -> Result<&str, Response> {
    headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
        .filter(|value| value.is_ascii() && (16..=128).contains(&value.len()))
        .ok_or_else(|| {
            response(problem(
                "idempotency_key_required",
                "Valid idempotency key required",
                400,
            ))
        })
}

fn bearer_token(headers: &HeaderMap) -> Result<Zeroizing<String>, Response> {
    headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(|value| Zeroizing::new(value.to_owned()))
        .ok_or_else(|| {
            response(problem(
                "authentication_required",
                "Authentication required",
                401,
            ))
        })
}

async fn release(
    backend: &dyn CoordinationBackend,
    export: &SyncExport,
    auth: &CoordinationAuth,
    owner_token: &str,
) {
    let scope = CoordinationScope {
        workspace_id: &export.workspace_id,
        auth,
    };
    let idempotency_key = lease_release_idempotency_key(&scope, &export.resource_id, owner_token);
    let _ = backend
        .release_lease_idempotent(scope, &export.resource_id, owner_token, &idempotency_key)
        .await;
}

fn receipt(
    operation_id: &str,
    export: &SyncExport,
    plan: &SyncImportPlan,
    dry_run: bool,
    created_blobs: usize,
    resumed_blobs: usize,
    state: &'static str,
) -> SyncImportReceipt {
    SyncImportReceipt {
        operation_id: operation_id.into(),
        export_id: export.export_id.clone(),
        workspace_id: export.workspace_id.as_str().into(),
        resource_id: export.resource_id.as_str().into(),
        generation: plan.manifest.generation,
        manifest_sha256: export.manifest_sha256.clone(),
        replayed: plan.replayed,
        dry_run,
        created_blobs,
        resumed_blobs,
        state: state.to_owned(),
    }
}

fn journal_write_key(journal_id: &str, state: &str, revision: u64) -> String {
    format!(
        "migration-sync-journal:{}",
        sha256_parts(&[journal_id, state, &revision.to_string()])
    )
}

fn sync_import_hashes(input: &SyncImportInput) -> Result<(String, String), Response> {
    struct HashWriter {
        request: Sha256,
        export: Sha256,
    }

    impl Write for HashWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.request.update(bytes);
            self.export.update(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let mut writer = HashWriter {
        request: Sha256::new(),
        export: Sha256::new(),
    };
    writer.request.update(b"{\"export\":");
    serde_json::to_writer(&mut writer, &input.export)
        .map_err(|_| response(problem("invalid_input", "Input is invalid", 400)))?;
    writer.request.update(if input.dry_run {
        &b",\"dryRun\":true}"[..]
    } else {
        &b",\"dryRun\":false}"[..]
    });
    Ok((
        format!("{:x}", writer.request.finalize()),
        format!("{:x}", writer.export.finalize()),
    ))
}

fn sha256_parts(parts: &[&str]) -> String {
    let mut digest = Sha256::new();
    for part in parts {
        digest.update((part.len() as u64).to_be_bytes());
        digest.update(part.as_bytes());
    }
    format!("{:x}", digest.finalize())
}

fn lease_attempt_idempotency_key(operation_id: &str) -> Result<String, Problem> {
    let mut nonce = [0_u8; 16];
    getrandom::fill(&mut nonce).map_err(|_| {
        problem(
            "lease_attempt_id_unavailable",
            "Lease attempt could not be initialized",
            503,
        )
    })?;
    Ok(format!(
        "migration-sync-lease-{operation_id}-{}",
        hex_encode(nonce)
    ))
}

struct SyncImportIdempotencyLock<'a> {
    state: &'a ApiState,
    lock_id: i64,
}

impl<'a> SyncImportIdempotencyLock<'a> {
    fn acquire(state: &'a ApiState, store_key: &str) -> Result<Self, Problem> {
        let namespace = "migration-sync-idempotency-lock-v1";
        let mut digest = Sha256::new();
        digest.update((namespace.len() as u64).to_be_bytes());
        digest.update(namespace.as_bytes());
        digest.update((store_key.len() as u64).to_be_bytes());
        digest.update(store_key.as_bytes());
        let digest = digest.finalize();
        let mut lock_bytes = [0_u8; 8];
        lock_bytes.copy_from_slice(&digest[..8]);
        let lock_id = i64::from_be_bytes(lock_bytes);
        if !state.control_persistence.try_advisory_lock(lock_id)? {
            return Err(problem(
                "migration_sync_import_in_progress",
                "Sync migration import is already in progress",
                409,
            ));
        }
        Ok(Self { state, lock_id })
    }
}

impl Drop for SyncImportIdempotencyLock<'_> {
    fn drop(&mut self) {
        let _ = self
            .state
            .control_persistence
            .release_advisory_lock(self.lock_id);
    }
}

fn lease_heartbeat_idempotency_key(
    operation_id: &str,
    owner_token: &str,
    sequence: &str,
) -> String {
    format!(
        "lease-renew-{}",
        sha256_parts(&[operation_id, "heartbeat", owner_token, sequence])
    )
}

fn next_lease_heartbeat_sequence(sequence: &mut u64) -> Result<String, Problem> {
    let next = sequence.checked_add(1).ok_or_else(|| {
        Problem::new(
            "sync_lease_sequence_exhausted",
            "Sync lease heartbeat sequence is exhausted",
            409,
            "sync-import",
        )
    })?;
    let current = *sequence;
    *sequence = next;
    Ok(current.to_string())
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        },
        time::Duration,
    };

    use super::{
        SyncImportIdempotencyLock, SyncImportInput, journal_objects, lease_attempt_idempotency_key,
        lease_heartbeat_idempotency_key, next_lease_heartbeat_sequence, spawn_lease_heartbeat,
        sync_import_hashes, sync_object_storage_key,
    };

    #[test]
    fn sync_import_idempotency_lock_serializes_same_key_and_releases_on_drop() {
        let state = ApiState::new(ProfileService::disabled());
        let first = SyncImportIdempotencyLock::acquire(&state, "migration-sync:test-key").unwrap();
        let concurrent = SyncImportIdempotencyLock::acquire(&state, "migration-sync:test-key")
            .err()
            .expect("concurrent request must be rejected");
        assert_eq!(concurrent.code, "migration_sync_import_in_progress");
        drop(first);

        assert!(SyncImportIdempotencyLock::acquire(&state, "migration-sync:test-key").is_ok());
    }

    #[test]
    fn sync_import_idempotency_lock_does_not_serialize_distinct_keys() {
        let state = ApiState::new(ProfileService::disabled());
        let first = SyncImportIdempotencyLock::acquire(&state, "migration-sync:key-a").unwrap();
        let second = SyncImportIdempotencyLock::acquire(&state, "migration-sync:key-b");
        assert!(second.is_ok());
        drop(second);
        drop(first);
    }

    #[test]
    fn lease_heartbeat_keys_are_stable_per_lease_and_distinct_across_reacquire() {
        let first = lease_heartbeat_idempotency_key("import-1", "owner-token-1", "0");
        assert_eq!(
            first,
            lease_heartbeat_idempotency_key("import-1", "owner-token-1", "0")
        );
        assert_ne!(
            first,
            lease_heartbeat_idempotency_key("import-1", "owner-token-2", "0")
        );
        assert_ne!(
            first,
            lease_heartbeat_idempotency_key("import-1", "owner-token-1", "1")
        );
    }

    #[test]
    fn lease_heartbeat_sequence_fails_closed_at_exhaustion() {
        let mut sequence = u64::MAX;
        let error = next_lease_heartbeat_sequence(&mut sequence).unwrap_err();
        assert_eq!(error.code, "sync_lease_sequence_exhausted");
        assert_eq!(sequence, u64::MAX);
    }
    use crate::http::{ApiState, SyncMigrationJournal, SyncMigrationJournalState};
    use axum::http::StatusCode;
    use buglogin_browser::profile::ProfileService;
    use buglogin_domain::ids::{ResourceId, WorkspaceId};
    use buglogin_storage::{
        control::MemoryControlRecordStore,
        remote::{ConcurrentRemoteChunkStore, MemoryRemoteChunkStore, RemoteChunk},
    };
    use buglogin_sync::{
        export::{SyncExport, SyncExportBlob, SyncRevision, canonical_manifest_checksum},
        import::{SyncImportPlan, SyncStageReceipt, SyncStagedObject, plan_sync_import},
        manifest::{ManifestObject, SyncManifest},
    };
    use sha2::{Digest, Sha256};

    #[test]
    fn journal_objects_index_sizes_and_reject_staged_objects_missing_from_plan() {
        let workspace = WorkspaceId::new("ws_journal_sizes").unwrap();
        let resource = ResourceId::new("res_journal_sizes").unwrap();
        let first_bytes = b"first-object";
        let second_bytes = b"second-object-with-different-size";
        let first = SyncExportBlob {
            key: "profile/first".into(),
            size: first_bytes.len() as u64,
            checksum: format!("{:x}", Sha256::digest(first_bytes)),
            data: first_bytes.to_vec(),
        };
        let second = SyncExportBlob {
            key: "profile/second".into(),
            size: second_bytes.len() as u64,
            checksum: format!("{:x}", Sha256::digest(second_bytes)),
            data: second_bytes.to_vec(),
        };
        let manifest = SyncManifest::new(
            workspace,
            resource,
            1,
            1,
            None,
            b"root",
            vec![
                ManifestObject::new(first.key.clone(), first_bytes).unwrap(),
                ManifestObject::new(second.key.clone(), second_bytes).unwrap(),
            ],
        )
        .unwrap();
        let plan = SyncImportPlan {
            export_id: "journal-size-test".into(),
            replayed: false,
            manifest,
            blobs: vec![first.clone(), second.clone()],
        };
        let staged = SyncStageReceipt {
            objects: vec![
                SyncStagedObject {
                    storage_key: sync_object_storage_key(&plan.manifest, &second),
                    checksum: second.checksum.clone(),
                    created: false,
                },
                SyncStagedObject {
                    storage_key: sync_object_storage_key(&plan.manifest, &first),
                    checksum: first.checksum.clone(),
                    created: true,
                },
            ],
        };

        let (created, resumed) = journal_objects(&plan, &staged).unwrap();
        assert_eq!(created.len(), 1);
        assert_eq!(created[0].size, first.size);
        assert_eq!(resumed.len(), 1);
        assert_eq!(resumed[0].size, second.size);

        let missing = SyncStageReceipt {
            objects: vec![SyncStagedObject {
                storage_key: "profiles/unplanned/object".into(),
                checksum: "a".repeat(64),
                created: true,
            }],
        };
        let error = journal_objects(&plan, &missing).unwrap_err();
        assert_eq!(error.code, "sync_import_staged_object_missing");
    }

    #[test]
    fn sync_import_rejects_unknown_fields_at_every_request_boundary() {
        let workspace = WorkspaceId::new("ws_strict_import").unwrap();
        let resource = ResourceId::new("res_strict_import").unwrap();
        let bytes = b"encrypted-object";
        let manifest = SyncManifest::new(
            workspace,
            resource,
            7,
            1,
            None,
            b"root",
            vec![ManifestObject::new("profile/state", bytes).unwrap()],
        )
        .unwrap();
        let export = SyncExport::new(
            "strict-import-export",
            "source-instance",
            manifest.clone(),
            None,
            vec![SyncRevision {
                generation: 1,
                parent_generation: None,
                manifest_sha256: canonical_manifest_checksum(&manifest).unwrap(),
            }],
            vec![SyncExportBlob {
                key: "profile/state".into(),
                size: bytes.len() as u64,
                checksum: format!("{:x}", Sha256::digest(bytes)),
                data: bytes.to_vec(),
            }],
            1,
            "0.1.0",
        )
        .unwrap();
        let input = SyncImportInput {
            export,
            dry_run: false,
        };
        let (request_hash, export_hash) = sync_import_hashes(&input).unwrap();
        assert_eq!(
            request_hash,
            format!("{:x}", Sha256::digest(serde_json::to_vec(&input).unwrap()))
        );
        assert_eq!(
            export_hash,
            format!(
                "{:x}",
                Sha256::digest(serde_json::to_vec(&input.export).unwrap())
            )
        );
        let mut dry_run_input = input.clone();
        dry_run_input.dry_run = true;
        let (dry_run_request_hash, dry_run_export_hash) =
            sync_import_hashes(&dry_run_input).unwrap();
        assert_eq!(
            dry_run_request_hash,
            format!(
                "{:x}",
                Sha256::digest(serde_json::to_vec(&dry_run_input).unwrap())
            )
        );
        assert_eq!(dry_run_export_hash, export_hash);
        let valid = serde_json::to_value(input).unwrap();

        let mut without_dry_run = valid.clone();
        without_dry_run.as_object_mut().unwrap().remove("dryRun");
        assert!(serde_json::from_value::<SyncImportInput>(without_dry_run).is_ok());

        let mut cases = Vec::new();
        let mut top_level = valid.clone();
        top_level["unexpected"] = serde_json::json!(true);
        cases.push(top_level);
        let mut export = valid.clone();
        export["export"]["unexpected"] = serde_json::json!(true);
        cases.push(export);
        let mut manifest = valid.clone();
        manifest["export"]["manifest"]["unexpected"] = serde_json::json!(true);
        cases.push(manifest);
        let mut manifest_object = valid.clone();
        manifest_object["export"]["manifest"]["objects"][0]["unexpected"] = serde_json::json!(true);
        cases.push(manifest_object);
        let mut tombstone = valid.clone();
        tombstone["export"]["manifest"]["tombstones"] = serde_json::json!([{
            "key": "profile/old",
            "deletedGeneration": 1,
            "deletedAtUnixMs": 1,
            "retentionDeadlineUnixMs": 2,
            "unexpected": true
        }]);
        cases.push(tombstone);
        let mut dependency = valid.clone();
        dependency["export"]["manifest"]["dependencies"] = serde_json::json!([{
            "kind": "proxy",
            "proxyId": "proxy_strict",
            "revision": 1,
            "metadataObject": null,
            "credentialEnvelopeObject": null,
            "unexpected": true
        }]);
        cases.push(dependency);
        let mut encryption = valid.clone();
        encryption["export"]["manifest"]["payloadEncryption"] = serde_json::json!({
            "schemaVersion": 1,
            "algorithm": "aes-256-gcm-chunk-v1",
            "resourceKeyEnvelope": {
                "key": ".buglogin/resource-key-envelope.json",
                "size": 1,
                "checksum": "a".repeat(64)
            },
            "unexpected": true
        });
        cases.push(encryption);
        let mut blob = valid.clone();
        blob["export"]["blobs"][0]["unexpected"] = serde_json::json!(true);
        cases.push(blob);
        let mut revision = valid;
        revision["export"]["revisions"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "generation": 1,
                "parentGeneration": null,
                "manifestSha256": "a".repeat(64),
                "unexpected": true
            }));
        cases.push(revision);

        for value in cases {
            assert!(
                serde_json::from_value::<SyncImportInput>(value).is_err(),
                "unknown request fields must be rejected recursively"
            );
        }
    }

    #[tokio::test]
    async fn sync_import_body_limit_returns_canonical_problem() {
        let error = super::parse_sync_import_input(axum::body::Body::from("{\"export\":null}"), 4)
            .await
            .unwrap_err();
        assert_eq!(error.status(), StatusCode::PAYLOAD_TOO_LARGE);
        let bytes = axum::body::to_bytes(error.into_body(), 1024).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["code"], "sync_import_request_too_large");
        assert_eq!(body["fields"]["maxBytes"], 4);
    }

    fn valid_sync_import_body() -> Vec<u8> {
        let workspace = WorkspaceId::new("ws_bounded_import").unwrap();
        let resource = ResourceId::new("res_bounded_import").unwrap();
        let object_bytes = b"encrypted-object";
        let manifest = SyncManifest::new(
            workspace,
            resource,
            7,
            1,
            None,
            b"root",
            vec![ManifestObject::new("profile/state", object_bytes).unwrap()],
        )
        .unwrap();
        let export = SyncExport::new(
            "bounded-import-export",
            "source-instance",
            manifest.clone(),
            None,
            vec![SyncRevision {
                generation: 1,
                parent_generation: None,
                manifest_sha256: canonical_manifest_checksum(&manifest).unwrap(),
            }],
            vec![SyncExportBlob {
                key: "profile/state".into(),
                size: object_bytes.len() as u64,
                checksum: format!("{:x}", Sha256::digest(object_bytes)),
                data: object_bytes.to_vec(),
            }],
            1,
            "0.1.0",
        )
        .unwrap();
        serde_json::to_vec(&SyncImportInput {
            export,
            dry_run: false,
        })
        .unwrap()
    }

    #[tokio::test]
    async fn sync_import_parser_consumes_one_document_within_byte_limit() {
        let valid = valid_sync_import_body();
        let exact_limit =
            super::parse_sync_import_input(axum::body::Body::from(valid.clone()), valid.len())
                .await;
        assert!(
            exact_limit.is_ok(),
            "a valid body exactly at the cap is accepted"
        );

        let mut second_document = valid.clone();
        second_document.extend_from_slice(b" {} ");
        let trailing_document_error = super::parse_sync_import_input(
            axum::body::Body::from(second_document.clone()),
            second_document.len(),
        )
        .await
        .unwrap_err();
        assert_eq!(
            trailing_document_error.status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );

        let mut oversized_whitespace = valid.clone();
        oversized_whitespace.extend_from_slice(b"  ");
        let oversized_error = super::parse_sync_import_input(
            axum::body::Body::from(oversized_whitespace),
            valid.len() + 1,
        )
        .await
        .unwrap_err();
        assert_eq!(oversized_error.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[test]
    fn direct_sync_import_admission_is_bounded_and_retryable() {
        let first = super::sync_import_request_slot().unwrap();
        let error = super::sync_import_request_slot().unwrap_err();
        assert_eq!(error.status(), StatusCode::SERVICE_UNAVAILABLE);
        drop(first);
        assert!(super::sync_import_request_slot().is_ok());
    }

    #[tokio::test]
    async fn lease_heartbeat_renews_periodically_and_stops_cleanly() {
        let renewals = Arc::new(AtomicUsize::new(0));
        let renewal_signals = Arc::new(tokio::sync::Semaphore::new(0));
        let heartbeat_renewals = Arc::clone(&renewals);
        let heartbeat_signals = Arc::clone(&renewal_signals);
        let heartbeat = spawn_lease_heartbeat(
            move || {
                let renewals = Arc::clone(&heartbeat_renewals);
                let signals = Arc::clone(&heartbeat_signals);
                async move {
                    renewals.fetch_add(1, Ordering::SeqCst);
                    signals.add_permits(1);
                    Ok(())
                }
            },
            Duration::from_millis(10),
        );

        tokio::time::timeout(Duration::from_secs(2), renewal_signals.acquire_many(2))
            .await
            .expect("heartbeat should renew the lease at least twice")
            .expect("heartbeat renewal signals should remain open")
            .forget();
        heartbeat.stop().await;
        let stopped_at = renewals.load(Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(20)).await;

        assert!(stopped_at >= 2);
        assert_eq!(renewals.load(Ordering::SeqCst), stopped_at);
    }

    #[tokio::test]
    async fn lease_heartbeat_stops_when_authority_renewal_fails() {
        let (failed_tx, failed_rx) = tokio::sync::oneshot::channel();
        let mut failed_tx = Some(failed_tx);
        let heartbeat = spawn_lease_heartbeat(
            move || {
                if let Some(failed_tx) = failed_tx.take() {
                    let _ = failed_tx.send(());
                }
                async {
                    Err(buglogin_domain::errors::Problem::new(
                        "test_lease_lost",
                        "Lease renewal failed",
                        503,
                        "test",
                    ))
                }
            },
            Duration::from_millis(10),
        );
        let healthy = Arc::clone(&heartbeat.healthy);
        let failure = Arc::clone(&heartbeat.failure);
        tokio::time::timeout(Duration::from_secs(1), failed_rx)
            .await
            .expect("lease heartbeat should attempt renewal")
            .expect("lease heartbeat should report failed renewal");
        heartbeat.stop().await;

        assert!(!healthy.load(Ordering::SeqCst));
        assert_eq!(
            failure.lock().unwrap().as_ref().unwrap().code,
            "test_lease_lost"
        );
    }

    #[derive(Clone)]
    struct AmbiguousPutStore {
        inner: MemoryRemoteChunkStore,
        fail_next_put_after_write: Arc<AtomicBool>,
        put_calls: Arc<AtomicUsize>,
    }

    impl ConcurrentRemoteChunkStore for AmbiguousPutStore {
        fn readiness(&self) -> Result<(), buglogin_domain::errors::Problem> {
            ConcurrentRemoteChunkStore::readiness(&self.inner)
        }

        fn checksum(
            &self,
            object_key: &str,
        ) -> Result<Option<String>, buglogin_domain::errors::Problem> {
            ConcurrentRemoteChunkStore::checksum(&self.inner, object_key)
        }

        fn put(
            &self,
            object_key: &str,
            bytes: &[u8],
            expected_checksum: &str,
        ) -> Result<(), buglogin_domain::errors::Problem> {
            self.put_calls.fetch_add(1, Ordering::SeqCst);
            ConcurrentRemoteChunkStore::put(&self.inner, object_key, bytes, expected_checksum)?;
            if self.fail_next_put_after_write.swap(false, Ordering::SeqCst) {
                return Err(buglogin_domain::errors::Problem::new(
                    "remote_timeout_after_write",
                    "Remote write result was interrupted",
                    503,
                    "test",
                )
                .retryable(true));
            }
            Ok(())
        }

        fn get(
            &self,
            object_key: &str,
            expected_checksum: &str,
        ) -> Result<RemoteChunk, buglogin_domain::errors::Problem> {
            ConcurrentRemoteChunkStore::get(&self.inner, object_key, expected_checksum)
        }

        fn delete_if_checksum(
            &self,
            object_key: &str,
            expected_checksum: &str,
        ) -> Result<bool, buglogin_domain::errors::Problem> {
            ConcurrentRemoteChunkStore::delete_if_checksum(
                &self.inner,
                object_key,
                expected_checksum,
            )
        }
    }

    #[test]
    fn staging_checkpoint_resumes_ambiguous_put_after_state_reopen() {
        use buglogin_domain::{
            ids::{AccountId, OrganizationId},
            organization::{Organization, OrganizationPolicy},
            permissions::{Principal, Role},
            workspace::{Workspace, WorkspacePolicy},
        };
        let workspace = WorkspaceId::new("ws_checkpoint").unwrap();
        let resource = ResourceId::new("res_checkpoint").unwrap();
        let bytes = b"encrypted-profile-object";
        let manifest = SyncManifest::new(
            workspace.clone(),
            resource.clone(),
            8,
            1,
            None,
            b"checkpoint-root",
            vec![ManifestObject::new("profile/state", bytes).unwrap()],
        )
        .unwrap();
        let export = SyncExport::new(
            "checkpoint-export-0001",
            "legacy-production",
            manifest.clone(),
            None,
            vec![SyncRevision {
                generation: 1,
                parent_generation: None,
                manifest_sha256: canonical_manifest_checksum(&manifest).unwrap(),
            }],
            vec![SyncExportBlob {
                key: "profile/state".into(),
                size: bytes.len() as u64,
                checksum: format!("{:x}", Sha256::digest(bytes)),
                data: bytes.to_vec(),
            }],
            1_753_142_400_000,
            "0.1.0",
        )
        .unwrap();
        let plan = plan_sync_import(&export, None, 41).unwrap();
        let control_store = MemoryControlRecordStore::default();
        let put_calls = Arc::new(AtomicUsize::new(0));
        let remote = AmbiguousPutStore {
            inner: MemoryRemoteChunkStore::default(),
            fail_next_put_after_write: Arc::new(AtomicBool::new(true)),
            put_calls: Arc::clone(&put_calls),
        };
        let state =
            ApiState::hosted_with_control_store(ProfileService::disabled(), control_store.clone())
                .unwrap()
                .with_concurrent_remote_chunks(Box::new(remote.clone()));
        let organization = Organization {
            id: OrganizationId::new("org_checkpoint").unwrap(),
            account_id: AccountId::new("acct_checkpoint").unwrap(),
            name: "Checkpoint test".into(),
            created_at: "2026-10-16T00:00:00Z".into(),
        };
        let mut workspaces = crate::services::workspaces::WorkspaceService::new(
            organization.clone(),
            OrganizationPolicy {
                max_workspaces: 1,
                offline_entitled: false,
                default_extension_groups_by_workspace: Default::default(),
            },
        )
        .unwrap();
        workspaces
            .create_workspace(
                &Principal {
                    roles: [Role::PlatformAdmin].into_iter().collect(),
                    ..Principal::default()
                },
                &[],
                &organization.id,
                Workspace {
                    id: workspace.clone(),
                    organization_id: organization.id.clone(),
                    name: "Checkpoint test".into(),
                    policy: WorkspacePolicy {
                        max_profiles: 10,
                        max_members: 10,
                        max_storage_bytes: 1024 * 1024,
                        offline_entitled: false,
                    },
                    created_at: "2026-10-16T00:00:00Z".into(),
                    default_extension_group_id: None,
                },
            )
            .unwrap();
        *state.workspaces.write().unwrap() = workspaces;
        let mut journal = SyncMigrationJournal {
            schema: "buglogin.migration-sync-journal/1".into(),
            operation_id: "op_checkpoint".into(),
            export_id: export.export_id.clone(),
            export_sha256: "a".repeat(64),
            manifest_sha256: export.manifest_sha256.clone(),
            blob_set_sha256: export.blob_set_sha256.clone(),
            workspace_id: workspace.as_str().into(),
            resource_id: resource.as_str().into(),
            base_generation: None,
            target_generation: 1,
            state: SyncMigrationJournalState::Staging,
            created_objects: Vec::new(),
            resumed_objects: Vec::new(),
            reserved_objects: Vec::new(),
            destination_fencing_token: None,
            control_revision_before: 0,
            control_revision_after: None,
            updated_at_ms: 1,
            error_code: None,
        };
        let journal_id = "msi_checkpoint";
        let revision = state
            .persist_sync_migration_journal(
                journal_id,
                0,
                "checkpoint-journal-initial".into(),
                "acct_checkpoint".into(),
                &journal,
            )
            .unwrap();

        let (error, revision) = super::stage_import_objects(
            &state,
            &plan,
            journal_id,
            revision,
            "acct_checkpoint",
            &mut journal,
        )
        .unwrap_err();
        assert!(error.retryable);
        assert_eq!(journal.created_objects.len(), 1);
        assert_eq!(put_calls.load(Ordering::SeqCst), 1);
        super::persist_stage_failure(
            &state,
            journal_id,
            revision,
            "acct_checkpoint",
            &mut journal,
            error,
        );

        let reopened =
            ApiState::hosted_with_control_store(ProfileService::disabled(), control_store.reopen())
                .unwrap()
                .with_concurrent_remote_chunks(Box::new(remote));
        let (mut recovered_journal, recovered_revision) = reopened
            .load_sync_migration_journal(journal_id)
            .unwrap()
            .unwrap();
        let (receipt, _) = super::stage_import_objects(
            &reopened,
            &plan,
            journal_id,
            recovered_revision,
            "acct_checkpoint",
            &mut recovered_journal,
        )
        .unwrap();

        assert_eq!(receipt.objects.len(), 1);
        assert!(receipt.objects[0].created);
        assert_eq!(put_calls.load(Ordering::SeqCst), 1);
        assert_eq!(recovered_journal.created_objects.len(), 1);
        assert_eq!(recovered_journal.state, SyncMigrationJournalState::Staging);
    }

    #[test]
    fn failed_non_retryable_import_plan_is_journaled_for_cleanup() {
        let store = MemoryControlRecordStore::default();
        let state = ApiState::hosted_with_control_store(ProfileService::disabled(), store).unwrap();
        let mut journal = SyncMigrationJournal {
            schema: "buglogin.migration-sync-journal/1".into(),
            operation_id: "op_plan_failure".into(),
            export_id: "export_plan_failure".into(),
            export_sha256: "a".repeat(64),
            manifest_sha256: "b".repeat(64),
            blob_set_sha256: "c".repeat(64),
            workspace_id: "ws_plan_failure".into(),
            resource_id: "res_plan_failure".into(),
            base_generation: None,
            target_generation: 1,
            state: SyncMigrationJournalState::Staging,
            created_objects: Vec::new(),
            resumed_objects: Vec::new(),
            reserved_objects: Vec::new(),
            destination_fencing_token: None,
            control_revision_before: 0,
            control_revision_after: None,
            updated_at_ms: 1,
            error_code: None,
        };
        let journal_id = "msi_plan_failure";
        let revision = state
            .persist_sync_migration_journal(
                journal_id,
                0,
                "plan-failure-initial".into(),
                "acct_plan_failure".into(),
                &journal,
            )
            .unwrap();
        let error = buglogin_domain::errors::Problem::new(
            "sync_generation_conflict",
            "Sync generation conflicts with destination",
            409,
            "test",
        );

        super::persist_stage_failure(
            &state,
            journal_id,
            revision,
            "acct_plan_failure",
            &mut journal,
            error,
        );

        let (persisted, _) = state
            .load_sync_migration_journal(journal_id)
            .unwrap()
            .unwrap();
        assert_eq!(persisted.state, SyncMigrationJournalState::CleanupPending);
        assert_eq!(
            persisted.error_code.as_deref(),
            Some("sync_generation_conflict")
        );
    }

    #[test]
    fn lease_retry_gets_a_fresh_coordination_idempotency_key() {
        let first = lease_attempt_idempotency_key("op_0123456789abcdef0123456789abcdef").unwrap();
        let retry = lease_attempt_idempotency_key("op_0123456789abcdef0123456789abcdef").unwrap();

        assert_ne!(first, retry);
        assert!(first.starts_with("migration-sync-lease-op_"));
        assert!(retry.starts_with("migration-sync-lease-op_"));
        assert!(first.len() < 256);
        assert!(retry.len() < 256);
    }
}
