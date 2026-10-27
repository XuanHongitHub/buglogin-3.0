use std::collections::{BTreeMap, BTreeSet};

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use buglogin_domain::{
    account::{Account, AccountIdentity, AccountStatus, canonical_email},
    entities::{Profile, ProfileState, Runtime, SyncMode},
    errors::Problem,
    grant::{GroupShareGrant, ResourceGrant},
    ids::{
        AccountId, GrantId, GroupId, MemberId, OrganizationId, ProfileId, ResourceId, TeamId,
        WorkspaceId,
    },
    member::{Member, MemberRole, MemberStatus},
    organization::{Organization, OrganizationPolicy},
    permissions::{Action, Capability, Effect, Grant, Principal, ResourceScope},
    profile::{ProfileArchiveState, ProfileResource},
    resource::{Resource, ResourceKind, ResourceState},
    team::Team,
    workspace::{Workspace, WorkspacePolicy},
};
use buglogin_migration::legacy::{
    EncryptedLegacyCredentials, LegacyScryptCredential, NeutralAccountRecord, NeutralGrantRecord,
    NeutralGroupRecord, NeutralGroupShareGrantRecord, NeutralMemberRecord,
    NeutralOrganizationRecord, NeutralProfileRecord, NeutralProfileResourceRecord,
    NeutralResourceRecord, NeutralTeamRecord, NeutralWorkspaceRecord, ProfileDataClassification,
    decrypt_package_credentials, legacy_runtime_fingerprint,
};
use buglogin_migration::package::{NeutralPackage, NeutralPackageManifest};
use buglogin_storage::control::{
    ControlAuditEntry, ControlDelete, ControlPut, ExecuteControlCommand, HttpResponse,
};
use buglogin_sync::{export::SyncExport, manifest::SyncManifest, payload::PayloadDescriptor};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use super::{
    ApiState, CONTROL_STATE_ID, CONTROL_STATE_TYPE, CONTROL_TENANT, ControlAggregate, authenticate,
    authorize_capability, control, grants, problem, response,
};

