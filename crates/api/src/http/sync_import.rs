use std::time::Duration;

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use buglogin_domain::{
    permissions::{Action, Capability, ResourceScope, is_allowed},
    profile::ProfileArchiveState,
};
use buglogin_storage::control::{
    ClaimControlCommand, ControlIdempotencyClaim, ControlIdempotencyClaimToken,
};
use buglogin_sync::{
    coordination::{CoordinationAuth, CoordinationBackend, CoordinationScope},
    export::SyncExport,
    import::{SyncImportPlan, plan_sync_import, sync_object_storage_key},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    ApiState, SyncMigrationJournal, SyncMigrationJournalObject, SyncMigrationJournalState,
    authenticate, grants, now, problem, response,
};

const JOURNAL_SCHEMA: &str = "buglogin.migration-sync-journal/1";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct SyncImportInput {
    pub(super) export: SyncExport,
    #[serde(default)]
    pub(super) dry_run: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
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
    headers: HeaderMap,
    Json(input): Json<SyncImportInput>,
) -> Result<Response, Response> {
    let principal = authenticate(&state, &headers)?;
    let receipt = import_sync_authorized(&state, &headers, &principal, input).await?;
    Ok((StatusCode::OK, Json(receipt)).into_response())
}

pub(super) async fn import_sync_authorized(
    state: &ApiState,
    headers: &HeaderMap,
    principal: &buglogin_domain::permissions::Principal,
    input: SyncImportInput,
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
    authorize(&state, &principal, &input.export)?;
    verify_binding(&state, &input.export)?;
    let idempotency_key = required_idempotency(&headers)?;
    let request_hash = sha256_json(&input)?;
    let export_hash = sha256_json(&input.export)?;
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

    let current = state
        .current_sync_manifest(&input.export.workspace_id, &input.export.resource_id)
        .map_err(response)?;
    if input.dry_run {
        let fencing_token = current
            .as_ref()
            .map_or(1, |manifest| manifest.fencing_token.saturating_add(1));
        let plan =
            plan_sync_import(&input.export, current.as_ref(), fencing_token).map_err(response)?;
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
                    SyncMigrationJournalState::Active
                        | SyncMigrationJournalState::CleanupPending
                        | SyncMigrationJournalState::Abandoned
                )
            {
                return Err(response(problem(
                    "migration_sync_journal_conflict",
                    "Sync migration journal conflicts with request",
                    409,
                )));
            }
            (journal, revision)
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
                destination_fencing_token: None,
                control_revision_before,
                control_revision_after: None,
                updated_at_ms: now().saturating_mul(1_000),
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

    let auth =
        CoordinationAuth::new(actor_id.clone(), bearer_token(&headers)?).map_err(response)?;
    let backend = state.coordination().map_err(response)?;
    let lease = backend
        .acquire_lease_idempotent(
            CoordinationScope {
                workspace_id: &input.export.workspace_id,
                auth: &auth,
            },
            &input.export.resource_id,
            Duration::from_secs(300),
            idempotency_key,
        )
        .await
        .map_err(response)?;
    let plan = match plan_sync_import(&input.export, current.as_ref(), lease.fencing_token) {
        Ok(plan) => plan,
        Err(error) => {
            release(&*backend, &input.export, &auth, &lease.owner_token).await;
            return Err(response(error));
        }
    };
    let staged = match state.stage_sync_import(&plan) {
        Ok(staged) => staged,
        Err(error) => {
            journal.state = SyncMigrationJournalState::CleanupPending;
            journal.error_code = Some(error.code.clone());
            journal.updated_at_ms = now().saturating_mul(1_000);
            let _ = state.persist_sync_migration_journal(
                &journal_id,
                journal_revision,
                journal_write_key(&journal_id, "cleanup", journal_revision),
                actor_id.clone(),
                &journal,
            );
            release(&*backend, &input.export, &auth, &lease.owner_token).await;
            return Err(response(error));
        }
    };
    if let Err(error) = verify_staged(&state, &staged) {
        let _ = state.rollback_sync_import(&staged);
        release(&*backend, &input.export, &auth, &lease.owner_token).await;
        return Err(response(error));
    }

    let (created_objects, resumed_objects) = journal_objects(&plan, &staged);
    journal.state = SyncMigrationJournalState::Finalizing;
    journal.created_objects = created_objects;
    journal.resumed_objects = resumed_objects;
    journal.destination_fencing_token = Some(lease.fencing_token);
    journal.updated_at_ms = now().saturating_mul(1_000);
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
            let _ = state.rollback_sync_import(&staged);
            release(&*backend, &input.export, &auth, &lease.owner_token).await;
            return Err(response(error));
        }
    };

    let finalize_result = {
        let _snapshot = std::sync::Arc::clone(&state.control_snapshot_gate)
            .write_owned()
            .await;
        let _mutation = state.control_mutation().lock().map_err(|_| {
            response(problem(
                "control_unavailable",
                "Control state unavailable",
                503,
            ))
        })?;
        let mut target = state.control_aggregate().map_err(response)?;
        let key = format!(
            "{}:{}",
            input.export.workspace_id.as_str(),
            input.export.resource_id.as_str()
        );
        let current = target.sync_manifests.get(&key).cloned();
        let final_plan = plan_sync_import(&input.export, current.as_ref(), lease.fencing_token)
            .map_err(response)?;
        target
            .sync_manifests
            .insert(key, final_plan.manifest.clone());
        journal.state = SyncMigrationJournalState::Active;
        journal.control_revision_before = state.control_revision().map_err(response)?;
        journal.control_revision_after = Some(journal.control_revision_before.saturating_add(1));
        journal.updated_at_ms = now().saturating_mul(1_000);
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
        let body = serde_json::to_value(&receipt).map_err(|_| {
            response(problem(
                "serialization_failed",
                "Response serialization failed",
                500,
            ))
        })?;
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
    };

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

