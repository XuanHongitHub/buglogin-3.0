use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::PathBuf,
    sync::{Arc, Mutex, RwLock},
    time::{SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use axum::{
    Json, Router,
    body::Bytes,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
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
    account::{AccountIdentity, AccountStatus},
    entities::{Profile, ProfileState, Runtime},
    errors::Problem,
    ids::{AccountId, GrantId, MemberId, ProfileId, WorkspaceId},
    member::MemberRole,
    permissions::{Action, Capability, Effect, Grant, Principal, ResourceScope, Role, is_allowed},
    profile::{ProfileCloneInput, ProfileResource, ProfileShareInput, ProfileShareRevokeInput},
};
use buglogin_migration::{
    MigrationKey, activate_package,
    cutover::{confirm_cutover, promote_staged},
    inspect::inspect_source,
    package::PackageMetadata,
    rollback::{confirm_rollback, rollback_activation, rollback_cutover},
    signature::{SignatureMetadata, sign_package, verify_signed_package},
    transform::stage_package,
};
use buglogin_network::proxy::{
    ParseReport, ProxyRepository, ProxyRepositorySnapshot, RepositoryError, parse_lines,
};
use buglogin_storage::{
    local::Versioned,
    remote::{MemoryRemoteChunkStore, RemoteChunkStore},
    vault::NativeVault,
};
use buglogin_sync::coordination::{
    AuthorizationRequest, CoordinationAuth, CoordinationAuthorizer, CoordinationBackend,
    CoordinationOperation,
};
use buglogin_sync::{
    import::{SyncStageReceipt, rollback_staged_sync_import, stage_sync_import},
    manifest::SyncManifest,
};
use serde::{Deserialize, Serialize};
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
            ConnectedCookieJobService, ConnectedCookieReceipt, EnqueueConnectedCookieJob,
        },
        control_persistence::ControlPersistence,
        credentials::CredentialService,
        device_authorization::DeviceAuthorizationService,
        events::EventService,
        identity_persistence::IdentityPersistence,
        members::MemberService,
        operations::{Operation, OperationService, OperationStatus},
        profile_hosted::HostedProfileService,
        recovery::{RecoveryDelivery, RecoveryService, WebhookRecoveryDelivery},
        resources::ResourceService,
        session_persistence::SessionPersistence,
        teams::TeamService,
        workspaces::WorkspaceService,
    },
};

#[path = "../services/profile_lifecycle.rs"]
pub mod profile_lifecycle;
use profile_lifecycle::{ProfileLifecycleCommand, ProfileLifecycleService};

mod control;
mod control_import;
mod package_upload;
mod sync_import;

const CONTROL_TENANT: &str = "control";
const CONTROL_STATE_TYPE: &str = "control_state";
const CONTROL_STATE_ID: &str = "primary";

#[derive(Clone, Default, Deserialize, Serialize)]
pub(crate) struct ControlAggregate {
    grants: Vec<Grant>,
    #[serde(default)]
    materialized_member_grants: BTreeSet<MemberId>,
    #[serde(default)]
    platform_admin_accounts: BTreeSet<AccountId>,
    #[serde(default)]
    profiles: BTreeMap<ProfileId, Versioned<Profile>>,
    #[serde(default)]
    profile_resources: BTreeMap<ProfileId, ProfileResource>,
    #[serde(default)]
    sync_manifests: BTreeMap<String, SyncManifest>,
    #[serde(default)]
    blob_idempotency: BTreeMap<String, BlobIdempotencyRecord>,
    #[serde(default)]
    runtime_projections: BTreeMap<ProfileId, RuntimeProjection>,
    #[serde(default)]
    proxies: ProxyRepositorySnapshot,
    workspaces: WorkspaceService,
    members: MemberService,
    resources: ResourceService,
    teams: TeamService,
    audit: AuditService,
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
    api_tokens: ApiTokenService,
    device_verification_uri: Arc<str>,
    web_origin: Arc<str>,
    pub grants: Arc<RwLock<Vec<Grant>>>,
    pub operations: Arc<RwLock<OperationService>>,
    pub events: Arc<RwLock<EventService>>,
    pub workspaces: Arc<RwLock<WorkspaceService>>,
    pub members: Arc<RwLock<MemberService>>,
    pub resources: Arc<RwLock<ResourceService>>,
    hosted_profiles: Arc<RwLock<BTreeMap<ProfileId, Versioned<Profile>>>>,
    profile_resources: Arc<RwLock<BTreeMap<ProfileId, ProfileResource>>>,
    sync_manifests: Arc<RwLock<BTreeMap<String, SyncManifest>>>,
    blob_idempotency: Arc<RwLock<BTreeMap<String, BlobIdempotencyRecord>>>,
    runtime_projections: Arc<RwLock<BTreeMap<ProfileId, RuntimeProjection>>>,
    pub proxies: Arc<RwLock<ProxyRepository>>,
    pub teams: Arc<RwLock<TeamService>>,
    pub connected_cookie_jobs: ConnectedCookieJobService,
    pub audit: Arc<RwLock<AuditService>>,
    local_cookies: Option<Arc<Mutex<CookieAuthority<NativeVault>>>>,
    account_persistence: AccountPersistence,
    identity_persistence: IdentityPersistence,
    credentials: CredentialService,
    pub(crate) recovery: RecoveryService,
    pub(crate) control_persistence: ControlPersistence,
    control_revision: Arc<RwLock<u64>>,
    control_mutation: Arc<Mutex<()>>,
    pub(crate) control_snapshot_gate: Arc<tokio::sync::RwLock<()>>,
    materialized_member_grants: Arc<RwLock<BTreeSet<MemberId>>>,
    platform_admin_accounts: Arc<RwLock<BTreeSet<AccountId>>>,
    local_account_provisioning: bool,
    external_object_storage: bool,
    pub lifecycles: Arc<RwLock<HashMap<String, Lifecycle>>>,
    pub runtime_binaries: Arc<RwLock<HashMap<String, RuntimeBinary>>>,
    migration_key: Arc<RwLock<Option<MigrationKey>>>,
    migration_signing_key: Arc<RwLock<Option<buglogin_storage::secrets::SecretValue>>>,
    coordination: Arc<RwLock<Option<Arc<dyn CoordinationBackend>>>>,
    remote_chunks: Arc<Mutex<Box<dyn RemoteChunkStore>>>,
}