const JOURNAL_TYPE: &str = "migration_import";
const COMMAND: &str = "migration.control.import";
pub(super) const MEMBERSHIP_CAPABILITIES: [&str; 15] = [
    "CAP-WS-001",
    "CAP-TEAM-001",
    "CAP-MEM-001",
    "CAP-RES-001",
    "CAP-AUD-001",
    "CAP-PROF-001",
    "CAP-PROF-002",
    "CAP-PROF-003",
    "CAP-NET-001",
    "CAP-NET-003",
    "CAP-EXT-002",
    "CAP-SYNC-001",
    "events.read",
    "operation.read",
    "profile.read",
];
const ALL_ACTIONS: [Action; 7] = [
    Action::Read,
    Action::Create,
    Action::Update,
    Action::Delete,
    Action::Execute,
    Action::Share,
    Action::Manage,
];

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ControlImportInput {
    package_id: String,
    #[serde(default)]
    dry_run: bool,
    #[serde(default)]
    accounts: Vec<NeutralAccountRecord>,
    #[serde(default)]
    profiles: Vec<NeutralProfileRecord>,
    #[serde(default)]
    organizations: Vec<NeutralOrganizationRecord>,
    #[serde(default)]
    workspaces: Vec<NeutralWorkspaceRecord>,
    #[serde(default)]
    teams: Vec<NeutralTeamRecord>,
    #[serde(default)]
    members: Vec<NeutralMemberRecord>,
    #[serde(default)]
    resources: Vec<NeutralResourceRecord>,
    #[serde(default)]
    profile_resources: Vec<NeutralProfileResourceRecord>,
    #[serde(default)]
    grants: Vec<NeutralGrantRecord>,
    #[serde(default)]
    groups: Vec<NeutralGroupRecord>,
    #[serde(default)]
    group_grants: Vec<NeutralGroupShareGrantRecord>,
    #[serde(skip)]
    legacy_credentials: Vec<LegacyScryptCredential>,
    #[serde(skip)]
    require_legacy_identity_import: bool,
    #[serde(default, skip_deserializing)]
    legacy_credentials_sha256: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct Counts {
    profiles: usize,
    accounts: usize,
    organizations: usize,
    workspaces: usize,
    teams: usize,
    members: usize,
    resources: usize,
    profile_resources: usize,
    grants: usize,
    groups: usize,
    group_grants: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct Plan {
    package_id: String,
    package_sha256: String,
    create: Counts,
    reuse: Counts,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Receipt {
    package_id: String,
    package_sha256: String,
    state: String,
    pub(super) replayed: bool,
    create: Counts,
    reuse: Counts,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct Journal {
    schema: String,
    package_id: String,
    package_sha256: String,
    state: String,
    before_sha256: String,
    after_sha256: String,
    before: Value,
    created_accounts: BTreeMap<String, String>,
    #[serde(default)]
    created_identity_records: Vec<CreatedIdentityRecord>,
    create: Counts,
    reuse: Counts,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct CreatedIdentityRecord {
    record_type: String,
    record_id: String,
    sha256: String,
}

struct Prepared {
    before: ControlAggregate,
    aggregate: ControlAggregate,
    account_puts: Vec<ControlPut>,
    identity_puts: Vec<ControlPut>,
    plan: Plan,
    before_sha256: String,
    after_sha256: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ControlRollbackInput {
    package_id: String,
    confirmation: String,
}

pub(super) async fn import_control(
    State(state): State<ApiState>,
    headers: HeaderMap,
    input: Result<Json<ControlImportInput>, axum::extract::rejection::JsonRejection>,
) -> Result<Response, Response> {
    let principal = authenticate(&state, &headers)?;
    let Json(input) = input.map_err(|_| {
        response(problem(
            "invalid_input",
            "Control import request body is invalid",
            422,
        ))
    })?;
    let expected_grants = grants(&state)?;
    authorize_capability(
        &principal,
        &expected_grants,
        "migration.write",
        Action::Execute,
    )?;
    let idempotency_key = headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
        .filter(|value| value.is_ascii() && (16..=128).contains(&value.len()))
        .ok_or_else(|| {
            response(problem(
                "idempotency_key_required",
                "Valid idempotency key required",
                400,
            ))
        })?;
    validate_package_id(&input.package_id).map_err(response)?;
    let package_sha256 = package_checksum(&input).map_err(response)?;
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
            let actor_id = current_principal
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
            let journal_revision = if let Some((journal, revision)) = state
                .control_persistence
                .load_versioned::<Journal>(CONTROL_TENANT, JOURNAL_TYPE, &input.package_id)
                .map_err(response)?
            {
                if journal.package_sha256 != package_sha256 {
                    return Err(response(collision("migration package")));
                }
                if journal.state != "rolled_back" {
                    return Ok((
                        StatusCode::OK,
                        Json(Receipt {
                            package_id: journal.package_id,
                            package_sha256: journal.package_sha256,
                            state: journal.state,
                            replayed: true,
                            create: journal.create,
                            reuse: journal.reuse,
                        }),
                    )
                        .into_response());
                }
                revision
            } else {
                0
            };

            let prepared = prepare(&state, &input, package_sha256, None).map_err(response)?;
            if input.dry_run {
                return Ok((StatusCode::OK, Json(prepared.plan)).into_response());
            }
            activate(
                &state,
                &input,
                idempotency_key,
                actor_id,
                journal_revision,
                prepared,
            )
            .map(|receipt| (StatusCode::OK, Json(receipt)).into_response())
            .map_err(response)
        },
    )
    .await
}

pub(super) fn import_package(
    state: &ApiState,
    principal: &Principal,
    package: &NeutralPackage,
    idempotency_key: &str,
) -> Result<Option<Receipt>, Problem> {
    reject_unorchestrated_sync_package(package)?;
    import_orchestrated_package(state, principal, package, idempotency_key)
}

pub(super) fn import_orchestrated_package(
    state: &ApiState,
    principal: &Principal,
    package: &NeutralPackage,
    idempotency_key: &str,
) -> Result<Option<Receipt>, Problem> {
    import_orchestrated_package_with_lock(state, principal, package, idempotency_key, None)
}

pub(super) fn import_orchestrated_package_with_lock(
    state: &ApiState,
    principal: &Principal,
    package: &NeutralPackage,
    idempotency_key: &str,
    package_lock: Option<&ControlImportPackageLock<'_>>,
) -> Result<Option<Receipt>, Problem> {
    let Some(input) = control_input_from_package(state, package)? else {
        return Ok(None);
    };
    let sync_manifest_to_import = package
        .file("sync/export.json")
        .map(|bytes| {
            let export = serde_json::from_slice::<SyncExport>(bytes)
                .map_err(|_| invalid("migration sync export"))?;
            export.validate_metadata()?;
            Ok::<_, Problem>((
                export.workspace_id.as_str().to_owned(),
                export.resource_id.as_str().to_owned(),
            ))
        })
        .transpose()?;
    import_control_input(
        state,
        principal,
        idempotency_key,
        input,
        false,
        package_lock,
        sync_manifest_to_import,
    )
    .map(Some)
}

pub(super) fn import_initial_operator_package(
    state: &ApiState,
    package: &NeutralPackage,
) -> Result<Receipt, Problem> {
    reject_unorchestrated_sync_package(package)?;
    let input = control_input_from_package(state, package)?
        .ok_or_else(|| invalid("initial migration control package"))?;
    require_password_capable_platform_admin(&input)?;
    let principal = Principal {
        account_id: Some(
            AccountId::new("production-migration-operator")
                .map_err(|_| invalid("migration operator"))?,
        ),
        ..Principal::default()
    };
    let idempotency_key = format!("initial-operator-{}", input.package_id);
    import_control_input(state, &principal, &idempotency_key, input, true, None, None)
}

fn control_input_from_package(
    state: &ApiState,
    package: &NeutralPackage,
) -> Result<Option<ControlImportInput>, Problem> {
    let migration_key = if package
        .file(buglogin_migration::legacy_account_credentials::PACKAGE_PATH)
        .is_some()
    {
        Some(
            state
                .migration_key
                .read()
                .map_err(|_| {
                    problem(
                        "migration_key_unavailable",
                        "Migration key unavailable",
                        503,
                    )
                })?
                .clone()
                .ok_or_else(|| {
                    problem(
                        "migration_key_unavailable",
                        "Migration key unavailable",
                        503,
                    )
                })?,
        )
    } else {
        None
    };
    package_input(package, migration_key.as_ref())
}

fn import_control_input(
    state: &ApiState,
    principal: &Principal,
    idempotency_key: &str,
    input: ControlImportInput,
    initial_operator: bool,
    package_lock: Option<&ControlImportPackageLock<'_>>,
    sync_manifest_to_import: Option<(String, String)>,
) -> Result<Receipt, Problem> {
    let actor_id = principal
        .account_id
        .as_ref()
        .ok_or_else(|| problem("authentication_required", "Authentication required", 401))?
        .as_str()
        .to_owned();
    validate_package_id(&input.package_id)?;
    let package_sha256 = package_checksum(&input)?;
    let _owned_package_lock = match package_lock {
        Some(lock) if lock.matches(&input.package_id) => None,
        Some(_) => return Err(invalid("migration package lock")),
        None => Some(ControlImportPackageLock::acquire(state, &input.package_id)?),
    };
    let _mutation = state
        .control_mutation
        .lock()
        .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?;
    let journal_revision = if let Some((journal, revision)) = state
        .control_persistence
        .load_versioned::<Journal>(CONTROL_TENANT, JOURNAL_TYPE, &input.package_id)?
    {
        if journal.package_sha256 != package_sha256 {
            return Err(collision("migration package"));
        }
        if journal.state != "rolled_back" {
            return Ok(Receipt {
                package_id: journal.package_id,
                package_sha256: journal.package_sha256,
                state: journal.state,
                replayed: true,
                create: journal.create,
                reuse: journal.reuse,
            });
        }
        revision
    } else {
        0
    };
    if initial_operator {
        ensure_initial_target_empty(state, &input.package_id)?;
    }
    let prepared = prepare(state, &input, package_sha256, sync_manifest_to_import)?;
    activate(
        state,
        &input,
        idempotency_key,
        actor_id,
        journal_revision,
        prepared,
    )
}

fn require_password_capable_platform_admin(input: &ControlImportInput) -> Result<(), Problem> {
    let credentials = input
        .legacy_credentials
        .iter()
        .map(|credential| credential.source_user_id.as_str())
        .collect::<BTreeSet<_>>();
    let accounts = account_models(input)?;
    let has_login_admin = accounts.iter().any(|(account_id, (account, is_admin))| {
        if !is_admin
            || account.status != AccountStatus::Active
            || !credentials.contains(account_id.as_str())
        {
            return false;
        }
        input
            .accounts
            .iter()
            .find(|record| record.id == *account_id)
            .and_then(|record| record.extensions.get("legacyAuthProvider"))
            .and_then(Value::as_str)
            .is_some_and(|provider| matches!(provider, "password" | "password_google"))
    });
    if has_login_admin {
        Ok(())
    } else {
        Err(problem(
            "migration_initial_admin_missing",
            "Initial migration package needs a password-capable platform administrator",
            422,
        ))
    }
}

fn ensure_initial_target_empty(state: &ApiState, package_id: &str) -> Result<(), Problem> {
    let accounts = state
        .control_persistence
        .list::<Account>(CONTROL_TENANT, "account")?;
    let identities = state
        .control_persistence
        .list::<AccountIdentity>("identity", "account_identity")?;
    let passwords = state
        .control_persistence
        .list::<Value>("identity", "account_password")?;
    let other_journals = state
        .control_persistence
        .list::<Journal>(CONTROL_TENANT, JOURNAL_TYPE)?
        .into_iter()
        .any(|journal| journal.package_id != package_id);
    let aggregate = state.control_aggregate()?;
    let aggregate_value = serde_json::to_value(&aggregate).map_err(|_| invalid("control state"))?;
    let empty_value = serde_json::to_value(ControlAggregate::default())
        .map_err(|_| invalid("empty control state"))?;
    if accounts.is_empty()
        && identities.is_empty()
        && passwords.is_empty()
        && !other_journals
        && aggregate_value == empty_value
    {
        Ok(())
    } else {
        Err(problem(
            "migration_initial_target_not_empty",
            "Initial migration can only run on an empty BugLogin 3 control database",
            409,
        ))
    }
}

fn reject_unorchestrated_sync_package(package: &NeutralPackage) -> Result<(), Problem> {
    if package.file("sync/export.json").is_some() {
        return Err(problem(
            "migration_sync_import_not_ready",
            "Hosted sync migration import requires package orchestration",
            503,
        ));
    }
    Ok(())
}

fn package_input(
    package: &NeutralPackage,
    migration_key: Option<&buglogin_migration::MigrationKey>,
) -> Result<Option<ControlImportInput>, Problem> {
    const FILES: [&str; 11] = [
        "entities/profiles.jsonl",
        "entities/accounts.jsonl",
        "entities/organizations.jsonl",
        "entities/workspaces.jsonl",
        "entities/teams.jsonl",
        "entities/members.jsonl",
        "entities/resources.jsonl",
        "entities/profile-resources.jsonl",
        "entities/grants.jsonl",
        "entities/groups.jsonl",
        "entities/group-grants.jsonl",
    ];
    if FILES.iter().all(|path| package.file(path).is_none()) {
        return Ok(None);
    }
    let manifest: NeutralPackageManifest = serde_json::from_slice(
        package
            .file("manifest.json")
            .ok_or_else(|| invalid("migration manifest"))?,
    )
    .map_err(|_| invalid("migration manifest"))?;
    let encrypted_credentials = package
        .file(buglogin_migration::legacy_account_credentials::PACKAGE_PATH)
        .map(serde_json::from_slice::<EncryptedLegacyCredentials>)
        .transpose()
        .map_err(|_| invalid("encrypted legacy credentials"))?;
    let legacy_credentials = match encrypted_credentials.as_ref() {
        Some(encrypted) => decrypt_package_credentials(
            encrypted,
            migration_key.ok_or_else(|| invalid("migration credential key"))?,
        )?,
        None => Vec::new(),
    };
    Ok(Some(ControlImportInput {
        package_id: manifest.package_id,
        dry_run: false,
        profiles: package_records(package, FILES[0])?,
        accounts: package_records(package, FILES[1])?,
        organizations: package_records(package, FILES[2])?,
        workspaces: package_records(package, FILES[3])?,
        teams: package_records(package, FILES[4])?,
        members: package_records(package, FILES[5])?,
        resources: package_records(package, FILES[6])?,
        profile_resources: package_records(package, FILES[7])?,
        grants: package_records(package, FILES[8])?,
        groups: package_records_optional(package, FILES[9])?,
        group_grants: package_records_optional(package, FILES[10])?,
        legacy_credentials_sha256: encrypted_credentials
            .as_ref()
            .map(|credentials| credentials.ciphertext_sha256.clone()),
        legacy_credentials,
        require_legacy_identity_import: true,
    }))
}

fn package_records<T: for<'de> Deserialize<'de>>(
    package: &NeutralPackage,
    path: &str,
) -> Result<Vec<T>, Problem> {
    let bytes = package
        .file(path)
        .ok_or_else(|| invalid("control-plane package graph"))?;
    bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).map_err(|_| invalid("control-plane record")))
        .collect()
}

fn package_records_optional<T: for<'de> Deserialize<'de>>(
    package: &NeutralPackage,
    path: &str,
) -> Result<Vec<T>, Problem> {
    match package.file(path) {
        Some(_) => package_records(package, path),
        None => Ok(Vec::new()),
    }
}

fn activate(
    state: &ApiState,
    input: &ControlImportInput,
    idempotency_key: &str,
    actor_id: String,
    journal_revision: u64,
    prepared: Prepared,
) -> Result<Receipt, Problem> {
    let created_accounts = prepared
        .account_puts
        .iter()
        .map(|put| {
            let account: Account = serde_json::from_value(put.value.clone())
                .map_err(|_| invalid("account journal value"))?;
            Ok((put.record_id.clone(), value_checksum(&account)?))
        })
        .collect::<Result<BTreeMap<_, _>, Problem>>()?;
    let created_identity_records = prepared
        .identity_puts
        .iter()
        .map(|put| {
            Ok(CreatedIdentityRecord {
                record_type: put.record_type.clone(),
                record_id: put.record_id.clone(),
                sha256: value_checksum(&put.value)?,
            })
        })
        .collect::<Result<Vec<_>, Problem>>()?;
    let receipt = Receipt {
        package_id: prepared.plan.package_id.clone(),
        package_sha256: prepared.plan.package_sha256.clone(),
        state: "active".into(),
        replayed: false,
        create: prepared.plan.create.clone(),
        reuse: prepared.plan.reuse.clone(),
    };
    let journal = Journal {
        schema: "buglogin.control-import-journal/1".into(),
        package_id: receipt.package_id.clone(),
        package_sha256: receipt.package_sha256.clone(),
        state: receipt.state.clone(),
        before_sha256: prepared.before_sha256,
        after_sha256: prepared.after_sha256,
        before: serde_json::to_value(&prepared.before).map_err(|_| invalid("control state"))?,
        created_accounts,
        created_identity_records,
        create: receipt.create.clone(),
        reuse: receipt.reuse.clone(),
    };
    let control_revision = *state
        .control_revision
        .read()
        .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?;
    let mut puts = prepared.account_puts;
    puts.extend(prepared.identity_puts);
    puts.push(ControlPut {
        tenant_id: CONTROL_TENANT.into(),
        record_type: CONTROL_STATE_TYPE.into(),
        record_id: CONTROL_STATE_ID.into(),
        expected_revision: Some(control_revision),
        value: serde_json::to_value(&prepared.aggregate).map_err(|_| {
            problem(
                "control_serialization_failed",
                "Control state serialization failed",
                500,
            )
        })?,
    });
    puts.push(ControlPut {
        tenant_id: CONTROL_TENANT.into(),
        record_type: JOURNAL_TYPE.into(),
        record_id: input.package_id.clone(),
        expected_revision: Some(journal_revision),
        value: serde_json::to_value(&journal).map_err(|_| {
            problem(
                "control_serialization_failed",
                "Control import journal serialization failed",
                500,
            )
        })?,
    });
    let response_value = serde_json::to_value(&receipt).map_err(|_| {
        problem(
            "control_serialization_failed",
            "Control import receipt serialization failed",
            500,
        )
    })?;
    let command_key = format!(
        "control-import-{}",
        hex_digest(format!("{}:{idempotency_key}:{journal_revision}", input.package_id).as_bytes(),)
    );
    let result = state.control_persistence.execute(ExecuteControlCommand {
        tenant_id: CONTROL_TENANT.into(),
        command_id: COMMAND.into(),
        idempotency_key: command_key,
        expected_revisions: Default::default(),
        puts,
        deletes: vec![],
        audit: ControlAuditEntry {
            tenant_id: CONTROL_TENANT.into(),
            command_id: COMMAND.into(),
            actor_id,
            allowed: true,
            detail: json!({"packageId": input.package_id, "packageSha256": receipt.package_sha256}),
        },
        response: HttpResponse {
            status: 200,
            headers: BTreeMap::from([(
                "x-buglogin-input-sha256".into(),
                receipt.package_sha256.clone(),
            )]),
            body: response_value,
        },
    })?;
    let revision = result
        .records
        .iter()
        .find(|record| {
            record.record_type == CONTROL_STATE_TYPE && record.record_id == CONTROL_STATE_ID
        })
        .map(|record| record.revision)
        .ok_or_else(|| {
            problem(
                "control_persistence_unavailable",
                "Control persistence is unavailable",
                503,
            )
        })?;
    state.replace_control_aggregate(prepared.aggregate)?;
    *state
        .control_revision
        .write()
        .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))? = revision;
    Ok(receipt)
}

