#[cfg(test)]
use std::time::{SystemTime, UNIX_EPOCH};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    ops::Bound::{Excluded, Unbounded},
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock, RwLock},
    time::Duration,
};

use async_trait::async_trait;
use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Path, Query, RawQuery, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{
        IntoResponse, Response,
        sse::{Event as SseEvent, KeepAlive, Sse},
    },
    routing::{get, post, put},
};
use buglogin_browser::{
    cookies::{
        CookieScope,
        authority::{CookieAuthority, ProfileRuntimeState},
    },
    lifecycle::Lifecycle,
    profile::ProfileService,
    runtime::RuntimeBinary,
};
use buglogin_domain::{
    account::{Account, AccountIdentity, AccountStatus},
    entities::{Profile, ProfileCreateInput, ProfileState, Runtime},
    errors::Problem,
    ids::{
        AccountId, DeviceId, ExtensionGroupId, GrantId, GroupId, MemberId, OrganizationId,
        ProfileId, WorkspaceId,
    },
    member::{Member, MemberRole, MemberStatus},
    organization::{Organization, OrganizationPolicy},
    permissions::{Action, Capability, Effect, Grant, Principal, ResourceScope, Role, is_allowed},
    profile::{
        ConfirmedProfileKeyShareEnvelope, ProfileArchiveState, ProfileCloneInput,
        ProfileKeyShareEnvelope, ProfileResource, ProfileShareInput, ProfileShareRevokeInput,
    },
    workspace::{Workspace, WorkspacePolicy},
};
use buglogin_migration::{
    MigrationKey, activate_package,
    cutover::{confirm_cutover, promote_staged},
    inspect::inspect_source,
    package::{NeutralPackage, PackageMetadata},
    rollback::{confirm_rollback, rollback_activation, rollback_cutover},
    signature::{SignatureMetadata, sign_package, verify_signed_package},
    transform::stage_package,
};
use buglogin_network::proxy::{
    ParseReport, ProxyRepository, ProxyRepositorySnapshot, RepositoryError, parse_lines,
};
use buglogin_storage::{
    local::Versioned,
    remote::{
        ConcurrentRemoteChunkStore, MemoryRemoteChunkStore, RemoteChunkStore,
        SerializedRemoteChunkStore,
    },
    vault::NativeVault,
};
use buglogin_sync::coordination::{
    AuthorizationRequest, CoordinationAuth, CoordinationAuthorizer, CoordinationBackend,
    CoordinationOperation, CoordinationScope,
};
use buglogin_sync::manifest::SyncManifest;
use futures_util::{Stream, stream};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tower_http::cors::{AllowOrigin, CorsLayer};
use url::Url;

use crate::{
    auth::AuthService,
    auth::SessionSummary,
    services::{
        account_persistence::AccountPersistence,
        api_tokens::{ApiTokenService, ApiTokenSummary},
        audit::AuditService,
        connected_cookie_jobs::{
            ConnectedCookieFailureCode, ConnectedCookieJob, ConnectedCookieJobService,
            ConnectedCookieJobState, ConnectedCookieReceipt, EnqueueConnectedCookieJob,
        },
        control_persistence::ControlPersistence,
        credentials::CredentialService,
        device_authorization::DeviceAuthorizationService,
        device_key_directory::{DeviceEncryptionKey, DeviceKeyDirectoryService},
        events::EventService,
        extension_groups::ExtensionGroupService,
        groups::GroupService,
        identity_persistence::IdentityPersistence,
        members::MemberService,
        operations::{Operation, OperationService, OperationStatus},
        profile_creation::ProfileCreationService,
        profile_hosted::HostedProfileService,
        recovery::{RecoveryDelivery, RecoveryService},
        resources::ResourceService,
        runtime_catalog::PublishedRuntimeCatalog,
        runtime_dispatch::RuntimeDispatchService,
        session_persistence::SessionPersistence,
        teams::TeamService,
        workspaces::WorkspaceService,
    },
};
#[cfg(test)]
use buglogin_storage::control::ControlIdempotencyClaimToken;
use buglogin_storage::control::{
    ControlAuditEntry, ControlPut, ControlRecordKey, ExecuteControlCommand, HttpResponse,
};
use serde_json::json;
use zeroize::Zeroize;

#[path = "../services/profile_lifecycle.rs"]
pub mod profile_lifecycle;
use profile_lifecycle::{ProfileLifecycleCommand, ProfileLifecycleService};

tokio::task_local! {
    static REQUEST_CORRELATION_ID: String;
}

#[cfg(test)]
mod auth_workspace_profile_e2e;
mod control;
mod control_import;
pub(crate) mod package_upload;
mod sso;
mod sync_import;

const CONTROL_TENANT: &str = "control";
const CONNECTED_COOKIE_JOB_TTL_SECONDS: u64 = 600;
const PROFILE_QUARANTINE_RETENTION_MS: u64 = 30 * 24 * 60 * 60 * 1_000;
const SYNC_BLOB_RESERVATION_TTL_MS: u64 = 24 * 60 * 60 * 1_000;
const CONTROL_STATE_TYPE: &str = "control_state";
const CONTROL_STATE_ID: &str = "primary";

#[derive(Clone, Default, Deserialize, Serialize)]
pub(crate) struct ControlAggregate {
    grants: Vec<Grant>,
    #[serde(default)]
    pub(crate) operations: OperationService,
    #[serde(default)]
    materialized_member_grants: BTreeSet<MemberId>,
    #[serde(default)]
    platform_admin_accounts: BTreeSet<AccountId>,
    #[serde(default)]
    profiles: BTreeMap<ProfileId, Versioned<Profile>>,
    #[serde(default)]
    profile_quarantine: BTreeMap<ProfileId, ProfileQuarantineRecord>,
    #[serde(default)]
    profile_resources: BTreeMap<ProfileId, ProfileResource>,
    #[serde(default)]
    sync_manifests: BTreeMap<String, SyncManifest>,
    #[serde(default)]
    sync_gc_claims: BTreeMap<String, BTreeSet<String>>,
    #[serde(default)]
    resource_key_rotation_pending: BTreeSet<String>,
    #[serde(default)]
    profile_key_share_envelopes: BTreeMap<String, StoredProfileKeyShareEnvelope>,
    #[serde(default)]
    blob_idempotency: BTreeMap<String, BlobIdempotencyRecord>,
    #[serde(default)]
    sync_blob_reservations: BTreeMap<String, SyncBlobReservation>,
    #[serde(default)]
    runtime_projections: BTreeMap<ProfileId, RuntimeProjection>,
    #[serde(default)]
    proxies: ProxyRepositorySnapshot,
    workspaces: WorkspaceService,
    members: MemberService,
    resources: ResourceService,
    #[serde(default)]
    groups: GroupService,
    #[serde(default)]
    extension_groups: ExtensionGroupService,
    #[serde(default)]
    profile_proxy_assignments:
        crate::services::profile_proxy_assignment::ProfileProxyAssignmentService,
    teams: TeamService,
    audit: AuditService,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProfileQuarantineRecord {
    deleted_at_unix_ms: u64,
    retention_deadline_unix_ms: u64,
    deleted_by: Option<AccountId>,
    #[serde(default)]
    was_archived: bool,
}

impl ProfileQuarantineRecord {
    pub(crate) fn deleted_by(&self) -> Option<&AccountId> {
        self.deleted_by.as_ref()
    }

    pub(crate) fn retention_deadline_unix_ms(&self) -> u64 {
        self.retention_deadline_unix_ms
    }

    pub(crate) fn was_archived(&self) -> bool {
        self.was_archived
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct StoredProfileKeyShareEnvelope {
    pub profile_id: ProfileId,
    pub workspace_id: WorkspaceId,
    pub resource_id: buglogin_domain::ids::ResourceId,
    pub member_id: MemberId,
    pub envelope: ProfileKeyShareEnvelope,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SyncMigrationJournalState {
    Staging,
    Finalizing,
    Active,
    CleanupPending,
    Abandoned,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SyncMigrationJournalObject {
    pub storage_key: String,
    pub checksum: String,
    pub size: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MigrationRecoveryReport {
    pub scanned: usize,
    pub recovered: usize,
    pub deferred: usize,
    pub failed: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SyncMigrationJournal {
    pub schema: String,
    pub operation_id: String,
    pub export_id: String,
    pub export_sha256: String,
    pub manifest_sha256: String,
    pub blob_set_sha256: String,
    pub workspace_id: String,
    pub resource_id: String,
    pub base_generation: Option<u64>,
    pub target_generation: u64,
    pub state: SyncMigrationJournalState,
    pub created_objects: Vec<SyncMigrationJournalObject>,
    pub resumed_objects: Vec<SyncMigrationJournalObject>,
    #[serde(default)]
    pub reserved_objects: Vec<SyncMigrationJournalObject>,
    pub destination_fencing_token: Option<u64>,
    pub control_revision_before: u64,
    pub control_revision_after: Option<u64>,
    pub updated_at_ms: u64,
    pub error_code: Option<String>,
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BlobIdempotencyRecord {
    pub request_hash: String,
    pub receipt: serde_json::Value,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SyncBlobReservation {
    pub workspace_id: String,
    pub resource_id: String,
    pub storage_key: String,
    pub checksum: String,
    pub size: u64,
    pub expires_at_ms: u64,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RuntimeProjection {
    pub profile_id: ProfileId,
    pub device_id: String,
    pub owner_id: String,
    pub state: String,
    pub generation: u64,
    pub root_checksum: String,
    pub updated_at_ms: u64,
}

#[derive(Clone)]
pub struct ApiState {
    pub profiles: ProfileService,
    pub auth: Arc<RwLock<AuthService>>,
    pub(crate) device_authorization: DeviceAuthorizationService,
    pub(crate) device_key_directory: DeviceKeyDirectoryService,
    api_tokens: ApiTokenService,
    device_verification_uri: Arc<str>,
    web_origin: Arc<str>,
    pub grants: Arc<RwLock<Vec<Grant>>>,
    pub operations: Arc<RwLock<OperationService>>,
    pub events: Arc<RwLock<EventService>>,
    pub workspaces: Arc<RwLock<WorkspaceService>>,
    pub members: Arc<RwLock<MemberService>>,
    pub resources: Arc<RwLock<ResourceService>>,
    pub groups: Arc<RwLock<GroupService>>,
    pub extension_groups: Arc<RwLock<ExtensionGroupService>>,
    pub profile_proxy_assignments:
        Arc<RwLock<crate::services::profile_proxy_assignment::ProfileProxyAssignmentService>>,
    hosted_profiles: Arc<RwLock<BTreeMap<ProfileId, Versioned<Profile>>>>,
    profile_quarantine: Arc<RwLock<BTreeMap<ProfileId, ProfileQuarantineRecord>>>,
    profile_resources: Arc<RwLock<BTreeMap<ProfileId, ProfileResource>>>,
    profile_resource_index:
        Arc<RwLock<BTreeMap<(WorkspaceId, buglogin_domain::ids::ResourceId), BTreeSet<ProfileId>>>>,
    sync_manifests: Arc<RwLock<BTreeMap<String, SyncManifest>>>,
    sync_gc_claims: Arc<RwLock<BTreeMap<String, BTreeSet<String>>>>,
    resource_key_rotation_pending: Arc<RwLock<BTreeSet<String>>>,
    profile_key_share_envelopes: Arc<RwLock<BTreeMap<String, StoredProfileKeyShareEnvelope>>>,
    blob_idempotency: Arc<RwLock<BTreeMap<String, BlobIdempotencyRecord>>>,
    sync_blob_reservations: Arc<RwLock<BTreeMap<String, SyncBlobReservation>>>,
    runtime_projections: Arc<RwLock<BTreeMap<ProfileId, RuntimeProjection>>>,
    pub proxies: Arc<RwLock<ProxyRepository>>,
    pub teams: Arc<RwLock<TeamService>>,
    pub connected_cookie_jobs: ConnectedCookieJobService,
    pub runtime_dispatch: RuntimeDispatchService,
    pub audit: Arc<RwLock<AuditService>>,
    local_cookies: Option<Arc<Mutex<CookieAuthority<NativeVault>>>>,
    account_persistence: AccountPersistence,
    identity_persistence: IdentityPersistence,
    credentials: CredentialService,
    pub(crate) recovery: RecoveryService,
    pub(crate) control_persistence: ControlPersistence,
    control_persistence_mode: &'static str,
    control_revision: Arc<RwLock<u64>>,
    control_mutation: Arc<Mutex<()>>,
    pub(crate) control_snapshot_gate: Arc<tokio::sync::RwLock<()>>,
    materialized_member_grants: Arc<RwLock<BTreeSet<MemberId>>>,
    platform_admin_accounts: Arc<RwLock<BTreeSet<AccountId>>>,
    local_account_provisioning: bool,
    external_object_storage: bool,
    require_runtime_catalog_for_readiness: bool,
    pub lifecycles: Arc<RwLock<HashMap<String, Lifecycle>>>,
    pub runtime_binaries: Arc<RwLock<HashMap<String, RuntimeBinary>>>,
    runtime_catalog: Option<Arc<PublishedRuntimeCatalog>>,
    migration_key: Arc<RwLock<Option<MigrationKey>>>,
    migration_signing_key: Arc<RwLock<Option<buglogin_storage::secrets::SecretValue>>>,
    coordination: Arc<RwLock<Option<Arc<dyn CoordinationBackend>>>>,
    remote_chunks: Arc<dyn ConcurrentRemoteChunkStore>,
}

pub struct FirstOwnerBootstrapInput {
    pub account_id: Option<String>,
    pub username: String,
    pub email: String,
    pub operator_attested_email: bool,
    pub password: zeroize::Zeroizing<String>,
    pub organization_name: String,
    pub workspace_name: String,
    pub max_workspaces: u32,
    pub max_profiles: u32,
    pub max_members: u32,
    pub max_storage_bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperatorPasswordResetResult {
    pub account_id: AccountId,
    pub revoked_sessions: usize,
    pub revoked_api_tokens: usize,
}

pub struct OperatorPlatformAdminProvisionInput {
    pub username: String,
    pub email: String,
    pub password: zeroize::Zeroizing<String>,
    pub organization_name: String,
    pub workspace_name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperatorPlatformAdminProvisionResult {
    pub account_id: AccountId,
    pub organization_id: OrganizationId,
    pub workspace_id: WorkspaceId,
}

pub struct LocalDevAdminProvisionInput {
    pub username: String,
    pub email: String,
    pub password: zeroize::Zeroizing<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalDevAdminProvisionResult {
    pub account_id: AccountId,
    pub organization_id: OrganizationId,
    pub workspace_id: WorkspaceId,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OperatorPlatformAdmin {
    pub account_id: AccountId,
    pub username: Option<String>,
    pub email: Option<String>,
    pub status: AccountStatus,
    pub identity_available: bool,
}

#[derive(Debug)]
pub struct FirstOwnerBootstrapResult {
    pub account_id: AccountId,
    pub organization_id: OrganizationId,
    pub workspace_id: WorkspaceId,
    pub created_account: bool,
    pub created_organization: bool,
    pub created_workspace: bool,
}

#[derive(Clone)]
struct ApiAuthorizationAuthority {
    auth: Arc<RwLock<AuthService>>,
    api_tokens: ApiTokenService,
    account_persistence: AccountPersistence,
    members: Arc<RwLock<MemberService>>,
    platform_admin_accounts: Arc<RwLock<BTreeSet<AccountId>>>,
    grants: Arc<RwLock<Vec<Grant>>>,
    materialized_member_grants: Arc<RwLock<BTreeSet<MemberId>>>,
}

impl ApiState {
    pub fn new(profiles: ProfileService) -> Self {
        Self::local(profiles)
    }

    pub fn local(profiles: ProfileService) -> Self {
        Self::try_with_account_store_mode(
            profiles,
            buglogin_storage::control::MemoryControlRecordStore::default(),
            true,
            "memory",
        )
        .expect("memory control store must initialize")
    }

    pub fn local_with_control_store<S>(profiles: ProfileService, store: S) -> Result<Self, Problem>
    where
        S: buglogin_storage::control::ControlRecordStore + Send + 'static,
    {
        Self::try_with_account_store_mode(profiles, store, true, "sqlite")
    }

    /// Runs hosted account/profile/sync authority with local SQLite control storage.
    /// This is for loopback development that must exercise the same REST semantics as production.
    pub fn local_hosted_with_control_store<S>(store: S) -> Result<Self, Problem>
    where
        S: buglogin_storage::control::ControlRecordStore + Send + 'static,
    {
        Self::try_with_account_store_mode(ProfileService::disabled(), store, false, "sqlite")
    }

    /// Runs the loopback API against PostgreSQL using the same hosted control authority.
    pub fn local_hosted_with_postgres_control_store<S>(store: S) -> Result<Self, Problem>
    where
        S: buglogin_storage::control::ControlRecordStore + Send + 'static,
    {
        Self::try_with_account_store_mode(ProfileService::disabled(), store, false, "postgres")
    }

    pub fn hosted_with_control_store<S>(profiles: ProfileService, store: S) -> Result<Self, Problem>
    where
        S: buglogin_storage::control::ControlRecordStore + Send + 'static,
    {
        Self::try_with_account_store(profiles, store, false)
    }

    pub fn production(config: buglogin_storage::control::PostgresConfig) -> Result<Self, Problem> {
        let mut store = buglogin_storage::control::PostgresControlRecordStore::connect(&config)?;
        store.ensure_schema()?;
        let mut state = Self::try_with_account_store_mode(
            ProfileService::disabled(),
            store,
            false,
            "postgres",
        )?;
        state.require_runtime_catalog_for_readiness = true;
        Ok(state)
    }

    fn try_with_account_store<S>(
        profiles: ProfileService,
        store: S,
        local_account_provisioning: bool,
    ) -> Result<Self, Problem>
    where
        S: buglogin_storage::control::ControlRecordStore + Send + 'static,
    {
        Self::try_with_account_store_mode(profiles, store, local_account_provisioning, "custom")
    }

    fn try_with_account_store_mode<S>(
        profiles: ProfileService,
        store: S,
        local_account_provisioning: bool,
        control_persistence_mode: &'static str,
    ) -> Result<Self, Problem>
    where
        S: buglogin_storage::control::ControlRecordStore + Send + 'static,
    {
        let control_persistence = ControlPersistence::new(store);
        let (aggregate, revision) = control_persistence
            .load_versioned::<ControlAggregate>(
                CONTROL_TENANT,
                CONTROL_STATE_TYPE,
                CONTROL_STATE_ID,
            )?
            .unwrap_or_default();
        let proxies = ProxyRepository::restore(aggregate.proxies.clone())
            .map_err(proxy_repository_problem)?;
        let identity_persistence = IdentityPersistence::new(control_persistence.clone());
        let credentials = CredentialService::new(control_persistence.clone())?;
        let recovery = RecoveryService::new(control_persistence.clone(), credentials.clone());
        let api_tokens = ApiTokenService::new(control_persistence.clone(), credentials.clone());
        let profile_resource_index = build_profile_resource_index(&aggregate.profile_resources);
        let local_cookies = if local_account_provisioning {
            profiles
                .repository_root()
                .ok()
                .map(|root| {
                    CookieAuthority::open(NativeVault::new(), root.join("cookies"))
                        .map(|authority| Arc::new(Mutex::new(authority)))
                })
                .transpose()
                .map_err(|error| {
                    Problem::new(
                        "cookie_authority_unavailable",
                        error.to_string(),
                        503,
                        "api",
                    )
                })?
        } else {
            None
        };
        Ok(Self {
            profiles,
            auth: Arc::new(RwLock::new(AuthService::persistent_with_credentials(
                SessionPersistence::new(control_persistence.clone()),
                credentials.clone(),
            )?)),
            device_authorization: DeviceAuthorizationService::new(control_persistence.clone()),
            device_key_directory: DeviceKeyDirectoryService::new(control_persistence.clone()),
            api_tokens,
            device_verification_uri: Arc::from("http://127.0.0.1:3000/device"),
            web_origin: Arc::from("http://127.0.0.1:3000"),
            grants: Arc::new(RwLock::new(aggregate.grants)),
            materialized_member_grants: Arc::new(RwLock::new(aggregate.materialized_member_grants)),
            platform_admin_accounts: Arc::new(RwLock::new(aggregate.platform_admin_accounts)),
            operations: Arc::new(RwLock::new(aggregate.operations)),
            events: Arc::new(RwLock::new(EventService::default())),
            workspaces: Arc::new(RwLock::new(aggregate.workspaces)),
            members: Arc::new(RwLock::new(aggregate.members)),
            resources: Arc::new(RwLock::new(aggregate.resources)),
            groups: Arc::new(RwLock::new(aggregate.groups)),
            extension_groups: Arc::new(RwLock::new(aggregate.extension_groups)),
            profile_proxy_assignments: Arc::new(RwLock::new(aggregate.profile_proxy_assignments)),
            hosted_profiles: Arc::new(RwLock::new(aggregate.profiles)),
            profile_quarantine: Arc::new(RwLock::new(aggregate.profile_quarantine)),
            profile_resources: Arc::new(RwLock::new(aggregate.profile_resources)),
            profile_resource_index: Arc::new(RwLock::new(profile_resource_index)),
            sync_manifests: Arc::new(RwLock::new(aggregate.sync_manifests)),
            sync_gc_claims: Arc::new(RwLock::new(aggregate.sync_gc_claims)),
            resource_key_rotation_pending: Arc::new(RwLock::new(
                aggregate.resource_key_rotation_pending,
            )),
            profile_key_share_envelopes: Arc::new(RwLock::new(
                aggregate.profile_key_share_envelopes,
            )),
            blob_idempotency: Arc::new(RwLock::new(aggregate.blob_idempotency)),
            sync_blob_reservations: Arc::new(RwLock::new(aggregate.sync_blob_reservations)),
            runtime_projections: Arc::new(RwLock::new(aggregate.runtime_projections)),
            proxies: Arc::new(RwLock::new(proxies)),
            teams: Arc::new(RwLock::new(aggregate.teams)),
            connected_cookie_jobs: ConnectedCookieJobService::new(control_persistence.clone()),
            runtime_dispatch: RuntimeDispatchService::new(control_persistence.clone()),
            audit: Arc::new(RwLock::new(aggregate.audit)),
            local_cookies,
            account_persistence: AccountPersistence::new(control_persistence.clone()),
            identity_persistence,
            credentials,
            recovery,
            control_persistence,
            control_persistence_mode,
            control_revision: Arc::new(RwLock::new(revision)),
            control_mutation: Arc::new(Mutex::new(())),
            control_snapshot_gate: Arc::new(tokio::sync::RwLock::new(())),
            local_account_provisioning,
            external_object_storage: false,
            require_runtime_catalog_for_readiness: false,
            lifecycles: Arc::new(RwLock::new(HashMap::new())),
            runtime_binaries: Arc::new(RwLock::new(HashMap::new())),
            runtime_catalog: None,
            migration_key: Arc::new(RwLock::new(None)),
            migration_signing_key: Arc::new(RwLock::new(None)),
            coordination: Arc::new(RwLock::new(None)),
            remote_chunks: Arc::new(MemoryRemoteChunkStore::default()),
        })
    }

    pub fn local_authority_enabled(&self) -> bool {
        self.local_account_provisioning
    }

    pub(crate) fn execute_local_cookie(
        &self,
        command: &str,
        input: serde_json::Value,
        principal: &Principal,
        grants: &[Grant],
        idempotency_key: Option<&str>,
        correlation_id: &str,
    ) -> Result<serde_json::Value, Problem> {
        if command != "cookie.list" {
            let _ = (input, principal, grants, idempotency_key);
            return Err(Problem::new(
                "production_cookie_device_dispatch_required",
                "Cookie mutation requires a connected authorized device",
                503,
                correlation_id,
            )
            .retryable(true));
        }
        if !self.local_authority_enabled() {
            return Err(Problem::new(
                "production_cookie_device_dispatch_required",
                "Hosted cookie operations require a connected authorized device",
                503,
                correlation_id,
            )
            .retryable(true));
        }
        let _ = idempotency_key;
        let authority = self.local_cookies.as_ref().ok_or_else(|| {
            Problem::new(
                "cookie_authority_unavailable",
                "Cookie authority unavailable",
                503,
                correlation_id,
            )
        })?;
        let mut authority = authority.lock().map_err(|_| {
            Problem::new(
                "cookie_authority_unavailable",
                "Cookie authority unavailable",
                503,
                correlation_id,
            )
        })?;
        match command {
            "cookie.list" => {
                let parsed: CookieListHttpInput = serde_json::from_value(input).map_err(|_| {
                    Problem::new(
                        "invalid_input",
                        "Invalid cookie list input",
                        400,
                        correlation_id,
                    )
                })?;
                let scope =
                    cookie_scope_http(&parsed.workspace_id, &parsed.profile_id, correlation_id)?;
                authorize_cookie(
                    self,
                    principal,
                    grants,
                    &scope,
                    Action::Read,
                    correlation_id,
                )?;
                set_cookie_state(self, &mut authority, &scope, correlation_id)?;
                serde_json::to_value(
                    authority
                        .list(&scope, principal, grants)
                        .map_err(|error| error.as_problem(correlation_id))?,
                )
                .map_err(|_| {
                    Problem::new(
                        "serialization_failed",
                        "Cookie response could not be serialized",
                        500,
                        correlation_id,
                    )
                })
            }
            _ => Err(Problem::new(
                "unknown_cookie_command",
                "Unknown cookie command",
                400,
                correlation_id,
            )),
        }
    }

    pub fn with_coordination(self, backend: Arc<dyn CoordinationBackend>) -> Self {
        *self.coordination.write().expect("coordination lock") = Some(backend);
        self
    }

    pub async fn readiness(&self) -> Result<(), Problem> {
        self.control_persistence.readiness()?;
        if !self.local_account_provisioning && !self.external_object_storage {
            return Err(problem(
                "object_storage_not_configured",
                "Remote object storage is not configured",
                503,
            ));
        }
        self.remote_chunks.readiness().map_err(|_| {
            problem(
                "readiness_unavailable",
                "Service readiness check failed",
                503,
            )
        })?;
        let coordination = self
            .coordination
            .read()
            .map_err(|_| {
                problem(
                    "readiness_unavailable",
                    "Service readiness check failed",
                    503,
                )
            })?
            .clone();
        match coordination {
            Some(backend) => backend.readiness().await?,
            None if !self.local_account_provisioning => {
                return Err(problem(
                    "coordination_not_configured",
                    "Redis coordination is not configured",
                    503,
                ));
            }
            None => {}
        }
        Ok(())
    }

    pub fn with_recovery_delivery(mut self, delivery: Arc<dyn RecoveryDelivery>) -> Self {
        self.recovery = self.recovery.with_delivery(delivery);
        self
    }

    pub fn with_remote_chunks(mut self, store: Box<dyn RemoteChunkStore>) -> Self {
        self.remote_chunks = Arc::new(SerializedRemoteChunkStore::new(store));
        self.external_object_storage = true;
        self
    }

    pub fn with_concurrent_remote_chunks(
        mut self,
        store: Box<dyn ConcurrentRemoteChunkStore>,
    ) -> Self {
        self.remote_chunks = Arc::from(store);
        self.external_object_storage = true;
        self
    }

    pub fn with_profile_resource(self, binding: ProfileResource) -> Self {
        self.replace_profile_resource(binding)
            .expect("profile resource locks");
        self
    }

    pub fn with_profile_record(self, profile: Profile) -> Self {
        self.create_profile_record(profile)
            .expect("profile record setup");
        self
    }

    pub(crate) fn put_remote_chunk(
        &self,
        object_key: &str,
        bytes: &[u8],
        expected_checksum: &str,
    ) -> Result<(), Problem> {
        self.remote_chunks.put(object_key, bytes, expected_checksum)
    }

    pub(crate) fn get_remote_chunk(
        &self,
        object_key: &str,
        expected_checksum: &str,
    ) -> Result<Vec<u8>, Problem> {
        Ok(self.remote_chunks.get(object_key, expected_checksum)?.bytes)
    }

    pub(crate) fn remote_chunk_checksum(
        &self,
        object_key: &str,
    ) -> Result<Option<String>, Problem> {
        self.remote_chunks.checksum(object_key)
    }

    fn delete_remote_chunk(
        &self,
        object_key: &str,
        expected_checksum: &str,
    ) -> Result<(), Problem> {
        self.remote_chunks
            .delete_if_checksum(object_key, expected_checksum)?;
        Ok(())
    }

    pub(crate) fn coordination(&self) -> Result<Arc<dyn CoordinationBackend>, Problem> {
        self.coordination
            .read()
            .map_err(|_| problem("coordination_unavailable", "Coordination unavailable", 503))?
            .clone()
            .ok_or_else(|| {
                problem("coordination_unavailable", "Coordination unavailable", 503).retryable(true)
            })
    }

    pub fn control_mutation(&self) -> &Arc<Mutex<()>> {
        &self.control_mutation
    }

    pub fn authenticate_token(&self, token: &str, now_ms: u64) -> Result<Principal, Problem> {
        self.authorization_authority()
            .authenticate_token(token, now_ms)
    }

    pub fn purge_expired_device_authorizations(&self, now_seconds: u64) -> Result<usize, Problem> {
        self.device_authorization
            .purge_expired_authorizations(now_seconds)
    }

    pub fn purge_expired_sessions(&self, now_seconds: u64) -> Result<usize, Problem> {
        self.auth
            .read()
            .map_err(|_| problem("auth_unavailable", "Authentication unavailable", 503))?
            .purge_expired_sessions(now_seconds)
    }

    pub(crate) fn account(
        &self,
        account_id: &AccountId,
    ) -> Result<Option<buglogin_domain::account::Account>, Problem> {
        self.account_persistence.load(account_id)
    }

    pub(crate) fn update_account_status_for_mcp(
        &self,
        actor_id: &AccountId,
        account_id: &AccountId,
        status: AccountStatus,
        expected_revision: u64,
        correlation_id: &str,
        idempotency_key: &str,
    ) -> Result<Account, Problem> {
        let current = checked_now(correlation_id)?;
        let (account, stored_revision) = self
            .account_persistence
            .load_versioned(account_id)?
            .ok_or_else(|| {
                Problem::new("account_not_found", "Account not found", 404, "identity")
            })?;
        if account.revision != stored_revision {
            return Err(Problem::new(
                "account_revision_conflict",
                "Account revision changed; reload and retry",
                409,
                "identity",
            ));
        }
        let updated = account.transition_status(expected_revision, status, current.to_string())?;
        let body = serde_json::to_value(&updated).map_err(|_| {
            Problem::new(
                "account_record_invalid",
                "Account record is invalid",
                500,
                "identity",
            )
        })?;
        let input_value = serde_json::json!({
            "accountId": account_id,
            "status": status,
            "expectedRevision": expected_revision,
        });
        let input_hash = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&input_value).map_err(|_| {
                Problem::new(
                    "serialization_failed",
                    "Request could not be serialized",
                    400,
                    "mcp",
                )
            })?)
        );
        let key_material = format!(
            "{}:{}:{}",
            actor_id.as_str(),
            account_id.as_str(),
            idempotency_key
        );
        let store_key = format!(
            "mcp-account-status-{:x}",
            Sha256::digest(key_material.as_bytes())
        );

        let mut auth = self.auth.write().map_err(|_| {
            Problem::new(
                "auth_unavailable",
                "Authentication unavailable",
                503,
                "identity",
            )
        })?;
        let mut puts = vec![ControlPut {
            tenant_id: CONTROL_TENANT.into(),
            record_type: "account".into(),
            record_id: account_id.as_str().into(),
            expected_revision: Some(stored_revision),
            value: body.clone(),
        }];
        let session_puts = auth.account_session_revocation_puts(account_id, current)?;
        let revoked_session_families = session_puts
            .iter()
            .filter(|put| put.record_type == "session_family")
            .count();
        puts.extend(session_puts);
        let token_puts = self
            .api_tokens
            .account_revocation_puts(account_id, current)?;
        let revoked_api_tokens = token_puts.len();
        puts.extend(token_puts);
        self.persist_control_aggregate_with_puts(
            "account.status.update",
            store_key,
            actor_id.as_str().to_owned(),
            true,
            &input_hash,
            serde_json::json!({
                "correlationId": correlation_id,
                "accountId": account_id,
                "status": status,
                "expectedRevision": expected_revision,
                "inputHash": input_hash,
                "revokedSessionFamilies": revoked_session_families,
                "revokedApiTokens": revoked_api_tokens,
            }),
            StatusCode::OK.as_u16(),
            body,
            puts,
        )?;
        auth.mark_account_sessions_revoked(account_id, current);
        Ok(updated)
    }

    pub fn operator_reset_platform_admin_password(
        &self,
        account_id: &AccountId,
        password: &str,
    ) -> Result<OperatorPasswordResetResult, Problem> {
        self.reset_platform_admin_password(account_id, password, "production_operator", false)
    }

    /// Provision an additional login-capable platform administrator through the
    /// production-only operator path. The identity is marked verified under the
    /// operator-attestation exception; this does not claim mailbox verification.
    pub fn operator_create_platform_admin(
        &self,
        mut input: OperatorPlatformAdminProvisionInput,
    ) -> Result<OperatorPlatformAdminProvisionResult, Problem> {
        if self.local_account_provisioning {
            input.password.zeroize();
            return Err(problem(
                "hosted_identity_required",
                "Operator administrator provisioning requires hosted identity storage",
                409,
            ));
        }

        let _mutation = self
            .control_mutation
            .lock()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?;
        let (mut aggregate, aggregate_revision) = self
            .control_persistence
            .load_versioned::<ControlAggregate>(
                CONTROL_TENANT,
                CONTROL_STATE_TYPE,
                CONTROL_STATE_ID,
            )?
            .ok_or_else(|| {
                problem(
                    "control_state_unavailable",
                    "Control state unavailable",
                    503,
                )
            })?;

        const OPERATOR_PROVISION_RECORD: &str = "platform_admin_operator_provision";
        const OPERATOR_PROVISION_ID: &str = "owner_account";
        if self
            .control_persistence
            .load::<serde_json::Value>(
                CONTROL_TENANT,
                OPERATOR_PROVISION_RECORD,
                OPERATOR_PROVISION_ID,
            )?
            .is_some()
        {
            input.password.zeroize();
            return Err(problem(
                "operator_admin_already_provisioned",
                "The one-time production owner administrator was already provisioned",
                409,
            ));
        }

        let account_id = AccountId::new(random_bootstrap_id("acct")?)
            .map_err(|_| problem("invalid_account", "Account ID is invalid", 500))?;
        let organization_id = OrganizationId::new(random_bootstrap_id("org")?)
            .map_err(|_| problem("invalid_organization", "Organization ID is invalid", 500))?;
        let workspace_id = WorkspaceId::new(random_bootstrap_id("ws")?)
            .map_err(|_| problem("invalid_workspace", "Workspace ID is invalid", 500))?;
        let member_id = MemberId::new(random_bootstrap_id("member")?)
            .map_err(|_| problem("invalid_member", "Member ID is invalid", 500))?;
        let current = checked_now("production-admin-provision")?;
        let created_at = migration_timestamp()?;
        let identity = AccountIdentity::new(
            account_id.clone(),
            input.username,
            input.email,
            created_at.clone(),
        )?
        .verify(1, created_at.clone())?;
        let mut puts = self.identity_persistence.prepare_create_puts(&identity)?;
        puts.push(self.credentials.prepare_initial_password_put(
            &account_id,
            input.password.as_str(),
            current,
        )?);
        input.password.zeroize();

        let platform_principal = Principal {
            account_id: Some(account_id.clone()),
            roles: BTreeSet::from([Role::PlatformAdmin]),
            ..Principal::default()
        };
        let organization = Organization {
            id: organization_id.clone(),
            account_id: account_id.clone(),
            name: input.organization_name,
            created_at: created_at.clone(),
        };
        aggregate.workspaces.add_organization(
            &platform_principal,
            &[],
            organization,
            OrganizationPolicy {
                max_workspaces: 10,
                offline_entitled: false,
            },
        )?;
        let workspace = Workspace {
            id: workspace_id.clone(),
            organization_id: organization_id.clone(),
            name: input.workspace_name,
            policy: WorkspacePolicy {
                max_profiles: 1_000,
                max_members: 100,
                max_storage_bytes: 10 * 1024 * 1024 * 1024,
                offline_entitled: false,
            },
            created_at: created_at.clone(),
        };
        aggregate.workspaces.create_workspace(
            &platform_principal,
            &[],
            &organization_id,
            workspace,
        )?;
        let expires_at = current.checked_add(600).ok_or_else(|| {
            problem(
                "admin_provision_expiry_invalid",
                "Administrator invitation expiry is invalid",
                500,
            )
        })?;
        aggregate.members.invite(
            &platform_principal,
            &[],
            Member {
                id: member_id.clone(),
                account_id: account_id.clone(),
                workspace_id: workspace_id.clone(),
                team_id: None,
                role: MemberRole::Owner,
                status: MemberStatus::Invited,
                invite_expires_at: Some(expires_at),
            },
            current,
        )?;
        aggregate
            .members
            .activate(&platform_principal, &[], &workspace_id, &member_id, current)?;
        aggregate.platform_admin_accounts.insert(account_id.clone());

        puts.push(ControlPut {
            tenant_id: CONTROL_TENANT.into(),
            record_type: "account".into(),
            record_id: account_id.as_str().into(),
            expected_revision: Some(0),
            value: serde_json::to_value(Account::new(account_id.clone(), created_at.clone()))
                .map_err(|_| problem("account_record_invalid", "Account record is invalid", 500))?,
        });
        puts.push(ControlPut {
            tenant_id: CONTROL_TENANT.into(),
            record_type: CONTROL_STATE_TYPE.into(),
            record_id: CONTROL_STATE_ID.into(),
            expected_revision: Some(aggregate_revision),
            value: serde_json::to_value(&aggregate)
                .map_err(|_| problem("control_state_invalid", "Control state is invalid", 500))?,
        });
        puts.push(ControlPut {
            tenant_id: CONTROL_TENANT.into(),
            record_type: OPERATOR_PROVISION_RECORD.into(),
            record_id: OPERATOR_PROVISION_ID.into(),
            expected_revision: Some(0),
            value: json!({
                "accountId": account_id,
                "provisionedAt": created_at,
                "verificationMethod": "operator-attested-owner",
            }),
        });
        self.control_persistence.execute(ExecuteControlCommand {
            tenant_id: CONTROL_TENANT.into(),
            command_id: "identity.production_operator_admin_provision".into(),
            idempotency_key: random_bootstrap_id("operator-admin")?,
            expected_revisions: BTreeMap::from([(
                ControlRecordKey {
                    tenant_id: CONTROL_TENANT.into(),
                    record_type: CONTROL_STATE_TYPE.into(),
                    record_id: CONTROL_STATE_ID.into(),
                },
                aggregate_revision,
            )]),
            puts,
            deletes: vec![],
            audit: ControlAuditEntry {
                tenant_id: CONTROL_TENANT.into(),
                command_id: "identity.production_operator_admin_provision".into(),
                actor_id: "production_operator".into(),
                allowed: true,
                detail: json!({
                    "accountId": account_id,
                    "organizationId": organization_id,
                    "workspaceId": workspace_id,
                    "memberId": member_id,
                    "identityVerification": "verified",
                    "verificationMethod": "operator-attested-owner",
                    "mailboxOwnershipClaimed": false,
                    "credentialIssued": true,
                    "sessionIssued": false,
                }),
            },
            response: HttpResponse {
                status: 201,
                headers: Default::default(),
                body: json!({
                    "accountId": account_id,
                    "organizationId": organization_id,
                    "workspaceId": workspace_id,
                }),
            },
        })?;
        self.reload_control_aggregate()?;

        Ok(OperatorPlatformAdminProvisionResult {
            account_id,
            organization_id,
            workspace_id,
        })
    }

    pub fn dev_reset_platform_admin_password(
        &self,
        account_id: &AccountId,
        password: &str,
    ) -> Result<OperatorPasswordResetResult, Problem> {
        let allowed = cfg!(debug_assertions)
            && std::env::var("BUGLOGIN_ENV").ok().as_deref() == Some("local")
            && std::env::var_os("BUGLOGIN_DATA_ROOT").is_none()
            && std::env::var_os("BUGLOGIN_POSTGRES_URL").is_some();
        if !allowed {
            return Err(problem(
                "local_dev_admin_reset_required",
                "Local administrator recovery is only available in local PostgreSQL development",
                409,
            ));
        }
        self.reset_platform_admin_password(account_id, password, "local_dev_admin_recovery", true)
    }

    fn reset_platform_admin_password(
        &self,
        account_id: &AccountId,
        password: &str,
        actor_id: &str,
        local_dev_password_policy: bool,
    ) -> Result<OperatorPasswordResetResult, Problem> {
        if self.local_account_provisioning {
            return Err(problem(
                "hosted_identity_required",
                "Operator password reset requires hosted identity storage",
                409,
            ));
        }
        let _mutation = self
            .control_mutation
            .lock()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?;
        let (aggregate, aggregate_revision) = self
            .control_persistence
            .load_versioned::<ControlAggregate>(
                CONTROL_TENANT,
                CONTROL_STATE_TYPE,
                CONTROL_STATE_ID,
            )?
            .ok_or_else(|| {
                problem(
                    "control_state_unavailable",
                    "Control state unavailable",
                    503,
                )
            })?;
        if !aggregate.platform_admin_accounts.contains(account_id) {
            return Err(problem(
                "platform_admin_required",
                "Target account is not a platform administrator",
                404,
            ));
        }
        let (account, account_revision) = self
            .account_persistence
            .load_versioned(account_id)?
            .ok_or_else(|| problem("account_not_found", "Account was not found", 404))?;
        if account.status != AccountStatus::Active
            || self.identity_persistence.load(account_id)?.is_none()
        {
            return Err(problem(
                "active_admin_identity_required",
                "Target account must be an active account with an identity",
                409,
            ));
        }
        let now = checked_now("operator-password-reset")?;
        let password_put = if local_dev_password_policy {
            self.credentials
                .local_dev_admin_password_reset_put(account_id, password, now)?
        } else {
            self.credentials
                .password_reset_put(account_id, password, now)?
        };
        let session_puts = self
            .auth
            .read()
            .map_err(|_| problem("auth_unavailable", "Authentication unavailable", 503))?
            .account_session_revocation_puts(account_id, now)?;
        let token_puts = self.api_tokens.account_revocation_puts(account_id, now)?;
        let session_count = session_puts.len();
        let api_token_count = token_puts.len();
        let mut puts = Vec::with_capacity(1 + session_count + api_token_count);
        puts.push(password_put);
        puts.extend(session_puts);
        puts.extend(token_puts);
        self.control_persistence.execute(ExecuteControlCommand {
            tenant_id: "identity".into(),
            command_id: "identity.operator_password_reset".into(),
            idempotency_key: random_bootstrap_id("op-reset")?,
            expected_revisions: BTreeMap::from([
                (
                    ControlRecordKey {
                        tenant_id: CONTROL_TENANT.into(),
                        record_type: "account".into(),
                        record_id: account_id.as_str().into(),
                    },
                    account_revision,
                ),
                (
                    ControlRecordKey {
                        tenant_id: CONTROL_TENANT.into(),
                        record_type: CONTROL_STATE_TYPE.into(),
                        record_id: CONTROL_STATE_ID.into(),
                    },
                    aggregate_revision,
                ),
            ]),
            puts,
            deletes: vec![],
            audit: ControlAuditEntry {
                tenant_id: "identity".into(),
                command_id: "identity.operator_password_reset".into(),
                actor_id: actor_id.into(),
                allowed: true,
                detail: json!({
                    "accountId": account_id,
                    "revokedSessions": session_count,
                    "revokedApiTokens": api_token_count,
                }),
            },
            response: HttpResponse {
                status: 204,
                headers: Default::default(),
                body: serde_json::Value::Null,
            },
        })?;
        self.auth
            .write()
            .map_err(|_| problem("auth_unavailable", "Authentication unavailable", 503))?
            .mark_account_sessions_revoked(account_id, now);
        Ok(OperatorPasswordResetResult {
            account_id: account_id.clone(),
            revoked_sessions: session_count,
            revoked_api_tokens: api_token_count,
        })
    }

    /// Adds a login-capable administrator to an existing local PostgreSQL development workspace.
    /// This is intentionally unavailable in release builds and hosted deployments.
    pub fn dev_provision_platform_admin(
        &self,
        mut input: LocalDevAdminProvisionInput,
    ) -> Result<LocalDevAdminProvisionResult, Problem> {
        let allowed = cfg!(debug_assertions)
            && std::env::var("BUGLOGIN_ENV").ok().as_deref() == Some("local")
            && std::env::var_os("BUGLOGIN_DATA_ROOT").is_none()
            && std::env::var_os("BUGLOGIN_POSTGRES_URL").is_some();
        if !allowed || self.local_account_provisioning {
            input.password.zeroize();
            return Err(problem(
                "local_dev_admin_provisioning_required",
                "Additional platform administrators can only be provisioned in local PostgreSQL development",
                409,
            ));
        }

        let _mutation = self
            .control_mutation
            .lock()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?;
        let (mut aggregate, aggregate_revision) = self
            .control_persistence
            .load_versioned::<ControlAggregate>(
                CONTROL_TENANT,
                CONTROL_STATE_TYPE,
                CONTROL_STATE_ID,
            )?
            .ok_or_else(|| {
                problem(
                    "control_state_unavailable",
                    "Control state unavailable",
                    503,
                )
            })?;
        if aggregate.platform_admin_accounts.is_empty() {
            input.password.zeroize();
            return Err(problem(
                "local_dev_admin_owner_required",
                "Bootstrap the local owner before adding another administrator",
                409,
            ));
        }

        let account_id = AccountId::new(random_bootstrap_id("acct")?)
            .map_err(|_| problem("invalid_account", "Account ID is invalid", 500))?;
        let principal = Principal {
            account_id: Some(account_id.clone()),
            roles: BTreeSet::from([Role::PlatformAdmin]),
            ..Principal::default()
        };
        let mut workspace = None;
        for organization in aggregate.workspaces.list_organizations(&principal, &[])? {
            for candidate in
                aggregate
                    .workspaces
                    .list_workspaces(&principal, &[], &organization.id)?
            {
                if candidate.name == "Main Workspace" {
                    workspace = Some((organization.id.clone(), candidate));
                    break;
                }
            }
            if workspace.is_some() {
                break;
            }
        }
        let (organization_id, workspace) = workspace.ok_or_else(|| {
            problem(
                "local_dev_workspace_not_found",
                "The local Main Workspace was not found",
                409,
            )
        })?;
        let active_members = aggregate
            .members
            .active_members()
            .into_iter()
            .filter(|member| member.workspace_id == workspace.id)
            .count() as u32;
        if active_members >= workspace.policy.max_members {
            input.password.zeroize();
            return Err(problem(
                "workspace_member_quota_exceeded",
                "The local workspace has reached its member limit",
                409,
            ));
        }

        let now = checked_now("local-dev-admin")?;
        let created_at = migration_timestamp()?;
        let identity = AccountIdentity::new(
            account_id.clone(),
            input.username,
            input.email,
            created_at.clone(),
        )?
        .verify(1, created_at.clone())?;
        let identity_puts = self.identity_persistence.prepare_create_puts(&identity)?;
        let password_put = self.credentials.prepare_initial_password_put(
            &account_id,
            input.password.as_str(),
            now,
        )?;
        input.password.zeroize();

        let member_id = MemberId::new(random_bootstrap_id("member")?)
            .map_err(|_| problem("invalid_member", "Member ID is invalid", 500))?;
        let expires_at = now.checked_add(600).ok_or_else(|| {
            problem(
                "local_dev_admin_expiry_invalid",
                "Invite expiry is invalid",
                500,
            )
        })?;
        aggregate.members.invite(
            &principal,
            &[],
            Member {
                id: member_id.clone(),
                account_id: account_id.clone(),
                workspace_id: workspace.id.clone(),
                team_id: None,
                role: MemberRole::Owner,
                status: MemberStatus::Invited,
                invite_expires_at: Some(expires_at),
            },
            now,
        )?;
        aggregate
            .members
            .activate(&principal, &[], &workspace.id, &member_id, now)?;
        aggregate.platform_admin_accounts.insert(account_id.clone());

        let mut puts = identity_puts;
        puts.push(password_put);
        puts.push(ControlPut {
            tenant_id: CONTROL_TENANT.into(),
            record_type: "account".into(),
            record_id: account_id.as_str().into(),
            expected_revision: Some(0),
            value: serde_json::to_value(Account::new(account_id.clone(), created_at.clone()))
                .map_err(|_| problem("account_record_invalid", "Account record is invalid", 500))?,
        });
        puts.push(ControlPut {
            tenant_id: CONTROL_TENANT.into(),
            record_type: CONTROL_STATE_TYPE.into(),
            record_id: CONTROL_STATE_ID.into(),
            expected_revision: Some(aggregate_revision),
            value: serde_json::to_value(&aggregate)
                .map_err(|_| problem("control_state_invalid", "Control state is invalid", 500))?,
        });
        self.control_persistence.execute(ExecuteControlCommand {
            tenant_id: CONTROL_TENANT.into(),
            command_id: "identity.local_dev_admin_provision".into(),
            idempotency_key: random_bootstrap_id("dev-admin")?,
            expected_revisions: BTreeMap::from([(
                ControlRecordKey {
                    tenant_id: CONTROL_TENANT.into(),
                    record_type: CONTROL_STATE_TYPE.into(),
                    record_id: CONTROL_STATE_ID.into(),
                },
                aggregate_revision,
            )]),
            puts,
            deletes: vec![],
            audit: ControlAuditEntry {
                tenant_id: CONTROL_TENANT.into(),
                command_id: "identity.local_dev_admin_provision".into(),
                actor_id: "local_development_operator".into(),
                allowed: true,
                detail: json!({
                    "accountId": account_id,
                    "organizationId": organization_id,
                    "workspaceId": workspace.id,
                    "memberId": member_id,
                }),
            },
            response: HttpResponse {
                status: 201,
                headers: Default::default(),
                body: json!({
                    "accountId": account_id,
                    "organizationId": organization_id,
                    "workspaceId": workspace.id,
                }),
            },
        })?;
        self.reload_control_aggregate()?;

        Ok(LocalDevAdminProvisionResult {
            account_id,
            organization_id,
            workspace_id: workspace.id,
        })
    }

    pub fn operator_list_platform_admins(&self) -> Result<Vec<OperatorPlatformAdmin>, Problem> {
        if self.local_account_provisioning {
            return Err(problem(
                "hosted_identity_required",
                "Operator admin listing requires hosted identity storage",
                409,
            ));
        }
        let (aggregate, _) = self
            .control_persistence
            .load_versioned::<ControlAggregate>(
                CONTROL_TENANT,
                CONTROL_STATE_TYPE,
                CONTROL_STATE_ID,
            )?
            .ok_or_else(|| {
                problem(
                    "control_state_unavailable",
                    "Control state unavailable",
                    503,
                )
            })?;
        let mut admins = aggregate
            .platform_admin_accounts
            .iter()
            .map(|account_id| {
                let account = match self.account_persistence.load(account_id) {
                    Ok(Some(account)) => account,
                    Ok(None) => {
                        return Err(problem(
                            "platform_admin_account_missing",
                            "A platform administrator account is missing",
                            500,
                        ));
                    }
                    Err(error) => return Err(error),
                };
                let identity = self.identity_persistence.load(account_id)?;
                Ok(OperatorPlatformAdmin {
                    account_id: account.id,
                    username: identity.as_ref().map(|identity| identity.username.clone()),
                    email: identity.as_ref().map(|identity| identity.email.clone()),
                    status: account.status,
                    identity_available: identity.is_some(),
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        admins.sort_by(|left, right| left.account_id.cmp(&right.account_id));
        Ok(admins)
    }

    pub fn grants(&self) -> Result<Vec<Grant>, Problem> {
        self.authorization_authority().grants()
    }

    fn authorization_authority(&self) -> ApiAuthorizationAuthority {
        ApiAuthorizationAuthority {
            auth: Arc::clone(&self.auth),
            api_tokens: self.api_tokens.clone(),
            account_persistence: self.account_persistence.clone(),
            members: Arc::clone(&self.members),
            platform_admin_accounts: Arc::clone(&self.platform_admin_accounts),
            grants: Arc::clone(&self.grants),
            materialized_member_grants: Arc::clone(&self.materialized_member_grants),
        }
    }

    pub fn with_runtime_binary(self, runtime: Runtime, binary: RuntimeBinary) -> Self {
        self.runtime_binaries
            .write()
            .expect("runtime catalog lock")
            .insert(runtime_key(&runtime), binary);
        self
    }

    pub(crate) fn control_aggregate(&self) -> Result<ControlAggregate, Problem> {
        Ok(ControlAggregate {
            grants: self
                .grants
                .read()
                .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?
                .clone(),
            operations: self
                .operations
                .read()
                .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?
                .clone(),
            materialized_member_grants: self
                .materialized_member_grants
                .read()
                .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?
                .clone(),
            platform_admin_accounts: self
                .platform_admin_accounts
                .read()
                .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?
                .clone(),
            profiles: self
                .hosted_profiles
                .read()
                .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?
                .clone(),
            profile_quarantine: self
                .profile_quarantine
                .read()
                .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?
                .clone(),
            profile_resources: self
                .profile_resources
                .read()
                .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?
                .clone(),
            sync_manifests: self
                .sync_manifests
                .read()
                .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?
                .clone(),
            sync_gc_claims: self
                .sync_gc_claims
                .read()
                .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?
                .clone(),
            resource_key_rotation_pending: self
                .resource_key_rotation_pending
                .read()
                .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?
                .clone(),
            profile_key_share_envelopes: self
                .profile_key_share_envelopes
                .read()
                .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?
                .clone(),
            blob_idempotency: self
                .blob_idempotency
                .read()
                .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?
                .clone(),
            sync_blob_reservations: self
                .sync_blob_reservations
                .read()
                .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?
                .clone(),
            runtime_projections: self
                .runtime_projections
                .read()
                .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?
                .clone(),
            proxies: self
                .proxies
                .read()
                .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?
                .snapshot(),
            workspaces: self
                .workspaces
                .read()
                .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?
                .clone(),
            members: self
                .members
                .read()
                .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?
                .clone(),
            resources: self
                .resources
                .read()
                .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?
                .clone(),
            groups: self
                .groups
                .read()
                .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?
                .clone(),
            extension_groups: self
                .extension_groups
                .read()
                .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?
                .clone(),
            profile_proxy_assignments: self
                .profile_proxy_assignments
                .read()
                .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?
                .clone(),
            teams: self
                .teams
                .read()
                .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?
                .clone(),
            audit: self
                .audit
                .read()
                .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?
                .clone(),
        })
    }

    pub(crate) fn replace_control_aggregate(
        &self,
        aggregate: ControlAggregate,
    ) -> Result<(), Problem> {
        let proxies = ProxyRepository::restore(aggregate.proxies.clone())
            .map_err(proxy_repository_problem)?;
        let profile_resource_index = build_profile_resource_index(&aggregate.profile_resources);
        *self
            .operations
            .write()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))? =
            aggregate.operations;
        *self
            .grants
            .write()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))? =
            aggregate.grants;
        *self
            .materialized_member_grants
            .write()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))? =
            aggregate.materialized_member_grants;
        *self
            .platform_admin_accounts
            .write()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))? =
            aggregate.platform_admin_accounts;
        *self
            .hosted_profiles
            .write()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))? =
            aggregate.profiles;
        *self
            .profile_quarantine
            .write()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))? =
            aggregate.profile_quarantine;
        {
            let mut index = self
                .profile_resource_index
                .write()
                .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?;
            let mut profile_resources = self
                .profile_resources
                .write()
                .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?;
            *profile_resources = aggregate.profile_resources;
            *index = profile_resource_index;
        }
        *self
            .sync_manifests
            .write()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))? =
            aggregate.sync_manifests;
        *self
            .sync_gc_claims
            .write()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))? =
            aggregate.sync_gc_claims;
        *self
            .resource_key_rotation_pending
            .write()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))? =
            aggregate.resource_key_rotation_pending;
        *self
            .profile_key_share_envelopes
            .write()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))? =
            aggregate.profile_key_share_envelopes;
        *self
            .blob_idempotency
            .write()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))? =
            aggregate.blob_idempotency;
        *self
            .sync_blob_reservations
            .write()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))? =
            aggregate.sync_blob_reservations;
        *self
            .runtime_projections
            .write()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))? =
            aggregate.runtime_projections;
        *self
            .proxies
            .write()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))? =
            proxies;
        *self
            .workspaces
            .write()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))? =
            aggregate.workspaces;
        *self
            .members
            .write()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))? =
            aggregate.members;
        *self
            .resources
            .write()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))? =
            aggregate.resources;
        *self
            .groups
            .write()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))? =
            aggregate.groups;
        *self
            .extension_groups
            .write()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))? =
            aggregate.extension_groups;
        *self
            .profile_proxy_assignments
            .write()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))? =
            aggregate.profile_proxy_assignments;
        *self
            .teams
            .write()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))? =
            aggregate.teams;
        *self
            .audit
            .write()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))? =
            aggregate.audit;
        Ok(())
    }

    pub(crate) fn mark_migrated_profile_ready_after_sync(
        &self,
        manifest: &SyncManifest,
    ) -> Result<(), Problem> {
        let mut aggregate = self.control_aggregate()?;
        if aggregate.mark_migrated_profile_ready_after_sync(manifest)? {
            self.replace_control_aggregate(aggregate)?;
        }
        Ok(())
    }

    pub(crate) fn list_profile_records(&self) -> Result<Vec<Versioned<Profile>>, Problem> {
        if self.local_authority_enabled() {
            return self.profiles.list();
        }
        let mut values = self
            .hosted_profiles
            .read()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?
            .values()
            .cloned()
            .collect::<Vec<_>>();
        let projections = self
            .runtime_projections
            .read()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?;
        for profile in &mut values {
            if let Some(projection) = projections.get(&profile.value.id) {
                profile.value.state = projected_profile_state(&projection.state)?;
            }
        }
        values.sort_by(|left, right| left.value.id.as_str().cmp(right.value.id.as_str()));
        Ok(values)
    }

    pub(crate) fn has_running_profile_using_proxy(
        &self,
        workspace_id: &WorkspaceId,
        proxy_id: &buglogin_domain::ids::ProxyId,
    ) -> Result<bool, Problem> {
        if self.local_authority_enabled() {
            for profile in self.list_profile_records()? {
                if profile.value.proxy_id.as_ref() == Some(proxy_id)
                    && profile.value.state == ProfileState::Running
                    && self
                        .profile_resource_record(&profile.value.id)?
                        .is_some_and(|binding| binding.workspace_id == *workspace_id)
                {
                    return Ok(true);
                }
            }
            return Ok(false);
        }

        let profile_ids = self
            .profile_resources
            .read()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?
            .values()
            .filter(|binding| &binding.workspace_id == workspace_id)
            .map(|binding| binding.profile_id.clone())
            .collect::<Vec<_>>();
        let profiles = self
            .hosted_profiles
            .read()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?;
        let projections = self
            .runtime_projections
            .read()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?;

        for profile_id in profile_ids {
            let Some(profile) = profiles.get(&profile_id) else {
                continue;
            };
            if profile.value.proxy_id.as_ref() != Some(proxy_id) {
                continue;
            }
            let profile_state = projections
                .get(&profile_id)
                .map(|projection| projected_profile_state(&projection.state))
                .transpose()?
                .unwrap_or_else(|| profile.value.state.clone());
            if profile_state == ProfileState::Running {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub(crate) fn list_profile_records_after(
        &self,
        cursor: Option<&ProfileId>,
        limit: usize,
    ) -> Result<Vec<Versioned<Profile>>, Problem> {
        if self.local_authority_enabled() {
            return self
                .profiles
                .list_after(cursor.map(ProfileId::as_str), limit);
        }

        let profiles = self
            .hosted_profiles
            .read()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?;
        let projections = self
            .runtime_projections
            .read()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?;
        profiles
            .range((
                cursor.map_or(Unbounded, |cursor| Excluded(cursor.clone())),
                Unbounded,
            ))
            .take(limit)
            .map(|(_, profile)| {
                let mut profile = profile.clone();
                if let Some(projection) = projections.get(&profile.value.id) {
                    profile.value.state = projected_profile_state(&projection.state)?;
                }
                Ok(profile)
            })
            .collect()
    }

    pub(crate) fn get_profile_record(
        &self,
        id: &ProfileId,
    ) -> Result<Option<Versioned<Profile>>, Problem> {
        if self.local_authority_enabled() {
            return self.profiles.get(id);
        }
        let mut profile = self
            .hosted_profiles
            .read()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?
            .get(id)
            .cloned();
        if let (Some(profile), Some(projection)) = (
            profile.as_mut(),
            self.runtime_projections
                .read()
                .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?
                .get(id),
        ) {
            profile.value.state = projected_profile_state(&projection.state)?;
        }
        Ok(profile)
    }

    pub(crate) fn create_profile_record(
        &self,
        profile: Profile,
    ) -> Result<Versioned<Profile>, Problem> {
        if self.local_authority_enabled() {
            return self.profiles.create(profile);
        }
        let mut profiles = self
            .hosted_profiles
            .write()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?;
        if profiles.contains_key(&profile.id) {
            return Err(Problem::new(
                "storage_revision_conflict",
                "Storage revision conflict",
                409,
                "storage",
            ));
        }
        let value = Versioned {
            schema_version: 2,
            revision: 1,
            value: profile,
        };
        profiles.insert(value.value.id.clone(), value.clone());
        Ok(value)
    }

    pub(crate) fn update_profile_record(
        &self,
        expected_revision: u64,
        profile: Profile,
    ) -> Result<Versioned<Profile>, Problem> {
        if self.local_authority_enabled() {
            return self.profiles.update(expected_revision, profile);
        }
        let mut profiles = self
            .hosted_profiles
            .write()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?;
        let current = profiles.get(&profile.id).ok_or_else(|| {
            Problem::new("profile_not_found", "Profile not found", 404, "profile")
        })?;
        if current.revision != expected_revision {
            return Err(Problem::new(
                "storage_revision_conflict",
                "Storage revision conflict",
                409,
                "storage",
            )
            .with_field("expectedRevision", expected_revision)
            .with_field("actualRevision", current.revision));
        }
        let next_revision = current.revision.checked_add(1).ok_or_else(|| {
            Problem::new(
                "profile_revision_exhausted",
                "Profile revision cannot be incremented",
                409,
                "profile",
            )
        })?;
        let value = Versioned {
            schema_version: current.schema_version,
            revision: next_revision,
            value: profile,
        };
        profiles.insert(value.value.id.clone(), value.clone());
        Ok(value)
    }

    pub(crate) fn ensure_no_pending_profile_proxy_assignment(
        &self,
        workspace_id: &WorkspaceId,
        profile_id: &ProfileId,
    ) -> Result<(), Problem> {
        let assignments = self.profile_proxy_assignments.read().map_err(|_| {
            problem(
                "profile_proxy_assignment_unavailable",
                "Profile proxy assignment service unavailable",
                503,
            )
        })?;
        if assignments.get_optional(workspace_id, profile_id).is_some() {
            return Err(problem(
                "profile_proxy_assignment_pending",
                "Apply or cancel the pending proxy assignment before editing this profile",
                409,
            ));
        }
        Ok(())
    }

    pub(crate) fn group_is_in_use(
        &self,
        workspace_id: &WorkspaceId,
        group_id: &GroupId,
    ) -> Result<bool, Problem> {
        for profile in self.list_profile_records()? {
            if !profile.value.group_ids.contains(group_id) {
                continue;
            }
            if self
                .profile_resource_record(&profile.value.id)?
                .is_some_and(|binding| &binding.workspace_id == workspace_id)
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub(crate) fn extension_group_is_in_use(
        &self,
        workspace_id: &WorkspaceId,
        extension_group_id: &ExtensionGroupId,
    ) -> Result<bool, Problem> {
        for profile in self.list_profile_records()? {
            if profile.value.extension_group_id.as_ref() != Some(extension_group_id) {
                continue;
            }
            if self
                .profile_resource_record(&profile.value.id)?
                .is_some_and(|binding| &binding.workspace_id == workspace_id)
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub(crate) fn set_hosted_profile_extension_group(
        &self,
        input: crate::services::extension_groups::ProfileExtensionGroupSetInput,
    ) -> Result<Versioned<Profile>, Problem> {
        let binding = self
            .profile_resource_record(&input.profile_id)?
            .ok_or_else(|| problem("binding_not_found", "Profile binding not found", 404))?;
        if binding.workspace_id != input.workspace_id {
            return Err(problem(
                "profile_scope_mismatch",
                "Profile workspace does not match request",
                403,
            ));
        }
        self.ensure_no_pending_profile_proxy_assignment(&input.workspace_id, &input.profile_id)?;
        if let Some(group_id) = &input.extension_group_id {
            self.extension_groups
                .read()
                .map_err(|_| {
                    problem(
                        "extension_group_unavailable",
                        "Extension group service unavailable",
                        503,
                    )
                })?
                .get_in_workspace(&input.workspace_id, group_id)?;
        }
        let mut profile = self
            .get_profile_record(&input.profile_id)?
            .ok_or_else(|| problem("profile_not_found", "Profile not found", 404))?
            .value;
        profile.extension_group_id = input.extension_group_id;
        self.update_profile_record(input.expected_revision, profile)
    }

    pub(crate) fn replace_hosted_profile_groups(
        &self,
        input: crate::services::groups::ProfileGroupsReplaceInput,
    ) -> Result<Versioned<Profile>, Problem> {
        let binding = self
            .profile_resource_record(&input.profile_id)?
            .ok_or_else(|| problem("binding_not_found", "Profile binding not found", 404))?;
        if binding.workspace_id != input.workspace_id {
            return Err(problem(
                "profile_scope_mismatch",
                "Profile workspace does not match request",
                403,
            ));
        }
        self.ensure_no_pending_profile_proxy_assignment(&input.workspace_id, &input.profile_id)?;
        self.groups
            .read()
            .map_err(|_| problem("group_unavailable", "Group service unavailable", 503))?
            .validate_membership(&input.workspace_id, &input.group_ids)?;
        let mut profile = self
            .get_profile_record(&input.profile_id)?
            .ok_or_else(|| problem("profile_not_found", "Profile not found", 404))?
            .value;
        profile.group_ids = input.group_ids;
        profile.group_ids.sort();
        self.update_profile_record(input.expected_revision, profile)
    }

    pub(crate) fn archive_profile_record(
        &self,
        expected_revision: u64,
        mut profile: Profile,
    ) -> Result<Versioned<Profile>, Problem> {
        if !self.local_authority_enabled() {
            let binding = self
                .profile_resource_record(&profile.id)?
                .ok_or_else(|| problem("binding_not_found", "Profile binding not found", 404))?;
            self.ensure_no_pending_profile_proxy_assignment(&binding.workspace_id, &profile.id)?;
        }
        profile.archived = true;
        self.update_profile_record(expected_revision, profile)
    }

    pub(crate) fn restore_profile_record(
        &self,
        expected_revision: u64,
        mut profile: Profile,
    ) -> Result<Versioned<Profile>, Problem> {
        if !self.local_authority_enabled() {
            let binding = self
                .profile_resource_record(&profile.id)?
                .ok_or_else(|| problem("binding_not_found", "Profile binding not found", 404))?;
            self.ensure_no_pending_profile_proxy_assignment(&binding.workspace_id, &profile.id)?;
        }
        profile.archived = false;
        self.update_profile_record(expected_revision, profile)
    }

    pub(crate) fn mutate_hosted_profile_archive_state(
        &self,
        profile_id: &ProfileId,
        expected_revision: u64,
        archive: bool,
    ) -> Result<Versioned<Profile>, Problem> {
        HostedProfileService::set_archive_state(self, profile_id, expected_revision, archive)
    }

    pub(crate) fn profile_exists(&self, id: &ProfileId) -> Result<bool, Problem> {
        Ok(self.get_profile_record(id)?.is_some())
    }

    pub(crate) fn current_sync_manifest(
        &self,
        workspace_id: &buglogin_domain::ids::WorkspaceId,
        resource_id: &buglogin_domain::ids::ResourceId,
    ) -> Result<Option<SyncManifest>, Problem> {
        Ok(self
            .sync_manifests
            .read()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?
            .get(&sync_manifest_key(workspace_id, resource_id))
            .cloned())
    }

    pub(crate) fn current_sync_manifest_fresh(
        &self,
        workspace_id: &buglogin_domain::ids::WorkspaceId,
        resource_id: &buglogin_domain::ids::ResourceId,
    ) -> Result<Option<SyncManifest>, Problem> {
        let _mutation = self
            .control_mutation
            .lock()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?;
        self.refresh_control_aggregate_locked()?;
        self.current_sync_manifest(workspace_id, resource_id)
    }

    pub(crate) fn resource_key_rotation_pending(
        &self,
        workspace_id: &buglogin_domain::ids::WorkspaceId,
        resource_id: &buglogin_domain::ids::ResourceId,
    ) -> Result<bool, Problem> {
        Ok(self
            .resource_key_rotation_pending
            .read()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?
            .contains(&sync_manifest_key(workspace_id, resource_id)))
    }

    pub(crate) fn mark_resource_key_rotation_pending(
        &self,
        workspace_id: &buglogin_domain::ids::WorkspaceId,
        resource_id: &buglogin_domain::ids::ResourceId,
    ) -> Result<(), Problem> {
        self.resource_key_rotation_pending
            .write()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?
            .insert(sync_manifest_key(workspace_id, resource_id));
        Ok(())
    }

    pub(crate) fn clear_resource_key_rotation_pending(
        &self,
        workspace_id: &buglogin_domain::ids::WorkspaceId,
        resource_id: &buglogin_domain::ids::ResourceId,
    ) -> Result<(), Problem> {
        self.resource_key_rotation_pending
            .write()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?
            .remove(&sync_manifest_key(workspace_id, resource_id));
        Ok(())
    }

    pub(crate) fn replace_profile_key_share_envelopes(
        &self,
        profile_id: &ProfileId,
        workspace_id: &WorkspaceId,
        resource_id: &buglogin_domain::ids::ResourceId,
        member_id: &MemberId,
        envelopes: Vec<ConfirmedProfileKeyShareEnvelope>,
    ) -> Result<(), Problem> {
        let mut stored = self
            .profile_key_share_envelopes
            .write()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?;
        stored.retain(|_, envelope| {
            envelope.profile_id != *profile_id || envelope.member_id != *member_id
        });
        for entry in envelopes {
            let device_id = entry.envelope.recipient_device_id.clone();
            let key = profile_key_share_envelope_record_key(resource_id, member_id, &device_id);
            stored.insert(
                key,
                StoredProfileKeyShareEnvelope {
                    profile_id: profile_id.clone(),
                    workspace_id: workspace_id.clone(),
                    resource_id: resource_id.clone(),
                    member_id: member_id.clone(),
                    envelope: entry.envelope,
                },
            );
        }
        Ok(())
    }

    pub(crate) fn replace_profile_key_share_envelope_set(
        &self,
        profile_id: &ProfileId,
        workspace_id: &WorkspaceId,
        resource_id: &buglogin_domain::ids::ResourceId,
        member_envelopes: Vec<(
            MemberId,
            Vec<buglogin_domain::profile::ConfirmedProfileKeyShareEnvelope>,
        )>,
    ) -> Result<(), Problem> {
        let mut stored = self
            .profile_key_share_envelopes
            .write()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?;
        stored.retain(|_, envelope| envelope.profile_id != *profile_id);
        for (member_id, envelopes) in member_envelopes {
            for entry in envelopes {
                let device_id = entry.envelope.recipient_device_id.clone();
                let key =
                    profile_key_share_envelope_record_key(resource_id, &member_id, &device_id);
                stored.insert(
                    key,
                    StoredProfileKeyShareEnvelope {
                        profile_id: profile_id.clone(),
                        workspace_id: workspace_id.clone(),
                        resource_id: resource_id.clone(),
                        member_id: member_id.clone(),
                        envelope: entry.envelope,
                    },
                );
            }
        }
        Ok(())
    }

    pub(crate) fn remove_profile_key_share_envelopes(
        &self,
        profile_id: &ProfileId,
        member_id: &MemberId,
    ) -> Result<(), Problem> {
        self.profile_key_share_envelopes
            .write()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?
            .retain(|_, envelope| {
                envelope.profile_id != *profile_id || envelope.member_id != *member_id
            });
        Ok(())
    }

    pub(crate) fn profile_key_share_envelope(
        &self,
        resource_id: &buglogin_domain::ids::ResourceId,
        member_id: &MemberId,
        device_id: &str,
    ) -> Result<Option<ProfileKeyShareEnvelope>, Problem> {
        Ok(self
            .profile_key_share_envelopes
            .read()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?
            .get(&profile_key_share_envelope_record_key(
                resource_id,
                member_id,
                device_id,
            ))
            .filter(|record| {
                record.resource_id == *resource_id
                    && record.member_id == *member_id
                    && record.envelope.recipient_device_id == device_id
            })
            .map(|record| record.envelope.clone()))
    }

    pub(crate) fn has_profile_key_share_envelope_for_member(
        &self,
        profile_id: &ProfileId,
        workspace_id: &WorkspaceId,
        resource_id: &buglogin_domain::ids::ResourceId,
        member_id: &MemberId,
        key_version: &str,
    ) -> Result<bool, Problem> {
        Ok(self
            .profile_key_share_envelopes
            .read()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?
            .values()
            .any(|record| {
                record.profile_id == *profile_id
                    && record.workspace_id == *workspace_id
                    && record.resource_id == *resource_id
                    && record.member_id == *member_id
                    && record.envelope.schema_version == 2
                    && record.envelope.workspace_id == *workspace_id
                    && record.envelope.resource_id == *resource_id
                    && record.envelope.key_version == key_version
            }))
    }

    pub(crate) fn replace_sync_manifest(&self, manifest: SyncManifest) -> Result<(), Problem> {
        self.sync_manifests
            .write()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?
            .insert(
                sync_manifest_key(&manifest.workspace_id, &manifest.resource_id),
                manifest,
            );
        Ok(())
    }

    pub(crate) fn validate_sync_manifest_gc_claims(
        &self,
        manifest: &SyncManifest,
    ) -> Result<(), Problem> {
        let claims = self
            .sync_gc_claims
            .read()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?;
        validate_manifest_gc_claims(manifest, &claims)
    }

    pub(crate) fn profile_resource_record(
        &self,
        profile_id: &ProfileId,
    ) -> Result<Option<ProfileResource>, Problem> {
        Ok(self
            .profile_resources
            .read()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?
            .get(profile_id)
            .cloned())
    }

    pub(crate) fn profile_quarantine_record(
        &self,
        profile_id: &ProfileId,
    ) -> Result<Option<ProfileQuarantineRecord>, Problem> {
        Ok(self
            .profile_quarantine
            .read()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?
            .get(profile_id)
            .cloned())
    }

    pub(crate) fn quarantine_profile(
        &self,
        profile_id: &ProfileId,
        deleted_by: Option<AccountId>,
        was_archived: bool,
    ) -> Result<(), Problem> {
        let deleted_at_unix_ms = checked_now_ms("profile-quarantine")?;
        let record = ProfileQuarantineRecord {
            deleted_at_unix_ms,
            retention_deadline_unix_ms: crate::clock::checked_add_millis(
                deleted_at_unix_ms,
                PROFILE_QUARANTINE_RETENTION_MS,
                "profile-quarantine",
            )?,
            deleted_by,
            was_archived,
        };
        let mut quarantine = self
            .profile_quarantine
            .write()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?;
        if quarantine.contains_key(profile_id) {
            return Err(problem(
                "profile_already_quarantined",
                "Profile is already in quarantine",
                409,
            ));
        }
        quarantine.insert(profile_id.clone(), record);
        Ok(())
    }

    pub(crate) fn restore_quarantined_profile(
        &self,
        profile_id: &ProfileId,
    ) -> Result<(), Problem> {
        self.profile_quarantine
            .write()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?
            .remove(profile_id);
        Ok(())
    }

    pub(crate) fn ensure_workspace_profile_capacity(
        &self,
        workspace_id: &WorkspaceId,
    ) -> Result<(), Problem> {
        let workspace = self
            .workspaces
            .read()
            .map_err(|_| {
                problem(
                    "workspace_unavailable",
                    "Workspace service unavailable",
                    503,
                )
            })?
            .workspace_for_policy(workspace_id)?;
        let active_count = self
            .profile_resources
            .read()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?
            .values()
            .filter(|binding| {
                binding.workspace_id == *workspace_id
                    && binding.archive_state == ProfileArchiveState::Active
            })
            .count();
        if active_count >= workspace.policy.max_profiles as usize {
            return Err(Problem::new(
                "profile_quota_exceeded",
                "Workspace profile quota exceeded",
                409,
                "workspace",
            )
            .with_field("maximumProfiles", workspace.policy.max_profiles));
        }
        Ok(())
    }

    pub(crate) fn profile_resource_for_resource(
        &self,
        workspace_id: &buglogin_domain::ids::WorkspaceId,
        resource_id: &buglogin_domain::ids::ResourceId,
    ) -> Result<Option<ProfileResource>, Problem> {
        let index = self
            .profile_resource_index
            .read()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?;
        let Some(profile_ids) = index.get(&(workspace_id.clone(), resource_id.clone())) else {
            return Ok(None);
        };
        if profile_ids.len() != 1 {
            return Err(problem(
                "profile_binding_ambiguous",
                "Resource has multiple profile bindings",
                409,
            ));
        }
        let profile_id = profile_ids.first().ok_or_else(|| {
            problem(
                "profile_binding_index_invalid",
                "Profile resource index is invalid",
                500,
            )
        })?;
        let profile_resources = self
            .profile_resources
            .read()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?;
        let binding = profile_resources.get(profile_id).ok_or_else(|| {
            problem(
                "profile_binding_index_invalid",
                "Profile resource index is invalid",
                500,
            )
        })?;
        if &binding.workspace_id != workspace_id || &binding.resource_id != resource_id {
            return Err(problem(
                "profile_binding_index_invalid",
                "Profile resource index is stale",
                500,
            ));
        }
        Ok(Some(binding.clone()))
    }

    pub(crate) fn replace_profile_resource(&self, binding: ProfileResource) -> Result<(), Problem> {
        let mut index = self
            .profile_resource_index
            .write()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?;
        let mut profile_resources = self
            .profile_resources
            .write()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?;
        if let Some(previous) = profile_resources.get(&binding.profile_id) {
            let previous_key = (previous.workspace_id.clone(), previous.resource_id.clone());
            let next_key = (binding.workspace_id.clone(), binding.resource_id.clone());
            if previous_key != next_key {
                if let Some(profile_ids) = index.get_mut(&previous_key) {
                    profile_ids.remove(&binding.profile_id);
                    if profile_ids.is_empty() {
                        index.remove(&previous_key);
                    }
                }
            }
        }
        index
            .entry((binding.workspace_id.clone(), binding.resource_id.clone()))
            .or_default()
            .insert(binding.profile_id.clone());
        profile_resources.insert(binding.profile_id.clone(), binding);
        Ok(())
    }

    pub(crate) fn blob_idempotency_record(
        &self,
        key: &str,
    ) -> Result<Option<BlobIdempotencyRecord>, Problem> {
        Ok(self
            .blob_idempotency
            .read()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?
            .get(key)
            .cloned())
    }

    pub(crate) fn remember_blob_idempotency(
        &self,
        key: String,
        record: BlobIdempotencyRecord,
    ) -> Result<(), Problem> {
        self.blob_idempotency
            .write()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?
            .insert(key, record);
        Ok(())
    }

    pub(crate) fn reserve_sync_blob_upload(
        &self,
        workspace_id: &WorkspaceId,
        resource_id: &buglogin_domain::ids::ResourceId,
        storage_key: &str,
        checksum: &str,
        size: u64,
        now_ms: u64,
        actor_id: &str,
    ) -> Result<(), Problem> {
        let object = SyncMigrationJournalObject {
            storage_key: storage_key.to_owned(),
            checksum: checksum.to_owned(),
            size,
        };
        self.reserve_sync_blob_uploads(workspace_id, resource_id, &[object], now_ms, actor_id)
    }

    pub(crate) fn reserve_sync_blob_uploads(
        &self,
        workspace_id: &WorkspaceId,
        resource_id: &buglogin_domain::ids::ResourceId,
        objects: &[SyncMigrationJournalObject],
        now_ms: u64,
        actor_id: &str,
    ) -> Result<(), Problem> {
        if self.local_authority_enabled() {
            return Ok(());
        }
        if objects.is_empty() {
            return Ok(());
        }
        let _mutation = self
            .control_mutation
            .lock()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?;
        self.refresh_control_aggregate_locked()?;
        let mut aggregate = self.control_aggregate()?;
        let committed_keys =
            manifest_storage_references_for_workspace(&aggregate, workspace_id.as_str());
        let mut seen = BTreeMap::<String, SyncMigrationJournalObject>::new();
        let mut requested = BTreeMap::<String, SyncMigrationJournalObject>::new();
        for object in objects {
            if aggregate.sync_gc_claims.contains_key(&object.storage_key) {
                return Err(problem(
                    "sync_object_cleanup_claimed",
                    "Sync object is currently claimed for cleanup",
                    409,
                )
                .retryable(true));
            }
            if let Some(previous) = seen.get(&object.storage_key) {
                if previous.checksum != object.checksum || previous.size != object.size {
                    return Err(problem(
                        "sync_blob_reservation_conflict",
                        "Sync batch contains conflicting data for one storage key",
                        409,
                    ));
                }
                continue;
            }
            seen.insert(object.storage_key.clone(), object.clone());
            if committed_keys.contains(&object.storage_key) {
                continue;
            }
            if let Some(existing) = aggregate.sync_blob_reservations.get(&object.storage_key) {
                if existing.workspace_id != workspace_id.as_str()
                    || existing.resource_id != resource_id.as_str()
                    || existing.checksum != object.checksum
                    || existing.size != object.size
                {
                    return Err(problem(
                        "sync_blob_reservation_conflict",
                        "Sync blob reservation conflicts with the requested object",
                        409,
                    ));
                }
            }
            requested.insert(object.storage_key.clone(), object.clone());
        }
        if requested.is_empty() {
            return Ok(());
        }
        let workspace = self
            .workspaces
            .read()
            .map_err(|_| {
                problem(
                    "workspace_unavailable",
                    "Workspace service unavailable",
                    503,
                )
            })?
            .workspace_for_policy(workspace_id)?;
        let committed_bytes = aggregate
            .sync_manifests
            .values()
            .filter(|manifest| manifest.workspace_id == *workspace_id)
            .flat_map(|manifest| manifest.objects.iter().map(|object| object.size))
            .try_fold(0u64, |total, value| total.checked_add(value))
            .ok_or_else(|| storage_quota_problem(u64::MAX, workspace.policy.max_storage_bytes))?;
        let reserved_bytes = aggregate
            .sync_blob_reservations
            .values()
            .filter(|reservation| {
                reservation.workspace_id == workspace_id.as_str()
                    && !committed_keys.contains(&reservation.storage_key)
            })
            .try_fold(0u64, |total, reservation| {
                total.checked_add(reservation.size)
            })
            .ok_or_else(|| storage_quota_problem(u64::MAX, workspace.policy.max_storage_bytes))?;
        let previous_total = committed_bytes
            .checked_add(reserved_bytes)
            .ok_or_else(|| storage_quota_problem(u64::MAX, workspace.policy.max_storage_bytes))?;
        let new_bytes = requested
            .values()
            .filter(|object| {
                !aggregate
                    .sync_blob_reservations
                    .contains_key(&object.storage_key)
            })
            .try_fold(0u64, |total, object| total.checked_add(object.size))
            .ok_or_else(|| storage_quota_problem(u64::MAX, workspace.policy.max_storage_bytes))?;
        let total = previous_total
            .checked_add(new_bytes)
            .ok_or_else(|| storage_quota_problem(u64::MAX, workspace.policy.max_storage_bytes))?;
        if total > workspace.policy.max_storage_bytes && total > previous_total {
            return Err(storage_quota_problem(
                total,
                workspace.policy.max_storage_bytes,
            ));
        }
        let expires_at_ms = crate::clock::checked_add_millis(
            now_ms,
            SYNC_BLOB_RESERVATION_TTL_MS,
            "sync-blob-reservation",
        )?;
        for object in requested.values() {
            aggregate.sync_blob_reservations.insert(
                object.storage_key.clone(),
                SyncBlobReservation {
                    workspace_id: workspace_id.as_str().to_owned(),
                    resource_id: resource_id.as_str().to_owned(),
                    storage_key: object.storage_key.clone(),
                    checksum: object.checksum.clone(),
                    size: object.size,
                    expires_at_ms,
                },
            );
        }
        self.replace_control_aggregate(aggregate)?;
        let reservation_hash = sha256_parts(&[
            workspace_id.as_str(),
            resource_id.as_str(),
            &requested
                .values()
                .map(|object| format!("{}:{}:{}", object.storage_key, object.checksum, object.size))
                .collect::<Vec<_>>()
                .join("\n"),
            &expires_at_ms.to_string(),
        ]);
        if let Err(error) = self.persist_control_aggregate(
            "profile.blob.reserve",
            format!("sync-blob-reservation:{reservation_hash}"),
            actor_id.to_owned(),
            true,
            &reservation_hash,
            serde_json::json!({
                "workspaceId": workspace_id,
                "resourceId": resource_id,
                "objectCount": requested.len(),
                "bytesReserved": new_bytes,
                "expiresAtMs": expires_at_ms,
            }),
            200,
            serde_json::json!({
                "reserved": requested.len(),
                "expiresAtMs": expires_at_ms,
            }),
        ) {
            self.reload_control_aggregate()?;
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn validate_sync_blob_upload(
        &self,
        workspace_id: &WorkspaceId,
        resource_id: &buglogin_domain::ids::ResourceId,
        storage_key: &str,
        checksum: &str,
        size: u64,
        now_ms: u64,
    ) -> Result<(), Problem> {
        if self.local_authority_enabled() {
            return Ok(());
        }
        let _mutation = self
            .control_mutation
            .lock()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?;
        self.refresh_control_aggregate_locked()?;
        let aggregate = self.control_aggregate()?;
        if aggregate.sync_gc_claims.contains_key(storage_key) {
            return Err(problem(
                "sync_object_cleanup_claimed",
                "Sync object is currently claimed for cleanup",
                409,
            )
            .retryable(true));
        }
        let referenced = manifest_storage_references_for_resource(
            &aggregate,
            workspace_id.as_str(),
            resource_id.as_str(),
        )
        .contains(storage_key);
        let reservation = aggregate.sync_blob_reservations.get(storage_key);
        if referenced {
            return Ok(());
        }
        let Some(reservation) = reservation else {
            return Err(problem(
                "sync_blob_reservation_missing",
                "Sync blob upload reservation is missing; retry the upload",
                409,
            )
            .retryable(true));
        };
        if reservation.workspace_id != workspace_id.as_str()
            || reservation.resource_id != resource_id.as_str()
            || reservation.checksum != checksum
            || reservation.size != size
        {
            return Err(problem(
                "sync_blob_reservation_conflict",
                "Sync blob reservation does not match the upload",
                409,
            ));
        }
        if reservation.expires_at_ms <= now_ms {
            return Err(problem(
                "sync_blob_reservation_expired",
                "Sync blob upload reservation expired; retry the upload",
                409,
            )
            .retryable(true));
        }
        Ok(())
    }

    pub(crate) fn release_committed_sync_blob_reservations(
        &self,
        manifest: &SyncManifest,
    ) -> Result<(), Problem> {
        let mut reservations = self
            .sync_blob_reservations
            .write()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?;
        for object in &manifest.objects {
            let storage_key = sync_manifest_object_storage_key(manifest, object);
            reservations.remove(&storage_key);
        }
        Ok(())
    }

    pub(crate) fn release_sync_blob_reservations_for_import(
        &self,
        journal_id: &str,
        workspace_id: &str,
        resource_id: &str,
        objects: &[SyncMigrationJournalObject],
        actor_id: &str,
    ) -> Result<(), Problem> {
        if self.local_authority_enabled() || objects.is_empty() {
            return Ok(());
        }
        let _mutation = self
            .control_mutation
            .lock()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?;
        self.release_sync_blob_reservations_for_import_locked(
            journal_id,
            workspace_id,
            resource_id,
            objects,
            actor_id,
        )
    }

    fn release_sync_blob_reservations_for_import_locked(
        &self,
        journal_id: &str,
        workspace_id: &str,
        resource_id: &str,
        objects: &[SyncMigrationJournalObject],
        actor_id: &str,
    ) -> Result<(), Problem> {
        if self.local_authority_enabled() || objects.is_empty() {
            return Ok(());
        }
        self.refresh_control_aggregate_locked()?;
        let mut aggregate = self.control_aggregate()?;
        let active_journals = self
            .control_persistence
            .list_versioned::<SyncMigrationJournal>(CONTROL_TENANT, "migration_sync_import")?;
        let active_keys = active_journals
            .iter()
            .filter(|(candidate_id, journal, _)| {
                candidate_id != journal_id
                    && matches!(
                        journal.state,
                        SyncMigrationJournalState::Staging | SyncMigrationJournalState::Finalizing
                    )
            })
            .flat_map(|(_, journal, _)| {
                journal
                    .created_objects
                    .iter()
                    .chain(&journal.reserved_objects)
                    .chain(&journal.resumed_objects)
                    .map(|object| object.storage_key.clone())
            })
            .collect::<BTreeSet<_>>();
        let committed_keys =
            manifest_storage_references_for_resource(&aggregate, workspace_id, resource_id);
        let mut released = Vec::new();
        for object in objects {
            if active_keys.contains(&object.storage_key)
                || committed_keys.contains(&object.storage_key)
            {
                continue;
            }
            let matches_import = aggregate
                .sync_blob_reservations
                .get(&object.storage_key)
                .is_some_and(|reservation| {
                    reservation.workspace_id == workspace_id
                        && reservation.resource_id == resource_id
                });
            if matches_import {
                aggregate.sync_blob_reservations.remove(&object.storage_key);
                released.push(object.storage_key.clone());
            }
        }
        if released.is_empty() {
            return Ok(());
        }
        released.sort();
        let digest = sha256_parts(&[journal_id, workspace_id, resource_id, &released.join("\n")]);
        self.replace_control_aggregate(aggregate)?;
        if let Err(error) = self.persist_control_aggregate(
            "profile.blob.reservation.release",
            format!("sync-import-reservation-release:{digest}"),
            actor_id.to_owned(),
            true,
            &digest,
            serde_json::json!({
                "journalId": journal_id,
                "objectCount": released.len(),
            }),
            200,
            serde_json::json!({"released": released.len()}),
        ) {
            self.reload_control_aggregate()?;
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn runtime_projection(
        &self,
        profile_id: &ProfileId,
    ) -> Result<Option<RuntimeProjection>, Problem> {
        Ok(self
            .runtime_projections
            .read()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?
            .get(profile_id)
            .cloned())
    }

    pub(crate) fn set_runtime_projection(
        &self,
        projection: RuntimeProjection,
    ) -> Result<(), Problem> {
        self.runtime_projections
            .write()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?
            .insert(projection.profile_id.clone(), projection);
        Ok(())
    }

    pub(crate) fn replay_control_response(
        &self,
        idempotency_key: &str,
        input_hash: &str,
    ) -> Result<Option<buglogin_storage::control::HttpResponse>, Problem> {
        let Some(result) = self
            .control_persistence
            .replay(CONTROL_TENANT, idempotency_key)?
        else {
            return Ok(None);
        };
        let stored_hash = result
            .response
            .headers
            .get("x-buglogin-input-sha256")
            .map(String::as_str)
            .unwrap_or("");
        if stored_hash != input_hash {
            return Err(problem(
                "idempotency_conflict",
                "Idempotency key was reused with different input",
                409,
            ));
        }
        Ok(Some(result.response))
    }
    pub(crate) fn claim_mcp_idempotency(
        &self,
        actor_id: &str,
        scope: &ResourceScope,
        command_id: &str,
        client_key: &str,
        input_hash: &str,
        operation_id: &str,
    ) -> Result<buglogin_storage::control::ControlIdempotencyClaim, Problem> {
        let storage_key = canonical_mcp_idempotency_key(actor_id, scope, command_id, client_key)?;
        let claim_token = sha256_parts(&["mcp-claim-v1", &storage_key, input_hash, operation_id]);
        self.control_persistence
            .claim_idempotency(buglogin_storage::control::ClaimControlCommand {
                tenant_id: CONTROL_TENANT.into(),
                idempotency_key: storage_key,
                request_checksum: input_hash.into(),
                claim_token,
                operation_id: operation_id.into(),
            })
    }
    pub(crate) fn complete_mcp_idempotency(
        &self,
        claim: buglogin_storage::control::ControlIdempotencyClaimToken,
        command_id: &str,
        actor_id: String,
        audit_allowed: bool,
        apply_mutations: bool,
        detail: serde_json::Value,
        status: u16,
        body: serde_json::Value,
    ) -> Result<buglogin_storage::control::ControlCommandResult, Problem> {
        let value = if apply_mutations {
            serde_json::to_value(self.control_aggregate()?).map_err(|_| {
                problem(
                    "control_serialization_failed",
                    "Control state serialization failed",
                    500,
                )
            })?
        } else {
            serde_json::Value::Null
        };
        let expected_revision = *self
            .control_revision
            .read()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?;
        let result = self.control_persistence.complete_idempotency(
            buglogin_storage::control::CompleteControlCommand {
                claim,
                command_id: command_id.into(),
                expected_revisions: BTreeMap::new(),
                puts: if apply_mutations {
                    vec![buglogin_storage::control::ControlPut {
                        tenant_id: CONTROL_TENANT.into(),
                        record_type: CONTROL_STATE_TYPE.into(),
                        record_id: CONTROL_STATE_ID.into(),
                        expected_revision: Some(expected_revision),
                        value,
                    }]
                } else {
                    vec![]
                },
                deletes: vec![],
                apply_mutations,
                audit: buglogin_storage::control::ControlAuditEntry {
                    tenant_id: CONTROL_TENANT.into(),
                    command_id: command_id.into(),
                    actor_id,
                    allowed: audit_allowed,
                    detail,
                },
                response: buglogin_storage::control::HttpResponse {
                    status,
                    headers: BTreeMap::new(),
                    body,
                },
            },
        );
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                self.reload_control_aggregate()?;
                return Err(error);
            }
        };
        if let Some(record) = result.records.first() {
            *self
                .control_revision
                .write()
                .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))? =
                record.revision;
        }
        Ok(result)
    }

    pub(crate) fn abandon_mcp_idempotency(
        &self,
        claim: &buglogin_storage::control::ControlIdempotencyClaimToken,
    ) -> Result<(), Problem> {
        self.control_persistence.abandon_idempotency(claim)
    }

    pub(crate) fn control_revision(&self) -> Result<u64, Problem> {
        self.control_revision
            .read()
            .map(|revision| *revision)
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))
    }

    pub(crate) fn load_sync_migration_journal(
        &self,
        journal_id: &str,
    ) -> Result<Option<(SyncMigrationJournal, u64)>, Problem> {
        self.control_persistence
            .load_versioned(CONTROL_TENANT, "migration_sync_import", journal_id)
    }

    pub(crate) fn try_sync_object_write_lock(
        &self,
        workspace_id: &str,
        resource_id: &str,
    ) -> Result<bool, Problem> {
        self.control_persistence
            .try_shared_advisory_lock(sync_gc_advisory_lock_id(workspace_id, resource_id))
    }

    pub(crate) fn release_sync_object_write_lock(
        &self,
        workspace_id: &str,
        resource_id: &str,
    ) -> Result<(), Problem> {
        self.control_persistence
            .release_shared_advisory_lock(sync_gc_advisory_lock_id(workspace_id, resource_id))
    }

    pub(crate) fn validate_sync_stage_write(
        &self,
        journal_id: &str,
        expected_revision: u64,
        storage_key: &str,
    ) -> Result<(), Problem> {
        let _mutation = self
            .control_mutation
            .lock()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?;
        self.refresh_control_aggregate_locked()?;
        let Some((journal, revision)) = self.load_sync_migration_journal(journal_id)? else {
            return Err(problem(
                "sync_import_journal_missing",
                "Sync import recovery journal is missing",
                409,
            ));
        };
        if revision != expected_revision || journal.state != SyncMigrationJournalState::Staging {
            return Err(problem(
                "sync_import_staging_fenced",
                "Sync import staging was superseded by recovery",
                409,
            )
            .retryable(true));
        }
        if self
            .control_aggregate()?
            .sync_gc_claims
            .contains_key(storage_key)
        {
            return Err(problem(
                "sync_object_cleanup_claimed",
                "Sync object is currently claimed for cleanup",
                409,
            )
            .retryable(true));
        }
        Ok(())
    }

    fn claim_sync_gc_objects(
        &self,
        journal_id: &str,
        objects: &[SyncMigrationJournalObject],
    ) -> Result<bool, Problem> {
        self.refresh_control_aggregate_locked()?;
        let Some((journal, _)) = self.load_sync_migration_journal(journal_id)? else {
            return Ok(false);
        };
        if journal.state != SyncMigrationJournalState::CleanupPending {
            return Ok(false);
        }
        let mut aggregate = self.control_aggregate()?;
        let references = manifest_storage_references(&aggregate);
        if objects
            .iter()
            .any(|object| references.contains(&object.storage_key))
            || objects.iter().any(|object| {
                aggregate
                    .sync_blob_reservations
                    .contains_key(&object.storage_key)
            })
        {
            return Ok(false);
        }
        let active_journals = self
            .control_persistence
            .list_versioned::<SyncMigrationJournal>(CONTROL_TENANT, "migration_sync_import")?;
        if active_journals.iter().any(|(candidate_id, journal, _)| {
            candidate_id != journal_id
                && matches!(
                    journal.state,
                    SyncMigrationJournalState::Staging | SyncMigrationJournalState::Finalizing
                )
                && objects.iter().any(|object| {
                    journal
                        .created_objects
                        .iter()
                        .any(|candidate| candidate.storage_key == object.storage_key)
                        || journal
                            .reserved_objects
                            .iter()
                            .any(|candidate| candidate.storage_key == object.storage_key)
                        || journal
                            .resumed_objects
                            .iter()
                            .any(|candidate| candidate.storage_key == object.storage_key)
                })
        }) {
            return Ok(false);
        }
        for object in objects {
            aggregate
                .sync_gc_claims
                .entry(object.storage_key.clone())
                .or_default()
                .insert(journal_id.to_owned());
        }
        self.replace_control_aggregate(aggregate)?;
        let revision = self.control_revision()?;
        let journal_digest = sha256_parts(&[journal_id]);
        let claim_key = format!("migration-sync-gc-claim:{journal_digest}:{revision}");
        if let Err(error) = self.persist_control_aggregate(
            "migration.sync.gc.claim",
            claim_key,
            "migration-recovery-worker".into(),
            true,
            &journal_digest,
            serde_json::json!({"journalId": journal_id, "objectCount": objects.len()}),
            200,
            serde_json::json!({"state": "claimed"}),
        ) {
            self.reload_control_aggregate()?;
            return Err(error);
        }
        Ok(true)
    }

    fn finish_sync_gc_cleanup(
        &self,
        journal_id: &str,
        objects: &[SyncMigrationJournalObject],
    ) -> Result<bool, Problem> {
        self.refresh_control_aggregate_locked()?;
        let Some((mut journal, journal_revision)) = self.load_sync_migration_journal(journal_id)?
        else {
            return Ok(false);
        };
        if journal.state != SyncMigrationJournalState::CleanupPending {
            return Ok(journal.state == SyncMigrationJournalState::Abandoned);
        }
        let Some((mut aggregate, control_revision)) = self
            .control_persistence
            .load_versioned::<ControlAggregate>(
                CONTROL_TENANT,
                CONTROL_STATE_TYPE,
                CONTROL_STATE_ID,
            )?
        else {
            return Err(problem(
                "control_state_missing",
                "Control state is missing",
                500,
            ));
        };
        for object in objects {
            if let Some(owners) = aggregate.sync_gc_claims.get_mut(&object.storage_key) {
                owners.remove(journal_id);
                if owners.is_empty() {
                    aggregate.sync_gc_claims.remove(&object.storage_key);
                }
            }
        }
        journal.state = SyncMigrationJournalState::Abandoned;
        journal.error_code = Some("migration_recovery_abandoned".into());
        journal.updated_at_ms = checked_now_ms("sync-migration-gc")?;
        let receipt = serde_json::json!({"journalId": journal_id, "state": "abandoned"});
        let idempotency_key = format!(
            "migration-sync-gc-finish:{}",
            sha256_parts(&[journal_id, &journal_revision.to_string()])
        );
        let result = self.control_persistence.execute(ExecuteControlCommand {
            tenant_id: CONTROL_TENANT.into(),
            command_id: "migration.sync.gc.finish".into(),
            idempotency_key,
            expected_revisions: BTreeMap::new(),
            puts: vec![
                ControlPut {
                    tenant_id: CONTROL_TENANT.into(),
                    record_type: CONTROL_STATE_TYPE.into(),
                    record_id: CONTROL_STATE_ID.into(),
                    expected_revision: Some(control_revision),
                    value: serde_json::to_value(&aggregate).map_err(|_| {
                        problem(
                            "control_serialization_failed",
                            "Control state serialization failed",
                            500,
                        )
                    })?,
                },
                ControlPut {
                    tenant_id: CONTROL_TENANT.into(),
                    record_type: "migration_sync_import".into(),
                    record_id: journal_id.into(),
                    expected_revision: Some(journal_revision),
                    value: serde_json::to_value(&journal).map_err(|_| {
                        problem(
                            "migration_sync_journal_invalid",
                            "Migration sync journal is invalid",
                            500,
                        )
                    })?,
                },
            ],
            deletes: vec![],
            audit: ControlAuditEntry {
                tenant_id: CONTROL_TENANT.into(),
                command_id: "migration.sync.gc.finish".into(),
                actor_id: "migration-recovery-worker".into(),
                allowed: true,
                detail: serde_json::json!({"journalId": journal_id, "objectCount": objects.len()}),
            },
            response: HttpResponse {
                status: 200,
                headers: BTreeMap::new(),
                body: receipt,
            },
        });
        match result {
            Ok(_) => {
                self.reload_control_aggregate()?;
                Ok(true)
            }
            Err(error) => {
                self.reload_control_aggregate()?;
                let current = self.load_sync_migration_journal(journal_id)?;
                if current.is_some_and(|(journal, _)| {
                    journal.state == SyncMigrationJournalState::Abandoned
                }) {
                    Ok(true)
                } else {
                    Err(error)
                }
            }
        }
    }

    pub fn recover_stale_sync_migrations(
        &self,
        stale_before_ms: u64,
    ) -> Result<MigrationRecoveryReport, Problem> {
        let _mutation = self
            .control_mutation
            .lock()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?;
        self.recover_expired_sync_blob_reservations_locked(checked_now_ms(
            "sync-migration-recovery",
        )?)?;
        let journals = self
            .control_persistence
            .list_versioned::<SyncMigrationJournal>(CONTROL_TENANT, "migration_sync_import")?;
        let mut report = MigrationRecoveryReport::default();
        for (journal_id, mut journal, revision) in journals {
            report.scanned += 1;
            if journal.updated_at_ms > stale_before_ms
                || matches!(
                    journal.state,
                    SyncMigrationJournalState::Active | SyncMigrationJournalState::Abandoned
                )
            {
                continue;
            }
            let mut journal_revision = revision;
            let _recovery_lock = if matches!(
                journal.state,
                SyncMigrationJournalState::Staging | SyncMigrationJournalState::Finalizing
            ) {
                match SyncMigrationJournalLock::acquire(self, &journal_id) {
                    Ok(lock) => Some(lock),
                    Err(error) if error.code == "migration_sync_import_in_progress" => {
                        report.deferred += 1;
                        continue;
                    }
                    Err(_) => {
                        report.failed += 1;
                        continue;
                    }
                }
            } else {
                None
            };
            if _recovery_lock.is_some() {
                self.refresh_control_aggregate_locked()?;
                let Some((current_journal, current_revision)) =
                    self.load_sync_migration_journal(&journal_id)?
                else {
                    report.deferred += 1;
                    continue;
                };
                if current_journal.updated_at_ms > stale_before_ms
                    || !matches!(
                        current_journal.state,
                        SyncMigrationJournalState::Staging | SyncMigrationJournalState::Finalizing
                    )
                {
                    report.deferred += 1;
                    continue;
                }
                journal = current_journal;
                journal_revision = current_revision;
            }
            if journal.state == SyncMigrationJournalState::Staging {
                journal.state = SyncMigrationJournalState::CleanupPending;
                journal.error_code = Some("stale_sync_import_staging".into());
                journal.updated_at_ms = checked_now_ms("sync-migration-recovery")?;
                match self.persist_sync_migration_journal(
                    &journal_id,
                    journal_revision,
                    format!("migration-staging-expire:{journal_id}:{journal_revision}"),
                    "migration-recovery-worker".into(),
                    &journal,
                ) {
                    Ok(_) => {}
                    Err(_) => {
                        report.deferred += 1;
                        continue;
                    }
                }
                let objects = journal
                    .reserved_objects
                    .iter()
                    .chain(&journal.created_objects)
                    .chain(&journal.resumed_objects)
                    .cloned()
                    .collect::<Vec<_>>();
                if self
                    .release_sync_blob_reservations_for_import_locked(
                        &journal_id,
                        &journal.workspace_id,
                        &journal.resource_id,
                        &objects,
                        "migration-recovery-worker",
                    )
                    .is_err()
                {
                    report.failed += 1;
                    continue;
                }
                // Establish a durable cleanup-pending checkpoint before the retention window.
                // A later worker pass must re-read this state before it can claim/delete blobs.
                report.deferred += 1;
                continue;
            }
            if journal.state == SyncMigrationJournalState::Finalizing {
                journal.state = SyncMigrationJournalState::CleanupPending;
                journal.error_code = Some("migration_recovery_cleanup_pending".into());
                journal.updated_at_ms = checked_now_ms("sync-migration-recovery")?;
                let finalized = self.persist_sync_migration_journal(
                    &journal_id,
                    journal_revision,
                    format!("migration-recovery-mark:{journal_id}:{journal_revision}"),
                    "migration-recovery-worker".into(),
                    &journal,
                );
                if finalized.is_err() {
                    report.failed += 1;
                    continue;
                }
                let objects = journal
                    .reserved_objects
                    .iter()
                    .chain(&journal.created_objects)
                    .chain(&journal.resumed_objects)
                    .cloned()
                    .collect::<Vec<_>>();
                if self
                    .release_sync_blob_reservations_for_import_locked(
                        &journal_id,
                        &journal.workspace_id,
                        &journal.resource_id,
                        &objects,
                        "migration-recovery-worker",
                    )
                    .is_ok()
                {
                    report.recovered += 1;
                } else {
                    report.failed += 1;
                }
                continue;
            }
            let lock_id = sync_gc_advisory_lock_id(&journal.workspace_id, &journal.resource_id);
            let acquired = match self.control_persistence.try_advisory_lock(lock_id) {
                Ok(acquired) => acquired,
                Err(_) => {
                    report.failed += 1;
                    continue;
                }
            };
            if !acquired {
                report.deferred += 1;
                continue;
            }
            let cleanup_result = (|| {
                let Some((current_journal, _)) = self.load_sync_migration_journal(&journal_id)?
                else {
                    return Ok(false);
                };
                if current_journal.state != SyncMigrationJournalState::CleanupPending
                    || !self.claim_sync_gc_objects(&journal_id, &current_journal.created_objects)?
                {
                    return Ok(false);
                }
                for object in &current_journal.created_objects {
                    self.delete_remote_chunk(&object.storage_key, &object.checksum)?;
                }
                self.finish_sync_gc_cleanup(&journal_id, &current_journal.created_objects)
            })();
            let release_result = self.control_persistence.release_advisory_lock(lock_id);
            match (cleanup_result, release_result) {
                (Ok(true), Ok(())) => report.recovered += 1,
                (Ok(false), Ok(())) => report.deferred += 1,
                _ => report.failed += 1,
            }
        }
        Ok(report)
    }

    fn recover_expired_sync_blob_reservations_locked(&self, now_ms: u64) -> Result<(), Problem> {
        self.refresh_control_aggregate_locked()?;
        let aggregate = self.control_aggregate()?;
        let committed = manifest_storage_references(&aggregate);
        let expired = aggregate
            .sync_blob_reservations
            .values()
            .filter(|reservation| reservation.expires_at_ms <= now_ms)
            .cloned()
            .collect::<Vec<_>>();
        for reservation in expired {
            let mut next = self.control_aggregate()?;
            let Some(current) = next
                .sync_blob_reservations
                .get(&reservation.storage_key)
                .cloned()
            else {
                continue;
            };
            if current != reservation {
                continue;
            }
            next.sync_blob_reservations.remove(&reservation.storage_key);
            if committed.contains(&reservation.storage_key) {
                self.replace_control_aggregate(next)?;
                let digest = sha256_parts(&[
                    &reservation.storage_key,
                    &reservation.expires_at_ms.to_string(),
                ]);
                if let Err(error) = self.persist_control_aggregate(
                    "profile.blob.reservation.release",
                    format!("sync-blob-reservation-release:{digest}"),
                    "sync-reservation-recovery-worker".into(),
                    true,
                    &digest,
                    serde_json::json!({"storageKeySha256": sha256_parts(&[&reservation.storage_key])}),
                    200,
                    serde_json::json!({"released": true}),
                ) {
                    self.reload_control_aggregate()?;
                    return Err(error);
                }
                continue;
            }
            let digest = sha256_parts(&[
                &reservation.storage_key,
                &reservation.checksum,
                &reservation.expires_at_ms.to_string(),
            ]);
            let journal_id = format!("blob-gc-{}", &digest[..32]);
            let journal = SyncMigrationJournal {
                schema: "buglogin.sync-blob-reservation-journal.v1".into(),
                operation_id: format!("sync-blob-reservation:{digest}"),
                export_id: format!("sync-blob:{}", &digest[..24]),
                export_sha256: digest.clone(),
                manifest_sha256: digest.clone(),
                blob_set_sha256: digest.clone(),
                workspace_id: reservation.workspace_id.clone(),
                resource_id: reservation.resource_id.clone(),
                base_generation: None,
                target_generation: 0,
                state: SyncMigrationJournalState::CleanupPending,
                created_objects: vec![SyncMigrationJournalObject {
                    storage_key: reservation.storage_key.clone(),
                    checksum: reservation.checksum.clone(),
                    size: reservation.size,
                }],
                resumed_objects: Vec::new(),
                reserved_objects: Vec::new(),
                destination_fencing_token: None,
                control_revision_before: self.control_revision()?,
                control_revision_after: None,
                updated_at_ms: now_ms,
                error_code: Some("sync_blob_reservation_expired".into()),
            };
            self.replace_control_aggregate(next)?;
            let journal_put = ControlPut {
                tenant_id: CONTROL_TENANT.into(),
                record_type: "migration_sync_import".into(),
                record_id: journal_id.clone(),
                expected_revision: None,
                value: serde_json::to_value(&journal).map_err(|_| {
                    problem(
                        "migration_sync_journal_invalid",
                        "Migration sync journal is invalid",
                        500,
                    )
                })?,
            };
            if let Err(error) = self.persist_control_aggregate_with_puts(
                "profile.blob.reservation.expire",
                format!("sync-blob-reservation-expire:{digest}"),
                "sync-reservation-recovery-worker".into(),
                true,
                &digest,
                serde_json::json!({
                    "journalId": journal_id,
                    "storageKeySha256": sha256_parts(&[&reservation.storage_key]),
                    "size": reservation.size,
                }),
                200,
                serde_json::json!({"journalId": journal_id, "state": "cleanup_pending"}),
                vec![journal_put],
            ) {
                self.reload_control_aggregate()?;
                return Err(error);
            }
        }
        Ok(())
    }

    pub fn recover_expired_profile_quarantines(&self, now_ms: u64) -> Result<usize, Problem> {
        if self.local_authority_enabled() {
            return Ok(0);
        }
        let _mutation = self
            .control_mutation
            .lock()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?;
        self.refresh_control_aggregate_locked()?;
        let snapshot = self.control_aggregate()?;
        let expired = snapshot
            .profile_quarantine
            .iter()
            .filter(|(_, record)| record.retention_deadline_unix_ms <= now_ms)
            .map(|(profile_id, record)| (profile_id.clone(), record.clone()))
            .collect::<Vec<_>>();
        let mut purged = 0;
        for (profile_id, quarantine) in expired {
            let aggregate = self.control_aggregate()?;
            let Some(profile) = aggregate.profiles.get(&profile_id) else {
                continue;
            };
            if matches!(
                profile.value.state,
                ProfileState::Running | ProfileState::Launching | ProfileState::Stopping
            ) {
                continue;
            }
            let Some(binding) = aggregate.profile_resources.get(&profile_id) else {
                continue;
            };
            let workspace_id = binding.workspace_id.as_str().to_owned();
            let resource_id = binding.resource_id.clone();
            let manifest_key = sync_manifest_key(&binding.workspace_id, &binding.resource_id);
            let manifest = aggregate.sync_manifests.get(&manifest_key).cloned();
            let objects = manifest
                .as_ref()
                .map(|manifest| {
                    manifest
                        .objects
                        .iter()
                        .map(|object| SyncMigrationJournalObject {
                            storage_key: sync_manifest_object_storage_key(manifest, object),
                            checksum: object.checksum.clone(),
                            size: object.size,
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let operation_digest = sha256_parts(&[
                profile_id.as_str(),
                &quarantine.retention_deadline_unix_ms.to_string(),
            ]);
            let journal_id = format!("profile-delete-{}", &operation_digest[..24]);
            let manifest_bytes = manifest
                .as_ref()
                .map(serde_json::to_vec)
                .transpose()
                .map_err(|_| {
                    problem(
                        "profile_delete_manifest_invalid",
                        "Profile sync manifest could not be serialized",
                        500,
                    )
                })?
                .unwrap_or_default();
            let object_bytes = serde_json::to_vec(&objects).map_err(|_| {
                problem(
                    "profile_delete_objects_invalid",
                    "Profile object list could not be serialized",
                    500,
                )
            })?;
            let manifest_sha256 = format!("{:x}", Sha256::digest(&manifest_bytes));
            let blob_set_sha256 = format!("{:x}", Sha256::digest(&object_bytes));
            let journal = SyncMigrationJournal {
                schema: "buglogin.sync-migration-journal.v1".into(),
                operation_id: format!("profile-delete:{operation_digest}"),
                export_id: format!("profile-delete:{}", profile_id.as_str()),
                export_sha256: operation_digest.clone(),
                manifest_sha256,
                blob_set_sha256,
                workspace_id: workspace_id.clone(),
                resource_id: resource_id.as_str().to_owned(),
                base_generation: manifest.as_ref().map(|value| value.generation),
                target_generation: manifest.as_ref().map_or(0, |value| value.generation),
                state: SyncMigrationJournalState::CleanupPending,
                created_objects: objects,
                resumed_objects: Vec::new(),
                reserved_objects: Vec::new(),
                destination_fencing_token: None,
                control_revision_before: self.control_revision()?,
                control_revision_after: None,
                updated_at_ms: now_ms,
                error_code: Some("profile_quarantine_retention_elapsed".into()),
            };
            let journal_put = ControlPut {
                tenant_id: CONTROL_TENANT.into(),
                record_type: "migration_sync_import".into(),
                record_id: journal_id.clone(),
                expected_revision: None,
                value: serde_json::to_value(&journal).map_err(|_| {
                    problem(
                        "profile_delete_journal_invalid",
                        "Profile deletion journal could not be serialized",
                        500,
                    )
                })?,
            };
            let mut next = aggregate;
            next.profiles.remove(&profile_id);
            next.profile_quarantine.remove(&profile_id);
            next.profile_resources.remove(&profile_id);
            next.sync_manifests.remove(&manifest_key);
            next.resource_key_rotation_pending.remove(&manifest_key);
            next.profile_key_share_envelopes
                .retain(|_, envelope| envelope.profile_id != profile_id);
            next.runtime_projections.remove(&profile_id);
            next.profile_proxy_assignments.purge_profile(&profile_id);
            next.resources.purge_resource(&resource_id);
            self.replace_control_aggregate(next)?;
            let persist_result = self.persist_control_aggregate_with_puts(
                "profile.quarantine.purge",
                format!("profile-quarantine-purge:{operation_digest}"),
                "profile-retention-worker".into(),
                true,
                &operation_digest,
                serde_json::json!({
                    "profileId": profile_id,
                    "workspaceId": workspace_id,
                    "resourceId": resource_id,
                    "objectCount": journal.created_objects.len(),
                }),
                200,
                serde_json::json!({ "profileId": profile_id, "state": "purged" }),
                vec![journal_put],
            );
            if let Err(error) = persist_result {
                self.reload_control_aggregate()?;
                return Err(error);
            }
            purged += 1;
        }
        Ok(purged)
    }

    pub(crate) fn persist_sync_migration_journal(
        &self,
        journal_id: &str,
        expected_revision: u64,
        idempotency_key: String,
        actor_id: String,
        journal: &SyncMigrationJournal,
    ) -> Result<u64, Problem> {
        let result =
            self.control_persistence
                .execute(buglogin_storage::control::ExecuteControlCommand {
                    tenant_id: CONTROL_TENANT.into(),
                    command_id: "migration.sync.journal.write".into(),
                    idempotency_key,
                    expected_revisions: BTreeMap::new(),
                    puts: vec![buglogin_storage::control::ControlPut {
                        tenant_id: CONTROL_TENANT.into(),
                        record_type: "migration_sync_import".into(),
                        record_id: journal_id.into(),
                        expected_revision: Some(expected_revision),
                        value: serde_json::to_value(journal).map_err(|_| {
                            problem(
                                "migration_sync_journal_invalid",
                                "Migration sync journal is invalid",
                                500,
                            )
                        })?,
                    }],
                    deletes: vec![],
                    audit: buglogin_storage::control::ControlAuditEntry {
                        tenant_id: CONTROL_TENANT.into(),
                        command_id: "migration.sync.journal.write".into(),
                        actor_id,
                        allowed: true,
                        detail: serde_json::json!({
                            "journalId": journal_id,
                            "state": journal.state,
                            "exportSha256": journal.export_sha256,
                        }),
                    },
                    response: buglogin_storage::control::HttpResponse {
                        status: 200,
                        headers: BTreeMap::new(),
                        body: serde_json::json!({
                            "journalId": journal_id,
                            "state": journal.state,
                        }),
                    },
                })?;
        if let Some(record) = result.records.first() {
            return Ok(record.revision);
        }
        self.control_persistence
            .load_versioned::<SyncMigrationJournal>(
                CONTROL_TENANT,
                "migration_sync_import",
                journal_id,
            )?
            .map(|(_, revision)| revision)
            .ok_or_else(|| {
                problem(
                    "migration_sync_journal_missing",
                    "Migration sync journal write produced no record",
                    500,
                )
            })
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn finalize_sync_import(
        &self,
        claim: buglogin_storage::control::ControlIdempotencyClaimToken,
        mut target: ControlAggregate,
        journal_id: &str,
        journal_revision: u64,
        journal: &SyncMigrationJournal,
        input_hash: &str,
        actor_id: String,
        detail: serde_json::Value,
        body: serde_json::Value,
    ) -> Result<buglogin_storage::control::ControlCommandResult, Problem> {
        release_promoted_sync_blob_reservations(&mut target, journal);
        let expected_control_revision = self.control_revision()?;
        let result = self.control_persistence.complete_idempotency(
            buglogin_storage::control::CompleteControlCommand {
                claim,
                command_id: "migration.sync.import".into(),
                expected_revisions: BTreeMap::new(),
                puts: vec![
                    buglogin_storage::control::ControlPut {
                        tenant_id: CONTROL_TENANT.into(),
                        record_type: CONTROL_STATE_TYPE.into(),
                        record_id: CONTROL_STATE_ID.into(),
                        expected_revision: Some(expected_control_revision),
                        value: serde_json::to_value(&target).map_err(|_| {
                            problem(
                                "control_serialization_failed",
                                "Control state serialization failed",
                                500,
                            )
                        })?,
                    },
                    buglogin_storage::control::ControlPut {
                        tenant_id: CONTROL_TENANT.into(),
                        record_type: "migration_sync_import".into(),
                        record_id: journal_id.into(),
                        expected_revision: Some(journal_revision),
                        value: serde_json::to_value(journal).map_err(|_| {
                            problem(
                                "migration_sync_journal_invalid",
                                "Migration sync journal is invalid",
                                500,
                            )
                        })?,
                    },
                ],
                deletes: vec![],
                apply_mutations: true,
                audit: buglogin_storage::control::ControlAuditEntry {
                    tenant_id: CONTROL_TENANT.into(),
                    command_id: "migration.sync.import".into(),
                    actor_id,
                    allowed: true,
                    detail,
                },
                response: buglogin_storage::control::HttpResponse {
                    status: 200,
                    headers: BTreeMap::from([(
                        "x-buglogin-input-sha256".into(),
                        input_hash.to_owned(),
                    )]),
                    body,
                },
            },
        )?;
        self.reload_control_aggregate()?;
        Ok(result)
    }

    pub(crate) fn reload_control_aggregate(&self) -> Result<(), Problem> {
        let Some((aggregate, revision)) = self.control_persistence.load_versioned(
            CONTROL_TENANT,
            CONTROL_STATE_TYPE,
            CONTROL_STATE_ID,
        )?
        else {
            return Err(problem(
                "control_state_missing",
                "Control state is missing",
                500,
            ));
        };
        self.replace_control_aggregate(aggregate)?;
        *self
            .control_revision
            .write()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))? =
            revision;
        Ok(())
    }

    pub(crate) fn refresh_control_aggregate(&self) -> Result<(), Problem> {
        if self.local_authority_enabled() {
            return Ok(());
        }
        let _mutation = self
            .control_mutation
            .lock()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?;
        self.refresh_control_aggregate_locked()
    }

    pub(crate) fn refresh_control_aggregate_locked(&self) -> Result<(), Problem> {
        if self.local_authority_enabled() {
            return Ok(());
        }
        let local_revision = *self
            .control_revision
            .read()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?;
        let durable_revision = self.control_persistence.revision(
            CONTROL_TENANT,
            CONTROL_STATE_TYPE,
            CONTROL_STATE_ID,
        )?;
        if durable_revision == Some(local_revision)
            || (durable_revision.is_none() && local_revision == 0)
        {
            return Ok(());
        }
        self.reload_control_aggregate()
    }

    pub(crate) fn persist_control_aggregate(
        &self,
        command: &str,
        idempotency_key: String,
        actor_id: String,
        allowed: bool,
        input_hash: &str,
        detail: serde_json::Value,
        status: u16,
        body: serde_json::Value,
    ) -> Result<(), Problem> {
        self.persist_control_aggregate_with_puts(
            command,
            idempotency_key,
            actor_id,
            allowed,
            input_hash,
            detail,
            status,
            body,
            Vec::new(),
        )
    }

    pub(crate) fn persist_control_aggregate_with_puts(
        &self,
        command: &str,
        idempotency_key: String,
        actor_id: String,
        allowed: bool,
        input_hash: &str,
        detail: serde_json::Value,
        status: u16,
        body: serde_json::Value,
        additional_puts: Vec<ControlPut>,
    ) -> Result<(), Problem> {
        let value = if allowed {
            serde_json::to_value(self.control_aggregate()?).map_err(|_| {
                problem(
                    "control_serialization_failed",
                    "Control state serialization failed",
                    500,
                )
            })?
        } else {
            serde_json::Value::Null
        };
        let expected_revision = *self
            .control_revision
            .read()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?;
        let result =
            self.control_persistence
                .execute(buglogin_storage::control::ExecuteControlCommand {
                    tenant_id: CONTROL_TENANT.into(),
                    command_id: command.into(),
                    idempotency_key,
                    expected_revisions: BTreeMap::new(),
                    puts: if allowed {
                        let mut puts = vec![buglogin_storage::control::ControlPut {
                            tenant_id: CONTROL_TENANT.into(),
                            record_type: CONTROL_STATE_TYPE.into(),
                            record_id: CONTROL_STATE_ID.into(),
                            expected_revision: Some(expected_revision),
                            value,
                        }];
                        puts.extend(additional_puts);
                        puts
                    } else {
                        vec![]
                    },
                    deletes: vec![],
                    audit: buglogin_storage::control::ControlAuditEntry {
                        tenant_id: CONTROL_TENANT.into(),
                        command_id: command.into(),
                        actor_id,
                        allowed,
                        detail,
                    },
                    response: buglogin_storage::control::HttpResponse {
                        status,
                        headers: BTreeMap::from([(
                            "x-buglogin-input-sha256".into(),
                            input_hash.to_owned(),
                        )]),
                        body,
                    },
                })?;
        if let Some(record) = result.records.first() {
            *self
                .control_revision
                .write()
                .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))? =
                record.revision;
        }
        Ok(())
    }

    pub(crate) fn mutate_operation(
        &self,
        operation_id: &str,
        command: &str,
        stage: &str,
        actor_id: &str,
        mutate: impl FnOnce(&mut OperationService) -> Result<(), Problem>,
    ) -> Result<Operation, Problem> {
        let _mutation = self
            .control_mutation
            .lock()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?;
        self.mutate_operation_locked(operation_id, command, stage, actor_id, mutate)
    }

    pub(crate) fn mutate_operation_locked(
        &self,
        operation_id: &str,
        command: &str,
        stage: &str,
        actor_id: &str,
        mutate: impl FnOnce(&mut OperationService) -> Result<(), Problem>,
    ) -> Result<Operation, Problem> {
        if !self.local_authority_enabled() {
            let local_revision = *self
                .control_revision
                .read()
                .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?;
            let durable_revision = self.control_persistence.revision(
                CONTROL_TENANT,
                CONTROL_STATE_TYPE,
                CONTROL_STATE_ID,
            )?;
            if durable_revision != Some(local_revision)
                && !(durable_revision.is_none() && local_revision == 0)
            {
                self.reload_control_aggregate()?;
            }
        }

        let before = self
            .operations
            .read()
            .map_err(|_| {
                problem(
                    "operation_unavailable",
                    "Operation service unavailable",
                    503,
                )
            })?
            .clone();
        let mutation_result = {
            let mut operations = self.operations.write().map_err(|_| {
                problem(
                    "operation_unavailable",
                    "Operation service unavailable",
                    503,
                )
            })?;
            mutate(&mut *operations)
        };
        if let Err(error) = mutation_result {
            *self.operations.write().map_err(|_| {
                problem(
                    "operation_unavailable",
                    "Operation service unavailable",
                    503,
                )
            })? = before;
            return Err(error);
        }
        let operation = self
            .operations
            .read()
            .map_err(|_| {
                problem(
                    "operation_unavailable",
                    "Operation service unavailable",
                    503,
                )
            })?
            .find(operation_id)
            .cloned()
            .ok_or_else(|| problem("operation_not_found", "Operation not found", 404))?;
        let input = match serde_json::to_vec(&operation) {
            Ok(input) => input,
            Err(_) => {
                *self.operations.write().map_err(|_| {
                    problem(
                        "operation_unavailable",
                        "Operation service unavailable",
                        503,
                    )
                })? = before;
                return Err(problem(
                    "operation_serialization_failed",
                    "Operation serialization failed",
                    500,
                ));
            }
        };
        use sha2::{Digest, Sha256};
        let input_hash = format!("{:x}", Sha256::digest(&input));
        let idempotency_digest = format!(
            "{:x}",
            Sha256::digest(format!("{operation_id}:{stage}").as_bytes())
        );
        let persisted = self.persist_control_aggregate(
            command,
            format!("operation-{}", &idempotency_digest[..32]),
            actor_id.to_owned(),
            true,
            &input_hash,
            serde_json::json!({
                "operationId": operation_id,
                "status": operation.status,
                "stage": stage
            }),
            200,
            serde_json::json!({ "operationId": operation_id }),
        );
        if let Err(error) = persisted {
            // A failed CAS means this process may have been stale. Reload the
            // durable aggregate so no caller can observe an uncommitted state.
            if self.reload_control_aggregate().is_err() {
                *self.operations.write().map_err(|_| {
                    problem(
                        "operation_unavailable",
                        "Operation service unavailable",
                        503,
                    )
                })? = before;
            }
            return Err(error);
        }
        Ok(operation)
    }
    pub fn with_migration_key(self, key: MigrationKey) -> Self {
        *self.migration_key.write().expect("migration key lock") = Some(key);
        self
    }

    pub fn with_runtime_catalog(mut self, catalog: PublishedRuntimeCatalog) -> Self {
        self.runtime_catalog = Some(Arc::new(catalog));
        self
    }

    pub fn published_runtime_catalog(&self) -> Option<&PublishedRuntimeCatalog> {
        self.runtime_catalog.as_deref()
    }

    pub fn with_device_verification_uri(mut self, uri: impl Into<String>) -> Result<Self, Problem> {
        let uri = uri.into();
        let parsed = Url::parse(&uri).ok();
        let valid_uri = parsed.as_ref().is_some_and(|url| {
            let authority_has_userinfo = uri
                .split_once("://")
                .and_then(|(_, remainder)| remainder.split(['/', '?', '#']).next())
                .is_some_and(|authority| authority.contains('@'));
            let local_http = url.scheme() == "http"
                && url.host_str().is_some_and(|host| {
                    host.eq_ignore_ascii_case("localhost")
                        || host
                            .parse::<std::net::IpAddr>()
                            .is_ok_and(|address| address.is_loopback())
                });
            let secure_web = url.scheme() == "https";
            uri.trim() == uri
                && !uri.bytes().any(|byte| byte.is_ascii_whitespace())
                && !authority_has_userinfo
                && url.username().is_empty()
                && url.password().is_none()
                && url.host_str().is_some()
                && url.fragment().is_none()
                && (secure_web || local_http)
        });
        if !valid_uri {
            return Err(problem(
                "device_verification_uri_invalid",
                "Device verification URI is invalid",
                500,
            ));
        }
        self.device_verification_uri = Arc::from(uri);
        Ok(self)
    }

    pub fn with_web_origin(mut self, origin: impl Into<String>) -> Result<Self, Problem> {
        let origin = origin.into();
        let parsed = Url::parse(&origin).ok();
        let valid_origin = parsed.as_ref().is_some_and(|url| {
            let local_http = url.scheme() == "http"
                && url
                    .host_str()
                    .and_then(|host| host.parse::<std::net::IpAddr>().ok())
                    .is_some_and(|address| address.is_loopback());
            let secure_web = url.scheme() == "https";
            url.origin().ascii_serialization() == origin
                && url.username().is_empty()
                && url.password().is_none()
                && url.path() == "/"
                && url.query().is_none()
                && url.fragment().is_none()
                && (secure_web || local_http)
        });
        if !valid_origin {
            return Err(problem("web_origin_invalid", "Web origin is invalid", 500));
        }
        self.web_origin = Arc::from(origin);
        Ok(self)
    }

    pub fn provision_web_identity(
        &self,
        account_id: impl Into<String>,
        username: impl Into<String>,
        email: impl Into<String>,
        password: &str,
    ) -> Result<AccountIdentity, Problem> {
        let account_id = AccountId::new(account_id.into())
            .map_err(|_| problem("invalid_account", "Invalid account", 400))?;
        let current = checked_now("web-identity")?;
        if self.local_account_provisioning {
            self.account_persistence
                .provision_default(account_id.clone(), current.to_string())?;
        } else if self.account_persistence.load(&account_id)?.is_none() {
            return Err(problem(
                "account_provisioning_disabled",
                "Account provisioning is disabled",
                403,
            ));
        }
        let mut identity =
            AccountIdentity::new(account_id.clone(), username, email, current.to_string())?;
        if self.local_account_provisioning {
            identity = identity.verify(identity.revision, current.to_string())?;
        }
        let identity = self.identity_persistence.create(&identity)?;
        self.credentials
            .set_initial_password(&account_id, password, current)?;
        Ok(identity)
    }

    pub fn bootstrap_first_owner(
        &self,
        mut input: FirstOwnerBootstrapInput,
    ) -> Result<FirstOwnerBootstrapResult, Problem> {
        if self.local_account_provisioning {
            return Err(Problem::new(
                "production_bootstrap_required",
                "Initial owner bootstrap is only available for hosted deployments",
                409,
                "identity",
            ));
        }
        if !input.operator_attested_email {
            return Err(Problem::new(
                "owner_email_attestation_required",
                "The operator must attest that the owner's email was verified out of band",
                400,
                "identity",
            ));
        }
        if input.max_workspaces == 0
            || input.max_profiles == 0
            || input.max_members == 0
            || input.max_storage_bytes == 0
        {
            return Err(Problem::new(
                "bootstrap_policy_invalid",
                "Initial workspace limits must be positive",
                400,
                "identity",
            ));
        }

        const IDENTITY_TENANT: &str = "identity";
        const BOOTSTRAP_RECORD: &str = "production_bootstrap";
        const BOOTSTRAP_ID: &str = "initial_owner";
        const BOOTSTRAP_KEY: &str = "initial-owner-bootstrap-v1";
        if self
            .control_persistence
            .revision(IDENTITY_TENANT, BOOTSTRAP_RECORD, BOOTSTRAP_ID)?
            .is_some()
        {
            return Err(Problem::new(
                "initial_owner_already_bootstrapped",
                "The initial owner has already been bootstrapped",
                409,
                "identity",
            ));
        }
        if !self
            .control_persistence
            .list::<AccountIdentity>(IDENTITY_TENANT, "account_identity")?
            .is_empty()
            || !self
                .control_persistence
                .list::<serde_json::Value>(IDENTITY_TENANT, "account_password")?
                .is_empty()
            || !self
                .control_persistence
                .list::<serde_json::Value>(IDENTITY_TENANT, "account_identity_username")?
                .is_empty()
            || !self
                .control_persistence
                .list::<serde_json::Value>(IDENTITY_TENANT, "account_identity_email")?
                .is_empty()
        {
            return Err(Problem::new(
                "initial_owner_bootstrap_not_empty",
                "Login identity records already exist; bootstrap is only for the first owner",
                409,
                "identity",
            ));
        }

        let accounts = self
            .control_persistence
            .list::<Account>(CONTROL_TENANT, "account")?;
        let (mut aggregate, aggregate_revision) = match self
            .control_persistence
            .load_versioned::<ControlAggregate>(
                CONTROL_TENANT,
                CONTROL_STATE_TYPE,
                CONTROL_STATE_ID,
            )? {
            Some((aggregate, revision)) => (aggregate, revision),
            None => (ControlAggregate::default(), 0),
        };
        if !aggregate.platform_admin_accounts.is_empty() {
            return Err(Problem::new(
                "initial_owner_bootstrap_not_empty",
                "A platform administrator already exists",
                409,
                "identity",
            ));
        }

        let (account_id, created_account) = match input.account_id {
            Some(value) => {
                let account_id = AccountId::new(value).map_err(|_| {
                    Problem::new("invalid_account", "Account ID is invalid", 400, "identity")
                })?;
                match accounts.iter().find(|account| account.id == account_id) {
                    Some(account) if account.status == AccountStatus::Active => (account_id, false),
                    Some(_) => {
                        return Err(Problem::new(
                            "account_inactive",
                            "The selected owner account is not active",
                            409,
                            "identity",
                        ));
                    }
                    None if accounts.is_empty() => (account_id, true),
                    None => {
                        return Err(Problem::new(
                            "account_not_found",
                            "The selected owner account does not exist",
                            404,
                            "identity",
                        ));
                    }
                }
            }
            None if accounts.is_empty() => (
                AccountId::new(random_bootstrap_id("acct")?).map_err(|_| {
                    Problem::new("invalid_account", "Account ID is invalid", 500, "identity")
                })?,
                true,
            ),
            None => {
                return Err(Problem::new(
                    "account_id_required",
                    "Select an existing account when control-plane data is already imported",
                    400,
                    "identity",
                ));
            }
        };

        let platform_principal = Principal {
            account_id: Some(account_id.clone()),
            roles: BTreeSet::from([Role::PlatformAdmin]),
            ..Principal::default()
        };
        let organizations = aggregate
            .workspaces
            .list_organizations(&platform_principal, &[])?;
        let owner_organizations = organizations
            .iter()
            .filter(|organization| organization.account_id == account_id)
            .cloned()
            .collect::<Vec<_>>();
        if owner_organizations.is_empty() && !organizations.is_empty() {
            return Err(Problem::new(
                "bootstrap_organization_owner_mismatch",
                "Imported organizations belong to another account; refusing to create an unrelated owner",
                409,
                "identity",
            ));
        }
        if created_account && !organizations.is_empty() {
            return Err(Problem::new(
                "bootstrap_control_data_exists",
                "Control-plane data already exists; select its existing owner account",
                409,
                "identity",
            ));
        }

        let current = checked_now("owner-bootstrap")?;
        let created_at = migration_timestamp()?;
        let identity = AccountIdentity::new(
            account_id.clone(),
            input.username,
            input.email,
            created_at.clone(),
        )?
        .verify(1, created_at.clone())?;
        let mut identity_puts = self.identity_persistence.prepare_create_puts(&identity)?;
        let password_put = self.credentials.prepare_initial_password_put(
            &account_id,
            input.password.as_str(),
            current,
        )?;
        input.password.zeroize();

        let (organization_id, created_organization, workspace_id, created_workspace) =
            if let Some(organization) = owner_organizations.first() {
                let existing_workspaces = aggregate.workspaces.list_workspaces(
                    &platform_principal,
                    &[],
                    &organization.id,
                )?;
                if let Some(workspace) = existing_workspaces.first() {
                    (organization.id.clone(), false, workspace.id.clone(), false)
                } else {
                    let workspace_id =
                        WorkspaceId::new(random_bootstrap_id("ws")?).map_err(|_| {
                            Problem::new(
                                "invalid_workspace",
                                "Workspace ID is invalid",
                                500,
                                "identity",
                            )
                        })?;
                    aggregate.workspaces.create_workspace(
                        &platform_principal,
                        &[],
                        &organization.id,
                        Workspace {
                            id: workspace_id.clone(),
                            organization_id: organization.id.clone(),
                            name: input.workspace_name,
                            policy: WorkspacePolicy {
                                max_profiles: input.max_profiles,
                                max_members: input.max_members,
                                max_storage_bytes: input.max_storage_bytes,
                                offline_entitled: false,
                            },
                            created_at: created_at.clone(),
                        },
                    )?;
                    (organization.id.clone(), false, workspace_id, true)
                }
            } else {
                let organization_id =
                    OrganizationId::new(random_bootstrap_id("org")?).map_err(|_| {
                        Problem::new(
                            "invalid_organization",
                            "Organization ID is invalid",
                            500,
                            "identity",
                        )
                    })?;
                let workspace_id = WorkspaceId::new(random_bootstrap_id("ws")?).map_err(|_| {
                    Problem::new(
                        "invalid_workspace",
                        "Workspace ID is invalid",
                        500,
                        "identity",
                    )
                })?;
                let organization = Organization {
                    id: organization_id.clone(),
                    account_id: account_id.clone(),
                    name: input.organization_name,
                    created_at: created_at.clone(),
                };
                let mut workspaces = WorkspaceService::new(
                    organization,
                    OrganizationPolicy {
                        max_workspaces: input.max_workspaces,
                        offline_entitled: false,
                    },
                )?;
                workspaces.create_workspace(
                    &platform_principal,
                    &[],
                    &organization_id,
                    Workspace {
                        id: workspace_id.clone(),
                        organization_id: organization_id.clone(),
                        name: input.workspace_name,
                        policy: WorkspacePolicy {
                            max_profiles: input.max_profiles,
                            max_members: input.max_members,
                            max_storage_bytes: input.max_storage_bytes,
                            offline_entitled: false,
                        },
                        created_at: created_at.clone(),
                    },
                )?;
                aggregate.workspaces = workspaces;
                (organization_id, true, workspace_id, true)
            };

        let existing_owner_membership = aggregate
            .members
            .active_members()
            .into_iter()
            .find(|member| member.account_id == account_id && member.workspace_id == workspace_id);
        if let Some(member) = existing_owner_membership {
            if member.role != MemberRole::Owner {
                aggregate.members.set_role(
                    &platform_principal,
                    &[],
                    &workspace_id,
                    &member.id,
                    MemberRole::Owner,
                )?;
            }
        } else {
            let member_id = MemberId::new(random_bootstrap_id("member")?).map_err(|_| {
                Problem::new("invalid_member", "Member ID is invalid", 500, "identity")
            })?;
            let expires_at = current.checked_add(600).ok_or_else(|| {
                Problem::new(
                    "member_invitation_expiry_invalid",
                    "Invitation expiry is invalid",
                    500,
                    "identity",
                )
            })?;
            aggregate.members.invite(
                &platform_principal,
                &[],
                Member {
                    id: member_id.clone(),
                    account_id: account_id.clone(),
                    workspace_id: workspace_id.clone(),
                    team_id: None,
                    role: MemberRole::Owner,
                    status: MemberStatus::Invited,
                    invite_expires_at: Some(expires_at),
                },
                current,
            )?;
            aggregate.members.activate(
                &platform_principal,
                &[],
                &workspace_id,
                &member_id,
                current,
            )?;
        }
        aggregate.platform_admin_accounts.insert(account_id.clone());
        let mut puts = Vec::with_capacity(identity_puts.len() + 4);
        if created_account {
            let account = Account::new(account_id.clone(), created_at.clone());
            puts.push(ControlPut {
                tenant_id: CONTROL_TENANT.into(),
                record_type: "account".into(),
                record_id: account_id.as_str().into(),
                expected_revision: Some(0),
                value: serde_json::to_value(account).map_err(|_| {
                    Problem::new(
                        "account_record_invalid",
                        "Account record is invalid",
                        500,
                        "identity",
                    )
                })?,
            });
        }
        puts.append(&mut identity_puts);
        puts.push(password_put);
        puts.push(ControlPut {
            tenant_id: CONTROL_TENANT.into(),
            record_type: CONTROL_STATE_TYPE.into(),
            record_id: CONTROL_STATE_ID.into(),
            expected_revision: Some(aggregate_revision),
            value: serde_json::to_value(&aggregate).map_err(|_| {
                Problem::new(
                    "control_state_invalid",
                    "Control state is invalid",
                    500,
                    "identity",
                )
            })?,
        });
        puts.push(ControlPut {
            tenant_id: IDENTITY_TENANT.into(),
            record_type: BOOTSTRAP_RECORD.into(),
            record_id: BOOTSTRAP_ID.into(),
            expected_revision: Some(0),
            value: json!({"accountId": account_id, "createdAt": created_at, "schema": "buglogin.initial-owner/1"}),
        });

        self.control_persistence.execute(ExecuteControlCommand {
            tenant_id: IDENTITY_TENANT.into(),
            command_id: "identity.bootstrap_initial_owner".into(),
            idempotency_key: BOOTSTRAP_KEY.into(),
            expected_revisions: Default::default(),
            puts,
            deletes: vec![],
            audit: ControlAuditEntry {
                tenant_id: IDENTITY_TENANT.into(),
                command_id: "identity.bootstrap_initial_owner".into(),
                actor_id: "production_operator".into(),
                allowed: true,
                detail: json!({
                    "state": "completed",
                    "accountId": account_id,
                    "organizationId": organization_id,
                    "workspaceId": workspace_id,
                    "createdAccount": created_account,
                    "createdOrganization": created_organization,
                    "createdWorkspace": created_workspace,
                    "identityVerification": "operator-attested-out-of-band"
                }),
            },
            response: HttpResponse {
                status: 201,
                headers: Default::default(),
                body: json!({"accountId": account_id, "organizationId": organization_id, "workspaceId": workspace_id}),
            },
        })?;
        self.reload_control_aggregate()?;

        Ok(FirstOwnerBootstrapResult {
            account_id,
            organization_id,
            workspace_id,
            created_account,
            created_organization,
            created_workspace,
        })
    }

    pub fn with_migration_signing_key(self, key: buglogin_storage::secrets::SecretValue) -> Self {
        *self
            .migration_signing_key
            .write()
            .expect("migration signing key lock") = Some(key);
        self
    }

    pub fn import_initial_signed_migration_package(
        &self,
        package: &NeutralPackage,
    ) -> Result<serde_json::Value, Problem> {
        if package.file("sync/export.json").is_some() {
            return Err(problem(
                "migration_initial_package_invalid",
                "Initial import accepts only the signed control-data package",
                422,
            ));
        }
        let total_bytes = package
            .files()
            .values()
            .try_fold(0_u64, |total, bytes| total.checked_add(bytes.len() as u64));
        if total_bytes.is_none_or(|bytes| bytes > 32 * 1024 * 1024) {
            return Err(problem(
                "migration_initial_package_too_large",
                "Initial control-data package exceeds the 32 MiB limit",
                413,
            ));
        }
        let index_path = buglogin_migration::encrypted_data::index_path();
        let index: buglogin_migration::encrypted_data::EncryptedProfileDataIndex =
            serde_json::from_slice(package.file(index_path).ok_or_else(|| {
                problem(
                    "migration_initial_profile_index_missing",
                    "Initial package profile-data index is missing",
                    422,
                )
            })?)
            .map_err(|_| {
                problem(
                    "migration_initial_profile_index_invalid",
                    "Initial package profile-data index is invalid",
                    422,
                )
            })?;
        if index.schema != "buglogin.encrypted-profile-data/2"
            || !index.files.is_empty()
            || package
                .files()
                .keys()
                .any(|path| path.starts_with("encrypted/profile-data/") && path != index_path)
        {
            return Err(problem(
                "migration_initial_profile_data_present",
                "Initial package must leave browser profile files for authenticated desktop sync",
                422,
            ));
        }
        let signing_key = self.migration_signing_key.read().map_err(|_| {
            problem(
                "migration_signing_key_unavailable",
                "Migration signing key unavailable",
                503,
            )
        })?;
        let signing_key = signing_key.as_ref().ok_or_else(|| {
            problem(
                "migration_signing_key_unavailable",
                "Migration signing key unavailable",
                503,
            )
        })?;
        verify_signed_package(package, signing_key.as_bytes())?;
        let receipt = control_import::import_initial_operator_package(self, package)?;
        serde_json::to_value(receipt).map_err(|_| {
            problem(
                "migration_initial_receipt_invalid",
                "Migration import receipt could not be serialized",
                500,
            )
        })
    }

    pub fn refresh_token(
        &self,
        token: &str,
        current: u64,
        expires_at: u64,
    ) -> Result<String, Problem> {
        self.authenticate_token(token, current)?;
        self.auth
            .write()
            .map_err(|_| problem("auth_unavailable", "Authentication unavailable", 503))?
            .refresh(token, current, expires_at)
    }

    pub fn refresh_token_with_idempotency(
        &self,
        token: &str,
        idempotency_key: Option<&str>,
        current: u64,
        expires_at: u64,
    ) -> Result<(String, u64), Problem> {
        if token.starts_with("blapi_") {
            self.authenticate_token(token, current)?;
            return self
                .auth
                .write()
                .map_err(|_| problem("auth_unavailable", "Authentication unavailable", 503))?
                .refresh_with_idempotency(token, idempotency_key, current, expires_at);
        }
        let mut auth = self
            .auth
            .write()
            .map_err(|_| problem("auth_unavailable", "Authentication unavailable", 503))?;
        let account_id = auth.refresh_account_id(token)?;
        let (_, account_revision) = self
            .account_persistence
            .load_active_versioned(&account_id)?
            .ok_or_else(|| problem("account_record_invalid", "Account record is invalid", 500))?;
        auth.refresh_with_idempotency_and_account_revision(
            token,
            idempotency_key,
            current,
            expires_at,
            Some(account_revision),
        )
    }

    pub fn revoke_token(&self, token: &str) -> Result<(), Problem> {
        self.auth
            .write()
            .map_err(|_| problem("auth_unavailable", "Authentication unavailable", 503))?
            .revoke(token, checked_now("token-revoke")?)
    }

    pub fn revoke_token_with_idempotency(
        &self,
        token: &str,
        idempotency_key: &str,
    ) -> Result<(), Problem> {
        self.auth
            .write()
            .map_err(|_| problem("auth_unavailable", "Authentication unavailable", 503))?
            .revoke_with_idempotency(token, idempotency_key, checked_now("token-revoke")?)
    }

    pub fn revoke_browser_session_with_idempotency(
        &self,
        raw_session: &str,
        raw_csrf: &str,
        idempotency_key: &str,
    ) -> Result<(), Problem> {
        self.auth
            .write()
            .map_err(|_| problem("auth_unavailable", "Authentication unavailable", 503))?
            .revoke_browser_with_idempotency(
                raw_session,
                raw_csrf,
                idempotency_key,
                checked_now("browser-session-revoke")?,
            )
    }

    pub fn issue_token(&self, account_id: impl Into<String>) -> Result<String, Problem> {
        let account_id = buglogin_domain::ids::AccountId::new(account_id.into())
            .map_err(|_| problem("invalid_account", "Invalid account", 400))?;
        let current = checked_now("api-token-issue")?;
        if self.local_account_provisioning {
            self.account_persistence
                .provision_default(account_id.clone(), current.to_string())?;
        }
        let (_, account_revision) = self
            .account_persistence
            .load_active_versioned(&account_id)?
            .ok_or_else(|| {
                problem(
                    "account_provisioning_disabled",
                    "Account provisioning is disabled",
                    403,
                )
            })?;
        let expires_at = crate::clock::checked_add_seconds(current, 3600, "api-token-issue")?;
        let mut auth = self
            .auth
            .write()
            .map_err(|_| problem("auth_unavailable", "Authentication unavailable", 503))?;
        match self.credentials.password_version(&account_id)? {
            Some(version) => auth.issue_with_credential_version_and_account_revision(
                account_id,
                expires_at,
                version,
                account_revision,
            ),
            None => auth.issue_with_account_revision(account_id, expires_at, account_revision),
        }
    }
}

impl ControlAggregate {
    pub(crate) fn mark_migrated_profile_ready_after_sync(
        &mut self,
        manifest: &SyncManifest,
    ) -> Result<bool, Problem> {
        let has_encrypted_profile_data = manifest.payload_encryption.is_some()
            && (manifest
                .file_chunks
                .iter()
                .any(|file| !file.path.starts_with(".buglogin/"))
                || manifest
                    .objects
                    .iter()
                    .any(|object| !object.key.starts_with(".buglogin/")));
        if !has_encrypted_profile_data {
            return Ok(false);
        }
        let profile_id = self
            .profile_resources
            .values()
            .find(|binding| {
                binding.workspace_id == manifest.workspace_id
                    && binding.resource_id == manifest.resource_id
                    && binding.archive_state == ProfileArchiveState::Active
            })
            .map(|binding| binding.profile_id.clone());
        let Some(profile_id) = profile_id else {
            return Ok(false);
        };
        let Some(profile) = self.profiles.get_mut(&profile_id) else {
            return Ok(false);
        };
        if profile.value.state == ProfileState::Created
            && profile.value.runtime != buglogin_domain::entities::Runtime::Preserved
        {
            let next_revision = profile.revision.checked_add(1).ok_or_else(|| {
                Problem::new(
                    "profile_revision_exhausted",
                    "Profile revision cannot be incremented",
                    409,
                    "profile",
                )
            })?;
            profile.value.state = ProfileState::Ready;
            profile.revision = next_revision;
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AuthTokenResponse {
    access_token: String,
    token_type: &'static str,
    expires_at: u64,
}

impl Drop for AuthTokenResponse {
    fn drop(&mut self) {
        self.access_token.zeroize();
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DeviceAuthTokenResponse {
    access_token: String,
    refresh_token: String,
    token_type: &'static str,
    expires_at: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DesktopLoginInput {
    identifier: String,
    password: String,
    device_id: String,
}

impl Drop for DesktopLoginInput {
    fn drop(&mut self) {
        self.identifier.zeroize();
        self.password.zeroize();
        self.device_id.zeroize();
    }
}

impl Drop for DeviceAuthTokenResponse {
    fn drop(&mut self) {
        self.access_token.zeroize();
        self.refresh_token.zeroize();
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WebLoginInput {
    identifier: String,
    password: String,
}

impl Drop for WebLoginInput {
    fn drop(&mut self) {
        self.identifier.zeroize();
        self.password.zeroize();
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WebSessionResponse {
    account_id: AccountId,
    expires_at: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WebSessionPage {
    sessions: Vec<SessionSummary>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RecoveryRequestInput {
    identifier: String,
}

impl Drop for RecoveryRequestInput {
    fn drop(&mut self) {
        self.identifier.zeroize();
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RecoveryCompleteInput {
    token: String,
    new_password: String,
}

impl Drop for RecoveryCompleteInput {
    fn drop(&mut self) {
        self.token.zeroize();
        self.new_password.zeroize();
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ApiTokenCreateInput {
    name: String,
    scopes: BTreeSet<String>,
    expires_at: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ApiTokenCreated {
    token: String,
    token_record: ApiTokenSummary,
}

impl Drop for ApiTokenCreated {
    fn drop(&mut self) {
        self.token.zeroize();
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ApiTokenPage {
    tokens: Vec<ApiTokenSummary>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DeviceAuthorizationStartInput {
    device_id: String,
    client_name: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DeviceAuthorizationStartResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    expires_in: u64,
    interval: u64,
}

impl Drop for DeviceAuthorizationStartResponse {
    fn drop(&mut self) {
        self.device_code.zeroize();
        self.user_code.zeroize();
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DeviceAuthorizationApproveInput {
    user_code: String,
}

impl Drop for DeviceAuthorizationApproveInput {
    fn drop(&mut self) {
        self.user_code.zeroize();
    }
}

#[derive(Serialize)]
struct DeviceAuthorizationApproveResponse {
    approved: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DeviceAuthorizationTokenInput {
    device_code: String,
}

impl Drop for DeviceAuthorizationTokenInput {
    fn drop(&mut self) {
        self.device_code.zeroize();
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DeviceRefreshInput {
    refresh_token: String,
    device_id: String,
}

impl Drop for DeviceRefreshInput {
    fn drop(&mut self) {
        self.refresh_token.zeroize();
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DeviceEncryptionKeyRegistrationInput {
    public_key: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Health {
    status: &'static str,
    service: &'static str,
}

async fn readyz(State(state): State<ApiState>) -> Result<Response, Response> {
    state.readiness().await.map_err(response)?;
    if state.require_runtime_catalog_for_readiness && state.runtime_catalog.is_none() {
        return Err(response(
            problem(
                "runtime_catalog_unavailable",
                "Signed runtime catalog is not configured",
                503,
            )
            .retryable(true),
        ));
    }
    let runtime_catalog = if state.runtime_catalog.is_some() {
        "available"
    } else {
        "unavailable"
    };
    let mut response = Json(Health {
        status: "ready",
        service: "buglogin-api",
    })
    .into_response();
    response.headers_mut().insert(
        header::HeaderName::from_static("x-buglogin-persistence-mode"),
        HeaderValue::from_static(state.control_persistence_mode),
    );
    response.headers_mut().insert(
        header::HeaderName::from_static("x-buglogin-runtime-catalog"),
        HeaderValue::from_static(runtime_catalog),
    );
    Ok(response)
}

impl ApiAuthorizationAuthority {
    fn authenticate_token(&self, token: &str, now_ms: u64) -> Result<Principal, Problem> {
        let (account_id, token_scopes) = if token.starts_with("blapi_") {
            let (account_id, scopes) = self.api_tokens.authenticate(token, now_ms)?;
            (account_id, Some(scopes))
        } else {
            let account_id = self
                .auth
                .read()
                .map_err(|_| {
                    Problem::new(
                        "auth_unavailable",
                        "Authentication unavailable",
                        503,
                        "identity",
                    )
                })?
                .authenticate(token, now_ms)?;
            (account_id, None)
        };
        let account = match self.account_persistence.load(&account_id)? {
            Some(account) if account.id == account_id => account,
            _ => {
                return Err(Problem::new(
                    "account_record_invalid",
                    "Account record is invalid",
                    500,
                    "identity",
                ));
            }
        };
        if account.status != AccountStatus::Active {
            return Err(Problem::new(
                "account_inactive",
                "Account is not active",
                403,
                "identity",
            ));
        }
        let memberships = self
            .members
            .read()
            .map_err(|_| {
                Problem::new(
                    "member_unavailable",
                    "Member state unavailable",
                    503,
                    "identity",
                )
            })?
            .active_for_account(&account_id);
        let mut principal = Principal {
            account_id: Some(account_id),
            roles: BTreeSet::from([Role::Account]),
            token_scopes,
            ..Principal::default()
        };
        if self
            .platform_admin_accounts
            .read()
            .map_err(|_| {
                Problem::new(
                    "permission_unavailable",
                    "Permission service unavailable",
                    503,
                    "identity",
                )
            })?
            .contains(principal.account_id.as_ref().expect("account assigned"))
        {
            principal.roles.insert(Role::PlatformAdmin);
        }
        for member in memberships {
            principal.workspace_ids.insert(member.workspace_id.clone());
            if principal.member_id.is_none() {
                principal.member_id = Some(member.id);
            } else {
                principal.member_id = None;
            }
        }
        Ok(principal)
    }

    fn coordination_actor_matches(
        &self,
        token: &str,
        principal: &Principal,
        actor_id: &str,
        now_ms: u64,
    ) -> Result<bool, Problem> {
        if principal
            .account_id
            .as_ref()
            .is_some_and(|account_id| account_id.as_str() == actor_id)
        {
            return Ok(true);
        }
        if token.starts_with("blapi_") {
            return Ok(false);
        }
        let (account_id, device_id) = self
            .auth
            .read()
            .map_err(|_| {
                Problem::new(
                    "auth_unavailable",
                    "Authentication unavailable",
                    503,
                    "identity",
                )
            })?
            .authenticate_device(token, now_ms)?;
        Ok(principal.account_id.as_ref() == Some(&account_id) && device_id == actor_id)
    }

    fn grants(&self) -> Result<Vec<Grant>, Problem> {
        let mut grants = self.grants.read().map(|value| value.clone()).map_err(|_| {
            Problem::new(
                "permission_unavailable",
                "Permission service unavailable",
                503,
                "mcp",
            )
        })?;
        let members = self
            .members
            .read()
            .map_err(|_| {
                Problem::new("member_unavailable", "Member state unavailable", 503, "mcp")
            })?
            .active_members();
        let materialized = self
            .materialized_member_grants
            .read()
            .map_err(|_| {
                Problem::new(
                    "permission_unavailable",
                    "Permission service unavailable",
                    503,
                    "mcp",
                )
            })?
            .clone();
        const CAPABILITIES: [&str; 15] = control_import::MEMBERSHIP_CAPABILITIES;
        const ALL: [Action; 7] = [
            Action::Read,
            Action::Create,
            Action::Update,
            Action::Delete,
            Action::Execute,
            Action::Share,
            Action::Manage,
        ];
        const OPERATE: [Action; 5] = [
            Action::Read,
            Action::Create,
            Action::Update,
            Action::Execute,
            Action::Share,
        ];
        const USE: [Action; 2] = [Action::Read, Action::Execute];
        for member in members {
            if materialized.contains(&member.id) {
                continue;
            }
            let actions: &[Action] = match member.role {
                MemberRole::Owner | MemberRole::Admin => &ALL,
                MemberRole::Operator => &OPERATE,
                MemberRole::Member => &USE,
            };
            for capability_name in CAPABILITIES {
                let capability = Capability::new(capability_name).map_err(|_| {
                    Problem::new("invalid_capability", "Invalid capability", 500, "mcp")
                })?;
                for action in actions {
                    grants.push(Grant {
                        id: GrantId::new(format!(
                            "membership:{}:{capability_name}:{action:?}",
                            member.id.as_str()
                        ))
                        .map_err(|_| {
                            Problem::new("invalid_grant", "Invalid derived grant", 500, "mcp")
                        })?,
                        account_id: Some(member.account_id.clone()),
                        member_id: None,
                        capability: capability.clone(),
                        action: *action,
                        scope: ResourceScope::workspace(member.workspace_id.clone()),
                        effect: Effect::Allow,
                    });
                }
            }
        }
        Ok(grants)
    }
}

pub fn coordination_authorizer(state: &ApiState) -> Arc<dyn CoordinationAuthorizer> {
    Arc::new(ApiCoordinationAuthorizer {
        authority: state.authorization_authority(),
    })
}

pub(crate) async fn read_workspace_events(
    state: &ApiState,
    auth: &CoordinationAuth,
    workspace_id: &WorkspaceId,
    after_stream_id: Option<&str>,
    limit: usize,
) -> Result<serde_json::Value, Problem> {
    if !(1..=1_000).contains(&limit) {
        return Err(problem(
            "invalid_event_read_limit",
            "Event limit is invalid",
            400,
        ));
    }
    if after_stream_id.is_some_and(|cursor| {
        let Some((milliseconds, sequence)) = cursor.split_once('-') else {
            return true;
        };
        milliseconds.parse::<u64>().is_err()
            || sequence.parse::<u64>().is_err()
            || sequence.contains('-')
    }) {
        return Err(problem(
            "invalid_event_cursor",
            "Event cursor is invalid",
            400,
        ));
    }
    let backend = state
        .coordination
        .read()
        .map_err(|_| {
            problem(
                "coordination_unavailable",
                "Coordination is unavailable",
                503,
            )
        })?
        .clone()
        .ok_or_else(|| {
            problem(
                "coordination_unavailable",
                "Hosted event stream is unavailable",
                503,
            )
        })?;
    let events = backend
        .read_events(
            CoordinationScope { workspace_id, auth },
            after_stream_id,
            limit,
        )
        .await?;
    let next_cursor = events.last().map(|event| event.stream_id.clone());
    Ok(serde_json::json!({
        "items": events.into_iter().map(|event| serde_json::json!({
            "streamId": event.stream_id,
            "event": event.event,
        })).collect::<Vec<_>>(),
        "nextCursor": next_cursor,
    }))
}

struct ApiCoordinationAuthorizer {
    authority: ApiAuthorizationAuthority,
}

#[async_trait]
impl CoordinationAuthorizer for ApiCoordinationAuthorizer {
    async fn authorize(
        &self,
        auth: &CoordinationAuth,
        request: AuthorizationRequest<'_>,
    ) -> Result<(), Problem> {
        let current = checked_now("coordination-auth")?;
        let principal = self
            .authority
            .authenticate_token(auth.bearer_token(), current)?;
        if !self.authority.coordination_actor_matches(
            auth.bearer_token(),
            &principal,
            auth.actor_id(),
            current,
        )? {
            return Err(problem(
                "coordination_actor_mismatch",
                "Coordination actor must match the authenticated account",
                403,
            ));
        }
        let (capability, action) = match request.operation {
            CoordinationOperation::LeaseAcquire | CoordinationOperation::LeaseRenew => {
                ("profile.launch", Action::Execute)
            }
            CoordinationOperation::LeaseRelease => ("profile.stop", Action::Execute),
            CoordinationOperation::LeaseRead => ("profile.read", Action::Read),
            CoordinationOperation::ManifestRead => ("profile.read", Action::Read),
            CoordinationOperation::ManifestWrite => ("profile.update", Action::Update),
            CoordinationOperation::PayloadWrite => ("profile.update", Action::Update),
            CoordinationOperation::EventRead => ("events.read", Action::Read),
            CoordinationOperation::EventPublish => {
                return Err(problem(
                    "coordination_publish_forbidden",
                    "Event publication is restricted to trusted API services",
                    403,
                ));
            }
            _ => {
                return Err(problem(
                    "coordination_operation_invalid",
                    "Coordination operation is invalid",
                    400,
                ));
            }
        };
        let capability = Capability::new(capability)
            .map_err(|_| problem("invalid_capability", "Invalid capability", 500))?;
        let scope = ResourceScope {
            workspace_id: Some(request.workspace_id.clone()),
            resource_id: request.resource_id.cloned(),
            ..ResourceScope::default()
        };
        let grants = self.authority.grants()?;
        if is_allowed(&principal, &capability, action, &scope, &grants) {
            Ok(())
        } else {
            Err(problem(
                "coordination_forbidden",
                "Coordination access denied",
                403,
            ))
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RevisionInput {
    revision: u64,
}

#[derive(Debug)]
struct ProfilePageQuery {
    cursor: Option<String>,
    limit: Option<usize>,
    workspace_id: Option<String>,
    include_archived: bool,
}

fn parse_profile_page_query(raw_query: Option<&str>) -> Result<ProfilePageQuery, Problem> {
    let mut query = ProfilePageQuery {
        cursor: None,
        limit: None,
        workspace_id: None,
        include_archived: false,
    };
    let mut seen = BTreeSet::new();
    for (key, value) in url::form_urlencoded::parse(raw_query.unwrap_or_default().as_bytes()) {
        if !matches!(
            key.as_ref(),
            "cursor" | "limit" | "workspaceId" | "includeArchived"
        ) {
            return Err(problem(
                "invalid_query_parameter",
                "Profile list query contains an unsupported parameter",
                400,
            ));
        }
        if !seen.insert(key.to_string()) {
            return Err(problem(
                "duplicate_query_parameter",
                "Profile list query parameters must not be repeated",
                400,
            ));
        }
        match key.as_ref() {
            "cursor" => query.cursor = Some(value.into_owned()),
            "limit" => {
                query.limit = Some(value.parse::<usize>().map_err(|_| {
                    problem(
                        "invalid_page_limit",
                        "Profile page limit must be between 1 and 100",
                        400,
                    )
                })?);
            }
            "workspaceId" => query.workspace_id = Some(value.into_owned()),
            "includeArchived" => {
                query.include_archived = match value.as_ref() {
                    "true" => true,
                    "false" => false,
                    _ => {
                        return Err(problem(
                            "invalid_include_archived",
                            "includeArchived must be a boolean",
                            400,
                        ));
                    }
                };
            }
            _ => unreachable!("query key was validated above"),
        }
    }
    Ok(query)
}

#[cfg(test)]
mod profile_page_query_tests {
    use super::parse_profile_page_query;

    #[test]
    fn profile_page_query_defaults_and_parses_canonical_fields() {
        let defaults = parse_profile_page_query(None).unwrap();
        assert_eq!(defaults.limit, None);
        assert_eq!(defaults.cursor, None);
        assert_eq!(defaults.workspace_id, None);
        assert!(!defaults.include_archived);

        let parsed = parse_profile_page_query(Some(
            "cursor=prf_1&limit=50&workspaceId=ws_1&includeArchived=true",
        ))
        .unwrap();
        assert_eq!(parsed.cursor.as_ref().map(|id| id.as_str()), Some("prf_1"));
        assert_eq!(parsed.limit, Some(50));
        assert_eq!(
            parsed.workspace_id.as_ref().map(|id| id.as_str()),
            Some("ws_1")
        );
        assert!(parsed.include_archived);
    }

    #[test]
    fn profile_page_query_rejects_malformed_duplicate_and_unknown_values() {
        for (raw, expected_code) in [
            ("limit=many", "invalid_page_limit"),
            ("includeArchived=1", "invalid_include_archived"),
            ("limit=10&limit=20", "duplicate_query_parameter"),
            ("unknown=value", "invalid_query_parameter"),
        ] {
            let error = parse_profile_page_query(Some(raw)).unwrap_err();
            assert_eq!(error.code, expected_code, "query: {raw}");
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LaunchInput {
    profiles_root: PathBuf,
    extra_args: Vec<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DeleteInput {
    revision: u64,
    confirmation: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CookieListQuery {
    workspace_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CookieListHttpInput {
    workspace_id: String,
    profile_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CookieJobWorkspaceQuery {
    workspace_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EnqueueCookieJobInput {
    command_id: String,
    workspace_id: String,
    device_id: String,
    profile_id: String,
    expected_generation: u64,
    #[serde(default)]
    source_workspace_id: Option<String>,
    #[serde(default)]
    source_profile_id: Option<String>,
    #[serde(default)]
    source_device_id: Option<String>,
    #[serde(default)]
    staged_import_ref: Option<String>,
}

pub fn router(state: ApiState) -> Router {
    let allowed_web_origin = state
        .web_origin
        .parse::<HeaderValue>()
        .expect("configured web origin must be a valid HTTP header value");
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(move |origin, _| {
            origin == &allowed_web_origin
        }))
        .allow_credentials(true)
        .allow_methods([
            axum::http::Method::GET,
            axum::http::Method::POST,
            axum::http::Method::PUT,
            axum::http::Method::PATCH,
            axum::http::Method::DELETE,
            axum::http::Method::OPTIONS,
        ])
        .allow_headers([
            header::ACCEPT,
            header::AUTHORIZATION,
            header::CONTENT_TYPE,
            axum::http::HeaderName::from_static("idempotency-key"),
            axum::http::HeaderName::from_static("x-buglogin-actor-id"),
            axum::http::HeaderName::from_static("x-buglogin-fencing-token"),
            axum::http::HeaderName::from_static("x-buglogin-generation"),
            axum::http::HeaderName::from_static("x-buglogin-owner-id"),
            axum::http::HeaderName::from_static("x-buglogin-owner-token"),
            axum::http::HeaderName::from_static("x-content-sha256"),
            axum::http::HeaderName::from_static("x-correlation-id"),
            axum::http::HeaderName::from_static("x-csrf-token"),
        ])
        .expose_headers([axum::http::HeaderName::from_static("x-correlation-id")]);
    Router::new()
        .route(
            "/v1/auth/login",
            post(web_login).layer(DefaultBodyLimit::max(AUTH_JSON_BODY_LIMIT_BYTES)),
        )
        .route(
            "/v1/auth/desktop/login",
            post(desktop_login).layer(DefaultBodyLimit::max(AUTH_JSON_BODY_LIMIT_BYTES)),
        )
        .route(
            "/v1/auth/sso/buglab/start",
            post(sso::start).layer(DefaultBodyLimit::max(AUTH_JSON_BODY_LIMIT_BYTES)),
        )
        .route(
            "/v1/auth/sso/buglab/complete",
            post(sso::complete).layer(DefaultBodyLimit::max(AUTH_JSON_BODY_LIMIT_BYTES)),
        )
        .route(
            "/v1/auth/recovery/request",
            post(request_recovery).layer(DefaultBodyLimit::max(AUTH_JSON_BODY_LIMIT_BYTES)),
        )
        .route(
            "/v1/auth/recovery/complete",
            post(complete_recovery).layer(DefaultBodyLimit::max(AUTH_JSON_BODY_LIMIT_BYTES)),
        )
        .route(
            "/v1/auth/api-tokens",
            get(list_api_tokens)
                .post(create_api_token)
                .layer(DefaultBodyLimit::max(AUTH_JSON_BODY_LIMIT_BYTES)),
        )
        .route(
            "/v1/auth/api-tokens/{token_id}",
            axum::routing::delete(revoke_api_token),
        )
        .route("/v1/auth/session", get(web_session))
        .route("/v1/auth/sessions", get(web_sessions))
        .route(
            "/v1/auth/sessions/{session_id}",
            axum::routing::delete(revoke_web_session),
        )
        .route(
            "/v1/auth/device/start",
            post(start_device_authorization)
                .layer(DefaultBodyLimit::max(AUTH_JSON_BODY_LIMIT_BYTES)),
        )
        .route(
            "/v1/auth/device/approve",
            post(approve_device_authorization)
                .layer(DefaultBodyLimit::max(AUTH_JSON_BODY_LIMIT_BYTES)),
        )
        .route(
            "/v1/auth/device/token",
            post(exchange_device_authorization)
                .layer(DefaultBodyLimit::max(AUTH_JSON_BODY_LIMIT_BYTES)),
        )
        .route(
            "/v1/auth/device/refresh",
            post(refresh_device_session).layer(DefaultBodyLimit::max(AUTH_JSON_BODY_LIMIT_BYTES)),
        )
        .route(
            "/v1/auth/device/encryption-key",
            put(register_device_encryption_key)
                .layer(DefaultBodyLimit::max(AUTH_JSON_BODY_LIMIT_BYTES)),
        )
        .route("/v1/auth/refresh", post(refresh_token))
        .route("/v1/auth/logout", post(logout))
        .route(
            "/v1/health",
            get(|| async {
                Json(Health {
                    status: "ok",
                    service: "buglogin-api",
                })
            }),
        )
        .route("/v1/openapi.yaml", get(openapi_document))
        .route("/v1/readyz", get(readyz))
        .route("/v1/runtime/catalog", get(get_runtime_catalog))
        .route("/v1/proxies/parse", post(parse_proxy))
        .route("/v1/profiles", get(list_profiles).post(create_profile))
        .route("/v1/profiles/{id}", get(get_profile).delete(delete_profile))
        .route("/v1/profiles/{id}/binding", get(get_profile_binding))
        .route("/v1/profiles/{id}/archive", put(archive_profile))
        .route("/v1/profiles/{id}/restore", put(restore_profile))
        .route("/v1/profiles/{id}/clone", post(clone_profile))
        .route("/v1/profiles/{id}/shares", post(share_profile))
        .route(
            "/v1/profiles/{id}/shares/{member_id}",
            axum::routing::delete(revoke_profile_share),
        )
        .route(
            "/v1/profiles/{id}/shares/{member_id}/device-keys",
            get(profile_share_recipient_device_keys),
        )
        .route(
            "/v1/profiles/{id}/key-envelope",
            get(get_profile_key_share_envelope),
        )
        .route("/v1/profiles/{id}/launch", post(launch_profile))
        .route("/v1/profiles/{id}/stop", post(stop_profile))
        .route("/v1/profiles/{id}/cookies", get(list_cookies))
        .route("/v1/cookie-jobs", post(enqueue_cookie_job))
        .route("/v1/cookie-jobs/{id}", get(get_cookie_job))
        .route(
            "/v1/workspaces/{workspace_id}/cookie-jobs/inbox",
            get(cookie_job_inbox),
        )
        .route("/v1/device/cookie-jobs/inbox", get(cookie_job_inbox_all))
        .route("/v1/cookie-jobs/{id}/claim", post(claim_cookie_job))
        .route("/v1/cookie-jobs/{id}/ack", post(ack_cookie_job))
        .route("/v1/cookie-jobs/{id}/fail", post(fail_cookie_job))
        .route("/v1/cookie-jobs/{id}/cancel", post(cancel_cookie_job))
        .route("/v1/cookie-jobs/{id}/complete", post(complete_cookie_job))
        .route("/v1/operations/{id}", get(get_operation))
        .route("/v1/events", get(read_events))
        .route("/v1/migrations/inspect", post(inspect_migration))
        .route("/v1/migrations/import", post(import_migration))
        .route(
            "/v1/migrations/control/import",
            post(control_import::import_control),
        )
        .route("/v1/migrations/sync/import", post(sync_import::import_sync))
        .route(
            "/v1/migrations/packages/{package_id}/files",
            put(package_upload::put_package_file),
        )
        .route(
            "/v1/migrations/packages/{package_id}/verify",
            post(package_upload::verify_package_upload),
        )
        .route(
            "/v1/migrations/packages/{package_id}/activate",
            post(package_upload::activate_package_upload),
        )
        .route(
            "/v1/migrations/control/rollback",
            post(control_import::rollback_control),
        )
        .merge(control::routes())
        .layer(middleware::from_fn_with_state(
            state.clone(),
            control::refresh_hosted_snapshot,
        ))
        .layer(middleware::from_fn(require_correlation))
        .layer(cors)
        .with_state(state)
}

static LARGE_TRANSFER_BODY_SLOTS: OnceLock<Arc<tokio::sync::Semaphore>> = OnceLock::new();

pub(crate) async fn large_transfer_body_slot() -> Result<tokio::sync::OwnedSemaphorePermit, Problem>
{
    let semaphore = Arc::clone(
        LARGE_TRANSFER_BODY_SLOTS.get_or_init(|| Arc::new(tokio::sync::Semaphore::new(2))),
    );
    tokio::time::timeout(Duration::from_secs(30), semaphore.acquire_owned())
        .await
        .map_err(|_| {
            Problem::new(
                "large_transfer_capacity_exhausted",
                "Large transfer capacity remained busy for 30 seconds; retry the request",
                503,
                "api",
            )
            .retryable(true)
        })?
        .map_err(|_| {
            Problem::new(
                "large_transfer_capacity_unavailable",
                "Large transfer capacity is unavailable",
                503,
                "api",
            )
            .retryable(true)
        })
}

pub(crate) fn declared_body_exceeds_limit(
    headers: &HeaderMap,
    max_bytes: usize,
) -> Result<bool, Problem> {
    let Some(value) = headers.get(header::CONTENT_LENGTH) else {
        return Ok(false);
    };
    let declared = value
        .to_str()
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| {
            Problem::new(
                "content_length_invalid",
                "Content-Length header is invalid",
                400,
                "api",
            )
        })?;
    Ok(declared > max_bytes as u64)
}

const WEB_SESSION_COOKIE: &str = "__Host-buglogin_session";
const WEB_CSRF_COOKIE: &str = "__Host-buglogin_csrf";
const WEB_SESSION_SECONDS: u64 = 12 * 60 * 60;
const AUTH_JSON_BODY_LIMIT_BYTES: usize = 8 * 1024;

async fn create_api_token(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(input): Json<ApiTokenCreateInput>,
) -> Result<Response, Response> {
    let (account_id, credential_version) =
        authenticated_account_and_credential_version_for_mutation(&state, &headers)?;
    let credential_version = credential_version.ok_or_else(|| {
        response(problem(
            "password_credential_unavailable",
            "API tokens require a password-backed account",
            409,
        ))
    })?;
    let _auth = state.auth.write().map_err(|_| {
        response(problem(
            "auth_unavailable",
            "Authentication unavailable",
            503,
        ))
    })?;
    require_active_account(&state, account_id.clone())?;
    let (_, account_revision) = state
        .account_persistence
        .load_active_versioned(&account_id)
        .map_err(response)?
        .ok_or_else(|| {
            response(problem(
                "account_record_invalid",
                "Account record is invalid",
                500,
            ))
        })?;
    let issued = state
        .api_tokens
        .issue_with_account_revision(
            account_id,
            credential_version,
            &input.name,
            input.scopes,
            input.expires_at,
            request_now(&headers)?,
            Some(account_revision),
        )
        .map_err(|error| {
            if error.code == "storage_revision_conflict" {
                response(
                    problem(
                        "account_status_changed",
                        "Account state changed; retry the request",
                        409,
                    )
                    .retryable(true),
                )
            } else {
                response(error)
            }
        })?;
    let mut response = sensitive_json(ApiTokenCreated {
        token: issued.token,
        token_record: issued.summary,
    });
    *response.status_mut() = StatusCode::CREATED;
    Ok(response)
}

async fn list_api_tokens(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Response, Response> {
    let account_id = if headers.get(header::AUTHORIZATION).is_some() {
        authenticate(&state, &headers)?.account_id.ok_or_else(|| {
            response(problem(
                "authentication_required",
                "Authentication required",
                401,
            ))
        })?
    } else {
        let raw_session = cookie(&headers, WEB_SESSION_COOKIE)?;
        let raw_csrf = cookie(&headers, WEB_CSRF_COOKIE)?;
        state
            .auth
            .read()
            .map_err(|_| {
                response(problem(
                    "auth_unavailable",
                    "Authentication unavailable",
                    503,
                ))
            })?
            .authenticate_browser(raw_session, raw_csrf, request_now(&headers)?)
            .map_err(response)?
    };
    let account_id = require_active_account(&state, account_id)?;
    Ok(sensitive_json(ApiTokenPage {
        tokens: state.api_tokens.list(&account_id).map_err(response)?,
    }))
}

async fn revoke_api_token(
    State(state): State<ApiState>,
    Path(token_id): Path<String>,
    headers: HeaderMap,
) -> Result<StatusCode, Response> {
    let idempotency_key = required_idempotency(&headers)?;
    let account_id = authenticated_account_for_mutation(&state, &headers)?;
    state
        .api_tokens
        .revoke(
            &account_id,
            &token_id,
            idempotency_key,
            request_now(&headers)?,
        )
        .map_err(response)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn request_recovery(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(input): Json<RecoveryRequestInput>,
) -> Result<StatusCode, Response> {
    require_web_origin(&state, &headers)?;
    if input.identifier.trim().is_empty() || input.identifier.len() > 254 {
        return Err(response(problem(
            "recovery_request_invalid",
            "Recovery request is invalid",
            400,
        )));
    }
    state
        .recovery
        .request(&input.identifier, request_now(&headers)?)
        .map_err(response)?;
    Ok(StatusCode::ACCEPTED)
}

async fn complete_recovery(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(input): Json<RecoveryCompleteInput>,
) -> Result<StatusCode, Response> {
    require_web_origin(&state, &headers)?;
    let idempotency_key = required_idempotency(&headers)?;
    let mut auth = state.auth.write().map_err(|_| {
        response(problem(
            "auth_unavailable",
            "Authentication unavailable",
            503,
        ))
    })?;
    let account_id = state
        .recovery
        .complete(
            &input.token,
            &input.new_password,
            idempotency_key,
            request_now(&headers)?,
        )
        .map_err(response)?;
    auth.mark_account_sessions_revoked(&account_id, request_now(&headers)?);
    Ok(StatusCode::NO_CONTENT)
}

async fn web_login(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(input): Json<WebLoginInput>,
) -> Result<Response, Response> {
    require_web_origin(&state, &headers)?;
    let current = request_now(&headers)?;
    let (account_id, credential_version) = state
        .credentials
        .authenticate_versioned(&input.identifier, &input.password, current)
        .map_err(response)?;
    let (_, account_revision) = state
        .account_persistence
        .load_active_versioned(&account_id)
        .map_err(response)?
        .ok_or_else(|| {
            response(problem(
                "account_record_invalid",
                "Account record is invalid",
                500,
            ))
        })?;
    let expires_at = crate::clock::checked_add_seconds(current, WEB_SESSION_SECONDS, "web-login")
        .map_err(response)?;
    let mut auth = state.auth.write().map_err(|_| {
        response(problem(
            "auth_unavailable",
            "Authentication unavailable",
            503,
        ))
    })?;
    // Keep the local serialization for same-process revocation and fence cross-node
    // suspension with the account record revision in the durable session transaction.
    require_active_account(&state, account_id.clone())?;
    let issued = auth
        .issue_browser_with_account_revision(
            account_id.clone(),
            expires_at,
            current,
            credential_version,
            account_revision,
        )
        .map_err(|error| {
            if error.code == "storage_revision_conflict" {
                response(problem(
                    "account_status_changed",
                    "Account state changed; sign in again",
                    409,
                ))
            } else {
                response(error)
            }
        })?;
    let mut result = Json(WebSessionResponse {
        account_id,
        expires_at,
    })
    .into_response();
    result
        .headers_mut()
        .append(header::SET_COOKIE, session_cookie(&issued.raw_session)?);
    result
        .headers_mut()
        .append(header::SET_COOKIE, csrf_cookie(&issued.raw_csrf)?);
    Ok(no_store(result))
}

async fn desktop_login(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(input): Json<DesktopLoginInput>,
) -> Result<Response, Response> {
    let correlation_id = correlation(&headers)?;
    let device_id = DeviceId::new(input.device_id.clone()).map_err(|_| {
        response(problem(
            "invalid_device_id",
            "Desktop device identity is invalid",
            400,
        ))
    })?;
    let current = request_now(&headers)?;
    let (account_id, credential_version) = state
        .credentials
        .authenticate_versioned(&input.identifier, &input.password, current)
        .map_err(response)?;
    let (_, account_revision) = state
        .account_persistence
        .load_active_versioned(&account_id)
        .map_err(response)?
        .ok_or_else(|| {
            response(problem(
                "account_record_invalid",
                "Account record is invalid",
                500,
            ))
        })?;
    let expires_at =
        crate::clock::checked_add_seconds(current, 3600, &correlation_id).map_err(response)?;
    let refresh_expires_at =
        crate::clock::checked_add_seconds(current, 30 * 24 * 60 * 60, &correlation_id)
            .map_err(response)?;
    let mut auth = state.auth.write().map_err(|_| {
        response(problem(
            "auth_unavailable",
            "Authentication unavailable",
            503,
        ))
    })?;
    let mut tokens = auth
        .issue_device_session_with_account_revision(
            account_id,
            expires_at,
            refresh_expires_at,
            device_id.as_str().to_owned(),
            Some(credential_version),
            Some(account_revision),
        )
        .map_err(|error| {
            if error.code == "storage_revision_conflict" {
                response(problem(
                    "account_status_changed",
                    "Account state changed during sign-in; sign in again",
                    409,
                ))
            } else {
                response(error)
            }
        })?;
    Ok(sensitive_json(DeviceAuthTokenResponse {
        access_token: std::mem::take(&mut tokens.access_token),
        refresh_token: std::mem::take(&mut tokens.refresh_token),
        token_type: "Bearer",
        expires_at,
    }))
}

async fn web_session(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Response, Response> {
    let raw_session = cookie(&headers, WEB_SESSION_COOKIE)?;
    let raw_csrf = cookie(&headers, WEB_CSRF_COOKIE)?;
    let (account_id, expires_at) = state
        .auth
        .read()
        .map_err(|_| {
            response(problem(
                "auth_unavailable",
                "Authentication unavailable",
                503,
            ))
        })?
        .browser_session(raw_session, raw_csrf, request_now(&headers)?)
        .map_err(response)?;
    let account_id = require_active_account(&state, account_id)?;
    Ok(sensitive_json(WebSessionResponse {
        account_id,
        expires_at,
    }))
}

async fn web_sessions(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Response, Response> {
    let raw_session = cookie(&headers, WEB_SESSION_COOKIE)?;
    let raw_csrf = cookie(&headers, WEB_CSRF_COOKIE)?;
    let account_id = state
        .auth
        .read()
        .map_err(|_| {
            response(problem(
                "auth_unavailable",
                "Authentication unavailable",
                503,
            ))
        })?
        .authenticate_browser(raw_session, raw_csrf, request_now(&headers)?)
        .map_err(response)?;
    let account_id = require_active_account(&state, account_id)?;
    let sessions = state
        .auth
        .read()
        .map_err(|_| {
            response(problem(
                "auth_unavailable",
                "Authentication unavailable",
                503,
            ))
        })?
        .list_sessions(&account_id, Some(raw_session))
        .map_err(response)?;
    Ok(sensitive_json(WebSessionPage { sessions }))
}

async fn revoke_web_session(
    State(state): State<ApiState>,
    Path(session_id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, Response> {
    let idempotency_key = required_idempotency(&headers)?;
    let account_id = authenticated_account_for_mutation(&state, &headers)?;
    let target_device_id = state
        .auth
        .read()
        .map_err(|_| {
            response(problem(
                "auth_unavailable",
                "Authentication unavailable",
                503,
            ))
        })?
        .list_sessions(&account_id, None)
        .map_err(response)?
        .into_iter()
        .find(|session| session.id == session_id)
        .and_then(|session| session.device_id);
    let current_id = cookie(&headers, WEB_SESSION_COOKIE)
        .ok()
        .map(session_id_for_raw);
    if let Some(device_id) = target_device_id {
        let principal = authenticate(&state, &headers)?;
        let current_grants = grants(&state)?;
        let rotation_input = json!({
            "sessionId": session_id,
            "deviceId": device_id,
        });
        control::mutate_with_additional_puts::<serde_json::Value, _>(
            &state,
            &headers,
            &principal,
            &current_grants,
            account_id.as_str(),
            "auth.sessions.revoke",
            rotation_input,
            ResourceScope::default(),
            &device_id,
            || {
                let affected = control::encrypted_profiles_with_device_envelopes(
                    &state,
                    &account_id,
                    &device_id,
                )?;
                for (workspace_id, resource_id) in affected {
                    state
                        .mark_resource_key_rotation_pending(&workspace_id, &resource_id)
                        .map_err(control::response_problem)?;
                }
                Ok((json!({ "revocationPrepared": true }), Vec::new(), None))
            },
        )
        .await?;
    }
    state
        .auth
        .write()
        .map_err(|_| {
            response(problem(
                "auth_unavailable",
                "Authentication unavailable",
                503,
            ))
        })?
        .revoke_session_with_idempotency(
            &account_id,
            &session_id,
            idempotency_key,
            request_now(&headers)?,
        )
        .map_err(response)?;
    if current_id.as_deref() == Some(session_id.as_str()) {
        let mut result = StatusCode::NO_CONTENT.into_response();
        result
            .headers_mut()
            .append(header::SET_COOKIE, clear_cookie(WEB_SESSION_COOKIE)?);
        result
            .headers_mut()
            .append(header::SET_COOKIE, clear_cookie(WEB_CSRF_COOKIE)?);
        return Ok(result);
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn start_device_authorization(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(input): Json<DeviceAuthorizationStartInput>,
) -> Result<Response, Response> {
    let started = state
        .device_authorization
        .start(&input.device_id, &input.client_name, request_now(&headers)?)
        .map_err(response)?;
    Ok(sensitive_json(DeviceAuthorizationStartResponse {
        device_code: started.device_code,
        user_code: started.user_code,
        verification_uri: state.device_verification_uri.to_string(),
        expires_in: started.expires_in,
        interval: started.interval,
    }))
}

async fn approve_device_authorization(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(input): Json<DeviceAuthorizationApproveInput>,
) -> Result<Response, Response> {
    let (account_id, credential_version) =
        authenticated_account_and_credential_version_for_mutation(&state, &headers)?;
    state
        .device_authorization
        .approve_with_credential_version(
            &input.user_code,
            account_id,
            credential_version,
            request_now(&headers)?,
        )
        .map_err(response)?;
    Ok(sensitive_json(DeviceAuthorizationApproveResponse {
        approved: true,
    }))
}

async fn exchange_device_authorization(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(input): Json<DeviceAuthorizationTokenInput>,
) -> Result<Response, Response> {
    if !buglogin_domain::security::is_lower_hex_token_64(&input.device_code) {
        return Err(response(problem(
            "invalid_grant",
            "Device code is invalid",
            400,
        )));
    }
    let correlation_id = correlation(&headers)?;
    let current = checked_now(&correlation_id).map_err(response)?;
    match state
        .device_authorization
        .poll_for_exchange(&input.device_code, current)
        .map_err(response)?
    {
        crate::services::device_authorization::DevicePoll::Pending => Err(response(problem(
            "authorization_pending",
            "Authorization pending",
            400,
        ))),
        crate::services::device_authorization::DevicePoll::SlowDown { interval } => Err(response(
            problem("slow_down", "Polling too quickly", 400)
                .with_detail(format!("Retry after {interval} seconds"))
                .with_field("retryAfterSeconds", interval),
        )),
        crate::services::device_authorization::DevicePoll::Approved {
            account_id,
            device_id,
            credential_version,
        } => {
            let expires_at = crate::clock::checked_add_seconds(current, 3600, &correlation_id)
                .map_err(response)?;
            let refresh_expires_at =
                crate::clock::checked_add_seconds(current, 30 * 24 * 60 * 60, &correlation_id)
                    .map_err(response)?;
            let mut auth = state.auth.write().map_err(|_| {
                response(problem(
                    "auth_unavailable",
                    "Authentication unavailable",
                    503,
                ))
            })?;
            let (_, account_revision) = state
                .account_persistence
                .load_active_versioned(&account_id)
                .map_err(response)?
                .ok_or_else(|| {
                    response(problem(
                        "account_record_invalid",
                        "Account record is invalid",
                        500,
                    ))
                })?;
            let current_credential_version = state
                .credentials
                .password_version(&account_id)
                .map_err(response)?;
            if current_credential_version != credential_version {
                return Err(response(problem(
                    "invalid_grant",
                    "Device authorization is no longer valid",
                    400,
                )));
            }
            let authorization_consumption = state
                .device_authorization
                .prepare_approved_exchange_consumption(
                    &input.device_code,
                    &account_id,
                    &device_id,
                    credential_version,
                    current,
                )
                .map_err(response)?;
            // Authorization consumption and durable session issuance share one
            // control-store transaction, so failures cannot strand a consumed code.
            let mut tokens = match auth.issue_device_session_with_account_revision_and_consumption(
                account_id.clone(),
                expires_at,
                refresh_expires_at,
                device_id.clone(),
                credential_version,
                Some(account_revision),
                authorization_consumption,
            ) {
                Ok(tokens) => tokens,
                Err(error) if error.code == "storage_revision_conflict" => {
                    return Err(response(problem(
                        "device_authorization_restart_required",
                        "Account state changed during sign-in; restart device authorization",
                        409,
                    )));
                }
                Err(error) => return Err(response(error)),
            };
            Ok(sensitive_json(DeviceAuthTokenResponse {
                access_token: std::mem::take(&mut tokens.access_token),
                refresh_token: std::mem::take(&mut tokens.refresh_token),
                token_type: "Bearer",
                expires_at,
            }))
        }
    }
}

async fn refresh_device_session(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(input): Json<DeviceRefreshInput>,
) -> Result<Response, Response> {
    let idempotency_key = optional_idempotency(&headers)?;
    if !buglogin_domain::security::is_lower_hex_token_64(&input.refresh_token)
        || buglogin_domain::ids::DeviceId::new(input.device_id.clone()).is_err()
    {
        return Err(response(problem(
            "invalid_device_refresh",
            "Device refresh request is invalid",
            400,
        )));
    }
    let current = request_now(&headers)?;
    let correlation_id = correlation(&headers)?;
    let expires_at =
        crate::clock::checked_add_seconds(current, 3600, &correlation_id).map_err(response)?;
    let account_id = state
        .auth
        .read()
        .map_err(|_| {
            response(problem(
                "auth_unavailable",
                "Authentication unavailable",
                503,
            ))
        })?
        .device_refresh_account_id(&input.refresh_token)
        .map_err(response)?;
    let (_, account_revision) = state
        .account_persistence
        .load_active_versioned(&account_id)
        .map_err(response)?
        .ok_or_else(|| {
            response(problem(
                "account_record_invalid",
                "Account record is invalid",
                500,
            ))
        })?;
    let mut auth = state.auth.write().map_err(|_| {
        response(problem(
            "auth_unavailable",
            "Authentication unavailable",
            503,
        ))
    })?;
    // The account storage revision is checked in the same durable transaction
    // as credential rotation, fencing status changes made by another API node.
    let mut tokens = auth
        .refresh_device_session_with_idempotency_and_account_revision(
            &input.refresh_token,
            &input.device_id,
            idempotency_key,
            current,
            expires_at,
            Some(account_revision),
        )
        .map_err(response)?;
    Ok(sensitive_json(DeviceAuthTokenResponse {
        access_token: std::mem::take(&mut tokens.access_token),
        refresh_token: std::mem::take(&mut tokens.refresh_token),
        token_type: "Bearer",
        expires_at: tokens.expires_at,
    }))
}

async fn register_device_encryption_key(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(input): Json<DeviceEncryptionKeyRegistrationInput>,
) -> Result<Response, Response> {
    required_idempotency(&headers)?;
    let bearer = bearer_token(&headers)?;
    let (account_id, device_id) = state
        .auth
        .read()
        .map_err(|_| {
            response(problem(
                "auth_unavailable",
                "Authentication unavailable",
                503,
            ))
        })?
        .authenticate_device(bearer, request_now(&headers)?)
        .map_err(response)?;
    let registered_at = request_now(&headers)?;
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    if principal.account_id.as_ref() != Some(&account_id) {
        return Err(response(problem(
            "device_session_account_mismatch",
            "Device session does not match the authenticated account",
            403,
        )));
    }
    let public_key = input.public_key.clone();
    let value = json!({
        "deviceId": device_id.clone(),
        "publicKey": public_key.clone(),
    });
    control::mutate_with_additional_puts(
        &state,
        &headers,
        &principal,
        &grants,
        account_id.as_str(),
        "auth.device.encryptionkey.register",
        value,
        ResourceScope::default(),
        &device_id,
        || {
            let affected = control::encrypted_profiles_missing_device_envelopes(
                &state,
                &account_id,
                &device_id,
            )?;
            let (key, key_put) = state
                .device_key_directory
                .prepare_registration(&account_id, &device_id, &public_key, registered_at)
                .map_err(control::response_problem)?;
            for (workspace_id, resource_id) in affected {
                state
                    .mark_resource_key_rotation_pending(&workspace_id, &resource_id)
                    .map_err(control::response_problem)?;
            }
            Ok((key, key_put.into_iter().collect(), None))
        },
    )
    .await
}

pub fn rest_operation_ids() -> Vec<&'static str> {
    crate::commands::CommandRegistry::all()
        .iter()
        .filter(|command| command.adapters.contains(&crate::commands::Adapter::Rest))
        .map(|command| command.id.as_str())
        .collect()
}

async fn refresh_token(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Response, Response> {
    let token = bearer_token(&headers)?;
    let idempotency_key = optional_idempotency(&headers)?;
    let current = request_now(&headers)?;
    let correlation_id = correlation(&headers)?;
    let expires_at =
        crate::clock::checked_add_seconds(current, 3600, &correlation_id).map_err(response)?;
    let (replacement, expires_at) = state
        .refresh_token_with_idempotency(token, idempotency_key, current, expires_at)
        .map_err(response)?;
    Ok(sensitive_json(AuthTokenResponse {
        access_token: replacement,
        token_type: "Bearer",
        expires_at,
    }))
}

async fn logout(State(state): State<ApiState>, headers: HeaderMap) -> Result<Response, Response> {
    let idempotency_key = required_idempotency(&headers)?;
    if headers.get(header::AUTHORIZATION).is_some() {
        state
            .revoke_token_with_idempotency(bearer_token(&headers)?, idempotency_key)
            .map_err(response)?;
        return Ok(StatusCode::NO_CONTENT.into_response());
    }

    require_web_origin(&state, &headers)?;
    let raw_session = cookie(&headers, WEB_SESSION_COOKIE)?;
    let raw_csrf = headers
        .get("x-csrf-token")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| response(problem("csrf_required", "CSRF validation is required", 403)))?;
    state
        .revoke_browser_session_with_idempotency(raw_session, raw_csrf, idempotency_key)
        .map_err(response)?;
    let mut result = StatusCode::NO_CONTENT.into_response();
    result
        .headers_mut()
        .append(header::SET_COOKIE, clear_cookie(WEB_SESSION_COOKIE)?);
    result
        .headers_mut()
        .append(header::SET_COOKIE, clear_cookie(WEB_CSRF_COOKIE)?);
    Ok(result)
}

const MAX_PROXY_PARSE_BYTES: usize = 1024 * 1024;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProxyParseView {
    valid_count: usize,
    invalid_count: usize,
    duplicate_count: usize,
    rows: Vec<ProxyParseRowView>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProxyParseRowView {
    protocol: Option<String>,
    endpoint: Option<String>,
    valid: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    line: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'static str>,
}

async fn parse_proxy(
    State(state): State<ApiState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<ProxyParseView>, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let capability = Capability::new("CAP-NET-001")
        .map_err(|_| response(problem("invalid_capability", "Invalid capability", 500)))?;
    if !is_allowed(
        &principal,
        &capability,
        Action::Read,
        &ResourceScope::default(),
        &grants,
    ) {
        return Err(response(problem(
            "forbidden",
            "Proxy parsing is not allowed",
            403,
        )));
    }
    if body.len() > MAX_PROXY_PARSE_BYTES {
        return Err(response(problem(
            "proxy_input_too_large",
            "Proxy input exceeds the maximum size",
            413,
        )));
    }
    let text = std::str::from_utf8(&body).map_err(|_| {
        response(problem(
            "proxy_input_invalid_utf8",
            "Proxy input must be valid UTF-8",
            400,
        ))
    })?;
    Ok(Json(proxy_parse_view(parse_lines(text))))
}

fn proxy_parse_view(report: ParseReport) -> ProxyParseView {
    let valid_count = report.rows.len();
    let invalid_count = report.errors.len();
    let mut rows = report
        .rows
        .into_iter()
        .map(|row| ProxyParseRowView {
            protocol: Some(format!("{:?}", row.protocol).to_ascii_lowercase()),
            endpoint: Some(format!("{}:{}", row.host, row.port)),
            valid: true,
            line: None,
            error: None,
        })
        .collect::<Vec<_>>();
    rows.extend(report.errors.into_iter().map(|error| ProxyParseRowView {
        protocol: None,
        endpoint: None,
        valid: false,
        line: Some(error.line),
        error: Some(error.code),
    }));
    ProxyParseView {
        valid_count,
        invalid_count,
        duplicate_count: report.duplicates,
        rows,
    }
}

async fn list_profiles(
    State(state): State<ApiState>,
    RawQuery(raw_query): RawQuery,
    headers: HeaderMap,
) -> Result<Json<crate::services::profile_listing::ProfileListPage>, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let query = parse_profile_page_query(raw_query.as_deref()).map_err(response)?;
    let query = crate::services::profile_listing::ProfileListQuery::parse(
        query.workspace_id.as_deref(),
        query.cursor.as_deref(),
        query.limit,
        query.include_archived,
    )
    .map_err(response)?;
    let page = crate::services::profile_listing::list(&state, &principal, &grants, query)
        .map_err(response)?;
    Ok(Json(page))
}

async fn list_cookies(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    Query(query): Query<CookieListQuery>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, Response> {
    let principal = authenticate(&state, &headers)?;
    let correlation_id = correlation(&headers)?;
    let profile_id = ProfileId::from_str(&id)
        .map_err(|_| response(problem("invalid_profile_id", "Invalid profile ID", 400)))?;
    let input =
        serde_json::json!({"workspaceId": query.workspace_id, "profileId": profile_id.as_str()});
    let result = state
        .execute_local_cookie(
            "cookie.list",
            input,
            &principal,
            &grants(&state)?,
            None,
            &correlation_id,
        )
        .map_err(response)?;
    Ok(Json(result))
}

async fn enqueue_cookie_job(
    State(state): State<ApiState>,
    headers: HeaderMap,
    input: Result<Json<EnqueueCookieJobInput>, axum::extract::rejection::JsonRejection>,
) -> Result<Response, Response> {
    let principal = authenticate(&state, &headers)?;
    let Json(input) = input.map_err(|_| {
        response(problem(
            "invalid_input",
            "Cookie job request body is invalid",
            422,
        ))
    })?;
    let raw_idempotency_key = required_idempotency(&headers)?;
    let account_id = principal
        .account_id
        .as_ref()
        .map(|id| id.as_str().to_owned())
        .ok_or_else(|| {
            response(problem(
                "authentication_required",
                "Authentication required",
                401,
            ))
        })?;
    let capability = match input.command_id.as_str() {
        "cookie.import" => "cookie.import",
        "cookie.copy" => "cookie.copy",
        _ => {
            return Err(response(problem(
                "cookie_job_command_invalid",
                "Cookie job command is invalid",
                400,
            )));
        }
    };
    let workspace_id = WorkspaceId::from_str(&input.workspace_id).map_err(|_| {
        response(problem(
            "workspace_id_invalid",
            "Workspace ID is invalid",
            400,
        ))
    })?;
    let profile_id = ProfileId::from_str(&input.profile_id)
        .map_err(|_| response(problem("profile_id_invalid", "Profile ID is invalid", 400)))?;
    let destination_binding = state
        .profile_resource_record(&profile_id)
        .map_err(response)?
        .ok_or_else(|| {
            response(problem(
                "profile_binding_not_found",
                "Profile binding not found",
                404,
            ))
        })?;
    if destination_binding.workspace_id != workspace_id {
        return Err(response(problem(
            "profile_forbidden",
            "Profile access denied",
            403,
        )));
    }
    let destination_profile = state
        .get_profile_record(&profile_id)
        .map_err(response)?
        .ok_or_else(|| response(problem("profile_not_found", "Profile not found", 404)))?;
    if destination_profile.value.archived
        || destination_binding.archive_state == ProfileArchiveState::Archived
    {
        return Err(response(problem(
            "profile_archived",
            "Archived profiles cannot receive cookie mutations",
            409,
        )));
    }
    let current_grants = grants(&state)?;
    if !authorized(
        &state,
        &principal,
        &current_grants,
        &profile_id,
        Action::Create,
        capability,
    )? {
        return Err(response(problem(
            "profile_forbidden",
            "Profile access denied",
            403,
        )));
    }

    if input.command_id == "cookie.copy" {
        let source_workspace = input.source_workspace_id.as_deref().ok_or_else(|| {
            response(problem(
                "cookie_copy_source_required",
                "Cookie copy source is required",
                400,
            ))
        })?;
        let source_workspace_id = WorkspaceId::from_str(source_workspace).map_err(|_| {
            response(problem(
                "workspace_id_invalid",
                "Source workspace ID is invalid",
                400,
            ))
        })?;
        let source_profile = input.source_profile_id.as_deref().ok_or_else(|| {
            response(problem(
                "cookie_copy_source_required",
                "Cookie copy source is required",
                400,
            ))
        })?;
        let source_profile_id = ProfileId::from_str(source_profile).map_err(|_| {
            response(problem(
                "profile_id_invalid",
                "Source profile ID is invalid",
                400,
            ))
        })?;
        let source_binding = state
            .profile_resource_record(&source_profile_id)
            .map_err(response)?
            .ok_or_else(|| {
                response(problem(
                    "profile_binding_not_found",
                    "Source profile binding not found",
                    404,
                ))
            })?;
        if source_binding.workspace_id != source_workspace_id {
            return Err(response(problem(
                "profile_forbidden",
                "Source profile access denied",
                403,
            )));
        }
        authorize_profile(&state, &principal, &current_grants, &source_profile_id)?;
        if input.source_device_id.as_deref() != Some(input.device_id.as_str()) {
            return Err(response(problem(
                "cookie_copy_cross_device_dispatch_unavailable",
                "Cookie copy currently requires the source and destination profiles on the same connected device",
                409,
            )));
        }
    }

    let created_at = request_now(&headers)?;
    let expires_at = created_at
        .checked_add(CONNECTED_COOKIE_JOB_TTL_SECONDS)
        .ok_or_else(|| {
            response(problem(
                "cookie_job_expiry_invalid",
                "Cookie job expiry is invalid",
                500,
            ))
        })?;
    let scoped_idempotency_key = format!(
        "{account_id}:{}:{}:{raw_idempotency_key}",
        workspace_id.as_str(),
        input.command_id,
    );
    let input = EnqueueConnectedCookieJob {
        command_id: input.command_id,
        workspace_id: workspace_id.as_str().to_owned(),
        device_id: input.device_id,
        profile_id: profile_id.as_str().to_owned(),
        expected_generation: input.expected_generation,
        source_workspace_id: input.source_workspace_id,
        source_profile_id: input.source_profile_id,
        source_device_id: input.source_device_id,
        staged_import_ref: input.staged_import_ref,
        requested_by: account_id,
        idempotency_key: scoped_idempotency_key,
        now: created_at,
        expires_at,
    };
    let job = control::with_authorization_snapshot(
        &state,
        &headers,
        &principal,
        &current_grants,
        |current_principal, latest_grants, control_revision| {
            let binding = state
                .profile_resource_record(&profile_id)
                .map_err(response)?
                .ok_or_else(|| {
                    response(problem(
                        "profile_binding_not_found",
                        "Profile binding not found",
                        404,
                    ))
                })?;
            let profile = state
                .get_profile_record(&profile_id)
                .map_err(response)?
                .ok_or_else(|| response(problem("profile_not_found", "Profile not found", 404)))?;
            if binding.workspace_id != workspace_id
                || profile.value.archived
                || binding.archive_state == ProfileArchiveState::Archived
            {
                return Err(response(problem(
                    "profile_forbidden",
                    "Profile access denied",
                    403,
                )));
            }
            if !authorized(
                &state,
                current_principal,
                latest_grants,
                &profile_id,
                Action::Create,
                capability,
            )? {
                return Err(response(problem(
                    "profile_forbidden",
                    "Profile access denied",
                    403,
                )));
            }
            if input.command_id == "cookie.copy" {
                let source_profile_id =
                    ProfileId::from_str(input.source_profile_id.as_deref().unwrap_or_default())
                        .map_err(|_| {
                            response(problem(
                                "profile_id_invalid",
                                "Source profile ID is invalid",
                                400,
                            ))
                        })?;
                let source_workspace_id =
                    WorkspaceId::from_str(input.source_workspace_id.as_deref().unwrap_or_default())
                        .map_err(|_| {
                            response(problem(
                                "workspace_id_invalid",
                                "Source workspace ID is invalid",
                                400,
                            ))
                        })?;
                let source_binding = state
                    .profile_resource_record(&source_profile_id)
                    .map_err(response)?
                    .ok_or_else(|| {
                        response(problem(
                            "profile_binding_not_found",
                            "Source profile binding not found",
                            404,
                        ))
                    })?;
                if source_binding.workspace_id != source_workspace_id
                    || input.source_device_id.as_deref() != Some(input.device_id.as_str())
                {
                    return Err(response(problem(
                        "cookie_copy_cross_device_dispatch_unavailable",
                        "Cookie copy requires current source access on the destination device",
                        409,
                    )));
                }
                authorize_profile(&state, current_principal, latest_grants, &source_profile_id)?;
            }
            state
                .connected_cookie_jobs
                .enqueue_with_control_revision(input, Some(control_revision))
                .map_err(response)
        },
    )
    .await?;
    let body = serde_json::to_value(job)
        .map_err(|_| response(problem("serialization_failed", "Serialization failed", 500)))?;
    Ok((StatusCode::ACCEPTED, Json(body)).into_response())
}

async fn get_cookie_job(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    Query(query): Query<CookieJobWorkspaceQuery>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, Response> {
    let principal = authenticate(&state, &headers)?;
    let account_id = principal.account_id.as_ref().ok_or_else(|| {
        response(problem(
            "authentication_required",
            "Authentication required",
            401,
        ))
    })?;
    let job = state
        .connected_cookie_jobs
        .get(&query.workspace_id, &id, account_id.as_str())
        .map_err(response)?
        .ok_or_else(|| response(problem("cookie_job_not_found", "Cookie job not found", 404)))?;
    let profile_id = ProfileId::from_str(&job.profile_id)
        .map_err(|_| response(problem("profile_id_invalid", "Profile ID is invalid", 500)))?;
    let current_grants = grants(&state)?;
    authorize_action(
        &state,
        &principal,
        &current_grants,
        &profile_id,
        Action::Read,
        "profile.read",
    )?;
    if job.command_id == "cookie.copy" {
        let source_profile_id = ProfileId::from_str(
            job.source_profile_id.as_deref().unwrap_or_default(),
        )
        .map_err(|_| {
            response(problem(
                "profile_id_invalid",
                "Source profile ID is invalid",
                500,
            ))
        })?;
        authorize_profile(&state, &principal, &current_grants, &source_profile_id)?;
    }
    Ok(Json(serde_json::to_value(job).map_err(|_| {
        response(problem("serialization_failed", "Serialization failed", 500))
    })?))
}

async fn cookie_job_inbox(
    State(state): State<ApiState>,
    Path(workspace_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, Response> {
    let token = bearer_token(&headers)?;
    let (account_id, device_id) = state
        .auth
        .read()
        .map_err(|_| {
            response(problem(
                "auth_unavailable",
                "Authentication unavailable",
                503,
            ))
        })?
        .authenticate_device(token, request_now(&headers)?)
        .map_err(response)?;
    let principal = state
        .authenticate_token(token, request_now(&headers)?)
        .map_err(response)?;
    if principal.account_id.as_ref() != Some(&account_id) {
        return Err(response(problem(
            "cookie_job_executor_mismatch",
            "Authenticated device account does not match its session",
            403,
        )));
    }
    let current_grants = grants(&state)?;
    let jobs = state
        .connected_cookie_jobs
        .inbox(
            &workspace_id,
            &device_id,
            account_id.as_str(),
            request_now(&headers)?,
        )
        .map_err(response)?;
    let items = filter_cookie_inbox_jobs(
        &state,
        &principal,
        &current_grants,
        &device_id,
        &workspace_id,
        jobs,
    )?;
    Ok(Json(serde_json::json!({"items": items})))
}

async fn cookie_job_inbox_all(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, Response> {
    const MAX_ORGANIZATIONS: usize = 256;
    const MAX_WORKSPACES: usize = 4096;

    let token = bearer_token(&headers)?;
    let (account_id, device_id) = state
        .auth
        .read()
        .map_err(|_| {
            response(problem(
                "auth_unavailable",
                "Authentication unavailable",
                503,
            ))
        })?
        .authenticate_device(token, request_now(&headers)?)
        .map_err(response)?;
    let principal = state
        .authenticate_token(token, request_now(&headers)?)
        .map_err(response)?;
    if principal.account_id.as_ref() != Some(&account_id) {
        return Err(response(problem(
            "cookie_job_executor_mismatch",
            "Authenticated device account does not match its session",
            403,
        )));
    }
    let current_grants = grants(&state)?;
    let workspace_ids = {
        let workspaces = state.workspaces.read().map_err(|_| {
            response(problem(
                "workspace_unavailable",
                "Workspace service unavailable",
                503,
            ))
        })?;
        let organizations = workspaces
            .list_organizations(&principal, &current_grants)
            .map_err(response)?;
        if organizations.len() > MAX_ORGANIZATIONS {
            return Err(response(problem(
                "cookie_job_inbox_capacity_exceeded",
                "Device inbox exceeds the supported organization limit",
                503,
            )));
        }
        let mut ids = BTreeSet::new();
        for organization in organizations {
            for workspace in workspaces
                .list_workspaces(&principal, &current_grants, &organization.id)
                .map_err(response)?
            {
                ids.insert(workspace.id.as_str().to_owned());
                if ids.len() > MAX_WORKSPACES {
                    return Err(response(problem(
                        "cookie_job_inbox_capacity_exceeded",
                        "Device inbox exceeds the supported workspace limit",
                        503,
                    )));
                }
            }
        }
        ids
    };
    let tenant_ids = workspace_ids.into_iter().collect::<Vec<_>>();
    let jobs = state
        .connected_cookie_jobs
        .inbox_for_tenants(
            &tenant_ids,
            &device_id,
            account_id.as_str(),
            request_now(&headers)?,
        )
        .map_err(response)?;
    let mut items = Vec::with_capacity(jobs.len());
    for job in jobs {
        let workspace_id = job.workspace_id.clone();
        let mut authorized = filter_cookie_inbox_jobs(
            &state,
            &principal,
            &current_grants,
            &device_id,
            &workspace_id,
            vec![job],
        )?;
        items.append(&mut authorized);
    }
    items.sort_by(|left: &ConnectedCookieJob, right: &ConnectedCookieJob| {
        left.created_at
            .cmp(&right.created_at)
            .then_with(|| left.job_id.cmp(&right.job_id))
    });
    Ok(Json(serde_json::json!({"items": items})))
}

fn filter_cookie_inbox_jobs(
    state: &ApiState,
    principal: &Principal,
    current_grants: &[Grant],
    device_id: &str,
    workspace_id: &str,
    jobs: Vec<ConnectedCookieJob>,
) -> Result<Vec<ConnectedCookieJob>, Response> {
    let mut items = Vec::with_capacity(jobs.len());
    for job in jobs {
        if job.workspace_id != workspace_id || job.device_id != device_id {
            continue;
        }
        if job.state == ConnectedCookieJobState::Running {
            if !matches!(job.command_id.as_str(), "cookie.import" | "cookie.copy")
                || (job.command_id == "cookie.copy"
                    && job.source_device_id.as_deref() != Some(device_id))
            {
                continue;
            }
            let authenticated_account = principal
                .account_id
                .as_ref()
                .map(|account| account.as_str());
            if job.executor_account_id.as_deref() != authenticated_account {
                continue;
            }
            // An already-running mutation must remain visible to its original
            // executor for reconciliation after ACL changes. Any resumed
            // profile read/write still passes through the current API grants.
            items.push(job);
            continue;
        }
        let profile_id = ProfileId::from_str(&job.profile_id)
            .map_err(|_| response(problem("profile_id_invalid", "Profile ID is invalid", 500)))?;
        let capability = match job.command_id.as_str() {
            "cookie.import" => "cookie.import",
            "cookie.copy" => "cookie.copy",
            _ => continue,
        };
        if !authorized(
            state,
            principal,
            current_grants,
            &profile_id,
            Action::Create,
            capability,
        )? {
            continue;
        }

        if job.command_id == "cookie.copy" {
            if job.source_device_id.as_deref() != Some(device_id) {
                continue;
            }
            let source_profile_id = ProfileId::from_str(
                job.source_profile_id.as_deref().unwrap_or_default(),
            )
            .map_err(|_| {
                response(problem(
                    "profile_id_invalid",
                    "Source profile ID is invalid",
                    500,
                ))
            })?;
            if let Err(error) =
                authorize_profile(state, principal, current_grants, &source_profile_id)
            {
                if error.status() != StatusCode::FORBIDDEN {
                    return Err(error);
                }
                continue;
            }
        }
        items.push(job);
    }
    Ok(items)
}

fn authorize_cookie_job_executor(
    headers: &HeaderMap,
    state: &ApiState,
    token: &str,
    device_id: &str,
    job: &ConnectedCookieJob,
    principal: &Principal,
    current_grants: &[Grant],
) -> Result<(), Response> {
    let profile_id = ProfileId::from_str(&job.profile_id)
        .map_err(|_| response(problem("profile_id_invalid", "Profile ID is invalid", 400)))?;
    let token_principal = state
        .authenticate_token(token, request_now(headers)?)
        .map_err(response)?;
    if &token_principal != principal {
        return Err(response(problem(
            "authorization_state_changed",
            "Authorization changed; retry the request",
            409,
        )));
    }
    let capability = match job.command_id.as_str() {
        "cookie.import" => "cookie.import",
        "cookie.copy" => "cookie.copy",
        _ => {
            return Err(response(problem(
                "cookie_job_command_invalid",
                "Cookie job command is invalid",
                409,
            )));
        }
    };
    if !authorized(
        state,
        principal,
        current_grants,
        &profile_id,
        Action::Create,
        capability,
    )? {
        return Err(response(problem(
            "profile_forbidden",
            "Profile access denied",
            403,
        )));
    }
    if job.command_id == "cookie.copy" {
        if job.source_device_id.as_deref() != Some(device_id) {
            return Err(response(problem(
                "cookie_copy_cross_device_dispatch_unavailable",
                "Cookie copy requires the source and destination profiles on the same connected device",
                409,
            )));
        }
        let source_profile_id = ProfileId::from_str(
            job.source_profile_id.as_deref().unwrap_or_default(),
        )
        .map_err(|_| {
            response(problem(
                "profile_id_invalid",
                "Source profile ID is invalid",
                400,
            ))
        })?;
        authorize_profile(state, principal, current_grants, &source_profile_id)?;
    }
    Ok(())
}

async fn claim_cookie_job(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    Query(query): Query<CookieJobWorkspaceQuery>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, Response> {
    let token = bearer_token(&headers)?;
    let principal = authenticate(&state, &headers)?;
    let expected_grants = grants(&state)?;
    let (account_id, device_id) = state
        .auth
        .read()
        .map_err(|_| {
            response(problem(
                "auth_unavailable",
                "Authentication unavailable",
                503,
            ))
        })?
        .authenticate_device(token, request_now(&headers)?)
        .map_err(response)?;
    let job = control::with_authorization_snapshot(
        &state,
        &headers,
        &principal,
        &expected_grants,
        |current_principal, current_grants, control_revision| {
            let (current_account, current_device) = state
                .auth
                .read()
                .map_err(|_| {
                    response(problem(
                        "auth_unavailable",
                        "Authentication unavailable",
                        503,
                    ))
                })?
                .authenticate_device(token, request_now(&headers)?)
                .map_err(response)?;
            if current_account != account_id || current_device != device_id {
                return Err(response(problem(
                    "cookie_job_executor_mismatch",
                    "Authenticated device changed during authorization",
                    403,
                )));
            }
            let queued = state
                .connected_cookie_jobs
                .get_for_device(&query.workspace_id, &id, &device_id)
                .map_err(response)?
                .ok_or_else(|| {
                    response(problem("cookie_job_not_found", "Cookie job not found", 404))
                })?;
            authorize_cookie_job_executor(
                &headers,
                &state,
                token,
                &device_id,
                &queued,
                current_principal,
                current_grants,
            )?;
            state
                .connected_cookie_jobs
                .claim_with_control_revision(
                    &query.workspace_id,
                    &id,
                    account_id.as_str(),
                    &device_id,
                    request_now(&headers)?,
                    Some(control_revision),
                )
                .map_err(response)
        },
    )
    .await?;
    Ok(Json(serde_json::to_value(job).map_err(|_| {
        response(problem("serialization_failed", "Serialization failed", 500))
    })?))
}

async fn ack_cookie_job(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    Query(query): Query<CookieJobWorkspaceQuery>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, Response> {
    let token = bearer_token(&headers)?;
    let principal = authenticate(&state, &headers)?;
    let expected_grants = grants(&state)?;
    let (account_id, device_id) = state
        .auth
        .read()
        .map_err(|_| {
            response(problem(
                "auth_unavailable",
                "Authentication unavailable",
                503,
            ))
        })?
        .authenticate_device(token, request_now(&headers)?)
        .map_err(response)?;
    let already_running = state
        .connected_cookie_jobs
        .get_for_device(&query.workspace_id, &id, &device_id)
        .map_err(response)?
        .is_some_and(|job| job.state == ConnectedCookieJobState::Running);
    if already_running {
        let job = state
            .connected_cookie_jobs
            .acknowledge(
                &query.workspace_id,
                &id,
                account_id.as_str(),
                &device_id,
                request_now(&headers)?,
            )
            .map_err(response)?;
        return Ok(Json(serde_json::to_value(job).map_err(|_| {
            response(problem("serialization_failed", "Serialization failed", 500))
        })?));
    }
    let queued = state
        .connected_cookie_jobs
        .get_for_device(&query.workspace_id, &id, &device_id)
        .map_err(response)?
        .ok_or_else(|| response(problem("cookie_job_not_found", "Cookie job not found", 404)))?;
    authorize_cookie_job_executor(
        &headers,
        &state,
        token,
        &device_id,
        &queued,
        &principal,
        &expected_grants,
    )?;
    validate_cookie_job_lease(&state, &headers, &query.workspace_id, &queued).await?;
    let job = control::with_authorization_snapshot(
        &state,
        &headers,
        &principal,
        &expected_grants,
        |current_principal, current_grants, control_revision| {
            let (current_account, current_device) = state
                .auth
                .read()
                .map_err(|_| {
                    response(problem(
                        "auth_unavailable",
                        "Authentication unavailable",
                        503,
                    ))
                })?
                .authenticate_device(token, request_now(&headers)?)
                .map_err(response)?;
            if current_account != account_id || current_device != device_id {
                return Err(response(problem(
                    "cookie_job_executor_mismatch",
                    "Authenticated device changed during authorization",
                    403,
                )));
            }
            let queued = state
                .connected_cookie_jobs
                .get_for_device(&query.workspace_id, &id, &device_id)
                .map_err(response)?
                .ok_or_else(|| {
                    response(problem("cookie_job_not_found", "Cookie job not found", 404))
                })?;
            authorize_cookie_job_executor(
                &headers,
                &state,
                token,
                &device_id,
                &queued,
                current_principal,
                current_grants,
            )?;
            state
                .connected_cookie_jobs
                .acknowledge_with_control_revision(
                    &query.workspace_id,
                    &id,
                    account_id.as_str(),
                    &device_id,
                    request_now(&headers)?,
                    Some(control_revision),
                )
                .map_err(response)
        },
    )
    .await?;
    Ok(Json(serde_json::to_value(job).map_err(|_| {
        response(problem("serialization_failed", "Serialization failed", 500))
    })?))
}

async fn validate_cookie_job_lease(
    state: &ApiState,
    headers: &HeaderMap,
    workspace_id: &str,
    job: &ConnectedCookieJob,
) -> Result<(), Response> {
    let workspace_id = WorkspaceId::from_str(workspace_id).map_err(|_| {
        response(problem(
            "workspace_id_invalid",
            "Workspace ID is invalid",
            400,
        ))
    })?;
    let profile_id = ProfileId::from_str(&job.profile_id)
        .map_err(|_| response(problem("profile_id_invalid", "Profile ID is invalid", 400)))?;
    let binding = state
        .profile_resource_record(&profile_id)
        .map_err(response)?
        .filter(|binding| binding.workspace_id == workspace_id)
        .ok_or_else(|| {
            response(problem(
                "profile_binding_not_found",
                "Profile binding not found",
                404,
            ))
        })?;
    let profile = state
        .get_profile_record(&profile_id)
        .map_err(response)?
        .ok_or_else(|| response(problem("profile_not_found", "Profile not found", 404)))?;
    if profile.value.archived || binding.archive_state == ProfileArchiveState::Archived {
        return Err(response(problem(
            "profile_archived",
            "Archived profiles cannot receive cookie mutations",
            409,
        )));
    }
    let owner_id = control::required_header(headers, "x-buglogin-owner-id")?;
    let owner_token = control::required_header(headers, "x-buglogin-owner-token")?;
    let fencing_token = control::required_header(headers, "x-buglogin-fencing-token")?
        .parse::<u64>()
        .map_err(|_| control::bad_header("x-buglogin-fencing-token"))?;
    let auth =
        CoordinationAuth::new(owner_id, bearer_token(headers)?.to_owned()).map_err(response)?;
    state
        .coordination()
        .map_err(response)?
        .validate_write_authority(
            CoordinationScope {
                workspace_id: &workspace_id,
                auth: &auth,
            },
            &binding.resource_id,
            owner_token,
            fencing_token,
        )
        .await
        .map_err(response)
}

async fn complete_cookie_job(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    Query(query): Query<CookieJobWorkspaceQuery>,
    headers: HeaderMap,
    Json(receipt): Json<ConnectedCookieReceipt>,
) -> Result<Json<serde_json::Value>, Response> {
    let token = bearer_token(&headers)?;
    let (account_id, device_id) = state
        .auth
        .read()
        .map_err(|_| {
            response(problem(
                "auth_unavailable",
                "Authentication unavailable",
                503,
            ))
        })?
        .authenticate_device(token, request_now(&headers)?)
        .map_err(response)?;
    let principal = state
        .authenticate_token(token, request_now(&headers)?)
        .map_err(response)?;
    if principal.account_id.as_ref() != Some(&account_id) {
        return Err(response(problem(
            "cookie_job_executor_mismatch",
            "Authenticated device account does not match its session",
            403,
        )));
    }
    if receipt.job_id != id || receipt.device_id != device_id {
        return Err(response(problem(
            "cookie_receipt_binding_invalid",
            "Cookie receipt binding invalid",
            409,
        )));
    }
    let expected_grants = grants(&state)?;
    let job = control::with_authorization_snapshot(
        &state,
        &headers,
        &principal,
        &expected_grants,
        |current_principal, _, control_revision| {
            let (current_account, current_device) = state
                .auth
                .read()
                .map_err(|_| {
                    response(problem(
                        "auth_unavailable",
                        "Authentication unavailable",
                        503,
                    ))
                })?
                .authenticate_device(token, request_now(&headers)?)
                .map_err(response)?;
            if current_account != account_id
                || current_device != device_id
                || current_principal != &principal
            {
                return Err(response(problem(
                    "cookie_job_executor_mismatch",
                    "Authenticated device changed during completion",
                    403,
                )));
            }
            let job = state
                .connected_cookie_jobs
                .get_for_device(&query.workspace_id, &id, &device_id)
                .map_err(response)?
                .ok_or_else(|| {
                    response(problem("cookie_job_not_found", "Cookie job not found", 404))
                })?;
            if job.state == ConnectedCookieJobState::Completed {
                return state
                    .connected_cookie_jobs
                    .complete_with_control_revision(
                        &query.workspace_id,
                        &id,
                        account_id.as_str(),
                        &device_id,
                        request_now(&headers)?,
                        receipt,
                        Some(control_revision),
                    )
                    .map_err(response);
            }
            if receipt.profile_id != job.profile_id {
                return Err(response(problem(
                    "cookie_receipt_binding_invalid",
                    "Cookie receipt binding invalid",
                    409,
                )));
            }
            let workspace_id = WorkspaceId::from_str(&query.workspace_id).map_err(|_| {
                response(problem(
                    "workspace_id_invalid",
                    "Workspace ID is invalid",
                    400,
                ))
            })?;
            let profile_id = ProfileId::from_str(&job.profile_id).map_err(|_| {
                response(problem("profile_id_invalid", "Profile ID is invalid", 400))
            })?;
            let binding = state
                .profile_resource_record(&profile_id)
                .map_err(response)?
                .filter(|binding| binding.workspace_id == workspace_id)
                .ok_or_else(|| {
                    response(problem(
                        "profile_binding_not_found",
                        "Profile binding not found",
                        404,
                    ))
                })?;
            let manifest = state
                .current_sync_manifest(&workspace_id, &binding.resource_id)
                .map_err(response)?;
            let committed_generation =
                cookie_receipt_generation_is_committed(manifest.as_ref(), receipt.new_generation);
            if !committed_generation {
                return Err(response(
                    problem(
                        "cookie_profile_sync_not_committed",
                        "Cookie job cannot complete before its encrypted profile data is committed",
                        409,
                    )
                    .retryable(true),
                ));
            }
            state
                .connected_cookie_jobs
                .complete_with_control_revision(
                    &query.workspace_id,
                    &id,
                    account_id.as_str(),
                    &device_id,
                    request_now(&headers)?,
                    receipt,
                    Some(control_revision),
                )
                .map_err(response)
        },
    )
    .await?;
    Ok(Json(serde_json::to_value(job).map_err(|_| {
        response(problem("serialization_failed", "Serialization failed", 500))
    })?))
}

fn cookie_receipt_generation_is_committed(
    manifest: Option<&SyncManifest>,
    generation: u64,
) -> bool {
    manifest.is_some_and(|manifest| {
        manifest.generation >= generation
            && manifest.payload_encryption.is_some()
            && (manifest
                .file_chunks
                .iter()
                .any(|file| !file.path.starts_with(".buglogin/"))
                || manifest
                    .objects
                    .iter()
                    .any(|object| !object.key.starts_with(".buglogin/")))
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FailCookieJobInput {
    failure_code: ConnectedCookieFailureCode,
}

async fn fail_cookie_job(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    Query(query): Query<CookieJobWorkspaceQuery>,
    headers: HeaderMap,
    Json(input): Json<FailCookieJobInput>,
) -> Result<Json<serde_json::Value>, Response> {
    let token = bearer_token(&headers)?;
    state.refresh_control_aggregate().map_err(response)?;
    let (account_id, device_id) = state
        .auth
        .read()
        .map_err(|_| {
            response(problem(
                "auth_unavailable",
                "Authentication unavailable",
                503,
            ))
        })?
        .authenticate_device(token, request_now(&headers)?)
        .map_err(response)?;
    let principal = state
        .authenticate_token(token, request_now(&headers)?)
        .map_err(response)?;
    if principal.account_id.as_ref() != Some(&account_id) {
        return Err(response(problem(
            "cookie_job_executor_mismatch",
            "Authenticated device account does not match its session",
            403,
        )));
    }
    let _job = state
        .connected_cookie_jobs
        .get_for_device(&query.workspace_id, &id, &device_id)
        .map_err(response)?
        .ok_or_else(|| response(problem("cookie_job_not_found", "Cookie job not found", 404)))?;
    let control_revision = state.control_revision().map_err(response)?;
    let job = state
        .connected_cookie_jobs
        .fail_with_control_revision(
            &query.workspace_id,
            &id,
            account_id.as_str(),
            &device_id,
            request_now(&headers)?,
            input.failure_code,
            Some(control_revision),
        )
        .map_err(response)?;
    Ok(Json(serde_json::to_value(job).map_err(|_| {
        response(problem("serialization_failed", "Serialization failed", 500))
    })?))
}

async fn cancel_cookie_job(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    Query(query): Query<CookieJobWorkspaceQuery>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, Response> {
    let principal = authenticate(&state, &headers)?;
    let account_id = principal.account_id.clone().ok_or_else(|| {
        response(problem(
            "authentication_required",
            "Authentication required",
            401,
        ))
    })?;
    let job = state
        .connected_cookie_jobs
        .get(&query.workspace_id, &id, account_id.as_str())
        .map_err(response)?
        .ok_or_else(|| response(problem("cookie_job_not_found", "Cookie job not found", 404)))?;
    let profile_id = ProfileId::from_str(&job.profile_id)
        .map_err(|_| response(problem("profile_id_invalid", "Profile ID is invalid", 500)))?;
    let capability = match job.command_id.as_str() {
        "cookie.import" => "cookie.import",
        "cookie.copy" => "cookie.copy",
        _ => {
            return Err(response(problem(
                "cookie_job_command_invalid",
                "Cookie job command is invalid",
                500,
            )));
        }
    };
    let current_grants = grants(&state)?;
    authorize_action(
        &state,
        &principal,
        &current_grants,
        &profile_id,
        Action::Create,
        capability,
    )?;
    let job = control::with_authorization_snapshot(
        &state,
        &headers,
        &principal,
        &current_grants,
        |current_principal, latest_grants, control_revision| {
            let current_job = state
                .connected_cookie_jobs
                .get(&query.workspace_id, &id, account_id.as_str())
                .map_err(response)?
                .ok_or_else(|| {
                    response(problem("cookie_job_not_found", "Cookie job not found", 404))
                })?;
            let current_profile_id =
                ProfileId::from_str(&current_job.profile_id).map_err(|_| {
                    response(problem("profile_id_invalid", "Profile ID is invalid", 500))
                })?;
            authorize_action(
                &state,
                current_principal,
                latest_grants,
                &current_profile_id,
                Action::Create,
                match current_job.command_id.as_str() {
                    "cookie.import" => "cookie.import",
                    "cookie.copy" => "cookie.copy",
                    _ => {
                        return Err(response(problem(
                            "cookie_job_command_invalid",
                            "Cookie job command is invalid",
                            500,
                        )));
                    }
                },
            )?;
            state
                .connected_cookie_jobs
                .cancel_with_control_revision(
                    &query.workspace_id,
                    &id,
                    account_id.as_str(),
                    request_now(&headers)?,
                    Some(control_revision),
                )
                .map_err(response)
        },
    )
    .await?;
    Ok(Json(serde_json::to_value(job).map_err(|_| {
        response(problem("serialization_failed", "Serialization failed", 500))
    })?))
}

async fn get_profile(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<buglogin_storage::local::Versioned<Profile>>, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let id = ProfileId::from_str(&id)
        .map_err(|_| response(problem("invalid_profile_id", "Invalid profile ID", 400)))?;
    authorize_profile(&state, &principal, &grants, &id)?;
    let profile = state
        .get_profile_record(&id)
        .map_err(response)?
        .ok_or_else(|| response(problem("profile_not_found", "Profile not found", 404)))?;
    Ok(Json(profile))
}

async fn get_profile_binding(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<ProfileResource>, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let id = ProfileId::from_str(&id)
        .map_err(|_| response(problem("invalid_profile_id", "Invalid profile ID", 400)))?;
    authorize_profile(&state, &principal, &grants, &id)?;
    let binding = state
        .profile_resources
        .read()
        .map_err(|_| {
            response(problem(
                "control_unavailable",
                "Control state unavailable",
                503,
            ))
        })?
        .get(&id)
        .cloned()
        .ok_or_else(|| {
            response(problem(
                "binding_not_found",
                "Profile binding not found",
                404,
            ))
        })?;
    Ok(Json(binding))
}

async fn create_profile(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(request): Json<ProfileCreateRequest>,
) -> Result<Response, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let idempotency_key = required_idempotency(&headers)?;
    let header_workspace_id = headers
        .get("x-buglogin-workspace-id")
        .and_then(|value| value.to_str().ok());
    let (create_input, legacy_profile) = match request {
        ProfileCreateRequest::Command(input) => (input, None),
        ProfileCreateRequest::Legacy(profile) => {
            let input = ProfileCreationService::legacy_input(&profile, header_workspace_id);
            (input, Some(profile))
        }
    };
    if let Some(legacy) = legacy_profile.as_ref() {
        ProfileCreationService::validate_legacy_profile_data(
            &legacy.runtime,
            legacy.runtime_config.as_ref(),
            legacy.fingerprint.as_ref(),
        )
        .map_err(response)?;
    }
    let managed = !state.local_authority_enabled();
    let workspace_id = ProfileCreationService::resolve_workspace(
        create_input.workspace_id.as_deref(),
        header_workspace_id,
        &principal,
        managed,
    )
    .map_err(response)?;
    let requested_id = create_input.id.as_deref();
    let profile_id = ProfileCreationService::profile_id(
        requested_id,
        workspace_id.as_ref().map(WorkspaceId::as_str),
        idempotency_key,
    )
    .map_err(response)?;
    let mut profile =
        ProfileCreationService::build(create_input.clone(), profile_id.clone(), managed)
            .map_err(response)?;
    if let Some(legacy) = legacy_profile {
        profile.runtime_config = legacy.runtime_config;
        profile.fingerprint = legacy.fingerprint;
        profile.vpn_id = legacy.vpn_id;
        profile.group_ids = legacy.group_ids;
        profile.tags = legacy.tags;
        profile.extension_group_id = legacy.extension_group_id;
    }
    if managed {
        let capability = Capability::new("profile.create")
            .map_err(|_| response(problem("invalid_capability", "Invalid capability", 500)))?;
        let scope = ResourceScope {
            workspace_id: workspace_id.clone(),
            profile_id: Some(profile_id.clone()),
            ..ResourceScope::default()
        };
        if !is_allowed(&principal, &capability, Action::Create, &scope, &grants) {
            return Err(response(problem(
                "profile_forbidden",
                "Profile creation denied",
                403,
            )));
        }
    } else {
        authorize_action(
            &state,
            &principal,
            &grants,
            &profile_id,
            Action::Create,
            "profile.create",
        )?;
    }
    if managed {
        return control::provision_managed_profile(
            state,
            headers,
            principal,
            grants,
            profile,
            workspace_id,
        )
        .await;
    }
    let input_value = serde_json::to_value(&create_input)
        .map_err(|_| response(problem("invalid_input", "Input is invalid", 400)))?;
    let resource_id = profile_id.as_str().to_owned();
    control::mutate(
        &state,
        &headers,
        &principal,
        &grants,
        "local",
        "profile.create",
        input_value,
        ResourceScope {
            account_id: principal.account_id.clone(),
            profile_id: Some(profile.id.clone()),
            ..ResourceScope::default()
        },
        &resource_id,
        || match state.profiles.get(&profile.id) {
            Ok(Some(existing))
                if ProfileCreationService::same_created_profile(&existing.value, &profile) =>
            {
                Ok(existing)
            }
            Ok(Some(_)) => Err(control::response_problem(problem(
                "storage_revision_conflict",
                "Storage revision conflict",
                409,
            ))),
            Ok(None) => state
                .create_profile_record(profile)
                .map_err(control::response_problem),
            Err(error) => Err(control::response_problem(error)),
        },
    )
    .await
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ProfileCreateRequest {
    Command(ProfileCreateInput),
    Legacy(Profile),
}

async fn archive_profile(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(input): Json<RevisionInput>,
) -> Result<Response, Response> {
    mutate_profile(state, id, headers, input, true).await
}

async fn restore_profile(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(input): Json<RevisionInput>,
) -> Result<Response, Response> {
    mutate_profile(state, id, headers, input, false).await
}

async fn clone_profile(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(input): Json<ProfileCloneInput>,
) -> Result<Response, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let source_id = ProfileId::from_str(&id)
        .map_err(|_| response(problem("invalid_profile_id", "Invalid profile ID", 400)))?;
    if source_id != input.source_profile_id {
        return Err(response(problem(
            "profile_id_mismatch",
            "Path profile ID does not match source profile ID",
            400,
        )));
    }
    control::clone_profile(state, headers, principal, grants, input).await
}

async fn share_profile(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(input): Json<ProfileShareInput>,
) -> Result<Response, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let profile_id = ProfileId::from_str(&id)
        .map_err(|_| response(problem("invalid_profile_id", "Invalid profile ID", 400)))?;
    if profile_id != input.profile_id {
        return Err(response(problem(
            "profile_id_mismatch",
            "Path profile ID does not match profile ID",
            400,
        )));
    }
    control::share_profile(state, headers, principal, grants, input).await
}

async fn profile_share_recipient_device_keys(
    State(state): State<ApiState>,
    Path((profile_id, member_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<Vec<DeviceEncryptionKey>>, Response> {
    let principal = authenticate(&state, &headers)?;
    let profile_id = ProfileId::from_str(&profile_id)
        .map_err(|_| response(problem("invalid_profile_id", "Invalid profile ID", 400)))?;
    let member_id = MemberId::from_str(&member_id)
        .map_err(|_| response(problem("invalid_member_id", "Invalid member ID", 400)))?;
    let binding = state
        .profile_resource_record(&profile_id)
        .map_err(response)?
        .ok_or_else(|| {
            response(problem(
                "binding_not_found",
                "Profile binding not found",
                404,
            ))
        })?;
    let capability = Capability::new("CAP-SHARE-001")
        .map_err(|_| response(problem("invalid_capability", "Invalid capability", 500)))?;
    let scope = ResourceScope {
        workspace_id: Some(binding.workspace_id.clone()),
        resource_id: Some(binding.resource_id.clone()),
        profile_id: Some(profile_id),
        ..ResourceScope::default()
    };
    if !is_allowed(
        &principal,
        &capability,
        Action::Manage,
        &scope,
        &grants(&state)?,
    ) {
        return Err(response(problem(
            "profile_forbidden",
            "Profile sharing denied",
            403,
        )));
    }
    let recipient = state
        .members
        .read()
        .map_err(|_| {
            response(problem(
                "member_unavailable",
                "Member service unavailable",
                503,
            ))
        })?
        .active_members()
        .into_iter()
        .find(|member| member.id == member_id && member.workspace_id == binding.workspace_id)
        .ok_or_else(|| {
            response(problem(
                "member_not_found",
                "Active workspace member not found",
                404,
            ))
        })?;
    let active_devices = state
        .auth
        .read()
        .map_err(|_| {
            response(problem(
                "auth_unavailable",
                "Authentication unavailable",
                503,
            ))
        })?
        .active_device_ids(&recipient.account_id, request_now(&headers)?)
        .map_err(response)?;
    let keys = state
        .device_key_directory
        .list_for_account(&recipient.account_id, &active_devices)
        .map_err(response)?;
    Ok(Json(keys))
}

async fn get_profile_key_share_envelope(
    State(state): State<ApiState>,
    Path(profile_path): Path<String>,
    headers: HeaderMap,
) -> Result<Json<ProfileKeyShareEnvelope>, Response> {
    let principal = authenticate(&state, &headers)?;
    let account_id = principal.account_id.as_ref().ok_or_else(|| {
        response(problem(
            "account_missing",
            "Authenticated account is missing",
            401,
        ))
    })?;
    let profile_id = ProfileId::from_str(&profile_path)
        .map_err(|_| response(problem("invalid_profile_id", "Invalid profile ID", 400)))?;
    let binding = state
        .profile_resource_record(&profile_id)
        .map_err(response)?
        .ok_or_else(|| {
            response(problem(
                "binding_not_found",
                "Profile binding not found",
                404,
            ))
        })?;
    let raw_token = bearer_token(&headers)?;
    let (device_account_id, device_id) = state
        .auth
        .read()
        .map_err(|_| {
            response(problem(
                "auth_unavailable",
                "Authentication unavailable",
                503,
            ))
        })?
        .authenticate_device(raw_token, request_now(&headers)?)
        .map_err(response)?;
    if &device_account_id != account_id {
        return Err(response(problem(
            "device_session_account_mismatch",
            "Device session does not match the authenticated account",
            403,
        )));
    }
    if state
        .resource_key_rotation_pending(&binding.workspace_id, &binding.resource_id)
        .map_err(response)?
    {
        return Err(response(problem(
            "sync_key_rotation_pending",
            "Profile key envelopes are unavailable until key rotation completes",
            409,
        )));
    }
    let active_members = state
        .members
        .read()
        .map_err(|_| {
            response(problem(
                "member_unavailable",
                "Member service unavailable",
                503,
            ))
        })?
        .active_for_account(account_id)
        .into_iter()
        .filter(|member| member.workspace_id == binding.workspace_id)
        .collect::<Vec<_>>();
    if active_members.is_empty() {
        return Err(response(problem(
            "member_not_found",
            "Active workspace member not found",
            404,
        )));
    }
    let current_grants = grants(&state)?;
    let allowed = HostedProfileService::authorized(
        &state,
        &principal,
        &current_grants,
        &profile_id,
        Action::Read,
        "profile.read",
    )
    .map_err(response)?;
    let key_recipients = HostedProfileService::effective_profile_key_recipients(
        &state,
        &profile_id,
        &binding,
        &[Action::Read],
    )
    .map_err(response)?;
    let active_member = active_members
        .into_iter()
        .find(|member| key_recipients.contains(&member.id))
        .ok_or_else(|| {
            response(problem(
                "profile_forbidden",
                "A current profile read share is required to retrieve this envelope",
                403,
            ))
        })?;
    if !allowed {
        return Err(response(problem(
            "profile_forbidden",
            "A current profile read share is required to retrieve this envelope",
            403,
        )));
    }
    let manifest = state
        .current_sync_manifest(&binding.workspace_id, &binding.resource_id)
        .map_err(response)?
        .ok_or_else(|| {
            response(problem(
                "profile_manifest_missing",
                "Profile sync manifest not found",
                404,
            ))
        })?;
    let active_key_version = manifest
        .payload_encryption
        .as_ref()
        .and_then(|encryption| encryption.key_version.as_deref())
        .ok_or_else(|| {
            response(problem(
                "profile_key_version_missing",
                "Profile sync manifest has no active resource key version",
                409,
            ))
        })?;
    let envelope = state
        .profile_key_share_envelope(&binding.resource_id, &active_member.id, &device_id)
        .map_err(response)?
        .filter(|envelope| envelope.key_version == active_key_version)
        .ok_or_else(|| {
            response(problem(
                "profile_key_envelope_not_found",
                "No active key envelope is available for this device",
                404,
            ))
        })?;
    Ok(Json(envelope))
}

async fn revoke_profile_share(
    State(state): State<ApiState>,
    Path((id, member_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(input): Json<ProfileShareRevokeInput>,
) -> Result<Response, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let profile_id = ProfileId::from_str(&id)
        .map_err(|_| response(problem("invalid_profile_id", "Invalid profile ID", 400)))?;
    let member_id = MemberId::from_str(&member_id)
        .map_err(|_| response(problem("invalid_member_id", "Invalid member ID", 400)))?;
    if profile_id != input.profile_id || member_id != input.member_id {
        return Err(response(problem(
            "share_id_mismatch",
            "Path identifiers do not match request input",
            400,
        )));
    }
    control::revoke_profile_share(state, headers, principal, grants, input).await
}

async fn mutate_profile(
    state: ApiState,
    id: String,
    headers: HeaderMap,
    input: RevisionInput,
    archive: bool,
) -> Result<Response, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let id = ProfileId::from_str(&id)
        .map_err(|_| response(problem("invalid_profile_id", "Invalid profile ID", 400)))?;
    authorize_action(
        &state,
        &principal,
        &grants,
        &id,
        Action::Update,
        if archive {
            "profile.archive"
        } else {
            "profile.restore"
        },
    )?;
    if !state.local_authority_enabled() {
        return control::mutate_hosted_profile_archive_state(
            state,
            headers,
            principal,
            id,
            input.revision,
            archive,
        )
        .await;
    }
    let current = state
        .get_profile_record(&id)
        .map_err(response)?
        .ok_or_else(|| response(problem("profile_not_found", "Profile not found", 404)))?;
    let expected = input.revision;
    let updated = if archive {
        state.archive_profile_record(expected, current.value)
    } else {
        state.restore_profile_record(expected, current.value)
    };
    updated
        .map(|value| Json(value).into_response())
        .map_err(response)
}

async fn launch_profile(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(input): Json<LaunchInput>,
) -> Result<(StatusCode, Json<Operation>), Response> {
    if !state.local_authority_enabled() {
        return Err(response(problem(
            "production_runtime_dispatch_required",
            "Production profile runtime must be dispatched through the desktop API client",
            503,
        )));
    }
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let id = ProfileId::from_str(&id)
        .map_err(|_| response(problem("invalid_profile_id", "Invalid profile ID", 400)))?;
    authorize_action(
        &state,
        &principal,
        &grants,
        &id,
        Action::Execute,
        "profile.launch",
    )?;
    let actor_id = principal
        .account_id
        .as_ref()
        .map_or("local", AccountId::as_str);
    let operation_id = operation_id(&headers, "profile.launch", actor_id, id.as_str())?;
    if let Some(operation) = existing_operation(&state, &operation_id)? {
        return Ok((StatusCode::ACCEPTED, Json(operation)));
    }
    begin_operation(&state, &operation_id, "profile.launch", actor_id)?;
    let result = ProfileLifecycleService.launch(
        &state.profiles,
        &state.lifecycles,
        &state.runtime_binaries,
        ProfileLifecycleCommand {
            profile_id: id,
            profiles_root: input.profiles_root,
            extra_args: input.extra_args,
        },
    );
    match result {
        Ok(value) => finish_operation(&state, &operation_id, actor_id, value),
        Err(error) => fail_operation(&state, &operation_id, actor_id, error),
    }
}

async fn stop_profile(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<(StatusCode, Json<Operation>), Response> {
    if !state.local_authority_enabled() {
        return Err(response(problem(
            "production_runtime_dispatch_required",
            "Production profile runtime must be dispatched through the desktop API client",
            503,
        )));
    }
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let id = ProfileId::from_str(&id)
        .map_err(|_| response(problem("invalid_profile_id", "Invalid profile ID", 400)))?;
    authorize_action(
        &state,
        &principal,
        &grants,
        &id,
        Action::Execute,
        "profile.stop",
    )?;
    let actor_id = principal
        .account_id
        .as_ref()
        .map_or("local", AccountId::as_str);
    let operation_id = operation_id(&headers, "profile.stop", actor_id, id.as_str())?;
    if let Some(operation) = existing_operation(&state, &operation_id)? {
        return Ok((StatusCode::ACCEPTED, Json(operation)));
    }
    begin_operation(&state, &operation_id, "profile.stop", actor_id)?;
    let result = ProfileLifecycleService.stop(&state.profiles, &state.lifecycles, id);
    match result {
        Ok(value) => finish_operation(&state, &operation_id, actor_id, value),
        Err(error) => fail_operation(&state, &operation_id, actor_id, error),
    }
}

async fn delete_profile(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(input): Json<DeleteInput>,
) -> Result<Response, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let id = ProfileId::from_str(&id)
        .map_err(|_| response(problem("invalid_profile_id", "Invalid profile ID", 400)))?;
    authorize_action(
        &state,
        &principal,
        &grants,
        &id,
        Action::Delete,
        "profile.delete",
    )?;
    let resource_id = id.as_str().to_owned();
    let input_value = serde_json::to_value(&input)
        .map_err(|_| response(problem("invalid_input", "Input is invalid", 400)))?;
    if !state.local_authority_enabled() {
        let binding = state
            .profile_resource_record(&id)
            .map_err(response)?
            .ok_or_else(|| {
                response(problem(
                    "binding_not_found",
                    "Profile binding not found",
                    404,
                ))
            })?;
        return control::mutate(
            &state,
            &headers,
            &principal,
            &grants,
            binding.workspace_id.as_str(),
            "profile.delete",
            input_value,
            ResourceScope {
                workspace_id: Some(binding.workspace_id.clone()),
                resource_id: Some(binding.resource_id.clone()),
                profile_id: Some(id.clone()),
                ..ResourceScope::default()
            },
            &resource_id,
            || {
                HostedProfileService::delete_to_quarantine(
                    &state,
                    &id,
                    input.revision,
                    &input.confirmation,
                    principal.account_id.clone(),
                )
                .map_err(control::response_problem)
            },
        )
        .await;
    }
    control::mutate(
        &state,
        &headers,
        &principal,
        &grants,
        "local",
        "profile.delete",
        input_value,
        ResourceScope {
            account_id: principal.account_id.clone(),
            organization_id: None,
            workspace_id: None,
            team_id: None,
            member_id: None,
            profile_id: Some(id.clone()),
            resource_id: None,
        },
        &resource_id,
        || {
            crate::services::profile_deletion::ProfileDeletionService::delete_local(
                &state,
                &id,
                input.revision,
                &input.confirmation,
            )
            .map_err(control::response_problem)
        },
    )
    .await
}

async fn get_operation(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<crate::services::operations::Operation>, Response> {
    state.refresh_control_aggregate().map_err(response)?;
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let operation = state
        .operations
        .read()
        .map_err(|_| {
            response(problem(
                "operation_unavailable",
                "Operation service unavailable",
                503,
            ))
        })?
        .get(&principal, &grants, &id)
        .map_err(response)?
        .clone();
    Ok(Json(operation))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EventQuery {
    workspace_id: WorkspaceId,
    #[serde(default)]
    after: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

async fn read_events(
    State(state): State<ApiState>,
    Query(query): Query<EventQuery>,
    headers: HeaderMap,
) -> Result<Sse<impl Stream<Item = Result<SseEvent, std::convert::Infallible>>>, Response> {
    let principal = authenticate(&state, &headers)?;
    let account_id = principal.account_id.ok_or_else(|| {
        response(problem(
            "authentication_required",
            "Authentication required",
            401,
        ))
    })?;
    let auth =
        CoordinationAuth::new(account_id.as_str(), bearer_token(&headers)?).map_err(response)?;
    let cursor = headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let cursor = cursor.or(query.after);
    let limit = query.limit.unwrap_or(100);
    let initial_page =
        read_workspace_events(&state, &auth, &query.workspace_id, cursor.as_deref(), limit)
            .await
            .map_err(response)?;
    let initial_items = initial_page["items"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let mut next_cursor = initial_page["nextCursor"]
        .as_str()
        .map(str::to_owned)
        .or(cursor);
    let stream = stream::unfold(
        Some((
            state,
            auth,
            query.workspace_id,
            next_cursor.take(),
            initial_items.into_iter(),
        )),
        move |state| async move {
            let (state, auth, workspace_id, mut cursor, mut pending) = state?;
            loop {
                if let Some(item) = pending.next() {
                    let stream_id = item["streamId"].as_str().unwrap_or_default().to_owned();
                    let event_name = item["event"]["name"].as_str().unwrap_or("message");
                    let payload =
                        serde_json::to_string(&item["event"]).unwrap_or_else(|_| "{}".to_owned());
                    let event = SseEvent::default()
                        .id(stream_id)
                        .event(event_name)
                        .data(payload);
                    return Some((
                        Ok(event),
                        Some((state, auth, workspace_id, cursor, pending)),
                    ));
                }

                match read_workspace_events(&state, &auth, &workspace_id, cursor.as_deref(), limit)
                    .await
                {
                    Ok(page) => {
                        cursor = page["nextCursor"].as_str().map(str::to_owned).or(cursor);
                        pending = page["items"]
                            .as_array()
                            .cloned()
                            .unwrap_or_default()
                            .into_iter();
                        if pending.len() == 0 {
                            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                        }
                    }
                    Err(error) => {
                        let payload =
                            serde_json::to_string(&error).unwrap_or_else(|_| "{}".to_owned());
                        let event = SseEvent::default().event("error").data(payload);
                        return Some((Ok(event), None));
                    }
                }
            }
        },
    );
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MigrationInspectInput {
    source_root: PathBuf,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MigrationImportInput {
    source_root: PathBuf,
    staging_parent: PathBuf,
    destination_root: PathBuf,
    run_id: String,
    confirmation: String,
}

#[derive(Serialize)]
struct MigrationInspectResult {
    report: buglogin_migration::inspect::InspectReport,
    snapshot: buglogin_migration::snapshot::SourceSnapshot,
}

async fn inspect_migration(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(input): Json<MigrationInspectInput>,
) -> Result<(StatusCode, Json<Operation>), Response> {
    if !state.local_authority_enabled() {
        return Err(response(problem(
            "production_local_migration_forbidden",
            "Production cannot run local filesystem migration",
            409,
        )));
    }
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    authorize_capability(&principal, &grants, "migration.read", Action::Read)?;
    let actor_id = principal
        .account_id
        .as_ref()
        .map_or("local", AccountId::as_str);
    let source_scope = input.source_root.to_string_lossy();
    let operation_id = operation_id(
        &headers,
        "migration.inspect",
        actor_id,
        source_scope.as_ref(),
    )?;
    if let Some(operation) = existing_operation(&state, &operation_id)? {
        return Ok((StatusCode::ACCEPTED, Json(operation)));
    }
    begin_operation(&state, &operation_id, "migration.inspect", actor_id)?;
    let result = inspect_source(&input.source_root).and_then(|(report, snapshot)| {
        serde_json::to_value(MigrationInspectResult { report, snapshot })
            .map_err(|_| problem("migration_result_invalid", "Migration result invalid", 500))
    });
    match result {
        Ok(value) => finish_operation(&state, &operation_id, actor_id, value),
        Err(error) => fail_operation(&state, &operation_id, actor_id, error),
    }
}

async fn import_migration(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(input): Json<MigrationImportInput>,
) -> Result<(StatusCode, Json<Operation>), Response> {
    if !state.local_authority_enabled() {
        return Err(response(problem(
            "production_local_migration_forbidden",
            "Production cannot run local filesystem migration",
            409,
        )));
    }
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    authorize_capability(&principal, &grants, "migration.write", Action::Execute)?;
    let destination = input.destination_root.canonicalize().map_err(|_| {
        response(problem(
            "migration_destination_invalid",
            "Migration destination is invalid",
            400,
        ))
    })?;
    let repository_root = state
        .profiles
        .repository_root()
        .map_err(response)?
        .canonicalize()
        .map_err(|_| {
            response(problem(
                "migration_destination_invalid",
                "Profile repository destination is invalid",
                500,
            ))
        })?;
    if destination != repository_root {
        return Err(response(problem(
            "migration_destination_mismatch",
            "Migration destination must match the active profile repository",
            409,
        )));
    }
    let actor_id = principal
        .account_id
        .as_ref()
        .map_or("local", AccountId::as_str);
    let source_scope = input.source_root.to_string_lossy().into_owned();
    let destination_scope = destination.to_string_lossy().into_owned();
    let migration_scope = sha256_parts(&[&source_scope, &destination_scope]);
    let operation_id = operation_id(&headers, "migration.import", actor_id, &migration_scope)?;
    if let Some(operation) = existing_operation(&state, &operation_id)? {
        return Ok((StatusCode::ACCEPTED, Json(operation)));
    }
    begin_operation(&state, &operation_id, "migration.import", actor_id)?;
    let result = (|| {
        let (report, snapshot) = inspect_source(&input.source_root)?;
        let key = state
            .migration_key
            .read()
            .map_err(|_| {
                problem(
                    "migration_key_unavailable",
                    "Migration key unavailable",
                    503,
                )
            })?
            .clone();
        let metadata = PackageMetadata {
            created_at: migration_timestamp()?,
            tool_version: env!("CARGO_PKG_VERSION").to_owned(),
            tool_commit: option_env!("GIT_COMMIT").unwrap_or("unknown").to_owned(),
        };
        let key = key.ok_or_else(|| {
            problem(
                "migration_key_unavailable",
                "Migration key unavailable",
                503,
            )
        })?;
        let package = buglogin_migration::package::build_encrypted_profile_package(
            &input.source_root,
            &report,
            &snapshot,
            &metadata,
            &key,
        )?;
        let signing_key = state.migration_signing_key.read().map_err(|_| {
            problem(
                "migration_signing_key_unavailable",
                "Migration signing key unavailable",
                503,
            )
        })?;
        let signing_key = signing_key.as_ref().ok_or_else(|| {
            problem(
                "migration_signing_key_unavailable",
                "Migration signing key unavailable",
                503,
            )
        })?;
        let package = sign_package(
            &package,
            signing_key.as_bytes(),
            SignatureMetadata {
                key_id: "api-migration-signing-v1".to_owned(),
                signer: "buglogin-api".to_owned(),
                signed_at: metadata.created_at.clone(),
            },
        )?;
        verify_signed_package(&package, signing_key.as_bytes())?;
        let staged = stage_package(&package, &input.staging_parent, &input.run_id)?;
        let receipt = promote_staged(
            &staged,
            &input.destination_root,
            confirm_cutover(&input.confirmation)?,
        )?;
        let activation =
            match activate_package(&input.destination_root, &input.run_id, &package, Some(&key)) {
                Ok(value) => value,
                Err(error) => {
                    let confirmation = confirm_rollback("ROLLBACK_MIGRATION_RUN")?;
                    let _ = rollback_activation(
                        &input.destination_root,
                        &input.run_id,
                        confirmation.clone(),
                    );
                    let _ = rollback_cutover(&input.destination_root, &input.run_id, confirmation);
                    return Err(error);
                }
            };
        let control_import = match control_import::import_package(
            &state,
            &principal,
            &package,
            &format!("{operation_id}-control"),
        ) {
            Ok(receipt) => receipt,
            Err(error) => {
                let confirmation = confirm_rollback("ROLLBACK_MIGRATION_RUN")?;
                let _ = rollback_activation(
                    &input.destination_root,
                    &input.run_id,
                    confirmation.clone(),
                );
                let _ = rollback_cutover(&input.destination_root, &input.run_id, confirmation);
                return Err(error);
            }
        };
        Ok::<_, Problem>(serde_json::json!({
            "runId": receipt.run_id,
            "packageId": receipt.package_id,
            "destinationRoot": receipt.destination_root,
            "runRoot": receipt.run_root,
            "artifacts": receipt.artifacts,
            "createdProfiles": activation.created_profiles,
            "reusedProfiles": activation.reused_profiles,
            "controlImport": control_import
        }))
    })();
    match result {
        Ok(value) => finish_operation(&state, &operation_id, actor_id, value),
        Err(error) => fail_operation(&state, &operation_id, actor_id, error),
    }
}

fn existing_operation(state: &ApiState, operation_id: &str) -> Result<Option<Operation>, Response> {
    state.refresh_control_aggregate().map_err(response)?;
    state
        .operations
        .read()
        .map_err(|_| {
            response(problem(
                "operation_unavailable",
                "Operation service unavailable",
                503,
            ))
        })
        .map(|operations| operations.find(operation_id).cloned())
}

fn operation_id(
    headers: &HeaderMap,
    command: &str,
    actor_id: &str,
    scope_key: &str,
) -> Result<String, Response> {
    let key = headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
        .filter(|value| (16..=128).contains(&value.len()))
        .ok_or_else(|| {
            response(problem(
                "idempotency_key_required",
                "Valid idempotency key required",
                400,
            ))
        })?;
    let digest = sha256_parts(&[actor_id, command, scope_key, key]);
    Ok(format!("op_{}", &digest[..24]))
}

fn begin_operation(
    state: &ApiState,
    operation_id: &str,
    command_id: &str,
    actor_id: &str,
) -> Result<(), Response> {
    state
        .mutate_operation(
            operation_id,
            command_id,
            "started",
            actor_id,
            |operations| {
                operations.insert(Operation::queued(
                    operation_id,
                    command_id,
                    ResourceScope::global(),
                )?)?;
                operations.transition(
                    operation_id,
                    OperationStatus::Running,
                    Some(0.1),
                    None,
                    None,
                )?;
                Ok(())
            },
        )
        .map(|_| ())
        .map_err(response)
}

fn finish_operation(
    state: &ApiState,
    operation_id: &str,
    actor_id: &str,
    result: serde_json::Value,
) -> Result<(StatusCode, Json<Operation>), Response> {
    let operation = state
        .mutate_operation(
            operation_id,
            "operation.complete",
            "completed",
            actor_id,
            |operations| {
                operations.transition(
                    operation_id,
                    OperationStatus::Completed,
                    Some(1.0),
                    Some(result),
                    None,
                )?;
                Ok(())
            },
        )
        .map_err(response)?;
    Ok((StatusCode::ACCEPTED, Json(operation)))
}

fn fail_operation(
    state: &ApiState,
    operation_id: &str,
    actor_id: &str,
    error: Problem,
) -> Result<(StatusCode, Json<Operation>), Response> {
    let operation = state
        .mutate_operation(
            operation_id,
            "operation.fail",
            "failed",
            actor_id,
            |operations| {
                operations.transition(
                    operation_id,
                    OperationStatus::Failed,
                    Some(1.0),
                    None,
                    Some(error.clone()),
                )?;
                Ok(())
            },
        )
        .map_err(response)?;
    let status = StatusCode::from_u16(error.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    Ok((status, Json(operation)))
}

fn authorize_capability(
    principal: &Principal,
    grants: &[Grant],
    capability: &'static str,
    action: Action,
) -> Result<(), Response> {
    let capability = Capability::new(capability)
        .map_err(|_| response(problem("invalid_capability", "Invalid capability", 500)))?;
    if is_allowed(
        principal,
        &capability,
        action,
        &ResourceScope::global(),
        grants,
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

fn bearer_token(headers: &HeaderMap) -> Result<&str, Response> {
    headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            response(problem(
                "authentication_required",
                "Authentication required",
                401,
            ))
        })
}

fn required_idempotency(headers: &HeaderMap) -> Result<&str, Response> {
    headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
        .filter(|value| (16..=128).contains(&value.len()))
        .ok_or_else(|| {
            response(problem(
                "idempotency_key_required",
                "Valid idempotency key required",
                400,
            ))
        })
}

fn optional_idempotency(headers: &HeaderMap) -> Result<Option<&str>, Response> {
    let Some(value) = headers.get("idempotency-key") else {
        return Ok(None);
    };
    value
        .to_str()
        .ok()
        .filter(|value| value.is_ascii() && (16..=128).contains(&value.len()))
        .map(Some)
        .ok_or_else(|| {
            response(problem(
                "invalid_idempotency_key",
                "Refresh idempotency key is invalid",
                400,
            ))
        })
}

fn correlation_id(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-correlation-id")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= 128)
        .map(str::to_owned)
}

fn correlation(headers: &HeaderMap) -> Result<String, Response> {
    correlation_id(headers).ok_or_else(|| {
        response(problem(
            "correlation_required",
            "A valid correlation ID is required",
            400,
        ))
    })
}

pub(crate) fn request_now(headers: &HeaderMap) -> Result<u64, Response> {
    let correlation_id = correlation(headers)?;
    checked_now(&correlation_id).map_err(response)
}

pub(crate) fn request_now_ms(headers: &HeaderMap) -> Result<u64, Response> {
    let correlation_id = correlation(headers)?;
    checked_now_ms(&correlation_id).map_err(response)
}

pub(crate) fn checked_now(correlation_id: &str) -> Result<u64, Problem> {
    crate::clock::unix_time_seconds(correlation_id)
}

pub(crate) fn checked_now_ms(correlation_id: &str) -> Result<u64, Problem> {
    crate::clock::unix_time_millis(correlation_id)
}

fn cookie_scope_http(
    workspace_id: &str,
    profile_id: &str,
    correlation_id: &str,
) -> Result<CookieScope, Problem> {
    Ok(CookieScope {
        workspace_id: WorkspaceId::from_str(workspace_id).map_err(|_| {
            Problem::new(
                "invalid_workspace_id",
                "Invalid workspace ID",
                400,
                correlation_id,
            )
        })?,
        profile_id: ProfileId::from_str(profile_id).map_err(|_| {
            Problem::new(
                "invalid_profile_id",
                "Invalid profile ID",
                400,
                correlation_id,
            )
        })?,
    })
}

fn authorize_cookie(
    state: &ApiState,
    principal: &Principal,
    grants: &[Grant],
    scope: &CookieScope,
    action: Action,
    correlation_id: &str,
) -> Result<(), Problem> {
    let capability = Capability::new("CAP-COOK-001").map_err(|_| {
        Problem::new(
            "invalid_capability",
            "Invalid capability",
            500,
            correlation_id,
        )
    })?;
    let resource_scope = ResourceScope {
        workspace_id: Some(scope.workspace_id.clone()),
        profile_id: Some(scope.profile_id.clone()),
        ..ResourceScope::default()
    };
    if is_allowed(principal, &capability, action, &resource_scope, grants) {
        Ok(())
    } else if state.local_authority_enabled() && principal.roles.contains(&Role::LocalOwner) {
        Ok(())
    } else {
        Err(Problem::new(
            "cookie_forbidden",
            "Cookie access denied",
            403,
            correlation_id,
        ))
    }
}

fn set_cookie_state(
    state: &ApiState,
    authority: &mut CookieAuthority<NativeVault>,
    scope: &CookieScope,
    correlation_id: &str,
) -> Result<(), Problem> {
    let profile = state
        .get_profile_record(&scope.profile_id)?
        .ok_or_else(|| {
            Problem::new(
                "profile_not_found",
                "Profile not found",
                404,
                correlation_id,
            )
        })?;
    let state = match profile.value.state {
        ProfileState::Running | ProfileState::Launching | ProfileState::Stopping => {
            ProfileRuntimeState::Running
        }
        _ => ProfileRuntimeState::Stopped,
    };
    authority.set_profile_state(scope.clone(), state);
    Ok(())
}

fn require_web_origin(state: &ApiState, headers: &HeaderMap) -> Result<(), Response> {
    let origin = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| response(problem("origin_required", "Origin is required", 403)))?;
    if origin != state.web_origin.as_ref() {
        return Err(response(problem(
            "origin_invalid",
            "Origin is not allowed",
            403,
        )));
    }
    Ok(())
}

fn cookie<'a>(headers: &'a HeaderMap, name: &str) -> Result<&'a str, Response> {
    headers
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| {
            value.split(';').find_map(|part| {
                let (key, value) = part.trim().split_once('=')?;
                (key == name && !value.is_empty()).then_some(value)
            })
        })
        .ok_or_else(|| {
            response(problem(
                "authentication_required",
                "Authentication required",
                401,
            ))
        })
}

fn session_cookie(value: &str) -> Result<HeaderValue, Response> {
    cookie_value(format!(
        "{WEB_SESSION_COOKIE}={value}; Path=/; Max-Age={WEB_SESSION_SECONDS}; HttpOnly; Secure; SameSite=Lax"
    ))
}

fn csrf_cookie(value: &str) -> Result<HeaderValue, Response> {
    cookie_value(format!(
        "{WEB_CSRF_COOKIE}={value}; Path=/; Max-Age={WEB_SESSION_SECONDS}; Secure; SameSite=Lax"
    ))
}

fn clear_cookie(name: &str) -> Result<HeaderValue, Response> {
    cookie_value(format!(
        "{name}=; Path=/; Max-Age=0; HttpOnly; Secure; SameSite=Lax"
    ))
}

fn cookie_value(mut value: String) -> Result<HeaderValue, Response> {
    let header = HeaderValue::from_str(&value);
    value.zeroize();
    header.map_err(|_| {
        response(problem(
            "session_cookie_failed",
            "Session cookie could not be created",
            500,
        ))
    })
}

fn authenticate(state: &ApiState, headers: &HeaderMap) -> Result<Principal, Response> {
    state
        .authenticate_token(bearer_token(headers)?, request_now(headers)?)
        .map_err(response)
}

fn authenticated_browser_account_for_mutation(
    state: &ApiState,
    headers: &HeaderMap,
) -> Result<AccountId, Response> {
    require_web_origin(state, headers)?;
    let raw_session = cookie(headers, WEB_SESSION_COOKIE)?;
    let raw_csrf = headers
        .get("x-csrf-token")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| response(problem("csrf_required", "CSRF validation is required", 403)))?;
    let account_id = state
        .auth
        .read()
        .map_err(|_| {
            response(problem(
                "auth_unavailable",
                "Authentication unavailable",
                503,
            ))
        })?
        .authenticate_browser(raw_session, raw_csrf, request_now(headers)?)
        .map_err(response)?;
    require_active_account(state, account_id)
}

fn require_active_account(state: &ApiState, account_id: AccountId) -> Result<AccountId, Response> {
    let account = state
        .account(&account_id)
        .map_err(response)?
        .ok_or_else(|| {
            response(problem(
                "account_record_invalid",
                "Account record is invalid",
                500,
            ))
        })?;
    if account.status != AccountStatus::Active {
        return Err(response(problem(
            "account_inactive",
            "Account is not active",
            403,
        )));
    }
    Ok(account_id)
}

fn authenticated_account_for_mutation(
    state: &ApiState,
    headers: &HeaderMap,
) -> Result<AccountId, Response> {
    if headers.get(header::AUTHORIZATION).is_some() {
        return authenticate(state, headers)?.account_id.ok_or_else(|| {
            response(problem(
                "authentication_required",
                "Authentication required",
                401,
            ))
        });
    }
    if headers.get(header::COOKIE).is_none() {
        return Err(response(problem(
            "authentication_required",
            "Authentication required",
            401,
        )));
    }
    require_web_origin(state, headers)?;
    let raw_session = cookie(headers, WEB_SESSION_COOKIE)?;
    let raw_csrf = headers
        .get("x-csrf-token")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| response(problem("csrf_required", "CSRF validation is required", 403)))?;
    let account_id = state
        .auth
        .read()
        .map_err(|_| {
            response(problem(
                "auth_unavailable",
                "Authentication unavailable",
                503,
            ))
        })?
        .authenticate_browser(raw_session, raw_csrf, request_now(headers)?)
        .map_err(response)?;
    require_active_account(state, account_id)
}

fn authenticated_account_and_credential_version_for_mutation(
    state: &ApiState,
    headers: &HeaderMap,
) -> Result<(AccountId, Option<u32>), Response> {
    if headers.get(header::AUTHORIZATION).is_some() {
        let principal = authenticate(state, headers)?;
        let account_id = principal.account_id.ok_or_else(|| {
            response(problem(
                "authentication_required",
                "Authentication required",
                401,
            ))
        })?;
        let token = bearer_token(headers)?;
        let version = if token.starts_with("blapi_") {
            state
                .credentials
                .password_version(&account_id)
                .map_err(response)
        } else {
            let (session_account, version) = state
                .auth
                .read()
                .map_err(|_| {
                    response(problem(
                        "auth_unavailable",
                        "Authentication unavailable",
                        503,
                    ))
                })?
                .authenticate_with_credential_version(token, request_now(headers)?)
                .map_err(response)?;
            if session_account != account_id {
                return Err(response(problem(
                    "authentication_required",
                    "Authentication required",
                    401,
                )));
            }
            Ok(version)
        }?;
        let version = match version {
            Some(version) => Some(version),
            None => state
                .credentials
                .password_version(&account_id)
                .map_err(response)?,
        };
        return Ok((account_id, version));
    }
    if headers.get(header::COOKIE).is_none() {
        return Err(response(problem(
            "authentication_required",
            "Authentication required",
            401,
        )));
    }
    require_web_origin(state, headers)?;
    let raw_session = cookie(headers, WEB_SESSION_COOKIE)?;
    let raw_csrf = headers
        .get("x-csrf-token")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| response(problem("csrf_required", "CSRF validation is required", 403)))?;
    let (account_id, credential_version) = state
        .auth
        .read()
        .map_err(|_| {
            response(problem(
                "auth_unavailable",
                "Authentication unavailable",
                503,
            ))
        })?
        .authenticate_browser_with_credential_version(raw_session, raw_csrf, request_now(headers)?)
        .map_err(response)?;
    require_active_account(state, account_id.clone())?;
    let credential_version = match credential_version {
        Some(version) => Some(version),
        None => state
            .credentials
            .password_version(&account_id)
            .map_err(response)?,
    };
    Ok((account_id, credential_version))
}

fn grants(state: &ApiState) -> Result<Vec<Grant>, Response> {
    state.grants().map_err(response)
}

fn authorized(
    state: &ApiState,
    principal: &Principal,
    grants: &[Grant],
    profile_id: &ProfileId,
    action: Action,
    capability: &'static str,
) -> Result<bool, Response> {
    crate::services::profile_hosted::HostedProfileService::authorized(
        state, principal, grants, profile_id, action, capability,
    )
    .map_err(response)
}

fn authorize_profile(
    state: &ApiState,
    principal: &Principal,
    grants: &[Grant],
    id: &ProfileId,
) -> Result<(), Response> {
    if authorized(state, principal, grants, id, Action::Read, "profile.read")? {
        Ok(())
    } else {
        Err(response(problem(
            "profile_forbidden",
            "Profile access denied",
            403,
        )))
    }
}

fn authorize_action(
    state: &ApiState,
    principal: &Principal,
    grants: &[Grant],
    id: &ProfileId,
    action: Action,
    capability: &'static str,
) -> Result<(), Response> {
    let allowed = authorized(state, principal, grants, id, action, capability)?;
    if allowed {
        Ok(())
    } else {
        Err(response(problem(
            "profile_forbidden",
            "Profile access denied",
            403,
        )))
    }
}

async fn require_correlation(
    headers: HeaderMap,
    request: axum::extract::Request,
    next: Next,
) -> Result<Response, Response> {
    let exempt = matches!(
        request.uri().path(),
        "/v1/health" | "/v1/openapi.yaml" | "/v1/readyz"
    );
    if exempt {
        return Ok(next.run(request).await);
    }
    let correlation_id = correlation_id(&headers).ok_or_else(correlation_required_response)?;
    let mut response = REQUEST_CORRELATION_ID
        .scope(correlation_id.clone(), next.run(request))
        .await;
    if (response.status().is_client_error() || response.status().is_server_error())
        && !response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("application/problem+json"))
    {
        response = canonicalize_unstructured_error(response, &correlation_id);
    }
    if let Ok(value) = HeaderValue::from_str(&correlation_id) {
        response.headers_mut().insert("x-correlation-id", value);
    }
    Ok(response)
}

fn canonicalize_unstructured_error(original: Response, correlation_id: &str) -> Response {
    let status = original.status();
    let (code, title) = match status {
        StatusCode::BAD_REQUEST | StatusCode::UNPROCESSABLE_ENTITY => {
            ("request_invalid", "Request is invalid")
        }
        StatusCode::UNAUTHORIZED => ("authentication_required", "Authentication required"),
        StatusCode::FORBIDDEN => ("permission_denied", "Permission denied"),
        StatusCode::NOT_FOUND => ("not_found", "Resource not found"),
        StatusCode::METHOD_NOT_ALLOWED => ("method_not_allowed", "Method not allowed"),
        StatusCode::CONFLICT => ("conflict", "Request conflicts with current state"),
        StatusCode::PAYLOAD_TOO_LARGE => ("payload_too_large", "Request payload is too large"),
        StatusCode::TOO_MANY_REQUESTS => ("rate_limited", "Too many requests"),
        StatusCode::SERVICE_UNAVAILABLE => ("service_unavailable", "Service unavailable"),
        StatusCode::GATEWAY_TIMEOUT => ("request_timeout", "Request timed out"),
        _ if status.is_server_error() => ("internal_error", "Internal server error"),
        _ => ("request_failed", "Request failed"),
    };
    let mut problem = Problem::new(code, title, status.as_u16(), correlation_id);
    problem.retryable = status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS;
    let (original_parts, _) = original.into_parts();
    let mut normalized = response(problem);
    for name in [
        header::CACHE_CONTROL,
        header::PRAGMA,
        header::RETRY_AFTER,
        header::WWW_AUTHENTICATE,
        header::ALLOW,
    ] {
        if let Some(value) = original_parts.headers.get(&name) {
            normalized.headers_mut().insert(name, value.clone());
        }
    }
    normalized
}

fn correlation_required_response() -> Response {
    let mut random = [0u8; 16];
    if getrandom::fill(&mut random).is_err() {
        return response(
            problem(
                "correlation_id_unavailable",
                "Request correlation is unavailable",
                503,
            )
            .retryable(true),
        );
    }
    let correlation_id = format!(
        "cor_{}",
        random
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
    let mut problem = problem(
        "correlation_required",
        "A valid correlation ID is required",
        400,
    );
    problem.correlation_id = correlation_id.clone();
    let mut response = response(problem);
    if let Ok(value) = HeaderValue::from_str(&correlation_id) {
        response.headers_mut().insert("x-correlation-id", value);
    }
    response
}

async fn openapi_document() -> Response {
    (
        [(header::CONTENT_TYPE, "application/yaml; charset=utf-8")],
        include_str!("../../openapi/openapi-v1.yaml"),
    )
        .into_response()
}

async fn get_runtime_catalog(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Response, Response> {
    let catalog = state.runtime_catalog.as_ref().ok_or_else(|| {
        response(
            problem(
                "runtime_catalog_unavailable",
                "Signed runtime catalog is not configured",
                503,
            )
            .retryable(true),
        )
    })?;
    let etag = HeaderValue::from_str(catalog.etag()).map_err(|_| {
        response(problem(
            "runtime_catalog_unavailable",
            "Signed runtime catalog is unavailable",
            503,
        ))
    })?;
    if if_none_match_satisfied(&headers, catalog.etag()) {
        let mut result = StatusCode::NOT_MODIFIED.into_response();
        result.headers_mut().insert(header::ETAG, etag);
        result.headers_mut().insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=60"),
        );
        return Ok(result);
    }
    let mut result = Json(catalog.catalog()).into_response();
    result.headers_mut().insert(header::ETAG, etag);
    result.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=60"),
    );
    Ok(result)
}

fn if_none_match_satisfied(headers: &HeaderMap, etag: &str) -> bool {
    headers
        .get_all(header::IF_NONE_MATCH)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .any(|candidate| {
            candidate == "*" || candidate.strip_prefix("W/").unwrap_or(candidate) == etag
        })
}

#[cfg(test)]
mod runtime_catalog_http_tests {
    use super::if_none_match_satisfied;
    use axum::http::{HeaderMap, HeaderValue, header};

    #[test]
    fn if_none_match_accepts_weak_tags_and_multiple_header_values() {
        let mut headers = HeaderMap::new();
        headers.append(
            header::IF_NONE_MATCH,
            HeaderValue::from_static("\"old\", W/\"catalog-hash\""),
        );
        headers.append(header::IF_NONE_MATCH, HeaderValue::from_static("\"other\""));

        assert!(if_none_match_satisfied(&headers, "\"catalog-hash\""));
    }

    #[test]
    fn if_none_match_accepts_wildcard_and_rejects_unmatched_tags() {
        let mut headers = HeaderMap::new();
        headers.insert(header::IF_NONE_MATCH, HeaderValue::from_static("*"));
        assert!(if_none_match_satisfied(&headers, "\"catalog-hash\""));

        headers.insert(header::IF_NONE_MATCH, HeaderValue::from_static("\"other\""));
        assert!(!if_none_match_satisfied(&headers, "\"catalog-hash\""));
    }
}

fn response(mut problem: Problem) -> Response {
    if let Ok(correlation_id) = REQUEST_CORRELATION_ID.try_with(|id| id.clone()) {
        problem.correlation_id = correlation_id;
    }
    let status = StatusCode::from_u16(problem.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (
        status,
        [(header::CONTENT_TYPE, "application/problem+json")],
        Json(problem),
    )
        .into_response()
}

pub(super) fn canonical_problem(status: u16, value: serde_json::Value) -> Problem {
    let fields = value.as_object();
    let code = fields
        .and_then(|fields| fields.get("code"))
        .and_then(serde_json::Value::as_str)
        .filter(|code| !code.is_empty())
        .unwrap_or("internal_error");
    let title = fields
        .and_then(|fields| fields.get("title"))
        .and_then(serde_json::Value::as_str)
        .filter(|title| !title.is_empty())
        .unwrap_or("Request failed");
    let fallback_correlation = fields
        .and_then(|fields| fields.get("correlationId"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("api");
    let correlation_id = REQUEST_CORRELATION_ID
        .try_with(|id| id.clone())
        .unwrap_or_else(|_| fallback_correlation.to_owned());
    let mut problem = Problem::new(code, title, status, correlation_id);
    if let Some(value) =
        fields.and_then(|fields| fields.get("type").and_then(serde_json::Value::as_str))
    {
        problem.problem_type = value.to_owned();
    }
    problem.detail = fields
        .and_then(|fields| fields.get("detail"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    problem.instance = fields
        .and_then(|fields| fields.get("instance"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    problem.retryable = fields
        .and_then(|fields| fields.get("retryable"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(status >= 500 || status == 429);
    if let Some(values) = fields
        .and_then(|fields| fields.get("fields"))
        .and_then(serde_json::Value::as_object)
    {
        problem.fields.extend(
            values
                .iter()
                .map(|(key, value)| (key.clone(), value.clone())),
        );
    }
    problem
}

fn runtime_key(runtime: &Runtime) -> String {
    serde_json::to_string(runtime).unwrap_or_else(|_| "unknown".to_owned())
}

fn profile_key_share_envelope_record_key(
    resource_id: &buglogin_domain::ids::ResourceId,
    member_id: &MemberId,
    device_id: &str,
) -> String {
    format!(
        "{}:{}:{}",
        resource_id.as_str(),
        member_id.as_str(),
        device_id
    )
}

fn build_profile_resource_index(
    profile_resources: &BTreeMap<ProfileId, ProfileResource>,
) -> BTreeMap<(WorkspaceId, buglogin_domain::ids::ResourceId), BTreeSet<ProfileId>> {
    let mut index = BTreeMap::new();
    for binding in profile_resources.values() {
        index
            .entry((binding.workspace_id.clone(), binding.resource_id.clone()))
            .or_insert_with(BTreeSet::new)
            .insert(binding.profile_id.clone());
    }
    index
}

fn sync_manifest_key(
    workspace_id: &buglogin_domain::ids::WorkspaceId,
    resource_id: &buglogin_domain::ids::ResourceId,
) -> String {
    format!("{}:{}", workspace_id.as_str(), resource_id.as_str())
}

fn storage_quota_problem(storage_bytes: u64, max_storage_bytes: u64) -> Problem {
    Problem::new(
        "storage_quota_exceeded",
        "Workspace storage quota exceeded",
        409,
        "sync",
    )
    .with_field("storageBytes", storage_bytes)
    .with_field("maxStorageBytes", max_storage_bytes)
}

fn sync_manifest_object_storage_key(
    manifest: &SyncManifest,
    object: &buglogin_sync::manifest::ManifestObject,
) -> String {
    format!(
        "profiles/{}/{}/{}/{}",
        manifest.resource_id.as_str(),
        object.source_generation.unwrap_or(manifest.generation),
        object.checksum,
        object.key
    )
}

fn release_promoted_sync_blob_reservations(
    aggregate: &mut ControlAggregate,
    journal: &SyncMigrationJournal,
) {
    let manifest_key = format!("{}:{}", journal.workspace_id, journal.resource_id);
    let committed_objects = aggregate
        .sync_manifests
        .get(&manifest_key)
        .map(|manifest| {
            manifest
                .objects
                .iter()
                .map(|object| {
                    (
                        sync_manifest_object_storage_key(manifest, object),
                        (object.checksum.clone(), object.size),
                    )
                })
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    for object in &journal.reserved_objects {
        if committed_objects
            .get(&object.storage_key)
            .is_some_and(|(checksum, size)| checksum == &object.checksum && *size == object.size)
            && aggregate
                .sync_blob_reservations
                .get(&object.storage_key)
                .is_some_and(|reservation| {
                    reservation.workspace_id == journal.workspace_id
                        && reservation.resource_id == journal.resource_id
                        && reservation.checksum == object.checksum
                        && reservation.size == object.size
                })
        {
            aggregate.sync_blob_reservations.remove(&object.storage_key);
        }
    }
}

fn validate_manifest_gc_claims(
    manifest: &SyncManifest,
    claims: &BTreeMap<String, BTreeSet<String>>,
) -> Result<(), Problem> {
    if manifest.objects.iter().any(|object| {
        claims
            .get(&sync_manifest_object_storage_key(manifest, object))
            .is_some_and(|owners| !owners.is_empty())
    }) {
        return Err(problem(
            "sync_object_gc_in_progress",
            "A profile object is being safely retired; retry the manifest update",
            409,
        )
        .retryable(true));
    }
    Ok(())
}

fn manifest_storage_references(aggregate: &ControlAggregate) -> BTreeSet<String> {
    aggregate
        .sync_manifests
        .values()
        .flat_map(|manifest| {
            manifest
                .objects
                .iter()
                .map(|object| sync_manifest_object_storage_key(manifest, object))
        })
        .collect()
}

fn manifest_storage_references_for_workspace(
    aggregate: &ControlAggregate,
    workspace_id: &str,
) -> BTreeSet<String> {
    aggregate
        .sync_manifests
        .values()
        .filter(|manifest| manifest.workspace_id.as_str() == workspace_id)
        .flat_map(|manifest| {
            manifest
                .objects
                .iter()
                .map(|object| sync_manifest_object_storage_key(manifest, object))
        })
        .collect()
}

fn manifest_storage_references_for_resource(
    aggregate: &ControlAggregate,
    workspace_id: &str,
    resource_id: &str,
) -> BTreeSet<String> {
    aggregate
        .sync_manifests
        .values()
        .filter(|manifest| {
            manifest.workspace_id.as_str() == workspace_id
                && manifest.resource_id.as_str() == resource_id
        })
        .flat_map(|manifest| {
            manifest
                .objects
                .iter()
                .map(|object| sync_manifest_object_storage_key(manifest, object))
        })
        .collect()
}

fn sync_gc_advisory_lock_id(workspace_id: &str, resource_id: &str) -> i64 {
    let parts = ["sync-object-gc", workspace_id, resource_id];
    let mut digest = Sha256::new();
    for part in parts {
        digest.update((part.len() as u64).to_be_bytes());
        digest.update(part.as_bytes());
    }
    let digest = digest.finalize();
    let mut lock_bytes = [0_u8; 8];
    lock_bytes.copy_from_slice(&digest[..8]);
    i64::from_be_bytes(lock_bytes)
}

fn sync_migration_journal_advisory_lock_id(journal_id: &str) -> i64 {
    let namespace = "migration-sync-journal-v1";
    let mut digest = Sha256::new();
    digest.update((namespace.len() as u64).to_be_bytes());
    digest.update(namespace.as_bytes());
    digest.update((journal_id.len() as u64).to_be_bytes());
    digest.update(journal_id.as_bytes());
    let digest = digest.finalize();
    let mut lock_bytes = [0_u8; 8];
    lock_bytes.copy_from_slice(&digest[..8]);
    i64::from_be_bytes(lock_bytes)
}

pub(super) struct SyncMigrationJournalLock<'a> {
    state: &'a ApiState,
    lock_id: i64,
}

impl<'a> SyncMigrationJournalLock<'a> {
    pub(super) fn acquire(state: &'a ApiState, journal_id: &str) -> Result<Self, Problem> {
        let lock_id = sync_migration_journal_advisory_lock_id(journal_id);
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

impl Drop for SyncMigrationJournalLock<'_> {
    fn drop(&mut self) {
        let _ = self
            .state
            .control_persistence
            .release_advisory_lock(self.lock_id);
    }
}

fn sensitive_json<T: Serialize>(value: T) -> Response {
    no_store(Json(value).into_response())
}

fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
        .headers_mut()
        .insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    response
}

fn problem(code: &'static str, title: &'static str, status: u16) -> Problem {
    Problem::new(code, title, status, "api")
}

fn proxy_repository_problem(error: RepositoryError) -> Problem {
    let detail = format!("proxy repository state invalid: {error:?}");
    Problem::new("proxy_state_invalid", detail, 500, "control")
}

fn canonical_mcp_idempotency_key(
    actor_id: &str,
    scope: &ResourceScope,
    command_id: &str,
    client_key: &str,
) -> Result<String, Problem> {
    use sha2::Digest;
    let bytes = serde_json::to_vec(&(actor_id, scope, command_id, client_key)).map_err(|_| {
        problem(
            "idempotency_key_failed",
            "Idempotency key could not be canonicalized",
            500,
        )
    })?;
    Ok(hex_encode(sha2::Sha256::digest(bytes)))
}

fn sha256_parts(parts: &[&str]) -> String {
    use sha2::Digest;
    let mut digest = sha2::Sha256::new();
    for part in parts {
        digest.update((part.len() as u64).to_be_bytes());
        digest.update(part.as_bytes());
    }
    hex_encode(digest.finalize())
}

fn hex_encode(bytes: impl AsRef<[u8]>) -> String {
    bytes
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn random_bootstrap_id(prefix: &str) -> Result<String, Problem> {
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(|_| {
        Problem::new(
            "bootstrap_randomness_unavailable",
            "Initial owner bootstrap is unavailable",
            503,
            "identity",
        )
    })?;
    Ok(format!("{prefix}_{}", hex_encode(random)))
}

fn session_id_for_raw(raw: &str) -> String {
    use sha2::Digest;
    hex_encode(sha2::Sha256::digest(raw.as_bytes()))
}

fn projected_profile_state(state: &str) -> Result<ProfileState, Problem> {
    match state {
        "running" => Ok(ProfileState::Running),
        "stopped" => Ok(ProfileState::Ready),
        _ => Err(problem(
            "runtime_projection_invalid",
            "Runtime projection is invalid",
            500,
        )),
    }
}
fn migration_timestamp() -> Result<String, Problem> {
    crate::clock::rfc3339("migration")
}
#[cfg(test)]
pub(crate) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs())
        .expect("tests require a valid system clock")
}

#[cfg(test)]
mod persistence_tests {
    use super::*;
    use buglogin_browser::profile::ProfileService;
    use buglogin_storage::local::LocalRepository;
    use buglogin_storage::{
        blob::checksum,
        control::{
            ClaimControlCommand, CompleteControlCommand, ControlAuditEntry, ControlCommandResult,
            ControlIdempotencyClaim, ControlRecord, ControlRecordStore, ExecuteControlCommand,
            MemoryControlRecordStore, PutControlRecord,
        },
        remote::{
            ConcurrentRemoteChunkStore, MemoryRemoteChunkStore, RemoteChunk, RemoteChunkStore,
        },
    };

    use std::{
        fs,
        path::PathBuf,
        sync::{
            Arc,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        },
        time::Duration,
    };

    #[tokio::test]
    async fn web_and_desktop_login_reject_pending_identity_generically() {
        use axum::{body::Body, http::StatusCode};
        use buglogin_domain::ids::AccountId;
        use tower::ServiceExt;

        let store = MemoryControlRecordStore::default();
        let account_id = AccountId::new("acct_pending_http").unwrap();
        let state = ApiState::hosted_with_control_store(ProfileService::disabled(), store).unwrap();
        state
            .account_persistence
            .provision_default(account_id, "2026-10-19T00:00:00Z".into())
            .unwrap();
        let identity = state
            .provision_web_identity(
                "acct_pending_http",
                "Pending.Owner",
                "pending.owner@example.com",
                "correct horse battery staple",
            )
            .unwrap();
        assert_eq!(
            identity.verification,
            buglogin_domain::account::IdentityVerification::Pending
        );

        let app = router(state);
        let web = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/v1/auth/login")
                    .header("content-type", "application/json")
                    .header("origin", "http://127.0.0.1:3000")
                    .header("x-correlation-id", "pending-web-auth-test")
                    .body(Body::from(
                        serde_json::to_vec(&serde_json::json!({
                            "identifier": "pending.owner@example.com",
                            "password": "correct horse battery staple"
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(web.status(), StatusCode::UNAUTHORIZED);
        assert!(
            web.headers()
                .get_all(header::SET_COOKIE)
                .iter()
                .next()
                .is_none()
        );
        let web_body: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(web.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();

        let desktop = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/v1/auth/desktop/login")
                    .header("content-type", "application/json")
                    .header("x-correlation-id", "pending-desktop-auth-test")
                    .body(Body::from(
                        serde_json::to_vec(&serde_json::json!({
                            "identifier": "pending.owner@example.com",
                            "password": "correct horse battery staple",
                            "deviceId": "pending-login-device-01"
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(desktop.status(), StatusCode::UNAUTHORIZED);
        let desktop_body: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(desktop.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();

        assert_eq!(web_body["code"], "invalid_credentials");
        assert_eq!(web_body["code"], desktop_body["code"]);
        assert_eq!(web_body["title"], desktop_body["title"]);
        assert!(desktop_body.get("accessToken").is_none());
        assert!(desktop_body.get("refreshToken").is_none());
    }

    #[test]
    fn first_owner_bootstrap_requires_and_records_operator_email_attestation() {
        let state = ApiState::hosted_with_control_store(
            ProfileService::disabled(),
            MemoryControlRecordStore::default(),
        )
        .unwrap();
        let error = state
            .bootstrap_first_owner(FirstOwnerBootstrapInput {
                account_id: None,
                username: "attested-owner".into(),
                email: "attested-owner@example.com".into(),
                operator_attested_email: false,
                password: zeroize::Zeroizing::new("initial owner password 2026".into()),
                organization_name: "BugLogin Organization".into(),
                workspace_name: "Default Workspace".into(),
                max_workspaces: 10,
                max_profiles: 1_000,
                max_members: 100,
                max_storage_bytes: 10 * 1024 * 1024 * 1024,
            })
            .unwrap_err();
        assert_eq!(error.code, "owner_email_attestation_required");
        assert!(
            state
                .control_persistence
                .list::<AccountIdentity>("identity", "account_identity")
                .unwrap()
                .is_empty()
        );

        let owner = state
            .bootstrap_first_owner(FirstOwnerBootstrapInput {
                account_id: None,
                username: "attested-owner".into(),
                email: "attested-owner@example.com".into(),
                operator_attested_email: true,
                password: zeroize::Zeroizing::new("initial owner password 2026".into()),
                organization_name: "BugLogin Organization".into(),
                workspace_name: "Default Workspace".into(),
                max_workspaces: 10,
                max_profiles: 1_000,
                max_members: 100,
                max_storage_bytes: 10 * 1024 * 1024 * 1024,
            })
            .unwrap();
        let identity = state
            .identity_persistence
            .load(&owner.account_id)
            .unwrap()
            .unwrap();
        assert_eq!(
            identity.verification,
            buglogin_domain::account::IdentityVerification::Verified
        );
        assert_eq!(
            state
                .credentials
                .authenticate(
                    "attested-owner@example.com",
                    "initial owner password 2026",
                    now()
                )
                .unwrap(),
            owner.account_id
        );
    }

    #[test]
    fn operator_admin_password_reset_revokes_sessions_and_api_tokens_together() {
        let store = MemoryControlRecordStore::default();
        let state = ApiState::hosted_with_control_store(ProfileService::disabled(), store).unwrap();
        let owner = state
            .bootstrap_first_owner(FirstOwnerBootstrapInput {
                account_id: None,
                username: "reset-owner".into(),
                email: "reset-owner@example.com".into(),
                operator_attested_email: true,
                password: zeroize::Zeroizing::new("initial owner password 2026".into()),
                organization_name: "BugLogin Organization".into(),
                workspace_name: "Default Workspace".into(),
                max_workspaces: 10,
                max_profiles: 1_000,
                max_members: 100,
                max_storage_bytes: 10 * 1024 * 1024 * 1024,
            })
            .unwrap();
        let access_token = state.issue_token(owner.account_id.as_str()).unwrap();
        let issued_at = now();
        let credential_version = state
            .credentials
            .password_version(&owner.account_id)
            .unwrap()
            .unwrap();
        let api_token = state
            .api_tokens
            .issue(
                owner.account_id.clone(),
                credential_version,
                "reset-regression",
                BTreeSet::from(["profile.read:read".to_owned()]),
                issued_at + 3_600,
                issued_at,
            )
            .unwrap();

        let reset = state
            .operator_reset_platform_admin_password(
                &owner.account_id,
                "replacement owner password 2026",
            )
            .unwrap();

        assert_eq!(reset.revoked_sessions, 1);
        assert_eq!(reset.revoked_api_tokens, 1);
        assert_eq!(
            state
                .authenticate_token(&access_token, now())
                .unwrap_err()
                .code,
            "session_revoked"
        );
        assert_eq!(
            state
                .api_tokens
                .authenticate(&api_token.token, now())
                .unwrap_err()
                .code,
            "invalid_api_token"
        );
    }

    #[test]
    fn operator_admin_listing_returns_identity_and_status_without_credentials() {
        let state = ApiState::hosted_with_control_store(
            ProfileService::disabled(),
            MemoryControlRecordStore::default(),
        )
        .unwrap();
        let owner = state
            .bootstrap_first_owner(FirstOwnerBootstrapInput {
                account_id: None,
                username: "list-owner".into(),
                email: "list-owner@example.com".into(),
                operator_attested_email: true,
                password: zeroize::Zeroizing::new("initial owner password 2026".into()),
                organization_name: "BugLogin Organization".into(),
                workspace_name: "Default Workspace".into(),
                max_workspaces: 10,
                max_profiles: 1_000,
                max_members: 100,
                max_storage_bytes: 10 * 1024 * 1024 * 1024,
            })
            .unwrap();

        let admins = state.operator_list_platform_admins().unwrap();

        assert_eq!(admins.len(), 1);
        assert_eq!(admins[0].account_id, owner.account_id);
        assert_eq!(admins[0].username.as_deref(), Some("list-owner"));
        assert_eq!(admins[0].email.as_deref(), Some("list-owner@example.com"));
        assert_eq!(admins[0].status, AccountStatus::Active);
        assert!(admins[0].identity_available);
        let serialized = serde_json::to_value(&admins).unwrap();
        assert!(serialized[0].get("password").is_none());
        assert!(serialized[0].get("token").is_none());
    }

    #[test]
    fn operator_admin_provision_is_atomic_verified_and_keeps_existing_tenants() {
        let state = ApiState::hosted_with_control_store(
            ProfileService::disabled(),
            MemoryControlRecordStore::default(),
        )
        .unwrap();
        let owner = state
            .bootstrap_first_owner(FirstOwnerBootstrapInput {
                account_id: None,
                username: "existing-owner".into(),
                email: "existing-owner@example.com".into(),
                operator_attested_email: true,
                password: zeroize::Zeroizing::new("initial owner password 2026".into()),
                organization_name: "Existing Organization".into(),
                workspace_name: "Existing Workspace".into(),
                max_workspaces: 10,
                max_profiles: 1_000,
                max_members: 100,
                max_storage_bytes: 10 * 1024 * 1024 * 1024,
            })
            .unwrap();

        let created = state
            .operator_create_platform_admin(OperatorPlatformAdminProvisionInput {
                username: "new-owner".into(),
                email: "new-owner@example.com".into(),
                password: zeroize::Zeroizing::new("new admin password 2026".into()),
                organization_name: "New Owner Organization".into(),
                workspace_name: "New Owner Workspace".into(),
            })
            .unwrap();

        let admins = state.operator_list_platform_admins().unwrap();
        assert_eq!(admins.len(), 2);
        assert!(admins.iter().any(|admin| {
            admin.account_id == created.account_id
                && admin.email.as_deref() == Some("new-owner@example.com")
                && admin.status == AccountStatus::Active
                && admin.identity_available
        }));
        assert_eq!(
            state
                .credentials
                .authenticate("new-owner@example.com", "new admin password 2026", now())
                .unwrap(),
            created.account_id
        );
        let aggregate = state.control_aggregate().unwrap();
        let platform_principal = Principal {
            account_id: Some(created.account_id.clone()),
            roles: BTreeSet::from([Role::PlatformAdmin]),
            ..Principal::default()
        };
        assert!(
            aggregate
                .workspaces
                .get_workspace_record(
                    &platform_principal,
                    &[],
                    &created.organization_id,
                    &created.workspace_id,
                )
                .is_ok()
        );
        assert!(
            aggregate
                .platform_admin_accounts
                .contains(&owner.account_id)
        );
        assert!(
            aggregate
                .platform_admin_accounts
                .contains(&created.account_id)
        );
        let repeated = state
            .operator_create_platform_admin(OperatorPlatformAdminProvisionInput {
                username: "another-owner".into(),
                email: "another-owner@example.com".into(),
                password: zeroize::Zeroizing::new("another admin password 2026".into()),
                organization_name: "Another Organization".into(),
                workspace_name: "Another Workspace".into(),
            })
            .unwrap_err();
        assert_eq!(repeated.code, "operator_admin_already_provisioned");
    }

    #[test]
    fn sync_gc_advisory_lock_ids_are_stable_and_resource_scoped() {
        assert_eq!(
            sync_gc_advisory_lock_id("workspace-a", "resource-a"),
            sync_gc_advisory_lock_id("workspace-a", "resource-a")
        );
        assert_ne!(
            sync_gc_advisory_lock_id("workspace-a", "resource-a"),
            sync_gc_advisory_lock_id("workspace-a", "resource-b")
        );
        assert_ne!(
            sync_gc_advisory_lock_id("workspace-a", "resource-a"),
            sync_gc_advisory_lock_id("workspace-b", "resource-a")
        );
    }

    #[tokio::test]
    async fn problem_boundary_uses_request_correlation_and_canonicalizes_legacy_errors() {
        let response = REQUEST_CORRELATION_ID
            .scope("cor_http_problem_test".to_owned(), async {
                response(canonical_problem(
                    503,
                    serde_json::json!({
                        "code": "control_unavailable",
                        "title": "Control unavailable",
                    }),
                ))
            })
            .await;
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/problem+json"
        );
        let value: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), 16 * 1024)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            value["type"],
            "https://buglogin.local/problems/control_unavailable"
        );
        assert_eq!(value["status"], 503);
        assert_eq!(value["code"], "control_unavailable");
        assert_eq!(value["correlationId"], "cor_http_problem_test");
        assert_eq!(value["retryable"], true);
        assert!(value["fields"].is_object());
    }

    #[test]
    fn sync_import_activation_removes_only_matching_promoted_reservations() {
        let workspace_id = WorkspaceId::new("ws_sync_reservation_promote").unwrap();
        let resource_id =
            buglogin_domain::ids::ResourceId::new("res_sync_reservation_promote").unwrap();
        let manifest = SyncManifest::new(
            workspace_id.clone(),
            resource_id.clone(),
            9,
            2,
            Some(1),
            b"reservation-promotion",
            vec![buglogin_sync::manifest::ManifestObject::new("profile/state", b"one").unwrap()],
        )
        .unwrap();
        let committed_object = &manifest.objects[0];
        let committed_key = sync_manifest_object_storage_key(&manifest, committed_object);
        let committed_checksum = committed_object.checksum.clone();
        let committed_size = committed_object.size;
        let other_key = format!("profiles/{}/2/other/profile/other", resource_id.as_str());
        let reservation = |storage_key: String, checksum: String, size| SyncBlobReservation {
            workspace_id: workspace_id.as_str().into(),
            resource_id: resource_id.as_str().into(),
            storage_key,
            checksum,
            size,
            expires_at_ms: 1_900_000_000_000,
        };
        let mut aggregate = ControlAggregate::default();
        aggregate.sync_manifests.insert(
            format!("{}:{}", workspace_id.as_str(), resource_id.as_str()),
            manifest,
        );
        aggregate.sync_blob_reservations.insert(
            committed_key.clone(),
            reservation(
                committed_key.clone(),
                committed_checksum.clone(),
                committed_size,
            ),
        );
        aggregate.sync_blob_reservations.insert(
            other_key.clone(),
            reservation(other_key.clone(), "other-checksum".into(), 5),
        );
        let journal_object =
            |storage_key: String, checksum: String, size| SyncMigrationJournalObject {
                storage_key,
                checksum,
                size,
            };
        let journal = SyncMigrationJournal {
            schema: "buglogin.migration-sync-journal/1".into(),
            operation_id: "op_sync_reservation_promote".into(),
            export_id: "export_sync_reservation_promote".into(),
            export_sha256: "a".repeat(64),
            manifest_sha256: "b".repeat(64),
            blob_set_sha256: "c".repeat(64),
            workspace_id: workspace_id.as_str().into(),
            resource_id: resource_id.as_str().into(),
            base_generation: Some(1),
            target_generation: 2,
            state: SyncMigrationJournalState::Active,
            created_objects: Vec::new(),
            resumed_objects: Vec::new(),
            reserved_objects: vec![
                journal_object(committed_key.clone(), committed_checksum, committed_size),
                journal_object(other_key.clone(), "other-checksum".into(), 5),
            ],
            destination_fencing_token: Some(9),
            control_revision_before: 1,
            control_revision_after: Some(2),
            updated_at_ms: 1_900_000_000_000,
            error_code: None,
        };

        release_promoted_sync_blob_reservations(&mut aggregate, &journal);

        assert!(
            !aggregate
                .sync_blob_reservations
                .contains_key(&committed_key)
        );
        assert!(aggregate.sync_blob_reservations.contains_key(&other_key));
    }

    #[test]
    fn local_operation_ids_are_scoped_to_actor_command_resource_and_key() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "idempotency-key",
            axum::http::HeaderValue::from_static("operation-key-123456"),
        );
        let first = operation_id(&headers, "profile.launch", "acc_1", "prf_1").unwrap();
        assert_eq!(
            first,
            operation_id(&headers, "profile.launch", "acc_1", "prf_1").unwrap()
        );
        assert_ne!(
            first,
            operation_id(&headers, "profile.launch", "acc_2", "prf_1").unwrap()
        );
        assert_ne!(
            first,
            operation_id(&headers, "profile.launch", "acc_1", "prf_2").unwrap()
        );
    }

    #[test]
    fn declared_body_limit_rejects_only_valid_lengths_above_the_cap() {
        let mut headers = HeaderMap::new();
        assert!(!declared_body_exceeds_limit(&headers, 64).unwrap());

        headers.insert(header::CONTENT_LENGTH, HeaderValue::from_static("64"));
        assert!(!declared_body_exceeds_limit(&headers, 64).unwrap());

        headers.insert(header::CONTENT_LENGTH, HeaderValue::from_static("65"));
        assert!(declared_body_exceeds_limit(&headers, 64).unwrap());

        headers.insert(header::CONTENT_LENGTH, HeaderValue::from_static("invalid"));
        assert_eq!(
            declared_body_exceeds_limit(&headers, 64).unwrap_err().code,
            "content_length_invalid"
        );
    }

    #[test]
    fn sync_gc_claim_fences_manifest_for_exact_physical_object() {
        use buglogin_domain::ids::{ResourceId, WorkspaceId};
        use buglogin_sync::manifest::{ManifestObject, SyncManifest};

        let workspace_id = WorkspaceId::from_str("ws_gc_fence").unwrap();
        let resource_id = ResourceId::from_str("res_gc_fence").unwrap();
        let object = ManifestObject::new("profile/state.json", b"payload").unwrap();
        let manifest = SyncManifest::new(
            workspace_id,
            resource_id,
            1,
            2,
            Some(1),
            b"root",
            vec![object.clone()],
        )
        .unwrap();
        let storage_key = sync_manifest_object_storage_key(&manifest, &object);
        let claims = BTreeMap::from([(storage_key, BTreeSet::from(["journal-1".into()]))]);

        let error = validate_manifest_gc_claims(&manifest, &claims).unwrap_err();
        assert_eq!(error.code, "sync_object_gc_in_progress");

        let unrelated_claim = BTreeMap::from([(
            "profiles/other-resource/2/checksum/profile/state.json".into(),
            BTreeSet::from(["journal-1".into()]),
        )]);
        validate_manifest_gc_claims(&manifest, &unrelated_claim).unwrap();
    }

    #[tokio::test]
    async fn suspended_account_cannot_reuse_browser_session_or_device_authorization_or_refresh() {
        use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
        use buglogin_domain::{account::AccountStatus, ids::AccountId};

        let root = test_root("suspended-web-login");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let state = ApiState::new(ProfileService::new(LocalRepository::open(&root).unwrap()))
            .with_web_origin("http://127.0.0.1:3000")
            .unwrap();
        let account_id = AccountId::new("acct_suspended_login").unwrap();
        state
            .provision_web_identity(
                account_id.as_str(),
                "suspended.owner",
                "suspended.owner@example.com",
                "correct horse battery staple",
            )
            .unwrap();
        let issued_session = state
            .auth
            .write()
            .unwrap()
            .issue_browser(account_id.clone(), now().saturating_add(3_600), now())
            .unwrap();
        let device_authorization = state
            .device_authorization
            .start("device-suspended-account", "BugLogin desktop", now())
            .unwrap();
        state
            .device_authorization
            .approve_with_credential_version(
                &device_authorization.user_code,
                account_id.clone(),
                Some(1),
                now(),
            )
            .unwrap();
        let mut device_tokens = state
            .auth
            .write()
            .unwrap()
            .issue_device_session(
                account_id.clone(),
                now().saturating_add(3_600),
                now().saturating_add(86_400),
                "device-suspended-refresh".into(),
                Some(1),
            )
            .unwrap();
        let (mut account, revision) = state
            .account_persistence
            .load_versioned(&account_id)
            .unwrap()
            .unwrap();
        account = account
            .transition_status(revision, AccountStatus::Suspended, "2026-10-18T00:00:01Z")
            .unwrap();
        let account_put = state
            .account_persistence
            .prepare_import_put(&account, revision)
            .unwrap();
        state
            .control_persistence
            .execute(ExecuteControlCommand {
                tenant_id: "control".into(),
                command_id: "account.status.test".into(),
                idempotency_key: "account-status-suspended-login-test".into(),
                expected_revisions: BTreeMap::new(),
                puts: vec![account_put],
                deletes: vec![],
                audit: ControlAuditEntry {
                    tenant_id: "control".into(),
                    command_id: "account.status.test".into(),
                    actor_id: "test".into(),
                    allowed: true,
                    detail: serde_json::json!({"accountId": account_id}),
                },
                response: buglogin_storage::control::HttpResponse {
                    status: 200,
                    headers: BTreeMap::new(),
                    body: serde_json::Value::Null,
                },
            })
            .unwrap();

        let mut session_headers = HeaderMap::new();
        session_headers.insert(
            header::COOKIE,
            HeaderValue::from_str(&format!(
                "{WEB_SESSION_COOKIE}={}; {WEB_CSRF_COOKIE}={}",
                issued_session.raw_session, issued_session.raw_csrf
            ))
            .unwrap(),
        );
        session_headers.insert(
            "x-correlation-id",
            HeaderValue::from_static("suspended-web-session-test"),
        );
        let session_error =
            super::web_session(axum::extract::State(state.clone()), session_headers)
                .await
                .unwrap_err();
        assert_eq!(session_error.status(), StatusCode::FORBIDDEN);

        let device_error = super::exchange_device_authorization(
            axum::extract::State(state.clone()),
            HeaderMap::from_iter([(
                "x-correlation-id".parse().unwrap(),
                HeaderValue::from_static("suspended-device-exchange-test"),
            )]),
            axum::Json(DeviceAuthorizationTokenInput {
                device_code: device_authorization.device_code,
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(device_error.status(), StatusCode::FORBIDDEN);

        let refresh_error = super::refresh_device_session(
            axum::extract::State(state.clone()),
            HeaderMap::from_iter([(
                "x-correlation-id".parse().unwrap(),
                HeaderValue::from_static("suspended-device-refresh-test"),
            )]),
            axum::Json(DeviceRefreshInput {
                refresh_token: std::mem::take(&mut device_tokens.refresh_token),
                device_id: "device-suspended-refresh".into(),
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(refresh_error.status(), StatusCode::FORBIDDEN);

        let mut headers = HeaderMap::new();
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("http://127.0.0.1:3000"),
        );
        headers.insert(
            "x-correlation-id",
            HeaderValue::from_static("suspended-web-login-test"),
        );
        let error = super::web_login(
            axum::extract::State(state),
            headers,
            axum::Json(WebLoginInput {
                identifier: "suspended.owner".into(),
                password: "correct horse battery staple".into(),
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(error.status(), StatusCode::FORBIDDEN);
        assert!(!error.headers().contains_key(header::SET_COOKIE));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn committed_blob_references_are_scoped_to_workspace_and_resource() {
        use buglogin_domain::ids::{ResourceId, WorkspaceId};
        use buglogin_sync::manifest::{ManifestObject, SyncManifest};

        let workspace_a = WorkspaceId::new("ws_blob_scope_a").unwrap();
        let workspace_b = WorkspaceId::new("ws_blob_scope_b").unwrap();
        let resource_id = ResourceId::new("res_blob_scope").unwrap();
        let object = ManifestObject::new("profile/state.json", b"same object").unwrap();
        let manifest_a = SyncManifest::new(
            workspace_a.clone(),
            resource_id.clone(),
            1,
            1,
            None,
            b"root-a",
            vec![object.clone()],
        )
        .unwrap();
        let mut aggregate = ControlAggregate::default();
        aggregate.sync_manifests.insert(
            sync_manifest_key(&workspace_a, &resource_id),
            manifest_a.clone(),
        );
        let storage_key = sync_manifest_object_storage_key(&manifest_a, &object);

        assert!(
            manifest_storage_references_for_workspace(&aggregate, workspace_a.as_str())
                .contains(&storage_key)
        );
        assert!(
            !manifest_storage_references_for_workspace(&aggregate, workspace_b.as_str())
                .contains(&storage_key)
        );
        assert!(
            !manifest_storage_references_for_resource(
                &aggregate,
                workspace_b.as_str(),
                resource_id.as_str(),
            )
            .contains(&storage_key)
        );
    }

    #[test]
    fn sync_gc_claim_revision_cas_rejects_a_stale_manifest_writer() {
        use buglogin_domain::ids::{ResourceId, WorkspaceId};
        use buglogin_sync::manifest::{ManifestObject, SyncManifest};

        let root = test_root("sync-gc-manifest-cas");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let store = MemoryControlRecordStore::default();
        let state = ApiState::try_with_account_store(
            ProfileService::new(LocalRepository::open(&root).unwrap()),
            store.clone(),
            false,
        )
        .unwrap();
        let workspace_id = WorkspaceId::new("ws_gc_cas").unwrap();
        let resource_id = ResourceId::new("res_gc_cas").unwrap();
        let bytes = b"immutable profile object";
        let object = ManifestObject::new("profile/state.json", bytes).unwrap();
        let manifest = SyncManifest::new(
            workspace_id.clone(),
            resource_id.clone(),
            7,
            1,
            None,
            b"root",
            vec![object.clone()],
        )
        .unwrap();
        let storage_key = sync_manifest_object_storage_key(&manifest, &object);
        let journal_id = "msi_gc_cas";
        let journal = SyncMigrationJournal {
            schema: "buglogin.migration-sync-journal/1".into(),
            operation_id: "op_gc_cas".into(),
            export_id: "export_gc_cas".into(),
            export_sha256: "a".repeat(64),
            manifest_sha256: "b".repeat(64),
            blob_set_sha256: "c".repeat(64),
            workspace_id: workspace_id.as_str().into(),
            resource_id: resource_id.as_str().into(),
            base_generation: None,
            target_generation: 1,
            state: SyncMigrationJournalState::CleanupPending,
            created_objects: vec![SyncMigrationJournalObject {
                storage_key: storage_key.clone(),
                checksum: object.checksum.clone(),
                size: object.size,
            }],
            resumed_objects: vec![],
            reserved_objects: vec![],
            destination_fencing_token: Some(7),
            control_revision_before: state.control_revision().unwrap(),
            control_revision_after: None,
            updated_at_ms: 1,
            error_code: Some("migration_recovery_cleanup_pending".into()),
        };
        state
            .persist_sync_migration_journal(
                journal_id,
                0,
                "sync-gc-manifest-cas-journal-seed".into(),
                "acct_gc_cas".into(),
                &journal,
            )
            .unwrap();

        // This second API instance holds the same aggregate revision the writer would use
        // when it began validating its manifest, before the GC claim wins the CAS.
        let stale_writer = ApiState::try_with_account_store(
            ProfileService::new(LocalRepository::open(&root).unwrap()),
            store.reopen(),
            false,
        )
        .unwrap();
        assert!(
            state
                .claim_sync_gc_objects(
                    journal_id,
                    &[SyncMigrationJournalObject {
                        storage_key: storage_key.clone(),
                        checksum: object.checksum.clone(),
                        size: object.size,
                    }],
                )
                .unwrap()
        );

        stale_writer.replace_sync_manifest(manifest).unwrap();
        assert!(
            stale_writer
                .persist_control_aggregate(
                    "profile.manifest.commit",
                    "sync-gc-stale-manifest-writer".into(),
                    "acct_gc_cas".into(),
                    true,
                    "gc-race-manifest-input",
                    serde_json::json!({"resourceId": resource_id}),
                    200,
                    serde_json::json!({"committed": true}),
                )
                .is_err()
        );

        let current = ApiState::try_with_account_store(
            ProfileService::new(LocalRepository::open(&root).unwrap()),
            store.reopen(),
            false,
        )
        .unwrap();
        assert!(
            current
                .control_aggregate()
                .unwrap()
                .sync_manifests
                .is_empty()
        );
        assert!(
            current
                .sync_gc_claims
                .read()
                .unwrap()
                .get(&storage_key)
                .is_some_and(|owners| owners.contains(journal_id))
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn sync_gc_claim_preserves_objects_reserved_by_an_active_import_journal() {
        use buglogin_domain::ids::{ResourceId, WorkspaceId};

        let root = test_root("sync-gc-active-reservation-journal");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let state = ApiState::try_with_account_store(
            ProfileService::new(LocalRepository::open(&root).unwrap()),
            MemoryControlRecordStore::default(),
            false,
        )
        .unwrap();
        let workspace_id = WorkspaceId::new("ws_gc_reservation").unwrap();
        let resource_id = ResourceId::new("res_gc_reservation").unwrap();
        let object = SyncMigrationJournalObject {
            storage_key: "profiles/res_gc_reservation/8/abc/profile/state.json".into(),
            checksum: "abc".into(),
            size: 12,
        };
        let journal = |operation_id: &str,
                       export_id: &str,
                       state: SyncMigrationJournalState,
                       created_objects: Vec<SyncMigrationJournalObject>,
                       reserved_objects: Vec<SyncMigrationJournalObject>| {
            SyncMigrationJournal {
                schema: "buglogin.migration-sync-journal/1".into(),
                operation_id: operation_id.into(),
                export_id: export_id.into(),
                export_sha256: "a".repeat(64),
                manifest_sha256: "b".repeat(64),
                blob_set_sha256: "c".repeat(64),
                workspace_id: workspace_id.as_str().into(),
                resource_id: resource_id.as_str().into(),
                base_generation: None,
                target_generation: 8,
                state,
                created_objects,
                resumed_objects: vec![],
                reserved_objects,
                destination_fencing_token: None,
                control_revision_before: 0,
                control_revision_after: None,
                updated_at_ms: 1,
                error_code: None,
            }
        };
        let cleanup = journal(
            "op_gc_reservation_cleanup",
            "export_gc_reservation_cleanup",
            SyncMigrationJournalState::CleanupPending,
            vec![object.clone()],
            vec![],
        );
        let active = journal(
            "op_gc_reservation_active",
            "export_gc_reservation_active",
            SyncMigrationJournalState::Staging,
            vec![],
            vec![object.clone()],
        );
        state
            .persist_sync_migration_journal(
                "msi_gc_reservation_cleanup",
                0,
                "sync-gc-reservation-cleanup-seed".into(),
                "acct_gc_reservation".into(),
                &cleanup,
            )
            .unwrap();
        state
            .persist_sync_migration_journal(
                "msi_gc_reservation_active",
                0,
                "sync-gc-reservation-active-seed".into(),
                "acct_gc_reservation".into(),
                &active,
            )
            .unwrap();

        let mut aggregate = state.control_aggregate().unwrap();
        aggregate.sync_blob_reservations.insert(
            object.storage_key.clone(),
            SyncBlobReservation {
                workspace_id: workspace_id.as_str().into(),
                resource_id: resource_id.as_str().into(),
                storage_key: object.storage_key.clone(),
                checksum: object.checksum.clone(),
                size: object.size,
                expires_at_ms: u64::MAX,
            },
        );
        state.replace_control_aggregate(aggregate).unwrap();
        state
            .persist_control_aggregate(
                "profile.blob.reserve",
                "sync-gc-reservation-release-seed".into(),
                "acct_gc_reservation".into(),
                true,
                "sync-gc-reservation-release-seed",
                serde_json::json!({"resourceId": resource_id}),
                200,
                serde_json::json!({"reserved": 1}),
            )
            .unwrap();
        state
            .release_sync_blob_reservations_for_import(
                "msi_gc_reservation_other",
                workspace_id.as_str(),
                resource_id.as_str(),
                std::slice::from_ref(&object),
                "acct_gc_reservation",
            )
            .unwrap();
        assert!(
            state
                .sync_blob_reservations
                .read()
                .unwrap()
                .contains_key(&object.storage_key)
        );

        let mut aggregate = state.control_aggregate().unwrap();
        aggregate.sync_blob_reservations.remove(&object.storage_key);
        state.replace_control_aggregate(aggregate).unwrap();
        state
            .persist_control_aggregate(
                "profile.blob.reservation.release",
                "sync-gc-reservation-release-test-reset".into(),
                "acct_gc_reservation".into(),
                true,
                "sync-gc-reservation-release-test-reset",
                serde_json::json!({"resourceId": resource_id}),
                200,
                serde_json::json!({"released": 1}),
            )
            .unwrap();

        assert!(
            !state
                .claim_sync_gc_objects("msi_gc_reservation_cleanup", &[object])
                .unwrap()
        );
        assert!(state.sync_gc_claims.read().unwrap().is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn active_manifest_commit_wins_before_gc_claim_refreshes() {
        use buglogin_domain::ids::{ResourceId, WorkspaceId};
        use buglogin_sync::manifest::{ManifestObject, SyncManifest};

        let root = test_root("sync-gc-active-manifest-wins");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let store = MemoryControlRecordStore::default();
        let writer = ApiState::try_with_account_store(
            ProfileService::new(LocalRepository::open(&root).unwrap()),
            store.clone(),
            false,
        )
        .unwrap();
        let workspace_id = WorkspaceId::new("ws_gc_active").unwrap();
        let resource_id = ResourceId::new("res_gc_active").unwrap();
        let bytes = b"already-active profile object";
        let object = ManifestObject::new("profile/state.json", bytes).unwrap();
        let manifest = SyncManifest::new(
            workspace_id.clone(),
            resource_id.clone(),
            8,
            1,
            None,
            b"root",
            vec![object.clone()],
        )
        .unwrap();
        let storage_key = sync_manifest_object_storage_key(&manifest, &object);
        let journal_id = "msi_gc_active";
        let cleanup_object = SyncMigrationJournalObject {
            storage_key: storage_key.clone(),
            checksum: object.checksum.clone(),
            size: object.size,
        };
        let journal = SyncMigrationJournal {
            schema: "buglogin.migration-sync-journal/1".into(),
            operation_id: "op_gc_active".into(),
            export_id: "export_gc_active".into(),
            export_sha256: "d".repeat(64),
            manifest_sha256: "e".repeat(64),
            blob_set_sha256: "f".repeat(64),
            workspace_id: workspace_id.as_str().into(),
            resource_id: resource_id.as_str().into(),
            base_generation: None,
            target_generation: 1,
            state: SyncMigrationJournalState::CleanupPending,
            created_objects: vec![cleanup_object.clone()],
            resumed_objects: vec![],
            reserved_objects: vec![],
            destination_fencing_token: Some(8),
            control_revision_before: writer.control_revision().unwrap(),
            control_revision_after: None,
            updated_at_ms: 1,
            error_code: Some("migration_recovery_cleanup_pending".into()),
        };
        writer
            .persist_sync_migration_journal(
                journal_id,
                0,
                "sync-gc-active-manifest-journal-seed".into(),
                "acct_gc_active".into(),
                &journal,
            )
            .unwrap();
        let cleaner = ApiState::try_with_account_store(
            ProfileService::new(LocalRepository::open(&root).unwrap()),
            store.reopen(),
            false,
        )
        .unwrap();

        writer.replace_sync_manifest(manifest).unwrap();
        writer
            .persist_control_aggregate(
                "profile.manifest.commit",
                "sync-gc-active-manifest-writer".into(),
                "acct_gc_active".into(),
                true,
                "active-manifest-input",
                serde_json::json!({"resourceId": resource_id}),
                200,
                serde_json::json!({"committed": true}),
            )
            .unwrap();
        assert!(
            !cleaner
                .claim_sync_gc_objects(journal_id, &[cleanup_object])
                .unwrap()
        );

        let current = ApiState::try_with_account_store(
            ProfileService::new(LocalRepository::open(&root).unwrap()),
            store.reopen(),
            false,
        )
        .unwrap();
        assert_eq!(current.control_aggregate().unwrap().sync_manifests.len(), 1);
        assert!(current.sync_gc_claims.read().unwrap().is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn control_advisory_lock_is_shared_and_released_between_store_handles() {
        let store = MemoryControlRecordStore::default();
        let mut first = store.clone();
        let mut second = store.reopen();
        assert!(first.try_advisory_lock(41).unwrap());
        assert!(!second.try_advisory_lock(41).unwrap());
        first.release_advisory_lock(41).unwrap();
        assert!(second.try_advisory_lock(41).unwrap());
        second.release_advisory_lock(41).unwrap();
    }

    #[test]
    fn local_profile_and_migration_body_dtos_reject_unknown_fields() {
        assert!(
            serde_json::from_value::<RevisionInput>(serde_json::json!({
                "revision": 1,
                "unexpected": true
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<LaunchInput>(serde_json::json!({
                "profilesRoot": "C:/profiles",
                "extraArgs": [],
                "unexpected": true
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<DeleteInput>(serde_json::json!({
                "revision": 1,
                "confirmation": "QUARANTINE_PROFILE_DATA",
                "unexpected": true
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<MigrationInspectInput>(serde_json::json!({
                "sourceRoot": "C:/source",
                "unexpected": true
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<MigrationImportInput>(serde_json::json!({
                "sourceRoot": "C:/source",
                "stagingParent": "C:/staging",
                "destinationRoot": "C:/destination",
                "runId": "run-1",
                "confirmation": "CUTOVER_AND_PROMOTE",
                "unexpected": true
            }))
            .is_err()
        );
    }

    #[test]
    fn operation_state_survives_api_state_restart() {
        let store = MemoryControlRecordStore::default();
        let first =
            ApiState::hosted_with_control_store(ProfileService::disabled(), store.clone()).unwrap();
        let operation_id = "op_restart_persisted";
        first
            .mutate_operation(
                operation_id,
                "operation.test",
                "started",
                "acct_operation_test",
                |operations| {
                    operations.insert(Operation::queued(
                        operation_id,
                        "operation.test",
                        ResourceScope::global(),
                    )?)?;
                    operations.transition(
                        operation_id,
                        OperationStatus::Running,
                        Some(0.1),
                        None,
                        None,
                    )?;
                    Ok(())
                },
            )
            .unwrap();

        let restarted =
            ApiState::hosted_with_control_store(ProfileService::disabled(), store.clone()).unwrap();
        let running = restarted
            .operations
            .read()
            .unwrap()
            .find(operation_id)
            .cloned()
            .expect("operation restored after restart");
        assert_eq!(running.status, OperationStatus::Running);

        restarted
            .mutate_operation(
                operation_id,
                "operation.complete",
                "completed",
                "acct_operation_test",
                |operations| {
                    operations.transition(
                        operation_id,
                        OperationStatus::Completed,
                        Some(1.0),
                        Some(serde_json::json!({ "ok": true })),
                        None,
                    )?;
                    Ok(())
                },
            )
            .unwrap();
        let after_completion_restart =
            ApiState::hosted_with_control_store(ProfileService::disabled(), store).unwrap();
        let completed = after_completion_restart
            .operations
            .read()
            .unwrap()
            .find(operation_id)
            .cloned()
            .expect("completed operation restored after restart");
        assert_eq!(completed.status, OperationStatus::Completed);
        assert_eq!(completed.result, Some(serde_json::json!({ "ok": true })));

        first.refresh_control_aggregate().unwrap();
        assert_eq!(
            first
                .operations
                .read()
                .unwrap()
                .find(operation_id)
                .unwrap()
                .status,
            OperationStatus::Completed
        );
    }

    #[tokio::test]
    async fn coordination_event_read_uses_workspace_grant_and_publish_is_service_only() {
        use buglogin_domain::{
            ids::{AccountId, GrantId, WorkspaceId},
            permissions::{Action, Capability, Effect, Grant, ResourceScope},
        };
        use buglogin_sync::coordination::{
            AuthorizationRequest, CoordinationAuth, CoordinationOperation,
        };

        let state = ApiState::local(ProfileService::disabled());
        let account_id = AccountId::new("acct_event_reader").unwrap();
        let workspace_id = WorkspaceId::new("ws_event_reader").unwrap();
        let identity = AccountIdentity::new(
            account_id.clone(),
            "event-reader",
            "event-reader@example.com",
            "100",
        )
        .unwrap();
        state.identity_persistence.create(&identity).unwrap();
        state
            .credentials
            .set_initial_password(&account_id, "correct horse battery staple", now())
            .unwrap();
        let credential_version = state
            .credentials
            .password_version(&account_id)
            .unwrap()
            .unwrap();
        let bearer = state.issue_token(account_id.as_str()).unwrap();
        state.grants.write().unwrap().push(Grant {
            id: GrantId::new("grant_event_read").unwrap(),
            account_id: Some(account_id.clone()),
            member_id: None,
            capability: Capability::new("events.read").unwrap(),
            action: Action::Read,
            scope: ResourceScope::workspace(workspace_id.clone()),
            effect: Effect::Allow,
        });
        let auth = CoordinationAuth::new(account_id.as_str(), bearer).unwrap();
        let authorizer = ApiCoordinationAuthorizer {
            authority: state.authorization_authority(),
        };
        authorizer
            .authorize(
                &auth,
                AuthorizationRequest {
                    operation: CoordinationOperation::EventRead,
                    workspace_id: &workspace_id,
                    resource_id: None,
                },
            )
            .await
            .unwrap();
        let now = now();
        let missing_scope_token = state
            .api_tokens
            .issue(
                account_id.clone(),
                credential_version,
                "event-reader-without-event-scope",
                std::collections::BTreeSet::from(["profile.read:read".to_owned()]),
                now + 3_600,
                now,
            )
            .unwrap();
        let missing_scope_auth =
            CoordinationAuth::new(account_id.as_str(), missing_scope_token.token).unwrap();
        assert_eq!(
            authorizer
                .authorize(
                    &missing_scope_auth,
                    AuthorizationRequest {
                        operation: CoordinationOperation::EventRead,
                        workspace_id: &workspace_id,
                        resource_id: None,
                    },
                )
                .await
                .unwrap_err()
                .code,
            "coordination_forbidden"
        );
        let event_scope_token = state
            .api_tokens
            .issue(
                account_id.clone(),
                credential_version,
                "event-reader-with-event-scope",
                std::collections::BTreeSet::from(["events.read:read".to_owned()]),
                now + 3_600,
                now,
            )
            .unwrap();
        let event_scope_auth =
            CoordinationAuth::new(account_id.as_str(), event_scope_token.token).unwrap();
        authorizer
            .authorize(
                &event_scope_auth,
                AuthorizationRequest {
                    operation: CoordinationOperation::EventRead,
                    workspace_id: &workspace_id,
                    resource_id: None,
                },
            )
            .await
            .unwrap();
        let other_workspace_id = WorkspaceId::new("ws_event_other").unwrap();
        assert_eq!(
            authorizer
                .authorize(
                    &event_scope_auth,
                    AuthorizationRequest {
                        operation: CoordinationOperation::EventRead,
                        workspace_id: &other_workspace_id,
                        resource_id: None,
                    },
                )
                .await
                .unwrap_err()
                .code,
            "coordination_forbidden"
        );
        assert_eq!(
            authorizer
                .authorize(
                    &auth,
                    AuthorizationRequest {
                        operation: CoordinationOperation::EventPublish,
                        workspace_id: &workspace_id,
                        resource_id: None,
                    },
                )
                .await
                .unwrap_err()
                .code,
            "coordination_publish_forbidden"
        );
    }

    #[test]
    fn coordination_actor_must_be_account_or_bound_desktop_device() {
        use buglogin_domain::ids::AccountId;

        let state = ApiState::local(ProfileService::disabled());
        let account_id = AccountId::new("acct_coord_actor").unwrap();
        let account_token = state.issue_token(account_id.as_str()).unwrap();
        let device_id = "device_coord_actor";
        let device_token = state
            .auth
            .write()
            .unwrap()
            .issue_device_session(
                account_id.clone(),
                now() + 3_600,
                now() + 30 * 24 * 60 * 60,
                device_id.to_owned(),
                None,
            )
            .map(|mut tokens| std::mem::take(&mut tokens.access_token))
            .unwrap();
        let authority = state.authorization_authority();
        let device_principal = authority.authenticate_token(&device_token, now()).unwrap();

        assert!(
            authority
                .coordination_actor_matches(&device_token, &device_principal, device_id, now(),)
                .unwrap()
        );
        assert!(
            !authority
                .coordination_actor_matches(
                    &device_token,
                    &device_principal,
                    "device_spoofed",
                    now(),
                )
                .unwrap()
        );

        let account_principal = authority.authenticate_token(&account_token, now()).unwrap();
        assert!(
            authority
                .coordination_actor_matches(
                    &account_token,
                    &account_principal,
                    account_id.as_str(),
                    now(),
                )
                .unwrap()
        );
    }

    fn test_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "buglogin-api-persistence-{label}-{}",
            std::process::id()
        ))
    }

    struct DeleteFailStore {
        inner: MemoryRemoteChunkStore,
    }

    struct OverlapProbeStore {
        inner: MemoryRemoteChunkStore,
        active: Arc<AtomicUsize>,
        maximum: Arc<AtomicUsize>,
    }

    impl ConcurrentRemoteChunkStore for OverlapProbeStore {
        fn readiness(&self) -> Result<(), Problem> {
            ConcurrentRemoteChunkStore::readiness(&self.inner)
        }

        fn checksum(&self, object_key: &str) -> Result<Option<String>, Problem> {
            ConcurrentRemoteChunkStore::checksum(&self.inner, object_key)
        }

        fn put(
            &self,
            object_key: &str,
            bytes: &[u8],
            expected_checksum: &str,
        ) -> Result<(), Problem> {
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.maximum.fetch_max(active, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(80));
            let result =
                ConcurrentRemoteChunkStore::put(&self.inner, object_key, bytes, expected_checksum);
            self.active.fetch_sub(1, Ordering::SeqCst);
            result
        }

        fn get(&self, object_key: &str, expected_checksum: &str) -> Result<RemoteChunk, Problem> {
            ConcurrentRemoteChunkStore::get(&self.inner, object_key, expected_checksum)
        }

        fn delete_if_checksum(
            &self,
            object_key: &str,
            expected_checksum: &str,
        ) -> Result<bool, Problem> {
            ConcurrentRemoteChunkStore::delete_if_checksum(
                &self.inner,
                object_key,
                expected_checksum,
            )
        }
    }

    impl RemoteChunkStore for DeleteFailStore {
        fn readiness(&self) -> Result<(), Problem> {
            RemoteChunkStore::readiness(&self.inner)
        }

        fn checksum(&self, object_key: &str) -> Result<Option<String>, Problem> {
            RemoteChunkStore::checksum(&self.inner, object_key)
        }

        fn put(
            &mut self,
            object_key: &str,
            bytes: &[u8],
            expected_checksum: &str,
        ) -> Result<(), Problem> {
            RemoteChunkStore::put(&mut self.inner, object_key, bytes, expected_checksum)
        }

        fn get(&self, object_key: &str, expected_checksum: &str) -> Result<RemoteChunk, Problem> {
            RemoteChunkStore::get(&self.inner, object_key, expected_checksum)
        }

        fn delete_if_checksum(
            &mut self,
            _object_key: &str,
            _expected_checksum: &str,
        ) -> Result<bool, Problem> {
            Err(Problem::new(
                "test_remote_delete_failed",
                "Remote delete failed",
                503,
                "test",
            ))
        }
    }

    #[test]
    fn concurrent_storage_adapter_does_not_serialize_distinct_blob_puts() {
        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let state = ApiState::local(ProfileService::disabled()).with_concurrent_remote_chunks(
            Box::new(OverlapProbeStore {
                inner: MemoryRemoteChunkStore::default(),
                active,
                maximum: Arc::clone(&maximum),
            }),
        );
        let workers: Vec<_> = (0..2)
            .map(|index| {
                let state = state.clone();
                std::thread::spawn(move || {
                    let key = format!("profiles/res_parallel/1/checksum/object-{index}");
                    let bytes = format!("blob-{index}").into_bytes();
                    state
                        .put_remote_chunk(&key, &bytes, &checksum(&bytes))
                        .unwrap();
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
        assert_eq!(maximum.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn stale_finalizing_sync_journal_recovers_after_restart_idempotently() {
        let first_root = test_root("recovery-first");
        let second_root = test_root("recovery-second");
        let _ = fs::remove_dir_all(&first_root);
        let _ = fs::remove_dir_all(&second_root);
        fs::create_dir_all(&first_root).unwrap();
        fs::create_dir_all(&second_root).unwrap();
        let store = MemoryControlRecordStore::default();
        let first = ApiState::try_with_account_store(
            ProfileService::new(LocalRepository::open(&first_root).unwrap()),
            store.clone(),
            false,
        )
        .unwrap();
        let object_key = "profiles/res_recovery/1/checksum/profile-state";
        let object_bytes = b"stale-profile-state";
        let object_checksum = checksum(object_bytes);
        let journal_id = "msi_recovery_restart";
        let journal = SyncMigrationJournal {
            schema: "buglogin.migration-sync-journal/1".into(),
            operation_id: "op_recovery_restart".into(),
            export_id: "export_recovery_restart".into(),
            export_sha256: "a".repeat(64),
            manifest_sha256: "b".repeat(64),
            blob_set_sha256: "c".repeat(64),
            workspace_id: "ws_recovery".into(),
            resource_id: "res_recovery".into(),
            base_generation: None,
            target_generation: 1,
            state: SyncMigrationJournalState::Finalizing,
            created_objects: vec![SyncMigrationJournalObject {
                storage_key: object_key.into(),
                checksum: object_checksum.clone(),
                size: object_bytes.len() as u64,
            }],
            resumed_objects: vec![],
            reserved_objects: vec![],
            destination_fencing_token: Some(9),
            control_revision_before: first.control_revision().unwrap(),
            control_revision_after: None,
            updated_at_ms: 1,
            error_code: None,
        };
        first
            .persist_sync_migration_journal(
                journal_id,
                0,
                "recovery-journal-seed-0001".into(),
                "acct_recovery".into(),
                &journal,
            )
            .unwrap();
        drop(first);

        let remote = MemoryRemoteChunkStore::default();
        remote
            .put(object_key, object_bytes, &object_checksum)
            .unwrap();
        let second = ApiState::try_with_account_store(
            ProfileService::new(LocalRepository::open(&second_root).unwrap()),
            store.reopen(),
            false,
        )
        .unwrap()
        .with_remote_chunks(Box::new(remote));
        let report = second.recover_stale_sync_migrations(u64::MAX).unwrap();
        assert_eq!(report.scanned, 1);
        assert_eq!(report.recovered, 1);
        assert_eq!(report.deferred, 0);
        assert_eq!(report.failed, 0);
        assert_eq!(
            second
                .get_remote_chunk(object_key, &object_checksum)
                .unwrap(),
            object_bytes
        );
        let (pending, _) = second
            .load_sync_migration_journal(journal_id)
            .unwrap()
            .unwrap();
        assert_eq!(pending.state, SyncMigrationJournalState::CleanupPending);
        assert_eq!(
            pending.error_code.as_deref(),
            Some("migration_recovery_cleanup_pending")
        );
        let cleanup = second.recover_stale_sync_migrations(u64::MAX).unwrap();
        assert_eq!(cleanup.scanned, 1);
        assert_eq!(cleanup.recovered, 1);
        assert_eq!(cleanup.failed, 0);
        assert_eq!(
            second
                .get_remote_chunk(object_key, &object_checksum)
                .unwrap_err()
                .code,
            "remote_chunk_not_found"
        );
        let (recovered, _) = second
            .load_sync_migration_journal(journal_id)
            .unwrap()
            .unwrap();
        assert_eq!(recovered.state, SyncMigrationJournalState::Abandoned);
        assert_eq!(
            recovered.error_code.as_deref(),
            Some("migration_recovery_abandoned")
        );
        let replay = second.recover_stale_sync_migrations(u64::MAX).unwrap();
        assert_eq!(replay.scanned, 1);
        assert_eq!(replay.recovered, 0);
        assert_eq!(replay.failed, 0);
        let _ = fs::remove_dir_all(first_root);
        let _ = fs::remove_dir_all(second_root);
    }

    #[test]
    fn stale_finalizing_journal_is_cleaned_when_manifest_reaches_target_elsewhere() {
        use buglogin_domain::ids::{ResourceId, WorkspaceId};
        use buglogin_sync::manifest::SyncManifest;

        let root = test_root("recovery-finalizing-superseded");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let store = MemoryControlRecordStore::default();
        let state = ApiState::try_with_account_store(
            ProfileService::new(LocalRepository::open(&root).unwrap()),
            store,
            false,
        )
        .unwrap();
        let workspace_id = WorkspaceId::new("ws_finalizing_superseded").unwrap();
        let resource_id = ResourceId::new("res_finalizing_superseded").unwrap();
        let manifest = SyncManifest::new(
            workspace_id.clone(),
            resource_id.clone(),
            10,
            1,
            None,
            b"other-import-manifest",
            vec![],
        )
        .unwrap();
        state.replace_sync_manifest(manifest).unwrap();
        state
            .persist_control_aggregate(
                "profile.manifest.commit",
                "recovery-finalizing-superseded-manifest".into(),
                "acct_recovery_finalizing".into(),
                true,
                "superseding-manifest",
                serde_json::json!({"resourceId": resource_id}),
                200,
                serde_json::json!({"committed": true}),
            )
            .unwrap();

        let journal_id = "msi_finalizing_superseded";
        let journal = SyncMigrationJournal {
            schema: "buglogin.migration-sync-journal/1".into(),
            operation_id: "op_finalizing_superseded".into(),
            export_id: "export_finalizing_superseded".into(),
            export_sha256: "a".repeat(64),
            manifest_sha256: "b".repeat(64),
            blob_set_sha256: "c".repeat(64),
            workspace_id: workspace_id.as_str().into(),
            resource_id: resource_id.as_str().into(),
            base_generation: None,
            target_generation: 1,
            state: SyncMigrationJournalState::Finalizing,
            created_objects: vec![],
            resumed_objects: vec![],
            reserved_objects: vec![],
            destination_fencing_token: Some(9),
            control_revision_before: state.control_revision().unwrap(),
            control_revision_after: None,
            updated_at_ms: 1,
            error_code: None,
        };
        state
            .persist_sync_migration_journal(
                journal_id,
                0,
                "recovery-finalizing-superseded-journal".into(),
                "acct_recovery_finalizing".into(),
                &journal,
            )
            .unwrap();

        let marked = state.recover_stale_sync_migrations(u64::MAX).unwrap();
        assert_eq!(marked.recovered, 1);
        assert_eq!(marked.deferred, 0);
        let (cleanup_pending, _) = state
            .load_sync_migration_journal(journal_id)
            .unwrap()
            .unwrap();
        assert_eq!(
            cleanup_pending.state,
            SyncMigrationJournalState::CleanupPending
        );

        let cleaned = state.recover_stale_sync_migrations(u64::MAX).unwrap();
        assert_eq!(cleaned.recovered, 1);
        let (abandoned, _) = state
            .load_sync_migration_journal(journal_id)
            .unwrap()
            .unwrap();
        assert_eq!(abandoned.state, SyncMigrationJournalState::Abandoned);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn stale_staging_journal_waits_for_cleanup_retention_before_deleting_blobs() {
        let root = test_root("recovery-staging-retention");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let object_key = "profiles/res_staging_retention/1/checksum/profile-state";
        let object_bytes = b"retain-before-cleanup";
        let object_checksum = checksum(object_bytes);
        let remote = MemoryRemoteChunkStore::default();
        remote
            .put(object_key, object_bytes, &object_checksum)
            .unwrap();
        let state = ApiState::try_with_account_store(
            ProfileService::new(LocalRepository::open(&root).unwrap()),
            MemoryControlRecordStore::default(),
            false,
        )
        .unwrap()
        .with_remote_chunks(Box::new(remote));
        let journal_id = "msi_staging_retention";
        let journal = SyncMigrationJournal {
            schema: "buglogin.migration-sync-journal/1".into(),
            operation_id: "op_staging_retention".into(),
            export_id: "export_staging_retention".into(),
            export_sha256: "a".repeat(64),
            manifest_sha256: "b".repeat(64),
            blob_set_sha256: "c".repeat(64),
            workspace_id: "ws_staging_retention".into(),
            resource_id: "res_staging_retention".into(),
            base_generation: None,
            target_generation: 1,
            state: SyncMigrationJournalState::Staging,
            created_objects: vec![SyncMigrationJournalObject {
                storage_key: object_key.into(),
                checksum: object_checksum.clone(),
                size: object_bytes.len() as u64,
            }],
            resumed_objects: vec![],
            reserved_objects: vec![],
            destination_fencing_token: Some(3),
            control_revision_before: state.control_revision().unwrap(),
            control_revision_after: None,
            updated_at_ms: 1,
            error_code: None,
        };
        state
            .persist_sync_migration_journal(
                journal_id,
                0,
                "staging-retention-seed".into(),
                "acct_staging_retention".into(),
                &journal,
            )
            .unwrap();

        let active_writer = SyncMigrationJournalLock::acquire(&state, journal_id).unwrap();
        let deferred = state.recover_stale_sync_migrations(u64::MAX).unwrap();
        assert_eq!(deferred.recovered, 0);
        assert_eq!(deferred.deferred, 1);
        let (still_staging, _) = state
            .load_sync_migration_journal(journal_id)
            .unwrap()
            .unwrap();
        assert_eq!(still_staging.state, SyncMigrationJournalState::Staging);
        drop(active_writer);

        let marked = state.recover_stale_sync_migrations(u64::MAX).unwrap();
        assert_eq!(marked.recovered, 0);
        assert_eq!(marked.deferred, 1);
        let (cleanup_pending, _) = state
            .load_sync_migration_journal(journal_id)
            .unwrap()
            .unwrap();
        assert_eq!(
            cleanup_pending.state,
            SyncMigrationJournalState::CleanupPending
        );
        assert_eq!(
            state
                .get_remote_chunk(object_key, &object_checksum)
                .unwrap(),
            object_bytes
        );

        let within_retention = state
            .recover_stale_sync_migrations(cleanup_pending.updated_at_ms.saturating_sub(1))
            .unwrap();
        assert_eq!(within_retention.recovered, 0);
        assert_eq!(within_retention.deferred, 0);
        assert_eq!(
            state
                .get_remote_chunk(object_key, &object_checksum)
                .unwrap(),
            object_bytes
        );

        let cleaned = state.recover_stale_sync_migrations(u64::MAX).unwrap();
        assert_eq!(cleaned.recovered, 1);
        assert_eq!(
            state
                .get_remote_chunk(object_key, &object_checksum)
                .unwrap_err()
                .code,
            "remote_chunk_not_found"
        );
        let (abandoned, _) = state
            .load_sync_migration_journal(journal_id)
            .unwrap()
            .unwrap();
        assert_eq!(abandoned.state, SyncMigrationJournalState::Abandoned);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn recovery_delete_failure_preserves_cleanup_pending_journal_and_object() {
        let root = test_root("recovery-delete-failure");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let object_key = "profiles/res_recovery_failure/1/checksum/profile-state";
        let object_bytes = b"preserve-on-delete-failure";
        let object_checksum = checksum(object_bytes);
        let remote = MemoryRemoteChunkStore::default();
        remote
            .put(object_key, object_bytes, &object_checksum)
            .unwrap();
        let store = MemoryControlRecordStore::default();
        let state = ApiState::try_with_account_store(
            ProfileService::new(LocalRepository::open(&root).unwrap()),
            store.clone(),
            false,
        )
        .unwrap()
        .with_remote_chunks(Box::new(DeleteFailStore {
            inner: remote.clone(),
        }));
        let journal_id = "msi_recovery_delete_failure";
        let journal = SyncMigrationJournal {
            schema: "buglogin.migration-sync-journal/1".into(),
            operation_id: "op_recovery_delete_failure".into(),
            export_id: "export_recovery_delete_failure".into(),
            export_sha256: "d".repeat(64),
            manifest_sha256: "e".repeat(64),
            blob_set_sha256: "f".repeat(64),
            workspace_id: "ws_recovery_failure".into(),
            resource_id: "res_recovery_failure".into(),
            base_generation: None,
            target_generation: 1,
            state: SyncMigrationJournalState::Finalizing,
            created_objects: vec![SyncMigrationJournalObject {
                storage_key: object_key.into(),
                checksum: object_checksum.clone(),
                size: object_bytes.len() as u64,
            }],
            resumed_objects: vec![],
            reserved_objects: vec![],
            destination_fencing_token: Some(11),
            control_revision_before: state.control_revision().unwrap(),
            control_revision_after: None,
            updated_at_ms: 1,
            error_code: None,
        };
        state
            .persist_sync_migration_journal(
                journal_id,
                0,
                "recovery-delete-failure-seed".into(),
                "acct_recovery".into(),
                &journal,
            )
            .unwrap();

        let mark = state.recover_stale_sync_migrations(u64::MAX).unwrap();
        assert_eq!(mark.scanned, 1);
        assert_eq!(mark.recovered, 1);
        assert_eq!(mark.failed, 0);
        let report = state.recover_stale_sync_migrations(u64::MAX).unwrap();
        assert_eq!(report.scanned, 1);
        assert_eq!(report.recovered, 0);
        assert_eq!(report.failed, 1);
        assert_eq!(
            state
                .get_remote_chunk(object_key, &object_checksum)
                .unwrap(),
            object_bytes
        );
        let (preserved, _) = state
            .load_sync_migration_journal(journal_id)
            .unwrap()
            .unwrap();
        assert_eq!(preserved.state, SyncMigrationJournalState::CleanupPending);
        assert_eq!(
            preserved.error_code.as_deref(),
            Some("migration_recovery_cleanup_pending")
        );
        assert!(
            state
                .sync_gc_claims
                .read()
                .unwrap()
                .get(object_key)
                .is_some_and(|owners| owners.contains(journal_id))
        );

        let recovered_after_restart = ApiState::try_with_account_store(
            ProfileService::new(LocalRepository::open(&root).unwrap()),
            store.reopen(),
            false,
        )
        .unwrap()
        .with_remote_chunks(Box::new(remote));
        let retry = recovered_after_restart
            .recover_stale_sync_migrations(u64::MAX)
            .unwrap();
        assert_eq!(retry.scanned, 1);
        assert_eq!(retry.recovered, 1);
        assert_eq!(retry.failed, 0);
        assert_eq!(
            recovered_after_restart
                .get_remote_chunk(object_key, &object_checksum)
                .unwrap_err()
                .code,
            "remote_chunk_not_found"
        );
        assert!(
            recovered_after_restart
                .sync_gc_claims
                .read()
                .unwrap()
                .is_empty()
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn cleanup_pending_sync_journal_defers_referenced_object() {
        use buglogin_domain::ids::{ResourceId, WorkspaceId};
        use buglogin_sync::manifest::{ManifestObject, SyncManifest};

        let root = test_root("recovery-referenced-object");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let object_bytes = b"active-profile-state";
        let object_checksum = checksum(object_bytes);
        let object_key =
            format!("profiles/res_recovery_referenced/1/{object_checksum}/profile-state");
        let remote = MemoryRemoteChunkStore::default();
        remote
            .put(&object_key, object_bytes, &object_checksum)
            .unwrap();
        let state = ApiState::try_with_account_store(
            ProfileService::new(LocalRepository::open(&root).unwrap()),
            MemoryControlRecordStore::default(),
            false,
        )
        .unwrap()
        .with_remote_chunks(Box::new(remote));
        let workspace = WorkspaceId::new("ws_recovery_referenced").unwrap();
        let resource = ResourceId::new("res_recovery_referenced").unwrap();
        let manifest = SyncManifest::new(
            workspace.clone(),
            resource.clone(),
            13,
            1,
            None,
            b"active-root",
            vec![ManifestObject::new("profile-state", object_bytes).unwrap()],
        )
        .unwrap();
        state.sync_manifests.write().unwrap().insert(
            format!("{}:{}", workspace.as_str(), resource.as_str()),
            manifest,
        );
        let journal_id = "msi_recovery_referenced";
        let journal = SyncMigrationJournal {
            schema: "buglogin.migration-sync-journal/1".into(),
            operation_id: "op_recovery_referenced".into(),
            export_id: "export_recovery_referenced".into(),
            export_sha256: "1".repeat(64),
            manifest_sha256: "2".repeat(64),
            blob_set_sha256: "3".repeat(64),
            workspace_id: workspace.as_str().into(),
            resource_id: resource.as_str().into(),
            base_generation: None,
            target_generation: 1,
            state: SyncMigrationJournalState::CleanupPending,
            created_objects: vec![SyncMigrationJournalObject {
                storage_key: object_key.clone(),
                checksum: object_checksum.clone(),
                size: object_bytes.len() as u64,
            }],
            resumed_objects: vec![],
            reserved_objects: vec![],
            destination_fencing_token: Some(13),
            control_revision_before: state.control_revision().unwrap(),
            control_revision_after: None,
            updated_at_ms: 1,
            error_code: Some("migration_recovery_cleanup_pending".into()),
        };
        state
            .persist_sync_migration_journal(
                journal_id,
                0,
                "recovery-referenced-seed".into(),
                "acct_recovery".into(),
                &journal,
            )
            .unwrap();

        let report = state.recover_stale_sync_migrations(u64::MAX).unwrap();
        assert_eq!(report.scanned, 1);
        assert_eq!(report.deferred, 1);
        assert_eq!(report.recovered, 0);
        assert_eq!(report.failed, 0);
        assert_eq!(
            state
                .get_remote_chunk(&object_key, &object_checksum)
                .unwrap(),
            object_bytes
        );
        let (preserved, _) = state
            .load_sync_migration_journal(journal_id)
            .unwrap()
            .unwrap();
        assert_eq!(preserved.state, SyncMigrationJournalState::CleanupPending);
        let _ = fs::remove_dir_all(root);
    }
    #[test]
    fn hosted_profile_reads_project_runtime_state() {
        let state = ApiState::hosted_with_control_store(
            ProfileService::disabled(),
            MemoryControlRecordStore::default(),
        )
        .unwrap();
        let profile_id = ProfileId::new("prf_runtime_projection_read").unwrap();
        let created = state
            .create_profile_record(Profile {
                id: profile_id.clone(),
                name: "Runtime projection".into(),
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
                sync_mode: Some(buglogin_domain::entities::SyncMode::Managed),
                created_at: "2026-07-22T00:00:00Z".into(),
                updated_at: "2026-07-22T00:00:00Z".into(),
            })
            .unwrap();
        state
            .set_runtime_projection(RuntimeProjection {
                profile_id: profile_id.clone(),
                device_id: "device-runtime-projection".into(),
                owner_id: "device-runtime-projection".into(),
                state: "running".into(),
                generation: 1,
                root_checksum: "a".repeat(64),
                updated_at_ms: 1,
            })
            .unwrap();

        let listed = state.list_profile_records().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].revision, created.revision);
        assert_eq!(listed[0].value.state, ProfileState::Running);
        let loaded = state.get_profile_record(&profile_id).unwrap().unwrap();
        assert_eq!(loaded.revision, created.revision);
        assert_eq!(loaded.value.state, ProfileState::Running);

        state
            .set_runtime_projection(RuntimeProjection {
                profile_id: profile_id.clone(),
                device_id: "device-runtime-projection".into(),
                owner_id: "device-runtime-projection".into(),
                state: "stopped".into(),
                generation: 2,
                root_checksum: "b".repeat(64),
                updated_at_ms: 2,
            })
            .unwrap();
        assert_eq!(
            state
                .get_profile_record(&profile_id)
                .unwrap()
                .unwrap()
                .value
                .state,
            ProfileState::Ready
        );
    }
    #[test]
    fn hosted_profile_metadata_survives_store_reopen_without_local_repository_write() {
        let first_root = test_root("hosted-profile-first");
        let second_root = test_root("hosted-profile-second");
        let _ = fs::remove_dir_all(&first_root);
        let _ = fs::remove_dir_all(&second_root);
        fs::create_dir_all(&first_root).unwrap();
        fs::create_dir_all(&second_root).unwrap();
        let store = MemoryControlRecordStore::default();
        let first = ApiState::try_with_account_store(
            ProfileService::new(LocalRepository::open(&first_root).unwrap()),
            store.clone(),
            false,
        )
        .unwrap();
        let profile = Profile {
            id: ProfileId::new("prf_hosted_persisted").unwrap(),
            name: "Hosted persisted profile".into(),
            runtime: Runtime::Wayfern,
            runtime_version: None,
            runtime_config: None,
            state: buglogin_domain::entities::ProfileState::Ready,
            archived: false,
            fingerprint: None,
            note: None,
            proxy_id: None,
            vpn_id: None,
            group_ids: vec![],
            tags: vec![],
            extension_group_id: None,
            sync_mode: Some(buglogin_domain::entities::SyncMode::Managed),
            created_at: "2026-07-22T00:00:00Z".into(),
            updated_at: "2026-07-22T00:00:00Z".into(),
        };
        first.create_profile_record(profile.clone()).unwrap();
        first
            .persist_control_aggregate(
                "test.hosted-profile.persist",
                "hosted-profile-persist-0001".into(),
                "acct_test".into(),
                true,
                &"a".repeat(64),
                serde_json::json!({"profileId": profile.id}),
                200,
                serde_json::json!({"profileId": profile.id}),
            )
            .unwrap();
        assert!(first.profiles.list().unwrap().is_empty());

        let second = ApiState::try_with_account_store(
            ProfileService::new(LocalRepository::open(&second_root).unwrap()),
            store,
            false,
        )
        .unwrap();
        let loaded = second.get_profile_record(&profile.id).unwrap().unwrap();
        assert_eq!(loaded.revision, 1);
        assert_eq!(loaded.value, profile);
        assert!(second.profiles.list().unwrap().is_empty());
        let _ = fs::remove_dir_all(first_root);
        let _ = fs::remove_dir_all(second_root);
    }

    #[tokio::test]
    async fn hosted_state_rejects_local_filesystem_migration() {
        let profile_root = test_root("hosted-local-migration-guard");
        let _ = fs::remove_dir_all(&profile_root);
        fs::create_dir_all(&profile_root).unwrap();
        let state = ApiState::try_with_account_store(
            ProfileService::new(LocalRepository::open(&profile_root).unwrap()),
            MemoryControlRecordStore::default(),
            false,
        )
        .unwrap();

        let inspect_error = inspect_migration(
            State(state.clone()),
            HeaderMap::new(),
            Json(MigrationInspectInput {
                source_root: profile_root.clone(),
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(inspect_error.status(), StatusCode::CONFLICT);

        let import_error = import_migration(
            State(state),
            HeaderMap::new(),
            Json(MigrationImportInput {
                source_root: profile_root.clone(),
                staging_parent: profile_root.clone(),
                destination_root: profile_root.clone(),
                run_id: "run_hosted_guard".into(),
                confirmation: "MIGRATE_COPY_ONLY".into(),
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(import_error.status(), StatusCode::CONFLICT);
        let _ = fs::remove_dir_all(profile_root);
    }

    #[derive(Clone)]
    struct AmbiguousCompleteStore {
        inner: MemoryControlRecordStore,
        fail_complete_once: Arc<AtomicBool>,
    }

    impl AmbiguousCompleteStore {
        fn new(inner: MemoryControlRecordStore) -> Self {
            Self {
                inner,
                fail_complete_once: Arc::new(AtomicBool::new(true)),
            }
        }
    }

    impl ControlRecordStore for AmbiguousCompleteStore {
        fn readiness(&mut self) -> Result<(), Problem> {
            self.inner.readiness()
        }

        fn get(
            &mut self,
            tenant_id: &str,
            record_type: &str,
            record_id: &str,
        ) -> Result<Option<ControlRecord>, Problem> {
            self.inner.get(tenant_id, record_type, record_id)
        }

        fn list(
            &mut self,
            tenant_id: &str,
            record_type: &str,
        ) -> Result<Vec<ControlRecord>, Problem> {
            self.inner.list(tenant_id, record_type)
        }

        fn replay(
            &mut self,
            tenant_id: &str,
            idempotency_key: &str,
        ) -> Result<Option<ControlCommandResult>, Problem> {
            self.inner.replay(tenant_id, idempotency_key)
        }

        fn abandon_idempotency(
            &mut self,
            claim: &ControlIdempotencyClaimToken,
        ) -> Result<(), Problem> {
            self.inner.abandon_idempotency(claim)
        }

        fn try_advisory_lock(&mut self, lock_id: i64) -> Result<bool, Problem> {
            self.inner.try_advisory_lock(lock_id)
        }

        fn release_advisory_lock(&mut self, lock_id: i64) -> Result<(), Problem> {
            self.inner.release_advisory_lock(lock_id)
        }

        fn claim_idempotency(
            &mut self,
            command: ClaimControlCommand,
        ) -> Result<ControlIdempotencyClaim, Problem> {
            self.inner.claim_idempotency(command)
        }

        fn complete_idempotency(
            &mut self,
            command: CompleteControlCommand,
        ) -> Result<ControlCommandResult, Problem> {
            let result = self.inner.complete_idempotency(command)?;
            if self.fail_complete_once.swap(false, Ordering::SeqCst) {
                return Err(Problem::new(
                    "control_persistence_unavailable",
                    "Control persistence is unavailable",
                    503,
                    "test",
                )
                .retryable(true));
            }
            Ok(result)
        }

        fn execute(
            &mut self,
            command: ExecuteControlCommand,
        ) -> Result<ControlCommandResult, Problem> {
            self.inner.execute(command)
        }

        fn audit(&mut self, tenant_id: &str) -> Result<Vec<ControlAuditEntry>, Problem> {
            self.inner.audit(tenant_id)
        }
    }

    #[test]
    fn control_state_and_replay_survive_store_reopen() {
        let profile_root = test_root("first");
        let second_root = test_root("second");
        let _ = fs::remove_dir_all(&profile_root);
        let _ = fs::remove_dir_all(&second_root);
        fs::create_dir_all(&profile_root).unwrap();
        fs::create_dir_all(&second_root).unwrap();
        let store = MemoryControlRecordStore::default();
        let first = ApiState::try_with_account_store(
            ProfileService::new(LocalRepository::open(&profile_root).unwrap()),
            store.clone(),
            true,
        )
        .unwrap();
        first
            .persist_control_aggregate(
                "workspace.create",
                "restart-replay-key-0001".into(),
                "acct_restart".into(),
                true,
                "input-hash-0001",
                serde_json::json!({"test": true}),
                200,
                serde_json::json!({"workspaceId": "ws_restart"}),
            )
            .unwrap();

        let second = ApiState::try_with_account_store(
            ProfileService::new(LocalRepository::open(&second_root).unwrap()),
            store.reopen(),
            true,
        )
        .unwrap();
        let replay = second
            .replay_control_response("restart-replay-key-0001", "input-hash-0001")
            .unwrap()
            .unwrap();
        assert_eq!(replay.body["workspaceId"], "ws_restart");
        assert_eq!(
            second
                .replay_control_response("restart-replay-key-0001", "different-input")
                .unwrap_err()
                .code,
            "idempotency_conflict"
        );
        let _ = fs::remove_dir_all(profile_root);
        let _ = fs::remove_dir_all(second_root);
    }

    #[tokio::test]
    async fn profile_key_envelope_is_available_to_group_reader_on_bound_device_only() {
        use crate::services::groups::GroupRecord;
        use axum::{body::Body, http::Request};
        use buglogin_domain::{
            grant::GroupShareGrant,
            member::{Member, MemberStatus},
            permissions::ResourceScope,
            resource::{Resource, ResourceKind, ResourceState},
        };
        use buglogin_sync::manifest::{
            ManifestObject, ManifestObjectReference, ManifestPayloadEncryption,
            PayloadEncryptionAlgorithm,
        };
        use tower::ServiceExt;

        let workspace_id = WorkspaceId::new("ws_group_envelope_http").unwrap();
        let profile_id = ProfileId::new("prf_group_envelope_http").unwrap();
        let resource_id = buglogin_domain::ids::ResourceId::new("res_group_envelope_http").unwrap();
        let group_id = GroupId::new("grp_group_envelope_http").unwrap();
        let owner_id = AccountId::new("acct_group_envelope_owner").unwrap();
        let recipient_id = AccountId::new("acct_group_envelope_member").unwrap();
        let member_id = MemberId::new("mem_group_envelope_member").unwrap();
        let control_store = MemoryControlRecordStore::default();
        let state =
            ApiState::hosted_with_control_store(ProfileService::disabled(), control_store.clone())
                .unwrap()
                .with_profile_resource(ProfileResource {
                    profile_id: profile_id.clone(),
                    resource_id: resource_id.clone(),
                    workspace_id: workspace_id.clone(),
                    archive_state: ProfileArchiveState::Active,
                });
        state
            .account_persistence
            .provision_default(recipient_id.clone(), "2026-10-14T00:00:00Z".into())
            .unwrap();
        let owner = Principal {
            account_id: Some(owner_id.clone()),
            workspace_ids: [workspace_id.clone()].into_iter().collect(),
            roles: [Role::WorkspaceAdmin].into_iter().collect(),
            ..Principal::default()
        };
        let platform_admin = Principal {
            roles: [Role::PlatformAdmin].into_iter().collect(),
            ..Principal::default()
        };
        let owner_member_id = MemberId::new("mem_group_envelope_owner").unwrap();
        state
            .members
            .write()
            .unwrap()
            .invite(
                &platform_admin,
                &[],
                Member {
                    id: owner_member_id.clone(),
                    account_id: owner_id.clone(),
                    workspace_id: workspace_id.clone(),
                    team_id: None,
                    role: MemberRole::Owner,
                    status: MemberStatus::Invited,
                    invite_expires_at: Some(u64::MAX),
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
                &workspace_id,
                &owner_member_id,
                2,
            )
            .unwrap();
        let admin_grants = vec![Grant {
            id: GrantId::new("grant_group_envelope_member_create").unwrap(),
            account_id: Some(owner_id.clone()),
            member_id: None,
            capability: Capability::new("CAP-MEM-001").unwrap(),
            action: Action::Create,
            scope: ResourceScope::workspace(workspace_id.clone()),
            effect: Effect::Allow,
        }];
        state
            .members
            .write()
            .unwrap()
            .invite(
                &owner,
                &admin_grants,
                Member {
                    id: member_id.clone(),
                    account_id: recipient_id.clone(),
                    workspace_id: workspace_id.clone(),
                    team_id: None,
                    role: MemberRole::Member,
                    status: MemberStatus::Invited,
                    invite_expires_at: Some(u64::MAX),
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
                &workspace_id,
                &member_id,
                2,
            )
            .unwrap();
        state
            .create_profile_record(Profile {
                id: profile_id.clone(),
                name: "Group-only encrypted profile".into(),
                runtime: Runtime::Wayfern,
                runtime_version: Some("1.0.0".into()),
                runtime_config: None,
                state: ProfileState::Ready,
                archived: false,
                fingerprint: None,
                note: None,
                proxy_id: None,
                vpn_id: None,
                group_ids: vec![group_id.clone()],
                tags: Vec::new(),
                extension_group_id: None,
                sync_mode: Some(buglogin_domain::entities::SyncMode::Managed),
                created_at: "2026-10-14T00:00:00Z".into(),
                updated_at: "2026-10-14T00:00:00Z".into(),
            })
            .unwrap();
        state
            .resources
            .write()
            .unwrap()
            .register(
                &owner,
                &[
                    admin_grants[0].clone(),
                    Grant {
                        id: GrantId::new("grant_group_envelope_resource_create").unwrap(),
                        account_id: Some(owner_id.clone()),
                        member_id: None,
                        capability: Capability::new("CAP-RES-001").unwrap(),
                        action: Action::Create,
                        scope: ResourceScope::workspace(workspace_id.clone()),
                        effect: Effect::Allow,
                    },
                ],
                Resource {
                    id: resource_id.clone(),
                    workspace_id: workspace_id.clone(),
                    kind: ResourceKind::Profile,
                    state: ResourceState::Active,
                    display_name: "Group-only encrypted profile".into(),
                },
            )
            .unwrap();
        state
            .groups
            .write()
            .unwrap()
            .import_records(
                vec![GroupRecord {
                    group: buglogin_domain::entities::Group {
                        id: group_id.clone(),
                        name: "Shared profiles".into(),
                        appearance: None,
                        created_at: "2026-10-14T00:00:00Z".into(),
                        updated_at: "2026-10-14T00:00:00Z".into(),
                    },
                    workspace_id: workspace_id.clone(),
                    revision: 1,
                }],
                vec![GroupShareGrant {
                    id: GrantId::new("grt_group_envelope_member_read").unwrap(),
                    workspace_id: workspace_id.clone(),
                    group_id,
                    member_id: member_id.clone(),
                    action: Action::Read,
                    effect: Effect::Allow,
                    revision: 1,
                    created_at: "2026-10-14T00:00:00Z".into(),
                    revoked_at: None,
                }],
            )
            .unwrap();

        let now = now();
        let device_id = "device_group_envelope_member";
        let access_token = state
            .auth
            .write()
            .unwrap()
            .issue_device_session(
                recipient_id.clone(),
                now + 3_600,
                now + 7_200,
                device_id.into(),
                None,
            )
            .map(|mut tokens| std::mem::take(&mut tokens.access_token))
            .unwrap();
        let key_version = "a".repeat(64);
        let key_object = ManifestObject::new(
            buglogin_sync::manifest::RESOURCE_KEY_ENVELOPE_KEY,
            b"encrypted-profile-key",
        )
        .unwrap();
        let mut manifest = SyncManifest::new(
            workspace_id.clone(),
            resource_id.clone(),
            1,
            1,
            None,
            b"group-envelope-manifest",
            vec![key_object.clone()],
        )
        .unwrap();
        manifest.payload_encryption = Some(ManifestPayloadEncryption {
            schema_version: 1,
            algorithm: PayloadEncryptionAlgorithm::Aes256GcmChunkV2,
            key_version: Some(key_version.clone()),
            resource_key_envelope: ManifestObjectReference::from(&key_object),
        });
        state.replace_sync_manifest(manifest).unwrap();
        let expected = ProfileKeyShareEnvelope {
            schema_version: 2,
            workspace_id,
            resource_id,
            recipient_device_id: device_id.into(),
            key_version,
            encapped_key: vec![1; 32],
            ciphertext: vec![2; 80],
        };
        state
            .replace_profile_key_share_envelopes(
                &profile_id,
                &expected.workspace_id,
                &expected.resource_id,
                &member_id,
                vec![ConfirmedProfileKeyShareEnvelope {
                    envelope: expected.clone(),
                    confirmed_fingerprint: "b".repeat(64),
                }],
            )
            .unwrap();

        let other_device_token = state
            .auth
            .write()
            .unwrap()
            .issue_device_session(
                recipient_id.clone(),
                now + 3_600,
                now + 7_200,
                "device_group_envelope_other".into(),
                None,
            )
            .map(|mut tokens| std::mem::take(&mut tokens.access_token))
            .unwrap();

        let revocation_targets = match control::encrypted_profiles_with_device_envelopes(
            &state,
            &recipient_id,
            device_id,
        ) {
            Ok(resources) => resources,
            Err(_) => panic!("current device envelope must be identified for rotation"),
        };
        assert!(
            revocation_targets
                .contains(&(expected.workspace_id.clone(), expected.resource_id.clone(),))
        );

        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {access_token}")).unwrap(),
        );
        headers.insert(
            "x-correlation-id",
            HeaderValue::from_static("group-envelope-http-test"),
        );
        let response = router(state.clone())
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/v1/profiles/{}/key-envelope", profile_id.as_str()))
                    .header("authorization", format!("Bearer {access_token}"))
                    .header("x-correlation-id", "group-envelope-http-test")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let actual: ProfileKeyShareEnvelope = serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), 64 * 1024)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(actual, expected);

        let other_device_response = router(state.clone())
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/v1/profiles/{}/key-envelope", profile_id.as_str()))
                    .header("authorization", format!("Bearer {other_device_token}"))
                    .header("x-correlation-id", "group-envelope-other-device-test")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(other_device_response.status(), StatusCode::NOT_FOUND);
        let other_device_problem: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(other_device_response.into_body(), 64 * 1024)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            other_device_problem["code"],
            "profile_key_envelope_not_found"
        );
        assert!(other_device_problem.get("encappedKey").is_none());
        assert!(other_device_problem.get("ciphertext").is_none());

        let target_session_id = state
            .auth
            .read()
            .unwrap()
            .list_sessions(&recipient_id, None)
            .unwrap()
            .into_iter()
            .find(|session| session.device_id.as_deref() == Some(device_id))
            .expect("device session exists")
            .id;
        headers.insert(
            "idempotency-key",
            HeaderValue::from_static("revoke-group-envelope-device-0001"),
        );
        let revoke_response =
            revoke_web_session(State(state.clone()), Path(target_session_id), headers)
                .await
                .unwrap();
        assert_eq!(revoke_response.status(), StatusCode::NO_CONTENT);
        assert!(
            state
                .resource_key_rotation_pending(&expected.workspace_id, &expected.resource_id)
                .unwrap()
        );

        let mut other_headers = HeaderMap::new();
        other_headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {other_device_token}")).unwrap(),
        );
        other_headers.insert(
            "idempotency-key",
            HeaderValue::from_static("device-group-envelope-register-0001"),
        );
        other_headers.insert(
            "x-correlation-id",
            HeaderValue::from_static("group-envelope-other-device-test"),
        );
        let device_private_key = buglogin_sync::crypto::DevicePrivateKey::generate().unwrap();
        let device_public_key = device_private_key.public_key().unwrap();
        let public_key = device_public_key
            .as_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let registered = register_device_encryption_key(
            State(state.clone()),
            other_headers.clone(),
            Json(DeviceEncryptionKeyRegistrationInput { public_key }),
        )
        .await
        .unwrap();
        assert_eq!(registered.status(), StatusCode::OK);
        assert!(
            state
                .resource_key_rotation_pending(&expected.workspace_id, &expected.resource_id)
                .unwrap()
        );
        let mut persisted_store = control_store;
        let device_key_records = persisted_store
            .list("identity", "device_encryption_key")
            .unwrap();
        assert!(
            device_key_records
                .iter()
                .any(|record| record.value["deviceId"] == "device_group_envelope_other")
        );
        let aggregate_record = persisted_store
            .get("control", "control_state", "primary")
            .unwrap()
            .unwrap();
        let persisted_aggregate: ControlAggregate =
            serde_json::from_value(aggregate_record.value).unwrap();
        assert!(
            persisted_aggregate
                .resource_key_rotation_pending
                .contains(&sync_manifest_key(
                    &expected.workspace_id,
                    &expected.resource_id
                ))
        );

        let error = get_profile_key_share_envelope(
            State(state),
            Path(profile_id.as_str().to_owned()),
            other_headers,
        )
        .await
        .unwrap_err();
        assert_eq!(error.status(), StatusCode::CONFLICT);
    }

    #[test]
    fn profile_key_share_envelopes_survive_reopen_and_are_removed_for_revoked_member() {
        let first_root = test_root("key-envelope-first");
        let second_root = test_root("key-envelope-second");
        let third_root = test_root("key-envelope-third");
        for root in [&first_root, &second_root, &third_root] {
            let _ = fs::remove_dir_all(root);
            fs::create_dir_all(root).unwrap();
        }
        let store = MemoryControlRecordStore::default();
        let first = ApiState::try_with_account_store(
            ProfileService::new(LocalRepository::open(&first_root).unwrap()),
            store.clone(),
            true,
        )
        .unwrap();
        let profile_id = ProfileId::from_str("prf_key_envelope_restart").unwrap();
        let workspace_id = WorkspaceId::from_str("ws_key_envelope_restart").unwrap();
        let resource_id =
            buglogin_domain::ids::ResourceId::from_str("res_key_envelope_restart").unwrap();
        let member_id = MemberId::from_str("mem_key_envelope_restart").unwrap();
        let envelope = ProfileKeyShareEnvelope {
            schema_version: 2,
            workspace_id: workspace_id.clone(),
            resource_id: resource_id.clone(),
            recipient_device_id: "device-key-envelope-1".into(),
            key_version: "a".repeat(64),
            encapped_key: vec![1; 32],
            ciphertext: vec![2; 80],
        };
        first
            .replace_profile_key_share_envelopes(
                &profile_id,
                &workspace_id,
                &resource_id,
                &member_id,
                vec![ConfirmedProfileKeyShareEnvelope {
                    envelope: envelope.clone(),
                    confirmed_fingerprint: "b".repeat(64),
                }],
            )
            .unwrap();
        first
            .persist_control_aggregate(
                "profile.share",
                "profile-key-envelope-reopen-0001".into(),
                "acct_key_envelope_owner".into(),
                true,
                "key-envelope-input-hash",
                serde_json::json!({}),
                200,
                serde_json::json!({"shared": true}),
            )
            .unwrap();

        let second = ApiState::try_with_account_store(
            ProfileService::new(LocalRepository::open(&second_root).unwrap()),
            store.reopen(),
            true,
        )
        .unwrap();
        assert_eq!(
            second
                .profile_key_share_envelope(&resource_id, &member_id, "device-key-envelope-1")
                .unwrap(),
            Some(envelope)
        );
        second
            .remove_profile_key_share_envelopes(&profile_id, &member_id)
            .unwrap();
        second
            .persist_control_aggregate(
                "profile.share.revoke",
                "profile-key-envelope-revoke-0001".into(),
                "acct_key_envelope_owner".into(),
                true,
                "key-envelope-revoke-hash",
                serde_json::json!({}),
                200,
                serde_json::json!({"revoked": true}),
            )
            .unwrap();
        let third = ApiState::try_with_account_store(
            ProfileService::new(LocalRepository::open(&third_root).unwrap()),
            store.reopen(),
            true,
        )
        .unwrap();
        assert_eq!(
            third
                .profile_key_share_envelope(&resource_id, &member_id, "device-key-envelope-1")
                .unwrap(),
            None
        );
        for root in [first_root, second_root, third_root] {
            let _ = fs::remove_dir_all(root);
        }
    }

    #[test]
    fn sync_finalize_persists_manifest_journal_and_receipt_atomically() {
        use buglogin_domain::ids::{ResourceId, WorkspaceId};
        use buglogin_storage::control::{
            ClaimControlCommand, ControlIdempotencyClaim, MemoryControlRecordStore,
        };
        use buglogin_sync::manifest::SyncManifest;

        let profile_root = test_root("sync-finalize-first");
        let second_root = test_root("sync-finalize-second");
        let _ = fs::remove_dir_all(&profile_root);
        let _ = fs::remove_dir_all(&second_root);
        fs::create_dir_all(&profile_root).unwrap();
        fs::create_dir_all(&second_root).unwrap();
        let store = MemoryControlRecordStore::default();
        let first = ApiState::try_with_account_store(
            ProfileService::new(LocalRepository::open(&profile_root).unwrap()),
            store.clone(),
            true,
        )
        .unwrap();
        let workspace = WorkspaceId::new("ws_sync_finalize").unwrap();
        let resource = ResourceId::new("res_sync_finalize").unwrap();
        let manifest = SyncManifest::new(
            workspace.clone(),
            resource.clone(),
            77,
            1,
            None,
            b"sync-finalize-root",
            vec![],
        )
        .unwrap();
        let journal_id = "msi_sync_finalize";
        let mut journal = SyncMigrationJournal {
            schema: "buglogin.migration-sync-journal/1".into(),
            operation_id: "op_sync_finalize".into(),
            export_id: "export_sync_finalize".into(),
            export_sha256: "a".repeat(64),
            manifest_sha256: "b".repeat(64),
            blob_set_sha256: "c".repeat(64),
            workspace_id: workspace.as_str().into(),
            resource_id: resource.as_str().into(),
            base_generation: None,
            target_generation: 1,
            state: SyncMigrationJournalState::Staging,
            created_objects: vec![],
            resumed_objects: vec![],
            reserved_objects: vec![],
            destination_fencing_token: Some(77),
            control_revision_before: first.control_revision().unwrap(),
            control_revision_after: None,
            updated_at_ms: 1,
            error_code: None,
        };
        let journal_revision = first
            .persist_sync_migration_journal(
                journal_id,
                0,
                "sync-finalize-journal-0001".into(),
                "acct_sync_finalize".into(),
                &journal,
            )
            .unwrap();
        assert_eq!(
            first
                .persist_sync_migration_journal(
                    journal_id,
                    0,
                    "sync-finalize-journal-0001".into(),
                    "acct_sync_finalize".into(),
                    &journal,
                )
                .unwrap(),
            journal_revision
        );
        let claim = match first
            .control_persistence
            .claim_idempotency(ClaimControlCommand {
                tenant_id: CONTROL_TENANT.into(),
                idempotency_key: "sync-finalize-import-0001".into(),
                request_checksum: "d".repeat(64),
                claim_token: "e".repeat(64),
                operation_id: "op_sync_finalize".into(),
            })
            .unwrap()
        {
            ControlIdempotencyClaim::Claimed { claim } => claim,
            other => panic!("unexpected claim: {other:?}"),
        };
        let mut target = first.control_aggregate().unwrap();
        target
            .sync_manifests
            .insert(sync_manifest_key(&workspace, &resource), manifest.clone());
        journal.state = SyncMigrationJournalState::Active;
        journal.control_revision_after = Some(journal.control_revision_before + 1);
        first
            .finalize_sync_import(
                claim,
                target,
                journal_id,
                journal_revision,
                &journal,
                &"d".repeat(64),
                "acct_sync_finalize".into(),
                serde_json::json!({"journalId": journal_id}),
                serde_json::json!({"state": "active", "generation": 1}),
            )
            .unwrap();

        let second = ApiState::try_with_account_store(
            ProfileService::new(LocalRepository::open(&second_root).unwrap()),
            store.reopen(),
            true,
        )
        .unwrap();
        assert_eq!(
            second.current_sync_manifest(&workspace, &resource).unwrap(),
            Some(manifest)
        );
        assert_eq!(
            second
                .load_sync_migration_journal(journal_id)
                .unwrap()
                .unwrap()
                .0
                .state,
            SyncMigrationJournalState::Active
        );
        assert_eq!(
            second
                .replay_control_response("sync-finalize-import-0001", &"d".repeat(64))
                .unwrap()
                .unwrap()
                .body["state"],
            "active"
        );
        let _ = fs::remove_dir_all(profile_root);
        let _ = fs::remove_dir_all(second_root);
    }

    #[test]
    fn fresh_sync_manifest_read_refreshes_a_stale_api_instance() {
        use buglogin_domain::ids::{ResourceId, WorkspaceId};

        let store = MemoryControlRecordStore::default();
        let reader =
            ApiState::hosted_with_control_store(ProfileService::disabled(), store.clone()).unwrap();
        let writer =
            ApiState::hosted_with_control_store(ProfileService::disabled(), store).unwrap();
        let workspace = WorkspaceId::new("ws_fresh_manifest").unwrap();
        let resource = ResourceId::new("res_fresh_manifest").unwrap();
        let manifest = SyncManifest::new(
            workspace.clone(),
            resource.clone(),
            11,
            1,
            None,
            b"fresh-manifest-root",
            vec![],
        )
        .unwrap();
        writer.replace_sync_manifest(manifest.clone()).unwrap();
        writer
            .persist_control_aggregate(
                "test.sync.manifest.write",
                "fresh-sync-manifest-write".into(),
                "acct_fresh_manifest".into(),
                true,
                "fresh-sync-manifest-hash",
                serde_json::json!({}),
                200,
                serde_json::json!({"written": true}),
            )
            .unwrap();

        assert_eq!(
            reader.current_sync_manifest(&workspace, &resource).unwrap(),
            None
        );
        assert_eq!(
            reader
                .current_sync_manifest_fresh(&workspace, &resource)
                .unwrap(),
            Some(manifest)
        );
    }

    #[test]
    fn ambiguous_finalize_is_recovered_from_durable_receipt_without_rollback() {
        use buglogin_domain::ids::{ResourceId, WorkspaceId};
        use buglogin_sync::manifest::SyncManifest;

        let profile_root = test_root("sync-ambiguous");
        let _ = fs::remove_dir_all(&profile_root);
        fs::create_dir_all(&profile_root).unwrap();
        let memory = MemoryControlRecordStore::default();
        let state = ApiState::try_with_account_store(
            ProfileService::new(LocalRepository::open(&profile_root).unwrap()),
            AmbiguousCompleteStore::new(memory.clone()),
            true,
        )
        .unwrap();
        let workspace = WorkspaceId::new("ws_sync_ambiguous").unwrap();
        let resource = ResourceId::new("res_sync_ambiguous").unwrap();
        let manifest = SyncManifest::new(
            workspace.clone(),
            resource.clone(),
            91,
            1,
            None,
            b"sync-ambiguous-root",
            vec![],
        )
        .unwrap();
        let journal_id = "msi_sync_ambiguous";
        let request_hash = "f".repeat(64);
        let mut journal = SyncMigrationJournal {
            schema: "buglogin.migration-sync-journal/1".into(),
            operation_id: "op_sync_ambiguous".into(),
            export_id: "export_sync_ambiguous".into(),
            export_sha256: "a".repeat(64),
            manifest_sha256: "b".repeat(64),
            blob_set_sha256: "c".repeat(64),
            workspace_id: workspace.as_str().into(),
            resource_id: resource.as_str().into(),
            base_generation: None,
            target_generation: 1,
            state: SyncMigrationJournalState::Staging,
            created_objects: vec![],
            resumed_objects: vec![],
            reserved_objects: vec![],
            destination_fencing_token: Some(91),
            control_revision_before: state.control_revision().unwrap(),
            control_revision_after: None,
            updated_at_ms: 1,
            error_code: None,
        };
        let journal_revision = state
            .persist_sync_migration_journal(
                journal_id,
                0,
                "sync-ambiguous-journal-0001".into(),
                "acct_sync_ambiguous".into(),
                &journal,
            )
            .unwrap();
        let claim = match state
            .control_persistence
            .claim_idempotency(ClaimControlCommand {
                tenant_id: CONTROL_TENANT.into(),
                idempotency_key: "sync-ambiguous-import-0001".into(),
                request_checksum: request_hash.clone(),
                claim_token: "e".repeat(64),
                operation_id: "op_sync_ambiguous".into(),
            })
            .unwrap()
        {
            ControlIdempotencyClaim::Claimed { claim } => claim,
            other => panic!("unexpected claim: {other:?}"),
        };
        let mut target = state.control_aggregate().unwrap();
        target
            .sync_manifests
            .insert(sync_manifest_key(&workspace, &resource), manifest.clone());
        journal.state = SyncMigrationJournalState::Active;
        journal.control_revision_after = Some(journal.control_revision_before + 1);
        assert_eq!(
            state
                .finalize_sync_import(
                    claim,
                    target,
                    journal_id,
                    journal_revision,
                    &journal,
                    &request_hash,
                    "acct_sync_ambiguous".into(),
                    serde_json::json!({"journalId": journal_id}),
                    serde_json::json!({"state": "active"}),
                )
                .unwrap_err()
                .code,
            "control_persistence_unavailable"
        );
        let replay = state
            .replay_control_response("sync-ambiguous-import-0001", &request_hash)
            .unwrap()
            .unwrap();
        assert_eq!(replay.body["state"], "active");
        state.reload_control_aggregate().unwrap();
        assert_eq!(
            state.current_sync_manifest(&workspace, &resource).unwrap(),
            Some(manifest)
        );
        assert_eq!(
            state
                .load_sync_migration_journal(journal_id)
                .unwrap()
                .unwrap()
                .0
                .state,
            SyncMigrationJournalState::Active
        );
        let _ = fs::remove_dir_all(profile_root);
    }

    fn hosted_test_profile(id: &str) -> Profile {
        Profile {
            id: ProfileId::from_str(id).unwrap(),
            name: "Hosted profile".into(),
            runtime: Runtime::Wayfern,
            runtime_version: Some("1.0.0".into()),
            runtime_config: None,
            state: ProfileState::Ready,
            archived: false,
            fingerprint: None,
            note: None,
            proxy_id: None,
            vpn_id: None,
            group_ids: vec![],
            tags: vec![],
            extension_group_id: None,
            sync_mode: Some(buglogin_domain::entities::SyncMode::Managed),
            created_at: "2026-07-22T00:00:00Z".into(),
            updated_at: "2026-07-22T00:00:00Z".into(),
        }
    }

    #[test]
    fn hosted_profile_archive_rejects_running_projection_without_mutation() {
        let root = test_root("hosted-profile-running");
        let _ = fs::remove_dir_all(&root);
        let profile_id = ProfileId::from_str("prf_hosted_running").unwrap();
        let state = ApiState::try_with_account_store(
            ProfileService::disabled(),
            MemoryControlRecordStore::default(),
            false,
        )
        .unwrap()
        .with_profile_resource(ProfileResource {
            profile_id: profile_id.clone(),
            resource_id: buglogin_domain::ids::ResourceId::from_str("res_hosted_running").unwrap(),
            workspace_id: buglogin_domain::ids::WorkspaceId::from_str("ws_hosted_running").unwrap(),
            archive_state: buglogin_domain::profile::ProfileArchiveState::Active,
        });
        state
            .create_profile_record(hosted_test_profile("prf_hosted_running"))
            .unwrap();
        state
            .set_runtime_projection(RuntimeProjection {
                profile_id: profile_id.clone(),
                device_id: "device-hosted-running".into(),
                owner_id: "desktop-hosted-running".into(),
                state: "running".into(),
                generation: 1,
                root_checksum: "a".repeat(64),
                updated_at_ms: 1,
            })
            .unwrap();

        let error = state
            .mutate_hosted_profile_archive_state(&profile_id, 1, true)
            .unwrap_err();
        assert_eq!(error.code, "profile_running");
        let stored = state
            .hosted_profiles
            .read()
            .unwrap()
            .get(&profile_id)
            .cloned()
            .unwrap();
        assert_eq!(stored.revision, 1);
        assert!(!stored.value.archived);
        assert_eq!(
            state
                .profile_resources
                .read()
                .unwrap()
                .get(&profile_id)
                .unwrap()
                .archive_state,
            buglogin_domain::profile::ProfileArchiveState::Active
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn profile_resource_index_tracks_rebinding_and_rejects_duplicate_resources() {
        let state = ApiState::new(ProfileService::disabled());
        let profile_id = ProfileId::from_str("prf_resource_index_one").unwrap();
        let first_workspace = WorkspaceId::from_str("ws_resource_index_first").unwrap();
        let second_workspace = WorkspaceId::from_str("ws_resource_index_second").unwrap();
        let first_resource =
            buglogin_domain::ids::ResourceId::from_str("res_resource_index_first").unwrap();
        let second_resource =
            buglogin_domain::ids::ResourceId::from_str("res_resource_index_second").unwrap();
        state
            .replace_profile_resource(ProfileResource {
                profile_id: profile_id.clone(),
                workspace_id: first_workspace.clone(),
                resource_id: first_resource.clone(),
                archive_state: ProfileArchiveState::Active,
            })
            .unwrap();
        state
            .replace_profile_resource(ProfileResource {
                profile_id: profile_id.clone(),
                workspace_id: second_workspace.clone(),
                resource_id: second_resource.clone(),
                archive_state: ProfileArchiveState::Active,
            })
            .unwrap();
        assert!(
            state
                .profile_resource_for_resource(&first_workspace, &first_resource)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            state
                .profile_resource_for_resource(&second_workspace, &second_resource)
                .unwrap()
                .unwrap()
                .profile_id,
            profile_id
        );

        state
            .replace_profile_resource(ProfileResource {
                profile_id: ProfileId::from_str("prf_resource_index_duplicate").unwrap(),
                workspace_id: second_workspace.clone(),
                resource_id: second_resource.clone(),
                archive_state: ProfileArchiveState::Active,
            })
            .unwrap();
        assert_eq!(
            state
                .profile_resource_for_resource(&second_workspace, &second_resource)
                .unwrap_err()
                .code,
            "profile_binding_ambiguous"
        );

        let reload_profile = ProfileId::from_str("prf_resource_index_reloaded").unwrap();
        let reload_workspace = WorkspaceId::from_str("ws_resource_index_reloaded").unwrap();
        let reload_resource =
            buglogin_domain::ids::ResourceId::from_str("res_resource_index_reloaded").unwrap();
        let mut aggregate = state.control_aggregate().unwrap();
        aggregate.profile_resources.clear();
        aggregate.profile_resources.insert(
            reload_profile.clone(),
            ProfileResource {
                profile_id: reload_profile.clone(),
                workspace_id: reload_workspace.clone(),
                resource_id: reload_resource.clone(),
                archive_state: ProfileArchiveState::Active,
            },
        );
        state.replace_control_aggregate(aggregate).unwrap();
        assert!(
            state
                .profile_resource_for_resource(&second_workspace, &second_resource)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            state
                .profile_resource_for_resource(&reload_workspace, &reload_resource)
                .unwrap()
                .unwrap()
                .profile_id,
            reload_profile
        );
    }

    #[test]
    fn quarantined_profile_restore_preserves_pre_delete_archive_state() {
        let root = test_root("hosted-profile-quarantine-archived");
        let _ = fs::remove_dir_all(&root);
        let profile_id = ProfileId::from_str("prf_quarantine_archived").unwrap();
        let resource_id =
            buglogin_domain::ids::ResourceId::from_str("res_quarantine_archived").unwrap();
        let workspace_id =
            buglogin_domain::ids::WorkspaceId::from_str("ws_quarantine_archived").unwrap();
        let state = ApiState::try_with_account_store(
            ProfileService::disabled(),
            MemoryControlRecordStore::default(),
            false,
        )
        .unwrap()
        .with_profile_resource(ProfileResource {
            profile_id: profile_id.clone(),
            resource_id,
            workspace_id,
            archive_state: ProfileArchiveState::Archived,
        });
        let mut profile = hosted_test_profile(profile_id.as_str());
        profile.archived = true;
        state.create_profile_record(profile).unwrap();

        HostedProfileService::delete_to_quarantine(
            &state,
            &profile_id,
            1,
            "QUARANTINE_PROFILE_DATA",
            None,
        )
        .unwrap();
        let restored =
            HostedProfileService::set_archive_state(&state, &profile_id, 2, false).unwrap();

        assert!(restored.value.archived);
        assert!(
            state
                .profile_quarantine_record(&profile_id)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            state
                .profile_resource_record(&profile_id)
                .unwrap()
                .unwrap()
                .archive_state,
            ProfileArchiveState::Archived
        );
        drop(state);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn hosted_mcp_profile_archive_recovers_ambiguous_commit_and_survives_restart() {
        let root = test_root("hosted-mcp-profile-archive");
        let _ = fs::remove_dir_all(&root);
        let account = AccountId::from_str("acct_hosted_mcp_profile").unwrap();
        let profile_id = ProfileId::from_str("prf_hosted_mcp_profile").unwrap();
        let mut store = MemoryControlRecordStore::default();
        let account_value = serde_json::to_value(buglogin_domain::account::Account::new(
            account.clone(),
            "2026-07-22T00:00:00Z",
        ))
        .unwrap();
        store
            .put(PutControlRecord {
                tenant_id: CONTROL_TENANT,
                record_type: "account",
                record_id: account.as_str(),
                expected_revision: Some(0),
                idempotency_key: "seed-hosted-mcp-profile-account",
                value: &account_value,
            })
            .unwrap();
        let reopen = store.reopen();
        let state = ApiState::try_with_account_store(
            ProfileService::disabled(),
            AmbiguousCompleteStore::new(store),
            false,
        )
        .unwrap()
        .with_profile_resource(ProfileResource {
            profile_id: profile_id.clone(),
            resource_id: buglogin_domain::ids::ResourceId::from_str("res_hosted_mcp_profile")
                .unwrap(),
            workspace_id: buglogin_domain::ids::WorkspaceId::from_str("ws_hosted_mcp_profile")
                .unwrap(),
            archive_state: buglogin_domain::profile::ProfileArchiveState::Active,
        });
        state
            .create_profile_record(hosted_test_profile("prf_hosted_mcp_profile"))
            .unwrap();
        state.grants.write().unwrap().extend([
            Grant {
                id: GrantId::from_str("grant_hosted_mcp_archive").unwrap(),
                account_id: Some(account.clone()),
                member_id: None,
                capability: Capability::new("profile.archive").unwrap(),
                action: Action::Update,
                scope: ResourceScope {
                    profile_id: Some(profile_id.clone()),
                    ..ResourceScope::default()
                },
                effect: Effect::Allow,
            },
            Grant {
                id: GrantId::from_str("grant_hosted_mcp_restore").unwrap(),
                account_id: Some(account.clone()),
                member_id: None,
                capability: Capability::new("profile.restore").unwrap(),
                action: Action::Update,
                scope: ResourceScope {
                    profile_id: Some(profile_id.clone()),
                    ..ResourceScope::default()
                },
                effect: Effect::Allow,
            },
        ]);
        let token = state.issue_token(account.as_str()).unwrap();
        let mut dispatcher = crate::mcp::McpDispatcher::new(
            crate::mcp::ProductionMcpPolicy::new(state.clone()),
            crate::mcp::ProductionMcpExecutor::new(state.clone()),
            true,
        );
        let request = crate::mcp::McpRequest {
            tool_name: "profile_archive".into(),
            correlation_id: "cor_hosted_mcp_profile_archive".into(),
            bearer_token: Some(token.clone()),
            idempotency_key: Some("hosted-mcp-profile-archive-0001".into()),
            input: serde_json::json!({
                "profileId": profile_id,
                "revision": 1,
            }),
        };
        let error = dispatcher.dispatch(request.clone()).unwrap_err();
        assert_eq!(error.code, "control_persistence_unavailable");
        assert!(
            state
                .get_profile_record(&profile_id)
                .unwrap()
                .unwrap()
                .value
                .archived
        );
        let replay = dispatcher.dispatch(request).unwrap();
        assert_eq!(replay["archived"], true);
        let restored = dispatcher
            .dispatch(crate::mcp::McpRequest {
                tool_name: "profile_restore".into(),
                correlation_id: "cor_hosted_mcp_profile_restore".into(),
                bearer_token: Some(token),
                idempotency_key: Some("hosted-mcp-profile-restore-0001".into()),
                input: serde_json::json!({
                    "profileId": profile_id,
                    "revision": 2,
                }),
            })
            .unwrap();
        assert_eq!(restored["archived"], false);
        drop(dispatcher);
        drop(state);

        let restarted =
            ApiState::try_with_account_store(ProfileService::disabled(), reopen, false).unwrap();
        let stored = restarted.get_profile_record(&profile_id).unwrap().unwrap();
        assert_eq!(stored.revision, 3);
        assert!(!stored.value.archived);
        assert_eq!(
            restarted
                .profile_resources
                .read()
                .unwrap()
                .get(&profile_id)
                .unwrap()
                .archive_state,
            buglogin_domain::profile::ProfileArchiveState::Active
        );
        let _ = fs::remove_dir_all(root);
    }
}