impl ApiState {
    pub fn new(profiles: ProfileService) -> Self {
        Self::local(profiles)
    }

    pub fn local(profiles: ProfileService) -> Self {
        Self::try_with_account_store(
            profiles,
            buglogin_storage::control::MemoryControlRecordStore::default(),
            true,
        )
        .expect("memory control store must initialize")
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
        Self::try_with_account_store(ProfileService::disabled(), store, false)
    }

    fn try_with_account_store<S>(
        profiles: ProfileService,
        store: S,
        local_account_provisioning: bool,
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
        let api_tokens = ApiTokenService::new(control_persistence.clone());
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
            api_tokens,
            device_verification_uri: Arc::from("http://127.0.0.1:3000/device"),
            web_origin: Arc::from("http://127.0.0.1:3000"),
            grants: Arc::new(RwLock::new(aggregate.grants)),
            materialized_member_grants: Arc::new(RwLock::new(aggregate.materialized_member_grants)),
            platform_admin_accounts: Arc::new(RwLock::new(aggregate.platform_admin_accounts)),
            operations: Arc::new(RwLock::new(OperationService::default())),
            events: Arc::new(RwLock::new(EventService::default())),
            workspaces: Arc::new(RwLock::new(aggregate.workspaces)),
            members: Arc::new(RwLock::new(aggregate.members)),
            resources: Arc::new(RwLock::new(aggregate.resources)),
            hosted_profiles: Arc::new(RwLock::new(aggregate.profiles)),
            profile_resources: Arc::new(RwLock::new(aggregate.profile_resources)),
            sync_manifests: Arc::new(RwLock::new(aggregate.sync_manifests)),
            blob_idempotency: Arc::new(RwLock::new(aggregate.blob_idempotency)),
            runtime_projections: Arc::new(RwLock::new(aggregate.runtime_projections)),
            proxies: Arc::new(RwLock::new(proxies)),
            teams: Arc::new(RwLock::new(aggregate.teams)),
            connected_cookie_jobs: ConnectedCookieJobService::new(control_persistence.clone()),
            audit: Arc::new(RwLock::new(aggregate.audit)),
            local_cookies,
            account_persistence: AccountPersistence::new(control_persistence.clone()),
            identity_persistence,
            credentials,
            recovery,
            control_persistence,
            control_revision: Arc::new(RwLock::new(revision)),
            control_mutation: Arc::new(Mutex::new(())),
            control_snapshot_gate: Arc::new(tokio::sync::RwLock::new(())),
            local_account_provisioning,
            external_object_storage: false,
            lifecycles: Arc::new(RwLock::new(HashMap::new())),
            runtime_binaries: Arc::new(RwLock::new(HashMap::new())),
            migration_key: Arc::new(RwLock::new(None)),
            migration_signing_key: Arc::new(RwLock::new(None)),
            coordination: Arc::new(RwLock::new(None)),
            remote_chunks: Arc::new(Mutex::new(Box::new(MemoryRemoteChunkStore::default()))),
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
                        .map_err(|error| cookie_problem_api(error.to_string(), correlation_id))?,
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
        self.remote_chunks
            .lock()
            .map_err(|_| {
                problem(
                    "readiness_unavailable",
                    "Service readiness check failed",
                    503,
                )
            })?
            .readiness()?;
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

    pub fn with_recovery_delivery_from_env(self, production: bool) -> Result<Self, Problem> {
        match WebhookRecoveryDelivery::from_env(production)? {
            Some(delivery) => Ok(self.with_recovery_delivery(delivery)),
            None if production => Err(problem(
                "recovery_delivery_not_configured",
                "Recovery delivery is required in production",
                500,
            )),
            None => Ok(self),
        }
    }

    pub fn with_remote_chunks(mut self, store: Box<dyn RemoteChunkStore>) -> Self {
        *self.remote_chunks.lock().expect("remote chunk lock") = store;
        self.external_object_storage = true;
        self
    }

    pub fn with_profile_resource(self, binding: ProfileResource) -> Self {
        self.profile_resources
            .write()
            .expect("profile resource lock")
            .insert(binding.profile_id.clone(), binding);
        self
    }

    pub(crate) fn put_remote_chunk(
        &self,
        object_key: &str,
        bytes: &[u8],
        expected_checksum: &str,
    ) -> Result<(), Problem> {
        self.remote_chunks
            .lock()
            .map_err(|_| problem("storage_unavailable", "Remote storage unavailable", 503))?
            .put(object_key, bytes, expected_checksum)
    }

    pub(crate) fn stage_sync_import(
        &self,
        plan: &buglogin_sync::import::SyncImportPlan,
    ) -> Result<SyncStageReceipt, Problem> {
        stage_sync_import(
            &mut **self
                .remote_chunks
                .lock()
                .map_err(|_| problem("storage_unavailable", "Remote storage unavailable", 503))?,
            plan,
        )
    }

    pub(crate) fn rollback_sync_import(&self, receipt: &SyncStageReceipt) -> Result<(), Problem> {
        rollback_staged_sync_import(
            &mut **self
                .remote_chunks
                .lock()
                .map_err(|_| problem("storage_unavailable", "Remote storage unavailable", 503))?,
            &receipt.objects,
        )
    }

    pub(crate) fn get_remote_chunk(
        &self,
        object_key: &str,
        expected_checksum: &str,
    ) -> Result<Vec<u8>, Problem> {
        Ok(self
            .remote_chunks
            .lock()
            .map_err(|_| problem("storage_unavailable", "Remote storage unavailable", 503))?
            .get(object_key, expected_checksum)?
            .bytes)
    }

    fn delete_remote_chunk(
        &self,
        object_key: &str,
        expected_checksum: &str,
    ) -> Result<(), Problem> {
        self.remote_chunks
            .lock()
            .map_err(|_| problem("storage_unavailable", "Remote storage unavailable", 503))?
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

    pub(crate) fn account(
        &self,
        account_id: &AccountId,
    ) -> Result<Option<buglogin_domain::account::Account>, Problem> {
        self.account_persistence.load(account_id)
    }

    pub fn grants(&self) -> Result<Vec<Grant>, Problem> {
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
        const CAPABILITIES: [&str; 7] = [
            "CAP-WS-001",
            "CAP-TEAM-001",
            "CAP-MEM-001",
            "CAP-RES-001",
            "CAP-AUD-001",
            "events.read",
            "operation.read",
        ];
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
            blob_idempotency: self
                .blob_idempotency
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
            .profile_resources
            .write()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))? =
            aggregate.profile_resources;
        *self
            .sync_manifests
            .write()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))? =
            aggregate.sync_manifests;
        *self
            .blob_idempotency
            .write()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))? =
            aggregate.blob_idempotency;
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
            schema_version: 1,
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
        let value = Versioned {
            schema_version: current.schema_version,
            revision: current.revision.saturating_add(1),
            value: profile,
        };
        profiles.insert(value.value.id.clone(), value.clone());
        Ok(value)
    }

    pub(crate) fn archive_profile_record(
        &self,
        expected_revision: u64,
        mut profile: Profile,
    ) -> Result<Versioned<Profile>, Problem> {
        profile.archived = true;
        self.update_profile_record(expected_revision, profile)
    }

    pub(crate) fn restore_profile_record(
        &self,
        expected_revision: u64,
        mut profile: Profile,
    ) -> Result<Versioned<Profile>, Problem> {
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

    pub(crate) fn replace_profile_resource(&self, binding: ProfileResource) -> Result<(), Problem> {
        self.profile_resources
            .write()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?
            .insert(binding.profile_id.clone(), binding);
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

    pub fn recover_stale_sync_migrations(
        &self,
        stale_before_ms: u64,
    ) -> Result<MigrationRecoveryReport, Problem> {
        let _mutation = self
            .control_mutation
            .lock()
            .map_err(|_| problem("control_unavailable", "Control state unavailable", 503))?;
        let journals = self
            .control_persistence
            .list_versioned::<SyncMigrationJournal>(CONTROL_TENANT, "migration_sync_import")?;
        let aggregate = self.control_aggregate()?;
        let referenced_objects: BTreeSet<&str> = aggregate
            .sync_manifests
            .values()
            .flat_map(|manifest| manifest.objects.iter().map(|object| object.key.as_str()))
            .collect();
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
            if journal.state == SyncMigrationJournalState::Staging {
                report.deferred += 1;
                continue;
            }
            let key = format!("{}:{}", journal.workspace_id, journal.resource_id);
            if journal.state == SyncMigrationJournalState::Finalizing {
                if aggregate
                    .sync_manifests
                    .get(&key)
                    .is_some_and(|manifest| manifest.generation >= journal.target_generation)
                {
                    report.deferred += 1;
                    continue;
                }
                journal.state = SyncMigrationJournalState::CleanupPending;
                journal.error_code = Some("migration_recovery_cleanup_pending".into());
                journal.updated_at_ms = now().saturating_mul(1_000);
                if self
                    .persist_sync_migration_journal(
                        &journal_id,
                        revision,
                        format!("migration-recovery-mark:{journal_id}:{revision}"),
                        "migration-recovery-worker".into(),
                        &journal,
                    )
                    .is_ok()
                {
                    report.recovered += 1;
                } else {
                    report.failed += 1;
                }
                continue;
            }
            if journal
                .created_objects
                .iter()
                .any(|object| referenced_objects.contains(object.storage_key.as_str()))
            {
                report.deferred += 1;
                continue;
            }
            let cleanup = journal.created_objects.iter().try_for_each(|object| {
                self.delete_remote_chunk(&object.storage_key, &object.checksum)
            });
            if cleanup.is_err() {
                report.failed += 1;
                continue;
            }
            journal.state = SyncMigrationJournalState::Abandoned;
            journal.error_code = Some("migration_recovery_abandoned".into());
            journal.updated_at_ms = now().saturating_mul(1_000);
            if self
                .persist_sync_migration_journal(
                    &journal_id,
                    revision,
                    format!("migration-recovery-delete:{journal_id}:{revision}"),
                    "migration-recovery-worker".into(),
                    &journal,
                )
                .is_ok()
            {
                report.recovered += 1;
            } else {
                report.failed += 1;
            }
        }
        Ok(report)
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
        target: ControlAggregate,
        journal_id: &str,
        journal_revision: u64,
        journal: &SyncMigrationJournal,
        input_hash: &str,
        actor_id: String,
        detail: serde_json::Value,
        body: serde_json::Value,
    ) -> Result<buglogin_storage::control::ControlCommandResult, Problem> {
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
    pub fn with_migration_key(self, key: MigrationKey) -> Self {
        *self.migration_key.write().expect("migration key lock") = Some(key);
        self
    }

    pub fn with_device_verification_uri(mut self, uri: impl Into<String>) -> Result<Self, Problem> {
        let uri = uri.into();
        if uri.trim() != uri || uri.is_empty() || uri.bytes().any(|byte| byte.is_ascii_whitespace())
        {
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
        if self.local_account_provisioning {
            self.account_persistence
                .provision_default(account_id.clone(), now().to_string())?;
        } else if self.account_persistence.load(&account_id)?.is_none() {
            return Err(problem(
                "account_provisioning_disabled",
                "Account provisioning is disabled",
                403,
            ));
        }
        let identity =
            AccountIdentity::new(account_id.clone(), username, email, now().to_string())?;
        let identity = self.identity_persistence.create(&identity)?;
        self.credentials
            .set_initial_password(&account_id, password, now())?;
        Ok(identity)
    }

    pub fn with_migration_signing_key(self, key: buglogin_storage::secrets::SecretValue) -> Self {
        *self
            .migration_signing_key
            .write()
            .expect("migration signing key lock") = Some(key);
        self
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

    pub fn revoke_token(&self, token: &str) -> Result<(), Problem> {
        self.auth
            .write()
            .map_err(|_| problem("auth_unavailable", "Authentication unavailable", 503))?
            .revoke(token, now())
    }

    pub fn issue_token(&self, account_id: impl Into<String>) -> Result<String, Problem> {
        let account_id = buglogin_domain::ids::AccountId::new(account_id.into())
            .map_err(|_| problem("invalid_account", "Invalid account", 400))?;
        if self.local_account_provisioning {
            self.account_persistence
                .provision_default(account_id.clone(), now().to_string())?;
        } else if self.account_persistence.load(&account_id)?.is_none() {
            return Err(problem(
                "account_provisioning_disabled",
                "Account provisioning is disabled",
                403,
            ));
        }
        let expires_at = now().saturating_add(3600);
        let mut auth = self
            .auth
            .write()
            .map_err(|_| problem("auth_unavailable", "Authentication unavailable", 503))?;
        match self.credentials.password_version(&account_id)? {
            Some(version) => auth.issue_with_credential_version(account_id, expires_at, version),
            None => auth.issue(account_id, expires_at),
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WebLoginInput {
    identifier: String,
    password: String,
}

#[derive(Serialize)]
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RecoveryCompleteInput {
    token: String,
    new_password: String,
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DeviceAuthorizationApproveInput {
    user_code: String,
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Health {
    status: &'static str,
    service: &'static str,
}

async fn readyz(State(state): State<ApiState>) -> Result<Json<Health>, Response> {
    state.readiness().await.map_err(response)?;
    Ok(Json(Health {
        status: "ready",
        service: "buglogin-api",
    }))
}

pub fn coordination_authorizer(state: &ApiState) -> Arc<dyn CoordinationAuthorizer> {
    Arc::new(ApiCoordinationAuthorizer {
        auth: Arc::clone(&state.auth),
        grants: Arc::clone(&state.grants),
    })
}

struct ApiCoordinationAuthorizer {
    auth: Arc<RwLock<AuthService>>,
    grants: Arc<RwLock<Vec<Grant>>>,
}

#[async_trait]
impl CoordinationAuthorizer for ApiCoordinationAuthorizer {
    async fn authorize(
        &self,
        auth: &CoordinationAuth,
        request: AuthorizationRequest<'_>,
    ) -> Result<(), Problem> {
        let account_id = self
            .auth
            .read()
            .map_err(|_| {
                problem(
                    "authentication_unavailable",
                    "Authentication unavailable",
                    503,
                )
            })?
            .authenticate(auth.bearer_token(), now())?;
        let principal = Principal {
            account_id: Some(account_id),
            workspace_ids: BTreeSet::from([request.workspace_id.clone()]),
            roles: BTreeSet::from([Role::Account]),
            ..Principal::default()
        };
        let (capability, action) = match request.operation {
            CoordinationOperation::LeaseAcquire | CoordinationOperation::LeaseRenew => {
                ("profile.launch", Action::Execute)
            }
            CoordinationOperation::LeaseRelease => ("profile.stop", Action::Execute),
            CoordinationOperation::ManifestRead => ("profile.read", Action::Read),
            CoordinationOperation::ManifestWrite => ("profile.update", Action::Update),
            CoordinationOperation::PayloadWrite => ("profile.update", Action::Update),
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
        let grants = self.grants.read().map_err(|_| {
            problem(
                "permission_unavailable",
                "Permission service unavailable",
                503,
            )
        })?;
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
#[serde(rename_all = "camelCase")]
struct RevisionInput {
    revision: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LaunchInput {
    profiles_root: PathBuf,
    extra_args: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
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
        ]);
    Router::new()
        .route("/v1/auth/login", post(web_login))
        .route("/v1/auth/recovery/request", post(request_recovery))
        .route("/v1/auth/recovery/complete", post(complete_recovery))
        .route(
            "/v1/auth/api-tokens",
            get(list_api_tokens).post(create_api_token),
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
        .route("/v1/auth/device/start", post(start_device_authorization))
        .route(
            "/v1/auth/device/approve",
            post(approve_device_authorization),
        )
        .route("/v1/auth/device/token", post(exchange_device_authorization))
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
        .route("/v1/readyz", get(readyz))
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
        .route("/v1/profiles/{id}/launch", post(launch_profile))
        .route("/v1/profiles/{id}/stop", post(stop_profile))
        .route("/v1/profiles/{id}/cookies", get(list_cookies))
        .route("/v1/profiles/{id}/cookies/stage", post(stage_cookies))
        .route("/v1/profiles/{id}/cookies/import", post(import_cookies))
        .route("/v1/profiles/{id}/cookies/copy", post(copy_cookies))
        .route("/v1/cookie-jobs", post(enqueue_cookie_job))
        .route("/v1/cookie-jobs/{id}", get(get_cookie_job))
        .route("/v1/cookie-jobs/{id}/claim", post(claim_cookie_job))
        .route("/v1/cookie-jobs/{id}/ack", post(ack_cookie_job))
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

const WEB_SESSION_COOKIE: &str = "__Host-buglogin_session";
const WEB_CSRF_COOKIE: &str = "__Host-buglogin_csrf";
const WEB_SESSION_SECONDS: u64 = 12 * 60 * 60;

async fn create_api_token(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(input): Json<ApiTokenCreateInput>,
) -> Result<(StatusCode, Json<ApiTokenCreated>), Response> {
    let account_id = authenticated_browser_account_for_mutation(&state, &headers)?;
    let issued = state
        .api_tokens
        .issue(
            account_id,
            &input.name,
            input.scopes,
            input.expires_at,
            now(),
        )
        .map_err(response)?;
    Ok((
        StatusCode::CREATED,
        Json(ApiTokenCreated {
            token: issued.token,
            token_record: issued.summary,
        }),
    ))
}

async fn list_api_tokens(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<ApiTokenPage>, Response> {
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
        .authenticate_browser(raw_session, raw_csrf, now())
        .map_err(response)?;
    Ok(Json(ApiTokenPage {
        tokens: state.api_tokens.list(&account_id).map_err(response)?,
    }))
}

async fn revoke_api_token(
    State(state): State<ApiState>,
    Path(token_id): Path<String>,
    headers: HeaderMap,
) -> Result<StatusCode, Response> {
    required_idempotency(&headers)?;
    let account_id = authenticated_browser_account_for_mutation(&state, &headers)?;
    state
        .api_tokens
        .revoke(&account_id, &token_id, now())
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
        .request(&input.identifier, now())
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
        .complete(&input.token, &input.new_password, idempotency_key, now())
        .map_err(response)?;
    auth.mark_account_sessions_revoked(&account_id, now());
    Ok(StatusCode::NO_CONTENT)
}

async fn web_login(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(input): Json<WebLoginInput>,
) -> Result<Response, Response> {
    require_web_origin(&state, &headers)?;
    let current = now();
    let (account_id, credential_version) = state
        .credentials
        .authenticate_versioned(&input.identifier, &input.password, current)
        .map_err(response)?;
    let expires_at = current.saturating_add(WEB_SESSION_SECONDS);
    let issued = state
        .auth
        .write()
        .map_err(|_| {
            response(problem(
                "auth_unavailable",
                "Authentication unavailable",
                503,
            ))
        })?
        .issue_browser_with_credential_version(
            account_id.clone(),
            expires_at,
            current,
            credential_version,
        )
        .map_err(response)?;
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
    result
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(result)
}

async fn web_session(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<WebSessionResponse>, Response> {
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
        .browser_session(raw_session, raw_csrf, now())
        .map_err(response)?;
    Ok(Json(WebSessionResponse {
        account_id,
        expires_at,
    }))
}

async fn web_sessions(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<WebSessionPage>, Response> {
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
        .authenticate_browser(raw_session, raw_csrf, now())
        .map_err(response)?;
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
    Ok(Json(WebSessionPage { sessions }))
}

async fn revoke_web_session(
    State(state): State<ApiState>,
    Path(session_id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, Response> {
    required_idempotency(&headers)?;
    let account_id = authenticated_account_for_mutation(&state, &headers)?;
    let current_id = cookie(&headers, WEB_SESSION_COOKIE)
        .ok()
        .map(session_id_for_raw);
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
        .revoke_session(&account_id, &session_id, now())
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
    Json(input): Json<DeviceAuthorizationStartInput>,
) -> Result<Json<DeviceAuthorizationStartResponse>, Response> {
    let started = state
        .device_authorization
        .start(&input.device_id, &input.client_name, now())
        .map_err(response)?;
    Ok(Json(DeviceAuthorizationStartResponse {
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
) -> Result<Json<DeviceAuthorizationApproveResponse>, Response> {
    let (account_id, credential_version) =
        authenticated_account_and_credential_version_for_mutation(&state, &headers)?;
    state
        .device_authorization
        .approve_with_credential_version(&input.user_code, account_id, credential_version, now())
        .map_err(response)?;
    Ok(Json(DeviceAuthorizationApproveResponse { approved: true }))
}

async fn exchange_device_authorization(
    State(state): State<ApiState>,
    Json(input): Json<DeviceAuthorizationTokenInput>,
) -> Result<Json<AuthTokenResponse>, Response> {
    let current = now();
    match state
        .device_authorization
        .poll(&input.device_code, current)
        .map_err(response)?
    {
        crate::services::device_authorization::DevicePoll::Pending => Err(response(problem(
            "authorization_pending",
            "Authorization pending",
            400,
        ))),
        crate::services::device_authorization::DevicePoll::SlowDown { interval } => Err(response(
            problem("slow_down", "Polling too quickly", 400)
                .with_detail(format!("Retry after {interval} seconds")),
        )),
        crate::services::device_authorization::DevicePoll::Approved {
            account_id,
            device_id,
            credential_version,
        } => {
            let expires_at = current.saturating_add(3600);
            let mut auth = state.auth.write().map_err(|_| {
                response(problem(
                    "auth_unavailable",
                    "Authentication unavailable",
                    503,
                ))
            })?;
            let access_token = match credential_version {
                Some(version) => auth.issue_for_device_with_credential_version(
                    account_id, expires_at, device_id, version,
                ),
                None => auth.issue_for_device(account_id, expires_at, device_id),
            }
            .map_err(response)?;
            Ok(Json(AuthTokenResponse {
                access_token,
                token_type: "Bearer",
                expires_at,
            }))
        }
    }
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
) -> Result<Json<AuthTokenResponse>, Response> {
    let token = bearer_token(&headers)?;
    let current = now();
    let expires_at = current.saturating_add(3600);
    let replacement = state
        .refresh_token(token, current, expires_at)
        .map_err(response)?;
    Ok(Json(AuthTokenResponse {
        access_token: replacement,
        token_type: "Bearer",
        expires_at,
    }))
}

async fn logout(State(state): State<ApiState>, headers: HeaderMap) -> Result<Response, Response> {
    if headers.get(header::AUTHORIZATION).is_some() {
        state
            .revoke_token(bearer_token(&headers)?)
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
        .auth
        .read()
        .map_err(|_| {
            response(problem(
                "auth_unavailable",
                "Authentication unavailable",
                503,
            ))
        })?
        .authenticate_browser(raw_session, raw_csrf, now())
        .map_err(response)?;
    state.revoke_token(raw_session).map_err(response)?;
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
    headers: HeaderMap,
) -> Result<Json<Vec<buglogin_storage::local::Versioned<Profile>>>, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let profiles = state.list_profile_records().map_err(response)?;
    let mut filtered = Vec::new();
    for profile in profiles {
        if authorized(
            &state,
            &principal,
            &grants,
            &profile.value.id,
            Action::Read,
            "profile.read",
        )? {
            filtered.push(profile);
        }
    }
    Ok(Json(filtered))
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

async fn stage_cookies(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, Response> {
    let _ = (state, headers);
    Err(response(problem(
        "production_cookie_device_dispatch_required",
        "Cookie staging requires a connected authorized device",
        503,
    )))
}

async fn import_cookies(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, Response> {
    let _ = (state, headers);
    Err(response(problem(
        "production_cookie_device_dispatch_required",
        "Cookie import requires a connected authorized device",
        503,
    )))
}

async fn copy_cookies(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, Response> {
    let _ = (state, headers);
    Err(response(problem(
        "production_cookie_device_dispatch_required",
        "Cookie copy requires a connected authorized device",
        503,
    )))
}

async fn enqueue_cookie_job(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(mut input): Json<EnqueueConnectedCookieJob>,
) -> Result<Json<serde_json::Value>, Response> {
    let principal = authenticate(&state, &headers)?;
    let idempotency_key = required_idempotency(&headers)?.to_owned();
    input.idempotency_key = idempotency_key;
    input.requested_by = principal
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
    let job = state
        .connected_cookie_jobs
        .enqueue(input)
        .map_err(response)?;
    Ok(Json(serde_json::to_value(job).map_err(|_| {
        response(problem("serialization_failed", "Serialization failed", 500))
    })?))
}

async fn get_cookie_job(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    Query(query): Query<CookieJobWorkspaceQuery>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, Response> {
    let principal = authenticate(&state, &headers)?;
    let job = state
        .connected_cookie_jobs
        .get(
            &query.workspace_id,
            &id,
            &principal
                .account_id
                .ok_or_else(|| {
                    response(problem(
                        "authentication_required",
                        "Authentication required",
                        401,
                    ))
                })?
                .as_str()
                .to_owned(),
        )
        .map_err(response)?
        .ok_or_else(|| response(problem("cookie_job_not_found", "Cookie job not found", 404)))?;
    Ok(Json(serde_json::to_value(job).map_err(|_| {
        response(problem("serialization_failed", "Serialization failed", 500))
    })?))
}

async fn claim_cookie_job(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    Query(query): Query<CookieJobWorkspaceQuery>,
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
        .authenticate_device(token, now())
        .map_err(response)?;
    let job = state
        .connected_cookie_jobs
        .claim(
            &query.workspace_id,
            &id,
            account_id.as_str(),
            &device_id,
            now(),
        )
        .map_err(response)?;
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
        .authenticate_device(token, now())
        .map_err(response)?;
    let job = state
        .connected_cookie_jobs
        .acknowledge(
            &query.workspace_id,
            &id,
            account_id.as_str(),
            &device_id,
            now(),
        )
        .map_err(response)?;
    Ok(Json(serde_json::to_value(job).map_err(|_| {
        response(problem("serialization_failed", "Serialization failed", 500))
    })?))
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
        .authenticate_device(token, now())
        .map_err(response)?;
    if receipt.job_id != id || receipt.device_id != device_id {
        return Err(response(problem(
            "cookie_receipt_binding_invalid",
            "Cookie receipt binding invalid",
            409,
        )));
    }
    let job = state
        .connected_cookie_jobs
        .complete(
            &query.workspace_id,
            &id,
            account_id.as_str(),
            now(),
            receipt,
        )
        .map_err(response)?;
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
    Json(profile): Json<Profile>,
) -> Result<Response, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    authorize_action(
        &state,
        &principal,
        &grants,
        &profile.id,
        Action::Create,
        "profile.create",
    )?;
    if profile.sync_mode == Some(buglogin_domain::entities::SyncMode::Managed) {
        return control::provision_managed_profile(state, headers, principal, grants, profile)
            .await;
    }
    if !state.local_authority_enabled() {
        return Err(response(problem(
            "production_managed_profile_required",
            "Production profiles must use hosted managed authority",
            422,
        )));
    }
    state
        .create_profile_record(profile)
        .map(|value| (StatusCode::CREATED, Json(value)).into_response())
        .map_err(response)
}

async fn archive_profile(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    input: Option<Json<RevisionInput>>,
) -> Result<Response, Response> {
    mutate_profile(state, id, headers, input, true).await
}

async fn restore_profile(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    input: Option<Json<RevisionInput>>,
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
    input: Option<Json<RevisionInput>>,
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
        let input =
            input.ok_or_else(|| response(problem("invalid_input", "revision is required", 400)))?;
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
    let expected = input
        .map(|Json(value)| value.revision)
        .unwrap_or(current.revision);
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
    let operation_id = operation_id(&headers, "profile.launch")?;
    begin_operation(&state, &operation_id, "profile.launch")?;
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
        Ok(value) => finish_operation(&state, &operation_id, value),
        Err(error) => fail_operation(&state, &operation_id, error),
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
    let operation_id = operation_id(&headers, "profile.stop")?;
    begin_operation(&state, &operation_id, "profile.stop")?;
    let result = ProfileLifecycleService.stop(&state.profiles, &state.lifecycles, id);
    match result {
        Ok(value) => finish_operation(&state, &operation_id, value),
        Err(error) => fail_operation(&state, &operation_id, error),
    }
}

async fn delete_profile(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(input): Json<DeleteInput>,
) -> Result<Json<serde_json::Value>, Response> {
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
    if input.confirmation != "QUARANTINE_PROFILE_DATA" {
        return Err(response(problem(
            "delete_not_confirmed",
            "Profile deletion is not confirmed",
            409,
        )));
    }
    let current = state
        .profiles
        .get(&id)
        .map_err(response)?
        .ok_or_else(|| response(problem("profile_not_found", "Profile not found", 404)))?;
    let path = state
        .profiles
        .delete(input.revision, &current.value)
        .map_err(response)?;
    Ok(Json(serde_json::json!({
        "id": id,
        "deleted": true,
        "recoverable": true,
        "quarantinePath": path
    })))
}

async fn get_operation(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<crate::services::operations::Operation>, Response> {
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
struct EventQuery {
    after: Option<u64>,
    limit: Option<usize>,
}

async fn read_events(
    State(state): State<ApiState>,
    Query(query): Query<EventQuery>,
    headers: HeaderMap,
) -> Result<Json<Vec<crate::services::events::Event>>, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let scope = ResourceScope::global();
    let events = state
        .events
        .read()
        .map_err(|_| {
            response(problem(
                "event_unavailable",
                "Event service unavailable",
                503,
            ))
        })?
        .read_after(
            &principal,
            &grants,
            &scope,
            query.after,
            query.limit.unwrap_or(100),
        )
        .map_err(response)?
        .into_iter()
        .cloned()
        .collect();
    Ok(Json(events))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MigrationInspectInput {
    source_root: PathBuf,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
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
    let operation_id = operation_id(&headers, "migration.inspect")?;
    begin_operation(&state, &operation_id, "migration.inspect")?;
    let result = inspect_source(&input.source_root).and_then(|(report, snapshot)| {
        serde_json::to_value(MigrationInspectResult { report, snapshot })
            .map_err(|_| problem("migration_result_invalid", "Migration result invalid", 500))
    });
    match result {
        Ok(value) => finish_operation(&state, &operation_id, value),
        Err(error) => fail_operation(&state, &operation_id, error),
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
    let operation_id = operation_id(&headers, "migration.import")?;
    if let Some(operation) = existing_operation(&state, &operation_id)? {
        return Ok((StatusCode::ACCEPTED, Json(operation)));
    }
    begin_operation(&state, &operation_id, "migration.import")?;
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
        Ok(value) => finish_operation(&state, &operation_id, value),
        Err(error) => fail_operation(&state, &operation_id, error),
    }
}

fn existing_operation(state: &ApiState, operation_id: &str) -> Result<Option<Operation>, Response> {
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

fn operation_id(headers: &HeaderMap, command: &str) -> Result<String, Response> {
    use sha2::{Digest, Sha256};
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
    let digest = format!(
        "{:x}",
        Sha256::digest(format!("{command}:{key}").as_bytes())
    );
    Ok(format!("op_{}", &digest[..24]))
}

fn begin_operation(state: &ApiState, operation_id: &str, command_id: &str) -> Result<(), Response> {
    let mut operations = state.operations.write().map_err(|_| {
        response(problem(
            "operation_unavailable",
            "Operation service unavailable",
            503,
        ))
    })?;
    operations
        .insert(
            Operation::queued(operation_id, command_id, ResourceScope::global())
                .map_err(response)?,
        )
        .map_err(response)?;
    operations
        .transition(
            operation_id,
            OperationStatus::Running,
            Some(0.1),
            None,
            None,
        )
        .map_err(response)?;
    Ok(())
}

fn finish_operation(
    state: &ApiState,
    operation_id: &str,
    result: serde_json::Value,
) -> Result<(StatusCode, Json<Operation>), Response> {
    let operation = state
        .operations
        .write()
        .map_err(|_| {
            response(problem(
                "operation_unavailable",
                "Operation service unavailable",
                503,
            ))
        })?
        .transition(
            operation_id,
            OperationStatus::Completed,
            Some(1.0),
            Some(result),
            None,
        )
        .map_err(response)?
        .clone();
    Ok((StatusCode::ACCEPTED, Json(operation)))
}

fn fail_operation(
    state: &ApiState,
    operation_id: &str,
    error: Problem,
) -> Result<(StatusCode, Json<Operation>), Response> {
    let operation = state
        .operations
        .write()
        .map_err(|_| {
            response(problem(
                "operation_unavailable",
                "Operation service unavailable",
                503,
            ))
        })?
        .transition(
            operation_id,
            OperationStatus::Failed,
            Some(1.0),
            None,
            Some(error.clone()),
        )
        .map_err(response)?
        .clone();
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

fn correlation(headers: &HeaderMap) -> Result<String, Response> {
    headers
        .get("x-correlation-id")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
        .ok_or_else(|| {
            response(problem(
                "correlation_required",
                "Correlation ID is required",
                400,
            ))
        })
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

fn cookie_problem_api(title: String, correlation_id: &str) -> Problem {
    Problem::new("cookie_command_failed", title, 409, correlation_id)
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

fn cookie_value(value: String) -> Result<HeaderValue, Response> {
    HeaderValue::from_str(&value).map_err(|_| {
        response(problem(
            "session_cookie_failed",
            "Session cookie could not be created",
            500,
        ))
    })
}

fn authenticate(state: &ApiState, headers: &HeaderMap) -> Result<Principal, Response> {
    state
        .authenticate_token(bearer_token(headers)?, now())
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
        .authenticate_browser(raw_session, raw_csrf, now())
        .map_err(response)
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
        .authenticate_browser(raw_session, raw_csrf, now())
        .map_err(response)
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
                .authenticate_with_credential_version(token, now())
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
        .authenticate_browser_with_credential_version(raw_session, raw_csrf, now())
        .map_err(response)?;
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
        .get(profile_id)
        .cloned();
    let scope = match &binding {
        Some(binding) => ResourceScope {
            workspace_id: Some(binding.workspace_id.clone()),
            resource_id: Some(binding.resource_id.clone()),
            profile_id: Some(profile_id.clone()),
            ..ResourceScope::default()
        },
        None => ResourceScope {
            profile_id: Some(profile_id.clone()),
            ..ResourceScope::default()
        },
    };
    let capability = Capability::new(capability)
        .map_err(|_| response(problem("invalid_capability", "Invalid capability", 500)))?;
    if is_allowed(principal, &capability, action, &scope, grants) {
        return Ok(true);
    }
    let Some(binding) = binding else {
        return Ok(false);
    };
    let Some(account_id) = principal.account_id.as_ref() else {
        return Ok(false);
    };
    let member_id = state
        .members
        .read()
        .map_err(|_| {
            response(problem(
                "member_unavailable",
                "Member state unavailable",
                503,
            ))
        })?
        .active_for_account(account_id)
        .into_iter()
        .find(|member| member.workspace_id == binding.workspace_id)
        .map(|member| member.id);
    let Some(member_id) = member_id else {
        return Ok(false);
    };
    Ok(state
        .resources
        .read()
        .map_err(|_| {
            response(problem(
                "resource_unavailable",
                "Resource state unavailable",
                503,
            ))
        })?
        .is_allowed(&binding.resource_id, &member_id, action))
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
) -> Result<Response, StatusCode> {
    if headers.get("x-correlation-id").is_none()
        && !matches!(request.uri().path(), "/v1/health" | "/v1/readyz")
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(next.run(request).await)
}

fn response(problem: Problem) -> Response {
    let status = StatusCode::from_u16(problem.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, Json(problem)).into_response()
}

fn runtime_key(runtime: &Runtime) -> String {
    serde_json::to_string(runtime).unwrap_or_else(|_| "unknown".to_owned())
}

fn sync_manifest_key(
    workspace_id: &buglogin_domain::ids::WorkspaceId,
    resource_id: &buglogin_domain::ids::ResourceId,
) -> String {
    format!("{}:{}", workspace_id.as_str(), resource_id.as_str())
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
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|_| {
            problem(
                "migration_timestamp_invalid",
                "Migration timestamp invalid",
                500,
            )
        })
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or(0)
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
        remote::{MemoryRemoteChunkStore, RemoteChunk, RemoteChunkStore},
    };
    use std::{
        fs,
        path::PathBuf,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
    };

    fn test_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "buglogin-api-persistence-{label}-{}",
            std::process::id()
        ))
    }

    struct DeleteFailStore {
        inner: MemoryRemoteChunkStore,
    }

    impl RemoteChunkStore for DeleteFailStore {
        fn checksum(&self, object_key: &str) -> Result<Option<String>, Problem> {
            self.inner.checksum(object_key)
        }

        fn put(
            &mut self,
            object_key: &str,
            bytes: &[u8],
            expected_checksum: &str,
        ) -> Result<(), Problem> {
            self.inner.put(object_key, bytes, expected_checksum)
        }

        fn get(&self, object_key: &str, expected_checksum: &str) -> Result<RemoteChunk, Problem> {
            self.inner.get(object_key, expected_checksum)
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

        let mut remote = MemoryRemoteChunkStore::default();
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
    fn recovery_delete_failure_preserves_cleanup_pending_journal_and_object() {
        let root = test_root("recovery-delete-failure");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let object_key = "profiles/res_recovery_failure/1/checksum/profile-state";
        let object_bytes = b"preserve-on-delete-failure";
        let object_checksum = checksum(object_bytes);
        let mut remote = MemoryRemoteChunkStore::default();
        remote
            .put(object_key, object_bytes, &object_checksum)
            .unwrap();
        let state = ApiState::try_with_account_store(
            ProfileService::new(LocalRepository::open(&root).unwrap()),
            MemoryControlRecordStore::default(),
            false,
        )
        .unwrap()
        .with_remote_chunks(Box::new(DeleteFailStore { inner: remote }));
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
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn cleanup_pending_sync_journal_defers_referenced_object() {
        use buglogin_domain::ids::{ResourceId, WorkspaceId};
        use buglogin_sync::manifest::{ManifestObject, SyncManifest};

        let root = test_root("recovery-referenced-object");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let object_key = "profiles/res_recovery_referenced/1/checksum/profile-state";
        let object_bytes = b"active-profile-state";
        let object_checksum = checksum(object_bytes);
        let mut remote = MemoryRemoteChunkStore::default();
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
        let workspace = WorkspaceId::new("ws_recovery_referenced").unwrap();
        let resource = ResourceId::new("res_recovery_referenced").unwrap();
        let manifest = SyncManifest::new(
            workspace.clone(),
            resource.clone(),
            13,
            1,
            None,
            b"active-root",
            vec![ManifestObject::new(object_key, object_bytes).unwrap()],
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
                storage_key: object_key.into(),
                checksum: object_checksum.clone(),
                size: object_bytes.len() as u64,
            }],
            resumed_objects: vec![],
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
                .get_remote_chunk(object_key, &object_checksum)
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