pub(super) async fn rollback_control(
    State(state): State<ApiState>,
    headers: HeaderMap,
    input: Result<Json<ControlRollbackInput>, axum::extract::rejection::JsonRejection>,
) -> Result<Response, Response> {
    let principal = authenticate(&state, &headers)?;
    let expected_grants = grants(&state)?;
    let Json(input) = input.map_err(|_| {
        response(problem(
            "invalid_input",
            "Control rollback request body is invalid",
            422,
        ))
    })?;
    authorize_capability(
        &principal,
        &expected_grants,
        "migration.write",
        Action::Execute,
    )?;
    if input.confirmation != "ROLLBACK_CONTROL_MIGRATION" {
        return Err(response(problem(
            "migration_confirmation_required",
            "Control migration rollback confirmation required",
            409,
        )));
    }
    validate_package_id(&input.package_id).map_err(response)?;
    let idempotency_key = headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
        .filter(|value| value.is_ascii() && (16..=128).contains(&value.len()))
        .ok_or_else(|| {
            response(problem(
                "idempotency_key_required",
                "Valid idempotency key required",
                400,
            ))
        })?;
    let _package_lock =
        ControlImportPackageLock::acquire(&state, &input.package_id).map_err(response)?;
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
            let actor_id = current_principal
                .account_id
                .as_ref()
                .ok_or_else(|| {
                    response(problem(
                        "authentication_required",
                        "Authentication required",
                        401,
                    ))
                })?
                .as_str();
            let receipt = rollback_package_control_locked(
                &state,
                &input.package_id,
                idempotency_key,
                actor_id,
            )
            .map_err(response)?;
            Ok((StatusCode::OK, Json(receipt)).into_response())
        },
    )
    .await
}

pub(super) fn rollback_package_control_with_lock(
    state: &ApiState,
    package_id: &str,
    idempotency_key: &str,
    actor_id: &str,
    package_lock: &ControlImportPackageLock<'_>,
) -> Result<Receipt, Problem> {
    validate_package_id(package_id)?;
    if !package_lock.matches(package_id) {
        return Err(invalid("migration package lock"));
    }
    let _mutation = state
        .control_mutation
        .lock()
        .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?;
    rollback_package_control_locked(state, package_id, idempotency_key, actor_id)
}

fn rollback_package_control_locked(
    state: &ApiState,
    package_id: &str,
    idempotency_key: &str,
    actor_id: &str,
) -> Result<Receipt, Problem> {
    validate_package_id(package_id)?;
    let (mut journal, journal_revision) = state
        .control_persistence
        .load_versioned::<Journal>(CONTROL_TENANT, JOURNAL_TYPE, package_id)?
        .ok_or_else(|| {
            problem(
                "migration_control_import_not_found",
                "Control import not found",
                404,
            )
        })?;
    if journal.state == "rolled_back" {
        return Ok(Receipt {
            package_id: journal.package_id,
            package_sha256: journal.package_sha256,
            state: journal.state,
            replayed: true,
            create: journal.create,
            reuse: journal.reuse,
        });
    }
    if journal.state != "active"
        || value_checksum(&state.control_aggregate()?)? != journal.after_sha256
    {
        return Err(problem(
            "migration_control_import_changed",
            "Control state changed after migration import",
            409,
        ));
    }
    let mut deletes = Vec::new();
    for (account_id, expected_sha256) in &journal.created_accounts {
        let id = AccountId::from_str(account_id).map_err(|_| invalid("account ID"))?;
        let (account, revision) =
            state
                .account_persistence
                .load_versioned(&id)?
                .ok_or_else(|| {
                    problem(
                        "migration_control_import_changed",
                        "Imported account changed",
                        409,
                    )
                })?;
        if value_checksum(&account)? != *expected_sha256 {
            return Err(problem(
                "migration_control_import_changed",
                "Imported account changed",
                409,
            ));
        }
        deletes.push(ControlDelete {
            tenant_id: CONTROL_TENANT.into(),
            record_type: "account".into(),
            record_id: account_id.clone(),
            expected_revision: Some(revision),
        });
    }
    for created in &journal.created_identity_records {
        let (value, revision) = state
            .control_persistence
            .load_versioned::<Value>("identity", &created.record_type, &created.record_id)?
            .ok_or_else(|| {
                problem(
                    "migration_control_import_changed",
                    "Imported identity record changed",
                    409,
                )
            })?;
        if value_checksum(&value)? != created.sha256 {
            return Err(problem(
                "migration_control_import_changed",
                "Imported identity record changed",
                409,
            ));
        }
        deletes.push(ControlDelete {
            tenant_id: "identity".into(),
            record_type: created.record_type.clone(),
            record_id: created.record_id.clone(),
            expected_revision: Some(revision),
        });
    }
    let before: ControlAggregate = serde_json::from_value(journal.before.clone())
        .map_err(|_| invalid("rollback control state"))?;
    if value_checksum(&before)? != journal.before_sha256 {
        return Err(invalid("rollback control state checksum"));
    }
    let control_revision = *state
        .control_revision
        .read()
        .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?;
    journal.state = "rolled_back".into();
    let receipt = Receipt {
        package_id: journal.package_id.clone(),
        package_sha256: journal.package_sha256.clone(),
        state: journal.state.clone(),
        replayed: false,
        create: journal.create.clone(),
        reuse: journal.reuse.clone(),
    };
    let result = state.control_persistence.execute(ExecuteControlCommand {
        tenant_id: CONTROL_TENANT.into(),
        command_id: "migration.control.rollback".into(),
        idempotency_key: format!(
            "control-rollback-{}",
            hex_digest(format!("{package_id}:{idempotency_key}:{journal_revision}").as_bytes())
        ),
        expected_revisions: Default::default(),
        puts: vec![
            ControlPut {
                tenant_id: CONTROL_TENANT.into(),
                record_type: CONTROL_STATE_TYPE.into(),
                record_id: CONTROL_STATE_ID.into(),
                expected_revision: Some(control_revision),
                value: serde_json::to_value(&before)
                    .map_err(|_| invalid("rollback control state"))?,
            },
            ControlPut {
                tenant_id: CONTROL_TENANT.into(),
                record_type: JOURNAL_TYPE.into(),
                record_id: package_id.to_owned(),
                expected_revision: Some(journal_revision),
                value: serde_json::to_value(&journal).map_err(|_| invalid("rollback journal"))?,
            },
        ],
        deletes,
        audit: ControlAuditEntry {
            tenant_id: CONTROL_TENANT.into(),
            command_id: "migration.control.rollback".into(),
            actor_id: actor_id.to_owned(),
            allowed: true,
            detail: json!({"packageId": package_id}),
        },
        response: HttpResponse {
            status: 200,
            headers: Default::default(),
            body: serde_json::to_value(&receipt).map_err(|_| invalid("rollback receipt"))?,
        },
    })?;
    let revision = result
        .records
        .iter()
        .find(|record| {
            record.record_type == CONTROL_STATE_TYPE && record.record_id == CONTROL_STATE_ID
        })
        .map(|record| record.revision)
        .ok_or_else(|| {
            problem(
                "control_persistence_unavailable",
                "Control persistence is unavailable",
                503,
            )
        })?;
    state.replace_control_aggregate(before)?;
    *state
        .control_revision
        .write()
        .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))? = revision;
    Ok(receipt)
}

fn prepare(
    state: &ApiState,
    input: &ControlImportInput,
    package_sha256: String,
    sync_manifest_to_import: Option<(String, String)>,
) -> Result<Prepared, Problem> {
    validate_graph(input)?;
    let before = state.control_aggregate()?;
    let before_sha256 = value_checksum(&before)?;
    let mut value = serde_json::to_value(&before).map_err(|_| invalid("control state"))?;
    let mut create = empty_counts();
    let mut reuse = empty_counts();
    let mut account_puts = Vec::new();
    let mut identity_puts = Vec::new();
    let accounts = account_models(input)?;
    let credentials = input
        .legacy_credentials
        .iter()
        .map(|credential| (credential.source_user_id.as_str(), credential))
        .collect::<BTreeMap<_, _>>();
    if credentials.len() != input.legacy_credentials.len()
        || credentials
            .keys()
            .any(|account_id| !accounts.contains_key(*account_id))
    {
        return Err(invalid("legacy account credential references"));
    }

    for (account, platform_admin) in accounts.values() {
        match state.account_persistence.load_versioned(&account.id)? {
            Some((existing, _)) if existing == *account => {
                reuse.accounts += 1;
                if before.platform_admin_accounts.contains(&account.id) != *platform_admin {
                    return Err(collision("account authority"));
                }
            }
            Some(_) => return Err(collision("account")),
            None => {
                create.accounts += 1;
                account_puts.push(state.account_persistence.prepare_import_put(account, 0)?);
                if *platform_admin {
                    insert_set(&mut value, "platform_admin_accounts", account.id.as_str())?;
                }
            }
        }
    }

    if input.require_legacy_identity_import {
        for (account, _) in accounts.values() {
            let source = input
                .accounts
                .iter()
                .find(|record| record.id == account.id.as_str())
                .ok_or_else(|| invalid("account identity mapping"))?;
            let email = source
                .extensions
                .get("legacyEmail")
                .and_then(Value::as_str)
                .ok_or_else(|| invalid("legacy account email"))?;
            let canonical_legacy_email =
                canonical_email(email).map_err(|_| invalid("legacy account email"))?;
            let auth_provider = source
                .extensions
                .get("legacyAuthProvider")
                .and_then(Value::as_str);
            let credential = credentials.get(account.id.as_str());
            match auth_provider {
                Some("password" | "password_google") if credential.is_none() => {
                    return Err(invalid("legacy password credential missing"));
                }
                Some("google") if credential.is_some() => {
                    return Err(invalid("google-only account has a password credential"));
                }
                Some("password" | "password_google" | "google") | None if credential.is_some() => {
                    if auth_provider.is_none() {
                        return Err(invalid("legacy credential provider missing"));
                    }
                }
                _ => {}
            }
            if let Some(existing) = state.identity_persistence.load(&account.id)? {
                if existing.canonical_email != canonical_legacy_email {
                    return Err(collision("account identity"));
                }
                if state.credentials.password_version(&account.id)?.is_none() {
                    if let Some(credential) = credential {
                        identity_puts.push(state.credentials.prepare_legacy_scrypt_put(
                            &account.id,
                            &credential.salt,
                            &credential.hash,
                            0,
                        )?);
                    }
                }
                continue;
            }
            let username = legacy_username(account.id.as_str());
            let identity = AccountIdentity::new(
                account.id.clone(),
                username,
                email,
                account.created_at.clone(),
            )?
            .verify(1, account.updated_at.clone())?;
            identity_puts.extend(state.identity_persistence.prepare_create_puts(&identity)?);
            if let Some(credential) = credential {
                identity_puts.push(state.credentials.prepare_legacy_scrypt_put(
                    &account.id,
                    &credential.salt,
                    &credential.hash,
                    0,
                )?);
            }
        }
    }

    merge_profiles(input, &mut value, &mut create, &mut reuse)?;
    merge_workspaces(input, &accounts, &mut value, &mut create, &mut reuse)?;
    merge_members(input, &mut value, &mut create, &mut reuse)?;
    merge_teams(input, &mut value, &mut create, &mut reuse)?;
    merge_resources(input, &mut value, &mut create, &mut reuse)?;
    merge_profile_resources(input, &mut value, &mut create, &mut reuse)?;
    merge_resource_grants(input, &mut value, &mut create, &mut reuse)?;
    merge_group_shares(input, &mut value, &mut create, &mut reuse)?;
    mark_managed_group_share_resources_pending(&before, &mut value)?;
    materialize_membership_grants(input, &accounts, &mut value)?;

    let mut aggregate: ControlAggregate =
        serde_json::from_value(value).map_err(|_| invalid("control aggregate"))?;
    for binding in &input.profile_resources {
        if sync_manifest_to_import
            .as_ref()
            .is_some_and(|(workspace_id, resource_id)| {
                workspace_id == &binding.data.workspace_id
                    && resource_id == &binding.data.resource_id
            })
        {
            continue;
        }
        if binding.data.archive_state != "active" {
            continue;
        }
        let workspace_id = WorkspaceId::from_str(&binding.data.workspace_id)
            .map_err(|_| invalid("profile resource workspace"))?;
        let resource_id = ResourceId::from_str(&binding.data.resource_id)
            .map_err(|_| invalid("profile resource ID"))?;
        let manifest_key = format!("{}:{}", workspace_id.as_str(), resource_id.as_str());
        if !aggregate.sync_manifests.contains_key(&manifest_key) {
            let empty_payload =
                PayloadDescriptor::new(workspace_id.clone(), resource_id.clone(), 1, Vec::new())?;
            let genesis = SyncManifest::new(
                workspace_id,
                resource_id,
                1,
                1,
                None,
                &empty_payload.canonical_root_bytes(),
                Vec::new(),
            )?;
            aggregate.sync_manifests.insert(manifest_key, genesis);
        }
    }
    let after_sha256 = value_checksum(&aggregate)?;
    Ok(Prepared {
        before,
        aggregate,
        account_puts,
        identity_puts,
        plan: Plan {
            package_id: input.package_id.clone(),
            package_sha256,
            create,
            reuse,
        },
        before_sha256,
        after_sha256,
    })
}

