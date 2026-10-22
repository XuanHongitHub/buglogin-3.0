use std::collections::BTreeMap;

use axum::{
    Json,
    extract::{Path, Query, Request, State},
    http::{HeaderMap, StatusCode, header::CONTENT_TYPE},
    response::{IntoResponse, Response},
};
use buglogin_domain::{errors::Problem, permissions::Action};
use buglogin_migration::{
    package::NeutralPackage, signature::verify_signed_package, verify::validate_relative_path,
};
use buglogin_storage::{
    blob::checksum,
    control::{
        ControlAuditEntry, ControlPut, ControlRecordKey, ExecuteControlCommand, HttpResponse,
    },
    remote::MAX_REMOTE_OBJECT_BYTES,
};
use buglogin_sync::export::SyncExport;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use super::control_import::validate_package_id;
use super::{
    ApiState, CONTROL_TENANT, authenticate, authorize_capability, control, control_import, grants,
    problem, request_now_ms, response, sync_import,
};

const JOURNAL_TYPE: &str = "migration_package_upload";
const JOURNAL_SCHEMA: &str = "buglogin.migration-package-upload/1";
const MAX_FILE_BYTES: usize = MAX_REMOTE_OBJECT_BYTES as usize;
// Package verification reconstructs all staged files in memory. Keep the
// aggregate ceiling aligned with the VPS memory budget; browser payloads use
// the separate encrypted profile-sync path.
const MAX_PACKAGE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_PACKAGE_FILES: usize = 10_000;

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
    request: Request,
) -> Result<Response, Response> {
    let (parts, body) = request.into_parts();
    let headers = parts.headers;
    let principal = authenticate(&state, &headers)?;
    let expected_grants = grants(&state)?;
    authorize_capability(
        &principal,
        &expected_grants,
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
    let content_type = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(str::trim);
    if !content_type.is_some_and(|value| value.eq_ignore_ascii_case("application/octet-stream")) {
        return Err(response(problem(
            "unsupported_media_type",
            "Migration package files require application/octet-stream",
            415,
        )));
    }
    if super::declared_body_exceeds_limit(&headers, MAX_FILE_BYTES).map_err(response)? {
        return Err(response(
            problem(
                "migration_package_file_too_large",
                "Migration package file exceeds upload limit",
                413,
            )
            .with_field("maxBytes", MAX_FILE_BYTES),
        ));
    }
    let _admission = super::large_transfer_body_slot().await.map_err(response)?;
    let body = axum::body::to_bytes(body, MAX_FILE_BYTES)
        .await
        .map_err(|_| {
            response(
                problem(
                    "migration_package_file_too_large",
                    "Migration package file exceeds upload limit",
                    413,
                )
                .with_field("maxBytes", MAX_FILE_BYTES),
            )
        })?;
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

    let loaded = state
        .control_persistence
        .load_versioned::<UploadJournal>(CONTROL_TENANT, JOURNAL_TYPE, &journal_id)
        .map_err(response)?;
    let initial_updated_at_ms = request_now_ms(&headers)?;
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
                updated_at_ms: initial_updated_at_ms,
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

    let _package_bytes = checked_package_size(&journal.files, Some(body.len() as u64))
        .map_err(package_limit_response)?;

    journal.files.insert(
        query.path.clone(),
        StagedFile {
            path: query.path.clone(),
            checksum: expected_checksum.clone(),
            size: body.len() as u64,
            storage_key: storage_key.clone(),
        },
    );
    journal.updated_at_ms = request_now_ms(&headers)?;
    control::with_authorization_snapshot(
        &state,
        &headers,
        &principal,
        &expected_grants,
        |current_principal, current_grants, control_revision| {
            authorize_capability(
                current_principal,
                current_grants,
                "migration.write",
                Action::Execute,
            )?;
            persist_journal(
                &state,
                &journal_id,
                revision,
                control_revision,
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
            )
        },
    )
    .await?;
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
    let expected_grants = grants(&state)?;
    authorize_capability(
        &principal,
        &expected_grants,
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
    let total_bytes = checked_package_size(&journal.files, None).map_err(package_limit_response)?;
    // The package is size-bounded before rehydration; peak verification memory
    // remains proportional to this 64 MiB ceiling until verification streams files.
    let package = load_package(&state, &journal)?;
    let verified = {
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
        verify_signed_package(&package, signing_key.as_bytes()).map_err(response)?
    };
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
    journal.updated_at_ms = request_now_ms(&headers)?;
    drop(_mutation);
    control::with_authorization_snapshot(
        &state,
        &headers,
        &principal,
        &expected_grants,
        |current_principal, current_grants, control_revision| {
            authorize_capability(
                current_principal,
                current_grants,
                "migration.write",
                Action::Execute,
            )?;
            persist_journal(
                &state,
                &journal_id,
                revision,
                control_revision,
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
            )
        },
    )
    .await?;
    Ok((StatusCode::OK, Json(receipt)).into_response())
}

pub(crate) async fn activate_package_upload(
    State(state): State<ApiState>,
    Path(package_id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, Response> {
    let principal = authenticate(&state, &headers)?;
    let expected_grants = grants(&state)?;
    authorize_capability(
        &principal,
        &expected_grants,
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
    let sync_export = package
        .file("sync/export.json")
        .map(|bytes| {
            let export = serde_json::from_slice::<SyncExport>(bytes).map_err(|_| {
                response(problem(
                    "migration_sync_export_invalid",
                    "Migration sync export is invalid",
                    422,
                ))
            })?;
            export.validate().map_err(response)?;
            Ok::<_, Response>(export)
        })
        .transpose()?;
    // Package verification can take long enough for an administrator to revoke
    // migration.write. Refresh and reauthenticate immediately before the first
    // control-plane mutation; the import's aggregate revision CAS fences a
    // revocation that commits after this snapshot check.
    control::with_authorization_snapshot(
        &state,
        &headers,
        &principal,
        &expected_grants,
        |current_principal, current_grants, _| {
            authorize_capability(
                current_principal,
                current_grants,
                "migration.write",
                Action::Execute,
            )?;
            Ok(())
        },
    )
    .await?;
    // Validate every package component before activating control records. A malformed
    // sync envelope must not leave an imported control graph behind on an otherwise
    // retryable package upload.
    // Serialize the full control-plus-sync activation. Releasing the package
    // lock after control import would let a concurrent retry depend on records
    // that the first activation may later compensate.
    let package_lock =
        control_import::ControlImportPackageLock::acquire(&state, &package_id).map_err(response)?;
    let control_import = control_import::import_orchestrated_package_with_lock(
        &state,
        &principal,
        &package,
        &format!("{idempotency_key}-control"),
        Some(&package_lock),
    )
    .map_err(response)?;
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
                // A replayed control receipt belongs to an earlier activation
                // attempt. Do not let a concurrent retry that loses the sync
                // lease roll back the control graph another request is using.
                if should_compensate_control_import(
                    control_import.as_ref().map(|receipt| receipt.replayed),
                    &sync_error,
                ) {
                    if let Err(rollback_error) = control_import::rollback_package_control_with_lock(
                        &state,
                        &package_id,
                        &format!("{idempotency_key}-control-rollback"),
                        &actor_id,
                        &package_lock,
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
    active_journal.updated_at_ms = request_now_ms(&headers)?;
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
        state.control_revision().map_err(response)?,
        idempotency_key,
        &actor_id,
        "migration.package.activate",
        &active_journal,
        json!({"packageId": package_id}),
        &receipt,
    )?;
    Ok((StatusCode::OK, Json(receipt)).into_response())
}

pub(crate) async fn response_problem(value: Response) -> Problem {
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

fn should_compensate_control_import(
    control_import_replayed: Option<bool>,
    sync_error: &Problem,
) -> bool {
    sync_error.code != "migration_sync_commit_outcome_unknown"
        && control_import_replayed == Some(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn staged_file(size: u64) -> StagedFile {
        StagedFile {
            path: "file.json".into(),
            checksum: "0".repeat(64),
            size,
            storage_key: "staged/file".into(),
        }
    }

    #[test]
    fn package_limits_cover_aggregate_bytes_file_count_and_overflow() {
        let mut files = BTreeMap::new();
        assert_eq!(checked_package_size(&files, None), Ok(0));

        files.insert("one".into(), staged_file(MAX_PACKAGE_BYTES));
        assert_eq!(checked_package_size(&files, None), Ok(MAX_PACKAGE_BYTES));
        assert_eq!(
            checked_package_size(&files, Some(1)),
            Err(PackageLimitError::TooLarge)
        );

        files.clear();
        for index in 0..MAX_PACKAGE_FILES {
            files.insert(format!("file-{index}"), staged_file(0));
        }
        assert!(checked_package_size(&files, None).is_ok());
        assert_eq!(
            checked_package_size(&files, Some(0)),
            Err(PackageLimitError::TooManyFiles)
        );

        files.clear();
        files.insert("one".into(), staged_file(u64::MAX));
        files.insert("two".into(), staged_file(1));
        assert_eq!(
            checked_package_size(&files, None),
            Err(PackageLimitError::SizeOverflow)
        );
    }

    #[test]
    fn only_the_attempt_that_created_control_state_can_compensate_it() {
        let ordinary_failure = problem("sync_import_invalid", "Sync import invalid", 422);
        assert!(should_compensate_control_import(
            Some(false),
            &ordinary_failure
        ));
        assert!(!should_compensate_control_import(
            Some(true),
            &ordinary_failure
        ));
        assert!(!should_compensate_control_import(None, &ordinary_failure));

        let ambiguous = problem(
            "migration_sync_commit_outcome_unknown",
            "Commit outcome unknown",
            503,
        );
        assert!(!should_compensate_control_import(Some(false), &ambiguous));
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PackageLimitError {
    TooManyFiles,
    SizeOverflow,
    TooLarge,
}

fn checked_package_size(
    files: &BTreeMap<String, StagedFile>,
    additional_file_size: Option<u64>,
) -> Result<u64, PackageLimitError> {
    if files.len() > MAX_PACKAGE_FILES
        || (additional_file_size.is_some() && files.len() >= MAX_PACKAGE_FILES)
    {
        return Err(PackageLimitError::TooManyFiles);
    }
    let staged_bytes = files
        .values()
        .try_fold(0u64, |total, file| total.checked_add(file.size))
        .ok_or(PackageLimitError::SizeOverflow)?;
    let total_bytes = staged_bytes
        .checked_add(additional_file_size.unwrap_or_default())
        .ok_or(PackageLimitError::SizeOverflow)?;
    if total_bytes > MAX_PACKAGE_BYTES {
        return Err(PackageLimitError::TooLarge);
    }
    Ok(total_bytes)
}

fn package_limit_response(error: PackageLimitError) -> Response {
    match error {
        PackageLimitError::TooManyFiles => response(
            problem(
                "migration_package_file_count_exceeded",
                "Migration package contains too many files",
                413,
            )
            .with_field("maxFiles", MAX_PACKAGE_FILES),
        ),
        PackageLimitError::SizeOverflow => response(problem(
            "migration_package_size_invalid",
            "Migration package size is invalid",
            409,
        )),
        PackageLimitError::TooLarge => response(
            problem(
                "migration_package_too_large",
                "Migration package exceeds upload limit",
                413,
            )
            .with_field("maxBytes", MAX_PACKAGE_BYTES),
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
    control_revision: u64,
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
            expected_revisions: [(
                ControlRecordKey {
                    tenant_id: CONTROL_TENANT.into(),
                    record_type: super::CONTROL_STATE_TYPE.into(),
                    record_id: super::CONTROL_STATE_ID.into(),
                },
                control_revision,
            )]
            .into(),
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
