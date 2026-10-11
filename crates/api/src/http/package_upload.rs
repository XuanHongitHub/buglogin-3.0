use std::collections::BTreeMap;

use axum::{
    Json,
    body::Bytes,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use buglogin_domain::{errors::Problem, permissions::Action};
use buglogin_migration::{
    package::NeutralPackage, signature::verify_signed_package, verify::validate_relative_path,
};
use buglogin_storage::{
    blob::checksum,
    control::{ControlAuditEntry, ControlPut, ExecuteControlCommand, HttpResponse},
};
use buglogin_sync::export::SyncExport;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use super::control_import::validate_package_id;
use super::{
    ApiState, CONTROL_TENANT, authenticate, authorize_capability, control_import, grants, now,
    problem, response, sync_import,
};

const JOURNAL_TYPE: &str = "migration_package_upload";
const JOURNAL_SCHEMA: &str = "buglogin.migration-package-upload/1";
const MAX_FILE_BYTES: usize = 64 * 1024 * 1024;
const MAX_PACKAGE_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PackageFileQuery {
    path: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct StagedFile {
    path: String,
    checksum: String,
    size: u64,
    storage_key: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum UploadState {
    Staging,
    Verified,
    Active,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct UploadJournal {
    schema: String,
    package_id: String,
    actor_id: String,
    state: UploadState,
    files: BTreeMap<String, StagedFile>,
    checksums_sha256: Option<String>,
    #[serde(default)]
    activation_receipt: Option<serde_json::Value>,
    updated_at_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PackageFileReceipt {
    package_id: String,
    path: String,
    checksum: String,
    size: u64,
    state: &'static str,
    replayed: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PackageVerifyReceipt {
    package_id: String,
    checksums_sha256: String,
    file_count: usize,
    total_bytes: u64,
    has_control: bool,
    has_sync: bool,
    state: &'static str,
    replayed: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PackageActivationReceipt {
    package_id: String,
    state: &'static str,
    replayed: bool,
    control_import: Option<serde_json::Value>,
    sync_import: Option<serde_json::Value>,
}

pub(super) async fn put_package_file(
    State(state): State<ApiState>,
    Path(package_id): Path<String>,
    Query(query): Query<PackageFileQuery>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, Response> {
    let principal = authenticate(&state, &headers)?;
    authorize_capability(
        &principal,
        &grants(&state)?,
        "migration.write",
        Action::Execute,
    )?;
    validate_package_id(&package_id).map_err(response)?;
    validate_relative_path(&query.path).map_err(response)?;
    if query.path.len() > 256 || query.path.bytes().any(|byte| byte.is_ascii_control()) {
        return Err(response(problem(
            "migration_package_path_invalid",
            "Migration package path is invalid",
            400,
        )));
    }
    if body.len() > MAX_FILE_BYTES {
        return Err(response(problem(
            "migration_package_file_too_large",
            "Migration package file exceeds upload limit",
            413,
        )));
    }
    let expected_checksum = required_checksum(&headers)?;
    if checksum(&body) != expected_checksum {
        return Err(response(problem(
            "migration_checksum_mismatch",
            "Migration checksum mismatch",
            409,
        )));
    }
    let idempotency_key = required_idempotency(&headers)?;
    let actor_id = actor_id(&principal)?;
    let journal_id = journal_id(&actor_id, &package_id);
    let storage_key = storage_key(&journal_id, &query.path);
    let receipt = PackageFileReceipt {
        package_id: package_id.clone(),
        path: query.path.clone(),
        checksum: expected_checksum.clone(),
        size: body.len() as u64,
        state: "staged",
        replayed: false,
    };

    let _mutation = state.control_mutation().lock().map_err(|_| {
        response(problem(
            "control_unavailable",
            "Control state unavailable",
            503,
        ))
    })?;
    let loaded = state
        .control_persistence
        .load_versioned::<UploadJournal>(CONTROL_TENANT, JOURNAL_TYPE, &journal_id)
        .map_err(response)?;
    let (mut journal, revision) = loaded.unwrap_or_else(|| {
        (
            UploadJournal {
                schema: JOURNAL_SCHEMA.into(),
                package_id: package_id.clone(),
                actor_id: actor_id.clone(),
                state: UploadState::Staging,
                files: BTreeMap::new(),
                checksums_sha256: None,
                activation_receipt: None,
                updated_at_ms: now().saturating_mul(1_000),
            },
            0,
        )
    });
    validate_journal(&journal, &package_id, &actor_id)?;
    if journal.state != UploadState::Staging {
        return Err(response(problem(
            "migration_package_already_verified",
            "Verified migration package cannot be changed",
            409,
        )));
    }
    if let Some(existing) = journal.files.get(&query.path) {
        if existing.checksum != expected_checksum || existing.size != body.len() as u64 {
            return Err(response(problem(
                "migration_package_file_conflict",
                "Migration package file conflicts with staged content",
                409,
            )));
        }
        match state.get_remote_chunk(&existing.storage_key, &existing.checksum) {
            Ok(_) => {}
            Err(problem) if problem.code == "remote_chunk_not_found" => state
                .put_remote_chunk(&existing.storage_key, &body, &expected_checksum)
                .map_err(response)?,
            Err(problem) => return Err(response(problem)),
        }
        return Ok((
            StatusCode::OK,
            Json(PackageFileReceipt {
                replayed: true,
                ..receipt
            }),
        )
            .into_response());
    }

    journal.files.insert(
        query.path.clone(),
        StagedFile {
            path: query.path.clone(),
            checksum: expected_checksum.clone(),
            size: body.len() as u64,
            storage_key: storage_key.clone(),
        },
    );
    journal.updated_at_ms = now().saturating_mul(1_000);
    persist_journal(
        &state,
        &journal_id,
        revision,
        idempotency_key,
        &actor_id,
        "migration.package.file.stage",
        &journal,
        json!({
            "packageId": package_id,
            "path": query.path,
            "checksum": expected_checksum,
            "size": body.len(),
        }),
        &receipt,
    )?;
    state
        .put_remote_chunk(&storage_key, &body, &expected_checksum)
        .map_err(response)?;
    Ok((StatusCode::OK, Json(receipt)).into_response())
}

pub(super) async fn verify_package_upload(
    State(state): State<ApiState>,
    Path(package_id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, Response> {
    let principal = authenticate(&state, &headers)?;
    authorize_capability(
        &principal,
        &grants(&state)?,
        "migration.write",
        Action::Execute,
    )?;
    validate_package_id(&package_id).map_err(response)?;
    let idempotency_key = required_idempotency(&headers)?;
    let actor_id = actor_id(&principal)?;
    let journal_id = journal_id(&actor_id, &package_id);
    let _mutation = state.control_mutation().lock().map_err(|_| {
        response(problem(
            "control_unavailable",
            "Control state unavailable",
            503,
        ))
    })?;
    let (mut journal, revision) = state
        .control_persistence
        .load_versioned::<UploadJournal>(CONTROL_TENANT, JOURNAL_TYPE, &journal_id)
        .map_err(response)?
        .ok_or_else(|| {
            response(problem(
                "migration_package_upload_not_found",
                "Migration package upload not found",
                404,
            ))
        })?;
    validate_journal(&journal, &package_id, &actor_id)?;
    let total_bytes = journal.files.values().map(|file| file.size).sum::<u64>();
    // ponytail: streamed verification removes this in-memory ceiling before production cutover.
    if total_bytes > MAX_PACKAGE_BYTES {
        return Err(response(problem(
            "migration_package_too_large",
            "Migration package exceeds verification limit",
            413,
        )));
    }
    let package = load_package(&state, &journal)?;
    let signing_key = state.migration_signing_key.read().map_err(|_| {
        response(problem(
            "migration_signing_key_unavailable",
            "Migration signing key unavailable",
            503,
        ))
    })?;
    let signing_key = signing_key.as_ref().ok_or_else(|| {
        response(problem(
            "migration_signing_key_unavailable",
            "Migration signing key unavailable",
            503,
        ))
    })?;
    let verified = verify_signed_package(&package, signing_key.as_bytes()).map_err(response)?;
    if verified.package_id != package_id {
        return Err(response(problem(
            "migration_package_id_mismatch",
            "Migration package ID does not match upload",
            409,
        )));
    }
    let receipt = PackageVerifyReceipt {
        package_id: package_id.clone(),
        checksums_sha256: verified.checksums_sha256.clone(),
        file_count: journal.files.len(),
        total_bytes,
        has_control: package
            .files()
            .keys()
            .any(|path| path.starts_with("entities/")),
        has_sync: package.file("sync/export.json").is_some(),
        state: "verified",
        replayed: journal.state != UploadState::Staging,
    };
    if journal.state != UploadState::Staging {
        if journal.checksums_sha256.as_deref() != Some(&verified.checksums_sha256) {
            return Err(response(problem(
                "migration_package_upload_changed",
                "Verified migration package upload changed",
                409,
            )));
        }
        return Ok((StatusCode::OK, Json(receipt)).into_response());
    }
    journal.state = UploadState::Verified;
    journal.checksums_sha256 = Some(verified.checksums_sha256);
    journal.updated_at_ms = now().saturating_mul(1_000);
    persist_journal(
        &state,
        &journal_id,
        revision,
        idempotency_key,
        &actor_id,
        "migration.package.verify",
        &journal,
        json!({
            "packageId": package_id,
            "fileCount": journal.files.len(),
            "totalBytes": total_bytes,
        }),
        &receipt,
    )?;
    Ok((StatusCode::OK, Json(receipt)).into_response())
}

pub(super) async fn activate_package_upload(
    State(state): State<ApiState>,
    Path(package_id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, Response> {
    let principal = authenticate(&state, &headers)?;
    authorize_capability(
        &principal,
        &grants(&state)?,
        "migration.write",
        Action::Execute,
    )?;
    validate_package_id(&package_id).map_err(response)?;
    let idempotency_key = required_idempotency(&headers)?;
    let actor_id = actor_id(&principal)?;
    let journal_id = journal_id(&actor_id, &package_id);
    let (journal, _) = state
        .control_persistence
        .load_versioned::<UploadJournal>(CONTROL_TENANT, JOURNAL_TYPE, &journal_id)
        .map_err(response)?
        .ok_or_else(|| {
            response(problem(
                "migration_package_upload_not_found",
                "Migration package upload not found",
                404,
            ))
        })?;
    validate_journal(&journal, &package_id, &actor_id)?;
    if journal.state == UploadState::Active {
        let mut receipt = journal.activation_receipt.ok_or_else(|| {
            response(problem(
                "migration_package_activation_receipt_missing",
                "Migration package activation receipt is missing",
                500,
            ))
        })?;
        if let Some(object) = receipt.as_object_mut() {
            object.insert("replayed".into(), serde_json::Value::Bool(true));
        }
        return Ok((StatusCode::OK, Json(receipt)).into_response());
    }
    if journal.state != UploadState::Verified {
        return Err(response(problem(
            "migration_package_not_verified",
            "Migration package must be verified before activation",
            409,
        )));
    }
    let package = load_package(&state, &journal)?;
    let signing_key = {
        let signing_key = state.migration_signing_key.read().map_err(|_| {
            response(problem(
                "migration_signing_key_unavailable",
                "Migration signing key unavailable",
                503,
            ))
        })?;
        signing_key
            .as_ref()
            .map(|key| key.as_bytes().to_vec())
            .ok_or_else(|| {
                response(problem(
                    "migration_signing_key_unavailable",
                    "Migration signing key unavailable",
                    503,
                ))
            })?
    };
    let verified = verify_signed_package(&package, &signing_key).map_err(response)?;
    if verified.package_id != package_id
        || journal.checksums_sha256.as_deref() != Some(&verified.checksums_sha256)
    {
        return Err(response(problem(
            "migration_package_upload_changed",
            "Verified migration package upload changed",
            409,
        )));
    }
    let control_import = control_import::import_orchestrated_package(
        &state,
        &principal,
        &package,
        &format!("{idempotency_key}-control"),
    )
    .map_err(response)?;
    let sync_export = package
        .file("sync/export.json")
        .map(|bytes| {
            serde_json::from_slice::<SyncExport>(bytes).map_err(|_| {
                response(problem(
                    "migration_sync_export_invalid",
                    "Migration sync export is invalid",
                    422,
                ))
            })
        })
        .transpose()?;
    let sync_import = if let Some(export) = sync_export {
        match sync_import::import_sync_authorized(
            &state,
            &headers,
            &principal,
            sync_import::SyncImportInput {
                export,
                dry_run: false,
            },
        )
        .await
        {
            Ok(receipt) => Some(receipt),
            Err(error_response) => {
                let sync_error = response_problem(error_response).await;
                if sync_error.code != "migration_sync_commit_outcome_unknown" {
                    if control_import.is_some() {
                        if let Err(rollback_error) = control_import::rollback_package_control(
                            &state,
                            &package_id,
                            &format!("{idempotency_key}-control-rollback"),
                            &actor_id,
                        ) {
                            return Err(response(
                                problem(
                                    "migration_package_activation_rollback_failed",
                                    "Migration package activation rollback failed",
                                    503,
                                )
                                .retryable(true)
                                .with_field("syncError", sync_error.code)
                                .with_field("rollbackError", rollback_error.code),
                            ));
                        }
                    }
                }
                return Err(response(sync_error));
            }
        }
    } else {
        None
    };
    if control_import.is_none() && sync_import.is_none() {
        return Err(response(problem(
            "migration_package_empty",
            "Migration package contains no activatable data",
            422,
        )));
    }
    let receipt = PackageActivationReceipt {
        package_id: package_id.clone(),
        state: "active",
        replayed: control_import
            .as_ref()
            .is_none_or(|receipt| receipt.replayed)
            && sync_import.as_ref().is_none_or(|receipt| receipt.replayed),
        control_import: control_import
            .as_ref()
            .map(serde_json::to_value)
            .transpose()
            .map_err(|_| {
                response(problem(
                    "serialization_failed",
                    "Response serialization failed",
                    500,
                ))
            })?,
        sync_import: sync_import
            .as_ref()
            .map(serde_json::to_value)
            .transpose()
            .map_err(|_| {
                response(problem(
                    "serialization_failed",
                    "Response serialization failed",
                    500,
                ))
            })?,
    };
    let mut active_journal = journal;
    active_journal.state = UploadState::Active;
    active_journal.activation_receipt = Some(serde_json::to_value(&receipt).map_err(|_| {
        response(problem(
            "serialization_failed",
            "Response serialization failed",
            500,
        ))
    })?);
    active_journal.updated_at_ms = now().saturating_mul(1_000);
    let (_, revision) = state
        .control_persistence
        .load_versioned::<UploadJournal>(CONTROL_TENANT, JOURNAL_TYPE, &journal_id)
        .map_err(response)?
        .ok_or_else(|| {
            response(problem(
                "migration_package_upload_not_found",
                "Migration package upload not found",
                404,
            ))
        })?;
    persist_journal(
        &state,
        &journal_id,
        revision,
        idempotency_key,
        &actor_id,
        "migration.package.activate",
        &active_journal,
        json!({"packageId": package_id}),
        &receipt,
    )?;
    Ok((StatusCode::OK, Json(receipt)).into_response())
}

async fn response_problem(value: Response) -> Problem {
    let status = value.status().as_u16();
    match axum::body::to_bytes(value.into_body(), 64 * 1024).await {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|_| {
            problem(
                "migration_package_activation_failed",
                "Migration package activation failed",
                status,
            )
        }),
        Err(_) => problem(
            "migration_package_activation_failed",
            "Migration package activation failed",
            status,
        ),
    }
}

fn load_package(state: &ApiState, journal: &UploadJournal) -> Result<NeutralPackage, Response> {
    let mut files = BTreeMap::new();
    for file in journal.files.values() {
        let bytes = state
            .get_remote_chunk(&file.storage_key, &file.checksum)
            .map_err(response)?;
        if bytes.len() as u64 != file.size {
            return Err(response(problem(
                "migration_package_file_size_mismatch",
                "Migration package file size does not match upload",
                409,
            )));
        }
        files.insert(file.path.clone(), bytes);
    }
    Ok(NeutralPackage::from_files(files))
}

#[allow(clippy::too_many_arguments)]
fn persist_journal<T: Serialize>(
    state: &ApiState,
    journal_id: &str,
    revision: u64,
    idempotency_key: &str,
    actor_id: &str,
    command_id: &str,
    journal: &UploadJournal,
    detail: serde_json::Value,
    receipt: &T,
) -> Result<(), Response> {
    state
        .control_persistence
        .execute(ExecuteControlCommand {
            tenant_id: CONTROL_TENANT.into(),
            command_id: command_id.into(),
            idempotency_key: format!(
                "migration-package:{}",
                sha256_parts(&[journal_id, command_id, idempotency_key])
            ),
            expected_revisions: Default::default(),
            puts: vec![ControlPut {
                tenant_id: CONTROL_TENANT.into(),
                record_type: JOURNAL_TYPE.into(),
                record_id: journal_id.into(),
                expected_revision: Some(revision),
                value: serde_json::to_value(journal).map_err(|_| {
                    response(problem(
                        "migration_package_journal_invalid",
                        "Migration package journal is invalid",
                        500,
                    ))
                })?,
            }],
            deletes: vec![],
            audit: ControlAuditEntry {
                tenant_id: CONTROL_TENANT.into(),
                command_id: command_id.into(),
                actor_id: actor_id.into(),
                allowed: true,
                detail,
            },
            response: HttpResponse {
                status: 200,
                headers: Default::default(),
                body: serde_json::to_value(receipt).map_err(|_| {
                    response(problem(
                        "serialization_failed",
                        "Response serialization failed",
                        500,
                    ))
                })?,
            },
        })
        .map(|_| ())
        .map_err(response)
}

fn validate_journal(
    journal: &UploadJournal,
    package_id: &str,
    actor_id: &str,
) -> Result<(), Response> {
    if journal.schema != JOURNAL_SCHEMA
        || journal.package_id != package_id
        || journal.actor_id != actor_id
    {
        return Err(response(problem(
            "migration_package_journal_conflict",
            "Migration package journal conflicts with upload",
            409,
        )));
    }
    Ok(())
}

fn actor_id(principal: &buglogin_domain::permissions::Principal) -> Result<String, Response> {
    principal
        .account_id
        .as_ref()
        .map(|account| account.as_str().to_owned())
        .ok_or_else(|| {
            response(problem(
                "authentication_required",
                "Authentication required",
                401,
            ))
        })
}

fn required_checksum(headers: &HeaderMap) -> Result<String, Response> {
    headers
        .get("x-content-sha256")
        .and_then(|value| value.to_str().ok())
        .filter(|value| value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .map(|value| value.to_ascii_lowercase())
        .ok_or_else(|| {
            response(problem(
                "migration_checksum_required",
                "Valid migration file checksum required",
                400,
            ))
        })
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

fn journal_id(actor_id: &str, package_id: &str) -> String {
    format!("mpu_{}", &sha256_parts(&[actor_id, package_id])[..40])
}

fn storage_key(journal_id: &str, path: &str) -> String {
    format!("migration-packages/{journal_id}/{}", sha256_parts(&[path]))
}

fn sha256_parts(parts: &[&str]) -> String {
    let mut digest = Sha256::new();
    for part in parts {
        digest.update((part.len() as u64).to_be_bytes());
        digest.update(part.as_bytes());
    }
    format!("{:x}", digest.finalize())
}
