use axum::{
    Json, Router,
    extract::{Path, Query, Request, State},
    http::{HeaderMap, Method, StatusCode},
    middleware::Next,
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
        AccountId, CredentialId, GrantId, GroupId, MemberId, OrganizationId, ProfileId, ProxyId,
        ResourceId, TeamId, WorkspaceId,
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
    manifest::{MAX_PROFILE_SYNC_OBJECT_BYTES, ManifestDependency, ManifestObject, SyncManifest},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
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
            "/v1/workspaces/{workspace_id}/groups",
            get(list_groups).post(create_group),
        )
        .route(
            "/v1/workspaces/{workspace_id}/groups/{group_id}",
            get(get_group).put(update_group).delete(delete_group),
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
            "/v1/profiles/{profile_id}/runtime/started",
            put(runtime_started),
        )
        .route(
            "/v1/profiles/{profile_id}/runtime/stopped",
            put(runtime_stopped),
        )
        .route(
            "/v1/profiles/{profile_id}/groups",
            put(replace_profile_groups),
        )
        .route(
            "/v1/workspaces/{workspace_id}/profiles/{profile_id}/proxy-assignment",
            get(get_profile_proxy_assignment).put(set_profile_proxy_assignment),
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

pub(super) async fn refresh_hosted_snapshot(
    State(state): State<ApiState>,
    request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path();
    let control_backed_read = request.method() == Method::GET
        && !matches!(path, "/v1/health" | "/v1/readyz")
        && !path.starts_with("/v1/auth/")
        && !path.starts_with("/v1/operations/")
        && path != "/v1/events"
        && !path.starts_with("/v1/cookie-jobs/");
    if !control_backed_read {
        return next.run(request).await;
    }
    let _snapshot = std::sync::Arc::clone(&state.control_snapshot_gate)
        .read_owned()
        .await;
    if let Err(error) = state.refresh_control_aggregate() {
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
        &grants,
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
        &grants,
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
            resource_id: Some(resource_id),
            profile_id: Some(profile_id.clone()),
            ..ResourceScope::default()
        },
        profile_id.as_str(),
        || {
            let updated = state
                .replace_hosted_profile_groups(input.clone())
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
    if assignment.expected_profile_revision != profile.revision {
        return Err(problem_response(
            "profile_proxy_assignment_stale",
            "Profile proxy assignment is stale",
            409,
            correlation(&headers),
        ));
    }
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
    crate::services::profile_proxy_assignment::ProfileProxyAssignmentService::validate_change(
        &profile.value,
        profile.revision,
        &input,
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
            crate::services::profile_proxy_assignment::ProfileProxyAssignmentService::validate_change(
                &current.value,
                current.revision,
                &input,
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
    let input_value = serde_json::to_value(&workspace).map_err(internal)?;
    let scope = ResourceScope::workspace(workspace.id.clone());
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
        &grants,
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
        &grants,
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
        &grants,
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
    authorize_lease(
        &state,
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
        &state,
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
        &state,
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
    let _ = required_header(&headers, "x-buglogin-actor-id")?;
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
    validate_manifest_source_references(&state, &workspace_id, &resource_id, &manifest)?;
    for object in &manifest.objects {
        let bytes = state
            .get_remote_chunk(
                &payload_object_key(
                    &resource_id,
                    object_storage_generation(manifest.generation, object),
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
    let body = serde_json::json!({
        "expectedProfileRevision": expected_profile_revision,
        "manifest": &manifest,
    });
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
        current
            .validate_successor(manifest, now().saturating_mul(1000))
            .map_err(response_problem)?;
        validate_reused_manifest_objects(current, manifest)?;
    } else if manifest.generation != 1
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
    Ok(())
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
        }) {
            if bound_profile.replace(profile).is_some() {
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
    if profile.value.archived
        || profile.value.state == buglogin_domain::entities::ProfileState::Running
    {
        return Err(response_problem(Problem::new(
            "profile_proxy_assignment_not_allowed",
            "Proxy assignment requires a stopped active profile",
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
    updated.updated_at = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|_| {
            response_problem(Problem::new(
                "clock_unavailable",
                "Clock unavailable",
                500,
                "sync",
            ))
        })?;
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
            if verify_metadata_object && let Some(metadata_object) = metadata_object {
                let metadata_bytes = state
                    .get_remote_chunk(
                        &payload_object_key(
                            resource_id,
                            referenced_object_storage_generation(manifest, metadata_object)
                                .ok_or_else(|| {
                                    response_problem(Problem::new(
                                        "manifest_dependency_object_missing",
                                        "Proxy metadata object is not in the manifest",
                                        409,
                                        "sync",
                                    ))
                                })?,
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
            &payload_object_key(
                &resource_id,
                object_storage_generation(generation, object),
                &object.key,
                &object.checksum,
            ),
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
        .remote_chunk_checksum(&storage_key)
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
    state: &ApiState,
    principal: &Principal,
    grants: &[buglogin_domain::permissions::Grant],
    workspace_id: &WorkspaceId,
    resource_id: &ResourceId,
    capability_name: &str,
    action: Action,
) -> Result<(), Response> {
    if let Some(binding) = state
        .profile_resource_for_resource(workspace_id, resource_id)
        .map_err(response_problem)?
        && crate::services::profile_hosted::HostedProfileService::authorized(
            state,
            principal,
            grants,
            &binding.profile_id,
            action,
            capability_name,
        )
        .map_err(response_problem)?
    {
        return Ok(());
    }
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
                .map_err(response_problem)
                .map(|_| serde_json::json!({"revoked": true, "grantId": grant_id}))
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
    let _snapshot = std::sync::Arc::clone(&state.control_snapshot_gate)
        .write_owned()
        .await;
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
        "profile.proxy.assignment.set" | "profile.manifest.commit"
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
        let allowed = if let Some(profile_id) = scope.profile_id.as_ref() {
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
        } else {
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
    let success_status = if command == "profile.create" {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    let (status, body, outcome) = match result {
        Ok(value) => (
            success_status,
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
            status.is_success(),
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
    if !matches!(command, "profile.create" | "profile.provision") {
        return input.clone();
    }
    let mut canonical = input.clone();
    let Some(profile) = canonical
        .get_mut("profile")
        .and_then(serde_json::Value::as_object_mut)
    else {
        return input.clone();
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
        (
            StatusCode::from_u16(error.0).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            Json(error.1),
        )
            .into_response()
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
    use axum::{body::Body, http::Request};
    use buglogin_browser::profile::ProfileService;
    use buglogin_domain::{
        account::Account,
        entities::{ProfileState, Runtime, SyncMode},
        grant::ResourceGrant,
        member::{Member, MemberRole, MemberStatus},
        permissions::{Effect, Grant},
        profile::{ProfileArchiveState, ProfileResource},
        resource::{ResourceKind, ResourceState},
    };
    use buglogin_storage::control::{
        ControlRecordStore, MemoryControlRecordStore, PutControlRecord,
    };
    use buglogin_sync::manifest::{
        ManifestObject, ManifestObjectReference, ManifestPayloadEncryption,
        PayloadEncryptionAlgorithm,
    };
    use tower::ServiceExt;

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
        let now = now();
        let token = state
            .api_tokens
            .issue(
                account.clone(),
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