fn account_models(
    input: &ControlImportInput,
) -> Result<BTreeMap<String, (Account, bool)>, Problem> {
    input
        .accounts
        .iter()
        .map(|record| {
            require_record(&record.schema, "buglogin.account/1", record.revision)?;
            let id = AccountId::from_str(&record.id).map_err(|_| invalid("account ID"))?;
            let created_at = required_timestamp(record.created_at.as_deref(), "account createdAt")?;
            let updated_at = required_timestamp(record.updated_at.as_deref(), "account updatedAt")?;
            let status = match record.data.status.as_str() {
                "active" => AccountStatus::Active,
                "suspended" => AccountStatus::Suspended,
                "disabled" => AccountStatus::Disabled,
                _ => return Err(invalid("account status")),
            };
            let platform_admin = match record.extensions.get("legacyPlatformRole") {
                None | Some(Value::Null) => false,
                Some(Value::String(role)) if role == "platform_admin" => true,
                _ => return Err(invalid("platform authority")),
            };
            Ok((
                record.id.clone(),
                (
                    Account {
                        id,
                        status,
                        revision: 1,
                        created_at,
                        updated_at,
                    },
                    platform_admin,
                ),
            ))
        })
        .collect()
}

fn legacy_username(account_id: &str) -> String {
    format!("legacy-{}", &hex_digest(account_id.as_bytes())[..32])
}

fn migrated_profile_runtime(legacy_runtime: &str) -> Runtime {
    match legacy_runtime.trim().to_ascii_lowercase().as_str() {
        "wayfern" => Runtime::Wayfern,
        _ => Runtime::Preserved,
    }
}

fn merge_group_shares(
    input: &ControlImportInput,
    aggregate: &mut Value,
    create: &mut Counts,
    reuse: &mut Counts,
) -> Result<(), Problem> {
    use crate::services::groups::{GroupRecord, GroupService};
    use buglogin_domain::{
        entities::Group,
        grant::GroupShareGrant,
        ids::{GrantId, MemberId},
    };

    let mut service: GroupService = serde_json::from_value(
        aggregate
            .get("groups")
            .cloned()
            .ok_or_else(|| invalid("groups"))?,
    )
    .map_err(|_| invalid("groups"))?;
    let groups = input
        .groups
        .iter()
        .filter(|record| record.data.workspace_id.is_some())
        .map(|record| {
            require_record(&record.schema, "buglogin.group/1", record.revision)?;
            let workspace_id = record
                .data
                .workspace_id
                .as_deref()
                .ok_or_else(|| invalid("group workspace"))?;
            let revision = u64::from(record.revision);
            Ok(GroupRecord {
                group: Group {
                    id: GroupId::from_str(&record.id).map_err(|_| invalid("group ID"))?,
                    name: record.data.name.clone(),
                    appearance: record.data.appearance.clone(),
                    created_at: optional_timestamp(
                        record.created_at.as_deref(),
                        "group createdAt",
                    )?
                    .unwrap_or_else(|| "1970-01-01T00:00:00Z".into()),
                    updated_at: optional_timestamp(
                        record.updated_at.as_deref(),
                        "group updatedAt",
                    )?
                    .unwrap_or_else(|| "1970-01-01T00:00:00Z".into()),
                },
                workspace_id: WorkspaceId::from_str(workspace_id)
                    .map_err(|_| invalid("group workspace"))?,
                revision,
            })
        })
        .collect::<Result<Vec<_>, Problem>>()?;
    let group_grants = input
        .group_grants
        .iter()
        .map(|record| {
            require_versioned_record(&record.schema, "buglogin.group-grant/1", record.revision)?;
            let revoked_at =
                optional_timestamp(record.data.revoked_at.as_deref(), "group grant revokedAt")?;
            Ok(GroupShareGrant {
                id: GrantId::from_str(&record.id).map_err(|_| invalid("group grant ID"))?,
                workspace_id: WorkspaceId::from_str(&record.data.workspace_id)
                    .map_err(|_| invalid("group grant workspace"))?,
                group_id: GroupId::from_str(&record.data.group_id)
                    .map_err(|_| invalid("group grant group"))?,
                member_id: MemberId::from_str(&record.data.member_id)
                    .map_err(|_| invalid("group grant member"))?,
                action: serde_json::from_value(json!(record.data.action))
                    .map_err(|_| invalid("group grant action"))?,
                effect: serde_json::from_value(json!(record.data.effect))
                    .map_err(|_| invalid("group grant effect"))?,
                revision: u64::from(record.revision),
                created_at: required_timestamp(
                    record.created_at.as_deref(),
                    "group grant createdAt",
                )?,
                revoked_at,
            })
        })
        .collect::<Result<Vec<_>, Problem>>()?;
    let (created_groups, reused_groups, created_grants, reused_grants) =
        service.import_records(groups, group_grants)?;
    create.groups += created_groups;
    reuse.groups += reused_groups;
    create.group_grants += created_grants;
    reuse.group_grants += reused_grants;
    aggregate["groups"] = serde_json::to_value(service).map_err(|_| invalid("groups"))?;
    Ok(())
}

fn merge_profiles(
    input: &ControlImportInput,
    aggregate: &mut Value,
    create: &mut Counts,
    reuse: &mut Counts,
) -> Result<(), Problem> {
    let profiles = object_path_mut(aggregate, &["profiles"])?;
    let managed_profiles = input
        .profile_resources
        .iter()
        .map(|binding| binding.data.profile_id.as_str())
        .collect::<BTreeSet<_>>();
    for record in &input.profiles {
        require_record(&record.schema, "buglogin.profile/1", record.revision)?;
        validate_name(&record.data.name, "profile name")?;
        let legacy_runtime = record
            .extensions
            .get("legacy_runtime")
            .and_then(Value::as_str)
            .unwrap_or(&record.data.runtime);
        let runtime = migrated_profile_runtime(legacy_runtime);
        let preserve_runtime = runtime == Runtime::Preserved;
        let preserve_profile_data = record.data.migration_class == "preserve-only"
            || record.data.source_data_object.disposition.classification
                == ProfileDataClassification::Excluded;
        let runtime_config = if preserve_runtime {
            Some(json!({
                "migration": {
                    "disposition": "preserve-only",
                    "legacyRuntime": legacy_runtime,
                    "legacyRuntimeConfig": record.data.legacy_runtime_config.clone()
                }
            }))
        } else {
            crate::services::profile_creation::ProfileCreationService::validate_legacy_profile_data(
                &runtime,
                record.data.legacy_runtime_config.as_ref(),
                None,
            )?;
            record.data.legacy_runtime_config.clone()
        };
        let id = ProfileId::from_str(&record.id).map_err(|_| invalid("profile ID"))?;
        let created_at = optional_timestamp(record.created_at.as_deref(), "profile createdAt")?
            .unwrap_or_else(|| "1970-01-01T00:00:00Z".into());
        let updated_at = optional_timestamp(record.updated_at.as_deref(), "profile updatedAt")?
            .unwrap_or_else(|| created_at.clone());
        let profile = Profile {
            id,
            name: record.data.name.clone(),
            runtime,
            runtime_version: record.data.runtime_version.clone(),
            runtime_config,
            state: if managed_profiles.contains(record.id.as_str())
                || preserve_runtime
                || preserve_profile_data
            {
                ProfileState::Created
            } else {
                ProfileState::Ready
            },
            archived: false,
            fingerprint: if preserve_runtime {
                None
            } else {
                legacy_runtime_fingerprint(record.data.legacy_runtime_config.as_ref())?
            },
            note: record.data.note.clone(),
            proxy_id: record
                .data
                .proxy_id
                .as_deref()
                .map(buglogin_domain::ids::ProxyId::from_str)
                .transpose()
                .map_err(|_| invalid("profile proxy ID"))?,
            vpn_id: record
                .data
                .vpn_id
                .as_deref()
                .map(buglogin_domain::ids::VpnId::from_str)
                .transpose()
                .map_err(|_| invalid("profile VPN ID"))?,
            group_ids: record
                .data
                .group_id
                .as_deref()
                .map(buglogin_domain::ids::GroupId::from_str)
                .transpose()
                .map_err(|_| invalid("profile group ID"))?
                .into_iter()
                .collect(),
            tags: record.data.tags.clone(),
            extension_group_id: record
                .data
                .extension_group_id
                .as_deref()
                .map(buglogin_domain::ids::ExtensionGroupId::from_str)
                .transpose()
                .map_err(|_| invalid("profile extension group ID"))?,
            sync_mode: Some(if managed_profiles.contains(record.id.as_str()) {
                SyncMode::Managed
            } else {
                SyncMode::Local
            }),
            created_at,
            updated_at,
        };
        let versioned = buglogin_storage::local::Versioned {
            schema_version: 1,
            revision: 1,
            value: profile,
        };
        merge_exact(
            profiles,
            &record.id,
            serde_json::to_value(versioned).map_err(|_| invalid("profile"))?,
            &mut create.profiles,
            &mut reuse.profiles,
            "profile",
        )?;
    }
    Ok(())
}

