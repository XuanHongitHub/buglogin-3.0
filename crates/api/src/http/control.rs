use axum::{
    Json, Router,
    extract::{Path, Query, Request, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
};
use buglogin_domain::{
    account::{Account, AccountStatus},
    audit::{AuditEvent, AuditOutcome, AuditResource},
    entities::{Profile, Proxy, ProxyProtocol},
    errors::Problem,
    grant::ResourceGrant,
    ids::{
        AccountId, CredentialId, GrantId, MemberId, OrganizationId, ProfileId, ProxyId, ResourceId,
        TeamId, WorkspaceId,
    },
    member::{Member, MemberRole},
    organization::{Organization, OrganizationPolicy},
    permissions::{Action, Capability, Principal, ResourceScope, is_allowed},
    profile::{
        ProfileArchiveState, ProfileCloneInput, ProfileResource, ProfileShareInput,
        ProfileShareRevokeInput,
    },
    resource::Resource,
    team::Team,
    workspace::{OfflineEntitlement, Workspace, WorkspaceUsage},
};
use buglogin_network::proxy::{ProxyDelete, ProxyRecord, ProxyUpdate, RepositoryError};
use buglogin_storage::control::{
    ControlAuditEntry, ControlPut, ExecuteControlCommand, HttpResponse,
};
use buglogin_sync::{
    coordination::{CoordinationAuth, CoordinationScope},
    crypto::{AuthenticatedChunk, ProxyCredentialEnvelope, ResourceKeyEnvelope},
    manifest::{MAX_PROFILE_SYNC_OBJECT_BYTES, ManifestDependency, SyncManifest},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::time::Duration;

use super::{
    ApiState, BlobIdempotencyRecord, RuntimeProjection, authenticate, authorize_action, grants,
    now, problem, response,
};
use crate::services::profile_hosted::HostedProfileService;

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
    mutate(
        &state,
        &headers,
        &principal,
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
) -> Result<Response, Response> {
    let workspace_id = headers
        .get("x-buglogin-workspace-id")
        .and_then(|value| value.to_str().ok())
        .map(WorkspaceId::from_str)
        .transpose()
        .map_err(|_| bad_header("x-buglogin-workspace-id"))?
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
        workspace_id.as_str(),
        "profile.provision",
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
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct AccountStatusInput {
    status: AccountStatus,
    expected_revision: u64,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct OrganizationInput {
    organization: Organization,
    policy: OrganizationPolicy,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct UsageInput {
    profiles: u32,
    members: u32,
    storage_bytes: u64,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
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

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RuntimeProjectionInput {
    device_id: String,
    generation: u64,
    root_checksum: String,
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

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProxyCreateResultView {
    record: ProxyRecordView,
    deduplicated: bool,
}

pub(super) fn routes() -> Router<ApiState> {
    Router::new()
        .route("/v1/account", get(account))
        .route("/v1/accounts", get(list_accounts))
        .route("/v1/accounts/{account_id}/status", put(set_account_status))
        .route(
            "/v1/organizations",
            get(list_organizations).post(create_organization),
        )
        .route(
            "/v1/organizations/{organization_id}",
            get(get_organization).patch(update_organization),
        )
        .route(
            "/v1/organizations/{organization_id}/workspaces",
            get(list_workspaces).post(create_workspace),
        )
        .route(
            "/v1/organizations/{organization_id}/workspaces/{workspace_id}",
            get(get_workspace).patch(update_workspace),
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
            "/v1/workspaces/{workspace_id}/members/{member_id}",
            get(get_member),
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
            "/v1/workspaces/{workspace_id}/resources/{resource_id}",
            get(get_resource),
        )
        .route(
            "/v1/workspaces/{workspace_id}/resources/{resource_id}/lease",
            post(acquire_resource_lease)
                .put(renew_resource_lease)
                .delete(release_resource_lease),
        )
        .route(
            "/v1/workspaces/{workspace_id}/resources/{resource_id}/manifest",
            get(get_resource_manifest).put(commit_resource_manifest),
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
            "/v1/profiles/{profile_id}/runtime/started",
            put(runtime_started),
        )
        .route(
            "/v1/profiles/{profile_id}/runtime/stopped",
            put(runtime_stopped),
        )
        .route(
            "/v1/workspaces/{workspace_id}/grants",
            get(list_resource_grants).post(grant_resource_collection),
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
    let input_value = serde_json::to_value(&input).map_err(internal)?;
    let has_credential = input.proxy.has_credential;
    let mut proxy = proxy_from_metadata(input.proxy.clone()).map_err(response_problem)?;
    proxy.credential_ref = match has_credential {
        None => current.proxy.credential_ref,
        Some(false) => None,
        Some(true) => Some(proxy_credential_id(&proxy.id).map_err(response_problem)?),
    };
    proxy.rotation_url = current.proxy.rotation_url;
    let expected_revision = input.expected_revision;
    mutate(
        &state,
        &headers,
        &principal,
        workspace_id.as_str(),
        "proxy.update",
        input_value,
        ResourceScope::workspace(workspace_id.clone()),
        proxy_id.as_str(),
        || {
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
    let profiles = state.list_profile_records().map_err(response)?;
    if profiles
        .iter()
        .any(|record| record.value.proxy_id.as_ref() == Some(&proxy_id))
    {
        return Err(problem_response(
            "proxy_assigned",
            "Proxy is assigned to a profile",
            409,
            correlation(&headers),
        ));
    }
    let expected_revision = input.expected_revision;
    let input_value = serde_json::to_value(&input).map_err(internal)?;
    mutate(
        &state,
        &headers,
        &principal,
        workspace_id.as_str(),
        "proxy.delete",
        input_value,
        ResourceScope::workspace(workspace_id.clone()),
        proxy_id.as_str(),
        || {
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
        Some(proxy_credential_id(&input.id)?)
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

fn proxy_credential_id(proxy_id: &ProxyId) -> Result<CredentialId, Problem> {
    CredentialId::new(format!("credential-{}", proxy_id.as_str())).map_err(|_| {
        Problem::new(
            "invalid_proxy_credential_id",
            "Proxy credential identifier is invalid",
            400,
            "proxy",
        )
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
    let problem = match error {
        RepositoryError::Forbidden => {
            Problem::new("forbidden", "Proxy access denied", 403, "proxy")
        }
        RepositoryError::InvalidProxy => {
            Problem::new("invalid_proxy", "Proxy is invalid", 400, "proxy")
        }
        RepositoryError::ProxyIdConflict => Problem::new(
            "proxy_conflict",
            "Proxy conflicts with existing inventory",
            409,
            "proxy",
        ),
        RepositoryError::ProxyNotFound | RepositoryError::WorkspaceMismatch => {
            Problem::new("proxy_not_found", "Proxy not found", 404, "proxy")
        }
        RepositoryError::RevisionConflict { expected, actual } => Problem::new(
            "storage_revision_conflict",
            "Storage revision conflict",
            409,
            "storage",
        )
        .with_field("expectedRevision", expected)
        .with_field("actualRevision", actual),
        RepositoryError::RevisionExhausted | RepositoryError::ProxyAssigned => {
            Problem::new("proxy_conflict", "Proxy cannot be changed", 409, "proxy")
        }
        RepositoryError::SnapshotVersionUnsupported { .. }
        | RepositoryError::SnapshotInvalidWorkspace
        | RepositoryError::SnapshotInvalidRevision
        | RepositoryError::SnapshotDuplicateProxyId
        | RepositoryError::SnapshotDuplicateEndpoint => Problem::new(
            "proxy_state_invalid",
            "Proxy state is invalid",
            500,
            "control",
        ),
    };
    response_problem(problem)
}

async fn list_accounts(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<Vec<Account>>, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    authorize_control(
        &principal,
        &grants,
        "CAP-ID-001",
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
    Ok(Json(accounts))
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
    let scope = ResourceScope::account(account_id.clone());
    authorize_control(
        &principal,
        &grants,
        "CAP-ID-001",
        Action::Manage,
        &scope,
        "account_forbidden",
        "Account access denied",
    )?;
    let correlation = correlation(&headers);
    let key = mutation_key(&headers, &correlation)?;
    let actor = principal
        .account_id
        .as_ref()
        .map(AccountId::as_str)
        .unwrap_or("anonymous");
    let store_key = format!("{actor}:account.status:{}:{key}", account_id.as_str());
    let input_value = serde_json::to_value(&input).map_err(internal)?;
    let input_hash = input_hash(&input_value, &correlation)?;
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
        .load(&account_id)
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
        .transition_status(input.expected_revision, input.status, now().to_string())
        .map_err(response)?;
    let body = serde_json::to_value(&updated).map_err(internal)?;
    let result = state
        .control_persistence
        .execute(ExecuteControlCommand {
            tenant_id: "control".into(),
            command_id: "account.status.update".into(),
            idempotency_key: store_key,
            expected_revisions: Default::default(),
            puts: vec![ControlPut {
                tenant_id: "control".into(),
                record_type: "account".into(),
                record_id: account_id.as_str().into(),
                expected_revision: Some(account.revision),
                value: body.clone(),
            }],
            deletes: vec![],
            audit: ControlAuditEntry {
                tenant_id: "control".into(),
                command_id: "account.status.update".into(),
                actor_id: actor.into(),
                allowed: true,
                detail: serde_json::json!({
                    "correlationId": correlation,
                    "accountId": account_id,
                    "status": input.status,
                    "expectedRevision": input.expected_revision,
                    "inputHash": input_hash,
                }),
            },
            response: HttpResponse {
                status: StatusCode::OK.as_u16(),
                headers: std::collections::BTreeMap::from([(
                    "x-buglogin-input-sha256".into(),
                    input_hash,
                )]),
                body,
            },
        })
        .map_err(response)?;
    Ok((StatusCode::OK, Json(result.response.body)).into_response())
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
    let account_id = principal.account_id.ok_or_else(|| {
        response(problem(
            "account_missing",
            "Authenticated account is missing",
            401,
        ))
    })?;
    Ok(Json(AccountView { account_id }))
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
    let value = serde_json::to_value(&input).map_err(internal)?;
    mutate(
        &state,
        &headers,
        &principal,
        workspace_id.as_str(),
        "workspace.update",
        value,
        ResourceScope::workspace(workspace_id.clone()),
        workspace_id.as_str(),
        || {
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
    let input_value = serde_json::to_value(&workspace).map_err(internal)?;
    let scope = ResourceScope::workspace(workspace.id.clone());
    mutate(
        &state,
        &headers,
        &principal,
        workspace.id.as_str(),
        "workspace.create",
        input_value,
        scope,
        workspace.id.as_str(),
        || {
            state
                .workspaces
                .write()
                .map_err(|_| service_unavailable("workspace"))?
                .create_workspace(&principal, &grants, &organization_id, workspace.clone())
                .map_err(response_problem)
                .map(|_| workspace.clone())
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
                .invite(&principal, &grants, member.clone(), now())
                .map_err(response_problem)
                .map(|_| member.clone())
        },
    )
    .await
}

async fn list_members(
    State(state): State<ApiState>,
    Path(workspace_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Vec<Member>>, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let workspace_id = WorkspaceId::from_str(&workspace_id).map_err(|_| bad_id("workspace_id"))?;
    let items = state
        .members
        .read()
        .map_err(|_| service_unavailable("member"))?
        .list(&principal, &grants, &workspace_id)
        .map_err(response_problem)?;
    Ok(Json(items))
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
    let value = serde_json::json!({"workspaceId": workspace_id, "memberId": member_id});
    mutate(
        &state,
        &headers,
        &principal,
        workspace_id.as_str(),
        "member.activate",
        value,
        ResourceScope::workspace(workspace_id.clone()),
        member_id.as_str(),
        || {
            state
                .members
                .write()
                .map_err(|_| service_unavailable("member"))?
                .activate(&principal, &grants, &member_id, now())
                .map_err(response_problem)
                .and_then(|_| {
                    state
                        .members
                        .read()
                        .map_err(|_| service_unavailable("member"))?
                        .get_in_workspace(&principal, &grants, &member_id, &workspace_id)
                        .map_err(response_problem)
                })
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
    let value = serde_json::json!({"workspaceId": workspace_id, "memberId": member_id});
    mutate(
        &state,
        &headers,
        &principal,
        workspace_id.as_str(),
        "member.suspend",
        value,
        ResourceScope::workspace(workspace_id.clone()),
        member_id.as_str(),
        || {
            state
                .members
                .write()
                .map_err(|_| service_unavailable("member"))?
                .suspend(&principal, &grants, &member_id)
                .map_err(response_problem)
                .and_then(|_| {
                    state
                        .members
                        .read()
                        .map_err(|_| service_unavailable("member"))?
                        .get_in_workspace(&principal, &grants, &member_id, &workspace_id)
                        .map_err(response_problem)
                })
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
    let value = serde_json::to_value(&input).map_err(internal)?;
    mutate(
        &state,
        &headers,
        &principal,
        workspace_id.as_str(),
        "member.role.update",
        value,
        ResourceScope::workspace(workspace_id.clone()),
        member_id.as_str(),
        || {
            state
                .members
                .write()
                .map_err(|_| service_unavailable("member"))?
                .set_role(&principal, &grants, &member_id, input.role.clone())
                .map_err(response_problem)
                .and_then(|_| {
                    state
                        .members
                        .read()
                        .map_err(|_| service_unavailable("member"))?
                        .get_in_workspace(&principal, &grants, &member_id, &workspace_id)
                        .map_err(response_problem)
                })
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
) -> Result<Json<Option<RuntimeProjection>>, Response> {
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
    Ok(Json(
        state
            .runtime_projection(&profile_id)
            .map_err(response_problem)?,
    ))
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
        updated_at_ms: now().saturating_mul(1000),
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
    authorize_lease(
        &principal,
        &grants,
        &workspace_id,
        &resource_id,
        "profile.launch",
        Action::Execute,
    )?;
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
    let idempotency_key = require_idempotency(&headers)?;
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
    authorize_lease(
        &principal,
        &grants,
        &workspace_id,
        &resource_id,
        "profile.launch",
        Action::Execute,
    )?;
    let _ = require_idempotency(&headers)?;
    let auth = CoordinationAuth::new(input.owner_id, bearer_token(&headers).to_owned())
        .map_err(response_problem)?;
    let backend = state.coordination().map_err(response_problem)?;
    let lease = backend
        .renew_lease(
            CoordinationScope {
                workspace_id: &workspace_id,
                auth: &auth,
            },
            &resource_id,
            &input.owner_token,
            Duration::from_secs(input.ttl_seconds),
        )
        .await
        .map_err(response_problem)?;
    Ok(Json(lease))
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
        &principal,
        &grants,
        &workspace_id,
        &resource_id,
        "profile.stop",
        Action::Execute,
    )?;
    let _ = require_idempotency(&headers)?;
    let auth = CoordinationAuth::new(input.owner_id, bearer_token(&headers).to_owned())
        .map_err(response_problem)?;
    let backend = state.coordination().map_err(response_problem)?;
    match backend
        .release_lease(
            CoordinationScope {
                workspace_id: &workspace_id,
                auth: &auth,
            },
            &resource_id,
            &input.owner_token,
        )
        .await
    {
        Ok(()) => Ok(Json(serde_json::json!({
            "resourceId": resource_id,
            "released": true
        }))),
        Err(error) if error.code == "lease_not_found" => Ok(Json(serde_json::json!({
            "resourceId": resource_id,
            "released": true
        }))),
        Err(error) => Err(response_problem(error).into()),
    }
}

async fn get_resource_manifest(
    State(state): State<ApiState>,
    Path((workspace_id, resource_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<Option<SyncManifest>>, Response> {
    let principal = authenticate(&state, &headers)?;
    {
        let _refresh = state.control_mutation.lock().map_err(|_| {
            problem_response(
                "control_unavailable",
                "Control state unavailable",
                503,
                correlation(&headers),
            )
        })?;
        if !state.local_authority_enabled() {
            state.reload_control_aggregate().map_err(response_problem)?;
        }
    }
    let grants = grants(&state)?;
    let workspace_id = WorkspaceId::from_str(&workspace_id).map_err(|_| bad_id("workspace_id"))?;
    let resource_id = ResourceId::from_str(&resource_id).map_err(|_| bad_id("resource_id"))?;
    authorize_lease(
        &principal,
        &grants,
        &workspace_id,
        &resource_id,
        "profile.read",
        Action::Read,
    )?;
    let _ = required_header(&headers, "x-buglogin-actor-id")?;
    let _snapshot = state.control_mutation.lock().map_err(|_| {
        problem_response(
            "control_unavailable",
            "Control state unavailable",
            503,
            correlation(&headers),
        )
    })?;
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
        )
        .map_err(Response::from)?;
    }
    Ok(Json(manifest))
}

async fn commit_resource_manifest(
    State(state): State<ApiState>,
    Path((workspace_id, resource_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(manifest): Json<SyncManifest>,
) -> Result<Response, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let workspace_id = WorkspaceId::from_str(&workspace_id).map_err(|_| bad_id("workspace_id"))?;
    let resource_id = ResourceId::from_str(&resource_id).map_err(|_| bad_id("resource_id"))?;
    authorize_lease(
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
    for object in &manifest.objects {
        let bytes = state
            .get_remote_chunk(
                &payload_object_key(
                    &resource_id,
                    manifest.generation,
                    &object.key,
                    &object.checksum,
                ),
                &object.checksum,
            )
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
    let backend = state.coordination().map_err(response_problem)?;
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
    let body = serde_json::to_value(&manifest).map_err(|_| {
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
        workspace_id.as_str(),
        "profile.manifest.commit",
        body,
        ResourceScope::resource(workspace_id.clone(), resource_id.clone()),
        resource_id.as_str(),
        || {
            validate_manifest_successor(&state, &workspace_id, &resource_id, &manifest)?;
            validate_profile_proxy_dependencies(
                &state,
                &principal,
                &grants,
                &workspace_id,
                &resource_id,
                &manifest,
                true,
            )?;
            state
                .replace_sync_manifest(manifest.clone())
                .map_err(response_problem)?;
            Ok(manifest.clone())
        },
    )
    .await
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
                metadata_object, ..
            } => object.key == metadata_object.key,
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
    if !authenticated.is_some_and(|chunk| chunk.version == 1) {
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
            && manifest
                .payload_encryption
                .as_ref()
                .is_none_or(|candidate| {
                    candidate.algorithm
                        != buglogin_sync::manifest::PayloadEncryptionAlgorithm::Aes256GcmChunkV2
                        || candidate.key_version != current_encryption.key_version
                })
        {
            return Err(response_problem(Problem::new(
                "sync_key_rotation_not_supported",
                "The active profile key version cannot change without a key rotation",
                409,
                "sync",
            )));
        }
        if manifest.fencing_token < current.fencing_token
            || manifest.generation != current.generation + 1
            || manifest.parent_generation != Some(current.generation)
        {
            return Err(response_problem(Problem::new(
                "sync_manifest_conflict",
                "Manifest is not a contiguous successor",
                409,
                "sync",
            )));
        }
    } else if manifest.generation != 1 || manifest.parent_generation.is_some() {
        return Err(response_problem(Problem::new(
            "manifest_base_missing",
            "Manifest base is missing",
            409,
            "sync",
        )));
    }
    Ok(())
}

fn validate_profile_proxy_dependencies(
    state: &ApiState,
    principal: &Principal,
    grants: &[buglogin_domain::permissions::Grant],
    workspace_id: &WorkspaceId,
    resource_id: &ResourceId,
    manifest: &SyncManifest,
    verify_metadata_object: bool,
) -> Result<(), ResponseProblem> {
    let profiles = state.list_profile_records().map_err(response_problem)?;
    let mut bound_profile = None;
    for profile in profiles {
        let binding = state
            .profile_resource_record(&profile.value.id)
            .map_err(response_problem)?;
        if binding.as_ref().is_some_and(|binding| {
            &binding.workspace_id == workspace_id && &binding.resource_id == resource_id
        }) {
            if bound_profile.replace(profile.value).is_some() {
                return Err(response_problem(Problem::new(
                    "profile_binding_ambiguous",
                    "Resource has multiple profile bindings",
                    409,
                    "sync",
                )));
            }
        }
    }
    let Some(profile) = bound_profile else {
        if manifest.dependencies.is_empty() {
            return Ok(());
        }
        return Err(response_problem(Problem::new(
            "manifest_dependency_profile_missing",
            "Proxy dependencies require a bound profile",
            409,
            "sync",
        )));
    };

    match profile.proxy_id.as_ref() {
        None if manifest.dependencies.is_empty() => Ok(()),
        None => Err(response_problem(Problem::new(
            "manifest_dependency_mismatch",
            "Manifest contains a proxy dependency that the profile does not select",
            409,
            "sync",
        ))),
        Some(proxy_id) => {
            let [
                ManifestDependency::Proxy {
                    proxy_id: dependency_id,
                    revision,
                    metadata_object,
                    credential_envelope_object,
                },
            ] = manifest.dependencies.as_slice()
            else {
                return Err(response_problem(Problem::new(
                    "manifest_dependency_mismatch",
                    "Manifest must contain exactly the profile's selected proxy dependency",
                    409,
                    "sync",
                )));
            };
            if dependency_id != proxy_id {
                return Err(response_problem(Problem::new(
                    "manifest_dependency_mismatch",
                    "Manifest proxy dependency does not match profile selection",
                    409,
                    "sync",
                )));
            }
            let record = state
                .proxies
                .read()
                .map_err(|_| service_unavailable("proxy"))?
                .get(workspace_id, proxy_id, principal, grants)
                .map_err(proxy_response_problem)?;
            if *revision != record.revision {
                return Err(response_problem(
                    Problem::new(
                        "manifest_dependency_stale",
                        "Proxy dependency revision is stale",
                        409,
                        "sync",
                    )
                    .with_field("expectedRevision", record.revision)
                    .with_field("actualRevision", *revision),
                ));
            }
            if record.proxy.credential_ref.is_some() != credential_envelope_object.is_some() {
                return Err(response_problem(Problem::new(
                    "proxy_credential_envelope_required",
                    "Proxy credential envelope does not match proxy credential state",
                    409,
                    "sync",
                )));
            }
            if verify_metadata_object {
                let metadata_bytes = state
                    .get_remote_chunk(
                        &payload_object_key(
                            resource_id,
                            manifest.generation,
                            &metadata_object.key,
                            &metadata_object.checksum,
                        ),
                        &metadata_object.checksum,
                    )
                    .map_err(response_problem)?;
                let metadata: ProxyRecordView =
                    serde_json::from_slice(&metadata_bytes).map_err(|_| {
                        response_problem(Problem::new(
                            "proxy_metadata_object_invalid",
                            "Proxy metadata object is invalid",
                            400,
                            "sync",
                        ))
                    })?;
                if metadata != proxy_record_view(record) {
                    return Err(response_problem(Problem::new(
                        "proxy_metadata_object_mismatch",
                        "Proxy metadata object does not match the authorized proxy revision",
                        409,
                        "sync",
                    )));
                }
            }
            Ok(())
        }
    }
}

async fn get_resource_blob(
    State(state): State<ApiState>,
    Path((workspace_id, resource_id)): Path<(String, String)>,
    Query(query): Query<BlobQuery>,
    headers: HeaderMap,
) -> Result<Response, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let workspace_id = WorkspaceId::from_str(&workspace_id).map_err(|_| bad_id("workspace_id"))?;
    let resource_id = ResourceId::from_str(&resource_id).map_err(|_| bad_id("resource_id"))?;
    authorize_lease(
        &principal,
        &grants,
        &workspace_id,
        &resource_id,
        "profile.read",
        Action::Read,
    )?;
    let generation = required_header(&headers, "x-buglogin-generation")?
        .parse::<u64>()
        .map_err(|_| bad_header("x-buglogin-generation"))?;
    let expected_checksum = required_header(&headers, "x-content-sha256")?;
    let actor_id = required_header(&headers, "x-buglogin-actor-id")?;
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
    let _ = actor_id;
    let bytes = state
        .get_remote_chunk(
            &payload_object_key(&resource_id, generation, &object.key, &object.checksum),
            &object.checksum,
        )
        .map_err(response_problem)?;
    Ok((
        StatusCode::OK,
        [("content-type", "application/octet-stream")],
        bytes,
    )
        .into_response())
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
        "size": body.len(),
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
        if state
            .get_remote_chunk(&storage_key, expected_checksum)
            .is_ok()
        {
            return Ok(Json(record.receipt).into_response());
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
    let storage_key = payload_object_key(&resource_id, generation, &query.key, expected_checksum);
    state
        .put_remote_chunk(&storage_key, &body, expected_checksum)
        .map_err(response_problem)?;
    let receipt = json!({
        "resourceId": resource_id,
        "key": query.key,
        "generation": generation,
        "checksum": expected_checksum,
        "size": body.len()
    });
    mutate::<serde_json::Value, _>(
        &state,
        &headers,
        &principal,
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
    if !chunk.is_some_and(|chunk| chunk.version == 1) {
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
    principal: &Principal,
    grants: &[buglogin_domain::permissions::Grant],
    workspace_id: &WorkspaceId,
    resource_id: &ResourceId,
    capability_name: &str,
    action: Action,
) -> Result<(), Response> {
    let capability = Capability::new(capability_name).map_err(|_| {
        problem_response(
            "invalid_capability",
            "Invalid capability",
            500,
            "unknown".into(),
        )
    })?;
    if is_allowed(
        principal,
        &capability,
        action,
        &ResourceScope::resource(workspace_id.clone(), resource_id.clone()),
        grants,
    ) {
        Ok(())
    } else {
        Err(problem_response(
            "coordination_forbidden",
            "Coordination access denied",
            403,
            "unknown".into(),
        ))
    }
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

fn required_header<'a>(headers: &'a HeaderMap, name: &'static str) -> Result<&'a str, Response> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| bad_header(name))
}

fn bad_header(name: &'static str) -> Response {
    problem_response(
        "required_header_missing",
        "Required header missing or invalid",
        400,
        "api".to_owned(),
    )
    .with_field(name)
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
                .map_err(response_problem)
                .map(|_| grant.clone())
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
                .map_err(response_problem)
                .map(|_| grant.clone())
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
                .map_err(response_problem)
                .map(|_| serde_json::json!({"revoked": true, "grantId": grant_id}))
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
                .map_err(response_problem)
                .map(|_| serde_json::json!({"revoked": true, "grantId": grant_id}))
        },
    )
    .await
}

async fn read_audit(
    State(state): State<ApiState>,
    Path(workspace_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Vec<AuditEvent>>, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let workspace_id = WorkspaceId::from_str(&workspace_id).map_err(|_| bad_id("workspace_id"))?;
    let events = state
        .audit
        .read()
        .map_err(|_| service_unavailable("audit"))?
        .read(&principal, &grants, &ResourceScope::workspace(workspace_id))
        .map_err(response_problem)?
        .into_iter()
        .cloned()
        .collect();
    Ok(Json(events))
}

async fn mutate<T, F>(
    state: &ApiState,
    headers: &HeaderMap,
    principal: &Principal,
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
    let _mutation = state.control_mutation.lock().map_err(|_| {
        problem_response(
            "control_unavailable",
            "Control state unavailable",
            503,
            correlation(headers),
        )
    })?;
    let correlation = correlation(headers);
    let key = headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .filter(|v| v.is_ascii() && (16..=128).contains(&v.len()))
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
    let store_key = format!("{actor}:{workspace}:{command}:{key}");
    let hash_input = idempotency_input(command, &input);
    let input_hash = format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&hash_input).map_err(|_| problem_response(
                "invalid_input",
                "Input is invalid",
                400,
                correlation.clone()
            ))?
        )
    );
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
    let before = state.control_aggregate().map_err(response_problem)?;
    let result = operation();
    let (status, body, outcome) = match result {
        Ok(value) => (
            StatusCode::OK,
            serde_json::to_value(value).map_err(|_| {
                problem_response(
                    "serialization_failed",
                    "Response serialization failed",
                    500,
                    correlation.clone(),
                )
            })?,
            AuditOutcome::Succeeded,
        ),
        Err(error) => {
            state
                .replace_control_aggregate(before.clone())
                .map_err(response_problem)?;
            (
                StatusCode::from_u16(error.0).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                error.1,
                if error.0 == 403 {
                    AuditOutcome::Denied
                } else {
                    AuditOutcome::Failed
                },
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
            now(),
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
        .persist_control_aggregate(
            command,
            store_key.clone(),
            principal
                .account_id
                .as_ref()
                .map(AccountId::as_str)
                .unwrap_or("anonymous")
                .to_owned(),
            status == StatusCode::OK,
            &input_hash,
            serde_json::json!({
                "correlationId": correlation,
                "resourceId": resource_id,
                "scope": scope,
                "inputHash": input_hash,
            }),
            status.as_u16(),
            body.clone(),
        )
        .map_err(|error| {
            if state.reload_control_aggregate().is_err() {
                let _ = state.replace_control_aggregate(before);
            }
            response_problem(error)
        })?;
    Ok((status, Json(body)).into_response())
}

fn idempotency_input(command: &str, input: &serde_json::Value) -> serde_json::Value {
    if command != "profile.provision" {
        return input.clone();
    }
    let mut canonical = input.clone();
    if let Some(profile) = canonical
        .get_mut("profile")
        .and_then(serde_json::Value::as_object_mut)
    {
        profile.remove("createdAt");
        profile.remove("updatedAt");
    }
    canonical
}

#[derive(Debug)]
struct ResponseProblem(u16, serde_json::Value);

impl From<ResponseProblem> for Response {
    fn from(error: ResponseProblem) -> Self {
        (
            StatusCode::from_u16(error.0).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            Json(error.1),
        )
            .into_response()
    }
}
fn response_problem(error: buglogin_domain::errors::Problem) -> ResponseProblem {
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
    use buglogin_browser::profile::ProfileService;
    use buglogin_sync::manifest::{
        ManifestObject, ManifestObjectReference, ManifestPayloadEncryption,
        PayloadEncryptionAlgorithm,
    };

    #[test]
    fn active_v2_manifest_rejects_key_version_change() {
        let state = ApiState::local(ProfileService::disabled());
        let workspace_id = WorkspaceId::new("ws_key_version").unwrap();
        let resource_id = ResourceId::new("res_key_version").unwrap();
        let envelope = ManifestObject::new(
            buglogin_sync::manifest::RESOURCE_KEY_ENVELOPE_KEY,
            b"encrypted-resource-key",
        )
        .unwrap();
        let encryption = |key_version: &str| ManifestPayloadEncryption {
            schema_version: 1,
            algorithm: PayloadEncryptionAlgorithm::Aes256GcmChunkV2,
            key_version: Some(key_version.to_owned()),
            resource_key_envelope: ManifestObjectReference::from(&envelope),
        };
        let mut current = SyncManifest::new(
            workspace_id.clone(),
            resource_id.clone(),
            1,
            1,
            None,
            b"generation-one",
            vec![envelope.clone()],
        )
        .unwrap();
        current.payload_encryption = Some(encryption(&"a".repeat(64)));
        current.validate().unwrap();
        state.replace_sync_manifest(current).unwrap();

        let mut candidate = SyncManifest::new(
            workspace_id.clone(),
            resource_id.clone(),
            1,
            2,
            Some(1),
            b"generation-two",
            vec![envelope.clone()],
        )
        .unwrap();
        candidate.payload_encryption = Some(encryption(&"b".repeat(64)));
        candidate.validate().unwrap();

        let error = validate_manifest_successor(&state, &workspace_id, &resource_id, &candidate)
            .unwrap_err();
        assert_eq!(error.1["code"], "sync_key_rotation_not_supported");
    }
}
