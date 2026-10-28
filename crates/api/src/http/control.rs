use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::{Path, Query, Request, State},
    http::{HeaderMap, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
};
use buglogin_domain::{
    account::{Account, AccountIdentity, AccountStatus, IdentityVerification, canonical_email},
    audit::{AuditEvent, AuditOutcome, AuditResource},
    entities::{Profile, Proxy, ProxyProtocol},
    errors::Problem,
    grant::ResourceGrant,
    ids::{
        AccountId, CredentialId, ExtensionGroupId, GrantId, GroupId, MemberId, MemberInvitationId,
        OrganizationId, OrganizationMemberId, ProfileId, ProxyId, ResourceId, TeamId, WorkspaceId,
    },
    member::{Member, MemberRole, MemberStatus},
    member_invitation::{MemberInvitation, MemberInvitationState},
    organization::{Organization, OrganizationMemberStatus, OrganizationPolicy},
    permissions::{Action, Capability, Effect, Principal, ResourceScope, is_allowed},
    profile::{
        ProfileArchiveState, ProfileCloneInput, ProfileKeyRotationEnvelopeUpdate, ProfileResource,
        ProfileShareInput, ProfileShareRevokeInput,
    },
    resource::{Resource, ResourceKind, ResourceState},
    team::Team,
    workspace::{OfflineEntitlement, Workspace, WorkspaceUsage},
};
use buglogin_network::proxy::{ProxyDelete, ProxyRecord, ProxyUpdate, RepositoryError};
use buglogin_storage::control::ControlPut;
use buglogin_sync::{
    coordination::{CoordinationAuth, CoordinationScope},
    crypto::{
        AuthenticatedChunk, ManagedResourceKeyEnvelope, ProxyCredentialEnvelope,
        ResourceKeyEnvelope,
    },
    manifest::{MAX_PROFILE_SYNC_OBJECT_BYTES, ManifestDependency, ManifestObject, SyncManifest},
};
use futures_util::{StreamExt, TryStreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, convert::Infallible, time::Duration};
use zeroize::{Zeroize, Zeroizing};

#[cfg(test)]
use super::SyncMigrationJournalObject;
use super::{
    ApiState, BlobIdempotencyRecord, RuntimeProjection, authenticate, authorize_action,
    checked_now, checked_now_ms, grants, problem, request_now_ms, response, storage_quota_problem,
};
use crate::services::members::MemberService;
use crate::services::profile_hosted::HostedProfileService;
use crate::services::proxy_credentials::{credential_version, next_proxy_revision};

mod management_tenancy;

mod management;
mod management_member_remove;

const PROFILE_STATUS_BATCH_CONCURRENCY: usize = 4;
static PROFILE_STATUS_BATCH_SLOTS: std::sync::OnceLock<std::sync::Arc<tokio::sync::Semaphore>> =
    std::sync::OnceLock::new();

pub(super) async fn mutate_hosted_profile_archive_state(
    state: ApiState,
    headers: HeaderMap,
    principal: Principal,
    profile_id: ProfileId,
    expected_revision: u64,
    archive: bool,
) -> Result<Response, Response> {
    let binding = state
        .profile_resources
        .read()
        .map_err(|_| service_unavailable("profile binding"))?
        .get(&profile_id)
        .cloned()
        .ok_or_else(|| {
            problem_response(
                "binding_not_found",
                "Profile binding not found",
                404,
                correlation(&headers),
            )
        })?;
    let command = if archive {
        "profile.archive"
    } else {
        "profile.restore"
    };
    let input = json!({
        "profileId": profile_id,
        "revision": expected_revision,
    });
    let grants = grants(&state)?;
    mutate(
        &state,
        &headers,
        &principal,
        &grants,
        binding.workspace_id.as_str(),
        command,
        input,
        ResourceScope {
            workspace_id: Some(binding.workspace_id.clone()),
            resource_id: Some(binding.resource_id.clone()),
            profile_id: Some(profile_id.clone()),
            ..ResourceScope::default()
        },
        profile_id.as_str(),
        || {
            state
                .mutate_hosted_profile_archive_state(&profile_id, expected_revision, archive)
                .map_err(response_problem)
        },
    )
    .await
}

pub(super) async fn provision_managed_profile(
    state: ApiState,
    headers: HeaderMap,
    principal: Principal,
    grants: Vec<buglogin_domain::permissions::Grant>,
    profile: Profile,
    requested_workspace: Option<WorkspaceId>,
) -> Result<Response, Response> {
    let header_workspace = headers
        .get("x-buglogin-workspace-id")
        .and_then(|value| value.to_str().ok())
        .map(WorkspaceId::from_str)
        .transpose()
        .map_err(|_| bad_header("x-buglogin-workspace-id"))?;
    let workspace_id = requested_workspace
        .or(header_workspace)
        .or_else(|| {
            (principal.workspace_ids.len() == 1)
                .then(|| principal.workspace_ids.iter().next().cloned())
                .flatten()
        })
        .ok_or_else(|| {
            problem_response(
                "workspace_selection_required",
                "Managed profile requires an explicit workspace",
                400,
                correlation(&headers),
            )
        })?;
    if let Some(proxy_id) = profile.proxy_id.as_ref() {
        state
            .proxies
            .read()
            .map_err(|_| service_unavailable("proxy repository"))?
            .get(&workspace_id, proxy_id, &principal, &grants)
            .map_err(proxy_response_problem)?;
    }
    let resource_id =
        HostedProfileService::managed_resource_id(&profile.id).map_err(response_problem)?;
    HostedProfileService::authorize_resource_creation(
        &principal,
        &grants,
        &workspace_id,
        &resource_id,
    )
    .map_err(response_problem)?;
    let plan = HostedProfileService::prepare_provision(profile, workspace_id.clone())
        .map_err(response_problem)?;
    let input = json!({
        "profile": plan.profile,
        "workspaceId": plan.workspace_id,
        "resourceId": plan.resource_id,
        "manifest": plan.manifest,
    });
    mutate(
        &state,
        &headers,
        &principal,
        &grants,
        workspace_id.as_str(),
        "profile.create",
        input,
        ResourceScope::resource(workspace_id.clone(), resource_id.clone()),
        resource_id.as_str(),
        || {
            HostedProfileService::provision(&state, &principal, &grants, &plan)
                .map_err(response_problem)
        },
    )
    .await
}

pub(super) async fn clone_profile(
    state: ApiState,
    headers: HeaderMap,
    principal: Principal,
    grants: Vec<buglogin_domain::permissions::Grant>,
    input: ProfileCloneInput,
) -> Result<Response, Response> {
    let binding = state
        .profile_resource_record(&input.source_profile_id)
        .map_err(response_problem)?
        .ok_or_else(|| {
            problem_response(
                "binding_not_found",
                "Source profile binding not found",
                404,
                correlation(&headers),
            )
        })?;
    let value = serde_json::to_value(&input).map_err(internal)?;
    let command_input = input.clone();
    mutate(
        &state,
        &headers,
        &principal,
        &grants,
        binding.workspace_id.as_str(),
        "profile.clone",
        value,
        ResourceScope {
            workspace_id: Some(binding.workspace_id.clone()),
            resource_id: Some(binding.resource_id.clone()),
            profile_id: Some(input.source_profile_id.clone()),
            ..ResourceScope::default()
        },
        input.source_profile_id.as_str(),
        || {
            HostedProfileService::clone_metadata(&state, &principal, &grants, command_input)
                .map_err(response_problem)
        },
    )
    .await
}

pub(super) async fn share_profile(
    state: ApiState,
    headers: HeaderMap,
    principal: Principal,
    grants: Vec<buglogin_domain::permissions::Grant>,
    input: ProfileShareInput,
) -> Result<Response, Response> {
    HostedProfileService::authorize_share_mutation(&state, &principal, &grants, &input.profile_id)
        .map_err(response_problem)?;
    let binding = state
        .profile_resource_record(&input.profile_id)
        .map_err(response_problem)?
        .ok_or_else(|| {
            problem_response(
                "binding_not_found",
                "Profile binding not found",
                404,
                correlation(&headers),
            )
        })?;
    let value = serde_json::to_value(&input).map_err(internal)?;
    let command_input = input.clone();
    mutate(
        &state,
        &headers,
        &principal,
        &grants,
        binding.workspace_id.as_str(),
        "profile.share",
        value,
        ResourceScope {
            workspace_id: Some(binding.workspace_id.clone()),
            resource_id: Some(binding.resource_id.clone()),
            profile_id: Some(input.profile_id.clone()),
            ..ResourceScope::default()
        },
        input.profile_id.as_str(),
        || {
            HostedProfileService::share(&state, &principal, &grants, command_input)
                .map_err(response_problem)
        },
    )
    .await
}

pub(super) async fn revoke_profile_share(
    state: ApiState,
    headers: HeaderMap,
    principal: Principal,
    grants: Vec<buglogin_domain::permissions::Grant>,
    input: ProfileShareRevokeInput,
) -> Result<Response, Response> {
    HostedProfileService::authorize_share_mutation(&state, &principal, &grants, &input.profile_id)
        .map_err(response_problem)?;
    let binding = state
        .profile_resource_record(&input.profile_id)
        .map_err(response_problem)?
        .ok_or_else(|| {
            problem_response(
                "binding_not_found",
                "Profile binding not found",
                404,
                correlation(&headers),
            )
        })?;
    let value = serde_json::to_value(&input).map_err(internal)?;
    let command_input = input.clone();
    mutate(
        &state,
        &headers,
        &principal,
        &grants,
        binding.workspace_id.as_str(),
        "profile.share.revoke",
        value,
        ResourceScope {
            workspace_id: Some(binding.workspace_id.clone()),
            resource_id: Some(binding.resource_id.clone()),
            profile_id: Some(input.profile_id.clone()),
            ..ResourceScope::default()
        },
        input.profile_id.as_str(),
        || {
            HostedProfileService::revoke_share(&state, &principal, &grants, command_input)
                .map_err(response_problem)
        },
    )
    .await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AccountView {
    account_id: AccountId,
    platform_admin: bool,
    managed_workspace_ids: Vec<WorkspaceId>,
    manageable_workspace_ids: Vec<WorkspaceId>,
    manageable_organization_ids: Vec<OrganizationId>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AccountStatusInput {
    status: AccountStatus,
    expected_revision: u64,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OrganizationInput {
    organization: Organization,
    policy: OrganizationPolicy,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OrganizationAccountCreateInput {
    organization_id: OrganizationId,
    account_id: AccountId,
    username: String,
    #[serde(default)]
    display_name: String,
    email: String,
    initial_password: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OrganizationMemberStatusUpdateInput {
    organization_id: OrganizationId,
    account_id: AccountId,
    status: OrganizationMemberStatus,
    expected_revision: u64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OrganizationWorkspaceMemberAssignInput {
    organization_id: OrganizationId,
    workspace_id: WorkspaceId,
    organization_member_id: OrganizationMemberId,
    organization_member_revision: u64,
    role: MemberRole,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UsageInput {
    profiles: u32,
    members: u32,
    storage_bytes: u64,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RoleInput {
    role: MemberRole,
}

use crate::services::teams::{TeamDelete, TeamRecord, TeamUpdate};
use crate::services::workspaces::{OrganizationUpdate, WorkspaceUpdate};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GrantListQuery {
    resource_id: Option<String>,
    member_id: Option<String>,
    cursor: Option<String>,
    limit: Option<usize>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LeaseAcquireInput {
    owner_id: String,
    ttl_seconds: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LeaseRenewInput {
    owner_id: String,
    owner_token: String,
    ttl_seconds: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LeaseReleaseInput {
    owner_id: String,
    owner_token: String,
}

#[derive(Deserialize)]
struct BlobQuery {
    key: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BlobReadQuery {
    key: String,
    #[serde(default)]
    repair_stale_proxy: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ManifestReadQuery {
    #[serde(default)]
    repair_stale_proxy: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BlobCheckQuery {
    key: String,
    generation: u64,
    checksum: String,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RuntimeProjectionInput {
    device_id: String,
    generation: u64,
    root_checksum: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeProjectionView {
    #[serde(flatten)]
    projection: RuntimeProjection,
    owner_email: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProxyMetadataInput {
    id: ProxyId,
    name: Option<String>,
    protocol: ProxyProtocol,
    host: String,
    port: u16,
    #[serde(default)]
    has_credential: Option<bool>,
    bypass_rules: Vec<String>,
    created_at: String,
    updated_at: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProxyCreateInput {
    workspace_id: WorkspaceId,
    proxy: ProxyMetadataInput,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProxyUpdateInput {
    workspace_id: WorkspaceId,
    proxy_id: ProxyId,
    expected_revision: u64,
    proxy: ProxyMetadataInput,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProxyDeleteInput {
    workspace_id: WorkspaceId,
    proxy_id: ProxyId,
    expected_revision: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProxyView {
    id: ProxyId,
    name: Option<String>,
    protocol: ProxyProtocol,
    host: String,
    port: u16,
    has_credential: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    credential_version: Option<CredentialId>,
    rotation_configured: bool,
    bypass_rules: Vec<String>,
    created_at: String,
    updated_at: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProxyRecordView {
    workspace_id: WorkspaceId,
    proxy: ProxyView,
    revision: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProxyPageView {
    items: Vec<ProxyRecordView>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CollectionPageQuery {
    cursor: Option<String>,
    limit: Option<usize>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ResourcePageQuery {
    cursor: Option<String>,
    limit: Option<usize>,
    search: Option<String>,
    kind: Option<ResourceKind>,
    state: Option<ResourceState>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CollectionPageView<T> {
    items: Vec<T>,
    next_cursor: Option<String>,
}

fn collection_page_limit(limit: Option<usize>) -> Result<usize, Response> {
    let limit = limit.unwrap_or(100);
    if (1..=100).contains(&limit) {
        Ok(limit)
    } else {
        Err(problem_response(
            "invalid_page_limit",
            "Page limit must be between 1 and 100",
            400,
            "request".to_owned(),
        ))
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RuntimeDispatchReceiptInput {
    job_id: String,
    operation_id: String,
    device_id: String,
    profile_id: String,
    command_id: String,
    state: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RuntimeDispatchFailureInput {
    failure_code: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RuntimeDispatchInput {
    command_id: String,
    target_device_id: String,
    #[serde(default)]
    input: Value,
}

fn authenticate_runtime_device(
    state: &ApiState,
    headers: &HeaderMap,
) -> Result<(Principal, String, String), Response> {
    let token = bearer_token(headers);
    let now = super::request_now(headers)?;
    let (account_id, device_id) = state
        .auth
        .read()
        .map_err(|_| service_unavailable("authentication"))?
        .authenticate_device(token, now)
        .map_err(response_problem)?;
    let principal = authenticate(state, headers)?;
    if principal.account_id.as_ref() != Some(&account_id) {
        return Err(problem_response(
            "runtime_job_executor_mismatch",
            "Authenticated device account does not match its session",
            403,
            correlation(headers),
        ));
    }
    Ok((principal, account_id.as_str().to_owned(), device_id))
}

fn authorize_runtime_job(
    state: &ApiState,
    principal: &Principal,
    current_grants: &[buglogin_domain::permissions::Grant],
    job: &crate::services::runtime_dispatch::RuntimeDispatchJob,
    headers: &HeaderMap,
) -> Result<(), Response> {
    let profile_id = ProfileId::from_str(&job.profile_id).map_err(|_| {
        problem_response(
            "runtime_job_profile_invalid",
            "Runtime dispatch profile is invalid",
            409,
            correlation(headers),
        )
    })?;
    let binding = state
        .profile_resources
        .read()
        .map_err(|_| service_unavailable("profile binding"))?
        .get(&profile_id)
        .cloned()
        .ok_or_else(|| {
            problem_response(
                "binding_not_found",
                "Profile binding not found",
                404,
                correlation(headers),
            )
        })?;
    if binding.workspace_id.as_str() != job.workspace_id {
        return Err(problem_response(
            "runtime_job_scope_mismatch",
            "Runtime dispatch scope no longer matches the profile",
            403,
            correlation(headers),
        ));
    }
    authorize_action(
        state,
        principal,
        current_grants,
        &profile_id,
        Action::Execute,
        if job.command_id == "profile.launch" {
            "profile.launch"
        } else {
            "profile.stop"
        },
    )
}

async fn runtime_dispatch_inbox(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, Response> {
    const MAX_WORKSPACES: usize = 4096;
    let (principal, account_id, device_id) = authenticate_runtime_device(&state, &headers)?;
    let tenant_ids = principal
        .workspace_ids
        .iter()
        .map(|workspace_id| workspace_id.as_str().to_owned())
        .collect::<Vec<_>>();
    if tenant_ids.len() > MAX_WORKSPACES {
        return Err(problem_response(
            "runtime_job_inbox_capacity_exceeded",
            "Device inbox exceeds the supported workspace limit",
            503,
            correlation(&headers),
        ));
    }
    let current_grants = grants(&state)?;
    let now = super::request_now(&headers)?;
    expire_overdue_device_dispatches(&state, &tenant_ids, &device_id, now)?;
    let jobs = state
        .runtime_dispatch
        .inbox(&tenant_ids, &device_id, &account_id, now)
        .map_err(response_problem)?;
    let mut items = Vec::with_capacity(jobs.len());
    for job in jobs {
        match authorize_runtime_job(&state, &principal, &current_grants, &job, &headers) {
            Ok(()) => items.push(job),
            Err(error)
                if error.status() == StatusCode::FORBIDDEN
                    || error.status() == StatusCode::NOT_FOUND => {}
            Err(error) => return Err(error),
        }
    }
    Ok(Json(json!({"items": items})))
}

async fn enqueue_runtime_dispatch(
    State(state): State<ApiState>,
    Path((workspace_id, profile_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(input): Json<RuntimeDispatchInput>,
) -> Result<(StatusCode, Json<serde_json::Value>), Response> {
    let principal = authenticate(&state, &headers)?;
    let account_id = principal.account_id.as_ref().ok_or_else(|| {
        problem_response(
            "authentication_required",
            "Authentication required",
            401,
            correlation(&headers),
        )
    })?;
    let workspace_id = WorkspaceId::from_str(&workspace_id).map_err(|_| bad_id("workspace_id"))?;
    let profile_id = ProfileId::from_str(&profile_id).map_err(|_| bad_id("profile_id"))?;
    let current_grants = grants(&state)?;
    authorize_action(
        &state,
        &principal,
        &current_grants,
        &profile_id,
        Action::Execute,
        if input.command_id == "profile.launch" {
            "profile.launch"
        } else {
            "profile.stop"
        },
    )?;
    let binding = state
        .profile_resources
        .read()
        .map_err(|_| service_unavailable("profile binding"))?
        .get(&profile_id)
        .cloned()
        .ok_or_else(|| {
            problem_response(
                "binding_not_found",
                "Profile binding not found",
                404,
                correlation(&headers),
            )
        })?;
    if binding.workspace_id != workspace_id || binding.archive_state != ProfileArchiveState::Active
    {
        return Err(problem_response(
            "profile_forbidden",
            "Profile access denied",
            403,
            correlation(&headers),
        ));
    }
    let idempotency_key = require_idempotency(&headers)?.to_owned();
    let now = super::request_now(&headers)?;
    let active_devices = state
        .auth
        .read()
        .map_err(|_| service_unavailable("authentication"))?
        .active_device_ids(account_id, now)
        .map_err(response_problem)?;
    if !active_devices
        .iter()
        .any(|device| device == &input.target_device_id)
    {
        return Err(problem_response(
            "runtime_job_target_device_forbidden",
            "Target device is not an active device for this account",
            403,
            correlation(&headers),
        ));
    }
    let expires_at = now.checked_add(120).ok_or_else(|| {
        problem_response(
            "runtime_job_input_invalid",
            "Runtime dispatch expiry is invalid",
            400,
            correlation(&headers),
        )
    })?;
    let job = state
        .runtime_dispatch
        .enqueue(
            crate::services::runtime_dispatch::EnqueueRuntimeDispatch {
                command_id: input.command_id,
                workspace_id: workspace_id.as_str().to_owned(),
                device_id: input.target_device_id,
                profile_id: profile_id.as_str().to_owned(),
                requested_by: account_id.as_str().to_owned(),
                idempotency_key,
                now,
                expires_at,
                input: input.input,
            },
            None,
        )
        .map_err(response_problem)?;
    record_dispatch_operation(&state, &job)?;
    Ok((StatusCode::ACCEPTED, Json(json!({"job": job}))))
}

/// The job's operationId is what web/MCP callers poll, so it must exist as a
/// real operation that follows the job to completed/failed.
fn record_dispatch_operation(
    state: &ApiState,
    job: &crate::services::runtime_dispatch::RuntimeDispatchJob,
) -> Result<(), Response> {
    let replay = state
        .operations
        .read()
        .map_err(|_| service_unavailable("operations"))?
        .find(&job.operation_id)
        .is_some();
    if replay {
        return Ok(());
    }
    let scope = ResourceScope {
        account_id: AccountId::from_str(&job.requested_by).ok(),
        workspace_id: WorkspaceId::from_str(&job.workspace_id).ok(),
        profile_id: ProfileId::from_str(&job.profile_id).ok(),
        ..ResourceScope::default()
    };
    state
        .mutate_operation(
            &job.operation_id,
            &job.command_id,
            "started",
            &job.requested_by,
            |operations| {
                operations.insert(crate::services::operations::Operation::queued(
                    &job.operation_id,
                    &job.command_id,
                    scope,
                )?)?;
                operations.transition(
                    &job.operation_id,
                    crate::services::operations::OperationStatus::Running,
                    Some(0.1),
                    None,
                    None,
                )?;
                Ok(())
            },
        )
        .map_err(response_problem)?;
    Ok(())
}

fn finish_dispatch_operation(
    state: &ApiState,
    job: &crate::services::runtime_dispatch::RuntimeDispatchJob,
    result: Result<serde_json::Value, buglogin_domain::errors::Problem>,
) {
    let already_terminal = state.operations.read().is_ok_and(|operations| {
        operations
            .find(&job.operation_id)
            .is_none_or(|operation| operation.status.is_terminal())
    });
    if already_terminal {
        return;
    }
    let (status, value, error) = match result {
        Ok(value) => (
            crate::services::operations::OperationStatus::Completed,
            Some(value),
            None,
        ),
        Err(problem) => (
            crate::services::operations::OperationStatus::Failed,
            None,
            Some(problem),
        ),
    };
    let progress =
        (status == crate::services::operations::OperationStatus::Completed).then_some(1.0);
    let _ = state.mutate_operation(
        &job.operation_id,
        &job.command_id,
        "finished",
        &job.requested_by,
        |operations| {
            operations.transition(&job.operation_id, status, progress, value, error)?;
            Ok(())
        },
    );
}

fn expired_dispatch_problem() -> buglogin_domain::errors::Problem {
    buglogin_domain::errors::Problem::new(
        "runtime_job_expired",
        "The desktop device did not finish the runtime command in time",
        409,
        "runtime-dispatch",
    )
}

/// Called when a dispatch operation is read: an offline or stuck device must not leave
/// the requester polling a "running" operation forever.
pub(crate) fn expire_overdue_dispatch(
    state: &ApiState,
    operation_id: &str,
    now: u64,
) -> Result<(), Response> {
    let Some(job_id) = crate::services::runtime_dispatch::job_id_for_operation(operation_id) else {
        return Ok(());
    };
    let workspace_id = {
        let operations = state
            .operations
            .read()
            .map_err(|_| service_unavailable("operations"))?;
        let Some(operation) = operations.find(operation_id) else {
            return Ok(());
        };
        if operation.status.is_terminal()
            || !matches!(
                operation.command_id.as_str(),
                "profile.launch" | "profile.stop"
            )
        {
            return Ok(());
        }
        let Some(workspace_id) = operation.scope.workspace_id.clone() else {
            return Ok(());
        };
        workspace_id
    };
    if let Some(job) = state
        .runtime_dispatch
        .expire_if_overdue(workspace_id.as_str(), &job_id, now)
        .map_err(response_problem)?
    {
        finish_dispatch_operation(state, &job, Err(expired_dispatch_problem()));
    }
    Ok(())
}

fn expire_overdue_device_dispatches(
    state: &ApiState,
    tenant_ids: &[String],
    device_id: &str,
    now: u64,
) -> Result<(), Response> {
    for job in state
        .runtime_dispatch
        .expire_overdue_for_device(tenant_ids, device_id, now)
        .map_err(response_problem)?
    {
        finish_dispatch_operation(state, &job, Err(expired_dispatch_problem()));
    }
    Ok(())
}

async fn claim_runtime_dispatch(
    State(state): State<ApiState>,
    Path((workspace_id, job_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, Response> {
    let (principal, account_id, device_id) = authenticate_runtime_device(&state, &headers)?;
    let current_grants = grants(&state)?;
    let job = state
        .runtime_dispatch
        .get_for_device(&workspace_id, &job_id, &device_id)
        .map_err(response_problem)?
        .ok_or_else(|| {
            problem_response(
                "runtime_job_not_found",
                "Runtime dispatch job not found",
                404,
                correlation(&headers),
            )
        })?;
    authorize_runtime_job(&state, &principal, &current_grants, &job, &headers)?;
    let job = state
        .runtime_dispatch
        .claim(
            &workspace_id,
            &job_id,
            &device_id,
            &account_id,
            super::request_now(&headers)?,
        )
        .map_err(response_problem)?;
    Ok(Json(json!({"job": job})))
}

async fn complete_runtime_dispatch(
    State(state): State<ApiState>,
    Path((workspace_id, job_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(receipt): Json<RuntimeDispatchReceiptInput>,
) -> Result<Json<serde_json::Value>, Response> {
    let (principal, account_id, device_id) = authenticate_runtime_device(&state, &headers)?;
    let current_grants = grants(&state)?;
    let job = state
        .runtime_dispatch
        .get_for_device(&workspace_id, &job_id, &device_id)
        .map_err(response_problem)?
        .ok_or_else(|| {
            problem_response(
                "runtime_job_not_found",
                "Runtime dispatch job not found",
                404,
                correlation(&headers),
            )
        })?;
    authorize_runtime_job(&state, &principal, &current_grants, &job, &headers)?;
    if receipt.job_id != job.job_id
        || receipt.operation_id != job.operation_id
        || receipt.device_id != job.device_id
        || receipt.profile_id != job.profile_id
        || receipt.command_id != job.command_id
        || !matches!(
            (job.command_id.as_str(), receipt.state.as_str()),
            ("profile.launch", "running") | ("profile.stop", "stopped")
        )
    {
        return Err(problem_response(
            "runtime_job_receipt_mismatch",
            "Runtime dispatch receipt does not match the claimed command",
            400,
            correlation(&headers),
        ));
    }
    let value = serde_json::to_value(receipt).map_err(internal)?;
    let job = state
        .runtime_dispatch
        .complete(&workspace_id, &job_id, &device_id, &account_id, value)
        .map_err(response_problem)?;
    finish_dispatch_operation(&state, &job, Ok(job.result.clone().unwrap_or_default()));
    Ok(Json(json!({"job": job})))
}

async fn fail_runtime_dispatch(
    State(state): State<ApiState>,
    Path((workspace_id, job_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(input): Json<RuntimeDispatchFailureInput>,
) -> Result<Json<serde_json::Value>, Response> {
    let (principal, account_id, device_id) = authenticate_runtime_device(&state, &headers)?;
    let current_grants = grants(&state)?;
    let job = state
        .runtime_dispatch
        .get_for_device(&workspace_id, &job_id, &device_id)
        .map_err(response_problem)?
        .ok_or_else(|| {
            problem_response(
                "runtime_job_not_found",
                "Runtime dispatch job not found",
                404,
                correlation(&headers),
            )
        })?;
    authorize_runtime_job(&state, &principal, &current_grants, &job, &headers)?;
    let job = state
        .runtime_dispatch
        .fail(
            &workspace_id,
            &job_id,
            &device_id,
            &account_id,
            &input.failure_code,
        )
        .map_err(response_problem)?;
    finish_dispatch_operation(
        &state,
        &job,
        Err(buglogin_domain::errors::Problem::new(
            &input.failure_code,
            "Runtime command failed on the desktop device",
            409,
            "runtime-dispatch",
        )),
    );
    Ok(Json(json!({"job": job})))
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProxyCreateResultView {
    record: ProxyRecordView,
    deduplicated: bool,
}

pub(super) fn routes() -> Router<ApiState> {
    routes_core().merge(management::routes())
}

fn routes_core() -> Router<ApiState> {
    Router::new()
        .route("/v1/account", get(account))
        .route("/v1/accounts", get(list_accounts).post(create_account))
        .route(
            "/v1/workspaces/{workspace_id}/accounts",
            post(create_scoped_account),
        )
        .route("/v1/accounts/{account_id}/status", put(set_account_status))
        .route(
            "/v1/organizations",
            get(list_organizations).post(create_organization),
        )
        .route(
            "/v1/organizations/{organization_id}",
            get(get_organization)
                .patch(update_organization)
                .delete(management_tenancy::delete_organization),
        )
        .route(
            "/v1/organizations/{organization_id}/owner",
            post(management_tenancy::transfer_owner),
        )
        .route(
            "/v1/organizations/{organization_id}/record",
            get(get_organization_record),
        )
        .route(
            "/v1/organizations/{organization_id}/delete-preflight",
            get(management_tenancy::organization_delete_preflight),
        )
        .route(
            "/v1/organizations/{organization_id}/members",
            get(list_organization_members),
        )
        .route(
            "/v1/organizations/{organization_id}/accounts",
            post(create_organization_account),
        )
        .route(
            "/v1/organizations/{organization_id}/members/{account_id}",
            axum::routing::patch(update_organization_member_status),
        )
        .route(
            "/v1/organizations/{organization_id}/workspaces/{workspace_id}/members",
            post(assign_organization_member_to_workspace),
        )
        .route(
            "/v1/organizations/{organization_id}/workspaces",
            get(list_workspaces).post(create_workspace),
        )
        .route(
            "/v1/organizations/{organization_id}/workspaces/{workspace_id}",
            get(get_workspace)
                .patch(update_workspace)
                .delete(management_tenancy::delete_workspace),
        )
        .route(
            "/v1/organizations/{organization_id}/workspaces/{workspace_id}/record",
            get(get_workspace_record),
        )
        .route(
            "/v1/organizations/{organization_id}/workspaces/{workspace_id}/delete-preflight",
            get(management_tenancy::workspace_delete_preflight),
        )
        .route(
            "/v1/organizations/{organization_id}/workspaces/{workspace_id}/snapshot",
            get(get_snapshot),
        )
        .route(
            "/v1/organizations/{organization_id}/workspaces/{workspace_id}/usage/check",
            post(check_usage),
        )
        .route(
            "/v1/organizations/{organization_id}/workspaces/{workspace_id}/offline-entitlement",
            get(offline_entitlement),
        )
        .route(
            "/v1/workspaces/{workspace_id}/members",
            get(list_members).post(invite_member),
        )
        .route(
            "/v1/workspaces/{workspace_id}/invitations",
            post(create_member_invitation),
        )
        .route(
            "/v1/workspaces/{workspace_id}/invitations/{invitation_id}",
            delete(revoke_member_invitation),
        )
        .route(
            "/v1/workspace-invitations/{invitation_id}/redeem",
            post(redeem_member_invitation),
        )
        .route(
            "/v1/workspaces/{workspace_id}/members/page",
            get(list_members_page),
        )
        .route(
            "/v1/workspaces/{workspace_id}/proxies",
            get(list_proxies).post(create_proxy),
        )
        .route(
            "/v1/workspaces/{workspace_id}/proxies/{proxy_id}",
            get(get_proxy).patch(update_proxy).delete(delete_proxy),
        )
        .route(
            "/v1/workspaces/{workspace_id}/teams",
            get(list_teams).post(create_team),
        )
        .route(
            "/v1/workspaces/{workspace_id}/teams/{team_id}",
            get(get_team).put(update_team).delete(delete_team),
        )
        .route(
            "/v1/workspaces/{workspace_id}/groups",
            get(list_groups).post(create_group),
        )
        .route(
            "/v1/workspaces/{workspace_id}/groups/{group_id}",
            get(get_group).put(update_group).delete(delete_group),
        )
        .route(
            "/v1/workspaces/{workspace_id}/extension-groups",
            get(list_extension_groups).post(create_extension_group),
        )
        .route(
            "/v1/workspaces/{workspace_id}/extension-groups/{extension_group_id}",
            get(get_extension_group)
                .put(update_extension_group)
                .delete(delete_extension_group),
        )
        .route(
            "/v1/workspaces/{workspace_id}/profiles/{profile_id}/runtime-dispatch",
            post(enqueue_runtime_dispatch),
        )
        .route(
            "/v1/device/runtime-dispatch/inbox",
            get(runtime_dispatch_inbox),
        )
        .route(
            "/v1/workspaces/{workspace_id}/runtime-dispatch/{job_id}/claim",
            post(claim_runtime_dispatch),
        )
        .route(
            "/v1/workspaces/{workspace_id}/runtime-dispatch/{job_id}/complete",
            post(complete_runtime_dispatch),
        )
        .route(
            "/v1/workspaces/{workspace_id}/runtime-dispatch/{job_id}/fail",
            post(fail_runtime_dispatch),
        )
        .route(
            "/v1/workspaces/{workspace_id}/groups/{group_id}/shares",
            get(list_group_shares).post(create_group_share),
        )
        .route(
            "/v1/workspaces/{workspace_id}/groups/{group_id}/shares/{grant_id}",
            delete(revoke_group_share),
        )
        .route(
            "/v1/workspaces/{workspace_id}/members/{member_id}",
            get(get_member).delete(management_member_remove::remove_member),
        )
        .route(
            "/v1/workspaces/{workspace_id}/members/{member_id}/activate",
            put(activate_member),
        )
        .route(
            "/v1/workspaces/{workspace_id}/members/{member_id}/suspend",
            put(suspend_member),
        )
        .route(
            "/v1/workspaces/{workspace_id}/members/{member_id}/role",
            put(set_member_role),
        )
        .route(
            "/v1/workspaces/{workspace_id}/resources",
            get(list_resources).post(register_resource),
        )
        .route(
            "/v1/workspaces/{workspace_id}/resources/page",
            get(list_resources_page),
        )
        .route(
            "/v1/workspaces/{workspace_id}/resources/{resource_id}",
            get(get_resource),
        )
        .route(
            "/v1/workspaces/{workspace_id}/resources/{resource_id}/lease",
            get(get_resource_lease)
                .post(acquire_resource_lease)
                .put(renew_resource_lease)
                .delete(release_resource_lease),
        )
        .route(
            "/v1/workspaces/{workspace_id}/resources/{resource_id}/manifest",
            get(get_resource_manifest).put(commit_resource_manifest),
        )
        .route(
            "/v1/profiles/{profile_id}/key-rotation-recipients",
            get(list_profile_key_rotation_recipients),
        )
        .route(
            "/v1/workspaces/{workspace_id}/resources/{resource_id}/blobs/check",
            get(check_resource_blob),
        )
        .route(
            "/v1/workspaces/{workspace_id}/resources/{resource_id}/blobs",
            get(get_resource_blob).put(put_resource_blob),
        )
        .route(
            "/v1/profiles/{profile_id}/runtime",
            get(get_runtime_projection),
        )
        .route(
            "/v1/workspaces/{workspace_id}/profile-runtime-status",
            get(get_profile_runtime_status_batch),
        )
        .route(
            "/v1/profiles/{profile_id}/runtime/started",
            put(runtime_started),
        )
        .route(
            "/v1/profiles/{profile_id}/runtime/stopped",
            put(runtime_stopped),
        )
        .route(
            "/v1/profiles/{profile_id}/runtime/syncing",
            put(runtime_syncing),
        )
        .route(
            "/v1/profiles/{profile_id}/groups",
            put(replace_profile_groups),
        )
        .route(
            "/v1/profiles/{profile_id}/extension-group",
            put(set_profile_extension_group),
        )
        .route(
            "/v1/workspaces/{workspace_id}/profiles/{profile_id}/proxy-assignment",
            get(get_profile_proxy_assignment).put(set_profile_proxy_assignment),
        )
        .route(
            "/v1/workspaces/{workspace_id}/profiles/{profile_id}",
            put(update_profile_metadata),
        )
        .route(
            "/v1/workspaces/{workspace_id}/grants",
            get(list_resource_grants).post(grant_resource_collection),
        )
        .route(
            "/v1/workspaces/{workspace_id}/grants/page",
            get(list_resource_grants_page),
        )
        .route(
            "/v1/workspaces/{workspace_id}/grants/{grant_id}",
            delete(revoke_workspace_resource_grant),
        )
        .route(
            "/v1/workspaces/{workspace_id}/resources/{resource_id}/grants",
            post(grant_resource),
        )
        .route(
            "/v1/resource-grants/{grant_id}",
            delete(revoke_resource_grant),
        )
        .route(
            "/v1/workspaces/{workspace_id}/audit-events",
            get(read_audit),
        )
}

pub(super) async fn refresh_hosted_snapshot(
    State(state): State<ApiState>,
    request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path();
    let profile_status_batch = request.method() == Method::GET
        && path.starts_with("/v1/workspaces/")
        && path.ends_with("/profile-runtime-status");
    let _status_slot = if profile_status_batch {
        let status_slots = std::sync::Arc::clone(PROFILE_STATUS_BATCH_SLOTS.get_or_init(|| {
            std::sync::Arc::new(tokio::sync::Semaphore::new(
                PROFILE_STATUS_BATCH_CONCURRENCY,
            ))
        }));
        match status_slots.try_acquire_owned() {
            Ok(permit) => Some(permit),
            Err(_) => {
                let headers = request.headers().clone();
                eprintln!(
                    "profile_status_batch overloaded active_limit={PROFILE_STATUS_BATCH_CONCURRENCY}"
                );
                let mut response = problem_response(
                    "profile_status_temporarily_overloaded",
                    "Profile status is busy; retry shortly",
                    503,
                    correlation(&headers),
                );
                response.headers_mut().insert(
                    axum::http::header::RETRY_AFTER,
                    axum::http::HeaderValue::from_static("2"),
                );
                return response;
            }
        }
    } else {
        None
    };
    let control_backed_read = request.method() == Method::GET
        && !matches!(path, "/v1/health" | "/v1/readyz")
        && !path.starts_with("/v1/auth/")
        && !path.starts_with("/v1/operations/")
        && path != "/v1/events"
        && !profile_status_batch;
    if !control_backed_read {
        return next.run(request).await;
    }
    let _snapshot = std::sync::Arc::clone(&state.control_snapshot_gate)
        .read_owned()
        .await;
    if let Err(error) = state.refresh_control_aggregate_async().await {
        return response(error);
    }
    next.run(request).await
}

async fn list_proxies(
    State(state): State<ApiState>,
    Path(workspace_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<ProxyPageView>, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let workspace_id = WorkspaceId::from_str(&workspace_id).map_err(|_| bad_id("workspace_id"))?;
    let records = state
        .proxies
        .read()
        .map_err(|_| service_unavailable("proxy"))?
        .list(&workspace_id, &principal, &grants)
        .map_err(proxy_response_problem)?;
    Ok(Json(ProxyPageView {
        items: records.into_iter().map(proxy_record_view).collect(),
    }))
}

async fn get_proxy(
    State(state): State<ApiState>,
    Path((workspace_id, proxy_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<ProxyRecordView>, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let workspace_id = WorkspaceId::from_str(&workspace_id).map_err(|_| bad_id("workspace_id"))?;
    let proxy_id = ProxyId::from_str(&proxy_id).map_err(|_| bad_id("proxy_id"))?;
    let record = state
        .proxies
        .read()
        .map_err(|_| service_unavailable("proxy"))?
        .get(&workspace_id, &proxy_id, &principal, &grants)
        .map_err(proxy_response_problem)?;
    Ok(Json(proxy_record_view(record)))
}

async fn create_proxy(
    State(state): State<ApiState>,
    Path(workspace_path): Path<String>,
    headers: HeaderMap,
    Json(input): Json<ProxyCreateInput>,
) -> Result<Response, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let workspace_id =
        WorkspaceId::from_str(&workspace_path).map_err(|_| bad_id("workspace_id"))?;
    ensure_workspace_input(&workspace_id, &input.workspace_id)?;
    let input_value = serde_json::to_value(&input).map_err(internal)?;
    let proxy = proxy_from_metadata(input.proxy).map_err(response_problem)?;
    let proxy_id = proxy.id.clone();
    mutate(
        &state,
        &headers,
        &principal,
        &grants,
        workspace_id.as_str(),
        "proxy.create",
        input_value,
        ResourceScope::workspace(workspace_id.clone()),
        proxy_id.as_str(),
        || {
            let result = state
                .proxies
                .write()
                .map_err(|_| service_unavailable("proxy"))?
                .create(workspace_id.clone(), proxy, &principal, &grants)
                .map_err(proxy_response_problem)?;
            Ok(ProxyCreateResultView {
                record: proxy_record_view(result.record),
                deduplicated: result.deduplicated,
            })
        },
    )
    .await
}

async fn update_proxy(
    State(state): State<ApiState>,
    Path((workspace_path, proxy_path)): Path<(String, String)>,
    headers: HeaderMap,
    Json(input): Json<ProxyUpdateInput>,
) -> Result<Response, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let workspace_id =
        WorkspaceId::from_str(&workspace_path).map_err(|_| bad_id("workspace_id"))?;
    let proxy_id = ProxyId::from_str(&proxy_path).map_err(|_| bad_id("proxy_id"))?;
    ensure_workspace_input(&workspace_id, &input.workspace_id)?;
    if proxy_id != input.proxy_id || proxy_id != input.proxy.id {
        return Err(problem_response(
            "invalid_proxy",
            "Proxy identifier mismatch",
            400,
            correlation(&headers),
        ));
    }
    let current = state
        .proxies
        .read()
        .map_err(|_| service_unavailable("proxy"))?
        .get(&workspace_id, &proxy_id, &principal, &grants)
        .map_err(proxy_response_problem)?;
    crate::services::profile_sync::ensure_proxy_update_allowed(
        &state,
        &workspace_id,
        &proxy_id,
        &correlation(&headers),
    )
    .map_err(response_problem)?;
    let input_value = serde_json::to_value(&input).map_err(internal)?;
    let has_credential = input.proxy.has_credential;
    let mut proxy = proxy_from_metadata(input.proxy.clone()).map_err(response_problem)?;
    proxy.credential_ref = match has_credential {
        None => current.proxy.credential_ref,
        Some(false) => None,
        Some(true) => Some(
            credential_version(
                &proxy.id,
                next_proxy_revision(input.expected_revision).map_err(response_problem)?,
            )
            .map_err(response_problem)?,
        ),
    };
    proxy.rotation_url = current.proxy.rotation_url;
    let expected_revision = input.expected_revision;
    mutate(
        &state,
        &headers,
        &principal,
        &grants,
        workspace_id.as_str(),
        "proxy.update",
        input_value,
        ResourceScope::workspace(workspace_id.clone()),
        proxy_id.as_str(),
        || {
            crate::services::profile_sync::ensure_proxy_update_allowed(
                &state,
                &workspace_id,
                &proxy_id,
                &correlation(&headers),
            )
            .map_err(response_problem)?;
            let record = state
                .proxies
                .write()
                .map_err(|_| service_unavailable("proxy"))?
                .update(
                    &workspace_id,
                    &proxy_id,
                    ProxyUpdate {
                        proxy,
                        expected_revision,
                    },
                    &principal,
                    &grants,
                )
                .map_err(proxy_response_problem)?;
            Ok(proxy_record_view(record))
        },
    )
    .await
}

async fn delete_proxy(
    State(state): State<ApiState>,
    Path((workspace_path, proxy_path)): Path<(String, String)>,
    headers: HeaderMap,
    Json(input): Json<ProxyDeleteInput>,
) -> Result<Response, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let workspace_id =
        WorkspaceId::from_str(&workspace_path).map_err(|_| bad_id("workspace_id"))?;
    let proxy_id = ProxyId::from_str(&proxy_path).map_err(|_| bad_id("proxy_id"))?;
    ensure_workspace_input(&workspace_id, &input.workspace_id)?;
    if proxy_id != input.proxy_id {
        return Err(problem_response(
            "invalid_proxy",
            "Proxy identifier mismatch",
            400,
            correlation(&headers),
        ));
    }
    crate::services::profile_sync::ensure_proxy_unassigned(
        &state,
        &workspace_id,
        &proxy_id,
        &correlation(&headers),
    )
    .map_err(response_problem)
    .map_err(Response::from)?;
    let expected_revision = input.expected_revision;
    let input_value = serde_json::to_value(&input).map_err(internal)?;
    mutate(
        &state,
        &headers,
        &principal,
        &grants,
        workspace_id.as_str(),
        "proxy.delete",
        input_value,
        ResourceScope::workspace(workspace_id.clone()),
        proxy_id.as_str(),
        || {
            crate::services::profile_sync::ensure_proxy_unassigned(
                &state,
                &workspace_id,
                &proxy_id,
                &correlation(&headers),
            )
            .map_err(response_problem)?;
            let record = state
                .proxies
                .write()
                .map_err(|_| service_unavailable("proxy"))?
                .delete(
                    &workspace_id,
                    &proxy_id,
                    ProxyDelete { expected_revision },
                    &buglogin_network::proxy::ProxyAssignmentRepository::default(),
                    &principal,
                    &grants,
                )
                .map_err(proxy_response_problem)?;
            Ok(proxy_record_view(record))
        },
    )
    .await
}

fn proxy_from_metadata(input: ProxyMetadataInput) -> Result<Proxy, Problem> {
    let credential_ref = if input.has_credential.unwrap_or(false) {
        Some(credential_version(&input.id, 1)?)
    } else {
        None
    };
    Ok(Proxy {
        id: input.id,
        name: input.name,
        protocol: input.protocol,
        host: input.host,
        port: input.port,
        credential_ref,
        rotation_url: None,
        bypass_rules: input.bypass_rules,
        created_at: input.created_at,
        updated_at: input.updated_at,
    })
}

fn proxy_record_view(record: ProxyRecord) -> ProxyRecordView {
    ProxyRecordView {
        workspace_id: record.workspace_id,
        proxy: ProxyView {
            id: record.proxy.id,
            name: record.proxy.name,
            protocol: record.proxy.protocol,
            host: record.proxy.host,
            port: record.proxy.port,
            has_credential: record.proxy.credential_ref.is_some(),
            credential_version: record.proxy.credential_ref.clone(),
            rotation_configured: record.proxy.rotation_url.is_some(),
            bypass_rules: record.proxy.bypass_rules,
            created_at: record.proxy.created_at,
            updated_at: record.proxy.updated_at,
        },
        revision: record.revision,
    }
}

fn ensure_workspace_input(path: &WorkspaceId, input: &WorkspaceId) -> Result<(), Response> {
    if path == input {
        Ok(())
    } else {
        Err(problem_response(
            "workspace_mismatch",
            "Workspace identifier mismatch",
            400,
            "unknown".into(),
        ))
    }
}

fn proxy_response_problem(error: RepositoryError) -> ResponseProblem {
    response_problem(crate::services::profile_sync::proxy_repository_problem(
        error,
    ))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AccountAdminRecordView {
    account: Account,
    identity: Option<AccountIdentity>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AccountAdminPageView {
    items: Vec<AccountAdminRecordView>,
    next_cursor: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AccountCreateInput {
    account_id: AccountId,
    username: String,
    email: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ScopedAccountCreateInput {
    account_id: AccountId,
    username: String,
    #[serde(default)]
    display_name: String,
    email: String,
    role: MemberRole,
    initial_password: String,
}

async fn list_accounts(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<AccountAdminPageView>, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    authorize_control(
        &principal,
        &grants,
        "CAP-ADM-001",
        Action::Read,
        &ResourceScope::global(),
        "account_forbidden",
        "Account access denied",
    )?;
    let mut accounts = state
        .control_persistence
        .list::<Account>("control", "account")
        .map_err(response)?;
    accounts.sort_by(|left, right| left.id.cmp(&right.id));
    let identities = crate::services::identity_persistence::IdentityPersistence::new(
        state.control_persistence.clone(),
    );
    let items = accounts
        .into_iter()
        .map(|account| {
            Ok(AccountAdminRecordView {
                identity: identities.load(&account.id).map_err(response_problem)?,
                account,
            })
        })
        .collect::<Result<Vec<_>, Response>>()?;
    Ok(Json(AccountAdminPageView {
        items,
        next_cursor: None,
    }))
}

async fn create_account(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(input): Json<AccountCreateInput>,
) -> Result<Response, Response> {
    let principal = authenticate(&state, &headers)?;
    let grant_snapshot = grants(&state)?;
    authorize_control(
        &principal,
        &grant_snapshot,
        "CAP-ADM-001",
        Action::Create,
        &ResourceScope::global(),
        "account_forbidden",
        "Account access denied",
    )?;

    let operation_state = state.clone();
    let operation_headers = headers.clone();
    with_authorization_snapshot(
        &state,
        &headers,
        &principal,
        &grant_snapshot,
        move |principal, current_grants, _| {
            authorize_control(
                principal,
                current_grants,
                "CAP-ADM-001",
                Action::Create,
                &ResourceScope::global(),
                "account_forbidden",
                "Account access denied",
            )?;
            let actor_id = principal.account_id.as_ref().ok_or_else(|| {
                problem_response(
                    "account_missing",
                    "Account is missing",
                    401,
                    correlation(&operation_headers),
                )
            })?;
            let idempotency_key =
                mutation_key(&operation_headers, &correlation(&operation_headers))?;
            let created_at = crate::clock::rfc3339(&correlation(&operation_headers))
                .map_err(response_problem)?;
            let pending = crate::services::account_provisioning::AccountProvisioningService::new(
                operation_state.control_persistence.clone(),
            )
            .create_pending(
                input.account_id,
                &input.username,
                &input.email,
                actor_id,
                &idempotency_key,
                &created_at,
            )
            .map_err(response_problem)?;
            Ok((
                StatusCode::CREATED,
                Json(json!({
                    "account": pending.account,
                    "identity": pending.identity,
                    "replayed": pending.replayed,
                    "onboardingStatus": "pending_verification",
                    "loginEnabled": false,
                })),
            )
                .into_response())
        },
    )
    .await
}

async fn create_scoped_account(
    State(state): State<ApiState>,
    Path(workspace_id): Path<String>,
    headers: HeaderMap,
    Json(input): Json<ScopedAccountCreateInput>,
) -> Result<Response, Response> {
    let workspace_id = WorkspaceId::from_str(&workspace_id).map_err(|_| bad_id("workspace_id"))?;
    let principal = authenticate(&state, &headers)?;
    let grant_snapshot = grants(&state)?;
    let organization_owner_scope = scoped_account_creation_authority(
        &state,
        &principal,
        &grant_snapshot,
        &workspace_id,
        &input.role,
    )?;
    crate::services::credentials::CredentialService::validate_initial_password(
        &input.initial_password,
    )
    .map_err(response_problem)?;

    let member_id =
        crate::services::account_provisioning::scoped_member_id(&workspace_id, &input.account_id)
            .map_err(response_problem)?;
    let member = Member {
        id: member_id,
        account_id: input.account_id.clone(),
        workspace_id: workspace_id.clone(),
        team_id: None,
        role: input.role.clone(),
        status: MemberStatus::Active,
        invite_expires_at: None,
    };
    let mutation_input = json!({
        "workspaceId": workspace_id,
        "account": {
            "accountId": input.account_id,
            "username": input.username,
            "displayName": input.display_name,
            "email": input.email,
        },
        "member": member,
    });
    let member_for_mutation = member.clone();
    let account_id_for_mutation = input.account_id.clone();
    let username_for_mutation = input.username.clone();
    let email_for_mutation = input.email.clone();
    let initial_password = zeroize::Zeroizing::new(input.initial_password);
    let credential_now =
        crate::clock::unix_time_seconds(&correlation(&headers)).map_err(response_problem)?;
    let organization_owner_scope_for_mutation = organization_owner_scope.clone();
    mutate_with_additional_puts_and_replay_check(
        &state,
        &headers,
        &principal,
        &grant_snapshot,
        workspace_id.as_str(),
        "account.create.scoped",
        mutation_input,
        ResourceScope::workspace(workspace_id.clone()),
        input.account_id.as_str(),
        || {
            let credentials = crate::services::credentials::CredentialService::new(
                state.control_persistence.clone(),
            )
            .map_err(response_problem)?;
            if credentials
                .matches_password_without_recording_attempt(
                    &member_for_mutation.account_id,
                    initial_password.as_str(),
                )
                .map_err(response_problem)?
            {
                Ok(())
            } else {
                Err(response_problem(Problem::new(
                    "idempotency_conflict",
                    "Idempotency key was already used with a different initial password",
                    409,
                    "identity",
                )))
            }
        },
        || {
            let (mut pending, mut account_puts) =
                crate::services::account_provisioning::AccountProvisioningService::new(
                    state.control_persistence.clone(),
                )
                .prepare_admin_provisioned_scoped(
                    account_id_for_mutation,
                    &username_for_mutation,
                    &email_for_mutation,
                    &crate::clock::rfc3339(&correlation(&headers)).map_err(response_problem)?,
                )
                .map_err(response_problem)?;
            management::set_provisioned_display_name(
                &mut pending.identity,
                &mut account_puts,
                &input.display_name,
            )?;
            let credential_put = crate::services::credentials::CredentialService::new(
                state.control_persistence.clone(),
            )
            .map_err(response_problem)?
            .prepare_initial_password_put(
                &pending.account.id,
                initial_password.as_str(),
                credential_now,
            )
            .map_err(response_problem)?;
            account_puts.push(credential_put);
            let mut members = state
                .members
                .write()
                .map_err(|_| service_unavailable("member"))?;
            if let Some(organization) = organization_owner_scope_for_mutation.as_ref() {
                members.provision_active_as_org_owner(
                    &principal,
                    &grant_snapshot,
                    organization,
                    member_for_mutation.clone(),
                )
            } else {
                members.provision_active(&principal, &grant_snapshot, member_for_mutation.clone())
            }
            .map_err(response_problem)?;
            Ok((
                json!({
                    "account": pending.account,
                    "identity": pending.identity,
                    "member": member_for_mutation,
                    "replayed": pending.replayed,
                    "onboardingStatus": "admin_provisioned",
                    "loginEnabled": true,
                }),
                account_puts,
                None,
            ))
        },
    )
    .await
}

fn scoped_account_creation_authority(
    state: &ApiState,
    principal: &Principal,
    grants: &[buglogin_domain::permissions::Grant],
    workspace_id: &WorkspaceId,
    role: &MemberRole,
) -> Result<Option<buglogin_domain::organization::Organization>, Response> {
    if state
        .members
        .read()
        .map_err(|_| service_unavailable("member"))?
        .authorize_invitation(principal, grants, workspace_id, role)
        .is_ok()
    {
        return Ok(None);
    }
    let organization = state
        .workspaces
        .read()
        .map_err(|_| service_unavailable("workspace"))?
        .managed_organization_for_workspace(principal, grants, workspace_id)
        .map_err(response_problem)?;
    MemberService::authorize_org_owner_account_creation(
        principal,
        grants,
        &organization,
        workspace_id,
        role,
    )
    .map_err(response_problem)?;
    Ok(Some(organization))
}

fn workspace_member_management_grants(
    state: &ApiState,
    principal: &Principal,
    grants: &[buglogin_domain::permissions::Grant],
    workspace_id: &WorkspaceId,
    action: Action,
    correlation_id: String,
) -> Result<(Vec<buglogin_domain::permissions::Grant>, bool), Response> {
    let capability = Capability::new("CAP-MEM-001").map_err(|_| {
        problem_response(
            "invalid_capability",
            "Membership capability is invalid",
            500,
            correlation_id.clone(),
        )
    })?;
    let scope = ResourceScope::workspace(workspace_id.clone());
    let organization_owner = if principal
        .roles
        .contains(&buglogin_domain::permissions::Role::PlatformAdmin)
    {
        false
    } else {
        let workspaces = state
            .workspaces
            .read()
            .map_err(|_| service_unavailable("workspace"))?;
        workspaces
            .organization_for_workspace(workspace_id)
            .is_some_and(|organization| {
                principal.account_id.as_ref() == Some(&organization.account_id)
            })
    };
    if is_allowed(principal, &capability, action, &scope, grants) {
        return Ok((grants.to_vec(), organization_owner));
    }
    if principal.token_scopes.is_some() {
        return Err(problem_response(
            "forbidden",
            "Permission denied",
            403,
            correlation_id.clone(),
        ));
    }
    let explicitly_denied = grants.iter().any(|grant| {
        grant.capability == capability
            && grant.action == Action::Manage
            && grant.effect == Effect::Deny
            && grant.scope.contains(&scope)
            && grant
                .account_id
                .as_ref()
                .is_none_or(|id| principal.account_id.as_ref() == Some(id))
            && grant
                .member_id
                .as_ref()
                .is_none_or(|id| principal.member_id.as_ref() == Some(id))
    });
    if explicitly_denied {
        return Err(problem_response(
            "forbidden",
            "Permission denied",
            403,
            correlation_id.clone(),
        ));
    }
    let organization = state
        .workspaces
        .read()
        .map_err(|_| service_unavailable("workspace"))?
        .organization_for_workspace(workspace_id)
        .ok_or_else(|| {
            problem_response(
                "workspace_not_found",
                "Workspace not found",
                404,
                correlation_id.clone(),
            )
        })?;
    MemberService::authorize_org_owner_access(principal, grants, &organization, workspace_id)
        .map_err(response_problem)?;
    let actor_id = principal.account_id.clone().ok_or_else(|| {
        problem_response(
            "account_missing",
            "Account is missing",
            401,
            correlation_id.clone(),
        )
    })?;
    let mut effective = grants.to_vec();
    for action in [Action::Read, Action::Manage, action] {
        if !effective.iter().any(|grant| {
            grant.capability == capability && grant.action == action && grant.scope == scope
        }) {
            effective.push(buglogin_domain::permissions::Grant {
                id: GrantId::new(format!(
                    "derived_org_member_{}_{}",
                    workspace_id.as_str(),
                    match action {
                        Action::Read => "read",
                        Action::Create => "create",
                        Action::Update => "update",
                        Action::Delete => "delete",
                        Action::Execute => "execute",
                        Action::Share => "share",
                        Action::Manage => "manage",
                    }
                ))
                .map_err(|_| {
                    problem_response(
                        "invalid_grant",
                        "Derived membership authority is invalid",
                        500,
                        correlation_id.clone(),
                    )
                })?,
                account_id: Some(actor_id.clone()),
                member_id: None,
                capability: capability.clone(),
                action,
                scope: scope.clone(),
                effect: Effect::Allow,
            });
        }
    }
    Ok((effective, true))
}

async fn set_account_status(
    State(state): State<ApiState>,
    Path(account_id): Path<String>,
    headers: HeaderMap,
    Json(input): Json<AccountStatusInput>,
) -> Result<Response, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let account_id = AccountId::from_str(&account_id).map_err(|_| bad_id("account_id"))?;
    let operation_state = state.clone();
    let operation_headers = headers.clone();
    with_authorization_snapshot(
        &state,
        &headers,
        &principal,
        &grants,
        move |principal, grants, _| {
            set_account_status_authorized(
                &operation_state,
                &operation_headers,
                principal,
                grants,
                &account_id,
                &input,
            )
        },
    )
    .await
}

fn set_account_status_authorized(
    state: &ApiState,
    headers: &HeaderMap,
    principal: &Principal,
    grants: &[buglogin_domain::permissions::Grant],
    account_id: &AccountId,
    input: &AccountStatusInput,
) -> Result<Response, Response> {
    let scope = ResourceScope::account(account_id.clone());
    authorize_control(
        principal,
        grants,
        "CAP-ADM-001",
        Action::Manage,
        &scope,
        "account_forbidden",
        "Account access denied",
    )?;
    let correlation = correlation(headers);
    let current = checked_now(&correlation).map_err(response)?;
    let key = mutation_key(headers, &correlation)?;
    let actor = principal
        .account_id
        .as_ref()
        .map(AccountId::as_str)
        .unwrap_or("anonymous");
    let legacy_store_key = format!("{actor}:account.status:{}:{key}", account_id.as_str());
    let store_key = format!(
        "http-account-status-{:x}",
        Sha256::digest(legacy_store_key.as_bytes())
    );
    let input_value = serde_json::to_value(input).map_err(internal)?;
    let legacy_input_hash = input_hash(&input_value, &correlation)?;
    let normalized_input = serde_json::json!({
        "accountId": account_id.as_str(),
        "input": input_value,
    });
    let input_hash = input_hash(&normalized_input, &correlation)?;
    let legacy_key_is_valid = legacy_store_key.len() <= 128
        && legacy_store_key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'));
    if legacy_key_is_valid
        && let Some(response) = state
            .replay_control_response(&legacy_store_key, &legacy_input_hash)
            .map_err(response_problem)?
    {
        return Ok((
            StatusCode::from_u16(response.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            Json(response.body),
        )
            .into_response());
    }
    if let Some(response) = state
        .replay_control_response(&store_key, &input_hash)
        .map_err(response_problem)?
    {
        return Ok((
            StatusCode::from_u16(response.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            Json(response.body),
        )
            .into_response());
    }
    let account = state
        .account_persistence
        .load(account_id)
        .map_err(response)?
        .ok_or_else(|| {
            problem_response(
                "account_not_found",
                "Account not found",
                404,
                correlation.clone(),
            )
        })?;
    let updated = account
        .transition_status(input.expected_revision, input.status, current.to_string())
        .map_err(response)?;
    let body = serde_json::to_value(&updated).map_err(internal)?;
    let mut auth = state.auth.write().map_err(|_| {
        response(problem(
            "auth_unavailable",
            "Authentication unavailable",
            503,
        ))
    })?;
    let mut puts = vec![ControlPut {
        tenant_id: "control".into(),
        record_type: "account".into(),
        record_id: account_id.as_str().into(),
        expected_revision: Some(account.revision),
        value: body.clone(),
    }];
    let session_revocations = auth
        .account_session_revocation_puts(account_id, current)
        .map_err(response)?;
    let revoked_family_count = session_revocations
        .iter()
        .filter(|put| put.record_type == "session_family")
        .count();
    puts.extend(session_revocations);
    let api_token_revocations = state
        .api_tokens
        .account_revocation_puts(account_id, current)
        .map_err(response)?;
    let revoked_api_token_count = api_token_revocations.len();
    puts.extend(api_token_revocations);
    state
        .persist_control_aggregate_with_puts(
            "account.status.update",
            store_key,
            actor.into(),
            true,
            &input_hash,
            serde_json::json!({
                "correlationId": correlation,
                "accountId": account_id,
                "status": input.status,
                "expectedRevision": input.expected_revision,
                "inputHash": input_hash,
                "revokedSessionFamilies": revoked_family_count,
                "revokedApiTokens": revoked_api_token_count,
            }),
            StatusCode::OK.as_u16(),
            body.clone(),
            puts,
        )
        .map_err(response)?;
    auth.mark_account_sessions_revoked(account_id, current);
    Ok((StatusCode::OK, Json(body)).into_response())
}

async fn create_team(
    State(state): State<ApiState>,
    Path(workspace_id): Path<String>,
    headers: HeaderMap,
    Json(team): Json<Team>,
) -> Result<Response, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let workspace_id = WorkspaceId::from_str(&workspace_id).map_err(|_| bad_id("workspace_id"))?;
    let input = serde_json::to_value(&team).map_err(internal)?;
    let scope = team_scope(&workspace_id, Some(&team.id));
    mutate(
        &state,
        &headers,
        &principal,
        &grants,
        workspace_id.as_str(),
        "team.create",
        input,
        scope,
        team.id.as_str(),
        || {
            state
                .teams
                .write()
                .map_err(|_| service_unavailable("team"))?
                .create(&principal, &grants, &workspace_id, team.clone())
                .map_err(response_problem)
        },
    )
    .await
}

async fn list_teams(
    State(state): State<ApiState>,
    Path(workspace_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Vec<TeamRecord>>, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let workspace_id = WorkspaceId::from_str(&workspace_id).map_err(|_| bad_id("workspace_id"))?;
    Ok(Json(
        state
            .teams
            .read()
            .map_err(|_| service_unavailable("team"))?
            .list(&principal, &grants, &workspace_id)
            .map_err(response_problem)?,
    ))
}

async fn get_team(
    State(state): State<ApiState>,
    Path((workspace_id, team_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<TeamRecord>, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let workspace_id = WorkspaceId::from_str(&workspace_id).map_err(|_| bad_id("workspace_id"))?;
    let team_id = TeamId::from_str(&team_id).map_err(|_| bad_id("team_id"))?;
    Ok(Json(
        state
            .teams
            .read()
            .map_err(|_| service_unavailable("team"))?
            .get(&principal, &grants, &workspace_id, &team_id)
            .map_err(response_problem)?,
    ))
}

async fn update_team(
    State(state): State<ApiState>,
    Path((workspace_id, team_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(input): Json<TeamUpdate>,
) -> Result<Response, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let workspace_id = WorkspaceId::from_str(&workspace_id).map_err(|_| bad_id("workspace_id"))?;
    let team_id = TeamId::from_str(&team_id).map_err(|_| bad_id("team_id"))?;
    let value = serde_json::to_value(&input).map_err(internal)?;
    mutate(
        &state,
        &headers,
        &principal,
        &grants,
        workspace_id.as_str(),
        "team.update",
        value,
        team_scope(&workspace_id, Some(&team_id)),
        team_id.as_str(),
        || {
            state
                .teams
                .write()
                .map_err(|_| service_unavailable("team"))?
                .update(&principal, &grants, &workspace_id, &team_id, input)
                .map_err(response_problem)
        },
    )
    .await
}

async fn delete_team(
    State(state): State<ApiState>,
    Path((workspace_id, team_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(input): Json<TeamDelete>,
) -> Result<Response, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let workspace_id = WorkspaceId::from_str(&workspace_id).map_err(|_| bad_id("workspace_id"))?;
    let team_id = TeamId::from_str(&team_id).map_err(|_| bad_id("team_id"))?;
    let value = serde_json::to_value(&input).map_err(internal)?;
    mutate(
        &state,
        &headers,
        &principal,
        &grants,
        workspace_id.as_str(),
        "team.delete",
        value,
        team_scope(&workspace_id, Some(&team_id)),
        team_id.as_str(),
        || {
            state
                .teams
                .write()
                .map_err(|_| service_unavailable("team"))?
                .delete(&principal, &grants, &workspace_id, &team_id, input)
                .map_err(response_problem)
        },
    )
    .await
}

async fn list_groups(
    State(state): State<ApiState>,
    Path(workspace_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Vec<crate::services::groups::GroupRecord>>, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let workspace_id = WorkspaceId::from_str(&workspace_id).map_err(|_| bad_id("workspace_id"))?;
    Ok(Json(
        state
            .groups
            .read()
            .map_err(|_| service_unavailable("group"))?
            .list(&principal, &grants, &workspace_id)
            .map_err(response_problem)?,
    ))
}

async fn get_group(
    State(state): State<ApiState>,
    Path((workspace_id, group_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<crate::services::groups::GroupRecord>, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let workspace_id = WorkspaceId::from_str(&workspace_id).map_err(|_| bad_id("workspace_id"))?;
    let group_id = GroupId::from_str(&group_id).map_err(|_| bad_id("group_id"))?;
    Ok(Json(
        state
            .groups
            .read()
            .map_err(|_| service_unavailable("group"))?
            .get(&principal, &grants, &workspace_id, &group_id)
            .map_err(response_problem)?,
    ))
}

async fn create_group(
    State(state): State<ApiState>,
    Path(workspace_id): Path<String>,
    headers: HeaderMap,
    Json(input): Json<crate::services::groups::GroupCreateInput>,
) -> Result<Response, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let workspace_id = WorkspaceId::from_str(&workspace_id).map_err(|_| bad_id("workspace_id"))?;
    let value = serde_json::to_value(&input).map_err(internal)?;
    let group_id = input.group_id.clone();
    mutate(
        &state,
        &headers,
        &principal,
        &grants,
        workspace_id.as_str(),
        "group.create",
        value,
        ResourceScope::workspace(workspace_id.clone()),
        group_id.as_str(),
        || {
            state
                .groups
                .write()
                .map_err(|_| service_unavailable("group"))?
                .create(&principal, &grants, &workspace_id, input.clone())
                .map_err(response_problem)
        },
    )
    .await
}

async fn update_group(
    State(state): State<ApiState>,
    Path((workspace_path, group_path)): Path<(String, String)>,
    headers: HeaderMap,
    Json(input): Json<crate::services::groups::GroupUpdateInput>,
) -> Result<Response, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let workspace_id =
        WorkspaceId::from_str(&workspace_path).map_err(|_| bad_id("workspace_id"))?;
    let group_id = GroupId::from_str(&group_path).map_err(|_| bad_id("group_id"))?;
    let value = serde_json::to_value(&input).map_err(internal)?;
    mutate(
        &state,
        &headers,
        &principal,
        &grants,
        workspace_id.as_str(),
        "group.update",
        value,
        ResourceScope::workspace(workspace_id.clone()),
        group_id.as_str(),
        || {
            state
                .groups
                .write()
                .map_err(|_| service_unavailable("group"))?
                .update(&principal, &grants, &workspace_id, &group_id, input)
                .map_err(response_problem)
        },
    )
    .await
}

async fn delete_group(
    State(state): State<ApiState>,
    Path((workspace_path, group_path)): Path<(String, String)>,
    headers: HeaderMap,
    Json(input): Json<crate::services::groups::GroupDeleteInput>,
) -> Result<Response, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let workspace_id =
        WorkspaceId::from_str(&workspace_path).map_err(|_| bad_id("workspace_id"))?;
    let group_id = GroupId::from_str(&group_path).map_err(|_| bad_id("group_id"))?;
    let value = serde_json::to_value(&input).map_err(internal)?;
    mutate(
        &state,
        &headers,
        &principal,
        &grants,
        workspace_id.as_str(),
        "group.delete",
        value,
        ResourceScope::workspace(workspace_id.clone()),
        group_id.as_str(),
        || {
            let in_use = state
                .group_is_in_use(&workspace_id, &group_id)
                .map_err(response_problem)?;
            state
                .groups
                .write()
                .map_err(|_| service_unavailable("group"))?
                .delete(&principal, &grants, &workspace_id, &group_id, input, in_use)
                .map_err(response_problem)
        },
    )
    .await
}

async fn list_extension_groups(
    State(state): State<ApiState>,
    Path(workspace_path): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Vec<crate::services::extension_groups::ExtensionGroupRecord>>, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let workspace_id =
        WorkspaceId::from_str(&workspace_path).map_err(|_| bad_id("workspace_id"))?;
    let records = state
        .extension_groups
        .read()
        .map_err(|_| service_unavailable("extension_group"))?
        .list(&principal, &grants, &workspace_id)
        .map_err(response_problem)?;
    Ok(Json(records))
}

async fn get_extension_group(
    State(state): State<ApiState>,
    Path((workspace_path, group_path)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<crate::services::extension_groups::ExtensionGroupRecord>, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let workspace_id =
        WorkspaceId::from_str(&workspace_path).map_err(|_| bad_id("workspace_id"))?;
    let group_id =
        ExtensionGroupId::from_str(&group_path).map_err(|_| bad_id("extension_group_id"))?;
    let record = state
        .extension_groups
        .read()
        .map_err(|_| service_unavailable("extension_group"))?
        .get(&principal, &grants, &workspace_id, &group_id)
        .map_err(response_problem)?;
    Ok(Json(record))
}

async fn create_extension_group(
    State(state): State<ApiState>,
    Path(workspace_path): Path<String>,
    headers: HeaderMap,
    Json(input): Json<crate::services::extension_groups::ExtensionGroupCreateInput>,
) -> Result<Response, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let workspace_id =
        WorkspaceId::from_str(&workspace_path).map_err(|_| bad_id("workspace_id"))?;
    if input.workspace_id != workspace_id {
        return Err(problem_response(
            "workspace_scope_mismatch",
            "Workspace scope does not match",
            403,
            correlation(&headers),
        ));
    }
    let value = serde_json::to_value(&input).map_err(internal)?;
    let group_id = input.extension_group_id.clone();
    mutate(
        &state,
        &headers,
        &principal,
        &grants,
        workspace_id.as_str(),
        "extension.group.create",
        value,
        ResourceScope::workspace(workspace_id.clone()),
        group_id.as_str(),
        || {
            state
                .extension_groups
                .write()
                .map_err(|_| service_unavailable("extension_group"))?
                .create(&principal, &grants, &workspace_id, input.clone())
                .map_err(response_problem)
        },
    )
    .await
}

async fn update_extension_group(
    State(state): State<ApiState>,
    Path((workspace_path, group_path)): Path<(String, String)>,
    headers: HeaderMap,
    Json(input): Json<crate::services::extension_groups::ExtensionGroupUpdateInput>,
) -> Result<Response, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let workspace_id =
        WorkspaceId::from_str(&workspace_path).map_err(|_| bad_id("workspace_id"))?;
    let group_id =
        ExtensionGroupId::from_str(&group_path).map_err(|_| bad_id("extension_group_id"))?;
    if input.workspace_id != workspace_id || input.extension_group_id != group_id {
        return Err(problem_response(
            "extension_group_scope_mismatch",
            "Extension group scope does not match",
            400,
            correlation(&headers),
        ));
    }
    let value = serde_json::to_value(&input).map_err(internal)?;
    mutate(
        &state,
        &headers,
        &principal,
        &grants,
        workspace_id.as_str(),
        "extension.group.update",
        value,
        ResourceScope::workspace(workspace_id.clone()),
        group_id.as_str(),
        || {
            state
                .extension_groups
                .write()
                .map_err(|_| service_unavailable("extension_group"))?
                .update(&principal, &grants, &workspace_id, input.clone())
                .map_err(response_problem)
        },
    )
    .await
}

async fn delete_extension_group(
    State(state): State<ApiState>,
    Path((workspace_path, group_path)): Path<(String, String)>,
    headers: HeaderMap,
    Json(input): Json<crate::services::extension_groups::ExtensionGroupDeleteInput>,
) -> Result<Response, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let workspace_id =
        WorkspaceId::from_str(&workspace_path).map_err(|_| bad_id("workspace_id"))?;
    let group_id =
        ExtensionGroupId::from_str(&group_path).map_err(|_| bad_id("extension_group_id"))?;
    if input.workspace_id != workspace_id || input.extension_group_id != group_id {
        return Err(problem_response(
            "extension_group_scope_mismatch",
            "Extension group scope does not match",
            400,
            correlation(&headers),
        ));
    }
    let value = serde_json::to_value(&input).map_err(internal)?;
    mutate(
        &state,
        &headers,
        &principal,
        &grants,
        workspace_id.as_str(),
        "extension.group.delete",
        value,
        ResourceScope::workspace(workspace_id.clone()),
        group_id.as_str(),
        || {
            let in_use = state
                .extension_group_is_in_use(&workspace_id, &group_id)
                .map_err(response_problem)?;
            state
                .extension_groups
                .write()
                .map_err(|_| service_unavailable("extension_group"))?
                .delete(&principal, &grants, &workspace_id, input.clone(), in_use)
                .map_err(response_problem)
        },
    )
    .await
}

async fn set_profile_extension_group(
    State(state): State<ApiState>,
    Path(profile_path): Path<String>,
    headers: HeaderMap,
    Json(input): Json<crate::services::extension_groups::ProfileExtensionGroupSetInput>,
) -> Result<Response, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let profile_id = ProfileId::from_str(&profile_path).map_err(|_| bad_id("profile_id"))?;
    if input.profile_id != profile_id {
        return Err(bad_id("profile_id"));
    }
    let binding = state
        .profile_resource_record(&profile_id)
        .map_err(response_problem)?
        .ok_or_else(|| {
            problem_response(
                "binding_not_found",
                "Profile binding not found",
                404,
                correlation(&headers),
            )
        })?;
    if binding.workspace_id != input.workspace_id {
        return Err(problem_response(
            "profile_scope_mismatch",
            "Profile workspace does not match request",
            403,
            correlation(&headers),
        ));
    }
    authorize_action(
        &state,
        &principal,
        &grants,
        &profile_id,
        Action::Update,
        "CAP-EXT-002",
    )?;
    let value = serde_json::to_value(&input).map_err(internal)?;
    let workspace_id = binding.workspace_id;
    let resource_id = binding.resource_id;
    mutate(
        &state,
        &headers,
        &principal,
        &grants,
        workspace_id.as_str(),
        "profile.extension.group.set",
        value,
        ResourceScope {
            workspace_id: Some(workspace_id.clone()),
            resource_id: Some(resource_id),
            profile_id: Some(profile_id.clone()),
            ..ResourceScope::default()
        },
        profile_id.as_str(),
        || {
            let updated = state
                .set_hosted_profile_extension_group(input.clone())
                .map_err(response_problem)?;
            Ok(buglogin_domain::entities::ProfileRecord::new(
                updated.value,
                updated.revision,
            ))
        },
    )
    .await
}

async fn list_group_shares(
    State(state): State<ApiState>,
    Path((workspace_path, group_path)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<Vec<buglogin_domain::grant::GroupShareGrant>>, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let workspace_id =
        WorkspaceId::from_str(&workspace_path).map_err(|_| bad_id("workspace_id"))?;
    let group_id = GroupId::from_str(&group_path).map_err(|_| bad_id("group_id"))?;
    let items = state
        .groups
        .read()
        .map_err(|_| service_unavailable("group"))?
        .list_share_grants(&principal, &grants, &workspace_id, Some(&group_id), None)
        .map_err(response_problem)?;
    Ok(Json(items))
}

async fn create_group_share(
    State(state): State<ApiState>,
    Path((workspace_path, group_path)): Path<(String, String)>,
    headers: HeaderMap,
    Json(input): Json<crate::services::groups::GroupShareCreateInput>,
) -> Result<Response, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let workspace_id =
        WorkspaceId::from_str(&workspace_path).map_err(|_| bad_id("workspace_id"))?;
    let group_id = GroupId::from_str(&group_path).map_err(|_| bad_id("group_id"))?;
    if input.workspace_id != workspace_id || input.group_id != group_id {
        return Err(problem_response(
            "group_scope_mismatch",
            "Group share scope does not match path",
            403,
            correlation(&headers),
        ));
    }
    let value = serde_json::to_value(&input).map_err(internal)?;
    mutate(
        &state,
        &headers,
        &principal,
        &grants,
        workspace_id.as_str(),
        "group.share.create",
        value,
        ResourceScope::workspace(workspace_id.clone()),
        group_id.as_str(),
        || {
            HostedProfileService::create_group_share(&state, &principal, &grants, input.clone())
                .map_err(response_problem)
        },
    )
    .await
}

async fn revoke_group_share(
    State(state): State<ApiState>,
    Path((workspace_path, group_path, grant_path)): Path<(String, String, String)>,
    headers: HeaderMap,
    Json(input): Json<crate::services::groups::GroupShareRevokeInput>,
) -> Result<Response, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let workspace_id =
        WorkspaceId::from_str(&workspace_path).map_err(|_| bad_id("workspace_id"))?;
    let group_id = GroupId::from_str(&group_path).map_err(|_| bad_id("group_id"))?;
    let grant_id = GrantId::from_str(&grant_path).map_err(|_| bad_id("grant_id"))?;
    if input.workspace_id != workspace_id
        || input.group_id != group_id
        || input.grant_id != grant_id
    {
        return Err(problem_response(
            "group_share_scope_mismatch",
            "Group share scope does not match path",
            403,
            correlation(&headers),
        ));
    }
    let value = serde_json::to_value(&input).map_err(internal)?;
    mutate(
        &state,
        &headers,
        &principal,
        &grants,
        workspace_id.as_str(),
        "group.share.revoke",
        value,
        ResourceScope::workspace(workspace_id.clone()),
        grant_id.as_str(),
        || {
            HostedProfileService::revoke_group_share(&state, &principal, &grants, input.clone())
                .map_err(response_problem)
        },
    )
    .await
}

pub(super) fn encrypted_profiles_missing_device_envelopes(
    state: &ApiState,
    account_id: &AccountId,
    device_id: &str,
) -> Result<Vec<(WorkspaceId, ResourceId)>, ResponseProblem> {
    encrypted_profile_device_envelope_resources(state, account_id, device_id, false)
}

pub(super) fn encrypted_profiles_with_device_envelopes(
    state: &ApiState,
    account_id: &AccountId,
    device_id: &str,
) -> Result<Vec<(WorkspaceId, ResourceId)>, ResponseProblem> {
    encrypted_profile_device_envelope_resources(state, account_id, device_id, true)
}

fn encrypted_profile_device_envelope_resources(
    state: &ApiState,
    account_id: &AccountId,
    device_id: &str,
    require_existing_envelope: bool,
) -> Result<Vec<(WorkspaceId, ResourceId)>, ResponseProblem> {
    let memberships = state
        .members
        .read()
        .map_err(|_| service_unavailable("member"))?
        .active_for_account(account_id);
    let mut members_by_workspace = std::collections::BTreeMap::<WorkspaceId, Vec<MemberId>>::new();
    for member in memberships {
        members_by_workspace
            .entry(member.workspace_id)
            .or_default()
            .push(member.id);
    }

    let mut affected = std::collections::BTreeSet::new();
    for profile in state.list_profile_records().map_err(response_problem)? {
        let Some(binding) = state
            .profile_resource_record(&profile.value.id)
            .map_err(response_problem)?
        else {
            continue;
        };
        let Some(member_ids) = members_by_workspace.get(&binding.workspace_id) else {
            continue;
        };
        let Some(manifest) = state
            .current_sync_manifest(&binding.workspace_id, &binding.resource_id)
            .map_err(response_problem)?
            .filter(|manifest| manifest.payload_encryption.is_some())
        else {
            continue;
        };
        if let Some(key_version) = manifest
            .payload_encryption
            .as_ref()
            .and_then(|encryption| encryption.key_version.as_deref())
            && state
                .has_managed_profile_key_envelope(
                    &profile.value.id,
                    &binding.workspace_id,
                    &binding.resource_id,
                    key_version,
                )
                .map_err(response_problem)?
        {
            // Service-managed envelope access is authorized against the current
            // profile ACL. Device enrollment/revocation no longer requires
            // client-side envelope fanout or fingerprint confirmation.
            continue;
        }
        let recipients = HostedProfileService::effective_profile_key_recipients(
            state,
            &profile.value.id,
            &binding,
            &[Action::Read, Action::Execute],
        )
        .map_err(response_problem)?;
        let account_is_recipient = member_ids
            .iter()
            .any(|member_id| recipients.contains(member_id));
        if account_is_recipient && !require_existing_envelope {
            if recipients.len() > crate::services::profile_hosted::MAX_PROFILE_KEY_ROTATION_MEMBERS
            {
                return Err(ResponseProblem(
                    413,
                    json!({
                        "code": "profile_key_rotation_recipient_limit_exceeded",
                        "title": "This device enrollment would exceed the encrypted profile recipient limit",
                    }),
                ));
            }
            let active_device_count = state
                .auth
                .read()
                .map_err(|_| service_unavailable("auth"))?
                .active_device_ids(
                    account_id,
                    checked_now("device-enrollment").map_err(response_problem)?,
                )
                .map_err(response_problem)?
                .len();
            if active_device_count
                > crate::services::profile_hosted::MAX_PROFILE_KEY_ROTATION_DEVICES
            {
                return Err(ResponseProblem(
                    413,
                    json!({
                        "code": "profile_key_rotation_device_limit_exceeded",
                        "title": "This device enrollment would exceed the encrypted profile device limit",
                    }),
                ));
            }
        }
        let mut fence_required = false;
        for member_id in member_ids {
            if !recipients.contains(member_id) {
                continue;
            }
            let envelope_exists = state
                .profile_key_share_envelope(&binding.resource_id, member_id, device_id)
                .map_err(response_problem)?;
            let active_key_version = manifest
                .payload_encryption
                .as_ref()
                .and_then(|encryption| encryption.key_version.as_deref());
            let has_current_envelope = envelope_exists
                .as_ref()
                .is_some_and(|envelope| Some(envelope.key_version.as_str()) == active_key_version);
            // Any currently valid envelope must be rotated when its device is
            // revoked or re-enrolled; otherwise the old device retains the active
            // profile key. Stale envelopes already cannot decrypt the active key.
            if has_current_envelope {
                fence_required = true;
                break;
            }
        }
        if fence_required {
            affected.insert((binding.workspace_id, binding.resource_id));
            if affected.len() > crate::services::profile_hosted::MAX_KEY_ROTATION_PROFILE_FANOUT {
                return Err(ResponseProblem(
                    413,
                    json!({
                        "code": "profile_key_rotation_fanout_exceeded",
                        "title": "This device enrollment affects too many encrypted profiles to rotate atomically",
                    }),
                ));
            }
        }
    }
    Ok(affected.into_iter().collect())
}

async fn replace_profile_groups(
    State(state): State<ApiState>,
    Path(profile_path): Path<String>,
    headers: HeaderMap,
    Json(input): Json<crate::services::groups::ProfileGroupsReplaceInput>,
) -> Result<Response, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let profile_id = ProfileId::from_str(&profile_path).map_err(|_| bad_id("profile_id"))?;
    if input.profile_id != profile_id {
        return Err(bad_id("profile_id"));
    }
    let binding = state
        .profile_resource_record(&profile_id)
        .map_err(response_problem)?
        .ok_or_else(|| {
            problem_response(
                "binding_not_found",
                "Profile binding not found",
                404,
                correlation(&headers),
            )
        })?;
    if binding.workspace_id != input.workspace_id {
        return Err(problem_response(
            "profile_scope_mismatch",
            "Profile workspace does not match request",
            403,
            correlation(&headers),
        ));
    }
    authorize_action(
        &state,
        &principal,
        &grants,
        &profile_id,
        Action::Update,
        "CAP-PROF-003",
    )?;
    let value = serde_json::to_value(&input).map_err(internal)?;
    let workspace_id = binding.workspace_id;
    let resource_id = binding.resource_id;
    mutate(
        &state,
        &headers,
        &principal,
        &grants,
        workspace_id.as_str(),
        "profile.groups.replace",
        value,
        ResourceScope {
            workspace_id: Some(workspace_id.clone()),
            resource_id: Some(resource_id.clone()),
            profile_id: Some(profile_id.clone()),
            ..ResourceScope::default()
        },
        profile_id.as_str(),
        || {
            let updated = HostedProfileService::replace_profile_groups(&state, input.clone())
                .map_err(response_problem)?;
            Ok(buglogin_domain::entities::ProfileRecord::new(
                updated.value,
                updated.revision,
            ))
        },
    )
    .await
}

async fn get_profile_proxy_assignment(
    State(state): State<ApiState>,
    Path((workspace_path, profile_path)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<crate::services::profile_proxy_assignment::ProfileProxyAssignment>, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let workspace_id =
        WorkspaceId::from_str(&workspace_path).map_err(|_| bad_id("workspace_id"))?;
    let profile_id = ProfileId::from_str(&profile_path).map_err(|_| bad_id("profile_id"))?;
    let binding = state
        .profile_resource_record(&profile_id)
        .map_err(response_problem)?
        .filter(|binding| binding.workspace_id == workspace_id)
        .ok_or_else(|| {
            problem_response(
                "binding_not_found",
                "Profile binding not found",
                404,
                correlation(&headers),
            )
        })?;
    let _ = binding;
    authorize_action(
        &state,
        &principal,
        &grants,
        &profile_id,
        Action::Read,
        "profile.read",
    )?;
    let assignment = state
        .profile_proxy_assignments
        .read()
        .map_err(|_| service_unavailable("profile proxy assignment"))?
        .get(&workspace_id, &profile_id)
        .map_err(response_problem)?;
    let _profile = state
        .get_profile_record(&profile_id)
        .map_err(response_problem)?
        .ok_or_else(|| {
            problem_response(
                "profile_not_found",
                "Profile not found",
                404,
                correlation(&headers),
            )
        })?;
    Ok(Json(assignment))
}

async fn set_profile_proxy_assignment(
    State(state): State<ApiState>,
    Path((workspace_path, profile_path)): Path<(String, String)>,
    headers: HeaderMap,
    Json(input): Json<crate::services::profile_proxy_assignment::ProfileProxyAssignmentSetInput>,
) -> Result<Response, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let workspace_id =
        WorkspaceId::from_str(&workspace_path).map_err(|_| bad_id("workspace_id"))?;
    let profile_id = ProfileId::from_str(&profile_path).map_err(|_| bad_id("profile_id"))?;
    if input.workspace_id != workspace_id || input.profile_id != profile_id {
        return Err(problem_response(
            "profile_scope_mismatch",
            "Profile assignment scope does not match request",
            403,
            correlation(&headers),
        ));
    }
    let binding = state
        .profile_resource_record(&profile_id)
        .map_err(response_problem)?
        .filter(|binding| binding.workspace_id == workspace_id)
        .ok_or_else(|| {
            problem_response(
                "binding_not_found",
                "Profile binding not found",
                404,
                correlation(&headers),
            )
        })?;
    let profile = state
        .get_profile_record(&profile_id)
        .map_err(response_problem)?
        .ok_or_else(|| {
            problem_response(
                "profile_not_found",
                "Profile not found",
                404,
                correlation(&headers),
            )
        })?;
    if state.local_authority_enabled() {
        return Err(problem_response(
            "hosted_proxy_assignment_required",
            "Managed proxy reassignment requires hosted profile sync",
            503,
            correlation(&headers),
        ));
    }
    let pending_assignment = state
        .profile_proxy_assignments
        .read()
        .map_err(|_| service_unavailable("profile proxy assignment"))?
        .get_optional(&workspace_id, &profile_id);
    crate::services::profile_proxy_assignment::ProfileProxyAssignmentService::validate_change(
        &profile.value,
        profile.revision,
        &input,
        pending_assignment.as_ref(),
    )
    .map_err(|mut error| {
        error.correlation_id = correlation(&headers);
        response_problem(error)
    })?;
    authorize_action(
        &state,
        &principal,
        &grants,
        &profile_id,
        Action::Update,
        "profile.update",
    )?;
    if let Some(proxy_id) = input.proxy_id.as_ref() {
        state
            .proxies
            .read()
            .map_err(|_| service_unavailable("proxy"))?
            .get(&workspace_id, proxy_id, &principal, &grants)
            .map_err(proxy_response_problem)?;
    }
    let value = serde_json::to_value(&input).map_err(internal)?;
    let scope = ResourceScope {
        workspace_id: Some(binding.workspace_id.clone()),
        resource_id: Some(binding.resource_id.clone()),
        profile_id: Some(profile_id.clone()),
        ..ResourceScope::default()
    };
    mutate(
        &state,
        &headers,
        &principal,
        &grants,
        workspace_id.as_str(),
        "profile.proxy.assignment.set",
        value,
        scope,
        profile_id.as_str(),
        || {
            let current_binding = state
                .profile_resource_record(&profile_id)
                .map_err(response_problem)?
                .filter(|current| current.workspace_id == workspace_id)
                .ok_or_else(|| {
                    response_problem(Problem::new(
                        "profile_binding_changed",
                        "Profile binding changed before proxy assignment",
                        409,
                        "profile",
                    ))
                })?;
            if current_binding.resource_id != binding.resource_id {
                return Err(response_problem(Problem::new(
                    "profile_binding_changed",
                    "Profile resource changed before proxy assignment",
                    409,
                    "profile",
                )));
            }
            let current = state
                .get_profile_record(&profile_id)
                .map_err(response_problem)?
                .ok_or_else(|| {
                    response_problem(Problem::new(
                        "profile_not_found",
                        "Profile not found",
                        404,
                        correlation(&headers),
                    ))
                })?;
            let pending_assignment = state
                .profile_proxy_assignments
                .read()
                .map_err(|_| service_unavailable("profile proxy assignment"))?
                .get_optional(&workspace_id, &profile_id);
            crate::services::profile_proxy_assignment::ProfileProxyAssignmentService::validate_change(
                &current.value,
                current.revision,
                &input,
                pending_assignment.as_ref(),
            )
            .map_err(|mut error| {
                error.correlation_id = correlation(&headers);
                response_problem(error)
            })?;
            if let Some(proxy_id) = input.proxy_id.as_ref() {
                state
                    .proxies
                    .read()
                    .map_err(|_| service_unavailable("proxy"))?
                    .get(&workspace_id, proxy_id, &principal, &grants)
                    .map_err(proxy_response_problem)?;
            }
            state
                .profile_proxy_assignments
                .write()
                .map_err(|_| service_unavailable("profile proxy assignment"))?
                .set(&workspace_id, input.clone())
                .map_err(response_problem)
        },
    )
    .await
}

async fn update_profile_metadata(
    State(state): State<ApiState>,
    Path((workspace_path, profile_path)): Path<(String, String)>,
    headers: HeaderMap,
    Json(input): Json<crate::services::profile_hosted::ProfileUpdateInput>,
) -> Result<Response, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let workspace_id =
        WorkspaceId::from_str(&workspace_path).map_err(|_| bad_id("workspace_id"))?;
    let profile_id = ProfileId::from_str(&profile_path).map_err(|_| bad_id("profile_id"))?;
    if input.workspace_id != workspace_id || input.profile_id != profile_id {
        return Err(problem_response(
            "profile_scope_mismatch",
            "Profile workspace and identity must match the request path",
            403,
            correlation(&headers),
        ));
    }
    let binding = state
        .profile_resource_record(&profile_id)
        .map_err(response_problem)?
        .filter(|binding| binding.workspace_id == workspace_id)
        .ok_or_else(|| {
            problem_response(
                "binding_not_found",
                "Profile binding not found",
                404,
                correlation(&headers),
            )
        })?;
    authorize_action(
        &state,
        &principal,
        &grants,
        &profile_id,
        Action::Update,
        "profile.update",
    )?;
    let command_input = serde_json::to_value(&input).map_err(internal)?;
    let resource_id = binding.resource_id;
    let workspace_scope = workspace_id.clone();
    mutate(
        &state,
        &headers,
        &principal,
        &grants,
        workspace_id.as_str(),
        "profile.update",
        command_input,
        ResourceScope {
            workspace_id: Some(workspace_scope),
            resource_id: Some(resource_id.clone()),
            profile_id: Some(profile_id),
            ..ResourceScope::default()
        },
        resource_id.as_str(),
        || {
            let updated = HostedProfileService::update_metadata(&state, &principal, &grants, input)
                .map_err(response_problem)?;
            Ok(buglogin_domain::entities::ProfileRecord::new(
                updated.value,
                updated.revision,
            ))
        },
    )
    .await
}

fn team_scope(workspace_id: &WorkspaceId, team_id: Option<&TeamId>) -> ResourceScope {
    ResourceScope {
        workspace_id: Some(workspace_id.clone()),
        team_id: team_id.cloned(),
        ..ResourceScope::default()
    }
}

fn authorize_control(
    principal: &Principal,
    grants: &[buglogin_domain::permissions::Grant],
    capability: &str,
    action: Action,
    scope: &ResourceScope,
    code: &'static str,
    title: &'static str,
) -> Result<(), Response> {
    let capability = Capability::new(capability).map_err(|_| {
        problem_response(
            "invalid_capability",
            "Invalid capability",
            500,
            "unknown".into(),
        )
    })?;
    if is_allowed(principal, &capability, action, scope, grants) {
        Ok(())
    } else {
        Err(problem_response(code, title, 403, "unknown".into()))
    }
}

fn mutation_key(headers: &HeaderMap, correlation: &str) -> Result<String, Response> {
    headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
        .filter(|value| value.is_ascii() && (16..=128).contains(&value.len()))
        .map(str::to_owned)
        .ok_or_else(|| {
            problem_response(
                "idempotency_key_required",
                "Valid idempotency key required",
                400,
                correlation.into(),
            )
        })
}

fn input_hash(input: &serde_json::Value, correlation: &str) -> Result<String, Response> {
    serde_json::to_vec(input)
        .map(|bytes| format!("{:x}", Sha256::digest(bytes)))
        .map_err(|_| problem_response("invalid_input", "Input is invalid", 400, correlation.into()))
}

async fn account(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<AccountView>, Response> {
    let principal = authenticate(&state, &headers)?;
    let account_id = principal.account_id.clone().ok_or_else(|| {
        response(problem(
            "account_missing",
            "Authenticated account is missing",
            401,
        ))
    })?;
    let grant_snapshot = grants(&state)?;
    let workspaces = state
        .workspaces
        .read()
        .map_err(|_| service_unavailable("workspace"))?;
    let managed_workspace_ids = workspaces.managed_workspace_ids(&principal, &grant_snapshot);
    let manageable_workspace_ids = workspaces.manageable_workspace_ids(&principal, &grant_snapshot);
    let manageable_organization_ids =
        workspaces.manageable_organization_ids(&principal, &grant_snapshot);
    Ok(Json(AccountView {
        account_id,
        platform_admin: principal
            .roles
            .contains(&buglogin_domain::permissions::Role::PlatformAdmin),
        managed_workspace_ids,
        manageable_workspace_ids,
        manageable_organization_ids,
    }))
}

async fn list_organizations(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<Vec<Organization>>, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let items = state
        .workspaces
        .read()
        .map_err(|_| service_unavailable("workspace"))?
        .list_organizations(&principal, &grants)
        .map_err(response_problem)?;
    Ok(Json(items))
}

async fn get_organization(
    State(state): State<ApiState>,
    Path(organization_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Organization>, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let organization_id =
        OrganizationId::from_str(&organization_id).map_err(|_| bad_id("organization_id"))?;
    let record = state
        .workspaces
        .read()
        .map_err(|_| service_unavailable("workspace"))?
        .get_organization(&principal, &grants, &organization_id)
        .map_err(response_problem)?;
    Ok(Json(record.organization))
}

async fn get_organization_record(
    State(state): State<ApiState>,
    Path(organization_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<crate::services::workspaces::OrganizationRecord>, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let organization_id =
        OrganizationId::from_str(&organization_id).map_err(|_| bad_id("organization_id"))?;
    let record = state
        .workspaces
        .read()
        .map_err(|_| service_unavailable("workspace"))?
        .get_organization(&principal, &grants, &organization_id)
        .map_err(response_problem)?;
    Ok(Json(record))
}

async fn list_organization_members(
    State(state): State<ApiState>,
    Path(organization_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, Response> {
    let organization_id =
        OrganizationId::from_str(&organization_id).map_err(|_| bad_id("organization_id"))?;
    let principal = authenticate(&state, &headers)?;
    let grant_snapshot = grants(&state)?;
    authorize_organization_management(
        &state,
        &principal,
        &grant_snapshot,
        &organization_id,
        Action::Read,
    )?;
    let members = state
        .organization_members
        .read()
        .map_err(|_| service_unavailable("organization_member"))?
        .list_for_organization(&organization_id);
    let items = members
        .iter()
        .map(|member| organization_member_resource(&state, member))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Json(json!({"items": items, "nextCursor": Value::Null})))
}

fn organization_member_resource(
    state: &ApiState,
    member: &buglogin_domain::organization::OrganizationMember,
) -> Result<Value, Response> {
    let identity = state
        .control_persistence
        .load::<AccountIdentity>("identity", "account_identity", member.account_id.as_str())
        .map_err(response_problem)?
        .ok_or_else(|| {
            problem_response(
                "organization_member_identity_missing",
                "Organization member identity is unavailable",
                500,
                "organization.members.list".into(),
            )
        })?;
    organization_member_resource_with_identity(member, &identity)
}

fn organization_member_resource_with_identity(
    member: &buglogin_domain::organization::OrganizationMember,
    identity: &AccountIdentity,
) -> Result<Value, Response> {
    let mut value = serde_json::to_value(member).map_err(internal)?;
    let object = value.as_object_mut().ok_or_else(|| {
        problem_response(
            "organization_member_serialization_failed",
            "Organization member could not be serialized",
            500,
            "organization.members.list".into(),
        )
    })?;
    object.insert("username".into(), Value::String(identity.username.clone()));
    object.insert("email".into(), Value::String(identity.email.clone()));
    Ok(value)
}

async fn create_organization_account(
    State(state): State<ApiState>,
    Path(organization_id): Path<String>,
    headers: HeaderMap,
    Json(input): Json<OrganizationAccountCreateInput>,
) -> Result<Response, Response> {
    let organization_id =
        OrganizationId::from_str(&organization_id).map_err(|_| bad_id("organization_id"))?;
    if input.organization_id != organization_id {
        return Err(problem_response(
            "tenant_mismatch",
            "Organization identifier does not match the request path",
            403,
            correlation(&headers),
        ));
    }
    let principal = authenticate(&state, &headers)?;
    let grant_snapshot = grants(&state)?;
    let organization = authorize_organization_management(
        &state,
        &principal,
        &grant_snapshot,
        &organization_id,
        Action::Create,
    )?;
    crate::services::credentials::CredentialService::validate_initial_password(
        &input.initial_password,
    )
    .map_err(response_problem)?;
    let credential_now =
        crate::clock::unix_time_seconds(&correlation(&headers)).map_err(response_problem)?;
    let mutation_input = serde_json::to_value(&input).map_err(internal)?;
    let initial_password = Zeroizing::new(input.initial_password);
    let account_id = input.account_id.clone();
    let username = input.username.clone();
    let email = input.email.clone();
    let scope = ResourceScope {
        account_id: Some(organization.account_id.clone()),
        organization_id: Some(organization.id.clone()),
        ..ResourceScope::default()
    };
    mutate_with_additional_puts_and_replay_check(
        &state,
        &headers,
        &principal,
        &grant_snapshot,
        organization.id.as_str(),
        "organization.account.create",
        mutation_input,
        scope,
        account_id.as_str(),
        || {
            let credentials = crate::services::credentials::CredentialService::new(
                state.control_persistence.clone(),
            )
            .map_err(response_problem)?;
            if credentials
                .matches_password_without_recording_attempt(&account_id, initial_password.as_str())
                .map_err(response_problem)?
            {
                Ok(())
            } else {
                Err(response_problem(Problem::new(
                    "idempotency_conflict",
                    "Idempotency key was already used with a different initial password",
                    409,
                    "identity",
                )))
            }
        },
        || {
            let (mut pending, mut account_puts) =
                crate::services::account_provisioning::AccountProvisioningService::new(
                    state.control_persistence.clone(),
                )
                .prepare_admin_provisioned_scoped(
                    account_id.clone(),
                    &username,
                    &email,
                    &crate::clock::rfc3339(&correlation(&headers)).map_err(response_problem)?,
                )
                .map_err(response_problem)?;
            management::set_provisioned_display_name(
                &mut pending.identity,
                &mut account_puts,
                &input.display_name,
            )?;
            let credential_put = crate::services::credentials::CredentialService::new(
                state.control_persistence.clone(),
            )
            .map_err(response_problem)?
            .prepare_initial_password_put(
                &pending.account.id,
                initial_password.as_str(),
                credential_now,
            )
            .map_err(response_problem)?;
            account_puts.push(credential_put);
            let organization_member = state
                .organization_members
                .write()
                .map_err(|_| service_unavailable("organization_member"))?
                .create_active(organization_id.clone(), pending.account.id.clone())
                .map_err(response_problem)?;
            let organization_member = json!({
                "id": organization_member.id,
                "organizationId": organization_member.organization_id,
                "accountId": organization_member.account_id,
                "status": organization_member.status,
                "revision": organization_member.revision,
                "username": pending.identity.username.clone(),
                "email": pending.identity.email.clone(),
            });
            Ok((
                json!({
                    "account": pending.account,
                    "identity": pending.identity,
                    "organizationMember": organization_member,
                    "replayed": pending.replayed,
                    "onboardingStatus": "admin_provisioned",
                    "loginEnabled": true,
                }),
                account_puts,
                None,
            ))
        },
    )
    .await
}

async fn update_organization_member_status(
    State(state): State<ApiState>,
    Path((organization_id, account_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(input): Json<OrganizationMemberStatusUpdateInput>,
) -> Result<Response, Response> {
    let organization_id =
        OrganizationId::from_str(&organization_id).map_err(|_| bad_id("organization_id"))?;
    let account_id = AccountId::from_str(&account_id).map_err(|_| bad_id("account_id"))?;
    if input.organization_id != organization_id || input.account_id != account_id {
        return Err(problem_response(
            "tenant_mismatch",
            "Organization member identifiers do not match the request path",
            403,
            correlation(&headers),
        ));
    }
    if input.expected_revision == 0 {
        return Err(problem_response(
            "invalid_revision",
            "Expected revision must be positive",
            400,
            correlation(&headers),
        ));
    }
    let principal = authenticate(&state, &headers)?;
    let grant_snapshot = grants(&state)?;
    let organization = authorize_organization_management(
        &state,
        &principal,
        &grant_snapshot,
        &organization_id,
        Action::Manage,
    )?;
    if input.status == OrganizationMemberStatus::Suspended && organization.account_id == account_id
    {
        return Err(problem_response(
            "organization_owner_protected",
            "The organization owner cannot be suspended",
            409,
            correlation(&headers),
        ));
    }
    let mutation_input = serde_json::to_value(&input).map_err(internal)?;
    let scope = ResourceScope {
        account_id: Some(organization.account_id),
        organization_id: Some(organization_id.clone()),
        ..ResourceScope::default()
    };
    mutate(
        &state,
        &headers,
        &principal,
        &grant_snapshot,
        organization_id.as_str(),
        "organization.member.status.update",
        mutation_input,
        scope,
        account_id.as_str(),
        || {
            state
                .organization_members
                .write()
                .map_err(|_| service_unavailable("organization_member"))?
                .update_status(
                    &organization_id,
                    &account_id,
                    input.expected_revision,
                    input.status,
                )
                .map_err(response_problem)
        },
    )
    .await
}

async fn assign_organization_member_to_workspace(
    State(state): State<ApiState>,
    Path((organization_path, workspace_path)): Path<(String, String)>,
    headers: HeaderMap,
    Json(input): Json<OrganizationWorkspaceMemberAssignInput>,
) -> Result<Response, Response> {
    let organization_id =
        OrganizationId::from_str(&organization_path).map_err(|_| bad_id("organization_id"))?;
    let workspace_id =
        WorkspaceId::from_str(&workspace_path).map_err(|_| bad_id("workspace_id"))?;
    if input.organization_id != organization_id || input.workspace_id != workspace_id {
        return Err(problem_response(
            "tenant_mismatch",
            "Organization or workspace identifier does not match the request path",
            403,
            correlation(&headers),
        ));
    }
    if input.organization_member_revision == 0 {
        return Err(problem_response(
            "invalid_revision",
            "Organization member revision must be positive",
            400,
            correlation(&headers),
        ));
    }
    let principal = authenticate(&state, &headers)?;
    let grant_snapshot = grants(&state)?;
    let organization = authorize_organization_management(
        &state,
        &principal,
        &grant_snapshot,
        &organization_id,
        Action::Manage,
    )?;
    let workspace_ids = state
        .workspaces
        .read()
        .map_err(|_| service_unavailable("workspace"))?
        .workspace_ids_in_organization(&organization_id);
    if !workspace_ids.contains(&workspace_id) {
        return Err(problem_response(
            "workspace_organization_mismatch",
            "Workspace does not belong to this organization",
            403,
            correlation(&headers),
        ));
    }
    let organization_owner = principal.account_id.as_ref() == Some(&organization.account_id);
    let organization_member = state
        .organization_members
        .read()
        .map_err(|_| service_unavailable("organization_member"))?
        .list_for_organization(&organization_id)
        .into_iter()
        .find(|member| member.id == input.organization_member_id)
        .ok_or_else(|| {
            problem_response(
                "organization_member_not_found",
                "Organization member not found",
                404,
                correlation(&headers),
            )
        })?;
    if organization_member.revision != input.organization_member_revision {
        return Err(problem_response(
            "revision_conflict",
            "Organization member revision conflict",
            409,
            correlation(&headers),
        ));
    }
    if organization_member.status != OrganizationMemberStatus::Active {
        return Err(problem_response(
            "organization_member_inactive",
            "Suspended organization members cannot be assigned to a workspace",
            409,
            correlation(&headers),
        ));
    }
    if organization_owner {
        MemberService::authorize_org_owner_account_creation(
            &principal,
            &grant_snapshot,
            &organization,
            &workspace_id,
            &input.role,
        )
        .map_err(response_problem)?;
    }
    let member = Member {
        id: crate::services::account_provisioning::scoped_member_id(
            &workspace_id,
            &organization_member.account_id,
        )
        .map_err(response_problem)?,
        account_id: organization_member.account_id.clone(),
        workspace_id: workspace_id.clone(),
        team_id: None,
        role: input.role.clone(),
        status: MemberStatus::Active,
        invite_expires_at: None,
    };
    let mutation_input = serde_json::to_value(&input).map_err(internal)?;
    let output_member = member.clone();
    mutate(
        &state,
        &headers,
        &principal,
        &grant_snapshot,
        organization_id.as_str(),
        "organization.member.workspace.assign",
        mutation_input,
        ResourceScope {
            account_id: Some(organization_member.account_id.clone()),
            organization_id: Some(organization_id.clone()),
            workspace_id: Some(workspace_id.clone()),
            ..ResourceScope::default()
        },
        organization_member.id.as_str(),
        || {
            // Hold the parent gate while committing the child membership so a
            // concurrent suspension cannot create access after revocation.
            let parent_members = state
                .organization_members
                .read()
                .map_err(|_| service_unavailable("organization_member"))?;
            let current_parent = parent_members
                .list_for_organization(&organization_id)
                .into_iter()
                .find(|current| current.id == input.organization_member_id)
                .ok_or_else(|| {
                    response_problem(Problem::new(
                        "organization_member_not_found",
                        "Organization member not found",
                        404,
                        correlation(&headers),
                    ))
                })?;
            if current_parent.status != OrganizationMemberStatus::Active
                || current_parent.revision != input.organization_member_revision
            {
                return Err(response_problem(Problem::new(
                    "revision_conflict",
                    "Active organization member revision changed",
                    409,
                    correlation(&headers),
                )));
            }
            let mut members = state
                .members
                .write()
                .map_err(|_| service_unavailable("member"))?;
            let assigned = if organization_owner {
                members.assign_active_as_org_owner(
                    &principal,
                    &grant_snapshot,
                    &organization,
                    output_member.clone(),
                )
            } else {
                members.provision_active(&principal, &grant_snapshot, output_member.clone())
            }
            .map_err(response_problem)?;
            Ok(assigned)
        },
    )
    .await
}

fn authorize_organization_management(
    state: &ApiState,
    principal: &Principal,
    grants: &[buglogin_domain::permissions::Grant],
    organization_id: &OrganizationId,
    action: Action,
) -> Result<Organization, Response> {
    Ok(state
        .workspaces
        .read()
        .map_err(|_| service_unavailable("workspace"))?
        .authorize_organization_management(principal, grants, organization_id, action)
        .map_err(response_problem)?)
}

async fn update_organization(
    State(state): State<ApiState>,
    Path(organization_id): Path<String>,
    headers: HeaderMap,
    Json(input): Json<OrganizationUpdate>,
) -> Result<Response, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let organization_id =
        OrganizationId::from_str(&organization_id).map_err(|_| bad_id("organization_id"))?;
    let value = serde_json::to_value(&input).map_err(internal)?;
    mutate(
        &state,
        &headers,
        &principal,
        &grants,
        organization_id.as_str(),
        "organization.update",
        value,
        ResourceScope {
            account_id: Some(input.organization.account_id.clone()),
            organization_id: Some(organization_id.clone()),
            ..ResourceScope::default()
        },
        organization_id.as_str(),
        || {
            state
                .workspaces
                .write()
                .map_err(|_| service_unavailable("workspace"))?
                .update_organization(&principal, &grants, &organization_id, input.clone())
                .map(|record| record.organization)
                .map_err(response_problem)
        },
    )
    .await
}

async fn list_workspaces(
    State(state): State<ApiState>,
    Path(organization_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Vec<Workspace>>, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let organization_id =
        OrganizationId::from_str(&organization_id).map_err(|_| bad_id("organization_id"))?;
    let items = state
        .workspaces
        .read()
        .map_err(|_| service_unavailable("workspace"))?
        .list_workspaces(&principal, &grants, &organization_id)
        .map_err(response_problem)?;
    Ok(Json(items))
}

async fn update_workspace(
    State(state): State<ApiState>,
    Path((organization_id, workspace_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(input): Json<WorkspaceUpdate>,
) -> Result<Response, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let organization_id =
        OrganizationId::from_str(&organization_id).map_err(|_| bad_id("organization_id"))?;
    let workspace_id = WorkspaceId::from_str(&workspace_id).map_err(|_| bad_id("workspace_id"))?;
    let default_extension_group_id = input.workspace.policy.default_extension_group_id.clone();
    let value = serde_json::to_value(&input).map_err(internal)?;
    mutate(
        &state,
        &headers,
        &principal,
        &grants,
        workspace_id.as_str(),
        "workspace.update",
        value,
        ResourceScope::workspace(workspace_id.clone()),
        workspace_id.as_str(),
        || {
            if let Some(extension_group_id) = &default_extension_group_id {
                state
                    .extension_groups
                    .read()
                    .map_err(|_| service_unavailable("extension_group"))?
                    .get(&principal, &grants, &workspace_id, extension_group_id)
                    .map_err(response_problem)?;
            }
            state
                .workspaces
                .write()
                .map_err(|_| service_unavailable("workspace"))?
                .update_workspace(
                    &principal,
                    &grants,
                    &organization_id,
                    &workspace_id,
                    input.clone(),
                )
                .map(|record| record.workspace)
                .map_err(response_problem)
        },
    )
    .await
}

async fn create_organization(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(input): Json<OrganizationInput>,
) -> Result<Response, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let input_value = serde_json::to_value(&input).map_err(internal)?;
    let scope = ResourceScope {
        account_id: Some(input.organization.account_id.clone()),
        organization_id: Some(input.organization.id.clone()),
        ..ResourceScope::default()
    };
    mutate(
        &state,
        &headers,
        &principal,
        &grants,
        "",
        "organization.create",
        input_value,
        scope,
        input.organization.id.as_str(),
        || {
            state
                .workspaces
                .write()
                .map_err(|_| service_unavailable("workspace"))?
                .add_organization(
                    &principal,
                    &grants,
                    input.organization.clone(),
                    input.policy.clone(),
                )
                .map_err(response_problem)
        },
    )
    .await
}

async fn create_workspace(
    State(state): State<ApiState>,
    Path(organization_id): Path<String>,
    headers: HeaderMap,
    Json(workspace): Json<Workspace>,
) -> Result<Response, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let organization_id =
        OrganizationId::from_str(&organization_id).map_err(|_| bad_id("organization_id"))?;
    let input_value = serde_json::json!({
        "organizationId": organization_id,
        "workspace": workspace,
    });
    let scope = ResourceScope {
        organization_id: Some(organization_id.clone()),
        workspace_id: Some(workspace.id.clone()),
        ..ResourceScope::default()
    };
    mutate(
        &state,
        &headers,
        &principal,
        &grants,
        workspace.id.as_str(),
        "workspace.create",
        input_value,
        scope,
        workspace.id.as_str(),
        || {
            let mut workspaces = state
                .workspaces
                .write()
                .map_err(|_| service_unavailable("workspace"))?;
            let mut members = state
                .members
                .write()
                .map_err(|_| service_unavailable("member"))?;
            workspaces
                .create_workspace(&principal, &grants, &organization_id, workspace.clone())
                .map_err(response_problem)?;
            let owner_candidates =
                workspaces.workspace_owner_membership_candidates(&members.all_members());
            members.backfill_workspace_owners(owner_candidates);
            Ok(workspace.clone())
        },
    )
    .await
}

async fn get_workspace(
    State(state): State<ApiState>,
    Path((organization_id, workspace_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<Workspace>, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let organization_id =
        OrganizationId::from_str(&organization_id).map_err(|_| bad_id("organization_id"))?;
    let workspace_id = WorkspaceId::from_str(&workspace_id).map_err(|_| bad_id("workspace_id"))?;
    Ok(Json(
        state
            .workspaces
            .read()
            .map_err(|_| service_unavailable("workspace"))?
            .get_workspace(&principal, &grants, &organization_id, &workspace_id)
            .map_err(response_problem)?,
    ))
}

async fn get_workspace_record(
    State(state): State<ApiState>,
    Path((organization_id, workspace_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<crate::services::workspaces::WorkspaceRecord>, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let organization_id =
        OrganizationId::from_str(&organization_id).map_err(|_| bad_id("organization_id"))?;
    let workspace_id = WorkspaceId::from_str(&workspace_id).map_err(|_| bad_id("workspace_id"))?;
    let record = state
        .workspaces
        .read()
        .map_err(|_| service_unavailable("workspace"))?
        .get_workspace_record(&principal, &grants, &organization_id, &workspace_id)
        .map_err(response_problem)?;
    Ok(Json(record))
}

async fn get_snapshot(
    State(state): State<ApiState>,
    Path((organization_id, workspace_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<buglogin_domain::organization::OrganizationSnapshot>, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let organization_id =
        OrganizationId::from_str(&organization_id).map_err(|_| bad_id("organization_id"))?;
    let workspace_id = WorkspaceId::from_str(&workspace_id).map_err(|_| bad_id("workspace_id"))?;
    Ok(Json(
        state
            .workspaces
            .read()
            .map_err(|_| service_unavailable("workspace"))?
            .organization_snapshot(&principal, &grants, &organization_id, &workspace_id)
            .map_err(response_problem)?,
    ))
}

async fn check_usage(
    State(state): State<ApiState>,
    Path((organization_id, workspace_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(input): Json<UsageInput>,
) -> Result<Json<serde_json::Value>, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let organization_id =
        OrganizationId::from_str(&organization_id).map_err(|_| bad_id("organization_id"))?;
    let workspace_id = WorkspaceId::from_str(&workspace_id).map_err(|_| bad_id("workspace_id"))?;
    state
        .workspaces
        .read()
        .map_err(|_| service_unavailable("workspace"))?
        .check_usage(
            &principal,
            &grants,
            &organization_id,
            &workspace_id,
            WorkspaceUsage {
                profiles: input.profiles,
                members: input.members,
                storage_bytes: input.storage_bytes,
            },
        )
        .map_err(response_problem)?;
    Ok(Json(serde_json::json!({"ok": true})))
}

async fn offline_entitlement(
    State(state): State<ApiState>,
    Path((organization_id, workspace_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<OfflineEntitlement>, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let organization_id =
        OrganizationId::from_str(&organization_id).map_err(|_| bad_id("organization_id"))?;
    let workspace_id = WorkspaceId::from_str(&workspace_id).map_err(|_| bad_id("workspace_id"))?;
    Ok(Json(
        state
            .workspaces
            .read()
            .map_err(|_| service_unavailable("workspace"))?
            .offline_entitlement(&principal, &grants, &organization_id, &workspace_id)
            .map_err(response_problem)?,
    ))
}

async fn invite_member(
    State(state): State<ApiState>,
    Path(workspace_id): Path<String>,
    headers: HeaderMap,
    Json(member): Json<Member>,
) -> Result<Response, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let workspace_id = WorkspaceId::from_str(&workspace_id).map_err(|_| bad_id("workspace_id"))?;
    if member.workspace_id != workspace_id {
        return Err(problem_response(
            "tenant_mismatch",
            "Member belongs to another workspace",
            403,
            correlation(&headers),
        ));
    }
    let value = serde_json::to_value(&member).map_err(internal)?;
    mutate(
        &state,
        &headers,
        &principal,
        &grants,
        workspace_id.as_str(),
        "member.invite",
        value,
        ResourceScope::workspace(workspace_id.clone()),
        member.id.as_str(),
        || {
            state
                .members
                .write()
                .map_err(|_| service_unavailable("member"))?
                .invite(
                    &principal,
                    &grants,
                    member.clone(),
                    checked_now("member-invite").map_err(response_problem)?,
                )
                .map_err(response_problem)
                .map(|_| member.clone())
        },
    )
    .await
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateMemberInvitationInput {
    email: String,
    role: MemberRole,
    expires_at: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RedeemMemberInvitationInput {
    code: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CreatedMemberInvitation {
    invitation_id: MemberInvitationId,
    code: String,
    expires_at: u64,
}

async fn create_member_invitation(
    State(state): State<ApiState>,
    Path(workspace_id): Path<String>,
    headers: HeaderMap,
    Json(input): Json<CreateMemberInvitationInput>,
) -> Result<Response, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let workspace_id = WorkspaceId::from_str(&workspace_id).map_err(|_| bad_id("workspace_id"))?;
    let now = checked_now("member-invitation-create").map_err(response_problem)?;
    let inviter = principal.account_id.clone().ok_or_else(|| {
        problem_response(
            "authenticated_account_required",
            "An authenticated account is required",
            401,
            correlation(&headers),
        )
    })?;
    let mut entropy = [0_u8; 48];
    getrandom::fill(&mut entropy).map_err(|_| {
        problem_response(
            "invitation_entropy_unavailable",
            "Invitation code could not be generated",
            503,
            correlation(&headers),
        )
    })?;
    let invitation_id = MemberInvitationId::new(format!("inv_{}", hex_bytes(&entropy[..16])))
        .map_err(|_| {
            problem_response(
                "invitation_id_invalid",
                "Invitation could not be created",
                500,
                correlation(&headers),
            )
        })?;
    let code = Zeroizing::new(hex_bytes(&entropy[16..]));
    entropy.zeroize();
    let token_hash = hex_bytes(&Sha256::digest(code.as_bytes()));
    let canonical_email = canonical_email(&input.email).map_err(response_problem)?;
    let invitation = MemberInvitation::issue(
        invitation_id.clone(),
        workspace_id.clone(),
        canonical_email,
        input.role,
        input.expires_at,
        inviter,
        token_hash,
        now,
    )
    .map_err(response_problem)?;
    let _authority = scoped_account_creation_authority(
        &state,
        &principal,
        &grants,
        &workspace_id,
        &invitation.role,
    )?;
    let input_value = serde_json::json!({
        "emailHash": hex_bytes(&Sha256::digest(invitation.canonical_email.as_bytes())),
        "role": invitation.role,
        "expiresAt": invitation.expires_at,
    });
    let store_value = serde_json::to_value(&invitation).map_err(internal)?;
    let put = ControlPut {
        tenant_id: super::CONTROL_TENANT.into(),
        record_type: "member_invitation".into(),
        record_id: invitation.id.as_str().into(),
        expected_revision: Some(0),
        value: store_value,
    };
    let output = CreatedMemberInvitation {
        invitation_id: invitation.id.clone(),
        code: code.to_string(),
        expires_at: invitation.expires_at,
    };
    let replay_body = serde_json::json!({
        "invitationId": invitation.id.as_str(),
        "code": null,
        "expiresAt": invitation.expires_at,
    });
    mutate_with_additional_puts(
        &state,
        &headers,
        &principal,
        &grants,
        workspace_id.as_str(),
        "member.invitation.create",
        input_value,
        ResourceScope::workspace(workspace_id.clone()),
        invitation.id.as_str(),
        || Ok((output, vec![put], Some(replay_body))),
    )
    .await
}

async fn redeem_member_invitation(
    State(state): State<ApiState>,
    Path(invitation_id): Path<String>,
    headers: HeaderMap,
    Json(input): Json<RedeemMemberInvitationInput>,
) -> Result<Response, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let invitation_id =
        MemberInvitationId::from_str(&invitation_id).map_err(|_| bad_id("invitation_id"))?;
    let account_id = principal.account_id.clone().ok_or_else(|| {
        problem_response(
            "authenticated_account_required",
            "An authenticated account is required",
            401,
            correlation(&headers),
        )
    })?;
    let now = checked_now("member-invitation-redeem").map_err(response_problem)?;
    let invitation_workspace = state
        .control_persistence
        .load::<MemberInvitation>(
            super::CONTROL_TENANT,
            "member_invitation",
            invitation_id.as_str(),
        )
        .map_err(response_problem)?
        .map(|invitation| invitation.workspace_id)
        .ok_or_else(|| {
            problem_response(
                "invitation_unavailable",
                "Invitation is unavailable",
                410,
                correlation(&headers),
            )
        })?;
    let input_value = serde_json::json!({
        "invitationId": invitation_id.as_str(),
        "codeHash": hex_bytes(&Sha256::digest(input.code.as_bytes())),
    });
    mutate_with_additional_puts(
        &state,
        &headers,
        &principal,
        &grants,
        invitation_workspace.as_str(),
        "member.invitation.redeem",
        input_value,
        ResourceScope::workspace(invitation_workspace.clone()),
        invitation_id.as_str(),
        || {
            let (invitation, store_revision) = state
                .control_persistence
                .load_versioned::<MemberInvitation>(
                    super::CONTROL_TENANT,
                    "member_invitation",
                    invitation_id.as_str(),
                )
                .map_err(response_problem)?
                .ok_or_else(|| {
                    response_problem(Problem::new(
                        "invitation_unavailable",
                        "Invitation is unavailable",
                        410,
                        "member_invitation",
                    ))
                })?;
            if invitation.id != invitation_id
                || invitation.state != MemberInvitationState::Issued
                || now >= invitation.expires_at
                || !invitation.verify_code(&input.code)
            {
                return Err(response_problem(Problem::new(
                    "invitation_unavailable",
                    "Invitation is unavailable",
                    410,
                    "member_invitation",
                )));
            }
            let account = state.account(&account_id).map_err(response_problem)?;
            let identity = state
                .identity_persistence
                .load(&account_id)
                .map_err(response_problem)?;
            if account.is_none_or(|account| account.status != AccountStatus::Active)
                || identity.as_ref().is_none_or(|identity| {
                    identity.verification != IdentityVerification::Verified
                        || identity.canonical_email != invitation.canonical_email
                })
            {
                return Err(response_problem(Problem::new(
                    "invitation_unavailable",
                    "Invitation is unavailable",
                    410,
                    "member_invitation",
                )));
            }
            let member_id = random_member_id().map_err(response_problem)?;
            let member = state
                .members
                .write()
                .map_err(|_| service_unavailable("member"))?
                .redeem_invitation(
                    invitation.workspace_id.clone(),
                    account_id.clone(),
                    member_id.clone(),
                    invitation.role.clone(),
                    invitation.expires_at,
                    now,
                )
                .map_err(response_problem)?;
            let redeemed = invitation
                .redeem(store_revision, account_id.clone(), member_id, now)
                .map_err(response_problem)?;
            let put = ControlPut {
                tenant_id: super::CONTROL_TENANT.into(),
                record_type: "member_invitation".into(),
                record_id: invitation.id.as_str().into(),
                expected_revision: Some(store_revision),
                value: serde_json::to_value(redeemed).map_err(|_| {
                    response_problem(problem("serialization_failed", "Serialization failed", 500))
                })?,
            };
            Ok((member, vec![put], None))
        },
    )
    .await
}

async fn revoke_member_invitation(
    State(state): State<ApiState>,
    Path((workspace_id, invitation_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let workspace_id = WorkspaceId::from_str(&workspace_id).map_err(|_| bad_id("workspace_id"))?;
    let invitation_id =
        MemberInvitationId::from_str(&invitation_id).map_err(|_| bad_id("invitation_id"))?;
    let input = serde_json::json!({"workspaceId": workspace_id, "invitationId": invitation_id});
    mutate_with_additional_puts(
        &state,
        &headers,
        &principal,
        &grants,
        workspace_id.as_str(),
        "member.invitation.revoke",
        input,
        ResourceScope::workspace(workspace_id.clone()),
        invitation_id.as_str(),
        || {
            let (invitation, store_revision) = state
                .control_persistence
                .load_versioned::<MemberInvitation>(
                    super::CONTROL_TENANT,
                    "member_invitation",
                    invitation_id.as_str(),
                )
                .map_err(response_problem)?
                .ok_or_else(|| {
                    response_problem(Problem::new(
                        "invitation_not_found",
                        "Invitation not found",
                        404,
                        "member_invitation",
                    ))
                })?;
            if invitation.workspace_id != workspace_id || invitation.id != invitation_id {
                return Err(response_problem(Problem::new(
                    "invitation_not_found",
                    "Invitation not found",
                    404,
                    "member_invitation",
                )));
            }
            state
                .members
                .read()
                .map_err(|_| service_unavailable("member"))?
                .authorize_invitation_revocation(
                    &principal,
                    &grants,
                    &workspace_id,
                    &invitation.role,
                )
                .map_err(response_problem)?;
            let revoked = invitation
                .revoke(store_revision)
                .map_err(response_problem)?;
            let put = ControlPut {
                tenant_id: super::CONTROL_TENANT.into(),
                record_type: "member_invitation".into(),
                record_id: invitation.id.as_str().into(),
                expected_revision: Some(store_revision),
                value: serde_json::to_value(revoked).map_err(|_| {
                    response_problem(problem("serialization_failed", "Serialization failed", 500))
                })?,
            };
            Ok((serde_json::json!({"revoked": true}), vec![put], None))
        },
    )
    .await
}

fn random_member_id() -> Result<MemberId, Problem> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| {
        Problem::new(
            "member_id_entropy_unavailable",
            "Membership could not be created",
            503,
            "member",
        )
    })?;
    MemberId::new(format!("mem_{}", hex_bytes(&bytes))).map_err(|_| {
        Problem::new(
            "member_id_invalid",
            "Membership could not be created",
            500,
            "member",
        )
    })
}

fn hex_bytes(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(HEX[(byte >> 4) as usize]));
        output.push(char::from(HEX[(byte & 0x0f) as usize]));
    }
    output
}

async fn list_members(
    State(state): State<ApiState>,
    Path(workspace_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Vec<MemberDirectoryView>>, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let workspace_id = WorkspaceId::from_str(&workspace_id).map_err(|_| bad_id("workspace_id"))?;
    let members = {
        let member_service = state
            .members
            .read()
            .map_err(|_| service_unavailable("member"))?;
        match member_service.list(&principal, &grants, &workspace_id) {
            Ok(members) => members,
            Err(error) if error.status == 403 => {
                let organization = state
                    .workspaces
                    .read()
                    .map_err(|_| service_unavailable("workspace"))?
                    .managed_organization_for_workspace(&principal, &grants, &workspace_id)
                    .map_err(response_problem)?;
                member_service
                    .list_as_org_owner(&principal, &grants, &organization, &workspace_id)
                    .map_err(response_problem)?
            }
            Err(error) => return Err(response(error)),
        }
    };
    let items = member_directory_views(state, principal, grants, members).await?;
    Ok(Json(items))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MemberIdentityView {
    username: String,
    email: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MemberDirectoryView {
    #[serde(flatten)]
    member: Member,
    #[serde(skip_serializing_if = "Option::is_none")]
    identity: Option<MemberIdentityView>,
}

async fn member_directory_views(
    state: ApiState,
    principal: Principal,
    grants: Vec<buglogin_domain::permissions::Grant>,
    members: Vec<Member>,
) -> Result<Vec<MemberDirectoryView>, Response> {
    tokio::task::spawn_blocking(move || {
        let identity_access = members
            .iter()
            .map(|member| can_read_member_identity(&state, &principal, &grants, member))
            .collect::<Result<Vec<_>, _>>()?;
        let mut account_ids = BTreeSet::new();
        for (member, can_read_identity) in members.iter().zip(&identity_access) {
            if *can_read_identity {
                account_ids.insert(member.account_id.as_str().to_owned());
            }
        }
        let account_ids = account_ids.into_iter().collect::<Vec<_>>();
        let identities = crate::services::identity_persistence::IdentityPersistence::new(
            state.control_persistence.clone(),
        )
        .load_many(&account_ids)?;
        members
            .into_iter()
            .zip(identity_access)
            .map(|(member, can_read_identity)| {
                let identity = if can_read_identity {
                    identities
                        .get(member.account_id.as_str())
                        .map(|identity| MemberIdentityView {
                            username: identity.username.clone(),
                            email: identity.email.clone(),
                        })
                } else {
                    None
                };
                Ok::<MemberDirectoryView, buglogin_domain::errors::Problem>(MemberDirectoryView {
                    member,
                    identity,
                })
            })
            .collect::<Result<Vec<_>, buglogin_domain::errors::Problem>>()
    })
    .await
    .map_err(|_| Response::from(service_unavailable("identity")))?
    .map_err(response_problem)
    .map_err(Response::from)
}

fn can_read_member_identity(
    state: &ApiState,
    principal: &Principal,
    grants: &[buglogin_domain::permissions::Grant],
    member: &Member,
) -> Result<bool, buglogin_domain::errors::Problem> {
    let direct_identity_access = is_allowed(
        principal,
        &Capability::new("CAP-MEM-001").map_err(|_| {
            buglogin_domain::errors::Problem::new(
                "invalid_capability",
                "Membership capability is invalid",
                500,
                "member-directory",
            )
        })?,
        Action::Manage,
        &ResourceScope::workspace(member.workspace_id.clone()),
        grants,
    );
    let organization_owner_access = state
        .workspaces
        .read()
        .map_err(|_| {
            buglogin_domain::errors::Problem::new(
                "workspace_unavailable",
                "Workspace service unavailable",
                503,
                "workspace",
            )
        })?
        .managed_organization_for_workspace(principal, grants, &member.workspace_id)
        .ok()
        .is_some_and(|organization| {
            MemberService::authorize_org_owner_access(
                principal,
                grants,
                &organization,
                &member.workspace_id,
            )
            .is_ok()
        });
    let can_read_identity = direct_identity_access || organization_owner_access;
    Ok(can_read_identity)
}

async fn list_members_page(
    State(state): State<ApiState>,
    Path(workspace_id): Path<String>,
    Query(query): Query<CollectionPageQuery>,
    headers: HeaderMap,
) -> Result<Json<CollectionPageView<MemberDirectoryView>>, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let workspace_id = WorkspaceId::from_str(&workspace_id).map_err(|_| bad_id("workspace_id"))?;
    let cursor = query
        .cursor
        .as_deref()
        .map(MemberId::from_str)
        .transpose()
        .map_err(|_| bad_id("cursor"))?;
    let limit = collection_page_limit(query.limit)?;
    let (members, next_cursor) = {
        let member_service = state
            .members
            .read()
            .map_err(|_| service_unavailable("member"))?;
        match member_service.list_page(&principal, &grants, &workspace_id, cursor.as_ref(), limit) {
            Ok(page) => page,
            Err(error) if error.status == 403 => {
                let organization = state
                    .workspaces
                    .read()
                    .map_err(|_| service_unavailable("workspace"))?
                    .managed_organization_for_workspace(&principal, &grants, &workspace_id)
                    .map_err(response_problem)?;
                member_service
                    .list_page_as_org_owner(
                        &principal,
                        &grants,
                        &organization,
                        &workspace_id,
                        cursor.as_ref(),
                        limit,
                    )
                    .map_err(response_problem)?
            }
            Err(error) => return Err(response(error)),
        }
    };
    let items = member_directory_views(state, principal, grants, members).await?;
    Ok(Json(CollectionPageView {
        items,
        next_cursor: next_cursor.map(|id| id.as_str().to_owned()),
    }))
}

async fn get_member(
    State(state): State<ApiState>,
    Path((workspace_id, member_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<Member>, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let workspace_id = WorkspaceId::from_str(&workspace_id).map_err(|_| bad_id("workspace_id"))?;
    let member_id = MemberId::from_str(&member_id).map_err(|_| bad_id("member_id"))?;
    Ok(Json(
        state
            .members
            .read()
            .map_err(|_| service_unavailable("member"))?
            .get_in_workspace(&principal, &grants, &member_id, &workspace_id)
            .map_err(response_problem)?,
    ))
}

async fn activate_member(
    State(state): State<ApiState>,
    Path((workspace_id, member_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let workspace_id = WorkspaceId::from_str(&workspace_id).map_err(|_| bad_id("workspace_id"))?;
    let member_id = MemberId::from_str(&member_id).map_err(|_| bad_id("member_id"))?;
    let (member_grants, organization_owner) = workspace_member_management_grants(
        &state,
        &principal,
        &grants,
        &workspace_id,
        Action::Manage,
        correlation(&headers),
    )?;
    let target = state
        .members
        .read()
        .map_err(|_| service_unavailable("member"))?
        .get_in_workspace(&principal, &member_grants, &member_id, &workspace_id)
        .map_err(response_problem)?;
    if organization_owner && target.role == MemberRole::Owner {
        return Err(problem_response(
            "role_escalation",
            "Organization owner cannot change a workspace Owner membership",
            403,
            correlation(&headers),
        ));
    }
    let value = serde_json::json!({"workspaceId": workspace_id, "memberId": member_id});
    mutate(
        &state,
        &headers,
        &principal,
        &grants,
        workspace_id.as_str(),
        "member.activate",
        value,
        ResourceScope::workspace(workspace_id.clone()),
        member_id.as_str(),
        || {
            let current_target = state
                .members
                .read()
                .map_err(|_| service_unavailable("member"))?
                .get_in_workspace(&principal, &member_grants, &member_id, &workspace_id)
                .map_err(response_problem)?;
            if current_target.status == MemberStatus::Suspended {
                let workspace = state
                    .workspaces
                    .read()
                    .map_err(|_| service_unavailable("workspace"))?
                    .workspace_for_policy(&workspace_id)
                    .map_err(response_problem)?;
                let parent_status = state
                    .organization_members
                    .read()
                    .map_err(|_| service_unavailable("organization_member"))?
                    .get(&workspace.organization_id, &current_target.account_id)
                    .map(|parent| parent.status.clone());
                if parent_status == Some(OrganizationMemberStatus::Suspended) {
                    return Err(response_problem(Problem::new(
                        "organization_member_inactive",
                        "Suspended organization members cannot be restored in a workspace",
                        409,
                        correlation(&headers),
                    )));
                }
            }
            HostedProfileService::activate_member(
                &state,
                &principal,
                &member_grants,
                &workspace_id,
                &member_id,
            )
            .map_err(response_problem)
        },
    )
    .await
}

async fn suspend_member(
    State(state): State<ApiState>,
    Path((workspace_id, member_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let workspace_id = WorkspaceId::from_str(&workspace_id).map_err(|_| bad_id("workspace_id"))?;
    let member_id = MemberId::from_str(&member_id).map_err(|_| bad_id("member_id"))?;
    let (member_grants, organization_owner) = workspace_member_management_grants(
        &state,
        &principal,
        &grants,
        &workspace_id,
        Action::Manage,
        correlation(&headers),
    )?;
    let target = state
        .members
        .read()
        .map_err(|_| service_unavailable("member"))?
        .get_in_workspace(&principal, &member_grants, &member_id, &workspace_id)
        .map_err(response_problem)?;
    if organization_owner && target.role == MemberRole::Owner {
        return Err(problem_response(
            "role_escalation",
            "Organization owner cannot change a workspace Owner membership",
            403,
            correlation(&headers),
        ));
    }
    let value = serde_json::json!({"workspaceId": workspace_id, "memberId": member_id});
    mutate(
        &state,
        &headers,
        &principal,
        &grants,
        workspace_id.as_str(),
        "member.suspend",
        value,
        ResourceScope::workspace(workspace_id.clone()),
        member_id.as_str(),
        || {
            HostedProfileService::suspend_member(
                &state,
                &principal,
                &member_grants,
                &workspace_id,
                &member_id,
            )
            .map_err(response_problem)
        },
    )
    .await
}

async fn set_member_role(
    State(state): State<ApiState>,
    Path((workspace_id, member_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(input): Json<RoleInput>,
) -> Result<Response, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let workspace_id = WorkspaceId::from_str(&workspace_id).map_err(|_| bad_id("workspace_id"))?;
    let member_id = MemberId::from_str(&member_id).map_err(|_| bad_id("member_id"))?;
    let (member_grants, organization_owner) = workspace_member_management_grants(
        &state,
        &principal,
        &grants,
        &workspace_id,
        Action::Manage,
        correlation(&headers),
    )?;
    let target = state
        .members
        .read()
        .map_err(|_| service_unavailable("member"))?
        .get_in_workspace(&principal, &member_grants, &member_id, &workspace_id)
        .map_err(response_problem)?;
    if organization_owner && (target.role == MemberRole::Owner || input.role == MemberRole::Owner) {
        return Err(problem_response(
            "role_escalation",
            "Organization owner may manage only memberships below Owner",
            403,
            correlation(&headers),
        ));
    }
    let value = serde_json::to_value(&input).map_err(internal)?;
    mutate(
        &state,
        &headers,
        &principal,
        &grants,
        workspace_id.as_str(),
        "member.role.update",
        value,
        ResourceScope::workspace(workspace_id.clone()),
        member_id.as_str(),
        || {
            {
                let mut members = state
                    .members
                    .write()
                    .map_err(|_| service_unavailable("member"))?;
                members
                    .set_role(
                        &principal,
                        &member_grants,
                        &workspace_id,
                        &member_id,
                        input.role.clone(),
                    )
                    .map_err(response_problem)?;
            }
            state
                .members
                .read()
                .map_err(|_| service_unavailable("member"))?
                .get_in_workspace(&principal, &member_grants, &member_id, &workspace_id)
                .map_err(response_problem)
        },
    )
    .await
}

async fn register_resource(
    State(state): State<ApiState>,
    Path(workspace_id): Path<String>,
    headers: HeaderMap,
    Json(resource): Json<Resource>,
) -> Result<Response, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let workspace_id = WorkspaceId::from_str(&workspace_id).map_err(|_| bad_id("workspace_id"))?;
    if resource.workspace_id != workspace_id {
        return Err(problem_response(
            "tenant_mismatch",
            "Resource belongs to another workspace",
            403,
            correlation(&headers),
        ));
    }
    let value = serde_json::to_value(&resource).map_err(internal)?;
    mutate(
        &state,
        &headers,
        &principal,
        &grants,
        workspace_id.as_str(),
        "resource.register",
        value,
        ResourceScope::workspace(workspace_id.clone()),
        resource.id.as_str(),
        || {
            state
                .resources
                .write()
                .map_err(|_| service_unavailable("resource"))?
                .register(&principal, &grants, resource.clone())
                .map_err(response_problem)
                .map(|_| resource.clone())
        },
    )
    .await
}

async fn list_resources(
    State(state): State<ApiState>,
    Path(workspace_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Vec<Resource>>, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let workspace_id = WorkspaceId::from_str(&workspace_id).map_err(|_| bad_id("workspace_id"))?;
    let items = state
        .resources
        .read()
        .map_err(|_| service_unavailable("resource"))?
        .list(&principal, &grants, &workspace_id)
        .map_err(response_problem)?;
    Ok(Json(items))
}

async fn list_resources_page(
    State(state): State<ApiState>,
    Path(workspace_id): Path<String>,
    Query(query): Query<ResourcePageQuery>,
    headers: HeaderMap,
) -> Result<Json<CollectionPageView<Resource>>, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let workspace_id = WorkspaceId::from_str(&workspace_id).map_err(|_| bad_id("workspace_id"))?;
    let cursor = query
        .cursor
        .as_deref()
        .map(ResourceId::from_str)
        .transpose()
        .map_err(|_| bad_id("cursor"))?;
    let limit = collection_page_limit(query.limit)?;
    let search = query
        .search
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if search.is_some_and(|value| value.chars().count() > 128) {
        return Err(problem_response(
            "invalid_resource_search",
            "Resource search is limited to 128 characters",
            400,
            correlation(&headers),
        ));
    }
    let (items, next_cursor) = state
        .resources
        .read()
        .map_err(|_| service_unavailable("resource"))?
        .list_page_filtered(
            &principal,
            &grants,
            &workspace_id,
            cursor.as_ref(),
            limit,
            search,
            query.kind.as_ref(),
            query.state.as_ref(),
        )
        .map_err(response_problem)?;
    Ok(Json(CollectionPageView {
        items,
        next_cursor: next_cursor.map(|id| id.as_str().to_owned()),
    }))
}

async fn get_resource(
    State(state): State<ApiState>,
    Path((workspace_id, resource_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<Resource>, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let workspace_id = WorkspaceId::from_str(&workspace_id).map_err(|_| bad_id("workspace_id"))?;
    let resource_id = ResourceId::from_str(&resource_id).map_err(|_| bad_id("resource_id"))?;
    Ok(Json(
        state
            .resources
            .read()
            .map_err(|_| service_unavailable("resource"))?
            .get(&principal, &grants, &resource_id, &workspace_id)
            .map_err(response_problem)?,
    ))
}

async fn get_runtime_projection(
    State(state): State<ApiState>,
    Path(profile_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Option<RuntimeProjectionView>>, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let profile_id = ProfileId::from_str(&profile_id).map_err(|_| bad_id("profile_id"))?;
    authorize_action(
        &state,
        &principal,
        &grants,
        &profile_id,
        Action::Read,
        "profile.read",
    )?;
    active_profile_binding(&state, &profile_id)?;
    let projection = state
        .runtime_projection(&profile_id)
        .map_err(response_problem)?;
    let projection = match projection {
        Some(projection) => {
            let account_id = AccountId::from_str(&projection.owner_id).map_err(|_| {
                problem_response(
                    "runtime_owner_invalid",
                    "Runtime owner identity is invalid",
                    500,
                    correlation(&headers),
                )
            })?;
            let owner_email = state
                .identity_persistence
                .load(&account_id)
                .map_err(response_problem)?
                .map(|identity| identity.email);
            Some(RuntimeProjectionView {
                projection,
                owner_email,
            })
        }
        None => None,
    };
    Ok(Json(projection))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProfileRuntimeStatusBatchQuery {
    #[serde(default)]
    profile_id: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProfileRuntimeStatusSnapshot {
    profile_id: ProfileId,
    lease: Option<buglogin_sync::coordination::LeaseStatus>,
    runtime: Option<RuntimeProjectionView>,
}

async fn get_profile_runtime_status_batch(
    State(state): State<ApiState>,
    Path(workspace_path): Path<String>,
    Query(query): Query<ProfileRuntimeStatusBatchQuery>,
    headers: HeaderMap,
) -> Result<Json<Vec<ProfileRuntimeStatusSnapshot>>, Response> {
    const MAX_PROFILE_STATUS_BATCH: usize = 250;
    if query.profile_id.is_empty() || query.profile_id.len() > MAX_PROFILE_STATUS_BATCH {
        return Err(problem_response(
            if query.profile_id.is_empty() {
                "profile_status_batch_empty"
            } else {
                "profile_status_batch_too_large"
            },
            "A status request must include between 1 and 250 profiles",
            if query.profile_id.is_empty() {
                400
            } else {
                413
            },
            correlation(&headers),
        ));
    }
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    state
        .refresh_control_aggregate_async()
        .await
        .map_err(response_problem)?;
    let workspace_id =
        WorkspaceId::from_str(&workspace_path).map_err(|_| bad_id("workspace_id"))?;
    let actor_id = principal.account_id.as_ref().ok_or_else(|| {
        problem_response(
            "account_missing",
            "Authenticated account is missing",
            401,
            correlation(&headers),
        )
    })?;
    let auth = CoordinationAuth::new(actor_id.as_str(), bearer_token(&headers).to_owned())
        .map_err(response_problem)?;
    let coordination = state.coordination().map_err(response_problem)?;
    let state_for_authorization = state.clone();
    let principal_for_authorization = principal.clone();
    let grants_for_authorization = grants.clone();
    let workspace_for_authorization = workspace_id.clone();
    let correlation_for_authorization = headers.clone();
    let authorized = tokio::task::spawn_blocking(move || {
        let mut authorized = Vec::with_capacity(query.profile_id.len());
        let mut seen = BTreeSet::new();
        for raw_profile_id in query.profile_id {
            let profile_id =
                ProfileId::from_str(&raw_profile_id).map_err(|_| bad_id("profile_id"))?;
            if !seen.insert(profile_id.clone()) {
                continue;
            }
            // Do not disclose whether an inaccessible profile is bound to this workspace.
            if let Err(response) = authorize_action(
                &state_for_authorization,
                &principal_for_authorization,
                &grants_for_authorization,
                &profile_id,
                Action::Read,
                "profile.read",
            ) {
                if matches!(
                    response.status(),
                    StatusCode::FORBIDDEN | StatusCode::NOT_FOUND
                ) {
                    continue;
                }
                return Err(response);
            }
            let binding = match active_profile_binding(&state_for_authorization, &profile_id) {
                Ok(binding) if binding.workspace_id == workspace_for_authorization => binding,
                Ok(_) => continue,
                Err(response)
                    if matches!(
                        response.status(),
                        StatusCode::NOT_FOUND | StatusCode::CONFLICT
                    ) =>
                {
                    continue;
                }
                Err(response) => return Err(response),
            };
            authorize_lease(
                &state_for_authorization,
                &principal_for_authorization,
                &grants_for_authorization,
                &workspace_for_authorization,
                &binding.resource_id,
                "profile.read",
                Action::Read,
            )?;
            authorized.push((profile_id, binding.resource_id));
        }
        Ok::<_, Response>(authorized)
    })
    .await
    .map_err(|_| {
        problem_response(
            "profile_status_unavailable",
            "Profile status is temporarily unavailable",
            503,
            correlation(&correlation_for_authorization),
        )
    })??;
    let mut lease_results =
        futures_util::stream::iter(authorized.into_iter().map(|(profile_id, resource_id)| {
            let coordination = &coordination;
            let workspace_id = &workspace_id;
            let auth = &auth;
            async move {
                coordination
                    .lease_status(CoordinationScope { workspace_id, auth }, &resource_id)
                    .await
                    .map(|lease| (profile_id, lease))
                    .map_err(response_problem)
            }
        }))
        .buffer_unordered(8)
        .try_collect::<Vec<_>>()
        .await?;
    lease_results.sort_by(|left, right| left.0.cmp(&right.0));
    let state_for_projection = state.clone();
    let headers_for_projection = headers.clone();
    let snapshots = tokio::task::spawn_blocking(move || {
        let mut snapshots = Vec::with_capacity(lease_results.len());
        for (profile_id, lease) in lease_results {
            let runtime = state_for_projection
                .runtime_projection(&profile_id)
                .map_err(response_problem)?
                .map(|projection| {
                    let owner_id = AccountId::from_str(&projection.owner_id).map_err(|_| {
                        problem_response(
                            "runtime_owner_invalid",
                            "Runtime owner identity is invalid",
                            500,
                            correlation(&headers_for_projection),
                        )
                    })?;
                    let owner_email = state_for_projection
                        .identity_persistence
                        .load(&owner_id)
                        .map_err(response_problem)?
                        .map(|identity| identity.email);
                    Ok::<_, Response>(RuntimeProjectionView {
                        projection,
                        owner_email,
                    })
                })
                .transpose()?;
            snapshots.push(ProfileRuntimeStatusSnapshot {
                profile_id,
                lease,
                runtime,
            });
        }
        Ok::<_, Response>(snapshots)
    })
    .await
    .map_err(|_| {
        problem_response(
            "profile_status_unavailable",
            "Profile status is temporarily unavailable",
            503,
            correlation(&headers),
        )
    })??;
    Ok(Json(snapshots))
}

async fn runtime_started(
    State(state): State<ApiState>,
    Path(profile_id): Path<String>,
    headers: HeaderMap,
    Json(input): Json<RuntimeProjectionInput>,
) -> Result<Response, Response> {
    set_runtime_projection(
        state,
        profile_id,
        headers,
        input,
        "running",
        "profile.launch",
        "profile.runtime.started",
    )
    .await
}

async fn runtime_stopped(
    State(state): State<ApiState>,
    Path(profile_id): Path<String>,
    headers: HeaderMap,
    Json(input): Json<RuntimeProjectionInput>,
) -> Result<Response, Response> {
    set_runtime_projection(
        state,
        profile_id,
        headers,
        input,
        "stopped",
        "profile.stop",
        "profile.runtime.stopped",
    )
    .await
}

async fn runtime_syncing(
    State(state): State<ApiState>,
    Path(profile_id): Path<String>,
    headers: HeaderMap,
    Json(input): Json<RuntimeProjectionInput>,
) -> Result<Response, Response> {
    set_runtime_projection(
        state,
        profile_id,
        headers,
        input,
        "syncing",
        "profile.stop",
        "profile.runtime.syncing",
    )
    .await
}

async fn set_runtime_projection(
    state: ApiState,
    profile_id: String,
    headers: HeaderMap,
    input: RuntimeProjectionInput,
    runtime_state: &'static str,
    capability: &'static str,
    command: &'static str,
) -> Result<Response, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let profile_id = ProfileId::from_str(&profile_id).map_err(|_| bad_id("profile_id"))?;
    authorize_action(
        &state,
        &principal,
        &grants,
        &profile_id,
        Action::Execute,
        capability,
    )?;
    validate_runtime_projection_input(&input, &headers)?;
    let binding = active_profile_binding(&state, &profile_id)?;
    authorize_lease(
        &state,
        &principal,
        &grants,
        &binding.workspace_id,
        &binding.resource_id,
        capability,
        Action::Execute,
    )?;
    let _ = require_idempotency(&headers)?;
    let owner_id = required_header(&headers, "x-buglogin-owner-id")?;
    let owner_token = required_header(&headers, "x-buglogin-owner-token")?;
    let fencing_token = required_header(&headers, "x-buglogin-fencing-token")?
        .parse::<u64>()
        .map_err(|_| bad_header("x-buglogin-fencing-token"))?;
    let auth = CoordinationAuth::new(owner_id, bearer_token(&headers).to_owned())
        .map_err(response_problem)?;
    state
        .coordination()
        .map_err(response_problem)?
        .validate_write_authority(
            CoordinationScope {
                workspace_id: &binding.workspace_id,
                auth: &auth,
            },
            &binding.resource_id,
            owner_token,
            fencing_token,
        )
        .await
        .map_err(response_problem)?;
    let projection = RuntimeProjection {
        profile_id: profile_id.clone(),
        device_id: input.device_id.clone(),
        owner_id: owner_id.to_owned(),
        state: runtime_state.to_owned(),
        generation: input.generation,
        root_checksum: input.root_checksum.clone(),
        updated_at_ms: request_now_ms(&headers)?,
    };
    let mutation_input = json!({
        "profileId": profile_id,
        "deviceId": input.device_id,
        "ownerId": owner_id,
        "state": runtime_state,
        "generation": input.generation,
        "rootChecksum": input.root_checksum,
        "fencingToken": fencing_token,
    });
    mutate(
        &state,
        &headers,
        &principal,
        &grants,
        binding.workspace_id.as_str(),
        command,
        mutation_input,
        ResourceScope::resource(binding.workspace_id.clone(), binding.resource_id.clone()),
        binding.resource_id.as_str(),
        || {
            state
                .set_runtime_projection(projection.clone())
                .map_err(response_problem)?;
            Ok(projection)
        },
    )
    .await
}

fn active_profile_binding(
    state: &ApiState,
    profile_id: &ProfileId,
) -> Result<ProfileResource, Response> {
    if !state.profile_exists(profile_id).map_err(response)? {
        return Err(problem_response(
            "profile_not_found",
            "Profile not found",
            404,
            "api".to_owned(),
        ));
    }
    let binding = state
        .profile_resources
        .read()
        .map_err(|_| service_unavailable("profile binding"))?
        .get(profile_id)
        .cloned()
        .ok_or_else(|| {
            problem_response(
                "binding_not_found",
                "Profile binding not found",
                404,
                "api".to_owned(),
            )
        })?;
    if binding.profile_id != *profile_id || binding.archive_state != ProfileArchiveState::Active {
        return Err(problem_response(
            "managed_profile_binding_required",
            "Managed profile requires an active binding",
            409,
            "api".to_owned(),
        ));
    }
    Ok(binding)
}

fn validate_runtime_projection_input(
    input: &RuntimeProjectionInput,
    headers: &HeaderMap,
) -> Result<(), Response> {
    if input.device_id.trim().is_empty() || input.device_id.len() > 128 {
        return Err(problem_response(
            "runtime_device_invalid",
            "Runtime device ID is invalid",
            400,
            correlation(headers),
        ));
    }
    if input.generation == 0
        || input.root_checksum.len() != 64
        || !input
            .root_checksum
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(problem_response(
            "runtime_projection_invalid",
            "Runtime projection is invalid",
            400,
            correlation(headers),
        ));
    }
    Ok(())
}

async fn acquire_resource_lease(
    State(state): State<ApiState>,
    Path((workspace_id, resource_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(input): Json<LeaseAcquireInput>,
) -> Result<Json<buglogin_sync::coordination::LeaseGrant>, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let workspace_id = WorkspaceId::from_str(&workspace_id).map_err(|_| bad_id("workspace_id"))?;
    let resource_id = ResourceId::from_str(&resource_id).map_err(|_| bad_id("resource_id"))?;
    let rotation_pending = key_rotation_blocks_resource(&state, &workspace_id, &resource_id)?;
    let (capability, action) = resource_lease_authority(rotation_pending);
    authorize_lease(
        &state,
        &principal,
        &grants,
        &workspace_id,
        &resource_id,
        capability,
        action,
    )?;
    let idempotency_key = require_idempotency(&headers)?;
    let registered = state
        .resources
        .read()
        .map_err(|_| service_unavailable("resource"))?
        .exists_in_workspace(&resource_id, &workspace_id);
    if !registered {
        // Reconcile through the normal control mutation path so a recovered
        // registration is durably written and visible to every API instance.
        // The same idempotency key is safe here: mutation receipts are scoped by
        // command, while lease idempotency is scoped by the coordination store.
        let _ = mutate::<bool, _>(
            &state,
            &headers,
            &principal,
            &grants,
            workspace_id.as_str(),
            "resource.registration.reconcile",
            serde_json::json!({
                "workspaceId": workspace_id,
                "resourceId": resource_id,
            }),
            ResourceScope::resource(workspace_id.clone(), resource_id.clone()),
            resource_id.as_str(),
            || {
                crate::services::profile_sync::reconcile_managed_profile_resource_registration(
                    &state,
                    &workspace_id,
                    &resource_id,
                )
                .map_err(response_problem)
            },
        )
        .await?;
    }
    if !state
        .resources
        .read()
        .map_err(|_| service_unavailable("resource"))?
        .exists_in_workspace(&resource_id, &workspace_id)
    {
        return Err(problem_response(
            "resource_not_found",
            "Resource not found",
            404,
            correlation(&headers),
        ));
    }
    let auth = CoordinationAuth::new(input.owner_id, bearer_token(&headers).to_owned())
        .map_err(response_problem)?;
    let backend = state.coordination().map_err(response_problem)?;
    let lease = backend
        .acquire_lease_idempotent(
            CoordinationScope {
                workspace_id: &workspace_id,
                auth: &auth,
            },
            &resource_id,
            Duration::from_secs(input.ttl_seconds),
            idempotency_key,
        )
        .await
        .map_err(response_problem)?;
    Ok(Json(lease))
}

async fn get_resource_lease(
    State(state): State<ApiState>,
    Path((workspace_path, resource_path)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<Option<buglogin_sync::coordination::LeaseStatus>>, Response> {
    let principal = authenticate(&state, &headers)?;
    let workspace_id =
        WorkspaceId::from_str(&workspace_path).map_err(|_| bad_id("workspace_id"))?;
    let resource_id = ResourceId::from_str(&resource_path).map_err(|_| bad_id("resource_id"))?;
    let grants = grants(&state)?;
    authorize_lease(
        &state,
        &principal,
        &grants,
        &workspace_id,
        &resource_id,
        "profile.read",
        Action::Read,
    )?;
    let actor_id = principal.account_id.as_ref().ok_or_else(|| {
        problem_response(
            "account_missing",
            "Authenticated account is missing",
            401,
            correlation(&headers),
        )
    })?;
    let auth = CoordinationAuth::new(actor_id.as_str(), bearer_token(&headers).to_owned())
        .map_err(response_problem)?;
    let status = state
        .coordination()
        .map_err(response_problem)?
        .lease_status(
            CoordinationScope {
                workspace_id: &workspace_id,
                auth: &auth,
            },
            &resource_id,
        )
        .await
        .map_err(response_problem)?;
    Ok(Json(status))
}

async fn renew_resource_lease(
    State(state): State<ApiState>,
    Path((workspace_id, resource_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(input): Json<LeaseRenewInput>,
) -> Result<Json<buglogin_sync::coordination::LeaseGrant>, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let workspace_id = WorkspaceId::from_str(&workspace_id).map_err(|_| bad_id("workspace_id"))?;
    let resource_id = ResourceId::from_str(&resource_id).map_err(|_| bad_id("resource_id"))?;
    let rotation_pending = key_rotation_blocks_resource(&state, &workspace_id, &resource_id)?;
    let (capability, action) = resource_lease_authority(rotation_pending);
    authorize_lease(
        &state,
        &principal,
        &grants,
        &workspace_id,
        &resource_id,
        capability,
        action,
    )?;
    let idempotency_key = require_idempotency(&headers)?;
    let auth = CoordinationAuth::new(input.owner_id, bearer_token(&headers).to_owned())
        .map_err(response_problem)?;
    let backend = state.coordination().map_err(response_problem)?;
    let lease = backend
        .renew_lease_idempotent(
            CoordinationScope {
                workspace_id: &workspace_id,
                auth: &auth,
            },
            &resource_id,
            &input.owner_token,
            Duration::from_secs(input.ttl_seconds),
            idempotency_key,
        )
        .await
        .map_err(response_problem)?;
    Ok(Json(lease))
}

fn key_rotation_blocks_resource(
    state: &ApiState,
    workspace_id: &WorkspaceId,
    resource_id: &ResourceId,
) -> Result<bool, Response> {
    if !state
        .resource_key_rotation_pending(workspace_id, resource_id)
        .map_err(response_problem)?
    {
        return Ok(false);
    }
    let Some(binding) = state
        .profile_resource_for_resource(workspace_id, resource_id)
        .map_err(response_problem)?
    else {
        return Ok(true);
    };
    let managed_key_available = state
        .current_sync_manifest(workspace_id, resource_id)
        .map_err(response_problem)?
        .and_then(|manifest| manifest.payload_encryption)
        .and_then(|encryption| encryption.key_version)
        .map(|key_version| {
            state.has_managed_profile_key_envelope(
                &binding.profile_id,
                workspace_id,
                resource_id,
                &key_version,
            )
        })
        .transpose()
        .map_err(response_problem)?
        .unwrap_or(false);
    Ok(!managed_key_available)
}

fn resource_lease_authority(rotation_pending: bool) -> (&'static str, Action) {
    if rotation_pending {
        ("profile.update", Action::Update)
    } else {
        ("profile.launch", Action::Execute)
    }
}

async fn release_resource_lease(
    State(state): State<ApiState>,
    Path((workspace_id, resource_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(input): Json<LeaseReleaseInput>,
) -> Result<Json<serde_json::Value>, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let workspace_id = WorkspaceId::from_str(&workspace_id).map_err(|_| bad_id("workspace_id"))?;
    let resource_id = ResourceId::from_str(&resource_id).map_err(|_| bad_id("resource_id"))?;
    authorize_lease(
        &state,
        &principal,
        &grants,
        &workspace_id,
        &resource_id,
        "profile.stop",
        Action::Execute,
    )?;
    let idempotency_key = require_idempotency(&headers)?;
    let auth = CoordinationAuth::new(input.owner_id, bearer_token(&headers).to_owned())
        .map_err(response_problem)?;
    let backend = state.coordination().map_err(response_problem)?;
    match backend
        .release_lease_idempotent(
            CoordinationScope {
                workspace_id: &workspace_id,
                auth: &auth,
            },
            &resource_id,
            &input.owner_token,
            idempotency_key,
        )
        .await
    {
        Ok(()) => Ok(Json(serde_json::json!({
            "resourceId": resource_id,
            "released": true
        }))),
        Err(error) => Err(response_problem(error).into()),
    }
}

async fn list_profile_key_rotation_recipients(
    State(state): State<ApiState>,
    Path(profile_path): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Vec<crate::services::profile_hosted::ProfileKeyRotationRecipientDevices>>, Response>
{
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let profile_id = ProfileId::from_str(&profile_path).map_err(|_| bad_id("profile_id"))?;
    let token = bearer_token(&headers);
    let now = super::request_now(&headers)?;
    let (device_account, device_id) = state
        .auth
        .read()
        .map_err(|_| service_unavailable("authentication"))?
        .authenticate_device(token, now)
        .map_err(response_problem)?;
    if principal.account_id.as_ref() != Some(&device_account) {
        return Err(problem_response(
            "device_session_account_mismatch",
            "Device session does not match the authenticated account",
            403,
            correlation(&headers),
        ));
    }
    let recipients = HostedProfileService::list_profile_key_rotation_recipients(
        &state,
        &principal,
        &grants,
        &profile_id,
        &device_id,
    )
    .map_err(response_problem)?;
    Ok(Json(recipients))
}

async fn get_resource_manifest(
    State(state): State<ApiState>,
    Path((workspace_id, resource_id)): Path<(String, String)>,
    Query(query): Query<ManifestReadQuery>,
    headers: HeaderMap,
) -> Result<Json<Option<SyncManifest>>, Response> {
    let principal = authenticate(&state, &headers)?;
    let workspace_id = WorkspaceId::from_str(&workspace_id).map_err(|_| bad_id("workspace_id"))?;
    let resource_id = ResourceId::from_str(&resource_id).map_err(|_| bad_id("resource_id"))?;
    let grants = grants(&state)?;
    authorize_lease(
        &state,
        &principal,
        &grants,
        &workspace_id,
        &resource_id,
        "profile.read",
        Action::Read,
    )?;
    if query.repair_stale_proxy {
        authorize_lease(
            &state,
            &principal,
            &grants,
            &workspace_id,
            &resource_id,
            "profile.update",
            Action::Update,
        )?;
    }
    require_actor_header(&principal, &headers)?;
    let manifest = state
        .current_sync_manifest(&workspace_id, &resource_id)
        .map_err(response_problem)?;
    if let Some(manifest) = manifest.as_ref().filter(|manifest| {
        !(manifest.generation == 1
            && manifest.objects.is_empty()
            && manifest.tombstones.is_empty()
            && manifest.dependencies.is_empty())
    }) {
        validate_profile_proxy_dependencies(
            &state,
            &principal,
            &grants,
            &workspace_id,
            &resource_id,
            manifest,
            false,
            query.repair_stale_proxy,
            false,
        )
        .map_err(Response::from)?;
    }
    Ok(Json(manifest))
}

async fn commit_resource_manifest(
    State(state): State<ApiState>,
    Path((workspace_id, resource_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(input): Json<ResourceManifestCommitRequest>,
) -> Result<Response, Response> {
    let (manifest, key_share_envelope_updates) = match input {
        ResourceManifestCommitRequest::Manifest(manifest) => (manifest, Vec::new()),
        ResourceManifestCommitRequest::KeyRotation(input) => {
            (input.manifest, input.key_share_envelope_updates)
        }
    };
    let principal = authenticate(&state, &headers)?;
    if !key_share_envelope_updates.is_empty() {
        let (device_account, _device_id) = state
            .auth
            .read()
            .map_err(|_| service_unavailable("authentication"))?
            .authenticate_device(bearer_token(&headers), super::request_now(&headers)?)
            .map_err(response_problem)?;
        if principal.account_id.as_ref() != Some(&device_account) {
            return Err(problem_response(
                "device_session_account_mismatch",
                "Device session does not match the authenticated account",
                403,
                correlation(&headers),
            ));
        }
    }
    let workspace_id = WorkspaceId::from_str(&workspace_id).map_err(|_| bad_id("workspace_id"))?;
    let resource_id = ResourceId::from_str(&resource_id).map_err(|_| bad_id("resource_id"))?;
    let grants = grants(&state)?;
    authorize_lease(
        &state,
        &principal,
        &grants,
        &workspace_id,
        &resource_id,
        "profile.update",
        Action::Update,
    )?;
    let _ = require_idempotency(&headers)?;
    let owner_id = required_header(&headers, "x-buglogin-owner-id")?;
    let owner_token = required_header(&headers, "x-buglogin-owner-token")?;
    let fencing_token = required_header(&headers, "x-buglogin-fencing-token")?
        .parse::<u64>()
        .map_err(|_| bad_header("x-buglogin-fencing-token"))?;
    let expected_profile_revision = headers
        .get("x-buglogin-profile-revision")
        .map(|value| {
            value
                .to_str()
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
                .filter(|revision| *revision > 0)
                .ok_or_else(|| bad_header("x-buglogin-profile-revision"))
        })
        .transpose()?;
    if manifest.fencing_token != fencing_token {
        return Err(problem_response(
            "manifest_fencing_header_mismatch",
            "Manifest fencing header mismatch",
            409,
            correlation(&headers),
        ));
    }
    manifest.validate().map_err(response_problem)?;
    if manifest.workspace_id != workspace_id || manifest.resource_id != resource_id {
        return Err(response_problem(Problem::new(
            "manifest_scope_mismatch",
            "Manifest scope does not match request",
            409,
            correlation(&headers),
        ))
        .into());
    }
    let envelope_ref = validate_payload_encryption_contract(&manifest, &correlation(&headers))?;
    let auth = CoordinationAuth::new(owner_id, bearer_token(&headers).to_owned())
        .map_err(response_problem)?;
    let body = serde_json::json!({
        "expectedProfileRevision": expected_profile_revision,
        "manifest": &manifest,
        "keyShareEnvelopeUpdates": &key_share_envelope_updates,
    });
    let replay_scope = ResourceScope::resource(workspace_id.clone(), resource_id.clone());
    let replay_keys = mutation_replay_keys(
        &headers,
        &principal,
        workspace_id.as_str(),
        "profile.manifest.commit",
        &body,
        &replay_scope,
        resource_id.as_str(),
    )?;
    if let Some(response) = replay_authorized_mutation_receipt(
        &state,
        &headers,
        &principal,
        &grants,
        &replay_keys,
        &workspace_id,
        &resource_id,
        "profile.update",
        Action::Update,
    )
    .await?
    {
        return Ok(response);
    }
    validate_manifest_source_references(&state, &workspace_id, &resource_id, &manifest)?;
    let backend = state.coordination().map_err(response_problem)?;
    // Reject an expired or replaced writer before touching any remote payloads.
    // Keep the second check below as a freshness check after object reads.
    backend
        .validate_write_authority(
            CoordinationScope {
                workspace_id: &workspace_id,
                auth: &auth,
            },
            &resource_id,
            owner_token,
            manifest.fencing_token,
        )
        .await
        .map_err(response_problem)?;
    for object in &manifest.objects {
        let bytes = state
            .get_remote_chunk_async(
                payload_object_key(
                    &resource_id,
                    object_storage_generation(manifest.generation, object),
                    &object.key,
                    &object.checksum,
                ),
                object.checksum.clone(),
            )
            .await
            .map_err(response_problem)?;
        if bytes.len() as u64 != object.size {
            return Err(response_problem(Problem::new(
                "manifest_object_size_mismatch",
                "Manifest object size mismatch",
                409,
                correlation(&headers),
            ))
            .into());
        }
        if let Some(envelope_ref) = envelope_ref {
            validate_payload_object_wire_format(
                &workspace_id,
                &manifest,
                envelope_ref,
                object,
                &bytes,
                &correlation(&headers),
            )?;
        }
    }
    backend
        .validate_write_authority(
            CoordinationScope {
                workspace_id: &workspace_id,
                auth: &auth,
            },
            &resource_id,
            owner_token,
            manifest.fencing_token,
        )
        .await
        .map_err(response_problem)?;
    let body = serde_json::to_value(body).map_err(|_| {
        problem_response(
            "serialization_failed",
            "Response serialization failed",
            500,
            correlation(&headers),
        )
    })?;
    mutate::<SyncManifest, _>(
        &state,
        &headers,
        &principal,
        &grants,
        workspace_id.as_str(),
        "profile.manifest.commit",
        body,
        ResourceScope::resource(workspace_id.clone(), resource_id.clone()),
        resource_id.as_str(),
        || {
            let binding = state
                .profile_resource_for_resource(&workspace_id, &resource_id)
                .map_err(response_problem)?
                .ok_or_else(|| {
                    response_problem(Problem::new(
                        "binding_not_found",
                        "Profile binding not found",
                        404,
                        "sync",
                    ))
                })?;
            let previous_manifest = state
                .current_sync_manifest(&workspace_id, &resource_id)
                .map_err(response_problem)?;
            let is_initial_encrypted_manifest = previous_manifest.as_ref().is_some_and(|previous| {
                previous.generation == 1
                    && previous.objects.is_empty()
                    && previous.payload_encryption.is_none()
            }) && manifest
                .payload_encryption
                .as_ref()
                .is_some_and(|encryption| {
                    encryption.algorithm
                        == buglogin_sync::manifest::PayloadEncryptionAlgorithm::Aes256GcmChunkV2
                });
            if is_initial_encrypted_manifest {
                // New encrypted profiles use the service-managed KEK path. The
                // desktop enrolls the resource key immediately after this
                // manifest commit; legacy per-device envelopes are not needed
                // to bootstrap a new profile.
                let _wrapping_key = state
                    .profile_key_wrapping_key()
                    .map_err(response_problem)?;
            }
            if manifest.payload_encryption.is_some() && !is_initial_encrypted_manifest {
                crate::services::profile_hosted::HostedProfileService::ensure_encrypted_manifest_share_compatible(
                    &state,
                    &binding.profile_id,
                    manifest
                        .payload_encryption
                        .as_ref()
                        .and_then(|encryption| encryption.key_version.as_deref()),
                )
                .map_err(response_problem)?;
            }
            validate_pending_key_rotation(&state, &workspace_id, &resource_id, &manifest)?;
            validate_manifest_successor(&state, &workspace_id, &resource_id, &manifest)?;
            activate_pending_profile_proxy_assignment(
                &state,
                &workspace_id,
                &resource_id,
                &manifest,
                expected_profile_revision,
            )?;
            validate_profile_proxy_dependencies(
                &state,
                &principal,
                &grants,
                &workspace_id,
                &resource_id,
                &manifest,
                true,
                false,
                true,
            )?;
            validate_workspace_sync_storage_quota(
                &state,
                &workspace_id,
                &resource_id,
                &manifest,
            )?;
            state
                .validate_sync_manifest_gc_claims(&manifest)
                .map_err(response_problem)?;
            let rotation_pending = state
                .resource_key_rotation_pending(&workspace_id, &resource_id)
                .map_err(response_problem)?;
            if is_initial_encrypted_manifest {
                manifest
                    .payload_encryption
                    .as_ref()
                    .and_then(|encryption| encryption.key_version.as_deref())
                    .ok_or_else(|| {
                        response_problem(Problem::new(
                            "profile_key_version_missing",
                            "Initial encrypted manifest has no resource key version",
                            409,
                            correlation(&headers),
                        ))
                    })?;
                // Older clients may still include a per-device envelope in
                // this request. Ignore it for initial enrollment; the KEK
                // envelope is persisted by profile.sync.key.enroll after the
                // manifest commit, before the stop/sync operation completes.
            } else if rotation_pending {
                let key_version = manifest
                    .payload_encryption
                    .as_ref()
                    .and_then(|encryption| encryption.key_version.as_deref())
                    .ok_or_else(|| {
                        response_problem(Problem::new(
                            "profile_key_version_missing",
                            "Key rotation manifest has no resource key version",
                            409,
                            "sync",
                        ))
                    })?;
                HostedProfileService::activate_profile_key_rotation_envelopes(
                    &state,
                    &binding.profile_id,
                    key_version,
                    &key_share_envelope_updates,
                )
                .map_err(response_problem)?;
            } else if !key_share_envelope_updates.is_empty() {
                return Err(response_problem(Problem::new(
                    "sync_key_rotation_not_pending",
                    "Key envelopes may be replaced only during a pending profile key rotation",
                    409,
                    correlation(&headers),
                )));
            }
            state
                .replace_sync_manifest(manifest.clone())
                .map_err(response_problem)?;
            state
                .mark_migrated_profile_ready_after_sync(&manifest)
                .map_err(response_problem)?;
            state
                .release_committed_sync_blob_reservations(&manifest)
                .map_err(response_problem)?;
            state
                .clear_resource_key_rotation_pending(&workspace_id, &resource_id)
                .map_err(response_problem)?;
            Ok(manifest.clone())
        },
    )
    .await
}

fn validate_workspace_sync_storage_quota(
    state: &ApiState,
    workspace_id: &WorkspaceId,
    resource_id: &ResourceId,
    proposed: &SyncManifest,
) -> Result<(), ResponseProblem> {
    if state.local_authority_enabled() {
        return Ok(());
    }
    let workspace = state
        .workspaces
        .read()
        .map_err(|_| service_unavailable("workspace"))?
        .workspace_for_policy(workspace_id)
        .map_err(response_problem)?;
    let manifests = state
        .sync_manifests
        .read()
        .map_err(|_| service_unavailable("sync manifest"))?;
    let mut other_committed_bytes = 0u64;
    let mut current_resource_bytes = 0u64;
    for manifest in manifests
        .values()
        .filter(|manifest| &manifest.workspace_id == workspace_id)
    {
        let destination = if &manifest.resource_id == resource_id {
            &mut current_resource_bytes
        } else {
            &mut other_committed_bytes
        };
        for object in &manifest.objects {
            *destination = (*destination).checked_add(object.size).ok_or_else(|| {
                response_problem(Problem::new(
                    "storage_quota_exceeded",
                    "Workspace storage quota exceeded",
                    409,
                    "sync",
                ))
            })?;
        }
    }
    let proposed_bytes = proposed.objects.iter().try_fold(0u64, |total, object| {
        total.checked_add(object.size).ok_or_else(|| {
            response_problem(Problem::new(
                "storage_quota_exceeded",
                "Workspace storage quota exceeded",
                409,
                "sync",
            ))
        })
    })?;
    let proposed_storage_keys = proposed
        .objects
        .iter()
        .map(|object| super::sync_manifest_object_storage_key(proposed, object))
        .collect::<std::collections::BTreeSet<_>>();
    let committed_storage_keys = manifests
        .values()
        .filter(|manifest| &manifest.workspace_id == workspace_id)
        .flat_map(|manifest| {
            manifest
                .objects
                .iter()
                .map(|object| super::sync_manifest_object_storage_key(manifest, object))
        })
        .collect::<std::collections::BTreeSet<_>>();
    drop(manifests);
    let reservations = state
        .sync_blob_reservations
        .read()
        .map_err(|_| service_unavailable("sync blob reservation"))?;
    let mut current_reserved_bytes = 0u64;
    let mut proposed_reserved_bytes = 0u64;
    for reservation in reservations.values().filter(|reservation| {
        reservation.workspace_id == workspace_id.as_str()
            && !committed_storage_keys.contains(&reservation.storage_key)
    }) {
        current_reserved_bytes = current_reserved_bytes
            .checked_add(reservation.size)
            .ok_or_else(|| {
                response_problem(storage_quota_problem(
                    u64::MAX,
                    workspace.policy.max_storage_bytes,
                ))
            })?;
        if !proposed_storage_keys.contains(&reservation.storage_key) {
            proposed_reserved_bytes = proposed_reserved_bytes
                .checked_add(reservation.size)
                .ok_or_else(|| {
                    response_problem(storage_quota_problem(
                        u64::MAX,
                        workspace.policy.max_storage_bytes,
                    ))
                })?;
        }
    }
    let total_bytes = other_committed_bytes
        .checked_add(proposed_bytes)
        .and_then(|total| total.checked_add(proposed_reserved_bytes))
        .ok_or_else(|| {
            response_problem(Problem::new(
                "storage_quota_exceeded",
                "Workspace storage quota exceeded",
                409,
                "sync",
            ))
        })?;
    let previous_total_bytes = other_committed_bytes
        .checked_add(current_resource_bytes)
        .and_then(|total| total.checked_add(current_reserved_bytes))
        .unwrap_or(u64::MAX);
    // A workspace that is already over quota must still be able to delete or
    // shrink data. Reject only commits that increase this resource's stored
    // footprint while the resulting workspace remains above its limit.
    if total_bytes > workspace.policy.max_storage_bytes && total_bytes > previous_total_bytes {
        return Err(response_problem(storage_quota_problem(
            total_bytes,
            workspace.policy.max_storage_bytes,
        )));
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ResourceManifestKeyRotationInput {
    manifest: SyncManifest,
    #[serde(default)]
    key_share_envelope_updates: Vec<ProfileKeyRotationEnvelopeUpdate>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ResourceManifestCommitRequest {
    KeyRotation(ResourceManifestKeyRotationInput),
    Manifest(SyncManifest),
}

fn validate_payload_encryption_contract<'a>(
    manifest: &'a SyncManifest,
    correlation_id: &str,
) -> Result<Option<&'a buglogin_sync::manifest::ManifestObjectReference>, Response> {
    if manifest.objects.is_empty() {
        return Ok(None);
    }
    let Some(encryption) = manifest.payload_encryption.as_ref() else {
        return Err(problem_response(
            "payload_encryption_required",
            "Profile payloads must use client-side encryption",
            409,
            correlation_id.to_owned(),
        ));
    };
    let envelope_ref = &encryption.resource_key_envelope;
    if envelope_ref.key != buglogin_sync::manifest::RESOURCE_KEY_ENVELOPE_KEY {
        return Err(problem_response(
            "invalid_resource_key_envelope_key",
            "Resource key envelope must use its canonical manifest key",
            400,
            correlation_id.to_owned(),
        ));
    }
    Ok(Some(envelope_ref))
}

fn validate_payload_object_wire_format(
    workspace_id: &WorkspaceId,
    manifest: &SyncManifest,
    envelope_ref: &buglogin_sync::manifest::ManifestObjectReference,
    object: &buglogin_sync::manifest::ManifestObject,
    bytes: &[u8],
    correlation_id: &str,
) -> Result<(), Response> {
    if object.key == envelope_ref.key {
        if let Ok(envelope) = serde_json::from_slice::<ManagedResourceKeyEnvelope>(bytes) {
            if envelope.schema_version != 1
                || &envelope.workspace_id != workspace_id
                || envelope.resource_id != manifest.resource_id
                || manifest
                    .payload_encryption
                    .as_ref()
                    .and_then(|encryption| encryption.key_version.as_deref())
                    != Some(envelope.key_version.as_str())
            {
                return Err(problem_response(
                    "managed_profile_key_scope_mismatch",
                    "Managed key envelope does not match the manifest scope or key version",
                    409,
                    correlation_id.to_owned(),
                ));
            }
            return Ok(());
        }
        let envelope: ResourceKeyEnvelope = serde_json::from_slice(bytes).map_err(|_| {
            problem_response(
                "resource_key_envelope_invalid",
                "Resource key envelope is invalid",
                409,
                correlation_id.to_owned(),
            )
        })?;
        if envelope.schema_version != 1
            || &envelope.workspace_id != workspace_id
            || envelope.resource_id != manifest.resource_id
        {
            return Err(problem_response(
                "resource_key_envelope_scope_mismatch",
                "Resource key envelope does not match the manifest scope",
                409,
                correlation_id.to_owned(),
            ));
        }
        return Ok(());
    }

    if manifest
        .dependencies
        .iter()
        .any(|dependency| match dependency {
            ManifestDependency::Proxy {
                metadata_object: Some(metadata_object),
                ..
            } => object.key == metadata_object.key,
            ManifestDependency::Proxy {
                metadata_object: None,
                ..
            } => false,
        })
    {
        let metadata: ProxyRecordView = serde_json::from_slice(bytes).map_err(|_| {
            problem_response(
                "proxy_metadata_object_invalid",
                "Proxy metadata object is invalid",
                409,
                correlation_id.to_owned(),
            )
        })?;
        if &metadata.workspace_id != workspace_id {
            return Err(problem_response(
                "api_scope_mismatch",
                "Proxy metadata does not match the workspace",
                409,
                correlation_id.to_owned(),
            ));
        }
        return Ok(());
    }

    if let Some((proxy_id, revision)) =
        manifest
            .dependencies
            .iter()
            .find_map(|dependency| match dependency {
                ManifestDependency::Proxy {
                    proxy_id,
                    revision,
                    credential_envelope_object: Some(reference),
                    ..
                } if object.key == reference.key => Some((proxy_id, revision)),
                _ => None,
            })
    {
        let envelope: ProxyCredentialEnvelope = serde_json::from_slice(bytes).map_err(|_| {
            problem_response(
                "proxy_credential_envelope_invalid",
                "Proxy credential envelope is invalid",
                409,
                correlation_id.to_owned(),
            )
        })?;
        if envelope.schema_version != 1
            || &envelope.workspace_id != workspace_id
            || envelope.resource_id != manifest.resource_id
            || &envelope.proxy_id != proxy_id
            || &envelope.proxy_revision != revision
            || envelope.encrypted_secret.version != 1
        {
            return Err(problem_response(
                "proxy_credential_envelope_scope_mismatch",
                "Proxy credential envelope does not match the manifest dependency",
                409,
                correlation_id.to_owned(),
            ));
        }
        return Ok(());
    }

    let authenticated = AuthenticatedChunk::decode(bytes).ok();
    if authenticated.is_none_or(|chunk| chunk.version != 1) {
        return Err(problem_response(
            "encrypted_payload_object_invalid",
            "Profile payload object is not a supported encrypted chunk",
            409,
            correlation_id.to_owned(),
        ));
    }
    Ok(())
}

fn validate_manifest_successor(
    state: &ApiState,
    workspace_id: &WorkspaceId,
    resource_id: &ResourceId,
    manifest: &SyncManifest,
) -> Result<(), ResponseProblem> {
    let current = state
        .current_sync_manifest(workspace_id, resource_id)
        .map_err(response_problem)?;
    if let Some(current) = current.as_ref() {
        if current == manifest {
            return Ok(());
        }
        if let Some(current_encryption) = current.payload_encryption.as_ref()
            && current_encryption.algorithm
                == buglogin_sync::manifest::PayloadEncryptionAlgorithm::Aes256GcmChunkV2
        {
            let candidate_encryption = manifest.payload_encryption.as_ref().filter(|candidate| {
                candidate.algorithm
                    == buglogin_sync::manifest::PayloadEncryptionAlgorithm::Aes256GcmChunkV2
            });
            let Some(candidate_encryption) = candidate_encryption else {
                return Err(response_problem(Problem::new(
                    "sync_key_rotation_invalid",
                    "Versioned profile encryption cannot be downgraded",
                    409,
                    "sync",
                )));
            };
            if candidate_encryption.key_version != current_encryption.key_version {
                validate_key_rotation_successor(current, manifest)?;
            }
        }
        if manifest.fencing_token < current.fencing_token
            || current.generation.checked_add(1) != Some(manifest.generation)
            || manifest.parent_generation != Some(current.generation)
        {
            return Err(response_problem(Problem::new(
                "sync_manifest_conflict",
                "Manifest is not a contiguous successor",
                409,
                "sync",
            )));
        }
        manifest
            .validate_successor(
                current,
                checked_now_ms("sync-manifest").map_err(response_problem)?,
            )
            .map_err(response_problem)?;
        validate_reused_manifest_objects(current, manifest)?;
    } else {
        manifest
            .validate_new_profile_paths()
            .map_err(response_problem)?;
        if manifest.generation != 1
            || manifest.parent_generation.is_some()
            || manifest
                .objects
                .iter()
                .any(|object| object.source_generation.is_some())
        {
            return Err(response_problem(Problem::new(
                "manifest_base_missing",
                "Manifest base is missing",
                409,
                "sync",
            )));
        }
    }
    Ok(())
}

fn validate_pending_key_rotation(
    state: &ApiState,
    workspace_id: &WorkspaceId,
    resource_id: &ResourceId,
    candidate: &SyncManifest,
) -> Result<(), ResponseProblem> {
    if !state
        .resource_key_rotation_pending(workspace_id, resource_id)
        .map_err(response_problem)?
    {
        return Ok(());
    }
    let current = state
        .current_sync_manifest(workspace_id, resource_id)
        .map_err(response_problem)?
        .ok_or_else(|| {
            response_problem(Problem::new(
                "sync_key_rotation_base_missing",
                "The active manifest required for key rotation is missing",
                409,
                "sync",
            ))
        })?;
    let current_version = current
        .payload_encryption
        .as_ref()
        .and_then(|value| value.key_version.as_ref());
    let candidate_encryption = candidate.payload_encryption.as_ref();
    let candidate_version = candidate_encryption.and_then(|value| value.key_version.as_ref());
    if current_version == candidate_version
        && let Some(binding) = state
            .profile_resource_for_resource(workspace_id, resource_id)
            .map_err(response_problem)?
        && state
            .has_managed_profile_key_envelope(
                &binding.profile_id,
                workspace_id,
                resource_id,
                current_version.map(String::as_str).unwrap_or_default(),
            )
            .map_err(response_problem)?
    {
        return Ok(());
    }
    let is_new_version = candidate_version
        .is_some_and(|version| current_version.is_none_or(|previous| previous != version));
    let uses_v2 = candidate_encryption.is_some_and(|value| {
        value.algorithm == buglogin_sync::manifest::PayloadEncryptionAlgorithm::Aes256GcmChunkV2
    });
    if !is_new_version || !uses_v2 {
        return Err(response_problem(Problem::new(
            "sync_key_rotation_required",
            "A new v2 resource key and fully re-encrypted manifest are required before shared access resumes",
            409,
            "sync",
        )));
    }
    validate_key_rotation_successor(&current, candidate)
}

fn validate_key_rotation_successor(
    current: &SyncManifest,
    candidate: &SyncManifest,
) -> Result<(), ResponseProblem> {
    let Some(current_encryption) = current.payload_encryption.as_ref() else {
        return Err(response_problem(Problem::new(
            "sync_key_rotation_invalid",
            "Active profile encryption metadata is missing",
            409,
            "sync",
        )));
    };
    let Some(candidate_encryption) = candidate.payload_encryption.as_ref() else {
        return Err(response_problem(Problem::new(
            "sync_key_rotation_invalid",
            "Rotated profile encryption metadata is missing",
            409,
            "sync",
        )));
    };
    if candidate
        .objects
        .iter()
        .any(|object| object.source_generation.is_some())
    {
        return Err(response_problem(Problem::new(
            "sync_key_rotation_requires_full_reencrypt",
            "Key rotation must upload every object with the new resource key",
            409,
            "sync",
        )));
    }
    if current_encryption.resource_key_envelope.checksum
        == candidate_encryption.resource_key_envelope.checksum
    {
        return Err(response_problem(Problem::new(
            "sync_key_rotation_envelope_unchanged",
            "Key rotation must publish a new resource key envelope",
            409,
            "sync",
        )));
    }
    let clear_metadata_keys = current
        .dependencies
        .iter()
        .filter_map(|dependency| match dependency {
            buglogin_sync::manifest::ManifestDependency::Proxy {
                metadata_object: Some(reference),
                ..
            } => Some(reference.key.as_str()),
            _ => None,
        })
        .collect::<std::collections::BTreeSet<_>>();
    for previous in &current.objects {
        if previous.key == current_encryption.resource_key_envelope.key
            || clear_metadata_keys.contains(previous.key.as_str())
        {
            continue;
        }
        if candidate
            .objects
            .iter()
            .any(|object| object.key == previous.key && object.checksum == previous.checksum)
        {
            return Err(response_problem(Problem::new(
                "sync_key_rotation_object_unchanged",
                "Key rotation cannot retain ciphertext encrypted with the previous key",
                409,
                "sync",
            )));
        }
    }
    let candidate_keys = candidate
        .objects
        .iter()
        .map(|object| object.key.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let tombstone_keys = candidate
        .tombstones
        .iter()
        .map(|tombstone| tombstone.key.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    if current.objects.iter().any(|object| {
        !candidate_keys.contains(object.key.as_str())
            && !tombstone_keys.contains(object.key.as_str())
    }) {
        return Err(response_problem(Problem::new(
            "sync_key_rotation_incomplete",
            "Key rotation must retain or tombstone every prior object",
            409,
            "sync",
        )));
    }
    Ok(())
}

fn validate_manifest_source_references(
    state: &ApiState,
    workspace_id: &WorkspaceId,
    resource_id: &ResourceId,
    manifest: &SyncManifest,
) -> Result<(), Response> {
    let current = state
        .current_sync_manifest(workspace_id, resource_id)
        .map_err(response_problem)?;
    if let Some(current) = current.as_ref() {
        validate_reused_manifest_objects(current, manifest).map_err(Response::from)?;
    } else if manifest
        .objects
        .iter()
        .any(|object| object.source_generation.is_some())
    {
        return Err(response_problem(Problem::new(
            "manifest_object_reuse_not_authorized",
            "Objects can only be reused from an active manifest",
            409,
            "sync",
        ))
        .into());
    }
    Ok(())
}

fn validate_reused_manifest_objects(
    current: &SyncManifest,
    candidate: &SyncManifest,
) -> Result<(), ResponseProblem> {
    for object in candidate
        .objects
        .iter()
        .filter(|object| object.source_generation.is_some())
    {
        let source_generation = object.source_generation.unwrap_or_default();
        let found_in_current = current.objects.iter().any(|previous| {
            previous.key == object.key
                && previous.size == object.size
                && previous.checksum == object.checksum
                && object_storage_generation(current.generation, previous) == source_generation
        });
        let current_key_version = current
            .payload_encryption
            .as_ref()
            .and_then(|encryption| encryption.key_version.as_deref());
        let candidate_key_version = candidate
            .payload_encryption
            .as_ref()
            .and_then(|encryption| encryption.key_version.as_deref());
        if !found_in_current || current_key_version != candidate_key_version {
            return Err(response_problem(Problem::new(
                "manifest_object_reuse_not_authorized",
                "Object reuse must match the active manifest and resource key version",
                409,
                "sync",
            )));
        }
    }
    Ok(())
}

fn object_storage_generation(
    manifest_generation: u64,
    object: &buglogin_sync::manifest::ManifestObject,
) -> u64 {
    object.source_generation.unwrap_or(manifest_generation)
}

fn referenced_object_storage_generation(
    manifest: &SyncManifest,
    reference: &buglogin_sync::manifest::ManifestObjectReference,
) -> Option<u64> {
    manifest
        .objects
        .iter()
        .find(|object| {
            object.key == reference.key
                && object.size == reference.size
                && object.checksum == reference.checksum
        })
        .map(|object| object_storage_generation(manifest.generation, object))
}

fn activate_pending_profile_proxy_assignment(
    state: &ApiState,
    workspace_id: &WorkspaceId,
    resource_id: &ResourceId,
    manifest: &SyncManifest,
    expected_profile_revision: Option<u64>,
) -> Result<(), ResponseProblem> {
    let mut bound_profile = None;
    for profile in state.list_profile_records().map_err(response_problem)? {
        let binding = state
            .profile_resource_record(&profile.value.id)
            .map_err(response_problem)?;
        if binding.as_ref().is_some_and(|binding| {
            &binding.workspace_id == workspace_id && &binding.resource_id == resource_id
        }) && bound_profile.replace(profile).is_some()
        {
            return Err(response_problem(Problem::new(
                "profile_binding_ambiguous",
                "Resource has multiple profile bindings",
                409,
                "sync",
            )));
        }
    }
    let Some(profile) = bound_profile else {
        return Ok(());
    };
    let pending = state
        .profile_proxy_assignments
        .read()
        .map_err(|_| service_unavailable("profile proxy assignment"))?
        .get_optional(workspace_id, &profile.value.id);
    let Some(pending) = pending else {
        return Ok(());
    };
    if state.local_authority_enabled() {
        return Err(response_problem(Problem::new(
            "hosted_proxy_assignment_required",
            "Pending proxy assignment requires hosted profile storage",
            503,
            "sync",
        )));
    }
    let expected_profile_revision = expected_profile_revision.ok_or_else(|| {
        response_problem(Problem::new(
            "profile_revision_required",
            "Profile revision header is required to activate a proxy assignment",
            400,
            "sync",
        ))
    })?;
    if expected_profile_revision != pending.expected_profile_revision
        || profile.revision != pending.expected_profile_revision
    {
        return Err(response_problem(
            Problem::new(
                "profile_proxy_assignment_stale",
                "Profile or pending proxy assignment changed before sync",
                409,
                "sync",
            )
            .with_field("expectedRevision", pending.expected_profile_revision)
            .with_field("actualRevision", profile.revision),
        ));
    }
    if profile.value.archived || !profile.value.state.allows_proxy_change() {
        return Err(response_problem(Problem::new(
            "profile_proxy_assignment_not_allowed",
            "Profile state does not allow proxy assignment",
            409,
            "sync",
        )));
    }
    let manifest_proxy_id = match manifest.dependencies.as_slice() {
        [] => None,
        [ManifestDependency::Proxy { proxy_id, .. }] => Some(proxy_id.clone()),
        _ => {
            return Err(response_problem(Problem::new(
                "manifest_dependency_mismatch",
                "Manifest must contain at most one proxy dependency",
                409,
                "sync",
            )));
        }
    };
    if manifest_proxy_id != pending.proxy_id {
        return Err(response_problem(Problem::new(
            "profile_proxy_assignment_manifest_mismatch",
            "Manifest proxy dependency does not match the pending assignment",
            409,
            "sync",
        )));
    }

    let mut updated = profile.value;
    updated.proxy_id = pending.proxy_id.clone();
    updated.updated_at =
        crate::clock::rfc3339("profile.proxy.assignment").map_err(response_problem)?;
    state
        .update_profile_record(expected_profile_revision, updated)
        .map_err(response_problem)?;
    let consumed = state
        .profile_proxy_assignments
        .write()
        .map_err(|_| service_unavailable("profile proxy assignment"))?
        .consume(workspace_id, &pending.profile_id, expected_profile_revision)
        .map_err(response_problem)?;
    if consumed != pending {
        return Err(response_problem(Problem::new(
            "profile_proxy_assignment_conflict",
            "Pending profile proxy assignment changed",
            409,
            "sync",
        )));
    }
    Ok(())
}

#[expect(
    clippy::too_many_arguments,
    reason = "explicit inputs make authorization and revision checks auditable"
)]
fn validate_profile_proxy_dependencies(
    state: &ApiState,
    principal: &Principal,
    grants: &[buglogin_domain::permissions::Grant],
    workspace_id: &WorkspaceId,
    resource_id: &ResourceId,
    manifest: &SyncManifest,
    verify_metadata_object: bool,
    allow_stale_proxy_revision: bool,
    require_credential_version: bool,
) -> Result<(), ResponseProblem> {
    validate_profile_proxy_dependencies_problem(
        state,
        principal,
        grants,
        workspace_id,
        resource_id,
        manifest,
        verify_metadata_object,
        allow_stale_proxy_revision,
        require_credential_version,
    )
    .map_err(response_problem)
}

#[expect(
    clippy::too_many_arguments,
    reason = "explicit inputs make authorization and revision checks auditable"
)]
pub(super) fn validate_profile_proxy_dependencies_problem(
    state: &ApiState,
    principal: &Principal,
    grants: &[buglogin_domain::permissions::Grant],
    workspace_id: &WorkspaceId,
    resource_id: &ResourceId,
    manifest: &SyncManifest,
    verify_metadata_object: bool,
    allow_stale_proxy_revision: bool,
    require_credential_version: bool,
) -> Result<(), Problem> {
    let record = crate::services::profile_sync::validate_proxy_dependency(
        state,
        principal,
        grants,
        workspace_id,
        resource_id,
        manifest,
        allow_stale_proxy_revision,
        require_credential_version,
    )?;
    let Some(record) = record else {
        return Ok(());
    };
    let metadata_object = manifest
        .dependencies
        .iter()
        .find_map(|dependency| match dependency {
            ManifestDependency::Proxy {
                metadata_object, ..
            } => metadata_object.as_ref(),
        });
    if verify_metadata_object && let Some(metadata_object) = metadata_object {
        let metadata_bytes = state.get_remote_chunk(
            &payload_object_key(
                resource_id,
                referenced_object_storage_generation(manifest, metadata_object).ok_or_else(
                    || {
                        Problem::new(
                            "manifest_dependency_object_missing",
                            "Proxy metadata object is not in the manifest",
                            409,
                            "sync",
                        )
                    },
                )?,
                &metadata_object.key,
                &metadata_object.checksum,
            ),
            &metadata_object.checksum,
        )?;
        let metadata: ProxyRecordView = serde_json::from_slice(&metadata_bytes).map_err(|_| {
            Problem::new(
                "proxy_metadata_object_invalid",
                "Proxy metadata object is invalid",
                400,
                "sync",
            )
        })?;
        if metadata != proxy_record_view(record) {
            return Err(Problem::new(
                "proxy_metadata_object_mismatch",
                "Proxy metadata object does not match the authorized proxy revision",
                409,
                "sync",
            ));
        }
    }
    Ok(())
}

async fn get_resource_blob(
    State(state): State<ApiState>,
    Path((workspace_id, resource_id)): Path<(String, String)>,
    Query(query): Query<BlobReadQuery>,
    headers: HeaderMap,
) -> Result<Response, Response> {
    let principal = authenticate(&state, &headers)?;
    let workspace_id = WorkspaceId::from_str(&workspace_id).map_err(|_| bad_id("workspace_id"))?;
    let resource_id = ResourceId::from_str(&resource_id).map_err(|_| bad_id("resource_id"))?;
    let grants = grants(&state)?;
    authorize_lease(
        &state,
        &principal,
        &grants,
        &workspace_id,
        &resource_id,
        "profile.read",
        Action::Read,
    )?;
    if query.repair_stale_proxy {
        authorize_lease(
            &state,
            &principal,
            &grants,
            &workspace_id,
            &resource_id,
            "profile.update",
            Action::Update,
        )?;
    }
    let generation = required_header(&headers, "x-buglogin-generation")?
        .parse::<u64>()
        .map_err(|_| bad_header("x-buglogin-generation"))?;
    let expected_checksum = required_header(&headers, "x-content-sha256")?;
    require_actor_header(&principal, &headers)?;
    let manifest = state
        .current_sync_manifest(&workspace_id, &resource_id)
        .map_err(response_problem)?
        .filter(|value| value.generation == generation)
        .ok_or_else(|| {
            problem_response(
                "manifest_not_found",
                "Manifest not found",
                404,
                correlation(&headers),
            )
        })?;
    // Blob reads are a second path to profile data. Keep their proxy ACL and
    // revision checks identical to manifest reads so a caller cannot bypass a
    // revoked proxy grant by replaying a previously observed object key.
    validate_profile_proxy_dependencies(
        &state,
        &principal,
        &grants,
        &workspace_id,
        &resource_id,
        &manifest,
        false,
        query.repair_stale_proxy,
        false,
    )
    .map_err(Response::from)?;
    let object = manifest
        .objects
        .iter()
        .find(|object| object.key == query.key && object.checksum == expected_checksum)
        .ok_or_else(|| {
            problem_response(
                "object_not_in_manifest",
                "Object is not in manifest",
                404,
                correlation(&headers),
            )
        })?;
    let admission = super::large_transfer_body_slot()
        .await
        .map_err(response_problem)?;
    let bytes = state
        .get_remote_chunk_async(
            payload_object_key(
                &resource_id,
                object_storage_generation(generation, object),
                &object.key,
                &object.checksum,
            ),
            object.checksum.clone(),
        )
        .await
        .map_err(response_problem)?;
    // Keep the shared transfer slot until the HTTP body is consumed or dropped,
    // not merely until the S3 read finishes. This bounds retained response bytes.
    let body = Body::from_stream(futures_util::stream::unfold(
        (Some(Bytes::from(bytes)), Some(admission)),
        |(chunk, admission)| async move {
            match chunk {
                Some(chunk) => Some((Ok::<_, Infallible>(chunk), (None, admission))),
                None => {
                    drop(admission);
                    None
                }
            }
        },
    ));
    Ok((
        StatusCode::OK,
        [("content-type", "application/octet-stream")],
        body,
    )
        .into_response())
}

async fn check_resource_blob(
    State(state): State<ApiState>,
    Path((workspace_id, resource_id)): Path<(String, String)>,
    Query(query): Query<BlobCheckQuery>,
    headers: HeaderMap,
) -> Result<Json<Value>, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let workspace_id = WorkspaceId::from_str(&workspace_id).map_err(|_| bad_id("workspace_id"))?;
    let resource_id = ResourceId::from_str(&resource_id).map_err(|_| bad_id("resource_id"))?;
    authorize_lease(
        &state,
        &principal,
        &grants,
        &workspace_id,
        &resource_id,
        "profile.update",
        Action::Update,
    )?;
    if query.generation == 0
        || query.checksum.len() != 64
        || !query
            .checksum
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(problem_response(
            "blob_check_input_invalid",
            "Blob check input is invalid",
            400,
            correlation(&headers),
        ));
    }
    ManifestObject::new(query.key.clone(), &[]).map_err(response_problem)?;
    let owner_id = required_header(&headers, "x-buglogin-owner-id")?;
    let owner_token = required_header(&headers, "x-buglogin-owner-token")?;
    let fencing_token = required_header(&headers, "x-buglogin-fencing-token")?
        .parse::<u64>()
        .map_err(|_| bad_header("x-buglogin-fencing-token"))?;
    let auth = CoordinationAuth::new(owner_id, bearer_token(&headers).to_owned())
        .map_err(response_problem)?;
    state
        .coordination()
        .map_err(response_problem)?
        .validate_write_authority(
            CoordinationScope {
                workspace_id: &workspace_id,
                auth: &auth,
            },
            &resource_id,
            owner_token,
            fencing_token,
        )
        .await
        .map_err(response_problem)?;
    let storage_key =
        payload_object_key(&resource_id, query.generation, &query.key, &query.checksum);
    let present = match state
        .remote_chunk_checksum_async(storage_key)
        .await
        .map_err(response_problem)?
    {
        Some(actual) if actual == query.checksum => true,
        Some(_) => {
            return Err(response_problem(Problem::new(
                "remote_chunk_conflict",
                "Staged blob checksum conflicts with the requested checksum",
                409,
                correlation(&headers),
            ))
            .into());
        }
        None => false,
    };
    Ok(Json(json!({ "present": present })))
}

async fn put_resource_blob(
    State(state): State<ApiState>,
    Path((workspace_id, resource_id)): Path<(String, String)>,
    Query(query): Query<BlobQuery>,
    request: Request,
) -> Result<Response, Response> {
    let (parts, body) = request.into_parts();
    let headers = parts.headers;
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let workspace_id = WorkspaceId::from_str(&workspace_id).map_err(|_| bad_id("workspace_id"))?;
    let resource_id = ResourceId::from_str(&resource_id).map_err(|_| bad_id("resource_id"))?;
    authorize_lease(
        &state,
        &principal,
        &grants,
        &workspace_id,
        &resource_id,
        "profile.update",
        Action::Update,
    )?;
    let idempotency_key = require_idempotency(&headers)?.to_owned();
    let owner_id = required_header(&headers, "x-buglogin-owner-id")?;
    let owner_token = required_header(&headers, "x-buglogin-owner-token")?;
    let fencing_token = required_header(&headers, "x-buglogin-fencing-token")?
        .parse::<u64>()
        .map_err(|_| bad_header("x-buglogin-fencing-token"))?;
    let generation = required_header(&headers, "x-buglogin-generation")?
        .parse::<u64>()
        .map_err(|_| bad_header("x-buglogin-generation"))?;
    let expected_checksum = required_header(&headers, "x-content-sha256")?;
    if super::declared_body_exceeds_limit(&headers, MAX_PROFILE_SYNC_OBJECT_BYTES as usize)
        .map_err(response_problem)?
    {
        return Err(problem_response(
            "manifest_object_too_large",
            "Manifest object exceeds the maximum size",
            413,
            correlation(&headers),
        ));
    }
    let body_admission = super::large_transfer_body_slot()
        .await
        .map_err(response_problem)?;
    let body = axum::body::to_bytes(body, MAX_PROFILE_SYNC_OBJECT_BYTES as usize)
        .await
        .map_err(|_| {
            problem_response(
                "manifest_object_too_large",
                "Manifest object exceeds the maximum size",
                413,
                correlation(&headers),
            )
        })?;
    let body_size = body.len() as u64;
    let actor_id = principal
        .account_id
        .as_ref()
        .map(AccountId::as_str)
        .unwrap_or("anonymous");
    let idempotency_store_key = format!(
        "{actor_id}:{}:{}:{idempotency_key}",
        workspace_id.as_str(),
        resource_id.as_str()
    );
    let request_input = json!({
        "workspaceId": workspace_id,
        "resourceId": resource_id,
        "key": query.key,
        "generation": generation,
        "checksum": expected_checksum,
        "size": body_size,
        "ownerId": owner_id,
        "fencingToken": fencing_token
    });
    let request_hash = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&request_input).map_err(|_| {
            problem_response(
                "invalid_input",
                "Blob input is invalid",
                400,
                correlation(&headers),
            )
        })?)
    );
    if let Some(record) = state
        .blob_idempotency_record(&idempotency_store_key)
        .map_err(response_problem)?
    {
        if record.request_hash != request_hash {
            return Err(problem_response(
                "idempotency_conflict",
                "Idempotency key conflict",
                409,
                correlation(&headers),
            ));
        }
        let storage_key =
            payload_object_key(&resource_id, generation, &query.key, expected_checksum);
        match state
            .get_remote_chunk_async(storage_key.clone(), expected_checksum.to_owned())
            .await
        {
            Ok(_) => return Ok(Json(record.receipt).into_response()),
            Err(error) if error.code == "remote_chunk_not_found" => {}
            Err(error) => return Err(response_problem(error).into()),
        }
    }
    let auth = CoordinationAuth::new(owner_id, bearer_token(&headers).to_owned())
        .map_err(response_problem)?;
    state
        .coordination()
        .map_err(response_problem)?
        .validate_write_authority(
            CoordinationScope {
                workspace_id: &workspace_id,
                auth: &auth,
            },
            &resource_id,
            owner_token,
            fencing_token,
        )
        .await
        .map_err(response_problem)?;
    validate_blob_wire_format(
        &workspace_id,
        &resource_id,
        &query.key,
        &body,
        &correlation(&headers),
    )?;
    let uploaded_object =
        ManifestObject::new(query.key.clone(), &body).map_err(response_problem)?;
    if uploaded_object.checksum != expected_checksum {
        return Err(response_problem(Problem::new(
            "manifest_object_checksum_mismatch",
            "Blob checksum does not match the uploaded bytes",
            409,
            "sync",
        ))
        .into());
    }
    let storage_key = payload_object_key(&resource_id, generation, &query.key, expected_checksum);
    let reservation_time_ms = request_now_ms(&headers)?;
    let actor_id = principal
        .account_id
        .as_ref()
        .map(AccountId::as_str)
        .unwrap_or("anonymous");
    state
        .reserve_sync_blob_upload_async(
            workspace_id.clone(),
            resource_id.clone(),
            storage_key.clone(),
            expected_checksum.to_owned(),
            body_size,
            reservation_time_ms,
            actor_id.to_owned(),
        )
        .await
        .map_err(response_problem)?;
    state
        .put_reserved_remote_chunk_async(
            workspace_id.clone(),
            resource_id.clone(),
            storage_key,
            expected_checksum.to_owned(),
            body_size,
            reservation_time_ms,
            body,
            body_admission,
        )
        .await
        .map_err(response_problem)?;
    let receipt = json!({
        "resourceId": resource_id,
        "key": query.key,
        "generation": generation,
        "checksum": expected_checksum,
        "size": body_size
    });
    mutate::<serde_json::Value, _>(
        &state,
        &headers,
        &principal,
        &grants,
        workspace_id.as_str(),
        "profile.blob.put",
        request_input,
        ResourceScope::resource(workspace_id.clone(), resource_id.clone()),
        resource_id.as_str(),
        || {
            state
                .remember_blob_idempotency(
                    idempotency_store_key,
                    BlobIdempotencyRecord {
                        request_hash,
                        receipt: receipt.clone(),
                    },
                )
                .map_err(response_problem)?;
            Ok(receipt.clone())
        },
    )
    .await
}

fn validate_blob_wire_format(
    workspace_id: &WorkspaceId,
    resource_id: &ResourceId,
    key: &str,
    bytes: &[u8],
    correlation_id: &str,
) -> Result<(), Response> {
    if key == buglogin_sync::manifest::RESOURCE_KEY_ENVELOPE_KEY {
        let envelope: ResourceKeyEnvelope = serde_json::from_slice(bytes).map_err(|_| {
            problem_response(
                "resource_key_envelope_invalid",
                "Resource key envelope is invalid",
                400,
                correlation_id.to_owned(),
            )
        })?;
        if envelope.schema_version != 1
            || &envelope.workspace_id != workspace_id
            || &envelope.resource_id != resource_id
        {
            return Err(problem_response(
                "resource_key_envelope_scope_mismatch",
                "Resource key envelope does not match the resource scope",
                409,
                correlation_id.to_owned(),
            ));
        }
        return Ok(());
    }

    let is_proxy_metadata = key.ends_with("/metadata.json")
        && (key.starts_with("proxy/") || key.starts_with(".buglogin/proxy-dependencies/"));
    if is_proxy_metadata {
        let metadata: ProxyRecordView = serde_json::from_slice(bytes).map_err(|_| {
            problem_response(
                "proxy_metadata_object_invalid",
                "Proxy metadata object is invalid",
                400,
                correlation_id.to_owned(),
            )
        })?;
        if &metadata.workspace_id != workspace_id {
            return Err(problem_response(
                "api_scope_mismatch",
                "Proxy metadata does not match the workspace",
                409,
                correlation_id.to_owned(),
            ));
        }
        return Ok(());
    }

    if key.starts_with(".buglogin/proxy-dependencies/")
        && key.ends_with("/credential-envelope.json")
    {
        let envelope: ProxyCredentialEnvelope = serde_json::from_slice(bytes).map_err(|_| {
            problem_response(
                "proxy_credential_envelope_invalid",
                "Proxy credential envelope is invalid",
                400,
                correlation_id.to_owned(),
            )
        })?;
        let key_scope = key
            .strip_prefix(".buglogin/proxy-dependencies/")
            .and_then(|value| value.strip_suffix("/credential-envelope.json"))
            .and_then(|value| value.split_once('/'));
        let key_scope_matches = key_scope.is_some_and(|(proxy_id, revision)| {
            proxy_id == envelope.proxy_id.as_str()
                && revision
                    .parse::<u64>()
                    .is_ok_and(|revision| revision == envelope.proxy_revision)
        });
        if envelope.schema_version != 1
            || &envelope.workspace_id != workspace_id
            || &envelope.resource_id != resource_id
            || envelope.proxy_revision == 0
            || envelope.encrypted_secret.version != 1
            || !key_scope_matches
        {
            return Err(problem_response(
                "proxy_credential_envelope_scope_mismatch",
                "Proxy credential envelope does not match its object key or resource scope",
                409,
                correlation_id.to_owned(),
            ));
        }
        return Ok(());
    }

    let chunk = AuthenticatedChunk::decode(bytes).ok();
    if chunk.is_none_or(|chunk| chunk.version != 1) {
        return Err(problem_response(
            "encrypted_payload_object_invalid",
            "Profile payload object is not a supported encrypted chunk",
            409,
            correlation_id.to_owned(),
        ));
    }
    Ok(())
}

fn payload_object_key(
    resource_id: &ResourceId,
    generation: u64,
    object_key: &str,
    checksum: &str,
) -> String {
    format!(
        "profiles/{}/{}/{}/{}",
        resource_id.as_str(),
        generation,
        checksum,
        object_key
    )
}

fn authorize_lease(
    state: &ApiState,
    principal: &Principal,
    grants: &[buglogin_domain::permissions::Grant],
    workspace_id: &WorkspaceId,
    resource_id: &ResourceId,
    capability_name: &str,
    action: Action,
) -> Result<(), Response> {
    crate::services::profile_sync::authorize_resource_access(
        state,
        principal,
        grants,
        workspace_id,
        resource_id,
        capability_name,
        action,
    )
    .map_err(|error| Response::from(response_problem(error)))
}

fn require_actor_header(principal: &Principal, headers: &HeaderMap) -> Result<(), Response> {
    let actor_id = required_header(headers, "x-buglogin-actor-id")?;
    if principal
        .account_id
        .as_ref()
        .is_some_and(|account_id| account_id.as_str() == actor_id)
    {
        return Ok(());
    }
    Err(problem_response(
        "actor_identity_mismatch",
        "Actor header does not match the authenticated account",
        403,
        correlation(headers),
    ))
}

fn require_idempotency(headers: &HeaderMap) -> Result<&str, Response> {
    headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
        .filter(|value| value.is_ascii() && (16..=128).contains(&value.len()))
        .ok_or_else(|| {
            problem_response(
                "idempotency_key_required",
                "Valid idempotency key required",
                400,
                correlation(headers),
            )
        })
}

pub(super) fn required_header<'a>(
    headers: &'a HeaderMap,
    name: &'static str,
) -> Result<&'a str, Response> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| bad_header(name))
}

pub(super) fn bad_header(name: &'static str) -> Response {
    response(
        problem(
            "required_header_missing",
            "Required header missing or invalid",
            400,
        )
        .with_field("header", name),
    )
}

fn bearer_token(headers: &HeaderMap) -> &str {
    headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or("")
}

async fn grant_resource(
    State(state): State<ApiState>,
    Path((workspace_id, resource_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(grant): Json<ResourceGrant>,
) -> Result<Response, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let workspace_id = WorkspaceId::from_str(&workspace_id).map_err(|_| bad_id("workspace_id"))?;
    let resource_id = ResourceId::from_str(&resource_id).map_err(|_| bad_id("resource_id"))?;
    if grant.workspace_id != workspace_id || grant.resource_id != resource_id {
        return Err(problem_response(
            "tenant_mismatch",
            "Grant resource scope does not match path",
            403,
            correlation(&headers),
        ));
    }
    let value = serde_json::to_value(&grant).map_err(internal)?;
    mutate(
        &state,
        &headers,
        &principal,
        &grants,
        workspace_id.as_str(),
        "resource.grant",
        value,
        ResourceScope::resource(workspace_id.clone(), resource_id.clone()),
        grant.id.as_str(),
        || {
            state
                .resources
                .write()
                .map_err(|_| service_unavailable("resource"))?
                .grant(&principal, &grants, grant.clone())
                .map_err(response_problem)?;
            if matches!(grant.action, Action::Read | Action::Execute) {
                HostedProfileService::mark_resource_key_rotation_if_encrypted(
                    &state,
                    &grant.workspace_id,
                    &grant.resource_id,
                )
                .map_err(response_problem)?;
            }
            Ok(grant.clone())
        },
    )
    .await
}

async fn list_resource_grants(
    State(state): State<ApiState>,
    Path(workspace_id): Path<String>,
    Query(query): Query<GrantListQuery>,
    headers: HeaderMap,
) -> Result<Json<Vec<ResourceGrant>>, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let workspace_id = WorkspaceId::from_str(&workspace_id).map_err(|_| bad_id("workspace_id"))?;
    let resource_id = query
        .resource_id
        .as_deref()
        .map(ResourceId::from_str)
        .transpose()
        .map_err(|_| bad_id("resource_id"))?;
    let member_id = query
        .member_id
        .as_deref()
        .map(MemberId::from_str)
        .transpose()
        .map_err(|_| bad_id("member_id"))?;
    let items = state
        .resources
        .read()
        .map_err(|_| service_unavailable("resource"))?
        .list_grants(
            &principal,
            &grants,
            &workspace_id,
            resource_id.as_ref(),
            member_id.as_ref(),
        )
        .map_err(response_problem)?;
    Ok(Json(items))
}

async fn list_resource_grants_page(
    State(state): State<ApiState>,
    Path(workspace_id): Path<String>,
    Query(query): Query<GrantListQuery>,
    headers: HeaderMap,
) -> Result<Json<CollectionPageView<ResourceGrant>>, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let workspace_id = WorkspaceId::from_str(&workspace_id).map_err(|_| bad_id("workspace_id"))?;
    let resource_id = query
        .resource_id
        .as_deref()
        .map(ResourceId::from_str)
        .transpose()
        .map_err(|_| bad_id("resource_id"))?;
    let member_id = query
        .member_id
        .as_deref()
        .map(MemberId::from_str)
        .transpose()
        .map_err(|_| bad_id("member_id"))?;
    let cursor = query
        .cursor
        .as_deref()
        .map(GrantId::from_str)
        .transpose()
        .map_err(|_| bad_id("cursor"))?;
    let limit = collection_page_limit(query.limit)?;
    let (items, next_cursor) = state
        .resources
        .read()
        .map_err(|_| service_unavailable("resource"))?
        .list_grants_page(
            &principal,
            &grants,
            &workspace_id,
            resource_id.as_ref(),
            member_id.as_ref(),
            cursor.as_ref(),
            limit,
        )
        .map_err(response_problem)?;
    Ok(Json(CollectionPageView {
        items,
        next_cursor: next_cursor.map(|id| id.as_str().to_owned()),
    }))
}

async fn grant_resource_collection(
    State(state): State<ApiState>,
    Path(workspace_id): Path<String>,
    headers: HeaderMap,
    Json(grant): Json<ResourceGrant>,
) -> Result<Response, Response> {
    let workspace_id = WorkspaceId::from_str(&workspace_id).map_err(|_| bad_id("workspace_id"))?;
    if grant.workspace_id != workspace_id {
        return Err(problem_response(
            "tenant_mismatch",
            "Grant workspace scope does not match path",
            403,
            correlation(&headers),
        ));
    }
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let value = serde_json::to_value(&grant).map_err(internal)?;
    mutate(
        &state,
        &headers,
        &principal,
        &grants,
        workspace_id.as_str(),
        "resource.grant",
        value,
        ResourceScope::resource(workspace_id.clone(), grant.resource_id.clone()),
        grant.id.as_str(),
        || {
            state
                .resources
                .write()
                .map_err(|_| service_unavailable("resource"))?
                .grant(&principal, &grants, grant.clone())
                .map_err(response_problem)?;
            if matches!(grant.action, Action::Read | Action::Execute) {
                HostedProfileService::mark_resource_key_rotation_if_encrypted(
                    &state,
                    &grant.workspace_id,
                    &grant.resource_id,
                )
                .map_err(response_problem)?;
            }
            Ok(grant.clone())
        },
    )
    .await
}

async fn revoke_resource_grant(
    State(state): State<ApiState>,
    Path(grant_id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let grant_id = GrantId::from_str(&grant_id).map_err(|_| bad_id("grant_id"))?;
    let existing = state
        .resources
        .read()
        .map_err(|_| service_unavailable("resource"))?
        .get_grant(&principal, &grants, &grant_id)
        .map_err(response_problem)?;
    let value = serde_json::json!({"grantId": grant_id});
    mutate(
        &state,
        &headers,
        &principal,
        &grants,
        existing.workspace_id.as_str(),
        "resource.grant.revoke",
        value,
        ResourceScope::resource(existing.workspace_id.clone(), existing.resource_id.clone()),
        grant_id.as_str(),
        || {
            state
                .resources
                .write()
                .map_err(|_| service_unavailable("resource"))?
                .revoke(&principal, &grants, &grant_id)
                .map_err(response_problem)?;
            if matches!(existing.action, Action::Read | Action::Execute) {
                HostedProfileService::mark_resource_key_rotation_if_encrypted(
                    &state,
                    &existing.workspace_id,
                    &existing.resource_id,
                )
                .map_err(response_problem)?;
            }
            Ok(serde_json::json!({"revoked": true, "grantId": grant_id}))
        },
    )
    .await
}

async fn revoke_workspace_resource_grant(
    State(state): State<ApiState>,
    Path((workspace_id, grant_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, Response> {
    let workspace_id = WorkspaceId::from_str(&workspace_id).map_err(|_| bad_id("workspace_id"))?;
    let grant_id = GrantId::from_str(&grant_id).map_err(|_| bad_id("grant_id"))?;
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let existing = state
        .resources
        .read()
        .map_err(|_| service_unavailable("resource"))?
        .get_grant(&principal, &grants, &grant_id)
        .map_err(response_problem)?;
    if existing.workspace_id != workspace_id {
        return Err(problem_response(
            "resource_forbidden",
            "Grant access denied",
            403,
            correlation(&headers),
        ));
    }
    let value = serde_json::json!({"workspaceId": workspace_id, "grantId": grant_id});
    mutate(
        &state,
        &headers,
        &principal,
        &grants,
        workspace_id.as_str(),
        "resource.grant.revoke",
        value,
        ResourceScope::resource(workspace_id.clone(), existing.resource_id.clone()),
        grant_id.as_str(),
        || {
            state
                .resources
                .write()
                .map_err(|_| service_unavailable("resource"))?
                .revoke(&principal, &grants, &grant_id)
                .map_err(response_problem)?;
            if matches!(existing.action, Action::Read | Action::Execute) {
                HostedProfileService::mark_resource_key_rotation_if_encrypted(
                    &state,
                    &existing.workspace_id,
                    &existing.resource_id,
                )
                .map_err(response_problem)?;
            }
            Ok(serde_json::json!({"revoked": true, "grantId": grant_id}))
        },
    )
    .await
}

async fn read_audit(
    State(state): State<ApiState>,
    Path(workspace_id): Path<String>,
    Query(query): Query<AuditEventPageQuery>,
    headers: HeaderMap,
) -> Result<Json<crate::services::audit::AuditEventPage>, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let workspace_id = WorkspaceId::from_str(&workspace_id).map_err(|_| bad_id("workspace_id"))?;
    let page = state
        .audit
        .read()
        .map_err(|_| service_unavailable("audit"))?
        .read_page(
            &principal,
            &grants,
            &ResourceScope::workspace(workspace_id),
            query.cursor.as_deref(),
            query.limit.unwrap_or(50),
        )
        .map_err(response_problem)?;
    Ok(Json(page))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AuditEventPageQuery {
    cursor: Option<String>,
    limit: Option<usize>,
}

#[expect(
    clippy::too_many_arguments,
    reason = "explicit fields preserve audited control mutation inputs"
)]
pub(super) async fn mutate<T, F>(
    state: &ApiState,
    headers: &HeaderMap,
    expected_principal: &Principal,
    expected_grants: &[buglogin_domain::permissions::Grant],
    workspace: &str,
    command: &str,
    input: serde_json::Value,
    scope: ResourceScope,
    resource_id: &str,
    operation: F,
) -> Result<Response, Response>
where
    T: Serialize,
    F: FnOnce() -> Result<T, ResponseProblem>,
{
    mutate_with_additional_puts(
        state,
        headers,
        expected_principal,
        expected_grants,
        workspace,
        command,
        input,
        scope,
        resource_id,
        || operation().map(|value| (value, Vec::new(), None)),
    )
    .await
}

#[derive(Clone)]
struct MutationReplayKeys {
    correlation: String,
    legacy_store_key: String,
    legacy_input_hash: String,
    legacy_key_is_valid: bool,
    store_key: String,
    input_hash: String,
}

fn mutation_replay_keys(
    headers: &HeaderMap,
    principal: &Principal,
    workspace: &str,
    command: &str,
    input: &serde_json::Value,
    scope: &ResourceScope,
    resource_id: &str,
) -> Result<MutationReplayKeys, Response> {
    let correlation = correlation(headers);
    let key = headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
        .filter(|value| value.is_ascii() && (16..=128).contains(&value.len()))
        .ok_or_else(|| {
            problem_response(
                "idempotency_key_required",
                "Valid idempotency key required",
                400,
                correlation.clone(),
            )
        })?;
    let actor = principal
        .account_id
        .as_ref()
        .map(AccountId::as_str)
        .unwrap_or("anonymous");
    let legacy_store_key = format!("{actor}:{workspace}:{command}:{key}");
    let store_key = format!(
        "http-command-{:x}",
        Sha256::digest(legacy_store_key.as_bytes())
    );
    let hash_input = idempotency_input(command, input);
    let legacy_input_hash = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&hash_input).map_err(|_| {
            problem_response(
                "invalid_input",
                "Input is invalid",
                400,
                correlation.clone(),
            )
        })?)
    );
    let normalized_input = serde_json::json!({
        "scope": scope,
        "resourceId": resource_id,
        "input": hash_input,
    });
    let input_hash = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&normalized_input).map_err(|_| {
            problem_response(
                "invalid_input",
                "Input is invalid",
                400,
                correlation.clone(),
            )
        })?)
    );
    let legacy_key_is_valid = legacy_store_key.len() <= 128
        && legacy_store_key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'));
    Ok(MutationReplayKeys {
        correlation,
        legacy_store_key,
        legacy_input_hash,
        legacy_key_is_valid,
        store_key,
        input_hash,
    })
}

fn replay_mutation_receipt(
    state: &ApiState,
    keys: &MutationReplayKeys,
) -> Result<Option<Response>, Response> {
    if keys.legacy_key_is_valid
        && let Some(receipt) = state
            .replay_control_response(&keys.legacy_store_key, &keys.legacy_input_hash)
            .map_err(response_problem)?
    {
        return Ok(Some(
            (
                StatusCode::from_u16(receipt.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                Json(receipt.body),
            )
                .into_response(),
        ));
    }
    state
        .replay_control_response(&keys.store_key, &keys.input_hash)
        .map_err(|error| Response::from(response_problem(error)))
        .map(|receipt| {
            receipt.map(|receipt| {
                (
                    StatusCode::from_u16(receipt.status)
                        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                    Json(receipt.body),
                )
                    .into_response()
            })
        })
}

#[expect(
    clippy::too_many_arguments,
    reason = "explicit fields preserve mutation replay authorization boundaries"
)]
async fn replay_authorized_mutation_receipt(
    state: &ApiState,
    headers: &HeaderMap,
    expected_principal: &Principal,
    expected_grants: &[buglogin_domain::permissions::Grant],
    keys: &MutationReplayKeys,
    workspace_id: &WorkspaceId,
    resource_id: &ResourceId,
    capability: &str,
    action: Action,
) -> Result<Option<Response>, Response> {
    let _snapshot = std::sync::Arc::clone(&state.control_snapshot_gate)
        .write_owned()
        .await;
    let state = state.clone();
    let headers = headers.clone();
    let expected_principal = expected_principal.clone();
    let expected_grants = expected_grants.to_vec();
    let keys = keys.clone();
    let workspace_id = workspace_id.clone();
    let resource_id = resource_id.clone();
    let capability = capability.to_owned();
    let join_correlation = keys.correlation.clone();
    tokio::task::spawn_blocking(move || {
        let _snapshot = _snapshot;
        let _mutation = state.control_mutation.lock().map_err(|_| {
            problem_response(
                "control_unavailable",
                "Control state unavailable",
                503,
                keys.correlation.clone(),
            )
        })?;
        state
            .refresh_control_aggregate_locked()
            .map_err(response_problem)?;
        let principal = authenticate(&state, &headers)?;
        let grants = state.grants().map_err(response_problem)?;
        if principal != expected_principal || !same_grant_snapshot(&expected_grants, &grants) {
            return Err(response(
                problem(
                    "authorization_state_changed",
                    "Authorization changed; retry the request",
                    409,
                )
                .with_instance(keys.correlation.clone())
                .retryable(true),
            ));
        }
        authorize_lease(
            &state,
            &principal,
            &grants,
            &workspace_id,
            &resource_id,
            &capability,
            action,
        )?;
        replay_mutation_receipt(&state, &keys)
    })
    .await
    .unwrap_or_else(|_| {
        Err(problem_response(
            "control_unavailable",
            "Control state worker unavailable",
            503,
            join_correlation,
        ))
    })
}

/// Run a specialized control-plane mutation while its authenticated principal
/// and ACL snapshot match the refreshed state under this process's mutation
/// gate. The callback receives the aggregate revision and must include it as a
/// durable store precondition to fence writes from other API instances.
pub(super) async fn with_authorization_snapshot<T, F>(
    state: &ApiState,
    headers: &HeaderMap,
    expected_principal: &Principal,
    expected_grants: &[buglogin_domain::permissions::Grant],
    operation: F,
) -> Result<T, Response>
where
    T: Send + 'static,
    F: FnOnce(&Principal, &[buglogin_domain::permissions::Grant], u64) -> Result<T, Response>
        + Send
        + 'static,
{
    let _snapshot = std::sync::Arc::clone(&state.control_snapshot_gate)
        .write_owned()
        .await;
    let state = state.clone();
    let headers = headers.clone();
    let expected_principal = expected_principal.clone();
    let expected_grants = expected_grants.to_vec();
    let join_correlation = correlation(&headers);
    tokio::task::spawn_blocking(move || {
        let _snapshot = _snapshot;
        let _mutation = state.control_mutation.lock().map_err(|_| {
            problem_response(
                "control_unavailable",
                "Control state unavailable",
                503,
                correlation(&headers),
            )
        })?;
        state
            .refresh_control_aggregate_locked()
            .map_err(response_problem)?;
        let principal = authenticate(&state, &headers)?;
        let grants = state.grants().map_err(response_problem)?;
        if principal != expected_principal || !same_grant_snapshot(&expected_grants, &grants) {
            return Err(response(
                problem(
                    "authorization_state_changed",
                    "Authorization changed; retry the request",
                    409,
                )
                .with_instance(correlation(&headers))
                .retryable(true),
            ));
        }
        let revision = state.control_revision().map_err(response_problem)?;
        operation(&principal, &grants, revision)
    })
    .await
    .unwrap_or_else(|_| {
        Err(problem_response(
            "control_unavailable",
            "Control state worker unavailable",
            503,
            join_correlation,
        ))
    })
}

#[expect(
    clippy::too_many_arguments,
    reason = "explicit fields preserve audited control mutation inputs"
)]
pub(super) async fn mutate_with_additional_puts<T, F>(
    state: &ApiState,
    headers: &HeaderMap,
    expected_principal: &Principal,
    expected_grants: &[buglogin_domain::permissions::Grant],
    workspace: &str,
    command: &str,
    input: serde_json::Value,
    scope: ResourceScope,
    resource_id: &str,
    operation: F,
) -> Result<Response, Response>
where
    T: Serialize,
    F: FnOnce() -> Result<(T, Vec<ControlPut>, Option<Value>), ResponseProblem>,
{
    mutate_with_additional_puts_and_replay_check(
        state,
        headers,
        expected_principal,
        expected_grants,
        workspace,
        command,
        input,
        scope,
        resource_id,
        || Ok(()),
        operation,
    )
    .await
}

#[expect(
    clippy::too_many_arguments,
    reason = "explicit fields preserve audited control mutation inputs"
)]
pub(super) async fn mutate_with_additional_puts_and_replay_check<T, F, R>(
    state: &ApiState,
    headers: &HeaderMap,
    expected_principal: &Principal,
    expected_grants: &[buglogin_domain::permissions::Grant],
    workspace: &str,
    command: &str,
    input: serde_json::Value,
    scope: ResourceScope,
    resource_id: &str,
    replay_check: R,
    operation: F,
) -> Result<Response, Response>
where
    T: Serialize,
    F: FnOnce() -> Result<(T, Vec<ControlPut>, Option<Value>), ResponseProblem>,
    R: FnOnce() -> Result<(), ResponseProblem>,
{
    let _snapshot = std::sync::Arc::clone(&state.control_snapshot_gate)
        .write_owned()
        .await;
    with_control_blocking(|| {
        let _mutation = state.control_mutation.lock().map_err(|_| {
            problem_response(
                "control_unavailable",
                "Control state unavailable",
                503,
                correlation(headers),
            )
        })?;
        state
            .refresh_control_aggregate_locked()
            .map_err(response_problem)?;
        let principal = authenticate(state, headers)?;
        let grants = state.grants().map_err(response_problem)?;
        if &principal != expected_principal || !same_grant_snapshot(expected_grants, &grants) {
            return Err(response(
                problem(
                    "authorization_state_changed",
                    "Authorization changed; retry the request",
                    409,
                )
                .with_instance(correlation(headers))
                .retryable(true),
            ));
        }
        let reauthorize = matches!(
            command,
            "profile.proxy.assignment.set"
                | "profile.manifest.commit"
                | "account.create.scoped"
                | "organization.account.create"
                | "organization.member.status.update"
                | "member.invitation.create"
                | "member.invitation.revoke"
                | "member.activate"
                | "member.suspend"
                | "member.role.update"
        );
        if reauthorize {
            let metadata = crate::commands::CommandRegistry::get(command).ok_or_else(|| {
                problem_response(
                    "command_contract_missing",
                    "Mutation command is not registered",
                    500,
                    correlation(headers),
                )
            })?;
            let allowed = if matches!(
                command,
                "account.create.scoped" | "member.invitation.create"
            ) {
                let workspace_id = scope.workspace_id.as_ref().ok_or_else(|| {
                    problem_response(
                        "invalid_scope",
                        "Scoped account creation requires a workspace scope",
                        400,
                        correlation(headers),
                    )
                })?;
                let role_value = if command == "account.create.scoped" {
                    input.get("member").and_then(|member| member.get("role"))
                } else {
                    input.get("role")
                };
                let role: MemberRole = serde_json::from_value(
                    role_value.cloned().unwrap_or(Value::Null),
                )
                .map_err(|_| {
                    problem_response(
                        "invalid_input",
                        "Scoped account role is invalid",
                        400,
                        correlation(headers),
                    )
                })?;
                scoped_account_creation_authority(state, &principal, &grants, workspace_id, &role)
                    .is_ok()
            } else if matches!(
                command,
                "member.activate" | "member.suspend" | "member.role.update"
            ) {
                let workspace_id = scope.workspace_id.as_ref().ok_or_else(|| {
                    problem_response(
                        "invalid_scope",
                        "Member management requires a workspace scope",
                        400,
                        correlation(headers),
                    )
                })?;
                let (member_grants, organization_owner) = workspace_member_management_grants(
                    state,
                    &principal,
                    &grants,
                    workspace_id,
                    Action::Manage,
                    correlation(headers),
                )?;
                if organization_owner {
                    let member_id =
                        MemberId::from_str(resource_id).map_err(|_| bad_id("member_id"))?;
                    let target = state
                        .members
                        .read()
                        .map_err(|_| service_unavailable("member"))?
                        .get_in_workspace(&principal, &member_grants, &member_id, workspace_id)
                        .map_err(response_problem)?;
                    if target.role == MemberRole::Owner
                        || (command == "member.role.update"
                            && input
                                .get("role")
                                .cloned()
                                .and_then(|value| serde_json::from_value::<MemberRole>(value).ok())
                                == Some(MemberRole::Owner))
                    {
                        return Err(problem_response(
                            "role_escalation",
                            "Organization owner may manage only memberships below Owner",
                            403,
                            correlation(headers),
                        ));
                    }
                }
                true
            } else if matches!(
                command,
                "organization.account.create"
                    | "organization.member.status.update"
                    | "organization.member.workspace.assign"
            ) {
                let organization_id = scope.organization_id.as_ref().ok_or_else(|| {
                    problem_response(
                        "invalid_scope",
                        "Organization membership mutation requires an organization scope",
                        400,
                        correlation(headers),
                    )
                })?;
                state
                    .workspaces
                    .read()
                    .map_err(|_| service_unavailable("workspace"))?
                    .authorize_organization_management(
                        &principal,
                        &grants,
                        organization_id,
                        metadata.action,
                    )
                    .is_ok()
            } else if command == "profile.manifest.commit" {
                let workspace_id = scope
                    .workspace_id
                    .as_ref()
                    .ok_or_else(|| bad_id("workspace_id"))?;
                let resource_id = scope
                    .resource_id
                    .as_ref()
                    .ok_or_else(|| bad_id("resource_id"))?;
                crate::services::profile_sync::authorize_resource_access(
                    state,
                    &principal,
                    &grants,
                    workspace_id,
                    resource_id,
                    metadata.capability,
                    metadata.action,
                )
                .is_ok()
            } else if let Some(profile_id) = scope.profile_id.as_ref() {
                HostedProfileService::authorized(
                    state,
                    &principal,
                    &grants,
                    profile_id,
                    metadata.action,
                    metadata.capability,
                )
                .map_err(response_problem)?
            } else {
                let capability = Capability::new(metadata.capability).map_err(|_| {
                    problem_response(
                        "invalid_capability",
                        "Mutation capability is invalid",
                        500,
                        correlation(headers),
                    )
                })?;
                is_allowed(&principal, &capability, metadata.action, &scope, &grants)
            };
            if !allowed {
                return Err(problem_response(
                    "forbidden",
                    "Permission denied",
                    403,
                    correlation(headers),
                ));
            }
            if command == "profile.proxy.assignment.set" {
                let input: crate::services::profile_proxy_assignment::ProfileProxyAssignmentSetInput =
                serde_json::from_value(input.clone()).map_err(|_| {
                    problem_response(
                        "invalid_input",
                        "Profile proxy assignment is invalid",
                        400,
                        correlation(headers),
                    )
                })?;
                if let Some(proxy_id) = input.proxy_id.as_ref() {
                    state
                        .proxies
                        .read()
                        .map_err(|_| service_unavailable("proxy"))?
                        .get(&input.workspace_id, proxy_id, &principal, &grants)
                        .map_err(proxy_response_problem)?;
                }
            } else if command == "profile.manifest.commit" {
                let manifest: SyncManifest =
                    serde_json::from_value(input.get("manifest").cloned().unwrap_or(Value::Null))
                        .map_err(|_| {
                        problem_response(
                            "invalid_input",
                            "Profile manifest is invalid",
                            400,
                            correlation(headers),
                        )
                    })?;
                for dependency in &manifest.dependencies {
                    let ManifestDependency::Proxy { proxy_id, .. } = dependency;
                    state
                        .proxies
                        .read()
                        .map_err(|_| service_unavailable("proxy"))?
                        .get(&manifest.workspace_id, proxy_id, &principal, &grants)
                        .map_err(proxy_response_problem)?;
                }
            }
        }
        let replay_keys = mutation_replay_keys(
            headers,
            &principal,
            workspace,
            command,
            &input,
            &scope,
            resource_id,
        )?;
        if let Some(response) = replay_mutation_receipt(state, &replay_keys)? {
            replay_check()?;
            return Ok(response);
        }
        let correlation = replay_keys.correlation.clone();
        let store_key = replay_keys.store_key.clone();
        let input_hash = replay_keys.input_hash.clone();
        // Validate the audit timestamp before executing the mutation. A clock
        // failure must not leave a committed control change without its audit row.
        let audit_time = checked_now(&correlation).map_err(response_problem)?;
        let before = state.control_aggregate().map_err(response_problem)?;
        let result = operation();
        let success_status = if matches!(
            command,
            "profile.create"
                | "account.create.scoped"
                | "organization.account.create"
                | "organization.member.workspace.assign"
                | "member.invitation.create"
        ) {
            StatusCode::CREATED
        } else {
            StatusCode::OK
        };
        let (status, body, replay_body, outcome, additional_puts) = match result {
            Ok((value, additional_puts, replay_body)) => (
                success_status,
                serde_json::to_value(value).map_err(|_| {
                    problem_response(
                        "serialization_failed",
                        "Response serialization failed",
                        500,
                        correlation.clone(),
                    )
                })?,
                replay_body,
                AuditOutcome::Succeeded,
                additional_puts,
            ),
            Err(error) => {
                state
                    .replace_control_aggregate(before.clone())
                    .map_err(response_problem)?;
                (
                    StatusCode::from_u16(error.0).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                    error.1,
                    None,
                    if error.0 == 403 {
                        AuditOutcome::Denied
                    } else {
                        AuditOutcome::Failed
                    },
                    Vec::new(),
                )
            }
        };
        if let Some(actor_id) = principal.account_id.clone() {
            let event_id = format!(
                "evt_{}",
                &Sha256::digest(store_key.as_bytes())
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>()[..24]
            );
            let event = AuditEvent::new(
                event_id,
                audit_time,
                actor_id,
                AuditResource {
                    resource_type: "http".into(),
                    resource_id: resource_id.into(),
                    scope: scope.clone(),
                },
                command,
                correlation.clone(),
                outcome,
                Default::default(),
            )
            .map_err(response_problem)?;
            state
                .audit
                .write()
                .map_err(|_| {
                    problem_response(
                        "audit_unavailable",
                        "Audit service unavailable",
                        503,
                        correlation.clone(),
                    )
                })?
                .append(event)
                .map_err(response_problem)?;
        }
        state
            .persist_control_aggregate_with_puts(
                command,
                store_key.clone(),
                principal
                    .account_id
                    .as_ref()
                    .map(AccountId::as_str)
                    .unwrap_or("anonymous")
                    .to_owned(),
                status.is_success(),
                &input_hash,
                serde_json::json!({
                    "correlationId": correlation,
                    "resourceId": resource_id,
                    "scope": scope,
                    "inputHash": input_hash,
                }),
                status.as_u16(),
                replay_body.unwrap_or_else(|| body.clone()),
                additional_puts,
            )
            .map_err(|error| {
                if state.reload_control_aggregate().is_err() {
                    let _ = state.replace_control_aggregate(before);
                }
                response_problem(error)
            })?;
        if status.is_success() {
            Ok((status, Json(body)).into_response())
        } else {
            Ok(response(super::canonical_problem(status.as_u16(), body)))
        }
    })
}

fn with_control_blocking<T>(operation: impl FnOnce() -> T) -> T {
    let multi_thread_runtime = tokio::runtime::Handle::try_current()
        .is_ok_and(|handle| handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread);
    if multi_thread_runtime {
        // ControlPersistence uses a bounded synchronous PostgreSQL worker. Mark
        // this whole mutation as blocking so Tokio can replace the occupied
        // runtime worker while its serialized database operations complete.
        tokio::task::block_in_place(operation)
    } else {
        // Current-thread runtimes are used by focused handler tests; they have
        // no worker pool to hand off to.
        operation()
    }
}

fn same_grant_snapshot(
    expected: &[buglogin_domain::permissions::Grant],
    actual: &[buglogin_domain::permissions::Grant],
) -> bool {
    if expected.len() != actual.len() {
        return false;
    }
    let mut expected = expected.to_vec();
    let mut actual = actual.to_vec();
    expected.sort_unstable_by(|left, right| left.id.as_str().cmp(right.id.as_str()));
    actual.sort_unstable_by(|left, right| left.id.as_str().cmp(right.id.as_str()));
    expected == actual
}

fn idempotency_input(command: &str, input: &serde_json::Value) -> serde_json::Value {
    if !matches!(
        command,
        "profile.create" | "profile.provision" | "organization.account.create"
    ) {
        return input.clone();
    }
    let mut canonical = input.clone();
    if command == "organization.account.create"
        && let Some(object) = canonical.as_object_mut()
    {
        object.remove("initialPassword");
    }
    let Some(profile) = canonical
        .get_mut("profile")
        .and_then(serde_json::Value::as_object_mut)
    else {
        return canonical;
    };
    if matches!(command, "profile.create" | "profile.provision") {
        profile.remove("createdAt");
        profile.remove("updatedAt");
    }
    canonical
}

#[derive(Debug)]
pub(super) struct ResponseProblem(u16, serde_json::Value);

impl From<ResponseProblem> for Response {
    fn from(error: ResponseProblem) -> Self {
        let retry_after = error.0 == 503
            && error
                .1
                .get("retryable")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
        let mut response = super::response(super::canonical_problem(error.0, error.1));
        if retry_after {
            response.headers_mut().insert(
                axum::http::header::RETRY_AFTER,
                axum::http::HeaderValue::from_static("2"),
            );
        }
        response
    }
}
pub(super) fn response_problem(error: buglogin_domain::errors::Problem) -> ResponseProblem {
    ResponseProblem(
        error.status,
        serde_json::to_value(error)
            .unwrap_or_else(|_| serde_json::json!({"code":"internal_error"})),
    )
}
fn internal(_: serde_json::Error) -> Response {
    problem_response(
        "serialization_failed",
        "Serialization failed",
        500,
        "unknown".into(),
    )
}
fn service_unavailable(service: &str) -> ResponseProblem {
    ResponseProblem(
        503,
        serde_json::json!({
            "code": format!("{service}_unavailable"),
            "title": format!("{service} service unavailable"),
        }),
    )
}
fn bad_id(field: &str) -> Response {
    problem_response("invalid_id", "Invalid identifier", 400, "unknown".into()).with_field(field)
}
fn correlation(headers: &HeaderMap) -> String {
    headers
        .get("x-correlation-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown")
        .to_owned()
}
fn problem_response(
    code: &'static str,
    title: &'static str,
    status: u16,
    correlation: String,
) -> Response {
    response(problem(code, title, status).with_instance(correlation))
}
trait ResponseExt {
    fn with_field(self, field: &str) -> Self;
}
impl ResponseExt for Response {
    fn with_field(self, _: &str) -> Self {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{HeaderMap, Request},
    };
    use buglogin_browser::profile::ProfileService;
    use buglogin_domain::{
        account::{Account, IdentityVerification},
        entities::{ProfileState, Runtime, SyncMode},
        grant::ResourceGrant,
        member::{Member, MemberRole, MemberStatus},
        permissions::{Effect, Grant},
        profile::{ProfileArchiveState, ProfileResource},
        resource::{ResourceKind, ResourceState},
    };
    use buglogin_storage::control::{
        ControlAuditEntry, ControlRecordKey, ControlRecordStore, ExecuteControlCommand,
        HttpResponse, MemoryControlRecordStore, PutControlRecord,
    };
    use buglogin_sync::manifest::{
        ManifestObject, ManifestObjectReference, ManifestPayloadEncryption, ManifestTombstone,
        PayloadEncryptionAlgorithm,
    };
    use tower::ServiceExt;

    #[tokio::test]
    async fn organization_account_creation_membership_listing_and_suspension_are_atomic() {
        let store = MemoryControlRecordStore::default();
        let state =
            ApiState::hosted_with_control_store(ProfileService::disabled(), store.clone()).unwrap();
        let owner = state
            .bootstrap_first_owner(crate::http::FirstOwnerBootstrapInput {
                account_id: Some("org_account_owner".into()),
                username: "org-account-owner".into(),
                email: "org-account-owner@example.test".into(),
                operator_attested_email: true,
                password: zeroize::Zeroizing::new("organization account owner password".into()),
                organization_name: "Org Account Test".into(),
                workspace_name: "Default Workspace".into(),
                max_workspaces: 2,
                max_profiles: 20,
                max_members: 10,
                max_storage_bytes: 1_000_000,
            })
            .unwrap();
        let removed_platform_admin = state
            .platform_admin_accounts
            .write()
            .unwrap()
            .remove(&owner.account_id);
        assert!(removed_platform_admin);
        state
            .persist_control_aggregate(
                "test.organization.owner.demote_platform_admin",
                "organization-owner-platform-admin-demotion-test-01".into(),
                owner.account_id.as_str().to_owned(),
                true,
                &"a".repeat(64),
                serde_json::json!({"testOnly": true, "platformAdmin": false}),
                200,
                serde_json::json!({"platformAdmin": false}),
            )
            .unwrap();
        state.reload_control_aggregate().unwrap();
        let token = state.issue_token(owner.account_id.as_str()).unwrap();
        let app = super::super::router(state.clone());

        let created = app
            .clone()
            .oneshot(
                Request::post(format!(
                    "/v1/organizations/{}/accounts",
                    owner.organization_id.as_str()
                ))
                .header("authorization", format!("Bearer {token}"))
                .header("x-correlation-id", "organization-account-create")
                .header("content-type", "application/json")
                .header("idempotency-key", "org-account-create-test-1")
                .body(Body::from(
                    serde_json::json!({
                        "organizationId": owner.organization_id,
                        "accountId": "org_account_member",
                        "username": "org-account-member",
                        "email": "org-account-member@example.test",
                        "initialPassword": "organization member password"
                    })
                    .to_string(),
                ))
                .unwrap(),
            )
            .await
            .unwrap();
        let created_status = created.status();
        let body = axum::body::to_bytes(created.into_body(), 2 * 1024 * 1024)
            .await
            .unwrap();
        assert_eq!(
            created_status,
            StatusCode::CREATED,
            "organization account creation failed: {}",
            String::from_utf8_lossy(&body)
        );
        let created: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(created["loginEnabled"], true);
        assert_eq!(created["organizationMember"]["status"], "active");
        assert!(created.get("member").is_none());
        assert_eq!(
            state
                .members
                .read()
                .unwrap()
                .all_members()
                .iter()
                .filter(|member| member.account_id.as_str() == "org_account_member")
                .count(),
            0
        );

        let listed = app
            .clone()
            .oneshot(
                Request::get(format!(
                    "/v1/organizations/{}/members",
                    owner.organization_id.as_str()
                ))
                .header("authorization", format!("Bearer {token}"))
                .header("x-correlation-id", "organization-members-list")
                .body(Body::empty())
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(listed.status(), StatusCode::OK);
        let body = axum::body::to_bytes(listed.into_body(), 2 * 1024 * 1024)
            .await
            .unwrap();
        let listed: Value = serde_json::from_slice(&body).unwrap();
        let membership = listed["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|member| member["accountId"] == "org_account_member")
            .unwrap();
        assert_eq!(membership["status"], "active");
        assert_eq!(membership["username"], "org-account-member");
        assert_eq!(membership["email"], "org-account-member@example.test");

        let assigned = app
            .clone()
            .oneshot(
                Request::post(format!(
                    "/v1/organizations/{}/workspaces/{}/members",
                    owner.organization_id.as_str(),
                    owner.workspace_id.as_str(),
                ))
                .header("authorization", format!("Bearer {token}"))
                .header("x-correlation-id", "organization-member-workspace-assign")
                .header("content-type", "application/json")
                .header("idempotency-key", "org-member-workspace-assign-1")
                .body(Body::from(
                    serde_json::json!({
                        "organizationId": owner.organization_id,
                        "workspaceId": owner.workspace_id,
                        "organizationMemberId": membership["id"],
                        "organizationMemberRevision": membership["revision"],
                        "role": "admin"
                    })
                    .to_string(),
                ))
                .unwrap(),
            )
            .await
            .unwrap();
        let assigned_status = assigned.status();
        let body = axum::body::to_bytes(assigned.into_body(), 2 * 1024 * 1024)
            .await
            .unwrap();
        assert_eq!(
            assigned_status,
            StatusCode::CREATED,
            "organization workspace assignment failed: {}",
            String::from_utf8_lossy(&body)
        );
        let assigned: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(assigned["accountId"], "org_account_member");
        assert_eq!(assigned["workspaceId"], owner.workspace_id.as_str());
        assert_eq!(assigned["role"], "admin");
        assert_eq!(
            state
                .members
                .read()
                .unwrap()
                .all_members()
                .iter()
                .filter(|member| member.account_id.as_str() == "org_account_member")
                .count(),
            1
        );

        let workspace_admin_token = state.issue_token("org_account_member").unwrap();
        let workspace_admin_assignment = app
            .clone()
            .oneshot(
                Request::post(format!(
                    "/v1/organizations/{}/workspaces/{}/members",
                    owner.organization_id.as_str(),
                    owner.workspace_id.as_str(),
                ))
                .header("authorization", format!("Bearer {workspace_admin_token}"))
                .header("x-correlation-id", "workspace-admin-org-assignment-denied")
                .header("content-type", "application/json")
                .header("idempotency-key", "workspace-admin-org-assignment-denied")
                .body(Body::from(
                    serde_json::json!({
                        "organizationId": owner.organization_id,
                        "workspaceId": owner.workspace_id,
                        "organizationMemberId": membership["id"],
                        "organizationMemberRevision": membership["revision"],
                        "role": "member"
                    })
                    .to_string(),
                ))
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(workspace_admin_assignment.status(), StatusCode::FORBIDDEN);

        // Organization-owner authority must remain visible even when the
        // owner also has a direct membership grant for this workspace.
        state.grants.write().unwrap().push(Grant {
            id: buglogin_domain::ids::GrantId::new("organization-owner-direct-member-manage")
                .unwrap(),
            account_id: Some(owner.account_id.clone()),
            member_id: None,
            capability: Capability::new("CAP-MEM-001").unwrap(),
            action: Action::Manage,
            scope: ResourceScope::workspace(owner.workspace_id.clone()),
            effect: Effect::Allow,
        });
        state.grants.write().unwrap().push(Grant {
            id: buglogin_domain::ids::GrantId::new("organization-owner-direct-member-read")
                .unwrap(),
            account_id: Some(owner.account_id.clone()),
            member_id: None,
            capability: Capability::new("CAP-MEM-001").unwrap(),
            action: Action::Read,
            scope: ResourceScope::workspace(owner.workspace_id.clone()),
            effect: Effect::Allow,
        });
        let mut owner_headers = HeaderMap::new();
        owner_headers.insert("authorization", format!("Bearer {token}").parse().unwrap());
        owner_headers.insert(
            "x-correlation-id",
            "organization-owner-direct-grant-ceiling".parse().unwrap(),
        );
        let owner_principal = authenticate(&state, &owner_headers).unwrap();
        assert!(
            !owner_principal
                .roles
                .contains(&buglogin_domain::permissions::Role::PlatformAdmin)
        );
        let grant_snapshot = grants(&state).unwrap();
        let (_, is_organization_owner) = workspace_member_management_grants(
            &state,
            &owner_principal,
            &grant_snapshot,
            &owner.workspace_id,
            Action::Manage,
            "organization-owner-direct-grant-ceiling".into(),
        )
        .unwrap();
        assert!(is_organization_owner);

        let owner_role_update = app
            .clone()
            .oneshot(
                Request::put(format!(
                    "/v1/workspaces/{}/members/{}/role",
                    owner.workspace_id.as_str(),
                    assigned["id"].as_str().unwrap(),
                ))
                .header("authorization", format!("Bearer {token}"))
                .header("x-correlation-id", "organization-owner-role-update-denied")
                .header("content-type", "application/json")
                .header("idempotency-key", "org-owner-member-role-update-denied")
                .body(Body::from(r#"{"role":"owner"}"#))
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(owner_role_update.status(), StatusCode::FORBIDDEN);

        let owner_role_downgrade = app
            .clone()
            .oneshot(
                Request::put(format!(
                    "/v1/workspaces/{}/members/{}/role",
                    owner.workspace_id.as_str(),
                    assigned["id"].as_str().unwrap(),
                ))
                .header("authorization", format!("Bearer {token}"))
                .header("x-correlation-id", "organization-owner-role-downgrade")
                .header("content-type", "application/json")
                .header("idempotency-key", "org-owner-member-role-downgrade")
                .body(Body::from(r#"{"role":"member"}"#))
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(owner_role_downgrade.status(), StatusCode::OK);

        let workspace_member_token = state.issue_token("org_account_member").unwrap();

        let workspace_member_suspended = app
            .clone()
            .oneshot(
                Request::put(format!(
                    "/v1/workspaces/{}/members/{}/suspend",
                    owner.workspace_id.as_str(),
                    assigned["id"].as_str().unwrap(),
                ))
                .header("authorization", format!("Bearer {token}"))
                .header("x-correlation-id", "organization-owner-member-suspend")
                .header("idempotency-key", "org-owner-member-suspend")
                .body(Body::empty())
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(workspace_member_suspended.status(), StatusCode::OK);

        let self_restore_denied = app
            .clone()
            .oneshot(
                Request::put(format!(
                    "/v1/workspaces/{}/members/{}/activate",
                    owner.workspace_id.as_str(),
                    assigned["id"].as_str().unwrap(),
                ))
                .header("authorization", format!("Bearer {workspace_member_token}"))
                .header("x-correlation-id", "suspended-member-self-restore-denied")
                .header("idempotency-key", "suspended-member-self-restore")
                .body(Body::empty())
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(self_restore_denied.status(), StatusCode::FORBIDDEN);

        let workspace_member_restored = app
            .clone()
            .oneshot(
                Request::put(format!(
                    "/v1/workspaces/{}/members/{}/activate",
                    owner.workspace_id.as_str(),
                    assigned["id"].as_str().unwrap(),
                ))
                .header("authorization", format!("Bearer {token}"))
                .header("x-correlation-id", "organization-owner-member-restore")
                .header("idempotency-key", "org-owner-member-restore")
                .body(Body::empty())
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(workspace_member_restored.status(), StatusCode::OK);

        let owner_role_assignment = app
            .clone()
            .oneshot(
                Request::post(format!(
                    "/v1/organizations/{}/workspaces/{}/members",
                    owner.organization_id.as_str(),
                    owner.workspace_id.as_str(),
                ))
                .header("authorization", format!("Bearer {token}"))
                .header("x-correlation-id", "organization-member-owner-role-denied")
                .header("content-type", "application/json")
                .header("idempotency-key", "org-member-workspace-owner-role-denied")
                .body(Body::from(
                    serde_json::json!({
                        "organizationId": owner.organization_id,
                        "workspaceId": owner.workspace_id,
                        "organizationMemberId": membership["id"],
                        "organizationMemberRevision": membership["revision"],
                        "role": "owner"
                    })
                    .to_string(),
                ))
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(owner_role_assignment.status(), StatusCode::FORBIDDEN);

        let cross_organization_workspace_assignment = app
            .clone()
            .oneshot(
                Request::post(format!(
                    "/v1/organizations/{}/workspaces/ws_other_organization/members",
                    owner.organization_id.as_str(),
                ))
                .header("authorization", format!("Bearer {token}"))
                .header("x-correlation-id", "organization-member-cross-scope-denied")
                .header("content-type", "application/json")
                .header("idempotency-key", "org-member-workspace-cross-scope-denied")
                .body(Body::from(
                    serde_json::json!({
                        "organizationId": owner.organization_id,
                        "workspaceId": "ws_other_organization",
                        "organizationMemberId": membership["id"],
                        "organizationMemberRevision": membership["revision"],
                        "role": "member"
                    })
                    .to_string(),
                ))
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            cross_organization_workspace_assignment.status(),
            StatusCode::FORBIDDEN
        );

        let suspended = app
            .clone()
            .oneshot(
                Request::patch(format!(
                    "/v1/organizations/{}/members/org_account_member",
                    owner.organization_id.as_str()
                ))
                .header("authorization", format!("Bearer {token}"))
                .header("x-correlation-id", "organization-member-suspend")
                .header("content-type", "application/json")
                .header("idempotency-key", "org-account-suspend-test-1")
                .body(Body::from(
                    serde_json::json!({
                        "organizationId": owner.organization_id,
                        "accountId": "org_account_member",
                        "status": "suspended",
                        "expectedRevision": 1
                    })
                    .to_string(),
                ))
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(suspended.status(), StatusCode::OK);
        let body = axum::body::to_bytes(suspended.into_body(), 2 * 1024 * 1024)
            .await
            .unwrap();
        let suspended: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(suspended["status"], "suspended");
        assert_eq!(suspended["revision"], 2);

        let suspended_assignment = app
            .clone()
            .oneshot(
                Request::post(format!(
                    "/v1/organizations/{}/workspaces/{}/members",
                    owner.organization_id.as_str(),
                    owner.workspace_id.as_str(),
                ))
                .header("authorization", format!("Bearer {token}"))
                .header(
                    "x-correlation-id",
                    "suspended-organization-member-assignment",
                )
                .header("content-type", "application/json")
                .header("idempotency-key", "org-member-workspace-assign-suspended")
                .body(Body::from(
                    serde_json::json!({
                        "organizationId": owner.organization_id,
                        "workspaceId": owner.workspace_id,
                        "organizationMemberId": membership["id"],
                        "organizationMemberRevision": 2,
                        "role": "member"
                    })
                    .to_string(),
                ))
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(suspended_assignment.status(), StatusCode::CONFLICT);

        let child_suspended = app
            .clone()
            .oneshot(
                Request::put(format!(
                    "/v1/workspaces/{}/members/{}/suspend",
                    owner.workspace_id.as_str(),
                    assigned["id"].as_str().unwrap(),
                ))
                .header("authorization", format!("Bearer {token}"))
                .header(
                    "x-correlation-id",
                    "workspace-member-suspend-parent-suspended",
                )
                .header(
                    "idempotency-key",
                    "workspace-member-suspend-parent-suspended",
                )
                .body(Body::empty())
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(child_suspended.status(), StatusCode::OK);

        let child_restore_parent_suspended = app
            .clone()
            .oneshot(
                Request::put(format!(
                    "/v1/workspaces/{}/members/{}/activate",
                    owner.workspace_id.as_str(),
                    assigned["id"].as_str().unwrap(),
                ))
                .header("authorization", format!("Bearer {token}"))
                .header(
                    "x-correlation-id",
                    "workspace-member-restore-parent-suspended",
                )
                .header(
                    "idempotency-key",
                    "workspace-member-restore-parent-suspended",
                )
                .body(Body::empty())
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            child_restore_parent_suspended.status(),
            StatusCode::CONFLICT
        );
        let body =
            axum::body::to_bytes(child_restore_parent_suspended.into_body(), 2 * 1024 * 1024)
                .await
                .unwrap();
        let problem: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(problem["code"], "organization_member_inactive");

        let persistence = crate::services::control_persistence::ControlPersistence::new(store);
        let (persisted, _) = persistence
            .load_versioned::<super::super::ControlAggregate>(
                super::super::CONTROL_TENANT,
                super::super::CONTROL_STATE_TYPE,
                super::super::CONTROL_STATE_ID,
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            persisted
                .organization_members
                .get(
                    &owner.organization_id,
                    &AccountId::new("org_account_member").unwrap()
                )
                .unwrap()
                .status,
            OrganizationMemberStatus::Suspended
        );
        assert!(
            persistence
                .load::<Account>("control", "account", "org_account_member")
                .unwrap()
                .is_some()
        );

        let organization_restored = app
            .clone()
            .oneshot(
                Request::patch(format!(
                    "/v1/organizations/{}/members/org_account_member",
                    owner.organization_id.as_str()
                ))
                .header("authorization", format!("Bearer {token}"))
                .header("x-correlation-id", "organization-member-restore")
                .header("content-type", "application/json")
                .header("idempotency-key", "org-account-restore-test-1")
                .body(Body::from(
                    serde_json::json!({
                        "organizationId": owner.organization_id,
                        "accountId": "org_account_member",
                        "status": "active",
                        "expectedRevision": 2
                    })
                    .to_string(),
                ))
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(organization_restored.status(), StatusCode::OK);

        let restored_child = app
            .oneshot(
                Request::put(format!(
                    "/v1/workspaces/{}/members/{}/activate",
                    owner.workspace_id.as_str(),
                    assigned["id"].as_str().unwrap(),
                ))
                .header("authorization", format!("Bearer {token}"))
                .header("x-correlation-id", "workspace-member-restore-parent-active")
                .header("idempotency-key", "workspace-member-restore-parent-active")
                .body(Body::empty())
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(restored_child.status(), StatusCode::OK);
    }

    fn expiring_dispatch(
        state: &ApiState,
        device_id: &str,
        key: &str,
    ) -> crate::services::runtime_dispatch::RuntimeDispatchJob {
        let job = state
            .runtime_dispatch
            .enqueue(
                crate::services::runtime_dispatch::EnqueueRuntimeDispatch {
                    command_id: "profile.launch".into(),
                    workspace_id: "ws_dispatch_expiry".into(),
                    device_id: device_id.into(),
                    profile_id: "prf_dispatch_expiry".into(),
                    requested_by: "acct_dispatch_expiry".into(),
                    idempotency_key: key.into(),
                    now: 100,
                    expires_at: 200,
                    input: serde_json::json!({"headless": false}),
                },
                None,
            )
            .unwrap();
        super::record_dispatch_operation(state, &job).unwrap();
        job
    }

    fn operation_status(
        state: &ApiState,
        operation_id: &str,
    ) -> (crate::services::operations::OperationStatus, Option<String>) {
        let operations = state.operations.read().unwrap();
        let operation = operations.find(operation_id).unwrap();
        (
            operation.status,
            operation.error.as_ref().map(|error| error.code.clone()),
        )
    }

    #[test]
    fn polling_an_overdue_dispatch_operation_reports_it_expired() {
        let state = ApiState::hosted_with_control_store(
            ProfileService::disabled(),
            MemoryControlRecordStore::default(),
        )
        .unwrap();
        let job = expiring_dispatch(&state, "device_dispatch_expiry", "dispatch-expiry-key-1");

        super::expire_overdue_dispatch(&state, &job.operation_id, 199).unwrap();
        assert_eq!(
            operation_status(&state, &job.operation_id).0,
            crate::services::operations::OperationStatus::Running
        );

        super::expire_overdue_dispatch(&state, &job.operation_id, 200).unwrap();
        assert_eq!(
            operation_status(&state, &job.operation_id),
            (
                crate::services::operations::OperationStatus::Failed,
                Some("runtime_job_expired".into())
            )
        );
    }

    #[test]
    fn device_inbox_poll_expires_its_overdue_dispatches() {
        let state = ApiState::hosted_with_control_store(
            ProfileService::disabled(),
            MemoryControlRecordStore::default(),
        )
        .unwrap();
        let mine = expiring_dispatch(&state, "device_dispatch_expiry", "dispatch-expiry-key-2");
        let other = expiring_dispatch(&state, "device_dispatch_other", "dispatch-expiry-key-3");

        super::expire_overdue_device_dispatches(
            &state,
            &["ws_dispatch_expiry".into()],
            "device_dispatch_expiry",
            200,
        )
        .unwrap();

        assert_eq!(
            operation_status(&state, &mine.operation_id).1.as_deref(),
            Some("runtime_job_expired")
        );
        assert_eq!(
            operation_status(&state, &other.operation_id).0,
            crate::services::operations::OperationStatus::Running
        );
    }

    #[test]
    fn resource_lease_authority_tracks_key_rotation_state() {
        assert_eq!(
            super::resource_lease_authority(true),
            ("profile.update", Action::Update)
        );
        assert_eq!(
            super::resource_lease_authority(false),
            ("profile.launch", Action::Execute)
        );
    }

    #[tokio::test]
    async fn lease_reconciliation_persists_missing_hosted_resource_registration() {
        let store = MemoryControlRecordStore::default();
        let state =
            ApiState::hosted_with_control_store(ProfileService::disabled(), store.clone()).unwrap();
        let owner = state
            .bootstrap_first_owner(crate::http::FirstOwnerBootstrapInput {
                account_id: Some("acct_resource_repair_owner".into()),
                username: "resource-repair-owner".into(),
                email: "resource-repair-owner@example.com".into(),
                operator_attested_email: true,
                password: zeroize::Zeroizing::new("owner-test-password-123".into()),
                organization_name: "Resource repair org".into(),
                workspace_name: "Resource repair workspace".into(),
                max_workspaces: 2,
                max_profiles: 10,
                max_members: 10,
                max_storage_bytes: 1024 * 1024,
            })
            .unwrap();
        let workspace_id = owner.workspace_id.clone();
        let profile_id = ProfileId::new("prf_resource_repair_http").unwrap();
        let resource_id = ResourceId::new("res_resource_repair_http").unwrap();
        let state = state.with_profile_resource(ProfileResource {
            profile_id: profile_id.clone(),
            resource_id: resource_id.clone(),
            workspace_id: workspace_id.clone(),
            archive_state: ProfileArchiveState::Active,
        });
        state
            .create_profile_record(buglogin_domain::entities::Profile {
                id: profile_id,
                name: "Recoverable profile".into(),
                runtime: Runtime::Wayfern,
                runtime_version: Some("1.0.0".into()),
                runtime_config: None,
                state: ProfileState::Ready,
                archived: false,
                fingerprint: None,
                note: None,
                proxy_id: None,
                vpn_id: None,
                group_ids: Vec::new(),
                tags: Vec::new(),
                extension_group_id: None,
                sync_mode: None,
                created_at: "2026-10-24T00:00:00Z".into(),
                updated_at: "2026-10-24T00:00:00Z".into(),
            })
            .unwrap();
        state
            .persist_control_aggregate(
                "test.resource.repair.seed",
                "resource-repair-seed-key-01".into(),
                owner.account_id.as_str().to_owned(),
                true,
                &"a".repeat(64),
                serde_json::json!({"seeded": true}),
                200,
                serde_json::json!({"seeded": true}),
            )
            .unwrap();

        assert!(
            !state
                .resources
                .read()
                .unwrap()
                .exists_in_workspace(&resource_id, &workspace_id)
        );
        let token = state.issue_token(owner.account_id.as_str()).unwrap();
        let headers = HeaderMap::from_iter([
            (
                axum::http::header::AUTHORIZATION,
                format!("Bearer {token}").parse().unwrap(),
            ),
            (
                axum::http::header::HeaderName::from_static("idempotency-key"),
                "resource-repair-lease-key-01".parse().unwrap(),
            ),
            (
                axum::http::header::HeaderName::from_static("x-correlation-id"),
                "resource-repair-lease-test".parse().unwrap(),
            ),
        ]);
        let principal = super::authenticate(&state, &headers).unwrap();
        let grants = super::grants(&state).unwrap();
        let result = super::mutate::<bool, _>(
            &state,
            &headers,
            &principal,
            &grants,
            workspace_id.as_str(),
            "resource.registration.reconcile",
            serde_json::json!({
                "workspaceId": workspace_id,
                "resourceId": resource_id,
            }),
            ResourceScope::resource(workspace_id.clone(), resource_id.clone()),
            resource_id.as_str(),
            || {
                crate::services::profile_sync::reconcile_managed_profile_resource_registration(
                    &state,
                    &workspace_id,
                    &resource_id,
                )
                .map_err(response_problem)
            },
        )
        .await;
        assert!(result.is_ok());

        let reopened =
            ApiState::hosted_with_control_store(ProfileService::disabled(), store).unwrap();
        assert!(
            reopened
                .resources
                .read()
                .unwrap()
                .exists_in_workspace(&resource_id, &workspace_id)
        );
    }

    #[test]
    fn control_http_body_dtos_reject_unknown_fields() {
        assert!(
            serde_json::from_value::<super::AccountStatusInput>(serde_json::json!({
                "status": "active",
                "expectedRevision": 1,
                "unexpected": true
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<super::OrganizationInput>(serde_json::json!({
                "organization": {},
                "policy": {},
                "unexpected": true
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<super::UsageInput>(serde_json::json!({
                "profiles": 1,
                "members": 1,
                "storageBytes": 0,
                "unexpected": true
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<super::RoleInput>(serde_json::json!({
                "role": "member",
                "unexpected": true
            }))
            .is_err()
        );
    }

    #[tokio::test]
    async fn scoped_account_creation_assigns_lower_role_without_invitation() {
        let state = ApiState::hosted_with_control_store(
            ProfileService::disabled(),
            MemoryControlRecordStore::default(),
        )
        .unwrap();
        let owner = state
            .bootstrap_first_owner(crate::http::FirstOwnerBootstrapInput {
                account_id: Some("acct_scoped_create_owner".into()),
                username: "scoped-create-owner".into(),
                email: "scoped-create-owner@example.com".into(),
                operator_attested_email: true,
                password: zeroize::Zeroizing::new("owner-test-password-123".into()),
                organization_name: "Scoped account test org".into(),
                workspace_name: "Scoped account test workspace".into(),
                max_workspaces: 2,
                max_profiles: 10,
                max_members: 10,
                max_storage_bytes: 1024 * 1024,
            })
            .unwrap();
        let owner_token = state.issue_token(owner.account_id.as_str()).unwrap();
        let app = crate::http::router(state.clone());
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/v1/workspaces/{}/accounts",
                        owner.workspace_id.as_str()
                    ))
                    .header("authorization", format!("Bearer {owner_token}"))
                    .header("content-type", "application/json")
                    .header("idempotency-key", "scoped-account-create-test-01")
                    .header("x-correlation-id", "scoped-account-create-test")
                    .body(Body::from(
                        serde_json::to_vec(&serde_json::json!({
                            "accountId": "acct_scoped_created_member",
                            "username": "scoped-created-member",
                            "email": "scoped-created-member@example.com",
                            "role": "member",
                            "initialPassword": "scoped-member-password-123",
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CREATED);
        let body: Value = serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(body["account"]["id"], "acct_scoped_created_member");
        assert_eq!(body["identity"]["verification"], "admin_provisioned");
        assert_eq!(body["member"]["accountId"], "acct_scoped_created_member");
        assert_eq!(body["member"]["workspaceId"], owner.workspace_id.as_str());
        assert_eq!(body["member"]["role"], "member");
        assert_eq!(body["member"]["status"], "active");
        assert_eq!(body["onboardingStatus"], "admin_provisioned");
        assert_eq!(body["loginEnabled"], true);
        assert!(!body.to_string().contains("scoped-member-password-123"));
        assert!(body.get("invitation").is_none());

        let replay = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/v1/workspaces/{}/accounts",
                        owner.workspace_id.as_str()
                    ))
                    .header("authorization", format!("Bearer {owner_token}"))
                    .header("content-type", "application/json")
                    .header("idempotency-key", "scoped-account-create-test-01")
                    .header("x-correlation-id", "scoped-account-create-replay")
                    .body(Body::from(
                        serde_json::to_vec(&serde_json::json!({
                            "accountId": "acct_scoped_created_member",
                            "username": "scoped-created-member",
                            "email": "scoped-created-member@example.com",
                            "role": "member",
                            "initialPassword": "scoped-member-password-123",
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(replay.status(), StatusCode::CREATED);
        let replay_body: Value = serde_json::from_slice(
            &axum::body::to_bytes(replay.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(replay_body["account"]["id"], "acct_scoped_created_member");
        assert_eq!(replay_body["loginEnabled"], true);
        assert!(
            !replay_body
                .to_string()
                .contains("scoped-member-password-123")
        );
        let changed_password_replay = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/v1/workspaces/{}/accounts",
                        owner.workspace_id.as_str()
                    ))
                    .header("authorization", format!("Bearer {owner_token}"))
                    .header("content-type", "application/json")
                    .header("idempotency-key", "scoped-account-create-test-01")
                    .header("x-correlation-id", "scoped-account-create-changed-password")
                    .body(Body::from(
                        serde_json::to_vec(&serde_json::json!({
                            "accountId": "acct_scoped_created_member",
                            "username": "scoped-created-member",
                            "email": "scoped-created-member@example.com",
                            "role": "member",
                            "initialPassword": "different-member-password-456",
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(changed_password_replay.status(), StatusCode::CONFLICT);
        let changed_password_body: Value = serde_json::from_slice(
            &axum::body::to_bytes(changed_password_replay.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(changed_password_body["code"], "idempotency_conflict");
        assert!(
            !changed_password_body
                .to_string()
                .contains("different-member-password-456")
        );
        let credential_after_changed_replay = state
            .control_persistence
            .load::<Value>("identity", "account_password", "acct_scoped_created_member")
            .unwrap()
            .unwrap();
        assert_eq!(credential_after_changed_replay["failedAttempts"], 0);
        assert_eq!(
            state
                .control_persistence
                .load::<Account>("control", "account", "acct_scoped_created_member")
                .unwrap()
                .unwrap()
                .id
                .as_str(),
            "acct_scoped_created_member"
        );
        assert_eq!(
            state
                .control_persistence
                .load::<buglogin_domain::account::AccountIdentity>(
                    "identity",
                    "account_identity",
                    "acct_scoped_created_member"
                )
                .unwrap()
                .unwrap()
                .email,
            "scoped-created-member@example.com"
        );
        let credential = state
            .control_persistence
            .load::<Value>("identity", "account_password", "acct_scoped_created_member")
            .unwrap()
            .unwrap();
        assert!(
            credential["passwordPhc"]
                .as_str()
                .unwrap()
                .starts_with("$argon2id$")
        );
        assert!(
            !credential
                .to_string()
                .contains("scoped-member-password-123")
        );
        assert_eq!(
            crate::services::credentials::CredentialService::new(state.control_persistence.clone())
                .unwrap()
                .authenticate(
                    "scoped-created-member@example.com",
                    "scoped-member-password-123",
                    1
                )
                .unwrap()
                .as_str(),
            "acct_scoped_created_member"
        );

        let persisted_member = state
            .members
            .read()
            .unwrap()
            .active_members()
            .into_iter()
            .find(|member| member.id.as_str() == body["member"]["id"].as_str().unwrap())
            .unwrap();
        assert_eq!(
            persisted_member.account_id.as_str(),
            "acct_scoped_created_member"
        );
        assert_eq!(persisted_member.workspace_id, owner.workspace_id);
        assert_eq!(persisted_member.role, MemberRole::Member);
        assert_eq!(persisted_member.status, MemberStatus::Active);
        assert_eq!(
            state
                .members
                .read()
                .unwrap()
                .active_members()
                .into_iter()
                .filter(|member| member.account_id.as_str() == "acct_scoped_created_member")
                .count(),
            1
        );

        let invalid_password = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/v1/workspaces/{}/accounts",
                        owner.workspace_id.as_str()
                    ))
                    .header("authorization", format!("Bearer {owner_token}"))
                    .header("content-type", "application/json")
                    .header("idempotency-key", "scoped-account-create-invalid-01")
                    .header("x-correlation-id", "scoped-account-create-invalid")
                    .body(Body::from(
                        serde_json::to_vec(&serde_json::json!({
                            "accountId": "acct_scoped_invalid_password",
                            "username": "scoped-invalid-password",
                            "email": "scoped-invalid-password@example.com",
                            "role": "member",
                            "initialPassword": "too-short",
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(invalid_password.status(), StatusCode::BAD_REQUEST);
        for (tenant, record_type) in [
            ("control", "account"),
            ("identity", "account_identity"),
            ("identity", "account_password"),
        ] {
            assert!(
                state
                    .control_persistence
                    .load::<Value>(tenant, record_type, "acct_scoped_invalid_password")
                    .unwrap()
                    .is_none()
            );
        }
    }

    #[tokio::test]
    async fn scoped_account_creation_membership_failure_leaves_no_account_or_identity() {
        let store = MemoryControlRecordStore::default();
        let state = ApiState::hosted_with_control_store(ProfileService::disabled(), store).unwrap();
        let owner = state
            .bootstrap_first_owner(crate::http::FirstOwnerBootstrapInput {
                account_id: Some("acct_scoped_failure_owner".into()),
                username: "scoped-failure-owner".into(),
                email: "scoped-failure-owner@example.com".into(),
                operator_attested_email: true,
                password: zeroize::Zeroizing::new("owner-test-password-123".into()),
                organization_name: "Scoped failure test org".into(),
                workspace_name: "Scoped failure test workspace".into(),
                max_workspaces: 2,
                max_profiles: 10,
                max_members: 10,
                max_storage_bytes: 1024 * 1024,
            })
            .unwrap();
        let owner_token = state.issue_token(owner.account_id.as_str()).unwrap();
        let account_id = AccountId::new("acct_scoped_membership_conflict").unwrap();
        let colliding_member_id = crate::services::account_provisioning::scoped_member_id(
            &owner.workspace_id,
            &account_id,
        )
        .unwrap();
        let conflicting_member = Member {
            id: colliding_member_id.clone(),
            account_id: AccountId::new("acct_existing_collision_member").unwrap(),
            workspace_id: owner.workspace_id.clone(),
            team_id: None,
            role: MemberRole::Member,
            status: MemberStatus::Active,
            invite_expires_at: None,
        };
        let mut members = serde_json::to_value(&*state.members.read().unwrap()).unwrap();
        members["members"][colliding_member_id.as_str()] =
            serde_json::to_value(conflicting_member).unwrap();
        *state.members.write().unwrap() = serde_json::from_value(members).unwrap();

        let response = crate::http::router(state.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/v1/workspaces/{}/accounts",
                        owner.workspace_id.as_str()
                    ))
                    .header("authorization", format!("Bearer {owner_token}"))
                    .header("content-type", "application/json")
                    .header("idempotency-key", "scoped-account-create-failure-01")
                    .header("x-correlation-id", "scoped-account-create-failure")
                    .body(Body::from(
                        serde_json::to_vec(&serde_json::json!({
                            "accountId": account_id,
                            "username": "scoped-membership-conflict",
                            "email": "scoped-membership-conflict@example.com",
                            "role": "member",
                            "initialPassword": "scoped-member-password-123",
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert!(
            state
                .control_persistence
                .load::<Account>("control", "account", "acct_scoped_membership_conflict")
                .unwrap()
                .is_none()
        );
        assert!(
            state
                .control_persistence
                .load::<Value>("identity", "account_password", account_id.as_str())
                .unwrap()
                .is_none()
        );
        assert!(
            state
                .control_persistence
                .load::<buglogin_domain::account::AccountIdentity>(
                    "identity",
                    "account_identity",
                    "acct_scoped_membership_conflict"
                )
                .unwrap()
                .is_none()
        );
        assert_eq!(
            state
                .members
                .read()
                .unwrap()
                .active_members()
                .into_iter()
                .filter(|member| member.account_id.as_str() == "acct_existing_collision_member")
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn workspace_invitation_rest_lifecycle_requires_verified_email_and_is_single_use() {
        let state = ApiState::hosted_with_control_store(
            ProfileService::disabled(),
            MemoryControlRecordStore::default(),
        )
        .unwrap();
        let owner = state
            .bootstrap_first_owner(crate::http::FirstOwnerBootstrapInput {
                account_id: Some("acct_invite_owner".into()),
                username: "invite-owner".into(),
                email: "invite-owner@example.com".into(),
                operator_attested_email: true,
                password: zeroize::Zeroizing::new("owner-test-password-123".into()),
                organization_name: "Invitation test org".into(),
                workspace_name: "Invitation test workspace".into(),
                max_workspaces: 2,
                max_profiles: 10,
                max_members: 10,
                max_storage_bytes: 1024 * 1024,
            })
            .unwrap();
        let owner_token = state.issue_token(owner.account_id.as_str()).unwrap();

        let invitee_id = AccountId::new("acct_invite_member").unwrap();
        state
            .account_persistence
            .provision_default(invitee_id.clone(), "2026-10-20T00:00:00Z".into())
            .unwrap();
        let pending_identity = state
            .provision_web_identity(
                invitee_id.as_str(),
                "invite-member",
                "Invitee@Example.com",
                "invitee-test-password-123",
            )
            .unwrap();
        assert_eq!(pending_identity.verification, IdentityVerification::Pending);
        let invitee_token = state.issue_token(invitee_id.as_str()).unwrap();
        let app = crate::http::router(state.clone());
        let expires_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 3600;
        let create_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/v1/workspaces/{}/invitations",
                        owner.workspace_id.as_str()
                    ))
                    .header("authorization", format!("Bearer {owner_token}"))
                    .header("content-type", "application/json")
                    .header("idempotency-key", "invitation-create-test-key-01")
                    .header("x-correlation-id", "invitation-create-test")
                    .body(Body::from(
                        serde_json::to_vec(&serde_json::json!({
                            "email": "INVITEE@example.com",
                            "role": "member",
                            "expiresAt": expires_at,
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let create_status = create_response.status();
        let create_body_bytes = axum::body::to_bytes(create_response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(
            create_status,
            StatusCode::CREATED,
            "{}",
            String::from_utf8_lossy(&create_body_bytes)
        );
        let create_body: Value = serde_json::from_slice(&create_body_bytes).unwrap();
        let invitation_id = create_body["invitationId"].as_str().unwrap();
        let code = create_body["code"].as_str().unwrap();
        let persisted = state
            .control_persistence
            .load::<MemberInvitation>("control", "member_invitation", invitation_id)
            .unwrap()
            .unwrap();
        assert_ne!(persisted.token_hash, code);
        assert!(persisted.verify_code(code));
        assert_eq!(persisted.canonical_email, "invitee@example.com");

        let redeem = |idempotency_key: &'static str| {
            Request::builder()
                .method("POST")
                .uri(format!("/v1/workspace-invitations/{invitation_id}/redeem"))
                .header("authorization", format!("Bearer {invitee_token}"))
                .header("content-type", "application/json")
                .header("idempotency-key", idempotency_key)
                .header("x-correlation-id", "invitation-redeem-test")
                .body(Body::from(
                    serde_json::to_vec(&serde_json::json!({"code": code})).unwrap(),
                ))
                .unwrap()
        };
        let unverified_response = app
            .clone()
            .oneshot(redeem("invitation-redeem-unverified-01"))
            .await
            .unwrap();
        assert_eq!(unverified_response.status(), StatusCode::GONE);

        let verified_identity = pending_identity.verify(1, "2026-10-20T00:01:00Z").unwrap();
        let identity_key = ControlRecordKey {
            tenant_id: "identity".into(),
            record_type: "account_identity".into(),
            record_id: invitee_id.as_str().into(),
        };
        state
            .control_persistence
            .execute(ExecuteControlCommand {
                tenant_id: "identity".into(),
                command_id: "identity.test.verify".into(),
                idempotency_key: "identity-test-verify-invitee".into(),
                expected_revisions: [(identity_key, 1)].into_iter().collect(),
                puts: vec![ControlPut {
                    tenant_id: "identity".into(),
                    record_type: "account_identity".into(),
                    record_id: invitee_id.as_str().into(),
                    expected_revision: Some(1),
                    value: serde_json::to_value(verified_identity).unwrap(),
                }],
                deletes: Vec::new(),
                audit: ControlAuditEntry {
                    tenant_id: "identity".into(),
                    command_id: "identity.test.verify".into(),
                    actor_id: owner.account_id.as_str().into(),
                    allowed: true,
                    detail: serde_json::json!({"testOnly": true}),
                },
                response: HttpResponse {
                    status: 200,
                    headers: Default::default(),
                    body: Value::Null,
                },
            })
            .unwrap();
        let redeem_response = app
            .clone()
            .oneshot(redeem("invitation-redeem-test-key-01"))
            .await
            .unwrap();
        assert_eq!(redeem_response.status(), StatusCode::OK);
        let member: Value = serde_json::from_slice(
            &axum::body::to_bytes(redeem_response.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(member["accountId"], invitee_id.as_str());
        assert_eq!(member["workspaceId"], owner.workspace_id.as_str());
        assert_eq!(member["status"], "invited");

        let member_page_uri = format!(
            "/v1/workspaces/{}/members/page?limit=50",
            owner.workspace_id.as_str()
        );
        let owner_page = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(&member_page_uri)
                    .header("authorization", format!("Bearer {owner_token}"))
                    .header("x-correlation-id", "member-directory-owner")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(owner_page.status(), StatusCode::OK);
        let owner_page: Value = serde_json::from_slice(
            &axum::body::to_bytes(owner_page.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        let listed_invitee = owner_page["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["accountId"] == invitee_id.as_str())
            .unwrap();
        assert_eq!(listed_invitee["identity"]["username"], "invite-member");
        assert_eq!(listed_invitee["identity"]["email"], "Invitee@Example.com");

        state.grants.write().unwrap().push(Grant {
            id: GrantId::new("grant_invitee_member_read").unwrap(),
            account_id: Some(invitee_id.clone()),
            member_id: None,
            capability: Capability::new("CAP-MEM-001").unwrap(),
            action: Action::Read,
            scope: ResourceScope::workspace(owner.workspace_id.clone()),
            effect: buglogin_domain::permissions::Effect::Allow,
        });
        let invitee_page = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(&member_page_uri)
                    .header("authorization", format!("Bearer {invitee_token}"))
                    .header("x-correlation-id", "member-directory-reader")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(invitee_page.status(), StatusCode::OK);
        let invitee_page: Value = serde_json::from_slice(
            &axum::body::to_bytes(invitee_page.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        let listed_owner = invitee_page["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["accountId"] == owner.account_id.as_str())
            .unwrap();
        assert!(listed_owner.get("identity").is_none());

        let replay_response = app
            .oneshot(redeem("invitation-redeem-test-key-02"))
            .await
            .unwrap();
        assert_eq!(replay_response.status(), StatusCode::GONE);
        let redeemed = state
            .control_persistence
            .load::<MemberInvitation>("control", "member_invitation", invitation_id)
            .unwrap()
            .unwrap();
        assert_eq!(redeemed.state, MemberInvitationState::Redeemed);
    }

    #[test]
    fn sync_actor_header_must_match_bearer_account() {
        let account_id = AccountId::new("acct_sync_actor").unwrap();
        let principal = Principal {
            account_id: Some(account_id.clone()),
            ..Principal::default()
        };
        let mut headers = HeaderMap::new();
        headers.insert("x-buglogin-actor-id", account_id.as_str().parse().unwrap());
        assert!(require_actor_header(&principal, &headers).is_ok());

        headers.insert("x-buglogin-actor-id", "acct_other_actor".parse().unwrap());
        assert_eq!(
            require_actor_header(&principal, &headers)
                .unwrap_err()
                .status(),
            StatusCode::FORBIDDEN
        );
    }

    #[test]
    fn hosted_manifest_commit_enforces_workspace_storage_quota_and_allows_reduction() {
        let organization = buglogin_domain::organization::Organization {
            id: OrganizationId::new("org_storage_quota").unwrap(),
            account_id: AccountId::new("acct_storage_quota").unwrap(),
            name: "Storage quota".into(),
            created_at: "2026-10-15T00:00:00Z".into(),
        };
        let workspace_id = WorkspaceId::new("ws_storage_quota").unwrap();
        let principal = Principal {
            roles: std::collections::BTreeSet::from([
                buglogin_domain::permissions::Role::PlatformAdmin,
            ]),
            ..Principal::default()
        };
        let mut workspaces = crate::services::workspaces::WorkspaceService::new(
            organization.clone(),
            buglogin_domain::organization::OrganizationPolicy {
                max_workspaces: 1,
                offline_entitled: false,
            },
        )
        .unwrap();
        workspaces
            .create_workspace(
                &principal,
                &[],
                &organization.id,
                buglogin_domain::workspace::Workspace {
                    id: workspace_id.clone(),
                    organization_id: organization.id.clone(),
                    name: "Storage quota".into(),
                    policy: buglogin_domain::workspace::WorkspacePolicy {
                        max_profiles: 10,
                        max_members: 10,
                        max_storage_bytes: 10,
                        offline_entitled: false,
                        default_extension_group_id: None,
                    },
                    created_at: "2026-10-15T00:00:00Z".into(),
                },
            )
            .unwrap();
        let state = ApiState::hosted_with_control_store(
            ProfileService::disabled(),
            MemoryControlRecordStore::default(),
        )
        .unwrap();
        *state.workspaces.write().unwrap() = workspaces;

        let other_resource = ResourceId::new("res_storage_other").unwrap();
        let current_resource = ResourceId::new("res_storage_current").unwrap();
        let other_object = ManifestObject::new("profile/other.bin", b"123456789012").unwrap();
        let current_object = ManifestObject::new("profile/current.bin", b"1234").unwrap();
        let other_manifest = SyncManifest::new(
            workspace_id.clone(),
            other_resource,
            1,
            1,
            None,
            b"other-root",
            vec![other_object],
        )
        .unwrap();
        let current_manifest = SyncManifest::new(
            workspace_id.clone(),
            current_resource.clone(),
            1,
            1,
            None,
            b"current-root",
            vec![current_object],
        )
        .unwrap();
        {
            let mut manifests = state.sync_manifests.write().unwrap();
            manifests.insert("other".into(), other_manifest);
            manifests.insert("current".into(), current_manifest);
        }

        let oversized = SyncManifest::new(
            workspace_id.clone(),
            current_resource.clone(),
            1,
            2,
            Some(1),
            b"oversized-root",
            vec![ManifestObject::new("profile/current.bin", b"12345").unwrap()],
        )
        .unwrap();
        assert_eq!(
            validate_workspace_sync_storage_quota(
                &state,
                &workspace_id,
                &current_resource,
                &oversized,
            )
            .unwrap_err()
            .1["code"]
                .as_str(),
            Some("storage_quota_exceeded")
        );

        let reduced = SyncManifest::new(
            workspace_id.clone(),
            current_resource.clone(),
            1,
            2,
            Some(1),
            b"reduced-root",
            vec![ManifestObject::new("profile/current.bin", b"123").unwrap()],
        )
        .unwrap();
        validate_workspace_sync_storage_quota(&state, &workspace_id, &current_resource, &reduced)
            .expect("a smaller manifest should remain writable");
    }

    #[test]
    fn sync_blob_reservation_batch_is_atomic_idempotent_and_quota_bounded() {
        let organization = buglogin_domain::organization::Organization {
            id: OrganizationId::new("org_blob_reservation").unwrap(),
            account_id: AccountId::new("acct_blob_reservation").unwrap(),
            name: "Blob reservation".into(),
            created_at: "2026-10-15T00:00:00Z".into(),
        };
        let workspace_id = WorkspaceId::new("ws_blob_reservation").unwrap();
        let principal = Principal {
            roles: std::collections::BTreeSet::from([
                buglogin_domain::permissions::Role::PlatformAdmin,
            ]),
            ..Principal::default()
        };
        let mut workspaces = crate::services::workspaces::WorkspaceService::new(
            organization.clone(),
            buglogin_domain::organization::OrganizationPolicy {
                max_workspaces: 1,
                offline_entitled: false,
            },
        )
        .unwrap();
        workspaces
            .create_workspace(
                &principal,
                &[],
                &organization.id,
                buglogin_domain::workspace::Workspace {
                    id: workspace_id.clone(),
                    organization_id: organization.id.clone(),
                    name: "Blob reservation".into(),
                    policy: buglogin_domain::workspace::WorkspacePolicy {
                        max_profiles: 10,
                        max_members: 10,
                        max_storage_bytes: 10,
                        offline_entitled: false,
                        default_extension_group_id: None,
                    },
                    created_at: "2026-10-15T00:00:00Z".into(),
                },
            )
            .unwrap();
        let state = ApiState::hosted_with_control_store(
            ProfileService::disabled(),
            MemoryControlRecordStore::default(),
        )
        .unwrap();
        *state.workspaces.write().unwrap() = workspaces;

        let resource_id = ResourceId::new("res_blob_reservation").unwrap();
        let too_large = vec![
            super::SyncMigrationJournalObject {
                storage_key: "resource/a".into(),
                checksum: "a".repeat(64),
                size: 6,
            },
            super::SyncMigrationJournalObject {
                storage_key: "resource/b".into(),
                checksum: "b".repeat(64),
                size: 5,
            },
        ];
        assert_eq!(
            state
                .reserve_sync_blob_uploads(
                    &workspace_id,
                    &resource_id,
                    &too_large,
                    1_000,
                    "acct_blob_reservation",
                )
                .unwrap_err()
                .code,
            "storage_quota_exceeded"
        );
        assert!(state.sync_blob_reservations.read().unwrap().is_empty());

        let conflicting_duplicate = [
            super::SyncMigrationJournalObject {
                storage_key: "resource/duplicate".into(),
                checksum: "a".repeat(64),
                size: 3,
            },
            super::SyncMigrationJournalObject {
                storage_key: "resource/duplicate".into(),
                checksum: "b".repeat(64),
                size: 3,
            },
        ];
        assert_eq!(
            state
                .reserve_sync_blob_uploads(
                    &workspace_id,
                    &resource_id,
                    &conflicting_duplicate,
                    1_500,
                    "acct_blob_reservation",
                )
                .unwrap_err()
                .code,
            "sync_blob_reservation_conflict"
        );
        assert!(state.sync_blob_reservations.read().unwrap().is_empty());

        let within_quota = vec![
            super::SyncMigrationJournalObject {
                storage_key: "resource/a".into(),
                checksum: "a".repeat(64),
                size: 3,
            },
            super::SyncMigrationJournalObject {
                storage_key: "resource/a".into(),
                checksum: "a".repeat(64),
                size: 3,
            },
            super::SyncMigrationJournalObject {
                storage_key: "resource/b".into(),
                checksum: "b".repeat(64),
                size: 4,
            },
        ];
        state
            .reserve_sync_blob_uploads(
                &workspace_id,
                &resource_id,
                &within_quota,
                2_000,
                "acct_blob_reservation",
            )
            .unwrap();
        state
            .reserve_sync_blob_uploads(
                &workspace_id,
                &resource_id,
                &within_quota,
                3_000,
                "acct_blob_reservation",
            )
            .expect("retrying the same batch must not double-count reserved bytes");
        assert_eq!(state.sync_blob_reservations.read().unwrap().len(), 2);

        state
            .workspaces
            .write()
            .unwrap()
            .update_workspace(
                &principal,
                &[],
                &OrganizationId::new("org_blob_reservation").unwrap(),
                &workspace_id,
                crate::services::workspaces::WorkspaceUpdate {
                    workspace: buglogin_domain::workspace::Workspace {
                        id: workspace_id.clone(),
                        organization_id: OrganizationId::new("org_blob_reservation").unwrap(),
                        name: "Blob reservation".into(),
                        policy: buglogin_domain::workspace::WorkspacePolicy {
                            max_profiles: 10,
                            max_members: 10,
                            max_storage_bytes: 6,
                            offline_entitled: false,
                            default_extension_group_id: None,
                        },
                        created_at: "2026-10-15T00:00:00Z".into(),
                    },
                    expected_revision: 1,
                },
            )
            .unwrap();
        state
            .reserve_sync_blob_uploads(
                &workspace_id,
                &resource_id,
                &within_quota,
                4_000,
                "acct_blob_reservation",
            )
            .expect("retrying an existing reservation remains possible after a quota reduction");

        let additional = [super::SyncMigrationJournalObject {
            storage_key: "resource/c".into(),
            checksum: "c".repeat(64),
            size: 1,
        }];
        assert_eq!(
            state
                .reserve_sync_blob_uploads(
                    &workspace_id,
                    &resource_id,
                    &additional,
                    5_000,
                    "acct_blob_reservation",
                )
                .unwrap_err()
                .code,
            "storage_quota_exceeded"
        );
        assert_eq!(state.sync_blob_reservations.read().unwrap().len(), 2);
    }

    #[test]
    fn encrypted_group_changes_fence_profiles_for_rekey() {
        let workspace = WorkspaceId::new("ws_group_key_gate").unwrap();
        let group = GroupId::new("grp_group_key_gate").unwrap();
        let profile = ProfileId::new("prf_group_key_gate").unwrap();
        let resource = ResourceId::new("res_group_key_gate").unwrap();
        let state = ApiState::hosted_with_control_store(
            ProfileService::disabled(),
            MemoryControlRecordStore::default(),
        )
        .unwrap()
        .with_profile_resource(ProfileResource {
            profile_id: profile.clone(),
            resource_id: resource.clone(),
            workspace_id: workspace.clone(),
            archive_state: ProfileArchiveState::Active,
        });
        state
            .create_profile_record(Profile {
                id: profile.clone(),
                name: "Encrypted shared profile".into(),
                runtime: Runtime::Wayfern,
                runtime_version: Some("1.0.0".into()),
                runtime_config: None,
                state: ProfileState::Ready,
                archived: false,
                fingerprint: None,
                note: None,
                proxy_id: None,
                vpn_id: None,
                group_ids: vec![group.clone()],
                tags: Vec::new(),
                extension_group_id: None,
                sync_mode: Some(SyncMode::Managed),
                created_at: "2026-10-14T00:00:00Z".into(),
                updated_at: "2026-10-14T00:00:00Z".into(),
            })
            .unwrap();
        let envelope = ManifestObject::new(
            buglogin_sync::manifest::RESOURCE_KEY_ENVELOPE_KEY,
            b"encrypted-resource-key-envelope",
        )
        .unwrap();
        let mut manifest = SyncManifest::new(
            workspace.clone(),
            resource.clone(),
            1,
            1,
            None,
            b"group-share-key-gate",
            vec![envelope.clone()],
        )
        .unwrap();
        manifest.payload_encryption = Some(ManifestPayloadEncryption {
            schema_version: 1,
            algorithm: PayloadEncryptionAlgorithm::Aes256GcmChunkV1,
            key_version: None,
            resource_key_envelope: ManifestObjectReference::from(&envelope),
        });
        state.replace_sync_manifest(manifest).unwrap();

        HostedProfileService::mark_resource_key_rotation_if_encrypted(
            &state, &workspace, &resource,
        )
        .unwrap();
        assert!(
            state
                .resource_key_rotation_pending(&workspace, &resource)
                .unwrap()
        );
        state
            .clear_resource_key_rotation_pending(&workspace, &resource)
            .unwrap();

        HostedProfileService::mark_group_profiles_for_key_rotation(&state, &workspace, &group)
            .unwrap();
        assert!(
            state
                .resource_key_rotation_pending(&workspace, &resource)
                .unwrap()
        );
        let owner_id = AccountId::new("acct_group_key_gate_owner").unwrap();
        let owner = Principal {
            account_id: Some(owner_id.clone()),
            workspace_ids: [workspace.clone()].into_iter().collect(),
            ..Principal::default()
        };
        let update_grant = Grant {
            id: GrantId::new("grant_group_key_gate_update").unwrap(),
            account_id: Some(owner_id),
            member_id: None,
            capability: Capability::new("profile.update").unwrap(),
            action: Action::Update,
            scope: ResourceScope::resource(workspace.clone(), resource.clone()),
            effect: Effect::Allow,
        };
        assert!(
            authorize_lease(
                &state,
                &owner,
                std::slice::from_ref(&update_grant),
                &workspace,
                &resource,
                "profile.update",
                Action::Update,
            )
            .is_ok()
        );
        let launch_error = authorize_lease(
            &state,
            &owner,
            &[update_grant],
            &workspace,
            &resource,
            "profile.launch",
            Action::Execute,
        )
        .unwrap_err();
        assert_eq!(launch_error.status(), StatusCode::CONFLICT);
    }

    #[test]
    fn profile_share_read_applies_to_sync_manifest_but_not_upload_authority() {
        use buglogin_domain::permissions::{Effect, Grant, Role};

        let workspace = WorkspaceId::new("ws_shared_manifest").unwrap();
        let resource = ResourceId::new("res_shared_manifest").unwrap();
        let profile = ProfileId::new("prf_shared_manifest").unwrap();
        let owner_id = AccountId::new("acct_share_owner").unwrap();
        let recipient_id = AccountId::new("acct_share_recipient").unwrap();
        let member_id = MemberId::new("mem_share_recipient").unwrap();
        let state = ApiState::hosted_with_control_store(
            ProfileService::disabled(),
            MemoryControlRecordStore::default(),
        )
        .unwrap()
        .with_profile_resource(ProfileResource {
            profile_id: profile,
            resource_id: resource.clone(),
            workspace_id: workspace.clone(),
            archive_state: ProfileArchiveState::Active,
        });
        let owner = Principal {
            account_id: Some(owner_id.clone()),
            workspace_ids: [workspace.clone()].into_iter().collect(),
            roles: [Role::WorkspaceAdmin].into_iter().collect(),
            ..Principal::default()
        };
        let grant = |id: &str, capability: &str, action, scope| Grant {
            id: GrantId::new(id).unwrap(),
            account_id: Some(owner_id.clone()),
            member_id: None,
            capability: Capability::new(capability).unwrap(),
            action,
            scope,
            effect: Effect::Allow,
        };
        let grants = vec![
            grant(
                "grant_member_create",
                "CAP-MEM-001",
                Action::Create,
                ResourceScope::workspace(workspace.clone()),
            ),
            grant(
                "grant_resource_create",
                "CAP-RES-001",
                Action::Create,
                ResourceScope::workspace(workspace.clone()),
            ),
            grant(
                "grant_share_manage",
                "CAP-SHARE-001",
                Action::Manage,
                ResourceScope::resource(workspace.clone(), resource.clone()),
            ),
        ];
        let platform_admin = Principal {
            roles: [Role::PlatformAdmin].into_iter().collect(),
            ..Principal::default()
        };
        state
            .members
            .write()
            .unwrap()
            .invite(
                &platform_admin,
                &[],
                Member {
                    id: MemberId::new("mem_share_owner").unwrap(),
                    account_id: owner_id.clone(),
                    workspace_id: workspace.clone(),
                    team_id: None,
                    role: MemberRole::Owner,
                    status: MemberStatus::Invited,
                    invite_expires_at: Some(100),
                },
                1,
            )
            .unwrap();
        state
            .members
            .write()
            .unwrap()
            .activate(
                &Principal {
                    account_id: Some(owner_id.clone()),
                    ..Principal::default()
                },
                &[],
                &workspace,
                &MemberId::new("mem_share_owner").unwrap(),
                2,
            )
            .unwrap();
        state
            .members
            .write()
            .unwrap()
            .invite(
                &owner,
                &grants,
                Member {
                    id: member_id.clone(),
                    account_id: recipient_id.clone(),
                    workspace_id: workspace.clone(),
                    team_id: None,
                    role: MemberRole::Member,
                    status: MemberStatus::Invited,
                    invite_expires_at: Some(100),
                },
                1,
            )
            .unwrap();
        state
            .members
            .write()
            .unwrap()
            .activate(
                &Principal {
                    account_id: Some(recipient_id.clone()),
                    ..Principal::default()
                },
                &[],
                &workspace,
                &member_id,
                2,
            )
            .unwrap();
        state
            .resources
            .write()
            .unwrap()
            .register(
                &owner,
                &grants,
                Resource {
                    id: resource.clone(),
                    workspace_id: workspace.clone(),
                    kind: ResourceKind::Profile,
                    state: ResourceState::Active,
                    display_name: "Shared profile".into(),
                },
            )
            .unwrap();
        state
            .resources
            .write()
            .unwrap()
            .grant(
                &owner,
                &grants,
                ResourceGrant {
                    id: GrantId::new("rg_shared_manifest_read").unwrap(),
                    workspace_id: workspace.clone(),
                    resource_id: resource.clone(),
                    member_id,
                    action: Action::Read,
                    effect: Effect::Allow,
                },
            )
            .unwrap();

        let recipient = Principal {
            account_id: Some(recipient_id),
            ..Principal::default()
        };
        authorize_lease(
            &state,
            &recipient,
            &[],
            &workspace,
            &resource,
            "profile.read",
            Action::Read,
        )
        .unwrap();
        assert!(
            authorize_lease(
                &state,
                &recipient,
                &[],
                &workspace,
                &resource,
                "profile.update",
                Action::Update,
            )
            .is_err()
        );
        state
            .mark_resource_key_rotation_pending(&workspace, &resource)
            .unwrap();
        assert!(
            authorize_lease(
                &state,
                &recipient,
                &[],
                &workspace,
                &resource,
                "profile.read",
                Action::Read,
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn hosted_sync_reads_refresh_committed_state_from_another_instance() {
        let account = AccountId::new("acct_blob_refresh").unwrap();
        let workspace = WorkspaceId::new("ws_blob_refresh").unwrap();
        let resource = ResourceId::new("res_blob_refresh").unwrap();
        let manifest_resource = ResourceId::new("res_manifest_refresh").unwrap();
        let mut store = MemoryControlRecordStore::default();
        let account_value =
            serde_json::to_value(Account::new(account.clone(), "2026-10-12")).unwrap();
        store
            .put(PutControlRecord {
                tenant_id: "control",
                record_type: "account",
                record_id: account.as_str(),
                expected_revision: Some(0),
                idempotency_key: "seed-blob-refresh-account",
                value: &account_value,
            })
            .unwrap();

        let writer =
            ApiState::hosted_with_control_store(ProfileService::disabled(), store.clone()).unwrap();
        let token = writer.issue_token(account.as_str()).unwrap();
        let reader =
            ApiState::hosted_with_control_store(ProfileService::disabled(), store.reopen())
                .unwrap();
        let manifest_reader =
            ApiState::hosted_with_control_store(ProfileService::disabled(), store.reopen())
                .unwrap();

        let grant = buglogin_domain::permissions::Grant {
            id: GrantId::new("grant_blob_refresh_read").unwrap(),
            account_id: Some(account.clone()),
            member_id: None,
            capability: buglogin_domain::permissions::Capability::new("profile.read").unwrap(),
            action: buglogin_domain::permissions::Action::Read,
            scope: ResourceScope::workspace(workspace.clone()),
            effect: buglogin_domain::permissions::Effect::Allow,
        };
        writer.grants.write().unwrap().push(grant);

        let bytes = b"verified encrypted bytes";
        let object = ManifestObject::new("Cookies", bytes).unwrap();
        let manifest = SyncManifest::new(
            workspace.clone(),
            resource.clone(),
            1,
            1,
            None,
            b"blob-refresh-manifest",
            vec![object.clone()],
        )
        .unwrap();
        writer.replace_sync_manifest(manifest).unwrap();
        writer
            .replace_sync_manifest(
                SyncManifest::new(
                    workspace.clone(),
                    manifest_resource.clone(),
                    1,
                    1,
                    None,
                    b"manifest-refresh-genesis",
                    Vec::new(),
                )
                .unwrap(),
            )
            .unwrap();
        writer
            .persist_control_aggregate(
                "test.sync.seed",
                "test-blob-refresh-seed".into(),
                account.as_str().to_owned(),
                true,
                &"a".repeat(64),
                json!({"test": "blob-refresh"}),
                200,
                json!({"seeded": true}),
            )
            .unwrap();

        let manifest_request = Request::builder()
            .method("GET")
            .uri(format!(
                "/v1/workspaces/{}/resources/{}/manifest",
                workspace.as_str(),
                manifest_resource.as_str()
            ))
            .header("authorization", format!("Bearer {token}"))
            .header("x-correlation-id", "test-manifest-refresh")
            .header("x-buglogin-actor-id", account.as_str())
            .body(Body::empty())
            .unwrap();
        let manifest_response = crate::http::router(manifest_reader)
            .oneshot(manifest_request)
            .await
            .unwrap();
        assert_eq!(manifest_response.status(), StatusCode::OK);
        let manifest_body: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(manifest_response.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(manifest_body["generation"], 1);

        let object_key = payload_object_key(&resource, 1, &object.key, &object.checksum);
        reader
            .put_remote_chunk(&object_key, bytes, &object.checksum)
            .unwrap();
        let request = Request::builder()
            .method("GET")
            .uri(format!(
                "/v1/workspaces/{}/resources/{}/blobs?key=Cookies",
                workspace.as_str(),
                resource.as_str()
            ))
            .header("authorization", format!("Bearer {token}"))
            .header("x-correlation-id", "test-blob-refresh")
            .header("x-buglogin-generation", "1")
            .header("x-content-sha256", &object.checksum)
            .header("x-buglogin-actor-id", account.as_str())
            .body(Body::empty())
            .unwrap();

        let response = crate::http::router(reader).oneshot(request).await.unwrap();
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
        assert_eq!(body.as_ref(), bytes);
    }

    #[test]
    fn active_v2_key_rotation_requires_a_complete_reencrypted_generation() {
        let state = ApiState::local(ProfileService::disabled());
        let workspace_id = WorkspaceId::new("ws_key_version").unwrap();
        let resource_id = ResourceId::new("res_key_version").unwrap();
        let envelope = ManifestObject::new(
            buglogin_sync::manifest::RESOURCE_KEY_ENVELOPE_KEY,
            b"encrypted-resource-key-v1",
        )
        .unwrap();
        let payload = ManifestObject::new("Default/Cookies", b"ciphertext-v1").unwrap();
        let old_payload_checksum = payload.checksum.clone();
        let encryption = |key_version: &str, envelope: &ManifestObject| ManifestPayloadEncryption {
            schema_version: 1,
            algorithm: PayloadEncryptionAlgorithm::Aes256GcmChunkV2,
            key_version: Some(key_version.to_owned()),
            resource_key_envelope: ManifestObjectReference::from(envelope),
        };
        let mut current = SyncManifest::new(
            workspace_id.clone(),
            resource_id.clone(),
            1,
            1,
            None,
            b"generation-one",
            vec![envelope.clone(), payload],
        )
        .unwrap();
        current.payload_encryption = Some(encryption(&"a".repeat(64), &envelope));
        current.validate().unwrap();
        state.replace_sync_manifest(current).unwrap();

        let rotated_envelope = ManifestObject::new(
            buglogin_sync::manifest::RESOURCE_KEY_ENVELOPE_KEY,
            b"encrypted-resource-key-v2",
        )
        .unwrap();
        let rotated_payload = ManifestObject::new("Default/Cookies", b"ciphertext-v2").unwrap();
        let mut candidate = SyncManifest::new(
            workspace_id.clone(),
            resource_id.clone(),
            1,
            2,
            Some(1),
            b"generation-two",
            vec![rotated_envelope.clone(), rotated_payload],
        )
        .unwrap();
        candidate.payload_encryption = Some(encryption(&"b".repeat(64), &rotated_envelope));
        candidate.validate().unwrap();

        assert!(
            validate_manifest_successor(&state, &workspace_id, &resource_id, &candidate).is_ok()
        );
        state
            .mark_resource_key_rotation_pending(&workspace_id, &resource_id)
            .unwrap();
        assert!(
            validate_pending_key_rotation(&state, &workspace_id, &resource_id, &candidate).is_ok()
        );
        let mut same_key = candidate.clone();
        same_key.payload_encryption.as_mut().unwrap().key_version = Some("a".repeat(64));
        assert_eq!(
            validate_pending_key_rotation(&state, &workspace_id, &resource_id, &same_key)
                .unwrap_err()
                .1["code"],
            "sync_key_rotation_required"
        );

        candidate.objects[1].source_generation = Some(1);
        assert_eq!(
            validate_manifest_successor(&state, &workspace_id, &resource_id, &candidate)
                .unwrap_err()
                .1["code"],
            "sync_key_rotation_requires_full_reencrypt"
        );

        candidate.objects[1].source_generation = None;
        candidate.objects[1].checksum = old_payload_checksum;
        assert_eq!(
            validate_manifest_successor(&state, &workspace_id, &resource_id, &candidate)
                .unwrap_err()
                .1["code"],
            "sync_key_rotation_object_unchanged"
        );

        candidate.objects[1].checksum = "c".repeat(64);
        candidate.objects.pop();
        let error = validate_manifest_successor(&state, &workspace_id, &resource_id, &candidate)
            .unwrap_err();
        assert_eq!(error.1["code"], "sync_key_rotation_incomplete");
    }

    #[test]
    fn manifest_successor_accepts_new_tombstones_and_preserves_legacy_replays() {
        let state = ApiState::local(ProfileService::disabled());
        let workspace_id = WorkspaceId::new("ws_manifest_tombstone").unwrap();
        let resource_id = ResourceId::new("res_manifest_tombstone").unwrap();
        let mut previous = SyncManifest::new(
            workspace_id.clone(),
            resource_id.clone(),
            1,
            1,
            None,
            b"generation-one",
            vec![ManifestObject::new("Default/Cookies", b"cookie-db").unwrap()],
        )
        .unwrap();
        previous.objects[0].key = "Default/Cache/legacy.bin".into();
        previous.validate().unwrap();
        state.replace_sync_manifest(previous.clone()).unwrap();

        assert!(
            validate_manifest_successor(&state, &workspace_id, &resource_id, &previous,).is_ok()
        );

        let candidate = SyncManifest::new_with_tombstones(
            workspace_id.clone(),
            resource_id.clone(),
            1,
            2,
            Some(1),
            b"generation-two",
            vec![ManifestObject::new("Default/Cookies", b"new-cookie-db").unwrap()],
            vec![ManifestTombstone::new("Default/Cache/legacy.bin", 2, 1_000, 2_000).unwrap()],
        )
        .unwrap();
        assert!(
            validate_manifest_successor(&state, &workspace_id, &resource_id, &candidate,).is_ok()
        );
    }

    #[test]
    fn manifest_commit_accepts_legacy_and_key_rotation_request_shapes() {
        let manifest = SyncManifest::new(
            WorkspaceId::new("ws_rotation_request").unwrap(),
            ResourceId::new("res_rotation_request").unwrap(),
            1,
            1,
            None,
            b"manifest-rotation-request",
            Vec::new(),
        )
        .unwrap();
        let legacy = serde_json::to_value(&manifest).unwrap();
        assert!(matches!(
            serde_json::from_value::<ResourceManifestCommitRequest>(legacy).unwrap(),
            ResourceManifestCommitRequest::Manifest(_)
        ));

        let rotation = serde_json::json!({
            "manifest": manifest,
            "keyShareEnvelopeUpdates": [{
                "memberId": "mem_rotation_recipient",
                "recipientKeyEnvelopes": []
            }]
        });
        let parsed = serde_json::from_value::<ResourceManifestCommitRequest>(rotation).unwrap();
        assert!(matches!(
            parsed,
            ResourceManifestCommitRequest::KeyRotation(input)
                if input.key_share_envelope_updates.len() == 1
        ));
    }

    #[test]
    fn object_reuse_must_match_the_previous_manifest_and_key_version() {
        let workspace_id = WorkspaceId::new("ws_object_reuse").unwrap();
        let resource_id = ResourceId::new("res_object_reuse").unwrap();
        let envelope = ManifestObject::new(
            buglogin_sync::manifest::RESOURCE_KEY_ENVELOPE_KEY,
            b"encrypted-resource-key",
        )
        .unwrap();
        let payload = ManifestObject::new("Default/Cookies", b"ciphertext").unwrap();
        let key_version = "a".repeat(64);
        let encryption = |object: &ManifestObject| ManifestPayloadEncryption {
            schema_version: 1,
            algorithm: PayloadEncryptionAlgorithm::Aes256GcmChunkV2,
            key_version: Some(key_version.clone()),
            resource_key_envelope: ManifestObjectReference::from(object),
        };
        let mut current = SyncManifest::new(
            workspace_id.clone(),
            resource_id.clone(),
            1,
            1,
            None,
            b"generation-one",
            vec![envelope.clone(), payload.clone()],
        )
        .unwrap();
        current.payload_encryption = Some(encryption(&envelope));
        current.validate().unwrap();

        let mut candidate = SyncManifest::new(
            workspace_id.clone(),
            resource_id.clone(),
            1,
            2,
            Some(1),
            b"generation-two",
            vec![envelope.clone(), payload.clone()],
        )
        .unwrap();
        candidate.objects[1].source_generation = Some(1);
        candidate.payload_encryption = Some(encryption(&envelope));
        candidate.validate().unwrap();
        assert!(validate_reused_manifest_objects(&current, &candidate).is_ok());

        candidate.objects[1].checksum = "b".repeat(64);
        assert_eq!(
            validate_reused_manifest_objects(&current, &candidate)
                .unwrap_err()
                .1["code"],
            "manifest_object_reuse_not_authorized"
        );
    }

    #[tokio::test]
    async fn proxy_assignment_and_manifest_activation_commit_atomically() {
        let account = AccountId::new("acct_proxy_atomic").unwrap();
        let workspace = WorkspaceId::new("ws_proxy_atomic").unwrap();
        let profile_id = ProfileId::new("prf_proxy_atomic").unwrap();
        let resource_id = ResourceId::new("res_proxy_atomic").unwrap();
        let proxy_id = ProxyId::new("pxy_proxy_atomic").unwrap();
        let mut store = MemoryControlRecordStore::default();
        let account_value =
            serde_json::to_value(Account::new(account.clone(), "2026-10-13")).unwrap();
        store
            .put(PutControlRecord {
                tenant_id: "control",
                record_type: "account",
                record_id: account.as_str(),
                expected_revision: Some(0),
                idempotency_key: "seed-proxy-atomic-account",
                value: &account_value,
            })
            .unwrap();
        let reopen = store.reopen();
        let state = ApiState::hosted_with_control_store(ProfileService::disabled(), store)
            .unwrap()
            .with_profile_resource(ProfileResource {
                profile_id: profile_id.clone(),
                resource_id: resource_id.clone(),
                workspace_id: workspace.clone(),
                archive_state: ProfileArchiveState::Active,
            });
        state
            .provision_web_identity(
                account.as_str(),
                "proxy.atomic",
                "proxy.atomic@example.test",
                "correct horse battery staple",
            )
            .unwrap();
        let principal = Principal {
            account_id: Some(account.clone()),
            ..Principal::default()
        };
        let read_proxy = Grant {
            id: GrantId::new("grant_proxy_atomic_read").unwrap(),
            account_id: Some(account.clone()),
            member_id: None,
            capability: Capability::new("CAP-NET-001").unwrap(),
            action: Action::Read,
            scope: ResourceScope::workspace(workspace.clone()),
            effect: Effect::Allow,
        };
        let create_proxy = Grant {
            id: GrantId::new("grant_proxy_atomic_create").unwrap(),
            account_id: Some(account.clone()),
            member_id: None,
            capability: Capability::new("CAP-NET-001").unwrap(),
            action: Action::Create,
            scope: ResourceScope::workspace(workspace.clone()),
            effect: Effect::Allow,
        };
        let update_profile = Grant {
            id: GrantId::new("grant_proxy_atomic_profile_update").unwrap(),
            account_id: Some(account.clone()),
            member_id: None,
            capability: Capability::new("profile.update").unwrap(),
            action: Action::Update,
            scope: ResourceScope::resource(workspace.clone(), resource_id.clone()),
            effect: Effect::Allow,
        };
        state.grants.write().unwrap().extend([
            read_proxy.clone(),
            create_proxy.clone(),
            update_profile.clone(),
        ]);
        let grant_snapshot = vec![read_proxy, create_proxy, update_profile];
        let now = checked_now("test").unwrap();
        let token = state
            .api_tokens
            .issue(
                account.clone(),
                1,
                "atomic proxy test",
                std::collections::BTreeSet::from([
                    "profile.update:update".to_owned(),
                    "CAP-NET-001:read".to_owned(),
                ]),
                now.saturating_add(3_600),
                now,
            )
            .unwrap()
            .token;
        state
            .proxies
            .write()
            .unwrap()
            .create(
                workspace.clone(),
                Proxy {
                    id: proxy_id.clone(),
                    name: None,
                    protocol: ProxyProtocol::Http,
                    host: "proxy.example".into(),
                    port: 8080,
                    credential_ref: None,
                    rotation_url: None,
                    bypass_rules: Vec::new(),
                    created_at: "2026-10-13T00:00:00Z".into(),
                    updated_at: "2026-10-13T00:00:00Z".into(),
                },
                &principal,
                &grant_snapshot,
            )
            .unwrap();
        state
            .create_profile_record(Profile {
                id: profile_id.clone(),
                name: "Atomic proxy test".into(),
                runtime: Runtime::Wayfern,
                runtime_version: Some("1.0.0".into()),
                runtime_config: None,
                state: ProfileState::Ready,
                archived: false,
                fingerprint: None,
                note: None,
                proxy_id: None,
                vpn_id: None,
                group_ids: Vec::new(),
                tags: Vec::new(),
                extension_group_id: None,
                sync_mode: Some(SyncMode::Managed),
                created_at: "2026-10-13T00:00:00Z".into(),
                updated_at: "2026-10-13T00:00:00Z".into(),
            })
            .unwrap();
        state
            .profile_proxy_assignments
            .write()
            .unwrap()
            .set(
                &workspace,
                crate::services::profile_proxy_assignment::ProfileProxyAssignmentSetInput {
                    workspace_id: workspace.clone(),
                    profile_id: profile_id.clone(),
                    expected_profile_revision: 1,
                    proxy_id: Some(proxy_id.clone()),
                },
            )
            .unwrap();
        let genesis = SyncManifest::new(
            workspace.clone(),
            resource_id.clone(),
            1,
            1,
            None,
            b"proxy-atomic-genesis",
            Vec::new(),
        )
        .unwrap();
        state.replace_sync_manifest(genesis).unwrap();

        let mut mismatched = SyncManifest::new(
            workspace.clone(),
            resource_id.clone(),
            1,
            2,
            Some(1),
            b"proxy-atomic-mismatch",
            Vec::new(),
        )
        .unwrap();
        let principal = state.authenticate_token(&token, now).unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("x-correlation-id", "proxy-atomic-mismatch".parse().unwrap());
        headers.insert(
            axum::http::header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
        headers.insert(
            "idempotency-key",
            "proxy-atomic-mismatch-key-0001".parse().unwrap(),
        );
        assert!(same_grant_snapshot(
            &grant_snapshot,
            &state.grants().unwrap()
        ));
        let mismatch_response = mutate::<SyncManifest, _>(
            &state,
            &headers,
            &principal,
            &grant_snapshot,
            workspace.as_str(),
            "profile.manifest.commit",
            json!({"expectedProfileRevision": 1, "manifest": mismatched}),
            ResourceScope::resource(workspace.clone(), resource_id.clone()),
            resource_id.as_str(),
            || {
                validate_manifest_successor(&state, &workspace, &resource_id, &mismatched)?;
                activate_pending_profile_proxy_assignment(
                    &state,
                    &workspace,
                    &resource_id,
                    &mismatched,
                    Some(1),
                )?;
                validate_profile_proxy_dependencies(
                    &state,
                    &principal,
                    &grant_snapshot,
                    &workspace,
                    &resource_id,
                    &mismatched,
                    true,
                    false,
                    true,
                )?;
                state
                    .replace_sync_manifest(mismatched.clone())
                    .map_err(response_problem)?;
                Ok(mismatched.clone())
            },
        )
        .await
        .unwrap();
        assert_eq!(mismatch_response.status(), StatusCode::CONFLICT);
        let unchanged = state.get_profile_record(&profile_id).unwrap().unwrap();
        assert_eq!(unchanged.revision, 1);
        assert_eq!(unchanged.value.proxy_id, None);
        assert!(
            state
                .profile_proxy_assignments
                .read()
                .unwrap()
                .get_optional(&workspace, &profile_id)
                .is_some()
        );
        assert_eq!(
            state
                .current_sync_manifest(&workspace, &resource_id)
                .unwrap()
                .unwrap()
                .generation,
            1
        );

        // The profile update permission still allows committing the manifest, but
        // the proxy read grant is revoked after the handler's original snapshot.
        // The mutation boundary must reload ACL state and reject the dependency
        // before the stale grant snapshot captured by the operation can be used.
        state.grants.write().unwrap().retain(|grant| {
            grant.action != Action::Read || grant.capability.as_str() != "CAP-NET-001"
        });
        let mut revoked_dependency_manifest = SyncManifest::new(
            workspace.clone(),
            resource_id.clone(),
            1,
            2,
            Some(1),
            b"proxy-read-revoked",
            Vec::new(),
        )
        .unwrap();
        revoked_dependency_manifest
            .dependencies
            .push(ManifestDependency::Proxy {
                proxy_id: proxy_id.clone(),
                revision: 1,
                credential_version: None,
                metadata_object: None,
                credential_envelope_object: None,
            });
        let mut revoked_headers = HeaderMap::new();
        revoked_headers.insert("x-correlation-id", "proxy-read-revoked".parse().unwrap());
        revoked_headers.insert(
            axum::http::header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
        revoked_headers.insert(
            "idempotency-key",
            "proxy-read-revoked-key-0001".parse().unwrap(),
        );
        let revoked_response = mutate::<SyncManifest, _>(
            &state,
            &revoked_headers,
            &principal,
            &grant_snapshot,
            workspace.as_str(),
            "profile.manifest.commit",
            json!({"expectedProfileRevision": 1, "manifest": revoked_dependency_manifest}),
            ResourceScope::resource(workspace.clone(), resource_id.clone()),
            resource_id.as_str(),
            || {
                state
                    .replace_sync_manifest(revoked_dependency_manifest.clone())
                    .map_err(response_problem)?;
                Ok(revoked_dependency_manifest.clone())
            },
        )
        .await
        .unwrap_err();
        assert_eq!(revoked_response.status(), StatusCode::CONFLICT);
        let revoked_problem = axum::body::to_bytes(revoked_response.into_body(), 4096)
            .await
            .unwrap();
        let revoked_problem: Value = serde_json::from_slice(&revoked_problem).unwrap();
        assert_eq!(revoked_problem["code"], "authorization_state_changed");
        assert_eq!(revoked_problem["retryable"], true);
        assert_eq!(
            state
                .current_sync_manifest(&workspace, &resource_id)
                .unwrap()
                .unwrap()
                .generation,
            1,
            "revoked proxy read access must prevent manifest publication"
        );
        state
            .grants
            .write()
            .unwrap()
            .push(grant_snapshot[0].clone());

        mismatched.dependencies.push(ManifestDependency::Proxy {
            proxy_id: proxy_id.clone(),
            revision: 1,
            credential_version: None,
            metadata_object: None,
            credential_envelope_object: None,
        });
        let mut headers = HeaderMap::new();
        headers.insert("x-correlation-id", "proxy-atomic-success".parse().unwrap());
        headers.insert(
            axum::http::header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
        headers.insert(
            "idempotency-key",
            "proxy-atomic-success-key-0001".parse().unwrap(),
        );
        let committed = mutate::<SyncManifest, _>(
            &state,
            &headers,
            &principal,
            &grant_snapshot,
            workspace.as_str(),
            "profile.manifest.commit",
            json!({"expectedProfileRevision": 1, "manifest": mismatched}),
            ResourceScope::resource(workspace.clone(), resource_id.clone()),
            resource_id.as_str(),
            || {
                validate_manifest_successor(&state, &workspace, &resource_id, &mismatched)?;
                activate_pending_profile_proxy_assignment(
                    &state,
                    &workspace,
                    &resource_id,
                    &mismatched,
                    Some(1),
                )?;
                validate_profile_proxy_dependencies(
                    &state,
                    &principal,
                    &grant_snapshot,
                    &workspace,
                    &resource_id,
                    &mismatched,
                    true,
                    false,
                    true,
                )?;
                state
                    .replace_sync_manifest(mismatched.clone())
                    .map_err(response_problem)?;
                Ok(mismatched.clone())
            },
        )
        .await
        .unwrap();
        assert_eq!(committed.status(), StatusCode::OK);
        let activated = state.get_profile_record(&profile_id).unwrap().unwrap();
        assert_eq!(activated.revision, 2);
        assert_eq!(activated.value.proxy_id, Some(proxy_id.clone()));
        assert!(
            state
                .profile_proxy_assignments
                .read()
                .unwrap()
                .get_optional(&workspace, &profile_id)
                .is_none()
        );
        assert_eq!(
            state
                .current_sync_manifest(&workspace, &resource_id)
                .unwrap()
                .unwrap()
                .generation,
            2
        );

        let reopened =
            ApiState::hosted_with_control_store(ProfileService::disabled(), reopen).unwrap();
        let durable_profile = reopened.get_profile_record(&profile_id).unwrap().unwrap();
        assert_eq!(durable_profile.revision, 2);
        assert_eq!(durable_profile.value.proxy_id, Some(proxy_id));
        assert!(
            reopened
                .profile_proxy_assignments
                .read()
                .unwrap()
                .get_optional(&workspace, &profile_id)
                .is_none()
        );
        assert_eq!(
            reopened
                .current_sync_manifest(&workspace, &resource_id)
                .unwrap()
                .unwrap()
                .generation,
            2
        );
    }
}