fn merge_workspaces(
    input: &ControlImportInput,
    accounts: &BTreeMap<String, (Account, bool)>,
    aggregate: &mut Value,
    create: &mut Counts,
    reuse: &mut Counts,
) -> Result<(), Problem> {
    let mut organizations = BTreeMap::<String, (Organization, OrganizationPolicy)>::new();
    for record in &input.organizations {
        require_record(&record.schema, "buglogin.organization/1", record.revision)?;
        let id = OrganizationId::from_str(&record.id).map_err(|_| invalid("organization ID"))?;
        let account = accounts
            .get(&record.data.account_id)
            .ok_or_else(|| invalid("organization account reference"))?;
        validate_name(&record.data.name, "organization name")?;
        let created_at =
            required_timestamp(record.created_at.as_deref(), "organization createdAt")?;
        let offline = input
            .workspaces
            .iter()
            .filter(|workspace| workspace.data.organization_id == record.id)
            .map(|workspace| workspace.data.policy.offline_entitled)
            .collect::<BTreeSet<_>>();
        if offline.len() > 1 {
            return Err(invalid("organization entitlement policy"));
        }
        organizations.insert(
            record.id.clone(),
            (
                Organization {
                    id,
                    account_id: account.0.id.clone(),
                    name: record.data.name.clone(),
                    created_at,
                },
                OrganizationPolicy {
                    max_workspaces: input
                        .workspaces
                        .iter()
                        .filter(|workspace| workspace.data.organization_id == record.id)
                        .count()
                        .try_into()
                        .map_err(|_| invalid("organization workspace count"))?,
                    offline_entitled: offline.into_iter().next().unwrap_or(false),
                },
            ),
        );
    }
    let mut workspace_models = Vec::new();
    for record in &input.workspaces {
        require_record(&record.schema, "buglogin.workspace/1", record.revision)?;
        let workspace_id =
            WorkspaceId::from_str(&record.id).map_err(|_| invalid("workspace ID"))?;
        let organization_id = OrganizationId::from_str(&record.data.organization_id)
            .map_err(|_| invalid("organization ID"))?;
        validate_name(&record.data.name, "workspace name")?;
        let created_at = required_timestamp(record.created_at.as_deref(), "workspace createdAt")?;
        if !organizations.contains_key(&record.data.organization_id) {
            return Err(invalid("workspace organization reference"));
        }
        workspace_models.push(Workspace {
            id: workspace_id,
            organization_id,
            name: record.data.name.clone(),
            policy: WorkspacePolicy {
                max_profiles: record.data.policy.max_profiles,
                max_members: record.data.policy.max_members,
                max_storage_bytes: record.data.policy.max_storage_bytes,
                offline_entitled: record.data.policy.offline_entitled,
                default_extension_group_id: None,
            },
            created_at,
        });
    }

    let organization_map = object_path_mut(aggregate, &["workspaces", "organizations"])?;
    for (id, (organization, policy)) in organizations {
        match organization_map.get_mut(&id) {
            Some(existing) => {
                let organization_value =
                    serde_json::to_value(&organization).map_err(|_| invalid("organization"))?;
                let policy_value =
                    serde_json::to_value(&policy).map_err(|_| invalid("organization policy"))?;
                if existing.get("organization") != Some(&organization_value)
                    || existing.get("policy") != Some(&policy_value)
                {
                    return Err(collision("organization"));
                }
                reuse.organizations += 1;
            }
            None => {
                organization_map.insert(
                    id,
                    json!({"organization": organization, "policy": policy, "workspaces": {}}),
                );
                create.organizations += 1;
            }
        }
    }
    for workspace in workspace_models {
        let workspaces = organization_map
            .get_mut(workspace.organization_id.as_str())
            .and_then(Value::as_object_mut)
            .and_then(|record| record.get_mut("workspaces"))
            .and_then(Value::as_object_mut)
            .ok_or_else(|| invalid("workspace collection"))?;
        merge_exact(
            workspaces,
            workspace.id.as_str(),
            serde_json::to_value(&workspace).map_err(|_| invalid("workspace"))?,
            &mut create.workspaces,
            &mut reuse.workspaces,
            "workspace",
        )?;
    }
    Ok(())
}

fn merge_members(
    input: &ControlImportInput,
    aggregate: &mut Value,
    create: &mut Counts,
    reuse: &mut Counts,
) -> Result<(), Problem> {
    let members = object_path_mut(aggregate, &["members", "members"])?;
    for record in &input.members {
        require_record(&record.schema, "buglogin.member/1", record.revision)?;
        let member = Member {
            id: MemberId::from_str(&record.id).map_err(|_| invalid("member ID"))?,
            account_id: AccountId::from_str(&record.data.account_id)
                .map_err(|_| invalid("member account ID"))?,
            workspace_id: WorkspaceId::from_str(&record.data.workspace_id)
                .map_err(|_| invalid("member workspace ID"))?,
            team_id: record
                .data
                .team_id
                .as_deref()
                .map(TeamId::from_str)
                .transpose()
                .map_err(|_| invalid("member team ID"))?,
            role: parse_member_role(&record.data.role)?,
            status: parse_member_status(&record.data.status)?,
            invite_expires_at: None,
        };
        if member.status == MemberStatus::Invited {
            return Err(invalid("invited member without expiry"));
        }
        merge_exact(
            members,
            &record.id,
            serde_json::to_value(member).map_err(|_| invalid("member"))?,
            &mut create.members,
            &mut reuse.members,
            "member",
        )?;
    }
    let mut active_memberships = BTreeSet::new();
    for value in members.values() {
        let member: Member =
            serde_json::from_value(value.clone()).map_err(|_| invalid("member collection"))?;
        if member.status == MemberStatus::Active
            && !active_memberships.insert((member.account_id, member.workspace_id))
        {
            return Err(invalid("duplicate active account workspace membership"));
        }
    }
    Ok(())
}

fn merge_teams(
    input: &ControlImportInput,
    aggregate: &mut Value,
    create: &mut Counts,
    reuse: &mut Counts,
) -> Result<(), Problem> {
    let teams = object_path_mut(aggregate, &["teams", "teams"])?;
    for record in &input.teams {
        require_record(&record.schema, "buglogin.team/1", record.revision)?;
        validate_name(&record.data.name, "team name")?;
        let team = Team {
            id: TeamId::from_str(&record.id).map_err(|_| invalid("team ID"))?,
            workspace_id: WorkspaceId::from_str(&record.data.workspace_id)
                .map_err(|_| invalid("team workspace ID"))?,
            name: record.data.name.clone(),
            member_ids: record
                .data
                .member_ids
                .iter()
                .map(|id| MemberId::from_str(id).map_err(|_| invalid("team member ID")))
                .collect::<Result<_, _>>()?,
        };
        merge_exact(
            teams,
            &record.id,
            json!({"team": team, "revision": 1}),
            &mut create.teams,
            &mut reuse.teams,
            "team",
        )?;
    }
    Ok(())
}

fn merge_resources(
    input: &ControlImportInput,
    aggregate: &mut Value,
    create: &mut Counts,
    reuse: &mut Counts,
) -> Result<(), Problem> {
    let resources = object_path_mut(aggregate, &["resources", "resources"])?;
    for record in &input.resources {
        require_record(&record.schema, "buglogin.resource/1", record.revision)?;
        validate_name(&record.data.display_name, "resource display name")?;
        let resource = Resource {
            id: ResourceId::from_str(&record.id).map_err(|_| invalid("resource ID"))?,
            workspace_id: WorkspaceId::from_str(&record.data.workspace_id)
                .map_err(|_| invalid("resource workspace ID"))?,
            kind: parse_resource_kind(&record.data.kind)?,
            state: parse_resource_state(&record.data.state)?,
            display_name: record.data.display_name.clone(),
        };
        merge_exact(
            resources,
            &record.id,
            serde_json::to_value(resource).map_err(|_| invalid("resource"))?,
            &mut create.resources,
            &mut reuse.resources,
            "resource",
        )?;
    }
    Ok(())
}

fn merge_resource_grants(
    input: &ControlImportInput,
    aggregate: &mut Value,
    create: &mut Counts,
    reuse: &mut Counts,
) -> Result<(), Problem> {
    let grants = object_path_mut(aggregate, &["resources", "grants"])?;
    for record in &input.grants {
        require_record(&record.schema, "buglogin.grant/1", record.revision)?;
        let grant = ResourceGrant {
            id: GrantId::from_str(&record.id).map_err(|_| invalid("grant ID"))?,
            workspace_id: WorkspaceId::from_str(&record.data.workspace_id)
                .map_err(|_| invalid("grant workspace ID"))?,
            resource_id: ResourceId::from_str(&record.data.resource_id)
                .map_err(|_| invalid("grant resource ID"))?,
            member_id: MemberId::from_str(&record.data.member_id)
                .map_err(|_| invalid("grant member ID"))?,
            action: parse_action(&record.data.action)?,
            effect: parse_effect(&record.data.effect)?,
        };
        merge_exact(
            grants,
            &record.id,
            serde_json::to_value(grant).map_err(|_| invalid("grant"))?,
            &mut create.grants,
            &mut reuse.grants,
            "grant",
        )?;
    }
    Ok(())
}

fn merge_profile_resources(
    input: &ControlImportInput,
    aggregate: &mut Value,
    create: &mut Counts,
    reuse: &mut Counts,
) -> Result<(), Problem> {
    let resources = object_path_mut(aggregate, &["resources", "resources"])?.clone();
    let bindings = object_path_mut(aggregate, &["profile_resources"])?;
    for record in &input.profile_resources {
        require_record(
            &record.schema,
            "buglogin.profile-resource/1",
            record.revision,
        )?;
        let profile_id = ProfileId::from_str(&record.data.profile_id)
            .map_err(|_| invalid("profile resource profile ID"))?;
        if !input
            .profiles
            .iter()
            .any(|profile| profile.id == record.data.profile_id)
        {
            return Err(invalid("profile resource profile reference"));
        }
        let resource_id = ResourceId::from_str(&record.data.resource_id)
            .map_err(|_| invalid("profile resource resource ID"))?;
        let workspace_id = WorkspaceId::from_str(&record.data.workspace_id)
            .map_err(|_| invalid("profile resource workspace ID"))?;
        let resource: Resource = serde_json::from_value(
            resources
                .get(&record.data.resource_id)
                .cloned()
                .ok_or_else(|| invalid("profile resource resource reference"))?,
        )
        .map_err(|_| invalid("profile resource resource reference"))?;
        if resource.kind != ResourceKind::Profile {
            return Err(invalid("profile resource kind"));
        }
        if resource.workspace_id != workspace_id {
            return Err(invalid("profile resource workspace reference"));
        }
        for value in bindings.values() {
            let existing: ProfileResource =
                serde_json::from_value(value.clone()).map_err(|_| invalid("profile resource"))?;
            if existing.resource_id == resource_id && existing.profile_id != profile_id {
                return Err(collision("profile resource binding"));
            }
        }
        let binding = ProfileResource {
            profile_id,
            resource_id,
            workspace_id,
            archive_state: parse_profile_archive_state(&record.data.archive_state)?,
        };
        let binding_key = binding.profile_id.as_str().to_owned();
        merge_exact(
            bindings,
            &binding_key,
            serde_json::to_value(binding).map_err(|_| invalid("profile resource"))?,
            &mut create.profile_resources,
            &mut reuse.profile_resources,
            "profile resource",
        )?;
    }
    Ok(())
}