fn verify_binding(state: &ApiState, export: &SyncExport) -> Result<(), Response> {
    let resource_exists = state
        .resources
        .read()
        .map_err(|_| {
            response(problem(
                "resource_unavailable",
                "Resource state unavailable",
                503,
            ))
        })?
        .exists_in_workspace(&export.resource_id, &export.workspace_id);
    let binding_exists = state
        .profile_resources
        .read()
        .map_err(|_| {
            response(problem(
                "binding_unavailable",
                "Profile binding unavailable",
                503,
            ))
        })?
        .values()
        .any(|binding| {
            binding.workspace_id == export.workspace_id
                && binding.resource_id == export.resource_id
                && binding.archive_state == ProfileArchiveState::Active
        });
    if resource_exists && binding_exists {
        Ok(())
    } else {
        Err(response(problem(
            "migration_sync_binding_not_found",
            "Active profile resource binding not found",
            404,
        )))
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
) -> (
    Vec<SyncMigrationJournalObject>,
    Vec<SyncMigrationJournalObject>,
) {
    let mut created = vec![];
    let mut resumed = vec![];
    for object in &staged.objects {
        let size = plan
            .blobs
            .iter()
            .find(|blob| sync_object_storage_key(&plan.manifest, blob) == object.storage_key)
            .map_or(0, |blob| blob.size);
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
    (created, resumed)
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

fn bearer_token(headers: &HeaderMap) -> Result<String, Response> {
    headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::to_owned)
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
    let _ = backend
        .release_lease(
            CoordinationScope {
                workspace_id: &export.workspace_id,
                auth,
            },
            &export.resource_id,
            owner_token,
        )
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

fn sha256_json(value: &impl Serialize) -> Result<String, Response> {
    serde_json::to_vec(value)
        .map(|bytes| format!("{:x}", Sha256::digest(bytes)))
        .map_err(|_| response(problem("invalid_input", "Input is invalid", 400)))
}

fn sha256_parts(parts: &[&str]) -> String {
    let mut digest = Sha256::new();
    for part in parts {
        digest.update((part.len() as u64).to_be_bytes());
        digest.update(part.as_bytes());
    }
    format!("{:x}", digest.finalize())
}