fn materialize_membership_grants(
    input: &ControlImportInput,
    accounts: &BTreeMap<String, (Account, bool)>,
    aggregate: &mut Value,
) -> Result<(), Problem> {
    let mut existing: Vec<Grant> = serde_json::from_value(
        aggregate
            .get("grants")
            .cloned()
            .ok_or_else(|| invalid("permission grants"))?,
    )
    .map_err(|_| invalid("permission grants"))?;
    let mut materialized: BTreeSet<MemberId> = serde_json::from_value(
        aggregate
            .get("materialized_member_grants")
            .cloned()
            .unwrap_or_else(|| json!([])),
    )
    .map_err(|_| invalid("materialized member grants"))?;

    for record in &input.members {
        let member_id = MemberId::from_str(&record.id).map_err(|_| invalid("member ID"))?;
        let account_id = &accounts
            .get(&record.data.account_id)
            .ok_or_else(|| invalid("member account reference"))?
            .0
            .id;
        let workspace_id = WorkspaceId::from_str(&record.data.workspace_id)
            .map_err(|_| invalid("member workspace ID"))?;
        let override_value = record
            .extensions
            .get("legacyCustomCapabilities")
            .ok_or_else(|| invalid("membership customCapabilities"))?;
        for grant in materialized_grants(
            &member_id,
            account_id,
            &workspace_id,
            &parse_member_role(&record.data.role)?,
            override_value,
        )? {
            match existing.iter().find(|item| item.id == grant.id) {
                Some(item) if item == &grant => {}
                Some(_) => return Err(collision("membership grant")),
                None => existing.push(grant),
            }
        }
        materialized.insert(member_id);
    }
    aggregate["grants"] =
        serde_json::to_value(existing).map_err(|_| invalid("permission grants"))?;
    aggregate["materialized_member_grants"] =
        serde_json::to_value(materialized).map_err(|_| invalid("materialized member grants"))?;
    Ok(())
}

fn materialized_grants(
    member_id: &MemberId,
    account_id: &AccountId,
    workspace_id: &WorkspaceId,
    role: &MemberRole,
    override_value: &Value,
) -> Result<Vec<Grant>, Problem> {
    let universe = MEMBERSHIP_CAPABILITIES
        .iter()
        .flat_map(|capability| ALL_ACTIONS.map(|action| ((*capability).to_owned(), action)))
        .collect::<BTreeSet<_>>();
    let allowed = if override_value.is_null() {
        role_default_pairs(role)
    } else {
        override_value
            .as_array()
            .ok_or_else(|| invalid("membership customCapabilities"))?
            .iter()
            .map(|action| {
                action
                    .as_str()
                    .ok_or_else(|| invalid("membership custom capability"))
                    .and_then(legacy_action_pair)
            })
            .collect::<Result<BTreeSet<_>, _>>()?
    };
    let explicit = !override_value.is_null();
    universe
        .into_iter()
        .filter_map(|(capability, action)| {
            let allowed_pair = allowed.contains(&(capability.clone(), action));
            if !explicit && !allowed_pair {
                return None;
            }
            Some(permission_grant(
                member_id,
                account_id,
                workspace_id,
                &capability,
                action,
                if allowed_pair {
                    Effect::Allow
                } else {
                    Effect::Deny
                },
            ))
        })
        .collect()
}

fn permission_grant(
    member_id: &MemberId,
    account_id: &AccountId,
    workspace_id: &WorkspaceId,
    capability_name: &str,
    action: Action,
    effect: Effect,
) -> Result<Grant, Problem> {
    let effect_name = if effect == Effect::Allow {
        "allow"
    } else {
        "deny"
    };
    Ok(Grant {
        id: GrantId::new(format!(
            "migration:{}:{}:{}:{effect_name}",
            member_id.as_str(),
            capability_name,
            action_name(action)
        ))
        .map_err(|_| invalid("membership grant ID"))?,
        account_id: Some(account_id.clone()),
        member_id: None,
        capability: Capability::new(capability_name)
            .map_err(|_| invalid("membership capability"))?,
        action,
        scope: ResourceScope::workspace(workspace_id.clone()),
        effect,
    })
}

pub(super) fn role_default_pairs(role: &MemberRole) -> BTreeSet<(String, Action)> {
    const OPERATE: [Action; 5] = [
        Action::Read,
        Action::Create,
        Action::Update,
        Action::Execute,
        Action::Share,
    ];
    const USE: [Action; 2] = [Action::Read, Action::Execute];
    let mut defaults = BTreeSet::new();
    for capability in MEMBERSHIP_CAPABILITIES {
        let actions: &[Action] = match role {
            MemberRole::Owner | MemberRole::Admin => &ALL_ACTIONS,
            MemberRole::Lead => match capability {
                "CAP-WS-001" | "CAP-AUD-001" | "events.read" | "operation.read"
                | "profile.read" => &[Action::Read],
                "CAP-MEM-001" => &[Action::Read, Action::Create, Action::Manage],
                "CAP-TEAM-001" => &[Action::Read, Action::Create, Action::Update, Action::Manage],
                _ => &OPERATE,
            },
            MemberRole::Operator => match capability {
                "CAP-WS-001" | "CAP-MEM-001" | "CAP-TEAM-001" | "CAP-AUD-001" | "events.read"
                | "operation.read" | "profile.read" => &[Action::Read],
                _ => &OPERATE,
            },
            MemberRole::Member => match capability {
                "CAP-WS-001" | "CAP-MEM-001" | "CAP-TEAM-001" | "CAP-AUD-001" | "events.read"
                | "operation.read" | "profile.read" => &[Action::Read],
                _ => &USE,
            },
        };
        defaults.extend(
            actions
                .iter()
                .map(|action| (capability.to_owned(), *action)),
        );
    }
    defaults
}

fn legacy_action_pair(value: &str) -> Result<(String, Action), Problem> {
    let pair = match value {
        "create_profile" | "clone_profile" => ("CAP-PROF-001", Action::Create),
        "rename_profile" => ("CAP-PROF-001", Action::Update),
        "delete_profile" | "delete_selected_profiles" => ("CAP-PROF-001", Action::Delete),
        "stop_profile" => ("CAP-PROF-002", Action::Execute),
        "assign_group" | "update_profile_tags" | "update_profile_note" => {
            ("CAP-PROF-003", Action::Update)
        }
        "assign_proxy" => ("CAP-NET-001", Action::Update),
        "assign_extension_group" => ("CAP-EXT-002", Action::Update),
        "toggle_profile_sync" => ("CAP-SYNC-001", Action::Update),
        "update_profile_vpn" => ("CAP-NET-003", Action::Update),
        "share_profile" => ("CAP-PROF-001", Action::Share),
        "share_group" => ("CAP-PROF-003", Action::Share),
        "export_profile" => ("CAP-PROF-001", Action::Execute),
        "force_take_lock" => ("CAP-PROF-002", Action::Manage),
        _ => return Err(invalid("membership custom capability")),
    };
    Ok((pair.0.into(), pair.1))
}

fn validate_graph(input: &ControlImportInput) -> Result<(), Problem> {
    let accounts = unique_ids(
        input.accounts.iter().map(|record| record.id.as_str()),
        "account",
    )?;
    let organizations = unique_ids(
        input.organizations.iter().map(|record| record.id.as_str()),
        "organization",
    )?;
    let workspaces = unique_ids(
        input.workspaces.iter().map(|record| record.id.as_str()),
        "workspace",
    )?;
    unique_ids(input.teams.iter().map(|record| record.id.as_str()), "team")?;
    let members = unique_ids(
        input.members.iter().map(|record| record.id.as_str()),
        "member",
    )?;
    let resources = unique_ids(
        input.resources.iter().map(|record| record.id.as_str()),
        "resource",
    )?;
    let profiles = unique_ids(
        input.profiles.iter().map(|record| record.id.as_str()),
        "profile",
    )?;
    unique_ids(
        input
            .profile_resources
            .iter()
            .map(|record| record.id.as_str()),
        "profile resource",
    )?;
    unique_ids(
        input
            .profile_resources
            .iter()
            .map(|record| record.data.resource_id.as_str()),
        "profile resource binding",
    )?;
    unique_ids(
        input.grants.iter().map(|record| record.id.as_str()),
        "grant",
    )?;
    let group_workspaces = input
        .groups
        .iter()
        .filter_map(|group| {
            group
                .data
                .workspace_id
                .as_deref()
                .map(|workspace| (group.id.as_str(), workspace))
        })
        .collect::<BTreeMap<_, _>>();
    unique_ids(
        input.groups.iter().map(|record| record.id.as_str()),
        "group",
    )?;
    unique_ids(
        input.group_grants.iter().map(|record| record.id.as_str()),
        "group grant",
    )?;
    for organization in &input.organizations {
        if !accounts.contains(organization.data.account_id.as_str()) {
            return Err(invalid("organization account reference"));
        }
    }
    for workspace in &input.workspaces {
        if !organizations.contains(workspace.data.organization_id.as_str()) {
            return Err(invalid("workspace organization reference"));
        }
    }
    let team_workspaces = input
        .teams
        .iter()
        .map(|team| (team.id.as_str(), team.data.workspace_id.as_str()))
        .collect::<BTreeMap<_, _>>();
    let member_workspaces = input
        .members
        .iter()
        .map(|member| (member.id.as_str(), member.data.workspace_id.as_str()))
        .collect::<BTreeMap<_, _>>();
    for member in &input.members {
        if !accounts.contains(member.data.account_id.as_str())
            || !workspaces.contains(member.data.workspace_id.as_str())
        {
            return Err(invalid("member reference"));
        }
        if let Some(team_id) = member.data.team_id.as_deref()
            && team_workspaces.get(team_id).copied() != Some(member.data.workspace_id.as_str())
        {
            return Err(invalid("member team reference"));
        }
    }
    for team in &input.teams {
        if !workspaces.contains(team.data.workspace_id.as_str()) {
            return Err(invalid("team workspace reference"));
        }
        for member_id in &team.data.member_ids {
            if !members.contains(member_id.as_str())
                || member_workspaces.get(member_id.as_str()).copied()
                    != Some(team.data.workspace_id.as_str())
            {
                return Err(invalid("team member reference"));
            }
        }
    }
    let resource_workspaces = input
        .resources
        .iter()
        .map(|resource| (resource.id.as_str(), resource.data.workspace_id.as_str()))
        .collect::<BTreeMap<_, _>>();
    for resource in &input.resources {
        if !workspaces.contains(resource.data.workspace_id.as_str()) {
            return Err(invalid("resource workspace reference"));
        }
    }
    for binding in &input.profile_resources {
        if !profiles.contains(binding.data.profile_id.as_str())
            || !resources.contains(binding.data.resource_id.as_str())
            || resource_workspaces
                .get(binding.data.resource_id.as_str())
                .copied()
                != Some(binding.data.workspace_id.as_str())
        {
            return Err(invalid("profile resource reference"));
        }
    }
    for grant in &input.grants {
        if !resources.contains(grant.data.resource_id.as_str())
            || !members.contains(grant.data.member_id.as_str())
            || resource_workspaces
                .get(grant.data.resource_id.as_str())
                .copied()
                != Some(grant.data.workspace_id.as_str())
            || member_workspaces
                .get(grant.data.member_id.as_str())
                .copied()
                != Some(grant.data.workspace_id.as_str())
        {
            return Err(invalid("grant reference"));
        }
    }
    for grant in &input.group_grants {
        if !workspaces.contains(grant.data.workspace_id.as_str())
            || !members.contains(grant.data.member_id.as_str())
            || member_workspaces
                .get(grant.data.member_id.as_str())
                .copied()
                != Some(grant.data.workspace_id.as_str())
            || group_workspaces.get(grant.data.group_id.as_str()).copied()
                != Some(grant.data.workspace_id.as_str())
        {
            return Err(invalid("group grant reference"));
        }
    }
    for profile in &input.profiles {
        if profile
            .data
            .group_id
            .as_deref()
            .is_some_and(|group_id| !group_workspaces.contains_key(group_id))
        {
            return Err(invalid("profile group reference"));
        }
    }
    Ok(())
}

fn mark_managed_group_share_resources_pending(
    before: &ControlAggregate,
    aggregate: &mut Value,
) -> Result<(), Problem> {
    let after_groups: crate::services::groups::GroupService = serde_json::from_value(
        aggregate
            .get("groups")
            .cloned()
            .ok_or_else(|| invalid("groups"))?,
    )
    .map_err(|_| invalid("groups"))?;
    let after_profiles: BTreeMap<ProfileId, buglogin_storage::local::Versioned<Profile>> =
        serde_json::from_value(
            aggregate
                .get("profiles")
                .cloned()
                .ok_or_else(|| invalid("profiles"))?,
        )
        .map_err(|_| invalid("profiles"))?;
    let after_bindings: BTreeMap<ProfileId, ProfileResource> = serde_json::from_value(
        aggregate
            .get("profile_resources")
            .cloned()
            .ok_or_else(|| invalid("profile resources"))?,
    )
    .map_err(|_| invalid("profile resources"))?;
    let sync_manifests: BTreeMap<String, SyncManifest> = serde_json::from_value(
        aggregate
            .get("sync_manifests")
            .cloned()
            .unwrap_or_else(|| json!({})),
    )
    .map_err(|_| invalid("sync manifests"))?;

    let before_grants = before.groups.all_share_grants().collect::<Vec<_>>();
    let after_grants = after_groups.all_share_grants().collect::<Vec<_>>();
    let changed_group_grants = changed_group_share_key_rotation_groups(
        before_grants.iter().copied(),
        after_grants.iter().copied(),
    );
    let all_key_rotation_groups =
        group_share_key_rotation_groups(before_grants.iter().chain(after_grants.iter()).copied());

    let assignments = after_profiles.iter().map(|(profile_id, profile)| {
        (
            profile_id.as_str(),
            profile
                .value
                .group_ids
                .iter()
                .map(|group_id| (profile_id.as_str(), Some(group_id.as_str())))
                .collect::<Vec<_>>(),
        )
    });
    let assigned_groups = assignments.flat_map(|(_, group_ids)| group_ids);
    let resource_bindings = after_bindings.values().map(|binding| {
        (
            binding.profile_id.as_str(),
            binding.workspace_id.as_str(),
            binding.resource_id.as_str(),
        )
    });
    let mut keys = pending_group_share_resource_keys(
        changed_group_grants
            .iter()
            .map(|(workspace_id, group_id)| (workspace_id.as_str(), group_id.as_str())),
        assigned_groups,
        resource_bindings,
    );

    let profile_ids = before
        .profiles
        .keys()
        .chain(after_profiles.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    for profile_id in profile_ids {
        let previous_groups = before
            .profiles
            .get(&profile_id)
            .map(|profile| &profile.value.group_ids);
        let current_groups = after_profiles
            .get(&profile_id)
            .map(|profile| &profile.value.group_ids);
        if previous_groups == current_groups {
            continue;
        }
        let Some(binding) = after_bindings.get(&profile_id) else {
            continue;
        };
        let workspace_id = binding.workspace_id.as_str();
        let mut groups = previous_groups
            .into_iter()
            .flat_map(|groups| groups.iter())
            .chain(current_groups.into_iter().flat_map(|groups| groups.iter()));
        if groups.any(|group_id| {
            all_key_rotation_groups
                .contains(&(workspace_id.to_owned(), group_id.as_str().to_owned()))
        }) {
            keys.insert(format!("{}:{}", workspace_id, binding.resource_id.as_str()));
        }
    }
    keys.retain(|key| {
        sync_manifests
            .get(key)
            .is_some_and(|manifest| manifest.payload_encryption.is_some())
    });
    let mut pending: BTreeSet<String> = serde_json::from_value(
        aggregate
            .get("resource_key_rotation_pending")
            .cloned()
            .unwrap_or_else(|| json!([])),
    )
    .map_err(|_| invalid("pending profile key rotations"))?;

    pending.extend(keys);
    aggregate["resource_key_rotation_pending"] =
        serde_json::to_value(pending).map_err(|_| invalid("pending profile key rotations"))?;
    Ok(())
}

fn requires_group_share_key_rotation(action: buglogin_domain::permissions::Action) -> bool {
    matches!(
        action,
        buglogin_domain::permissions::Action::Read | buglogin_domain::permissions::Action::Execute
    )
}

fn changed_group_share_key_rotation_groups<'a>(
    before: impl Iterator<Item = &'a GroupShareGrant>,
    after: impl Iterator<Item = &'a GroupShareGrant>,
) -> BTreeSet<(String, String)> {
    let before = before
        .map(|grant| (grant.id.clone(), grant))
        .collect::<BTreeMap<_, _>>();
    let after = after
        .map(|grant| (grant.id.clone(), grant))
        .collect::<BTreeMap<_, _>>();
    let grant_ids = before
        .keys()
        .chain(after.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    grant_ids
        .iter()
        .filter_map(|grant_id| {
            let previous = before.get(grant_id).copied();
            let current = after.get(grant_id).copied();
            (previous != current).then_some([previous, current])
        })
        .flat_map(|grants| grants.into_iter().flatten())
        .filter(|grant| requires_group_share_key_rotation(grant.action))
        .map(|grant| {
            (
                grant.workspace_id.as_str().to_owned(),
                grant.group_id.as_str().to_owned(),
            )
        })
        .collect()
}

fn group_share_key_rotation_groups<'a>(
    grants: impl Iterator<Item = &'a GroupShareGrant>,
) -> BTreeSet<(String, String)> {
    grants
        .filter(|grant| requires_group_share_key_rotation(grant.action))
        .map(|grant| {
            (
                grant.workspace_id.as_str().to_owned(),
                grant.group_id.as_str().to_owned(),
            )
        })
        .collect()
}

fn pending_group_share_resource_keys<'a>(
    key_rotation_groups: impl Iterator<Item = (&'a str, &'a str)>,
    assigned_profiles: impl Iterator<Item = (&'a str, Option<&'a str>)>,
    bindings: impl Iterator<Item = (&'a str, &'a str, &'a str)>,
) -> BTreeSet<String> {
    let key_rotation_groups = key_rotation_groups.collect::<BTreeSet<_>>();
    let assigned_groups = assigned_profiles
        .filter_map(|(profile_id, group_id)| group_id.map(|group_id| (profile_id, group_id)))
        .collect::<BTreeSet<_>>();
    bindings
        .filter_map(|(profile_id, workspace_id, resource_id)| {
            key_rotation_groups
                .iter()
                .any(|(grant_workspace_id, grant_group_id)| {
                    *grant_workspace_id == workspace_id
                        && assigned_groups.contains(&(profile_id, *grant_group_id))
                })
                .then(|| format!("{workspace_id}:{resource_id}"))
        })
        .collect()
}

fn unique_ids<'a>(
    ids: impl Iterator<Item = &'a str>,
    kind: &str,
) -> Result<BTreeSet<&'a str>, Problem> {
    let mut unique = BTreeSet::new();
    for id in ids {
        if !unique.insert(id) {
            return Err(invalid(&format!("duplicate {kind} ID")));
        }
    }
    Ok(unique)
}

fn empty_counts() -> Counts {
    Counts {
        profiles: 0,
        accounts: 0,
        organizations: 0,
        workspaces: 0,
        teams: 0,
        members: 0,
        resources: 0,
        profile_resources: 0,
        grants: 0,
        groups: 0,
        group_grants: 0,
    }
}

fn insert_set(root: &mut Value, field: &str, value: &str) -> Result<(), Problem> {
    let array = root
        .get_mut(field)
        .and_then(Value::as_array_mut)
        .ok_or_else(|| invalid(field))?;
    if !array.iter().any(|item| item.as_str() == Some(value)) {
        array.push(Value::String(value.to_owned()));
        array.sort_by(|left, right| left.as_str().cmp(&right.as_str()));
    }
    Ok(())
}

fn object_path_mut<'a>(
    root: &'a mut Value,
    path: &[&str],
) -> Result<&'a mut Map<String, Value>, Problem> {
    let mut current = root;
    for segment in path {
        current = current
            .as_object_mut()
            .and_then(|object| object.get_mut(*segment))
            .ok_or_else(|| invalid("control aggregate path"))?;
    }
    current
        .as_object_mut()
        .ok_or_else(|| invalid("control aggregate object"))
}

fn merge_exact(
    target: &mut Map<String, Value>,
    id: &str,
    value: Value,
    created: &mut usize,
    reused: &mut usize,
    kind: &str,
) -> Result<(), Problem> {
    match target.get(id) {
        Some(existing) if existing == &value => *reused += 1,
        Some(_) => return Err(collision(kind)),
        None => {
            target.insert(id.to_owned(), value);
            *created += 1;
        }
    }
    Ok(())
}

fn require_record(schema: &str, expected: &str, revision: u32) -> Result<(), Problem> {
    if schema == expected && revision == 1 {
        Ok(())
    } else {
        Err(invalid("record schema"))
    }
}

fn require_versioned_record(schema: &str, expected: &str, revision: u32) -> Result<(), Problem> {
    if schema == expected && revision > 0 {
        Ok(())
    } else {
        Err(invalid("record schema or revision"))
    }
}

fn optional_timestamp(value: Option<&str>, field: &str) -> Result<Option<String>, Problem> {
    value
        .map(|value| {
            OffsetDateTime::parse(value, &Rfc3339).map_err(|_| invalid(field))?;
            Ok(value.to_owned())
        })
        .transpose()
}

fn required_timestamp(value: Option<&str>, field: &str) -> Result<String, Problem> {
    let value = value.ok_or_else(|| invalid(field))?;
    OffsetDateTime::parse(value, &Rfc3339).map_err(|_| invalid(field))?;
    Ok(value.to_owned())
}

fn validate_name(value: &str, field: &str) -> Result<(), Problem> {
    if value.trim().is_empty() || value.len() > 256 {
        Err(invalid(field))
    } else {
        Ok(())
    }
}

fn parse_member_role(value: &str) -> Result<MemberRole, Problem> {
    match value {
        "owner" => Ok(MemberRole::Owner),
        "admin" => Ok(MemberRole::Admin),
        "lead" => Ok(MemberRole::Lead),
        "operator" => Ok(MemberRole::Operator),
        "member" => Ok(MemberRole::Member),
        _ => Err(invalid("member role")),
    }
}

fn parse_member_status(value: &str) -> Result<MemberStatus, Problem> {
    match value {
        "invited" => Ok(MemberStatus::Invited),
        "active" => Ok(MemberStatus::Active),
        "suspended" => Ok(MemberStatus::Suspended),
        _ => Err(invalid("member status")),
    }
}

fn parse_resource_kind(value: &str) -> Result<ResourceKind, Problem> {
    match value {
        "profile" => Ok(ResourceKind::Profile),
        "proxy" => Ok(ResourceKind::Proxy),
        "vpn" => Ok(ResourceKind::Vpn),
        "cookie_set" | "cookieset" => Ok(ResourceKind::CookieSet),
        "extension" => Ok(ResourceKind::Extension),
        "runtime" => Ok(ResourceKind::Runtime),
        _ => Err(invalid("resource kind")),
    }
}

fn parse_resource_state(value: &str) -> Result<ResourceState, Problem> {
    match value {
        "active" => Ok(ResourceState::Active),
        "archived" => Ok(ResourceState::Archived),
        "locked" => Ok(ResourceState::Locked),
        _ => Err(invalid("resource state")),
    }
}

fn parse_profile_archive_state(value: &str) -> Result<ProfileArchiveState, Problem> {
    match value {
        "active" => Ok(ProfileArchiveState::Active),
        "archived" => Ok(ProfileArchiveState::Archived),
        _ => Err(invalid("profile resource archive state")),
    }
}

fn parse_action(value: &str) -> Result<Action, Problem> {
    match value {
        "read" => Ok(Action::Read),
        "create" => Ok(Action::Create),
        "update" => Ok(Action::Update),
        "delete" => Ok(Action::Delete),
        "execute" => Ok(Action::Execute),
        "share" => Ok(Action::Share),
        "manage" => Ok(Action::Manage),
        _ => Err(invalid("grant action")),
    }
}

fn parse_effect(value: &str) -> Result<Effect, Problem> {
    match value {
        "allow" => Ok(Effect::Allow),
        "deny" => Ok(Effect::Deny),
        _ => Err(invalid("grant effect")),
    }
}

fn action_name(action: Action) -> &'static str {
    match action {
        Action::Read => "read",
        Action::Create => "create",
        Action::Update => "update",
        Action::Delete => "delete",
        Action::Execute => "execute",
        Action::Share => "share",
        Action::Manage => "manage",
    }
}

pub(super) fn validate_package_id(value: &str) -> Result<(), Problem> {
    if !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
        Ok(())
    } else {
        Err(invalid("package ID"))
    }
}

fn package_checksum(input: &ControlImportInput) -> Result<String, Problem> {
    let mut value = serde_json::to_value(input).map_err(|_| invalid("control import package"))?;
    value
        .as_object_mut()
        .ok_or_else(|| invalid("control import package"))?
        .remove("dryRun");
    value_checksum(&value)
}

fn value_checksum(value: &impl Serialize) -> Result<String, Problem> {
    serde_json::to_vec(value)
        .map(|bytes| hex_digest(&bytes))
        .map_err(|_| invalid("checksum value"))
}

fn hex_digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub(super) struct ControlImportPackageLock<'a> {
    state: &'a ApiState,
    lock_id: i64,
}

impl<'a> ControlImportPackageLock<'a> {
    pub(super) fn acquire(state: &'a ApiState, package_id: &str) -> Result<Self, Problem> {
        let lock_id = control_import_lock_id(package_id);
        if !state.control_persistence.try_advisory_lock(lock_id)? {
            return Err(problem(
                "migration_control_import_in_progress",
                "Control import or rollback for this package is already in progress",
                409,
            ));
        }
        Ok(Self { state, lock_id })
    }

    fn matches(&self, package_id: &str) -> bool {
        self.lock_id == control_import_lock_id(package_id)
    }
}

fn control_import_lock_id(package_id: &str) -> i64 {
    let digest =
        Sha256::digest(format!("buglogin.control-import-package-lock.v1:{package_id}").as_bytes());
    let mut lock_bytes = [0_u8; 8];
    lock_bytes.copy_from_slice(&digest[..8]);
    i64::from_be_bytes(lock_bytes)
}

impl Drop for ControlImportPackageLock<'_> {
    fn drop(&mut self) {
        let _ = self
            .state
            .control_persistence
            .release_advisory_lock(self.lock_id);
    }
}

fn invalid(field: &str) -> Problem {
    problem(
        "migration_control_import_invalid",
        "Control import data is invalid",
        422,
    )
    .with_field("field", field)
}

fn collision(entity: &str) -> Problem {
    problem(
        "migration_control_import_collision",
        "Control import collides with destination data",
        409,
    )
    .with_field("entity", entity)
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use buglogin_domain::{
        entities::Runtime,
        grant::GroupShareGrant,
        ids::{GrantId, GroupId, MemberId, WorkspaceId},
        member::MemberRole,
        permissions::{Action, Effect},
    };
    use buglogin_migration::package::NeutralPackage;

    use super::{
        changed_group_share_key_rotation_groups, control_import_lock_id, migrated_profile_runtime,
        pending_group_share_resource_keys, reject_unorchestrated_sync_package,
        requires_group_share_key_rotation, role_default_pairs,
    };

    #[test]
    fn sync_package_never_reports_false_control_import_success() {
        let package = NeutralPackage::from_files(BTreeMap::from([(
            "sync/export.json".to_owned(),
            b"{}".to_vec(),
        )]));
        assert_eq!(
            reject_unorchestrated_sync_package(&package)
                .unwrap_err()
                .code,
            "migration_sync_import_not_ready"
        );
    }

    #[test]
    fn lead_defaults_are_workspace_scoped_and_cannot_delete_or_manage_policy() {
        let defaults = role_default_pairs(&MemberRole::Lead);
        assert!(defaults.contains(&("CAP-MEM-001".into(), Action::Manage)));
        assert!(defaults.contains(&("CAP-MEM-001".into(), Action::Create)));
        assert!(defaults.contains(&("CAP-TEAM-001".into(), Action::Manage)));
        assert!(defaults.contains(&("CAP-PROF-001".into(), Action::Update)));
        assert!(!defaults.contains(&("CAP-MEM-001".into(), Action::Delete)));
        assert!(!defaults.contains(&("CAP-WS-001".into(), Action::Manage)));
        assert!(!defaults.contains(&("CAP-WS-001".into(), Action::Update)));
        assert!(!defaults.contains(&("CAP-ADM-001".into(), Action::Create)));
        let operator = role_default_pairs(&MemberRole::Operator);
        assert!(!operator.contains(&("CAP-WS-001".into(), Action::Update)));
        assert!(!operator.contains(&("CAP-MEM-001".into(), Action::Create)));
    }

    #[test]
    fn control_import_lock_ids_are_stable_and_package_scoped() {
        assert_eq!(
            control_import_lock_id("package-a"),
            control_import_lock_id("package-a")
        );
        assert_ne!(
            control_import_lock_id("package-a"),
            control_import_lock_id("package-b")
        );
    }

    fn group_grant(action: Action, effect: Effect, revoked_at: Option<&str>) -> GroupShareGrant {
        GroupShareGrant {
            id: GrantId::from_str("grant-1").unwrap(),
            workspace_id: WorkspaceId::from_str("workspace-1").unwrap(),
            group_id: GroupId::from_str("group-1").unwrap(),
            member_id: MemberId::from_str("member-1").unwrap(),
            action,
            effect,
            revision: 1,
            created_at: "2026-10-14T00:00:00Z".to_owned(),
            revoked_at: revoked_at.map(str::to_owned),
        }
    }

    #[test]
    fn managed_group_import_marks_only_resources_requiring_key_rotation() {
        let grants = [
            group_grant(Action::Read, Effect::Allow, None),
            group_grant(Action::Execute, Effect::Allow, None),
            group_grant(Action::Read, Effect::Deny, None),
            group_grant(Action::Read, Effect::Allow, Some("2026-10-14T00:00:00Z")),
            group_grant(Action::Update, Effect::Allow, None),
        ];
        let active = grants
            .iter()
            .filter(|grant| requires_group_share_key_rotation(grant.action))
            .map(|grant| (grant.workspace_id.as_str(), grant.group_id.as_str()));
        let keys = pending_group_share_resource_keys(
            active,
            [
                ("profile-1", Some("group-1")),
                ("profile-2", Some("group-2")),
                ("profile-local", None),
            ]
            .into_iter(),
            [
                ("profile-1", "workspace-1", "resource-1"),
                ("profile-2", "workspace-1", "resource-2"),
                ("profile-local", "workspace-1", "resource-local"),
            ]
            .into_iter(),
        );
        assert_eq!(keys, BTreeSet::from(["workspace-1:resource-1".to_owned()]));
    }

    #[test]
    fn managed_group_import_preserves_multiple_group_assignments() {
        let grant = group_grant(Action::Read, Effect::Allow, None);
        let keys = pending_group_share_resource_keys(
            [(grant.workspace_id.as_str(), grant.group_id.as_str())].into_iter(),
            [
                ("profile-1", Some("other-group")),
                ("profile-1", Some("group-1")),
            ]
            .into_iter(),
            [("profile-1", "workspace-1", "resource-1")].into_iter(),
        );

        assert_eq!(keys, BTreeSet::from(["workspace-1:resource-1".to_owned()]));
    }

    #[test]
    fn group_import_rotation_fence_includes_denied_and_revoked_read_grants() {
        for grant in [
            group_grant(Action::Read, Effect::Deny, None),
            group_grant(Action::Read, Effect::Allow, Some("2026-10-14T00:00:00Z")),
        ] {
            assert!(requires_group_share_key_rotation(grant.action));
            let keys = pending_group_share_resource_keys(
                [(grant.workspace_id.as_str(), grant.group_id.as_str())].into_iter(),
                [("profile-1", Some("group-1"))].into_iter(),
                [("profile-1", "workspace-1", "resource-1")].into_iter(),
            );

            assert_eq!(keys, BTreeSet::from(["workspace-1:resource-1".to_owned()]));
        }
    }

    #[test]
    fn unchanged_reimported_group_grant_does_not_reopen_rotation_fence() {
        let grant = group_grant(Action::Read, Effect::Allow, None);
        assert!(
            changed_group_share_key_rotation_groups([&grant].into_iter(), [&grant].into_iter(),)
                .is_empty()
        );
    }

    #[test]
    fn changed_read_grant_fences_the_group_when_revoked() {
        let active = group_grant(Action::Read, Effect::Allow, None);
        let revoked = group_grant(Action::Read, Effect::Allow, Some("2026-10-15T00:00:00Z"));
        assert_eq!(
            changed_group_share_key_rotation_groups([&active].into_iter(), [&revoked].into_iter(),),
            BTreeSet::from([("workspace-1".to_owned(), "group-1".to_owned())]),
        );
    }

    #[test]
    fn unsupported_legacy_runtimes_map_to_non_launchable_preserved_runtime() {
        assert_eq!(migrated_profile_runtime("Wayfern"), Runtime::Wayfern);
        assert_eq!(migrated_profile_runtime("bugmini"), Runtime::Preserved);
        assert_eq!(migrated_profile_runtime("cloak"), Runtime::Preserved);
        assert_eq!(
            migrated_profile_runtime("unknown-browser"),
            Runtime::Preserved
        );
    }
}
