use std::{
    collections::{BTreeMap, BTreeSet, HashMap, VecDeque},
    fmt, fs,
    future::Future,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use crate::clock::{checked_add_seconds, unix_time_millis, unix_time_seconds};
use buglogin_api::{
    commands::CommandResult,
    services::{
        connected_cookie_jobs::{
            ConnectedCookieFailureCode, ConnectedCookieJobState, ConnectedCookieReceipt,
        },
        operations::{Operation, OperationService, OperationStatus},
        runtime_dispatch::{RuntimeDispatchJob, RuntimeDispatchState},
    },
};
use buglogin_browser::{
    cookies::{
        CookieInput, CookieSameSite, CookieScope,
        authority::{
            CookieAuthority, CookieMutationResult, HostedCookieJobBinding, IdempotencyKey,
            ProfileRuntimeState, StagedImportId,
        },
    },
    process::SystemProcessController,
    runtime::catalog::RuntimeCatalog,
};
use buglogin_domain::{
    entities::{ProfileState, Runtime, SyncMode},
    errors::Problem,
    ids::{AccountId, CredentialId, DeviceId, GrantId, ProfileId, ProxyId, WorkspaceId},
    permissions::{Action, Capability, Effect, Grant, Principal, ResourceScope},
    profile::{
        ConfirmedProfileKeyShareEnvelope, ProfileArchiveState, ProfileKeyRotationEnvelopeUpdate,
        ProfileShareInput,
    },
};
use buglogin_network::proxy::{ProxyRow, parse_lines};
use buglogin_storage::{
    secrets::{SecretValue, SecretVersion},
    vault::NativeVault,
};
use buglogin_sync::{
    coordination::{CoordinationAuth, CoordinationScope, LeaseGrant},
    payload::PayloadDescriptor,
};
use futures_util::{StreamExt, TryStreamExt, stream};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex as AsyncMutex, Notify};
use zeroize::{Zeroize, Zeroizing};

use crate::{
    adapter::{AdapterState, RuntimeTarget, RuntimeTargets, push_or_replace_arg},
    auth_vault::AuthVault,
    device_key_vault::DeviceKeyVault,
    profile_projection::{HydratedProxyCredential, ProfileProjection, SnapshotBlob},
    profile_runtime::{
        DesktopSyncBackend, LaunchRequest, LeaseScope, ManagedCoordination, ProfileRuntimeHandle,
        StopReceipt, spawn_profile_runtime,
    },
    proxy_bridge::{ProxyBridgeHandle, ProxyBridgeSpec},
    proxy_credential_vault::ProxyCredentialVault,
    proxy_store::{ImportedProxy, ProxyImportReport},
    remote_api::{
        ApiTokenPage, BugLabSsoStartResponse, CreatedApiToken, DeviceSessionTokens,
        FeedbackReceipt, FeedbackSubmission, RemoteApiClient,
    },
    sync_key_vault::SyncKeyVault,
    wayfern_fingerprint::{
        apply_or_generate as apply_wayfern_fingerprint, clear_stale_cdp_marker,
        configure_local_cdp_args, target_from_runtime_config,
    },
};

const MAX_CONCURRENT_PROFILE_BLOB_UPLOADS: usize = 2;
const MAX_OPERATION_IDEMPOTENCY_RECORDS: usize = 1_024;
const PROXY_MUTATION_JOURNAL_SCHEMA_VERSION: u16 = 1;

#[derive(Clone)]
pub enum DesktopState {
    Local(AdapterState),
    Production(ProductionDesktopState),
    Unavailable(Problem),
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopIntegrationStatus {
    pub deployment_mode: &'static str,
    pub api_endpoint: Option<String>,
    pub api_reachable: bool,
    pub api_service: Option<String>,
    pub mcp_endpoint: Option<String>,
    pub mcp_enabled: Option<bool>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct WorkspaceSelectionStore {
    accounts: BTreeMap<String, String>,
    legacy_workspace_id: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WorkspaceSelectionDocument {
    schema_version: u8,
    accounts: BTreeMap<String, String>,
}

impl WorkspaceSelectionStore {
    fn load(path: &std::path::Path) -> Self {
        let Ok(contents) = fs::read_to_string(path) else {
            return Self::default();
        };
        Self::from_contents(&contents)
    }

    fn from_contents(contents: &str) -> Self {
        if let Ok(document) = serde_json::from_str::<WorkspaceSelectionDocument>(&contents)
            && document.schema_version == 1
        {
            return Self {
                accounts: document.accounts,
                legacy_workspace_id: None,
            };
        }
        let legacy_workspace_id = contents.trim();
        Self {
            accounts: BTreeMap::new(),
            legacy_workspace_id: (!legacy_workspace_id.is_empty())
                .then(|| legacy_workspace_id.to_owned()),
        }
    }

    fn resolve_for_account(
        &mut self,
        account_id: &str,
        authorized_workspace_ids: &[String],
    ) -> Option<String> {
        let authorized = |workspace_id: &str| {
            authorized_workspace_ids
                .iter()
                .any(|authorized_id| authorized_id == workspace_id)
        };
        let selected = self
            .accounts
            .get(account_id)
            .filter(|workspace_id| authorized(workspace_id))
            .cloned()
            .or_else(|| {
                self.legacy_workspace_id
                    .as_ref()
                    .filter(|workspace_id| authorized(workspace_id))
                    .cloned()
            })
            .or_else(|| authorized_workspace_ids.iter().min().cloned());
        if let Some(workspace_id) = selected.as_ref() {
            self.accounts
                .insert(account_id.to_owned(), workspace_id.clone());
            self.legacy_workspace_id = None;
        }
        selected
    }

    fn persist(&self, path: &std::path::Path) -> std::io::Result<()> {
        use std::io::Write;

        let document = WorkspaceSelectionDocument {
            schema_version: 1,
            accounts: self.accounts.clone(),
        };
        let bytes = serde_json::to_vec(&document).map_err(std::io::Error::other)?;
        let temporary = path.with_extension("selection.tmp");
        let mut file = fs::File::create(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(temporary, path)
    }
}

#[derive(Clone)]
pub struct ProductionDesktopState {
    client: RemoteApiClient,
    auth: Arc<RwLock<Option<CoordinationAuth>>>,
    device_id: String,
    auth_vault: Arc<Mutex<AuthVault>>,
    auth_vault_version: Arc<Mutex<Option<SecretVersion>>>,
    auth_refresh_token: Arc<Mutex<Option<Zeroizing<String>>>>,
    auth_expires_at: Arc<Mutex<u64>>,
    auth_refresh_uncertain: Arc<Mutex<bool>>,
    auth_refresh_gate: Arc<AsyncMutex<()>>,
    projection: ProfileProjection,
    device_key_vault: Arc<DeviceKeyVault>,
    device_key_enrolled: Arc<AtomicBool>,
    sync_key_vault: Arc<SyncKeyVault>,
    proxy_credential_vault: Arc<ProxyCredentialVault>,
    cookies: Arc<Mutex<CookieAuthority<NativeVault>>>,
    proxy_mutation_gate: Arc<AsyncMutex<()>>,
    workspace_selections: Arc<Mutex<WorkspaceSelectionStore>>,
    selected_workspace_path: PathBuf,
    profile_runtime: ProfileRuntimeHandle,
    runtime_targets: Arc<RwLock<RuntimeTargets>>,
    operations: Arc<RwLock<OperationService>>,
    idempotency: Arc<Mutex<BTreeMap<String, IdempotencyRecord>>>,
    idempotency_order: Arc<Mutex<VecDeque<String>>>,
    operation_gate: Arc<Mutex<OperationGate>>,
    operation_drained: Arc<Notify>,
    profile_operations: Arc<ProfileOperationLocks>,
    exit_reconcile_backoff: Arc<Mutex<ExitReconcileBackoff>>,
    runtime_dispatch_attempts: Arc<Mutex<HashMap<String, u32>>>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProxyCredentialInput {
    username: String,
    password: String,
}

impl Drop for ProxyCredentialInput {
    fn drop(&mut self) {
        self.zeroize_credentials();
    }
}

impl ProxyCredentialInput {
    fn zeroize_credentials(&mut self) {
        self.username.zeroize();
        self.password.zeroize();
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CookieStageCommandInput {
    workspace_id: String,
    profile_id: String,
    cookies: Vec<CookieStageCommandItem>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CookieStageCommandItem {
    name: String,
    domain: String,
    path: String,
    secure: bool,
    http_only: bool,
    expires_at: Option<i64>,
    same_site: CookieSameSite,
    value: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CookieImportCommandInput {
    workspace_id: String,
    profile_id: String,
    stage_id: String,
    expected_generation: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CookieCopyCommandInput {
    source_workspace_id: String,
    source_profile_id: String,
    destination_workspace_id: String,
    destination_profile_id: String,
    expected_generation: u64,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PendingProxyMutation {
    schema_version: u16,
    command: String,
    idempotency_key: String,
    request_hash: String,
    wire_hash: String,
    payload: Value,
    credential_bytes: Option<SensitiveBytes>,
    previous_revision: Option<u64>,
    clear_credential: bool,
}

#[derive(Deserialize, Serialize)]
#[serde(transparent)]
struct SensitiveBytes(Vec<u8>);

impl SensitiveBytes {
    fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl Drop for SensitiveBytes {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

struct SensitiveJson(Value);

impl SensitiveJson {
    fn new(value: Value) -> Self {
        Self(value)
    }

    fn as_value(&self) -> &Value {
        &self.0
    }

    fn into_value(mut self) -> Value {
        std::mem::replace(&mut self.0, Value::Null)
    }
}

impl std::ops::Deref for SensitiveJson {
    type Target = Value;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for SensitiveJson {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Drop for SensitiveJson {
    fn drop(&mut self) {
        zeroize_json_strings(&mut self.0);
    }
}

fn zeroize_json_strings(value: &mut Value) {
    match value {
        Value::String(value) => value.zeroize(),
        Value::Array(values) => values.iter_mut().for_each(zeroize_json_strings),
        Value::Object(values) => values.values_mut().for_each(zeroize_json_strings),
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProxyParseInput {
    text: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProxyImportInput {
    workspace_id: String,
    text: String,
    created_at: String,
}

impl Drop for ProxyImportInput {
    fn drop(&mut self) {
        self.zeroize_text();
    }
}

impl ProxyImportInput {
    fn zeroize_text(&mut self) {
        self.text.zeroize();
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProxyCheckInput {
    workspace_id: String,
    proxy_id: String,
}

#[derive(Clone, Eq, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum DesktopAuthStatus {
    AuthNotApplicable,
    Unauthenticated,
    Authenticated,
}

impl fmt::Debug for DesktopAuthStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AuthNotApplicable => formatter.write_str("AuthNotApplicable"),
            Self::Unauthenticated => formatter.write_str("Unauthenticated"),
            Self::Authenticated => formatter.write_str("Authenticated"),
        }
    }
}

#[derive(Clone)]
struct IdempotencyRecord {
    command_id: &'static str,
    payload_hash: String,
    operation_id: String,
    accepted: Value,
    completed: bool,
}

/// One launch/stop/sync at a time per profile: local clicks, runtime dispatch
/// jobs, the exit reconcile and shutdown otherwise race on the same lease and
/// manifest generation.
#[derive(Default)]
struct ProfileOperationLocks {
    locks: Mutex<HashMap<ProfileId, Arc<AsyncMutex<()>>>>,
}

impl ProfileOperationLocks {
    fn lock_for(&self, profile_id: &ProfileId) -> Arc<AsyncMutex<()>> {
        let mut locks = self
            .locks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        locks.entry(profile_id.clone()).or_default().clone()
    }

    fn is_busy(&self, profile_id: &ProfileId) -> bool {
        self.lock_for(profile_id).try_lock().is_err()
    }
}

/// Exponential backoff (3 s doubling to 5 min) for re-stopping profiles whose
/// browser exited on its own, so a sync that keeps failing isn't retried every tick.
#[derive(Default)]
struct ExitReconcileBackoff {
    attempts: HashMap<ProfileId, (u32, std::time::Instant)>,
}

impl ExitReconcileBackoff {
    fn ready(&self, profile_id: &ProfileId, now: std::time::Instant) -> bool {
        self.attempts
            .get(profile_id)
            .is_none_or(|(_, next_attempt)| now >= *next_attempt)
    }

    fn record_attempt(&mut self, profile_id: &ProfileId, now: std::time::Instant) {
        let attempts = self.attempts.get(profile_id).map_or(0, |(count, _)| *count) + 1;
        let delay = Duration::from_secs(3)
            .saturating_mul(1 << attempts.saturating_sub(1).min(16))
            .min(Duration::from_secs(300));
        self.attempts
            .insert(profile_id.clone(), (attempts, now + delay));
    }

    fn retain_exited(&mut self, exited: &[ProfileId]) {
        self.attempts
            .retain(|profile_id, _| exited.contains(profile_id));
    }
}

#[derive(Default)]
struct OperationGate {
    draining: bool,
    in_flight: usize,
}

struct OperationGuard {
    gate: Arc<Mutex<OperationGate>>,
    drained: Arc<Notify>,
}

fn make_idempotency_room(
    records: &mut BTreeMap<String, IdempotencyRecord>,
    insertion_order: &mut VecDeque<String>,
    correlation_id: &str,
) -> Result<(), Problem> {
    if records.len() < MAX_OPERATION_IDEMPOTENCY_RECORDS {
        return Ok(());
    }
    let Some(index) = insertion_order
        .iter()
        .position(|key| records.get(key).is_some_and(|record| record.completed))
    else {
        return Err(Problem::new(
            "desktop_idempotency_capacity",
            "All desktop operation idempotency slots are still active",
            503,
            correlation_id,
        )
        .retryable(true));
    };
    if let Some(key) = insertion_order.remove(index) {
        records.remove(&key);
    }
    Ok(())
}

impl Drop for OperationGuard {
    fn drop(&mut self) {
        if let Ok(mut gate) = self.gate.lock() {
            gate.in_flight = gate.in_flight.saturating_sub(1);
            if gate.in_flight == 0 {
                self.drained.notify_waiters();
            }
        }
    }
}

impl ProductionDesktopState {
    #[allow(clippy::too_many_arguments)]
    pub fn open(
        api_base_url: &str,
        token: Option<String>,
        device_id: String,
        profiles_root: PathBuf,
        runtime_targets: RuntimeTargets,
    ) -> Result<Self, Problem> {
        let selected_workspace_path = profiles_root.join(".buglogin-workspace-selection");
        let workspace_selections = WorkspaceSelectionStore::load(&selected_workspace_path);
        let auth_vault = AuthVault::native();
        let stored = auth_vault.load_session(&device_id, "desktop-bootstrap")?;
        let auth_vault_version = if token.is_none() {
            stored.as_ref().map(|value| value.version())
        } else {
            None
        };
        let stored_refresh_token = if token.is_none() {
            stored.as_ref().and_then(|value| {
                value
                    .refresh_token()
                    .map(|token| Zeroizing::new(token.to_owned()))
            })
        } else {
            None
        };
        let stored_expires_at = if token.is_none() {
            stored.as_ref().map_or(0, |value| value.expires_at())
        } else {
            0
        };
        let stored_refresh_pending = if token.is_none() {
            stored.as_ref().is_some_and(|value| value.refresh_pending())
        } else {
            false
        };
        let stored_access_token = if token.is_none() {
            stored
                .as_ref()
                .and_then(|value| value.access_token_for_resume().map(str::to_owned))
        } else {
            None
        };
        let token = token.or(stored_access_token);
        let (client, auth) = production_transport(api_base_url, token)?;
        let cookie_authority =
            CookieAuthority::open(NativeVault::new(), profiles_root.join(".cookie-authority"))
                .map_err(|error| error.as_problem("desktop-cookie-authority"))?;
        let approved = runtime_targets.approved_executables()?;
        let coordination = auth.clone().map(|auth| ManagedCoordination {
            backend: Arc::new(client.clone()),
            auth,
            lease_ttl: Duration::from_secs(60),
            heartbeat_interval: Duration::from_secs(15),
        });
        let profile_runtime =
            spawn_profile_runtime(SystemProcessController::new(approved), coordination);
        let state = Self {
            client,
            auth: Arc::new(RwLock::new(auth)),
            device_id,
            auth_vault: Arc::new(Mutex::new(auth_vault)),
            auth_vault_version: Arc::new(Mutex::new(auth_vault_version)),
            auth_refresh_token: Arc::new(Mutex::new(stored_refresh_token)),
            auth_expires_at: Arc::new(Mutex::new(stored_expires_at)),
            auth_refresh_uncertain: Arc::new(Mutex::new(stored_refresh_pending)),
            auth_refresh_gate: Arc::new(AsyncMutex::new(())),
            projection: ProfileProjection::open(profiles_root)?,
            device_key_vault: Arc::new(DeviceKeyVault::native()),
            device_key_enrolled: Arc::new(AtomicBool::new(false)),
            sync_key_vault: Arc::new(SyncKeyVault::native()),
            proxy_credential_vault: Arc::new(ProxyCredentialVault::native()),
            cookies: Arc::new(Mutex::new(cookie_authority)),
            proxy_mutation_gate: Arc::new(AsyncMutex::new(())),
            workspace_selections: Arc::new(Mutex::new(workspace_selections)),
            selected_workspace_path,
            profile_runtime,
            runtime_targets: Arc::new(RwLock::new(runtime_targets)),
            operations: Arc::new(RwLock::new(OperationService::with_capacity(
                MAX_OPERATION_IDEMPOTENCY_RECORDS,
            )?)),
            idempotency: Arc::new(Mutex::new(BTreeMap::new())),
            idempotency_order: Arc::new(Mutex::new(VecDeque::new())),
            operation_gate: Arc::new(Mutex::new(OperationGate::default())),
            operation_drained: Arc::new(Notify::new()),
            profile_operations: Arc::new(ProfileOperationLocks::default()),
            exit_reconcile_backoff: Arc::new(Mutex::new(ExitReconcileBackoff::default())),
            runtime_dispatch_attempts: Arc::new(Mutex::new(HashMap::new())),
        };
        Ok(state)
    }

    pub fn start_background_auth_refresh(&self) {
        let refresh_state = self.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(30)).await;
                let _ = refresh_state
                    .refresh_session_if_needed("desktop-auth-refresh")
                    .await;
            }
        });

        let inbox_state = self.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(30)).await;
                let _ = inbox_state
                    .refresh_session_if_needed("desktop-cookie-inbox")
                    .await;
                if !inbox_state.client.has_token().unwrap_or(false) {
                    continue;
                }
                inbox_state.poll_cookie_job_inbox().await;
            }
        });

        // Launch/stop requests from web or MCP wait on this poll, so it runs on its own
        // shorter interval and is never queued behind a long cookie job.
        let dispatch_state = self.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::time::sleep(RUNTIME_DISPATCH_POLL_INTERVAL).await;
                let _ = dispatch_state
                    .refresh_session_if_needed("desktop-runtime-dispatch")
                    .await;
                if !dispatch_state.client.has_token().unwrap_or(false) {
                    continue;
                }
                dispatch_state.poll_runtime_dispatch_inbox().await;
            }
        });

        let runtime_state = self.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(3)).await;
                if runtime_state.client.has_token().unwrap_or(false) {
                    runtime_state.reconcile_exited_browser_profiles().await;
                }
            }
        });
    }

    async fn reconcile_exited_browser_profiles(&self) {
        let operation_busy = || {
            self.operation_gate
                .lock()
                .map(|gate| gate.draining || gate.in_flight > 0)
                .unwrap_or(true)
        };
        if operation_busy() {
            return;
        }
        let Ok(active) = self.profile_runtime.active_profiles().await else {
            return;
        };
        let Ok(exited) = self.profile_runtime.unexpectedly_stopped_profiles().await else {
            return;
        };
        if let Ok(mut backoff) = self.exit_reconcile_backoff.lock() {
            backoff.retain_exited(&exited);
        }
        if exited.is_empty() {
            return;
        }
        let Ok(timestamp) = unix_time_seconds("desktop-runtime-reconcile") else {
            return;
        };
        for profile_id in exited {
            if active.contains(&profile_id) || self.profile_operations.is_busy(&profile_id) {
                continue;
            }
            let due = match self.exit_reconcile_backoff.lock() {
                Ok(mut backoff) => {
                    let now = std::time::Instant::now();
                    let due = backoff.ready(&profile_id, now);
                    if due {
                        backoff.record_attempt(&profile_id, now);
                    }
                    due
                }
                Err(_) => return,
            };
            if !due {
                continue;
            }
            if operation_busy() {
                return;
            }
            let profile_key = profile_id.as_str().replace(':', "-");
            let idempotency_key = format!("runtime-reconcile-{profile_key}-{timestamp}");
            let correlation_id = format!("runtime-reconcile-{profile_key}-{timestamp}");
            let _ = self
                .invoke(
                    "profile_stop",
                    &correlation_id,
                    Some(&idempotency_key),
                    json!({"profileId": profile_id}),
                )
                .await;
        }
    }

    async fn poll_runtime_dispatch_inbox(&self) {
        let Ok(timestamp) = unix_time_seconds("desktop-runtime-dispatch") else {
            return;
        };
        let correlation_id = format!("runtime-dispatch-{timestamp}");
        let Ok(jobs) = self.client.runtime_dispatch_inbox(&correlation_id).await else {
            return;
        };
        for job in jobs {
            if job.device_id != self.device_id
                || !matches!(
                    job.state,
                    RuntimeDispatchState::Accepted
                        | RuntimeDispatchState::Dispatched
                        | RuntimeDispatchState::Running
                )
                || !matches!(job.command_id.as_str(), "profile.launch" | "profile.stop")
            {
                continue;
            }
            self.execute_runtime_dispatch_job(job, &correlation_id)
                .await;
        }
    }

    async fn execute_runtime_dispatch_job(&self, job: RuntimeDispatchJob, correlation_id: &str) {
        let Ok(workspace_id) = WorkspaceId::new(job.workspace_id.clone()) else {
            return;
        };
        let Ok(profile_id) = parse_profile_id(&job.profile_id, correlation_id) else {
            return;
        };
        let Ok(claimed) = self
            .client
            .claim_runtime_dispatch(&workspace_id, &job.job_id, correlation_id)
            .await
        else {
            return;
        };
        if claimed.device_id != self.device_id
            || claimed.profile_id != profile_id.as_str()
            || claimed.command_id != job.command_id
            || claimed.operation_id != job.operation_id
            || !matches!(claimed.state, RuntimeDispatchState::Running)
        {
            return;
        }

        let attempt = self
            .runtime_dispatch_attempts
            .lock()
            .map(|mut attempts| {
                let attempt = attempts.entry(claimed.job_id.clone()).or_insert(0);
                *attempt += 1;
                *attempt
            })
            .unwrap_or(RUNTIME_DISPATCH_MAX_ATTEMPTS);
        let out_of_attempts = attempt >= RUNTIME_DISPATCH_MAX_ATTEMPTS;
        let local_launch_receipt = if claimed.command_id == "profile.launch" {
            self.profile_runtime
                .active_profiles()
                .await
                .ok()
                .is_some_and(|profiles| profiles.iter().any(|running| running == &profile_id))
        } else {
            false
        };
        let execution = if local_launch_receipt {
            Ok(None)
        } else {
            let mut payload = json!({"profileId": profile_id});
            if claimed.command_id == "profile.launch" {
                payload["headless"] = json!(
                    claimed
                        .input
                        .as_ref()
                        .and_then(|input| input.get("headless"))
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                );
            }
            let invoke_name = if claimed.command_id == "profile.launch" {
                "profile_launch"
            } else {
                "profile_stop"
            };
            let idempotency_key = runtime_dispatch_operation_key(&claimed.job_id, attempt);
            self.invoke(invoke_name, correlation_id, Some(&idempotency_key), payload)
                .await
                .and_then(|accepted| {
                    accepted
                        .data
                        .get("operationId")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                        .ok_or_else(|| {
                            Problem::new(
                                "runtime_dispatch_operation_missing",
                                "Runtime operation was not accepted",
                                502,
                                correlation_id,
                            )
                        })
                })
                .map(Some)
        };

        match execution {
            Ok(operation_id) => {
                let operation_result = match operation_id {
                    Some(operation_id) => {
                        self.wait_runtime_dispatch_operation(&operation_id, correlation_id)
                            .await
                    }
                    None => Ok(()),
                };
                let execution = match operation_result {
                    Ok(()) => {
                        self.verify_runtime_dispatch_state(
                            &claimed.command_id,
                            &profile_id,
                            correlation_id,
                        )
                        .await
                    }
                    Err(error) => Err(error),
                };
                if let Err(error) = execution {
                    if !error.retryable || out_of_attempts {
                        self.fail_runtime_dispatch_job(
                            &workspace_id,
                            &claimed.job_id,
                            &error,
                            correlation_id,
                        )
                        .await;
                    }
                    return;
                }
                let state = if claimed.command_id == "profile.launch" {
                    "running"
                } else {
                    "stopped"
                };
                let receipt = json!({
                    "jobId": claimed.job_id,
                    "operationId": claimed.operation_id,
                    "deviceId": self.device_id,
                    "profileId": claimed.profile_id,
                    "commandId": claimed.command_id,
                    "state": state,
                });
                match self
                    .client
                    .complete_runtime_dispatch(
                        &workspace_id,
                        &claimed.job_id,
                        receipt,
                        correlation_id,
                    )
                    .await
                {
                    Ok(_) => self.forget_runtime_dispatch_attempts(&claimed.job_id),
                    Err(error) => eprintln!(
                        "Runtime dispatch {} completion was not recorded: {}",
                        claimed.job_id, error.code
                    ),
                }
            }
            Err(error) if !error.retryable || out_of_attempts => {
                self.fail_runtime_dispatch_job(
                    &workspace_id,
                    &claimed.job_id,
                    &error,
                    correlation_id,
                )
                .await;
            }
            Err(_) => {}
        }
    }

    async fn fail_runtime_dispatch_job(
        &self,
        workspace_id: &WorkspaceId,
        job_id: &str,
        error: &Problem,
        correlation_id: &str,
    ) {
        let failure_code = stable_runtime_failure_code(&error.code);
        match self
            .client
            .fail_runtime_dispatch(workspace_id, job_id, &failure_code, correlation_id)
            .await
        {
            Ok(_) => self.forget_runtime_dispatch_attempts(job_id),
            Err(error) => eprintln!(
                "Runtime dispatch {job_id} failure was not recorded: {}",
                error.code
            ),
        }
    }

    fn forget_runtime_dispatch_attempts(&self, job_id: &str) {
        if let Ok(mut attempts) = self.runtime_dispatch_attempts.lock() {
            attempts.remove(job_id);
        }
    }

    async fn wait_runtime_dispatch_operation(
        &self,
        operation_id: &str,
        correlation_id: &str,
    ) -> Result<(), Problem> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
        loop {
            let operation_result = self
                .invoke(
                    "operation_get",
                    correlation_id,
                    None,
                    json!({"operationId": operation_id}),
                )
                .await;
            let operation_result = match operation_result {
                Ok(result) => result,
                Err(error) if error.code == "operation_not_found" => {
                    if tokio::time::Instant::now() >= deadline {
                        return Err(Problem::new(
                            "runtime_dispatch_operation_missing",
                            "Runtime operation was not found",
                            503,
                            correlation_id,
                        )
                        .retryable(true));
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
                Err(error) => return Err(error),
            };
            let operation: Operation =
                serde_json::from_value(operation_result.data).map_err(|_| {
                    Problem::new(
                        "runtime_dispatch_operation_invalid",
                        "Runtime operation response is invalid",
                        502,
                        correlation_id,
                    )
                })?;
            match operation.status {
                OperationStatus::Completed => return Ok(()),
                OperationStatus::Failed => {
                    return Err(operation.error.unwrap_or_else(|| {
                        Problem::new(
                            "runtime_dispatch_operation_failed",
                            "Runtime operation failed",
                            500,
                            correlation_id,
                        )
                    }));
                }
                OperationStatus::Cancelled => {
                    return Err(Problem::new(
                        "runtime_dispatch_operation_cancelled",
                        "Runtime operation was cancelled",
                        409,
                        correlation_id,
                    ));
                }
                OperationStatus::Queued | OperationStatus::Running => {}
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(Problem::new(
                    "runtime_dispatch_operation_pending",
                    "Runtime operation is still in progress",
                    503,
                    correlation_id,
                )
                .retryable(true));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    async fn verify_runtime_dispatch_state(
        &self,
        command_id: &str,
        profile_id: &ProfileId,
        correlation_id: &str,
    ) -> Result<(), Problem> {
        let active = self
            .profile_runtime
            .active_profiles()
            .await
            .map_err(|mut error| {
                error.correlation_id = correlation_id.to_owned();
                error
            })?
            .iter()
            .any(|running| running == profile_id);
        if command_id == "profile.launch" {
            return if active {
                Ok(())
            } else {
                Err(Problem::new(
                    "runtime_dispatch_process_not_running",
                    "Wayfern process is not verified as running",
                    503,
                    correlation_id,
                )
                .retryable(true))
            };
        }

        let retained = self
            .profile_runtime
            .running_profiles()
            .await
            .map_err(|mut error| {
                error.correlation_id = correlation_id.to_owned();
                error
            })?
            .iter()
            .any(|running| running == profile_id);
        let lease_held = self
            .profile_runtime
            .write_authority(profile_id.clone())
            .await
            .map_err(|mut error| {
                error.correlation_id = correlation_id.to_owned();
                error
            })?
            .is_some();
        if !active && !retained && !lease_held {
            Ok(())
        } else {
            Err(Problem::new(
                "runtime_dispatch_stop_unverified",
                "Wayfern process or runtime lease is still active",
                503,
                correlation_id,
            )
            .retryable(true))
        }
    }

    async fn poll_cookie_job_inbox(&self) {
        let Ok(timestamp) = unix_time_seconds("desktop-cookie-inbox") else {
            return;
        };
        let correlation_id = format!("cookie-inbox-{timestamp}");
        let Ok(jobs) = self.client.cookie_job_inbox_all(&correlation_id).await else {
            return;
        };
        for job in jobs {
            if job.device_id != self.device_id
                || !matches!(job.command_id.as_str(), "cookie.import" | "cookie.copy")
                || !matches!(
                    job.state,
                    ConnectedCookieJobState::Accepted
                        | ConnectedCookieJobState::Dispatched
                        | ConnectedCookieJobState::Running
                )
            {
                continue;
            }
            let Ok(workspace_id) = WorkspaceId::new(job.workspace_id.clone()) else {
                continue;
            };
            match job.command_id.as_str() {
                "cookie.import" => {
                    let _ = self
                        .execute_cookie_import_job(job, &workspace_id, None, &correlation_id)
                        .await;
                }
                "cookie.copy" => {
                    let _ = self
                        .execute_cookie_copy_job(job, &workspace_id, &correlation_id)
                        .await;
                }
                _ => {}
            }
        }
    }

    fn auth(&self, correlation_id: &str) -> Result<CoordinationAuth, Problem> {
        self.auth
            .read()
            .map_err(|_| {
                Problem::new(
                    "desktop_state_unavailable",
                    "Desktop state unavailable",
                    503,
                    correlation_id,
                )
            })?
            .clone()
            .ok_or_else(|| {
                Problem::new(
                    "production_api_token_required",
                    "Production API token is required",
                    401,
                    correlation_id,
                )
            })
    }

    async fn install_authenticated_coordination(
        &self,
        correlation_id: &str,
    ) -> Result<(), Problem> {
        let auth = self
            .client
            .authenticated_coordination_auth(correlation_id)
            .await?;
        self.profile_runtime
            .set_coordination(Some(ManagedCoordination {
                backend: Arc::new(self.client.clone()),
                auth: auth.clone(),
                lease_ttl: Duration::from_secs(60),
                heartbeat_interval: Duration::from_secs(15),
            }))
            .await?;
        *self
            .auth
            .write()
            .map_err(|_| desktop_state_problem(correlation_id))? = Some(auth);
        Ok(())
    }

    async fn ensure_authenticated_coordination(&self, correlation_id: &str) -> Result<(), Problem> {
        let _identity_guard = self.auth_refresh_gate.lock().await;
        if !self.client.has_token()? {
            return Ok(());
        }
        let needs_install = {
            let auth = self
                .auth
                .read()
                .map_err(|_| desktop_state_problem(correlation_id))?;
            auth.is_none()
        };
        if !needs_install {
            return Ok(());
        }
        match self
            .install_authenticated_coordination(correlation_id)
            .await
        {
            Err(error) if definitive_auth_error(&error) => {
                self.clear_local_auth(correlation_id).await
            }
            result => result,
        }
    }

    async fn cookie_stage(
        &self,
        payload: Value,
        correlation_id: &str,
    ) -> Result<CommandResult<Value>, Problem> {
        let input: CookieStageCommandInput = parse_payload(payload, correlation_id)?;
        let workspace_id = WorkspaceId::new(input.workspace_id)
            .map_err(|_| invalid_input("workspaceId", correlation_id))?;
        let profile_id = ProfileId::new(input.profile_id)
            .map_err(|_| invalid_input("profileId", correlation_id))?;
        let binding = self
            .client
            .profile_binding(&profile_id, correlation_id)
            .await?;
        if binding.workspace_id != workspace_id
            || binding.profile_id != profile_id
            || binding.archive_state != ProfileArchiveState::Active
        {
            return Err(Problem::new(
                "profile_forbidden",
                "Profile access denied",
                403,
                correlation_id,
            ));
        }
        let profile = self.client.profile(&profile_id, correlation_id).await?;
        if profile.value.archived
            || profile.value.runtime != buglogin_domain::entities::Runtime::Wayfern
        {
            return Err(Problem::new(
                "cookie_runtime_unsupported",
                "Cookie operations require an active Wayfern profile",
                409,
                correlation_id,
            ));
        }
        let scope = CookieScope {
            workspace_id,
            profile_id,
        };
        let cookies = input
            .cookies
            .into_iter()
            .map(|cookie| {
                let value = SecretValue::new(cookie.value.into_bytes()).map_err(|_| {
                    Problem::new(
                        "invalid_cookie_value",
                        "Cookie value is invalid",
                        400,
                        correlation_id,
                    )
                })?;
                Ok(CookieInput {
                    name: cookie.name,
                    domain: cookie.domain,
                    path: cookie.path,
                    secure: cookie.secure,
                    http_only: cookie.http_only,
                    expires_at: cookie.expires_at,
                    same_site: cookie.same_site,
                    value,
                })
            })
            .collect::<Result<Vec<_>, Problem>>()?;
        let mut authority = self
            .cookies
            .lock()
            .map_err(|_| desktop_state_problem(correlation_id))?;
        authority.set_profile_state(
            scope.clone(),
            if matches!(
                &profile.value.state,
                ProfileState::Running | ProfileState::Launching | ProfileState::Stopping
            ) {
                ProfileRuntimeState::Running
            } else {
                ProfileRuntimeState::Stopped
            },
        );
        let stage_id = authority
            .stage_import(scope, cookies)
            .map_err(|error| error.as_problem(correlation_id))?;
        Ok(result(
            "cookie.stage",
            correlation_id,
            json!({"stageId": stage_id.as_str()}),
        ))
    }

    async fn cookie_import(
        &self,
        payload: Value,
        idempotency_key: Option<&str>,
        correlation_id: &str,
    ) -> Result<CommandResult<Value>, Problem> {
        let idempotency_key = validate_idempotency_key(idempotency_key, correlation_id)?;
        let input: CookieImportCommandInput = parse_payload(payload, correlation_id)?;
        let workspace_id = WorkspaceId::new(input.workspace_id)
            .map_err(|_| invalid_input("workspaceId", correlation_id))?;
        let profile_id = ProfileId::new(input.profile_id)
            .map_err(|_| invalid_input("profileId", correlation_id))?;
        let stage_id = StagedImportId::from_opaque(input.stage_id)
            .map_err(|error| error.as_problem(correlation_id))?;
        let job = self
            .client
            .enqueue_cookie_job(
                "cookie.import",
                &workspace_id,
                &profile_id,
                &self.device_id,
                input.expected_generation,
                Some(stage_id.as_str()),
                None,
                &idempotency_key,
                correlation_id,
            )
            .await?;
        if job.command_id != "cookie.import"
            || job.workspace_id != workspace_id.as_str()
            || job.profile_id != profile_id.as_str()
            || job.device_id != self.device_id
            || job.expected_generation != input.expected_generation
            || job.staged_import_ref.as_deref() != Some(stage_id.as_str())
        {
            return Err(Problem::new(
                "cookie_job_response_invalid",
                "BugLogin returned a cookie job that does not match this request",
                502,
                correlation_id,
            ));
        }
        if job.state == ConnectedCookieJobState::Completed {
            let receipt = job.receipt.ok_or_else(|| {
                Problem::new(
                    "cookie_job_response_invalid",
                    "Completed cookie job has no receipt",
                    502,
                    correlation_id,
                )
            })?;
            return Ok(result(
                "cookie.import",
                correlation_id,
                serde_json::to_value(receipt)
                    .map_err(|_| invalid_input("cookieReceipt", correlation_id))?,
            ));
        }
        self.execute_cookie_import_job(job, &workspace_id, None, correlation_id)
            .await
    }

    async fn cookie_copy(
        &self,
        payload: Value,
        idempotency_key: Option<&str>,
        correlation_id: &str,
    ) -> Result<CommandResult<Value>, Problem> {
        let idempotency_key = validate_idempotency_key(idempotency_key, correlation_id)?;
        let input: CookieCopyCommandInput = parse_payload(payload, correlation_id)?;
        let source_workspace = WorkspaceId::new(input.source_workspace_id)
            .map_err(|_| invalid_input("sourceWorkspaceId", correlation_id))?;
        let source_profile = ProfileId::new(input.source_profile_id)
            .map_err(|_| invalid_input("sourceProfileId", correlation_id))?;
        let destination_workspace = WorkspaceId::new(input.destination_workspace_id)
            .map_err(|_| invalid_input("destinationWorkspaceId", correlation_id))?;
        let destination_profile = ProfileId::new(input.destination_profile_id)
            .map_err(|_| invalid_input("destinationProfileId", correlation_id))?;
        let job = self
            .client
            .enqueue_cookie_job(
                "cookie.copy",
                &destination_workspace,
                &destination_profile,
                &self.device_id,
                input.expected_generation,
                None,
                Some((&source_workspace, &source_profile)),
                &idempotency_key,
                correlation_id,
            )
            .await?;
        if job.command_id != "cookie.copy"
            || job.workspace_id != destination_workspace.as_str()
            || job.profile_id != destination_profile.as_str()
            || job.device_id != self.device_id
            || job.expected_generation != input.expected_generation
            || job.source_workspace_id.as_deref() != Some(source_workspace.as_str())
            || job.source_profile_id.as_deref() != Some(source_profile.as_str())
            || job.source_device_id.as_deref() != Some(self.device_id.as_str())
        {
            return Err(Problem::new(
                "cookie_job_response_invalid",
                "BugLogin returned a cookie copy job that does not match this request",
                502,
                correlation_id,
            ));
        }
        if job.state == ConnectedCookieJobState::Completed {
            let receipt = job.receipt.ok_or_else(|| {
                Problem::new(
                    "cookie_job_response_invalid",
                    "Completed cookie job has no receipt",
                    502,
                    correlation_id,
                )
            })?;
            return Ok(result(
                "cookie.copy",
                correlation_id,
                serde_json::to_value(receipt)
                    .map_err(|_| invalid_input("cookieReceipt", correlation_id))?,
            ));
        }
        self.execute_cookie_copy_job(job, &destination_workspace, correlation_id)
            .await
    }

    async fn execute_cookie_copy_job(
        &self,
        job: buglogin_api::services::connected_cookie_jobs::ConnectedCookieJob,
        destination_workspace: &WorkspaceId,
        correlation_id: &str,
    ) -> Result<CommandResult<Value>, Problem> {
        // Reject an invalid local clock before reading or staging source cookies.
        unix_time_seconds(correlation_id)?;
        if job.command_id != "cookie.copy"
            || job.workspace_id != destination_workspace.as_str()
            || job.device_id != self.device_id
            || job.source_device_id.as_deref() != Some(self.device_id.as_str())
            || !matches!(
                job.state,
                ConnectedCookieJobState::Accepted
                    | ConnectedCookieJobState::Dispatched
                    | ConnectedCookieJobState::Running
            )
        {
            return Err(Problem::new(
                "cookie_job_scope_mismatch",
                "Cookie copy job is not assigned to this device and workspace",
                403,
                correlation_id,
            ));
        }
        let already_committed = {
            let authority = self
                .cookies
                .lock()
                .map_err(|_| desktop_state_problem(correlation_id))?;
            authority
                .hosted_job_receipts()
                .map_err(|error| error.as_problem(correlation_id))?
                .iter()
                .any(|record| record.job.job_id == job.job_id)
        };
        if already_committed {
            return self
                .execute_cookie_import_job(job, destination_workspace, None, correlation_id)
                .await;
        }
        let source_workspace = WorkspaceId::new(
            job.source_workspace_id
                .clone()
                .ok_or_else(|| invalid_input("sourceWorkspaceId", correlation_id))?,
        )
        .map_err(|_| invalid_input("sourceWorkspaceId", correlation_id))?;
        let source_profile = ProfileId::new(
            job.source_profile_id
                .clone()
                .ok_or_else(|| invalid_input("sourceProfileId", correlation_id))?,
        )
        .map_err(|_| invalid_input("sourceProfileId", correlation_id))?;
        let binding = self
            .client
            .profile_binding(&source_profile, correlation_id)
            .await?;
        if binding.workspace_id != source_workspace
            || binding.archive_state != ProfileArchiveState::Active
        {
            return Err(Problem::new(
                "profile_forbidden",
                "Source profile access denied",
                403,
                correlation_id,
            ));
        }
        let source = self.client.profile(&source_profile, correlation_id).await?;
        if source.value.archived
            || source.value.runtime != buglogin_domain::entities::Runtime::Wayfern
            || source.value.state != ProfileState::Ready
        {
            return Err(Problem::new(
                "cookie_source_unavailable",
                "Cookie copy requires an active, stopped Wayfern source profile",
                409,
                correlation_id,
            ));
        }
        let auth = self.auth(correlation_id)?;
        let source_scope = CoordinationScope {
            workspace_id: &source_workspace,
            auth: &auth,
        };
        let manifest = self
            .client
            .current_manifest(source_scope.clone(), &binding.resource_id)
            .await?
            .ok_or_else(|| {
                Problem::new(
                    "production_manifest_not_found",
                    "Source profile data must be synced to this device before copying cookies",
                    409,
                    correlation_id,
                )
            })?;
        let source_path = self
            .projection
            .existing_local_generation(&source_profile, &manifest)?
            .ok_or_else(|| {
                Problem::new(
                    "cookie_source_not_hydrated",
                    "Sync the source profile on this device before copying cookies",
                    409,
                    correlation_id,
                )
            })?;
        let lease = self
            .client
            .acquire_lease_idempotent(
                source_scope.clone(),
                &binding.resource_id,
                Duration::from_secs(120),
                &lease_idempotency_key(
                    &format!("cookie-copy-source-{}", job.job_id),
                    manifest.generation,
                ),
            )
            .await?;
        let stopped = match self
            .profile_runtime
            .hold_idle_profile_for_sync(
                source_profile.clone(),
                source_path,
                LeaseScope {
                    workspace_id: source_workspace.clone(),
                    resource_id: binding.resource_id.clone(),
                },
                lease.clone(),
            )
            .await
        {
            Ok(stopped) => stopped,
            Err(error) => {
                let _ = self
                    .client
                    .release_lease(
                        source_scope.clone(),
                        &binding.resource_id,
                        &lease.owner_token,
                    )
                    .await;
                return Err(error);
            }
        };
        let source_cookies = buglogin_browser::cookies::wayfern::read_inputs(&stopped.profile_path)
            .map_err(|_| {
                Problem::new(
                    "cookie_source_read_failed",
                    "Source browser cookies could not be read",
                    503,
                    correlation_id,
                )
            });
        let finish_source = self
            .profile_runtime
            .complete_stop(source_profile.clone())
            .await;
        if let Err(error) = finish_source {
            return Err(error);
        }
        let source_cookies = source_cookies?;
        let destination_profile = ProfileId::new(job.profile_id.clone())
            .map_err(|_| invalid_input("profileId", correlation_id))?;
        let destination_scope = CookieScope {
            workspace_id: destination_workspace.clone(),
            profile_id: destination_profile,
        };
        let stage_id = {
            let mut authority = self
                .cookies
                .lock()
                .map_err(|_| desktop_state_problem(correlation_id))?;
            authority
                .stage_import(destination_scope, source_cookies)
                .map_err(|error| error.as_problem(correlation_id))?
        };
        self.execute_cookie_import_job(job, destination_workspace, Some(stage_id), correlation_id)
            .await
    }

    async fn execute_cookie_import_job(
        &self,
        job: buglogin_api::services::connected_cookie_jobs::ConnectedCookieJob,
        workspace_id: &WorkspaceId,
        staged_import_id: Option<StagedImportId>,
        correlation_id: &str,
    ) -> Result<CommandResult<Value>, Problem> {
        let result_command_id = job.command_id.clone();
        if !matches!(job.command_id.as_str(), "cookie.import" | "cookie.copy")
            || job.workspace_id != workspace_id.as_str()
            || job.device_id != self.device_id
            || !matches!(
                job.state,
                ConnectedCookieJobState::Accepted
                    | ConnectedCookieJobState::Dispatched
                    | ConnectedCookieJobState::Running
            )
        {
            return Err(Problem::new(
                "cookie_job_scope_mismatch",
                "Cookie job is not assigned to this device and workspace",
                403,
                correlation_id,
            ));
        }
        let profile_id = ProfileId::new(job.profile_id.clone())
            .map_err(|_| invalid_input("profileId", correlation_id))?;
        let scope_cookie = CookieScope {
            workspace_id: workspace_id.clone(),
            profile_id: profile_id.clone(),
        };
        let local_idempotency_key = format!("cookie-job-{}", job.job_id);
        let local_key = IdempotencyKey::new(local_idempotency_key.clone())
            .map_err(|error| error.as_problem(correlation_id))?;
        let binding = self
            .client
            .profile_binding(&profile_id, correlation_id)
            .await?;
        if binding.workspace_id != *workspace_id
            || binding.profile_id != profile_id
            || binding.archive_state != ProfileArchiveState::Active
        {
            return Err(Problem::new(
                "profile_forbidden",
                "Profile access denied",
                403,
                correlation_id,
            ));
        }
        let profile = self.client.profile(&profile_id, correlation_id).await?;
        if profile.value.archived
            || profile.value.runtime != buglogin_domain::entities::Runtime::Wayfern
            || profile.value.state != ProfileState::Ready
        {
            return Err(Problem::new(
                "cookie_profile_unavailable",
                "Cookie import requires an active, stopped Wayfern profile",
                409,
                correlation_id,
            ));
        }
        let existing_receipt = {
            let authority = self
                .cookies
                .lock()
                .map_err(|_| desktop_state_problem(correlation_id))?;
            authority
                .hosted_job_receipts()
                .map_err(|error| error.as_problem(correlation_id))?
                .into_iter()
                .find(|record| record.job.job_id == job.job_id)
        };
        if let Some(existing) = &existing_receipt {
            if existing.job.operation_id != job.operation_id
                || existing.job.device_id != self.device_id
                || existing.scope != scope_cookie
                || existing.idempotency_key != local_idempotency_key
            {
                return Err(Problem::new(
                    "cookie_recovery_record_mismatch",
                    "Cookie recovery record does not match this job",
                    409,
                    correlation_id,
                ));
            }
        } else {
            let authority = self
                .cookies
                .lock()
                .map_err(|_| desktop_state_problem(correlation_id))?;
            if authority.generation(&scope_cookie) != job.expected_generation {
                return Err(Problem::new(
                    "cookie_generation_mismatch",
                    "Cookie data changed; refresh the profile before importing",
                    409,
                    correlation_id,
                ));
            }
        }
        let auth = self.auth(correlation_id)?;
        let coordination_scope = CoordinationScope {
            workspace_id,
            auth: &auth,
        };
        let manifest = self
            .client
            .current_manifest(coordination_scope.clone(), &binding.resource_id)
            .await?
            .ok_or_else(|| {
                Problem::new(
                    "production_manifest_not_found",
                    "Profile data must be synced to this device before cookie import",
                    409,
                    correlation_id,
                )
            })?;
        let profile_path = self
            .projection
            .existing_local_generation(&profile_id, &manifest)?
            .ok_or_else(|| {
                Problem::new(
                    "cookie_profile_not_hydrated",
                    "Open or sync this profile on this device before importing cookies",
                    409,
                    correlation_id,
                )
            })?;
        let claimed = if job.state == ConnectedCookieJobState::Running {
            if job.executor_account_id.as_deref() != Some(auth.actor_id()) {
                return Err(Problem::new(
                    "cookie_job_executor_mismatch",
                    "Cookie job recovery belongs to a different account",
                    403,
                    correlation_id,
                ));
            }
            job.clone()
        } else {
            self.client
                .claim_cookie_job(workspace_id, &job.job_id, correlation_id)
                .await?
        };
        if !matches!(
            claimed.state,
            ConnectedCookieJobState::Dispatched | ConnectedCookieJobState::Running
        ) {
            return Err(Problem::new(
                "cookie_job_not_dispatchable",
                "Cookie job is not available to this device",
                409,
                correlation_id,
            ));
        }
        let lease = self
            .client
            .acquire_lease_idempotent(
                coordination_scope.clone(),
                &binding.resource_id,
                Duration::from_secs(120),
                &lease_idempotency_key(&local_idempotency_key, manifest.generation),
            )
            .await?;
        if claimed.state == ConnectedCookieJobState::Dispatched {
            let acknowledged = match self
                .client
                .acknowledge_cookie_job(&workspace_id, &job.job_id, &lease, correlation_id)
                .await
            {
                Ok(acknowledged) => acknowledged,
                Err(error) => {
                    let _ = self
                        .client
                        .release_lease(
                            coordination_scope.clone(),
                            &binding.resource_id,
                            &lease.owner_token,
                        )
                        .await;
                    return Err(error);
                }
            };
            if acknowledged.state != ConnectedCookieJobState::Running
                || acknowledged.executor_account_id.as_deref() != Some(auth.actor_id())
            {
                let _ = self
                    .client
                    .release_lease(
                        coordination_scope.clone(),
                        &binding.resource_id,
                        &lease.owner_token,
                    )
                    .await;
                return Err(Problem::new(
                    "cookie_job_not_dispatchable",
                    "Cookie job is no longer available to this device",
                    409,
                    correlation_id,
                ));
            }
        }
        let stopped = match self
            .profile_runtime
            .hold_idle_profile_for_sync(
                profile_id.clone(),
                profile_path,
                LeaseScope {
                    workspace_id: workspace_id.clone(),
                    resource_id: binding.resource_id.clone(),
                },
                lease.clone(),
            )
            .await
        {
            Ok(stopped) => stopped,
            Err(error) => {
                let failure = if error.code == "profile_already_running" {
                    ConnectedCookieFailureCode::ProfileRunning
                } else {
                    ConnectedCookieFailureCode::RuntimeUnavailable
                };
                let _ = self
                    .client
                    .fail_cookie_job(&workspace_id, &job.job_id, failure, correlation_id)
                    .await;
                let _ = self
                    .client
                    .release_lease(
                        coordination_scope.clone(),
                        &binding.resource_id,
                        &lease.owner_token,
                    )
                    .await;
                return Err(error);
            }
        };
        let hosted_job = HostedCookieJobBinding {
            job_id: job.job_id.clone(),
            operation_id: job.operation_id.clone(),
            device_id: self.device_id.clone(),
        };
        let cookie_lease = buglogin_domain::lease::Lease {
            lease_id: format!("cookie-{}", job.job_id),
            fencing_token: lease.fencing_token,
            resource_id: binding.resource_id.clone(),
            workspace_id: workspace_id.clone(),
            owner_id: lease.owner_id.clone(),
            token: lease.owner_token.clone(),
            issued_at: lease.issued_at_ms,
            expires_at: lease.expires_at_ms,
        };
        // Snapshot the operation clock after remote preflight and immediately
        // before the local mutation; reuse it for the durable job receipt.
        let now_seconds = unix_time_seconds(correlation_id)?;
        let now_millis = unix_time_millis(correlation_id)?;
        let mutation_attempt = (|| -> Result<CookieMutationResult, Problem> {
            let mut authority = self
                .cookies
                .lock()
                .map_err(|_| desktop_state_problem(correlation_id))?;
            authority.set_profile_state(scope_cookie.clone(), ProfileRuntimeState::Stopped);
            authority
                .set_current_lease(scope_cookie.clone(), cookie_lease.clone())
                .map_err(|error| error.as_problem(correlation_id))?;
            let principal = local_cookie_principal();
            if !authority.has_idempotency_receipt(&local_key) {
                let stage_id = match staged_import_id.clone() {
                    Some(stage_id) => stage_id,
                    None => StagedImportId::from_opaque(
                        job.staged_import_ref
                            .clone()
                            .ok_or_else(|| invalid_input("stagedImportRef", correlation_id))?,
                    )
                    .map_err(|error| error.as_problem(correlation_id))?,
                };
                let mut mutation = authority
                    .import_hosted(
                        &stage_id,
                        job.expected_generation,
                        &cookie_lease,
                        now_millis,
                        &principal,
                        &[],
                        local_key.clone(),
                        &hosted_job,
                    )
                    .map_err(|error| error.as_problem(correlation_id))?;
                let cookies = authority
                    .materialization_inputs(&scope_cookie, &principal, &[])
                    .map_err(|error| error.as_problem(correlation_id))?;
                let now_unix = now_seconds.min(i64::MAX as u64) as i64;
                buglogin_browser::cookies::wayfern::materialize_from_inputs(
                    &stopped.profile_path,
                    buglogin_browser::cookies::wayfern::ProfileCookieState::Stopped,
                    &cookies,
                    now_unix,
                )
                .map_err(|_| {
                    Problem::new(
                        "cookie_materialization_failed",
                        "Browser cookie database could not be updated",
                        503,
                        correlation_id,
                    )
                })?;
                authority
                    .complete_materialization(&scope_cookie, &local_idempotency_key)
                    .map_err(|error| error.as_problem(correlation_id))?;
                mutation.replayed = false;
                Ok(mutation)
            } else {
                let records = authority
                    .hosted_job_receipts()
                    .map_err(|error| error.as_problem(correlation_id))?;
                let existing = records
                    .into_iter()
                    .find(|record| record.job.job_id == job.job_id)
                    .ok_or_else(|| {
                        Problem::new(
                            "cookie_recovery_record_missing",
                            "Cookie job recovery record is unavailable",
                            503,
                            correlation_id,
                        )
                    })?;
                if existing.job != hosted_job
                    || existing.scope != scope_cookie
                    || existing.idempotency_key != local_idempotency_key
                {
                    return Err(Problem::new(
                        "cookie_recovery_record_mismatch",
                        "Cookie recovery record does not match this job",
                        409,
                        correlation_id,
                    ));
                }
                if existing.materialization_pending {
                    let principal = local_cookie_principal();
                    let cookies = authority
                        .materialization_inputs(&scope_cookie, &principal, &[])
                        .map_err(|error| error.as_problem(correlation_id))?;
                    buglogin_browser::cookies::wayfern::materialize_from_inputs(
                        &stopped.profile_path,
                        buglogin_browser::cookies::wayfern::ProfileCookieState::Stopped,
                        &cookies,
                        now_seconds.min(i64::MAX as u64) as i64,
                    )
                    .map_err(|_| {
                        Problem::new(
                            "cookie_materialization_failed",
                            "Browser cookie database could not be recovered",
                            503,
                            correlation_id,
                        )
                    })?;
                    authority
                        .complete_materialization(&scope_cookie, &existing.idempotency_key)
                        .map_err(|error| error.as_problem(correlation_id))?;
                }
                Ok(existing.mutation)
            }
        })();
        let mut mutation = match mutation_attempt {
            Ok(mutation) => mutation,
            Err(error) => {
                let has_hosted_receipt = self
                    .cookies
                    .lock()
                    .ok()
                    .and_then(|authority| authority.hosted_job_receipts().ok())
                    .is_some_and(|records| {
                        records.iter().any(|record| record.job.job_id == job.job_id)
                    });
                if !has_hosted_receipt {
                    let failure = match error.code.as_str() {
                        "cookie_generation_mismatch" => {
                            ConnectedCookieFailureCode::GenerationConflict
                        }
                        "staged_cookie_import_expired" | "staged_cookie_import_not_found" => {
                            ConnectedCookieFailureCode::StageExpired
                        }
                        "cookie_authority_storage_error" | "cookie_authority_lock_unavailable" => {
                            ConnectedCookieFailureCode::VaultUnavailable
                        }
                        _ => ConnectedCookieFailureCode::MaterializationFailed,
                    };
                    let _ = self
                        .client
                        .fail_cookie_job(&workspace_id, &job.job_id, failure, correlation_id)
                        .await;
                    let _ = self.profile_runtime.complete_stop(profile_id.clone()).await;
                }
                return Err(error);
            }
        };
        if result_command_id == "cookie.copy" {
            mutation.report.copied = mutation
                .report
                .inserted
                .saturating_add(mutation.report.replaced);
        }
        let listed = {
            let authority = self
                .cookies
                .lock()
                .map_err(|_| desktop_state_problem(correlation_id))?;
            authority
                .list(&scope_cookie, &local_cookie_principal(), &[])
                .map_err(|error| error.as_problem(correlation_id))?
        };
        let checksum_bytes = serde_json::to_vec(&listed.cookies)
            .map_err(|_| invalid_input("cookieMetadata", correlation_id))?;
        let metadata_checksum = format!("{:x}", Sha256::digest(&checksum_bytes));
        let stop_result = self
            .stop_profile(
                profile_id.clone(),
                format!("cookie-sync-{}", job.job_id),
                correlation_id.to_owned(),
                None,
                Some(lease.clone()),
                Some(stopped),
            )
            .await?;
        let receipt = ConnectedCookieReceipt {
            operation_id: job.operation_id.clone(),
            job_id: job.job_id.clone(),
            device_id: self.device_id.clone(),
            profile_id: profile_id.as_str().to_owned(),
            previous_generation: job.expected_generation,
            new_generation: mutation.generation,
            created_count: mutation.report.inserted as u64,
            replaced_count: mutation.report.replaced as u64,
            deleted_count: 0,
            metadata_checksum,
            completed_at: now_seconds,
        };
        let completed = self
            .client
            .complete_cookie_job(&workspace_id, &job.job_id, &receipt, correlation_id)
            .await?;
        Ok(result(
            &result_command_id,
            correlation_id,
            json!({"job": completed, "sync": stop_result, "mutation": mutation}),
        ))
    }

    pub fn profiles_root(&self) -> &Path {
        self.projection.root()
    }

    pub fn auth_status(&self) -> Result<DesktopAuthStatus, Problem> {
        if self.client.has_token()? {
            return Ok(DesktopAuthStatus::Authenticated);
        }
        Ok(DesktopAuthStatus::Unauthenticated)
    }

    pub async fn login_with_credentials(
        &self,
        mut identifier: Zeroizing<String>,
        mut password: Zeroizing<String>,
        correlation_id: &str,
    ) -> Result<DesktopAuthStatus, Problem> {
        self.refresh_session_if_needed(correlation_id).await?;
        if self.client.has_token()? {
            self.ensure_authenticated_coordination(correlation_id)
                .await?;
            if self.client.has_token()? {
                self.ensure_device_key_registered(correlation_id).await?;
                return Ok(DesktopAuthStatus::Authenticated);
            }
        }
        let _login_guard = self.auth_refresh_gate.lock().await;
        if self.client.has_token()? {
            self.ensure_device_key_registered(correlation_id).await?;
            return Ok(DesktopAuthStatus::Authenticated);
        }
        let token = self
            .client
            .desktop_login(
                std::mem::take(&mut *identifier),
                std::mem::take(&mut *password),
                &self.device_id,
                correlation_id,
            )
            .await?;
        self.install_session_tokens(token, correlation_id).await
    }

    pub async fn start_buglab_sso(
        &self,
        correlation_id: &str,
    ) -> Result<BugLabSsoStartResponse, Problem> {
        if self.client.has_token()? {
            return Err(Problem::new(
                "already_authenticated",
                "Sign out before starting another sign-in",
                409,
                correlation_id,
            ));
        }
        self.client
            .buglab_sso_start(&self.device_id, correlation_id)
            .await
    }

    pub async fn complete_buglab_sso(
        &self,
        state: Zeroizing<String>,
        provider_token: Zeroizing<String>,
        correlation_id: &str,
    ) -> Result<DesktopAuthStatus, Problem> {
        self.refresh_session_if_needed(correlation_id).await?;
        if self.client.has_token()? {
            self.ensure_authenticated_coordination(correlation_id)
                .await?;
            if self.client.has_token()? {
                self.ensure_device_key_registered(correlation_id).await?;
                return Ok(DesktopAuthStatus::Authenticated);
            }
        }
        let _login_guard = self.auth_refresh_gate.lock().await;
        if self.client.has_token()? {
            self.ensure_device_key_registered(correlation_id).await?;
            return Ok(DesktopAuthStatus::Authenticated);
        }
        let token = self
            .client
            .buglab_sso_complete(state.as_str(), provider_token.as_str(), correlation_id)
            .await?;
        self.install_session_tokens(token, correlation_id).await
    }

    async fn install_session_tokens(
        &self,
        token: DeviceSessionTokens,
        correlation_id: &str,
    ) -> Result<DesktopAuthStatus, Problem> {
        let version = self.persist_session(&token, correlation_id)?;
        self.client
            .replace_refreshed_token(token.access_token.clone())?;
        *self
            .auth_refresh_token
            .lock()
            .map_err(|_| desktop_state_problem(correlation_id))? =
            Some(Zeroizing::new(token.refresh_token.clone()));
        *self
            .auth_expires_at
            .lock()
            .map_err(|_| desktop_state_problem(correlation_id))? = token.expires_at;
        *self
            .auth_refresh_uncertain
            .lock()
            .map_err(|_| desktop_state_problem(correlation_id))? = false;
        if let Err(error) = self
            .install_authenticated_coordination(correlation_id)
            .await
        {
            let _ = self.client.clear_token();
            if let Ok(mut stored_version) = self.auth_vault_version.lock() {
                *stored_version = None;
            }
            let _ = self.remove_persisted_bearer(version, correlation_id);
            if let Ok(mut refresh_token) = self.auth_refresh_token.lock() {
                *refresh_token = None;
            }
            if let Ok(mut expires_at) = self.auth_expires_at.lock() {
                *expires_at = 0;
            }
            return Err(error);
        }
        self.ensure_device_key_registered(correlation_id).await?;
        self.auth_status()
    }

    pub async fn logout(&self, correlation_id: &str) -> Result<DesktopAuthStatus, Problem> {
        self.refresh_session_if_needed(correlation_id).await?;
        let running = self.profile_runtime.running_profiles().await?;
        if !running.is_empty() {
            return Err(Problem::new(
                "logout_running_profiles",
                "Stop running profiles before logout",
                409,
                correlation_id,
            ));
        }
        if self.client.has_token()? {
            self.client.logout(correlation_id).await?;
        }
        self.profile_runtime.set_coordination(None).await?;
        self.client.clear_token()?;
        *self
            .auth_refresh_token
            .lock()
            .map_err(|_| desktop_state_problem(correlation_id))? = None;
        *self
            .auth_expires_at
            .lock()
            .map_err(|_| desktop_state_problem(correlation_id))? = 0;
        *self
            .auth
            .write()
            .map_err(|_| desktop_state_problem(correlation_id))? = None;
        if let Some(version) = self
            .auth_vault_version
            .lock()
            .map_err(|_| desktop_state_problem(correlation_id))?
            .take()
        {
            self.auth_vault
                .lock()
                .map_err(|_| desktop_state_problem(correlation_id))?
                .remove_bearer(&self.device_id, version, correlation_id)?;
        }
        self.auth_status()
    }

    fn persist_session(
        &self,
        session: &DeviceSessionTokens,
        correlation_id: &str,
    ) -> Result<SecretVersion, Problem> {
        let mut vault = self
            .auth_vault
            .lock()
            .map_err(|_| desktop_state_problem(correlation_id))?;
        let mut version = self
            .auth_vault_version
            .lock()
            .map_err(|_| desktop_state_problem(correlation_id))?;
        let next = match *version {
            Some(current) => vault.replace_session(
                &self.device_id,
                current,
                &session.access_token,
                &session.refresh_token,
                session.expires_at,
                correlation_id,
            )?,
            None => vault.store_session(
                &self.device_id,
                &session.access_token,
                &session.refresh_token,
                session.expires_at,
                correlation_id,
            )?,
        };
        *version = Some(next);
        Ok(next)
    }

    fn remove_persisted_bearer(
        &self,
        version: SecretVersion,
        correlation_id: &str,
    ) -> Result<(), Problem> {
        self.auth_vault
            .lock()
            .map_err(|_| desktop_state_problem(correlation_id))?
            .remove_bearer(&self.device_id, version, correlation_id)
    }

    async fn refresh_session_if_needed(&self, correlation_id: &str) -> Result<(), Problem> {
        if !self.client.has_token()? {
            return Ok(());
        }
        let _refresh_guard = self.auth_refresh_gate.lock().await;
        if *self
            .auth_refresh_uncertain
            .lock()
            .map_err(|_| desktop_state_problem(correlation_id))?
        {
            self.disable_local_auth(correlation_id).await?;
            return Ok(());
        }
        let expires_at = *self
            .auth_expires_at
            .lock()
            .map_err(|_| desktop_state_problem(correlation_id))?;
        if expires_at
            > checked_add_seconds(unix_time_seconds(correlation_id)?, 120, correlation_id)?
        {
            return Ok(());
        }
        let refresh_token = self
            .auth_refresh_token
            .lock()
            .map_err(|_| desktop_state_problem(correlation_id))?
            .clone();
        if refresh_token.is_none()
            && self
                .auth_vault_version
                .lock()
                .map_err(|_| desktop_state_problem(correlation_id))?
                .is_none()
        {
            // An explicitly supplied bearer has no refresh credential or vault
            // session to rotate. Keep it available and let the API validate it.
            return Ok(());
        }
        if let Some(refresh_token) = refresh_token {
            self.stage_refresh_attempt(correlation_id)?;
            let token = match self
                .client
                .device_session_refresh(&refresh_token, &self.device_id, correlation_id)
                .await
            {
                Ok(token) => token,
                Err(error) if definitive_auth_error(&error) => {
                    self.clear_local_auth(correlation_id).await?;
                    return Ok(());
                }
                Err(error) if preserve_refresh_credentials(&error) => {
                    self.clear_refresh_attempt(correlation_id)?;
                    return Err(error);
                }
                Err(error) => {
                    self.disable_local_auth(correlation_id).await?;
                    return Err(error);
                }
            };
            let persistence_error = self.persist_session(&token, correlation_id).err();
            self.client
                .replace_refreshed_token(token.access_token.clone())?;
            *self
                .auth_refresh_token
                .lock()
                .map_err(|_| desktop_state_problem(correlation_id))? =
                Some(Zeroizing::new(token.refresh_token.clone()));
            *self
                .auth_expires_at
                .lock()
                .map_err(|_| desktop_state_problem(correlation_id))? = token.expires_at;
            self.install_authenticated_coordination(correlation_id)
                .await?;
            if persistence_error.is_none() {
                *self
                    .auth_refresh_uncertain
                    .lock()
                    .map_err(|_| desktop_state_problem(correlation_id))? = false;
            } else {
                self.disable_local_auth(correlation_id).await?;
            }
            return persistence_error.map_or(Ok(()), Err);
        }

        self.stage_refresh_attempt(correlation_id)?;
        let token = match self.client.legacy_session_refresh(correlation_id).await {
            Ok(token) => token,
            Err(error) if definitive_auth_error(&error) => {
                self.clear_local_auth(correlation_id).await?;
                return Ok(());
            }
            Err(error) if preserve_refresh_credentials(&error) => {
                self.clear_refresh_attempt(correlation_id)?;
                return Err(error);
            }
            Err(error) => {
                self.disable_local_auth(correlation_id).await?;
                return Err(error);
            }
        };
        self.client.set_token(token.access_token.clone())?;
        let persistence_error = self
            .persist_legacy_session(&token.access_token, token.expires_at, correlation_id)
            .err();
        *self
            .auth_expires_at
            .lock()
            .map_err(|_| desktop_state_problem(correlation_id))? = token.expires_at;
        self.install_authenticated_coordination(correlation_id)
            .await?;
        if persistence_error.is_none() {
            *self
                .auth_refresh_uncertain
                .lock()
                .map_err(|_| desktop_state_problem(correlation_id))? = false;
        } else {
            self.disable_local_auth(correlation_id).await?;
        }
        persistence_error.map_or(Ok(()), Err)
    }

    fn persist_legacy_session(
        &self,
        bearer: &str,
        expires_at: u64,
        correlation_id: &str,
    ) -> Result<SecretVersion, Problem> {
        let mut vault = self
            .auth_vault
            .lock()
            .map_err(|_| desktop_state_problem(correlation_id))?;
        let mut version = self
            .auth_vault_version
            .lock()
            .map_err(|_| desktop_state_problem(correlation_id))?;
        let next = match *version {
            Some(current) => vault.replace_legacy_session(
                &self.device_id,
                current,
                bearer,
                expires_at,
                correlation_id,
            )?,
            None => {
                vault.store_legacy_session(&self.device_id, bearer, expires_at, correlation_id)?
            }
        };
        *version = Some(next);
        Ok(next)
    }

    async fn clear_local_auth(&self, correlation_id: &str) -> Result<(), Problem> {
        self.client.clear_token()?;
        *self
            .auth
            .write()
            .map_err(|_| desktop_state_problem(correlation_id))? = None;
        *self
            .auth_refresh_token
            .lock()
            .map_err(|_| desktop_state_problem(correlation_id))? = None;
        *self
            .auth_expires_at
            .lock()
            .map_err(|_| desktop_state_problem(correlation_id))? = 0;
        *self
            .auth_refresh_uncertain
            .lock()
            .map_err(|_| desktop_state_problem(correlation_id))? = false;
        if let Some(version) = self
            .auth_vault_version
            .lock()
            .map_err(|_| desktop_state_problem(correlation_id))?
            .take()
        {
            self.remove_persisted_bearer(version, correlation_id)?;
        }
        self.profile_runtime.set_coordination(None).await?;
        Ok(())
    }

    fn stage_refresh_attempt(&self, correlation_id: &str) -> Result<(), Problem> {
        let mut vault = self
            .auth_vault
            .lock()
            .map_err(|_| desktop_state_problem(correlation_id))?;
        let mut version = self
            .auth_vault_version
            .lock()
            .map_err(|_| desktop_state_problem(correlation_id))?;
        let current = version.ok_or_else(|| desktop_state_problem(correlation_id))?;
        let next = vault.mark_refresh_pending(&self.device_id, current, correlation_id)?;
        *version = Some(next);
        *self
            .auth_refresh_uncertain
            .lock()
            .map_err(|_| desktop_state_problem(correlation_id))? = true;
        Ok(())
    }

    fn clear_refresh_attempt(&self, correlation_id: &str) -> Result<(), Problem> {
        let mut vault = self
            .auth_vault
            .lock()
            .map_err(|_| desktop_state_problem(correlation_id))?;
        let mut version = self
            .auth_vault_version
            .lock()
            .map_err(|_| desktop_state_problem(correlation_id))?;
        let current = version.ok_or_else(|| desktop_state_problem(correlation_id))?;
        *version = Some(vault.clear_refresh_pending(&self.device_id, current, correlation_id)?);
        *self
            .auth_refresh_uncertain
            .lock()
            .map_err(|_| desktop_state_problem(correlation_id))? = false;
        Ok(())
    }

    async fn disable_local_auth(&self, correlation_id: &str) -> Result<(), Problem> {
        self.client.clear_token()?;
        *self
            .auth
            .write()
            .map_err(|_| desktop_state_problem(correlation_id))? = None;
        *self
            .auth_refresh_token
            .lock()
            .map_err(|_| desktop_state_problem(correlation_id))? = None;
        *self
            .auth_expires_at
            .lock()
            .map_err(|_| desktop_state_problem(correlation_id))? = 0;
        self.profile_runtime.set_coordination(None).await?;
        Ok(())
    }

    async fn invoke(
        &self,
        invoke_name: &str,
        correlation_id: &str,
        idempotency_key: Option<&str>,
        payload: Value,
    ) -> Result<CommandResult<Value>, Problem> {
        let result = self
            .invoke_authenticated(invoke_name, correlation_id, idempotency_key, payload)
            .await;
        if let Err(error) = &result
            && definitive_auth_error(error)
        {
            let _auth_guard = self.auth_refresh_gate.lock().await;
            if self.client.has_token()? {
                self.clear_local_auth(correlation_id).await?;
            }
        }
        result
    }

    async fn invoke_authenticated(
        &self,
        invoke_name: &str,
        correlation_id: &str,
        idempotency_key: Option<&str>,
        payload: Value,
    ) -> Result<CommandResult<Value>, Problem> {
        self.refresh_session_if_needed(correlation_id).await?;
        self.ensure_authenticated_coordination(correlation_id)
            .await?;
        if !self.client.has_token()? {
            return Err(Problem::new(
                "authentication_required",
                "Sign in to BugLogin",
                401,
                correlation_id,
            ));
        }
        if invoke_name == "profile_share" {
            return self
                .invoke_profile_share(correlation_id, idempotency_key, payload)
                .await;
        }
        match invoke_name {
            "cookie_stage" => self.cookie_stage(payload, correlation_id).await,
            "cookie_import" => {
                self.cookie_import(payload, idempotency_key, correlation_id)
                    .await
            }
            "cookie_copy" => {
                self.cookie_copy(payload, idempotency_key, correlation_id)
                    .await
            }
            "proxy_parse" => local_proxy_parse(payload, correlation_id),
            "proxy_import" => {
                self.import_proxy_text(payload, idempotency_key, correlation_id)
                    .await
            }
            "proxy_get" => {
                let input: ProxyCheckInput = serde_json::from_value(payload.clone())
                    .map_err(|_| invalid_input("proxyId", correlation_id))?;
                let workspace_id = WorkspaceId::new(input.workspace_id)
                    .map_err(|_| invalid_input("workspaceId", correlation_id))?;
                let proxy_id = ProxyId::new(input.proxy_id)
                    .map_err(|_| invalid_input("proxyId", correlation_id))?;
                self.reconcile_pending_proxy_mutation(&workspace_id, &proxy_id, correlation_id)
                    .await?;
                self.client
                    .invoke(invoke_name, correlation_id, idempotency_key, payload)
                    .await
            }
            "proxy_check" => {
                let input: ProxyCheckInput = serde_json::from_value(payload)
                    .map_err(|_| invalid_input("proxyId", correlation_id))?;
                let workspace_id = WorkspaceId::new(input.workspace_id)
                    .map_err(|_| invalid_input("workspaceId", correlation_id))?;
                let proxy_id = ProxyId::new(input.proxy_id)
                    .map_err(|_| invalid_input("proxyId", correlation_id))?;
                self.reconcile_pending_proxy_mutation(&workspace_id, &proxy_id, correlation_id)
                    .await?;
                let proxy = self
                    .client
                    .proxy_for_launch(&workspace_id, &proxy_id, correlation_id)
                    .await?;
                let credential = self.proxy_credential_vault.load(
                    &workspace_id,
                    &proxy.proxy_id,
                    proxy.revision,
                    correlation_id,
                )?;
                if proxy.has_credential && credential.is_none() {
                    return Err(Problem::new(
                        "proxy_secret_unavailable",
                        "This device does not have the selected proxy credentials in its operating system vault",
                        409,
                        correlation_id,
                    ));
                }
                if !proxy.has_credential && credential.is_some() {
                    return Err(Problem::new(
                        "proxy_credential_state_mismatch",
                        "Proxy credential state does not match the API record",
                        409,
                        correlation_id,
                    ));
                }
                let latency_ms = crate::proxy_bridge::check_proxy_connectivity(ProxyBridgeSpec {
                    protocol: proxy.protocol,
                    host: proxy.host,
                    port: proxy.port,
                    credential,
                })
                .await?;
                Ok(result(
                    "proxy.check",
                    correlation_id,
                    json!({
                        "proxyId": proxy_id,
                        "revision": proxy.revision,
                        "status": "healthy",
                        "latencyMs": latency_ms
                    }),
                ))
            }
            "proxy_create" | "proxy_update" | "proxy_delete" => {
                self.invoke_proxy_mutation(invoke_name, correlation_id, idempotency_key, payload)
                    .await
            }
            "profile_sync_recovery_code_get" => {
                let input: ProfileRecoveryCodeInput = parse_payload(payload, correlation_id)?;
                let profile_id = parse_profile_id(&input.profile_id, correlation_id)?;
                let code = self
                    .profile_sync_recovery_code_get(&profile_id, correlation_id)
                    .await?;
                Ok(result(
                    "profile.sync.recovery.code.get",
                    correlation_id,
                    json!({ "recoveryCode": code }),
                ))
            }
            "profile_sync_recover" => {
                let key = validate_idempotency_key(idempotency_key, correlation_id)?;
                let hash = payload_hash(&payload)?;
                let input: ProfileSyncRecoveryInput = parse_payload(payload, correlation_id)?;
                let profile_id = parse_profile_id(&input.profile_id, correlation_id)?;
                let action_correlation = correlation_id.to_owned();
                let recovery_code = input.recovery_code;
                let data = self
                    .run_operation(
                        "profile.sync.recover",
                        key,
                        &hash,
                        correlation_id,
                        move |state| async move {
                            state
                                .profile_sync_recover(
                                    &profile_id,
                                    &recovery_code,
                                    &action_correlation,
                                )
                                .await?;
                            Ok(json!({ "recovered": true }))
                        },
                    )
                    .await?;
                Ok(result("profile.sync.recover", correlation_id, data))
            }
            "migration_profile_data_import" => {
                let key = validate_idempotency_key(idempotency_key, correlation_id)?;
                let hash = payload_hash(&payload)?;
                let input: MigrationProfileDataImportInput =
                    parse_payload(payload, correlation_id)?;
                let source_root = input.source_root;
                let action_correlation = correlation_id.to_owned();
                let operation_key = key.to_owned();
                let data = self
                    .run_operation(
                        "migration.profile.data.import",
                        key,
                        &hash,
                        correlation_id,
                        move |state| async move {
                            state
                                .import_legacy_profile_data_tree(
                                    source_root,
                                    operation_key,
                                    action_correlation,
                                )
                                .await
                        },
                    )
                    .await?;
                Ok(result(
                    "migration.profile.data.import",
                    correlation_id,
                    data,
                ))
            }
            "profile_launch" => {
                let key = validate_idempotency_key(idempotency_key, correlation_id)?;
                let hash = payload_hash(&payload)?;
                let input: ProfileLaunchInput = parse_payload(payload, correlation_id)?;
                let profile_id = parse_profile_id(&input.profile_id, correlation_id)?;
                let action_correlation = correlation_id.to_owned();
                let runtime_key = key.to_owned();
                let data = self
                    .run_operation(
                        "profile.launch",
                        key,
                        &hash,
                        correlation_id,
                        move |state| async move {
                            let _profile_operation = state
                                .profile_operations
                                .lock_for(&profile_id)
                                .lock_owned()
                                .await;
                            state
                                .launch_profile(
                                    profile_id,
                                    input.headless,
                                    runtime_key,
                                    action_correlation,
                                )
                                .await
                        },
                    )
                    .await?;
                Ok(result("profile.launch", correlation_id, data))
            }
            "profile_stop" => {
                let key = validate_idempotency_key(idempotency_key, correlation_id)?;
                let hash = payload_hash(&payload)?;
                let input: ProfileStopInput = parse_payload(payload, correlation_id)?;
                let profile_id = parse_profile_id(&input.profile_id, correlation_id)?;
                let action_correlation = correlation_id.to_owned();
                let sync_idempotency_key = key.to_owned();
                let data = self
                    .run_operation(
                        "profile.stop",
                        key,
                        &hash,
                        correlation_id,
                        move |state| async move {
                            let _profile_operation = state
                                .profile_operations
                                .lock_for(&profile_id)
                                .lock_owned()
                                .await;
                            state
                                .stop_profile(
                                    profile_id,
                                    sync_idempotency_key,
                                    action_correlation,
                                    None,
                                    None,
                                    None,
                                )
                                .await
                        },
                    )
                    .await?;
                Ok(result("profile.stop", correlation_id, data))
            }
            "profile_keyrotation_execute" => {
                let key = validate_idempotency_key(idempotency_key, correlation_id)?;
                let hash = payload_hash(&payload)?;
                let input: ProfileKeyRotationExecuteInput = parse_payload(payload, correlation_id)?;
                let profile_id = parse_profile_id(&input.profile_id, correlation_id)?;
                let action_correlation = correlation_id.to_owned();
                let sync_idempotency_key = key.to_owned();
                let confirmed_recipients = input.confirmed_recipients;
                let data = self
                    .run_operation(
                        "profile.keyrotation.execute",
                        key,
                        &hash,
                        correlation_id,
                        move |state| async move {
                            let _profile_operation = state
                                .profile_operations
                                .lock_for(&profile_id)
                                .lock_owned()
                                .await;
                            state
                                .stop_profile(
                                    profile_id,
                                    sync_idempotency_key,
                                    action_correlation,
                                    Some(confirmed_recipients),
                                    None,
                                    None,
                                )
                                .await
                        },
                    )
                    .await?;
                Ok(result("profile.keyrotation.execute", correlation_id, data))
            }
            "operation_get" => self.operation_get(payload, correlation_id),
            _ => {
                self.client
                    .invoke(invoke_name, correlation_id, idempotency_key, payload)
                    .await
            }
        }
    }

    async fn ensure_device_key_registered(&self, correlation_id: &str) -> Result<(), Problem> {
        if self.device_key_enrolled.load(Ordering::Acquire) {
            return Ok(());
        }
        let private_key = self
            .device_key_vault
            .load_or_create(&self.device_id, correlation_id)?;
        let public_key = private_key.public_key()?;
        let public_key_hex = encode_hex(public_key.as_bytes());
        let fingerprint = format!("{:x}", Sha256::digest(public_key.as_bytes()));
        let idempotency_key = format!("device-key-enrollment-{fingerprint}");
        self.client
            .register_device_encryption_key(
                &self.device_id,
                &public_key_hex,
                &fingerprint,
                &idempotency_key,
                correlation_id,
            )
            .await?;
        self.device_key_enrolled.store(true, Ordering::Release);
        Ok(())
    }

    async fn recover_profile_key_envelope(
        &self,
        binding: &buglogin_domain::profile::ProfileResource,
        profile_id: &ProfileId,
        key_version: &str,
        correlation_id: &str,
    ) -> Result<crate::sync_key_vault::ResourceKeys, Problem> {
        self.ensure_device_key_registered(correlation_id).await?;
        let envelope = match self
            .client
            .profile_key_share_envelope(profile_id, correlation_id)
            .await
        {
            Ok(envelope) => envelope,
            Err(error) if error.code == "profile_key_envelope_not_found" => {
                return Err(Problem::new(
                    "sync_key_recovery_required",
                    "Enter this profile's recovery code on this device to open it",
                    409,
                    correlation_id,
                ));
            }
            Err(error) => return Err(error),
        };
        let device_private_key = self
            .device_key_vault
            .load_or_create(&self.device_id, correlation_id)?;
        let shared = buglogin_sync::crypto::open_profile_key_share(
            &envelope,
            &binding.workspace_id,
            &binding.resource_id,
            &self.device_id,
            key_version,
            &device_private_key,
        )?;
        self.sync_key_vault.replace_recovered_for_manifest(
            &binding.workspace_id,
            &binding.resource_id,
            shared.client,
            shared.recovery,
            key_version,
            correlation_id,
        )?;
        self.sync_key_vault
            .load_for_manifest(
                &binding.workspace_id,
                &binding.resource_id,
                key_version,
                correlation_id,
            )?
            .ok_or_else(|| {
                Problem::new(
                    "sync_key_vault_unavailable",
                    "Shared profile keys were not persisted on this device",
                    503,
                    correlation_id,
                )
            })
    }

    async fn invoke_profile_share(
        &self,
        correlation_id: &str,
        idempotency_key: Option<&str>,
        mut payload: Value,
    ) -> Result<CommandResult<Value>, Problem> {
        let confirmations = payload
            .as_object_mut()
            .and_then(|payload| payload.remove("confirmedDeviceFingerprints"))
            .map(|value| {
                serde_json::from_value::<Vec<ConfirmedDeviceFingerprint>>(value)
                    .map_err(|_| invalid_input("confirmedDeviceFingerprints", correlation_id))
            })
            .transpose()?
            .unwrap_or_default();
        let mut request: ProfileShareInput = serde_json::from_value(payload)
            .map_err(|_| invalid_input("profileShare", correlation_id))?;
        let binding = self
            .client
            .profile_binding(&request.profile_id, correlation_id)
            .await?;
        let auth = self.auth(correlation_id)?;
        let manifest = self
            .client
            .current_manifest(
                CoordinationScope {
                    workspace_id: &binding.workspace_id,
                    auth: &auth,
                },
                &binding.resource_id,
            )
            .await?;

        if let Some(manifest) = manifest.filter(|manifest| manifest.payload_encryption.is_some()) {
            let key_version = manifest
                .payload_encryption
                .as_ref()
                .and_then(|encryption| encryption.key_version.as_deref())
                .ok_or_else(|| {
                    Problem::new(
                        "profile_key_version_missing",
                        "Profile sync manifest has no active resource key version",
                        409,
                        correlation_id,
                    )
                })?;
            let keys = self
                .sync_key_vault
                .load(&binding.workspace_id, &binding.resource_id, correlation_id)?
                .ok_or_else(|| {
                    Problem::new(
                        "sync_key_recovery_required",
                        "Recover this profile's sync keys on this device before sharing it",
                        409,
                        correlation_id,
                    )
                })?;
            let actual_key_version = format!("{:x}", Sha256::digest(keys.client.as_bytes()));
            if actual_key_version != key_version {
                return Err(Problem::new(
                    "profile_key_version_stale",
                    "The local profile key does not match the current synced profile; refresh before sharing",
                    409,
                    correlation_id,
                ));
            }

            let device_keys = self
                .client
                .profile_share_recipient_device_keys(
                    &request.profile_id,
                    &request.member_id,
                    correlation_id,
                )
                .await?;
            if device_keys.is_empty()
                || device_keys.len() > 64
                || confirmations.len() != device_keys.len()
            {
                return Err(Problem::new(
                    "recipient_device_fingerprint_review_required",
                    "Review every active recipient device fingerprint before sharing this encrypted profile",
                    409,
                    correlation_id,
                ));
            }
            let mut confirmed_by_device = BTreeMap::new();
            for confirmation in confirmations {
                if DeviceId::new(confirmation.device_id.clone()).is_err()
                    || confirmation.fingerprint.len() != 64
                    || !confirmation
                        .fingerprint
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit())
                    || confirmed_by_device
                        .insert(confirmation.device_id.clone(), confirmation.fingerprint)
                        .is_some()
                {
                    return Err(invalid_input("confirmedDeviceFingerprints", correlation_id));
                }
            }
            if confirmed_by_device.len() != device_keys.len() {
                return Err(Problem::new(
                    "recipient_device_fingerprint_review_required",
                    "Review every active recipient device fingerprint before sharing this encrypted profile",
                    409,
                    correlation_id,
                ));
            }

            let mut envelopes = Vec::with_capacity(device_keys.len());
            for device in device_keys {
                let confirmed_fingerprint = confirmed_by_device
                    .remove(&device.device_id)
                    .filter(|fingerprint| *fingerprint == device.fingerprint)
                    .ok_or_else(|| {
                        Problem::new(
                            "recipient_device_fingerprint_changed",
                            "A recipient device fingerprint changed; review devices again",
                            409,
                            correlation_id,
                        )
                    })?;
                let public_key = parse_device_public_key(&device.public_key, correlation_id)?;
                let public_key_fingerprint = format!("{:x}", Sha256::digest(public_key.as_bytes()));
                if public_key_fingerprint != device.fingerprint {
                    return Err(Problem::new(
                        "recipient_device_key_invalid",
                        "Recipient device key does not match its registered fingerprint",
                        409,
                        correlation_id,
                    ));
                }
                let envelope = buglogin_sync::crypto::seal_profile_key_share(
                    binding.workspace_id.clone(),
                    binding.resource_id.clone(),
                    device.device_id,
                    key_version.to_owned(),
                    &public_key,
                    &keys.client,
                    &keys.recovery,
                )?;
                envelopes.push(ConfirmedProfileKeyShareEnvelope {
                    envelope,
                    confirmed_fingerprint,
                });
            }
            request.recipient_key_envelopes = envelopes;
        } else {
            request.recipient_key_envelopes.clear();
        }

        let payload = serde_json::to_value(request).map_err(|_| serialization(correlation_id))?;
        self.client
            .invoke("profile_share", correlation_id, idempotency_key, payload)
            .await
    }

    async fn import_proxy_text(
        &self,
        payload: Value,
        idempotency_key: Option<&str>,
        correlation_id: &str,
    ) -> Result<CommandResult<Value>, Problem> {
        let mut input: ProxyImportInput =
            serde_json::from_value(payload).map_err(|_| invalid_input("text", correlation_id))?;
        let workspace_id = WorkspaceId::new(std::mem::take(&mut input.workspace_id))
            .map_err(|_| invalid_input("workspaceId", correlation_id))?;
        if input.text.trim().is_empty()
            || input.text.len() > 1_048_576
            || input.created_at.trim().is_empty()
            || input.created_at.len() > 128
        {
            return Err(invalid_input("text", correlation_id));
        }
        let parent_key = validate_idempotency_key(idempotency_key, correlation_id)?.to_owned();
        let source_lines = input.text.lines().collect::<Vec<_>>();
        if source_lines
            .iter()
            .filter(|line| !line.trim().is_empty())
            .count()
            > 200
        {
            return Err(Problem::new(
                "proxy_import_too_many_rows",
                "Proxy import accepts at most 200 non-empty rows at a time",
                413,
                correlation_id,
            ));
        }

        let mut seen = Vec::<ProxyRow>::new();
        let mut parsed = Vec::<(usize, ProxyRow)>::new();
        let mut rows = Vec::<Value>::new();
        let mut duplicate_count = 0usize;
        let mut invalid_count = 0usize;
        for (line_index, raw) in source_lines.iter().enumerate() {
            if raw.trim().is_empty() {
                continue;
            }
            let report = parse_lines(raw);
            if let Some(row) = report.rows.into_iter().next() {
                if seen.contains(&row) {
                    duplicate_count += 1;
                    rows.push(json!({"index": line_index + 1, "status": "duplicate"}));
                } else {
                    seen.push(row.clone());
                    parsed.push((line_index + 1, row));
                }
            } else {
                invalid_count += 1;
                let error = report
                    .errors
                    .first()
                    .map(|item| item.code)
                    .unwrap_or("invalid_proxy");
                rows.push(json!({"index": line_index + 1, "status": "invalid", "error": error}));
            }
        }

        let mut created_count = 0usize;
        let mut failed_count = 0usize;
        for (line_index, mut row) in parsed {
            let proxy_id_hash = Sha256::digest(
                format!("{parent_key}:{line_index}:{}:{}", row.host, row.port).as_bytes(),
            );
            let proxy_id_digest = format!("{:x}", proxy_id_hash);
            let proxy_id = format!("prx_{}", &proxy_id_digest[..24]);
            let child_hash = Sha256::digest(format!("{parent_key}:{line_index}").as_bytes());
            let child_digest = format!("{:x}", child_hash);
            let child_key = format!("idem_proxy_import_{}", &child_digest[..32]);
            let credentials_provided = row.username.is_some() || row.password.is_some();
            let credentials_complete = row
                .username
                .as_ref()
                .is_some_and(|username| !username.is_empty())
                && row
                    .password
                    .as_ref()
                    .is_some_and(|password| !password.is_empty());
            let credential = if !credentials_provided {
                None
            } else {
                match (row.username.take(), row.password.take()) {
                    (Some(username), Some(password)) if credentials_complete => {
                        Some(ProxyCredentialInput { username, password })
                    }
                    (mut username, mut password) => {
                        if let Some(username) = &mut username {
                            username.zeroize();
                        }
                        if let Some(password) = &mut password {
                            password.zeroize();
                        }
                        failed_count += 1;
                        rows.push(json!({"index": line_index, "status": "failed", "error": "credential_password_required"}));
                        continue;
                    }
                }
            };
            let protocol =
                serde_json::to_value(&row.protocol).map_err(|_| serialization(correlation_id))?;
            let mut request = json!({
                "workspaceId": workspace_id.as_str(),
                "proxy": {
                    "id": proxy_id,
                    "protocol": protocol,
                    "host": row.host,
                    "port": row.port,
                    "bypassRules": [],
                    "createdAt": input.created_at.as_str(),
                    "updatedAt": input.created_at.as_str()
                }
            });
            if let Some(credential) = credential {
                request["credential"] =
                    serde_json::to_value(&credential).map_err(|_| serialization(correlation_id))?;
            }
            match self
                .invoke_proxy_mutation("proxy_create", correlation_id, Some(&child_key), request)
                .await
            {
                Ok(result)
                    if result.data.get("deduplicated").and_then(Value::as_bool) == Some(true) =>
                {
                    duplicate_count += 1;
                    rows.push(
                        json!({"index": line_index, "status": "duplicate", "proxyId": proxy_id}),
                    );
                }
                Ok(_) => {
                    created_count += 1;
                    rows.push(
                        json!({"index": line_index, "status": "created", "proxyId": proxy_id}),
                    );
                }
                Err(problem) => {
                    failed_count += 1;
                    rows.push(json!({"index": line_index, "status": "failed", "proxyId": proxy_id, "error": problem.code}));
                }
            }
        }
        rows.sort_by_key(|row| row.get("index").and_then(Value::as_u64).unwrap_or_default());

        Ok(result(
            "proxy.import",
            correlation_id,
            json!({
                "createdCount": created_count,
                "duplicateCount": duplicate_count,
                "invalidCount": invalid_count,
                "failedCount": failed_count,
                "rows": rows
            }),
        ))
    }

    async fn invoke_proxy_mutation(
        &self,
        invoke_name: &str,
        correlation_id: &str,
        idempotency_key: Option<&str>,
        payload: Value,
    ) -> Result<CommandResult<Value>, Problem> {
        let mut payload = SensitiveJson::new(payload);
        let _mutation_guard = self.proxy_mutation_gate.lock().await;
        let idempotency_key = validate_idempotency_key(idempotency_key, correlation_id)?.to_owned();
        let workspace_id = payload
            .get("workspaceId")
            .and_then(Value::as_str)
            .and_then(|value| WorkspaceId::new(value.to_owned()).ok())
            .ok_or_else(|| invalid_input("workspaceId", correlation_id))?;
        let proxy_id_value = if invoke_name == "proxy_create" {
            payload.pointer("/proxy/id").and_then(Value::as_str)
        } else {
            payload.get("proxyId").and_then(Value::as_str)
        };
        let proxy_id = proxy_id_value
            .and_then(|value| ProxyId::new(value.to_owned()).ok())
            .ok_or_else(|| invalid_input("proxyId", correlation_id))?;
        let _process_guard = self.proxy_credential_vault.acquire_mutation_lock(
            &workspace_id,
            &proxy_id,
            correlation_id,
        )?;

        if payload.pointer("/proxy/credential").is_some() {
            return Err(invalid_input("credential", correlation_id));
        }
        let credential = payload
            .as_object_mut()
            .and_then(|object| object.remove("credential"));
        let clear_credential = match payload
            .as_object_mut()
            .and_then(|object| object.remove("clearCredential"))
        {
            Some(Value::Bool(value)) => value,
            Some(_) => return Err(invalid_input("clearCredential", correlation_id)),
            None => false,
        };
        let secret = match credential {
            Some(value) => {
                let input: ProxyCredentialInput = serde_json::from_value(value)
                    .map_err(|_| invalid_input("credential", correlation_id))?;
                if input.username.trim().is_empty() || input.password.is_empty() {
                    return Err(invalid_input("credential", correlation_id));
                }
                let encoded =
                    serde_json::to_vec(&input).map_err(|_| serialization(correlation_id))?;
                Some(
                    buglogin_storage::secrets::SecretValue::new(encoded)
                        .map_err(|_| serialization(correlation_id))?,
                )
            }
            None => None,
        };
        if clear_credential && secret.is_some() {
            return Err(invalid_input("credential", correlation_id));
        }
        if let Some(proxy) = payload.get_mut("proxy").and_then(Value::as_object_mut) {
            match (&secret, clear_credential) {
                (Some(_), _) => {
                    proxy.insert("hasCredential".into(), Value::Bool(true));
                }
                (None, true) => {
                    proxy.insert("hasCredential".into(), Value::Bool(false));
                }
                (None, false) => {
                    proxy.remove("hasCredential");
                }
            }
        }
        // Persist only a digest of the sanitized wire payload. Hashing the
        // original credential-bearing JSON would create an offline password
        // guessing oracle in the durable OS-vault journal.
        let request_hash = payload_hash(&payload)?;

        if let Some(bytes) =
            self.proxy_credential_vault
                .load_pending(&workspace_id, &proxy_id, correlation_id)?
        {
            let pending: PendingProxyMutation =
                serde_json::from_slice(bytes.as_bytes()).map_err(|_| {
                    Problem::new(
                        "proxy_mutation_recovery_invalid",
                        "A pending proxy change cannot be safely recovered; contact support before editing this proxy",
                        500,
                        correlation_id,
                    )
                })?;
            let credential_conflict = secret.as_ref().is_some_and(|candidate| {
                pending
                    .credential_bytes
                    .as_ref()
                    .is_none_or(|stored| stored.as_bytes() != candidate.as_bytes())
            });
            if pending.idempotency_key == idempotency_key
                && (pending.wire_hash != request_hash
                    || pending.command != invoke_name
                    || credential_conflict)
            {
                return Err(Problem::new(
                    "idempotency_conflict",
                    "Idempotency key was already used for a different proxy change",
                    409,
                    correlation_id,
                ));
            }
            let replay = self
                .replay_pending_proxy_mutation(&workspace_id, &proxy_id, &pending, correlation_id)
                .await;
            match replay {
                Ok(result) => {
                    let duplicate_create = pending.command == "proxy_create"
                        && result.data.get("deduplicated").and_then(Value::as_bool) == Some(true)
                        && pending.credential_bytes.is_some();
                    if duplicate_create {
                        self.proxy_credential_vault.remove_pending(
                            &workspace_id,
                            &proxy_id,
                            correlation_id,
                        )?;
                    } else {
                        self.finalize_pending_proxy_mutation(
                            &workspace_id,
                            &proxy_id,
                            &pending,
                            &result,
                            correlation_id,
                        )?;
                        self.proxy_credential_vault.remove_pending(
                            &workspace_id,
                            &proxy_id,
                            correlation_id,
                        )?;
                    }
                    if pending.idempotency_key == idempotency_key {
                        if duplicate_create {
                            return Err(Problem::new(
                                "proxy_duplicate_credentials_conflict",
                                "A proxy with this endpoint already exists; edit that proxy to change its saved credentials",
                                409,
                                correlation_id,
                            ));
                        }
                        return Ok(result);
                    }
                }
                Err(error) if proxy_mutation_requires_recovery(&error) => {
                    let should_retry = ambiguous_proxy_mutation_error(&error);
                    return Err(error.retryable(should_retry));
                }
                Err(error) => {
                    self.proxy_credential_vault.remove_pending(
                        &workspace_id,
                        &proxy_id,
                        correlation_id,
                    )?;
                    if pending.idempotency_key == idempotency_key {
                        return Err(error);
                    }
                }
            }
        }

        let previous_revision = payload.get("expectedRevision").and_then(Value::as_u64);
        let previous = match previous_revision {
            Some(revision) => self.proxy_credential_vault.load(
                &workspace_id,
                &proxy_id,
                revision,
                correlation_id,
            )?,
            None => None,
        };
        let pending = PendingProxyMutation {
            schema_version: PROXY_MUTATION_JOURNAL_SCHEMA_VERSION,
            command: invoke_name.to_owned(),
            idempotency_key: idempotency_key.clone(),
            request_hash,
            wire_hash: payload_hash(&payload)?,
            payload: payload.as_value().clone(),
            credential_bytes: if clear_credential || invoke_name == "proxy_delete" {
                None
            } else {
                secret
                    .as_ref()
                    .map(|value| SensitiveBytes::new(value.as_bytes().to_vec()))
                    .or_else(|| {
                        previous
                            .as_ref()
                            .map(|value| SensitiveBytes::new(value.as_bytes().to_vec()))
                    })
            },
            previous_revision,
            clear_credential,
        };
        let pending_bytes =
            serde_json::to_vec(&pending).map_err(|_| serialization(correlation_id))?;
        self.proxy_credential_vault.store_pending(
            &workspace_id,
            &proxy_id,
            pending_bytes,
            correlation_id,
        )?;
        let result = self
            .client
            .invoke(
                invoke_name,
                correlation_id,
                Some(&idempotency_key),
                payload.into_value(),
            )
            .await;
        match result {
            Ok(result) => {
                if invoke_name == "proxy_create"
                    && result.data.get("deduplicated").and_then(Value::as_bool) == Some(true)
                    && secret.is_some()
                {
                    self.proxy_credential_vault.remove_pending(
                        &workspace_id,
                        &proxy_id,
                        correlation_id,
                    )?;
                    return Err(Problem::new(
                        "proxy_duplicate_credentials_conflict",
                        "A proxy with this endpoint already exists; edit that proxy to change its saved credentials",
                        409,
                        correlation_id,
                    ));
                }
                self.finalize_pending_proxy_mutation(
                    &workspace_id,
                    &proxy_id,
                    &pending,
                    &result,
                    correlation_id,
                )?;
                self.proxy_credential_vault.remove_pending(
                    &workspace_id,
                    &proxy_id,
                    correlation_id,
                )?;
                Ok(result)
            }
            Err(error) if proxy_mutation_requires_recovery(&error) => {
                let should_retry = ambiguous_proxy_mutation_error(&error);
                Err(error.retryable(should_retry))
            }
            Err(error) => {
                self.proxy_credential_vault.remove_pending(
                    &workspace_id,
                    &proxy_id,
                    correlation_id,
                )?;
                Err(error)
            }
        }
    }

    async fn replay_pending_proxy_mutation(
        &self,
        workspace_id: &WorkspaceId,
        proxy_id: &ProxyId,
        pending: &PendingProxyMutation,
        correlation_id: &str,
    ) -> Result<CommandResult<Value>, Problem> {
        if pending.schema_version != PROXY_MUTATION_JOURNAL_SCHEMA_VERSION {
            return Err(Problem::new(
                "proxy_mutation_recovery_version_unsupported",
                "This pending proxy change was written by an incompatible BugLogin version; update BugLogin before using this proxy",
                409,
                correlation_id,
            ));
        }
        validate_idempotency_key(Some(&pending.idempotency_key), correlation_id).map_err(|_| {
            Problem::new(
                "proxy_mutation_recovery_invalid",
                "A pending proxy change cannot be safely recovered; contact support before editing this proxy",
                500,
                correlation_id,
            )
        })?;
        if payload_hash(&pending.payload)? != pending.wire_hash {
            return Err(Problem::new(
                "proxy_mutation_recovery_invalid",
                "A pending proxy change cannot be safely recovered; contact support before editing this proxy",
                500,
                correlation_id,
            ));
        }
        if !matches!(
            pending.command.as_str(),
            "proxy_create" | "proxy_update" | "proxy_delete"
        ) || pending.payload.get("workspaceId").and_then(Value::as_str)
            != Some(workspace_id.as_str())
        {
            return Err(Problem::new(
                "proxy_mutation_recovery_invalid",
                "A pending proxy change cannot be safely recovered; contact support before editing this proxy",
                500,
                correlation_id,
            ));
        }
        let payload_proxy_id = if pending.command == "proxy_create" {
            pending.payload.pointer("/proxy/id").and_then(Value::as_str)
        } else {
            pending.payload.get("proxyId").and_then(Value::as_str)
        };
        if payload_proxy_id != Some(proxy_id.as_str()) {
            return Err(Problem::new(
                "proxy_mutation_recovery_invalid",
                "A pending proxy change cannot be safely recovered; contact support before editing this proxy",
                500,
                correlation_id,
            ));
        }
        self.client
            .invoke(
                &pending.command,
                correlation_id,
                Some(&pending.idempotency_key),
                pending.payload.clone(),
            )
            .await
    }

    async fn reconcile_pending_proxy_mutation(
        &self,
        workspace_id: &WorkspaceId,
        proxy_id: &ProxyId,
        correlation_id: &str,
    ) -> Result<(), Problem> {
        let _mutation_guard = self.proxy_mutation_gate.lock().await;
        let _process_guard = self.proxy_credential_vault.acquire_mutation_lock(
            workspace_id,
            proxy_id,
            correlation_id,
        )?;
        let Some(bytes) =
            self.proxy_credential_vault
                .load_pending(workspace_id, proxy_id, correlation_id)?
        else {
            return Ok(());
        };
        let pending: PendingProxyMutation = serde_json::from_slice(bytes.as_bytes()).map_err(|_| {
            Problem::new(
                "proxy_mutation_recovery_invalid",
                "A pending proxy change cannot be safely recovered; contact support before using this proxy",
                500,
                correlation_id,
            )
        })?;
        let result = match self
            .replay_pending_proxy_mutation(workspace_id, proxy_id, &pending, correlation_id)
            .await
        {
            Ok(result) => result,
            Err(error) => {
                if proxy_mutation_requires_recovery(&error) {
                    let should_retry = ambiguous_proxy_mutation_error(&error);
                    return Err(error.retryable(should_retry));
                }
                self.proxy_credential_vault.remove_pending(
                    workspace_id,
                    proxy_id,
                    correlation_id,
                )?;
                return Err(error);
            }
        };
        if pending.command == "proxy_create"
            && result.data.get("deduplicated").and_then(Value::as_bool) == Some(true)
            && pending.credential_bytes.is_some()
        {
            self.proxy_credential_vault
                .remove_pending(workspace_id, proxy_id, correlation_id)?;
            return Err(Problem::new(
                "proxy_duplicate_credentials_conflict",
                "A proxy with this endpoint already exists; review the proxy list before using this profile",
                409,
                correlation_id,
            ));
        }
        self.finalize_pending_proxy_mutation(
            workspace_id,
            proxy_id,
            &pending,
            &result,
            correlation_id,
        )?;
        self.proxy_credential_vault
            .remove_pending(workspace_id, proxy_id, correlation_id)?;
        Ok(())
    }

    fn finalize_pending_proxy_mutation(
        &self,
        workspace_id: &WorkspaceId,
        proxy_id: &ProxyId,
        pending: &PendingProxyMutation,
        result: &CommandResult<Value>,
        correlation_id: &str,
    ) -> Result<(), Problem> {
        if pending.command == "proxy_delete" {
            if let Some(revision) = pending.previous_revision {
                self.proxy_credential_vault.remove(
                    workspace_id,
                    proxy_id,
                    revision,
                    correlation_id,
                )?;
            }
            return Ok(());
        }
        let record = if pending.command == "proxy_create" {
            result.data.get("record")
        } else {
            Some(&result.data)
        }
        .ok_or_else(|| invalid_proxy_mutation_response(correlation_id))?;
        let revision = record
            .get("revision")
            .and_then(Value::as_u64)
            .ok_or_else(|| {
                Problem::new(
                    "proxy_revision_missing",
                    "BugLogin API did not return the updated proxy revision",
                    502,
                    correlation_id,
                )
            })?;
        let proxy = record
            .get("proxy")
            .ok_or_else(|| invalid_proxy_mutation_response(correlation_id))?;
        let returned_workspace = record.get("workspaceId").and_then(Value::as_str);
        let returned_proxy = proxy.get("id").and_then(Value::as_str);
        let has_credential = proxy.get("hasCredential").and_then(Value::as_bool);
        let credential_version = proxy.get("credentialVersion").and_then(Value::as_str);
        let has_valid_credential_version = credential_version
            .and_then(|value| CredentialId::new(value.to_owned()).ok())
            .is_some();
        if returned_workspace != Some(workspace_id.as_str())
            || returned_proxy != Some(proxy_id.as_str())
            || has_credential.is_none()
            || (has_credential == Some(true) && !has_valid_credential_version)
            || (has_credential == Some(false) && credential_version.is_some())
            || (pending.clear_credential && has_credential != Some(false))
            || (pending.credential_bytes.is_some() && has_credential != Some(true))
        {
            return Err(invalid_proxy_mutation_response(correlation_id));
        }
        if pending.clear_credential {
            if let Some(previous_revision) = pending.previous_revision {
                self.proxy_credential_vault.remove(
                    workspace_id,
                    proxy_id,
                    previous_revision,
                    correlation_id,
                )?;
            }
            return Ok(());
        }
        if let Some(bytes) = &pending.credential_bytes {
            let credential = buglogin_storage::secrets::SecretValue::new(bytes.as_bytes().to_vec())
                .map_err(|_| serialization(correlation_id))?;
            self.proxy_credential_vault.store(
                workspace_id,
                proxy_id,
                revision,
                credential,
                correlation_id,
            )?;
        }
        if let Some(previous_revision) = pending.previous_revision {
            if previous_revision != revision {
                self.proxy_credential_vault.remove(
                    workspace_id,
                    proxy_id,
                    previous_revision,
                    correlation_id,
                )?;
            }
        }
        Ok(())
    }

    async fn profile_sync_recovery_code_get(
        &self,
        profile_id: &ProfileId,
        correlation_id: &str,
    ) -> Result<String, Problem> {
        let auth = self.auth(correlation_id)?;
        let binding = self
            .client
            .profile_binding(profile_id, correlation_id)
            .await?;
        if binding.profile_id != *profile_id || binding.archive_state != ProfileArchiveState::Active
        {
            return Err(Problem::new(
                "managed_profile_binding_required",
                "Managed profile requires an active binding",
                409,
                correlation_id,
            ));
        }
        let scope = CoordinationScope {
            workspace_id: &binding.workspace_id,
            auth: &auth,
        };
        let manifest = self
            .client
            .current_manifest(scope.clone(), &binding.resource_id)
            .await?
            .ok_or_else(|| {
                Problem::new(
                    "production_manifest_not_found",
                    "Production profile manifest not found",
                    409,
                    correlation_id,
                )
            })?;
        let keys = if let Some(encryption) = manifest.payload_encryption.as_ref() {
            let keys = self
                .sync_key_vault
                .load(&binding.workspace_id, &binding.resource_id, correlation_id)?
                .ok_or_else(|| {
                    Problem::new(
                        "sync_key_recovery_required",
                        "This device does not have this profile's recovery key",
                        409,
                        correlation_id,
                    )
                })?;
            let envelope_bytes = self
                .client
                .download_blob(
                    scope,
                    &binding.resource_id,
                    &encryption.resource_key_envelope.key,
                    manifest.generation,
                    &encryption.resource_key_envelope.checksum,
                    encryption.resource_key_envelope.size,
                    correlation_id,
                )
                .await?;
            let envelope: buglogin_sync::crypto::ResourceKeyEnvelope =
                serde_json::from_slice(&envelope_bytes).map_err(|_| {
                    Problem::new(
                        "resource_key_envelope_invalid",
                        "Resource key envelope is invalid",
                        409,
                        correlation_id,
                    )
                })?;
            let unwrapped =
                envelope.unwrap(&binding.workspace_id, &binding.resource_id, &keys.recovery)?;
            if unwrapped != keys.client {
                return Err(Problem::new(
                    "resource_key_vault_mismatch",
                    "Stored profile key does not match the active manifest",
                    409,
                    correlation_id,
                ));
            }
            keys
        } else {
            if !manifest.objects.is_empty() {
                return Err(Problem::new(
                    "profile_payload_encryption_required",
                    "This profile has no supported client-side encryption metadata",
                    409,
                    correlation_id,
                ));
            }
            self.sync_key_vault.load_or_create(
                &binding.workspace_id,
                &binding.resource_id,
                correlation_id,
            )?
        };
        Ok(keys.recovery.recovery_code())
    }

    async fn profile_sync_recover(
        &self,
        profile_id: &ProfileId,
        recovery_code: &str,
        correlation_id: &str,
    ) -> Result<(), Problem> {
        let auth = self.auth(correlation_id)?;
        let binding = self
            .client
            .profile_binding(profile_id, correlation_id)
            .await?;
        if binding.profile_id != *profile_id || binding.archive_state != ProfileArchiveState::Active
        {
            return Err(Problem::new(
                "managed_profile_binding_required",
                "Managed profile requires an active binding",
                409,
                correlation_id,
            ));
        }
        let scope = CoordinationScope {
            workspace_id: &binding.workspace_id,
            auth: &auth,
        };
        let manifest = self
            .client
            .current_manifest(scope.clone(), &binding.resource_id)
            .await?
            .ok_or_else(|| {
                Problem::new(
                    "production_manifest_not_found",
                    "Production profile manifest not found",
                    409,
                    correlation_id,
                )
            })?;
        let encryption = manifest.payload_encryption.as_ref().ok_or_else(|| {
            Problem::new(
                "profile_payload_encryption_required",
                "This profile has no supported client-side encryption metadata",
                409,
                correlation_id,
            )
        })?;
        let envelope_bytes = self
            .client
            .download_blob(
                scope,
                &binding.resource_id,
                &encryption.resource_key_envelope.key,
                manifest.generation,
                &encryption.resource_key_envelope.checksum,
                encryption.resource_key_envelope.size,
                correlation_id,
            )
            .await?;
        let envelope: buglogin_sync::crypto::ResourceKeyEnvelope =
            serde_json::from_slice(&envelope_bytes).map_err(|_| {
                Problem::new(
                    "resource_key_envelope_invalid",
                    "Resource key envelope is invalid",
                    409,
                    correlation_id,
                )
            })?;
        let recovery = buglogin_sync::crypto::RecoveryKey::parse_code(recovery_code)?;
        let client = envelope.unwrap(&binding.workspace_id, &binding.resource_id, &recovery)?;
        let key_version = encryption.key_version.as_deref().ok_or_else(|| {
            Problem::new(
                "profile_key_version_missing",
                "Profile sync manifest has no active resource key version",
                409,
                correlation_id,
            )
        })?;
        self.sync_key_vault.replace_recovered_for_manifest(
            &binding.workspace_id,
            &binding.resource_id,
            client,
            recovery,
            key_version,
            correlation_id,
        )
    }

    fn operation_get(
        &self,
        payload: Value,
        correlation_id: &str,
    ) -> Result<CommandResult<Value>, Problem> {
        let input: OperationIdInput = parse_payload(payload, correlation_id)?;
        let grant = Grant {
            id: GrantId::new("grant_production_operation_read")
                .map_err(|_| serialization(correlation_id))?,
            account_id: None,
            member_id: None,
            capability: Capability::new("operation.read")
                .map_err(|_| serialization(correlation_id))?,
            action: Action::Read,
            scope: ResourceScope::global(),
            effect: Effect::Allow,
        };
        let operations = self
            .operations
            .read()
            .map_err(|_| unavailable("operation_unavailable", correlation_id))?;
        let operation = operations
            .get(&Principal::default(), &[grant], &input.operation_id)
            .map_err(|mut error| {
                error.correlation_id = correlation_id.to_owned();
                error
            })?;
        Ok(result(
            "operation.get",
            correlation_id,
            serde_json::to_value(operation).map_err(|_| serialization(correlation_id))?,
        ))
    }

    async fn launch_profile(
        &self,
        profile_id: ProfileId,
        headless: bool,
        idempotency_key: String,
        correlation_id: String,
    ) -> Result<Value, Problem> {
        let auth = self.auth(&correlation_id)?;
        let mut profile = self.client.profile(&profile_id, &correlation_id).await?;
        // Reconcile the process table before deciding whether this profile is
        // already open. A browser window can exit without going through the
        // Stop button; running_profiles intentionally retains its entry while
        // the final encrypted snapshot is pending, so it is not a live-process
        // check.
        let active_profiles = self
            .profile_runtime
            .active_profiles()
            .await?
            .iter()
            .any(|running| running == &profile_id);
        let pending_local_session = !active_profiles
            && self
                .profile_runtime
                .running_profiles()
                .await?
                .iter()
                .any(|running| running == &profile_id);
        if pending_local_session {
            // Finish the prior session's sync/lease release before opening a
            // new browser process for the same profile.
            self.stop_profile(
                profile_id.clone(),
                runtime_projection_idempotency_key(&idempotency_key, "exited-session-recovery"),
                correlation_id.clone(),
                None,
                None,
                None,
            )
            .await?;
            profile = self.client.profile(&profile_id, &correlation_id).await?;
        }
        if !launch_state_allows_start(
            &profile.value.state,
            profile.value.archived,
            active_profiles,
        ) {
            return Err(Problem::new(
                "profile_not_launchable",
                "Profile cannot be launched",
                409,
                &correlation_id,
            )
            .with_detail(format!(
                "Profile is not launchable: state={:?}, archived={}",
                profile.value.state, profile.value.archived
            ))
            .with_field("profileState", format!("{:?}", profile.value.state))
            .with_field("archived", profile.value.archived));
        }
        let mut fingerprint_target =
            target_from_runtime_config(profile.value.runtime_config.as_ref(), &correlation_id)?;
        if fingerprint_target.is_some()
            && profile.value.runtime != buglogin_domain::entities::Runtime::Wayfern
        {
            return Err(Problem::new(
                "fingerprint_runtime_unsupported",
                "Fingerprint targets are supported only by the Wayfern runtime",
                409,
                &correlation_id,
            ));
        }
        let runtime_supported = self
            .runtime_targets
            .read()
            .map_err(|_| runtime_state_unavailable(&correlation_id))?
            .supports(&profile.value.runtime);
        if !runtime_supported {
            return Err(Problem::new(
                "runtime_not_configured",
                "Profile runtime is not configured",
                503,
                correlation_id,
            ));
        }
        if profile.value.sync_mode != Some(SyncMode::Managed) {
            return Err(Problem::new(
                "production_managed_profile_required",
                "Production launch requires a managed profile",
                409,
                correlation_id,
            ));
        }
        let binding = self
            .client
            .profile_binding(&profile_id, &correlation_id)
            .await?;
        if binding.archive_state != ProfileArchiveState::Active || binding.profile_id != profile_id
        {
            return Err(Problem::new(
                "managed_profile_binding_required",
                "Managed profile requires an active binding",
                409,
                correlation_id,
            ));
        }
        let scope = CoordinationScope {
            workspace_id: &binding.workspace_id,
            auth: &auth,
        };
        let (mut manifest, manifest_proxy_stale) = match self
            .client
            .current_manifest(scope.clone(), &binding.resource_id)
            .await
        {
            Ok(Some(manifest)) => (manifest, false),
            Err(error) if error.code == "manifest_dependency_stale" => (
                self.client
                    .current_manifest_for_proxy_repair(
                        scope.clone(),
                        &binding.resource_id,
                        &correlation_id,
                    )
                    .await?
                    .ok_or_else(|| {
                        Problem::new(
                            "production_manifest_not_found",
                            "Production profile manifest not found for proxy repair",
                            409,
                            &correlation_id,
                        )
                    })?,
                true,
            ),
            Ok(None) => {
                return Err(Problem::new(
                    "production_manifest_not_found",
                    "Production profile manifest not found",
                    409,
                    &correlation_id,
                ));
            }
            Err(error) => return Err(error),
        };
        self.ensure_device_key_registered(&correlation_id).await?;
        match self
            .client
            .profile_key_rotation_recipients(&profile_id, &correlation_id)
            .await
        {
            Ok(_)
                if manifest.generation == 1
                    && manifest.parent_generation.is_none()
                    && manifest.payload_encryption.is_none()
                    && manifest.objects.is_empty()
                    && manifest.tombstones.is_empty() => {}
            Ok(_) => {
                return Err(Problem::new(
                    "profile_key_rotation_required",
                    "Complete profile key rotation before launching this shared profile",
                    409,
                    &correlation_id,
                ));
            }
            Err(error)
                if error.code == "sync_key_rotation_not_pending"
                    || error.code == "profile_forbidden"
                    || error.status == 404 => {}
            Err(error) => return Err(error),
        }
        let pending_proxy_assignment = self
            .client
            .profile_proxy_assignment(&binding.workspace_id, &profile_id, &correlation_id)
            .await?;
        let stale_proxy_dependency = manifest_proxy_stale || pending_proxy_assignment.is_some();
        let recovery_path = self
            .projection
            .active_session_recovery_path(&profile_id, &manifest)?
            .or(self
                .projection
                .dirty_local_generation(&profile_id, &manifest)?);
        if recovery_path.is_some() || stale_proxy_dependency {
            self.stop_profile(
                profile_id.clone(),
                runtime_projection_idempotency_key(&idempotency_key, "local-recovery-sync"),
                correlation_id.clone(),
                None,
                None,
                None,
            )
            .await?;
            manifest = self
                .client
                .current_manifest(scope.clone(), &binding.resource_id)
                .await?
                .ok_or_else(|| {
                    Problem::new(
                        "production_manifest_not_found",
                        "Production profile manifest not found after local recovery sync",
                        409,
                        &correlation_id,
                    )
                })?;
            profile = self.client.profile(&profile_id, &correlation_id).await?;
            fingerprint_target =
                target_from_runtime_config(profile.value.runtime_config.as_ref(), &correlation_id)?;
            if fingerprint_target.is_some()
                && profile.value.runtime != buglogin_domain::entities::Runtime::Wayfern
            {
                return Err(Problem::new(
                    "fingerprint_runtime_unsupported",
                    "Fingerprint targets are supported only by the Wayfern runtime",
                    409,
                    &correlation_id,
                ));
            }
        }
        let proxy_launch = match profile.value.proxy_id.as_ref() {
            Some(proxy_id) => {
                self.reconcile_pending_proxy_mutation(
                    &binding.workspace_id,
                    proxy_id,
                    &correlation_id,
                )
                .await?;
                Some(
                    self.client
                        .proxy_for_launch(&binding.workspace_id, proxy_id, &correlation_id)
                        .await?,
                )
            }
            None => None,
        };
        let workspace_keys = if manifest.payload_encryption.is_some() {
            let key_version = manifest
                .payload_encryption
                .as_ref()
                .and_then(|encryption| encryption.key_version.as_deref())
                .ok_or_else(|| {
                    Problem::new(
                        "profile_key_version_missing",
                        "Profile sync manifest has no active resource key version",
                        409,
                        &correlation_id,
                    )
                })?;
            match self.sync_key_vault.load_for_manifest(
                &binding.workspace_id,
                &binding.resource_id,
                key_version,
                &correlation_id,
            ) {
                Ok(Some(keys)) => Some(keys),
                Ok(None) => Some(
                    self.recover_profile_key_envelope(
                        &binding,
                        &profile_id,
                        key_version,
                        &correlation_id,
                    )
                    .await?,
                ),
                Err(error) if error.code == "profile_key_version_stale" => Some(
                    self.recover_profile_key_envelope(
                        &binding,
                        &profile_id,
                        key_version,
                        &correlation_id,
                    )
                    .await?,
                ),
                Err(error) => return Err(error),
            }
        } else {
            None
        };
        for attempt in 0..2 {
            let (projection, mut proxy_credentials) = self
                .projection
                .hydrate_encrypted_with_proxy_credentials(
                    &self.client,
                    &auth,
                    &profile_id,
                    &manifest,
                    workspace_keys.as_ref().map(|keys| &keys.client),
                    workspace_keys.as_ref().map(|keys| &keys.recovery),
                    &correlation_id,
                )
                .await?;
            let lease_key = lease_idempotency_key(&idempotency_key, manifest.generation);
            let lease = self
                .client
                .acquire_lease_idempotent(
                    scope.clone(),
                    &binding.resource_id,
                    Duration::from_secs(60),
                    &lease_key,
                )
                .await?;
            let latest = match self
                .client
                .current_manifest(scope.clone(), &binding.resource_id)
                .await
            {
                Ok(Some(manifest)) => manifest,
                Ok(None) => {
                    let _ = self
                        .client
                        .release_lease(scope.clone(), &binding.resource_id, &lease.owner_token)
                        .await;
                    return Err(Problem::new(
                        "production_manifest_not_found",
                        "Production profile manifest not found",
                        409,
                        &correlation_id,
                    ));
                }
                Err(error) => {
                    let _ = self
                        .client
                        .release_lease(scope.clone(), &binding.resource_id, &lease.owner_token)
                        .await;
                    return Err(error);
                }
            };
            if latest != manifest {
                let _ = self
                    .client
                    .release_lease(scope.clone(), &binding.resource_id, &lease.owner_token)
                    .await;
                if attempt == 1 {
                    return Err(Problem::new(
                        "production_manifest_changed",
                        "Production profile changed during hydration",
                        409,
                        correlation_id,
                    )
                    .retryable(true));
                }
                manifest = latest;
                continue;
            }
            let launch_specification = self
                .runtime_targets
                .read()
                .map_err(|_| runtime_state_unavailable(&correlation_id))?
                .launch_specification(&profile.value, &projection.path, headless);
            let mut specification = match launch_specification {
                Ok(specification) => specification,
                Err(mut error) => {
                    let _ = self
                        .client
                        .release_lease(scope.clone(), &binding.resource_id, &lease.owner_token)
                        .await;
                    error.correlation_id = correlation_id.clone();
                    return Err(error);
                }
            };
            let mut proxy_bridge = None;
            if let Some(proxy) = proxy_launch.as_ref() {
                let proxy_server = if proxy.has_credential {
                    let credential_result = (|| {
                        if let Some(credential) = self.proxy_credential_vault.load(
                            &binding.workspace_id,
                            &proxy.proxy_id,
                            proxy.revision,
                            &correlation_id,
                        )? {
                            return Ok(credential);
                        }
                        let Some(index) = proxy_credentials.iter().position(|credential| {
                            credential.proxy_id == proxy.proxy_id
                                && credential.revision == proxy.revision
                        }) else {
                            return Err(Problem::new(
                                "proxy_credential_envelope_missing",
                                "This device has no credential matching the current proxy revision",
                                409,
                                &correlation_id,
                            ));
                        };
                        let hydrated: HydratedProxyCredential = proxy_credentials.remove(index);
                        self.proxy_credential_vault.store(
                            &binding.workspace_id,
                            &proxy.proxy_id,
                            proxy.revision,
                            hydrated.secret.clone(),
                            &correlation_id,
                        )?;
                        Ok(hydrated.secret)
                    })();
                    let credential = match credential_result {
                        Ok(credential) => credential,
                        Err(error) => {
                            let _ = self
                                .client
                                .release_lease(
                                    scope.clone(),
                                    &binding.resource_id,
                                    &lease.owner_token,
                                )
                                .await;
                            return Err(error);
                        }
                    };
                    let bridge = ProxyBridgeHandle::start(ProxyBridgeSpec {
                        protocol: proxy.protocol.clone(),
                        host: proxy.host.clone(),
                        port: proxy.port,
                        credential: Some(credential),
                    })
                    .await
                    .map_err(|mut error| {
                        error.correlation_id = correlation_id.clone();
                        error
                    });
                    match bridge {
                        Ok(bridge) => {
                            let server = bridge.proxy_url().to_owned();
                            proxy_bridge = Some(bridge);
                            server
                        }
                        Err(error) => {
                            let _ = self
                                .client
                                .release_lease(
                                    scope.clone(),
                                    &binding.resource_id,
                                    &lease.owner_token,
                                )
                                .await;
                            return Err(error);
                        }
                    }
                } else {
                    proxy.server.clone()
                };
                push_or_replace_arg(
                    &mut specification.args,
                    format!("--proxy-server={proxy_server}"),
                );
                if !proxy.bypass_rules.is_empty() {
                    push_or_replace_arg(
                        &mut specification.args,
                        format!("--proxy-bypass-list={}", proxy.bypass_rules.join(";")),
                    );
                }
            }
            let generation = projection.generation;
            let root_checksum = projection.root_checksum.clone();
            let profile_path = projection.path.clone();
            let fingerprint_cdp_port = if fingerprint_target.is_some() {
                let port = match configure_local_cdp_args(&mut specification.args, &correlation_id)
                {
                    Ok(port) => port,
                    Err(mut error) => {
                        if let Some(bridge) = proxy_bridge.take() {
                            let _ = bridge.stop().await;
                        }
                        let _ = self
                            .client
                            .release_lease(scope.clone(), &binding.resource_id, &lease.owner_token)
                            .await;
                        error.correlation_id = correlation_id.clone();
                        return Err(error);
                    }
                };
                if let Err(mut error) = clear_stale_cdp_marker(&profile_path, &correlation_id) {
                    if let Some(bridge) = proxy_bridge.take() {
                        let _ = bridge.stop().await;
                    }
                    let _ = self
                        .client
                        .release_lease(scope.clone(), &binding.resource_id, &lease.owner_token)
                        .await;
                    error.correlation_id = correlation_id.clone();
                    return Err(error);
                }
                Some(port)
            } else {
                None
            };
            let runtime_key = runtime_projection_idempotency_key(&idempotency_key, "started");
            let lease_for_projection = lease.clone();
            if let Err(error) =
                self.projection
                    .begin_active_session(&profile_id, generation, &root_checksum)
            {
                if let Some(bridge) = proxy_bridge.take() {
                    let _ = bridge.stop().await;
                }
                let _ = self
                    .client
                    .release_lease(scope.clone(), &binding.resource_id, &lease.owner_token)
                    .await;
                return Err(error);
            }
            let receipt = match self
                .profile_runtime
                .launch(LaunchRequest {
                    profile_id: profile_id.clone(),
                    idempotency_key: idempotency_key.clone(),
                    profile_path: profile_path.clone(),
                    specification,
                    lease_scope: Some(LeaseScope {
                        workspace_id: binding.workspace_id.clone(),
                        resource_id: binding.resource_id.clone(),
                    }),
                    preacquired_lease: Some(lease),
                    proxy_bridge,
                })
                .await
            {
                Ok(receipt) => receipt,
                Err(mut error) => {
                    let _ = self.projection.clear_active_session(&profile_id);
                    error.correlation_id = correlation_id.clone();
                    return Err(error);
                }
            };
            if let Some(target) = fingerprint_target.as_ref() {
                let cdp_port = match fingerprint_cdp_port {
                    Some(port) => port,
                    None => {
                        if let Err(mut cleanup_error) =
                            self.profile_runtime.stop(profile_id.clone()).await
                        {
                            cleanup_error.correlation_id = correlation_id.clone();
                            return Err(cleanup_error);
                        }
                        let _ = self.projection.clear_active_session(&profile_id);
                        return Err(Problem::new(
                            "wayfern_cdp_unavailable",
                            "Wayfern local control port was not configured",
                            503,
                            &correlation_id,
                        ));
                    }
                };
                if let Err(error) = apply_wayfern_fingerprint(
                    &profile_path,
                    target,
                    profile.value.runtime_version.as_deref().unwrap_or_default(),
                    cdp_port,
                    &correlation_id,
                )
                .await
                {
                    if let Err(mut cleanup_error) =
                        self.profile_runtime.stop(profile_id.clone()).await
                    {
                        cleanup_error.correlation_id = correlation_id.clone();
                        return Err(cleanup_error);
                    }
                    let _ = self.projection.clear_active_session(&profile_id);
                    return Err(error);
                }
            }
            if let Err(error) = self
                .client
                .runtime_started(
                    &profile_id,
                    auth.actor_id(),
                    generation,
                    &root_checksum,
                    &lease_for_projection,
                    &runtime_key,
                    &correlation_id,
                )
                .await
            {
                if let Err(mut cleanup_error) = self.profile_runtime.stop(profile_id.clone()).await
                {
                    cleanup_error.correlation_id = correlation_id.clone();
                    return Err(cleanup_error);
                }
                return Err(error);
            }
            return Ok(json!({
                "profileId": profile_id,
                "processId": receipt.process_id,
                "generation": generation,
                "rootChecksum": root_checksum
            }));
        }
        unreachable!()
    }

    async fn proxy_snapshot_for_sync(
        &self,
        workspace_id: &WorkspaceId,
        proxy_id: Option<&ProxyId>,
        previous: &buglogin_sync::manifest::SyncManifest,
        correlation_id: &str,
    ) -> Result<Option<crate::remote_api::RemoteProxySyncSnapshot>, Problem> {
        let Some(proxy_id) = proxy_id else {
            return Ok(None);
        };
        self.reconcile_pending_proxy_mutation(workspace_id, proxy_id, correlation_id)
            .await?;
        let proxy = self
            .client
            .proxy_for_launch(workspace_id, proxy_id, correlation_id)
            .await?;
        if proxy.has_credential != proxy.credential_version.is_some() {
            return Err(Problem::new(
                "proxy_credential_version_missing",
                "BugLogin API returned inconsistent proxy credential version metadata",
                502,
                correlation_id,
            ));
        }
        let mut credential = self.proxy_credential_vault.load(
            workspace_id,
            proxy_id,
            proxy.revision,
            correlation_id,
        )?;
        if credential.is_none() && proxy.has_credential {
            let previous_revision =
                previous
                    .dependencies
                    .iter()
                    .find_map(|dependency| match dependency {
                        buglogin_sync::manifest::ManifestDependency::Proxy {
                            proxy_id: previous_proxy_id,
                            revision,
                            credential_version: Some(previous_version),
                            ..
                        } if previous_proxy_id == proxy_id
                            && Some(previous_version) == proxy.credential_version.as_ref() =>
                        {
                            Some(*revision)
                        }
                        _ => None,
                    });
            if let Some(previous_revision) = previous_revision {
                if let Some(previous_credential) = self.proxy_credential_vault.load(
                    workspace_id,
                    proxy_id,
                    previous_revision,
                    correlation_id,
                )? {
                    self.proxy_credential_vault.store(
                        workspace_id,
                        proxy_id,
                        proxy.revision,
                        previous_credential.clone(),
                        correlation_id,
                    )?;
                    credential = Some(previous_credential);
                }
            }
        }
        self.client
            .proxy_for_sync(
                workspace_id,
                proxy_id,
                proxy.revision,
                credential,
                correlation_id,
            )
            .await
            .map(Some)
    }

    fn profile_key_rotation_envelopes(
        &self,
        binding: &buglogin_domain::profile::ProfileResource,
        recipients: Vec<buglogin_api::services::profile_hosted::ProfileKeyRotationRecipientDevices>,
        keys: &crate::sync_key_vault::ResourceKeys,
        key_version: &str,
        correlation_id: &str,
    ) -> Result<Vec<ProfileKeyRotationEnvelopeUpdate>, Problem> {
        let mut updates = Vec::with_capacity(recipients.len());
        for recipient in recipients {
            if recipient.device_keys.is_empty() || recipient.device_keys.len() > 64 {
                return Err(Problem::new(
                    "profile_key_distribution_not_ready",
                    "A shared member has an unsupported number of active devices",
                    409,
                    correlation_id,
                ));
            }
            let mut recipient_key_envelopes = Vec::with_capacity(recipient.device_keys.len());
            for device in recipient.device_keys {
                let public_key = parse_device_public_key(&device.public_key, correlation_id)?;
                let fingerprint = format!("{:x}", Sha256::digest(public_key.as_bytes()));
                if fingerprint != device.fingerprint {
                    return Err(Problem::new(
                        "recipient_device_key_invalid",
                        "Recipient device key does not match its registered fingerprint",
                        409,
                        correlation_id,
                    ));
                }
                let envelope = buglogin_sync::crypto::seal_profile_key_share(
                    binding.workspace_id.clone(),
                    binding.resource_id.clone(),
                    device.device_id,
                    key_version.to_owned(),
                    &public_key,
                    &keys.client,
                    &keys.recovery,
                )?;
                recipient_key_envelopes.push(ConfirmedProfileKeyShareEnvelope {
                    envelope,
                    confirmed_fingerprint: device.fingerprint,
                });
            }
            updates.push(ProfileKeyRotationEnvelopeUpdate {
                member_id: recipient.member_id,
                recipient_key_envelopes,
            });
        }
        Ok(updates)
    }

    async fn stop_profile(
        &self,
        profile_id: ProfileId,
        idempotency_key: String,
        correlation_id: String,
        confirmed_recipients: Option<Vec<ConfirmedKeyRotationRecipient>>,
        preacquired_lease: Option<LeaseGrant>,
        preheld_stop: Option<StopReceipt>,
    ) -> Result<Value, Problem> {
        let cleanup_profile_id = profile_id.clone();
        let result = self
            .stop_profile_inner(
                profile_id,
                idempotency_key,
                correlation_id,
                confirmed_recipients,
                preacquired_lease,
                preheld_stop,
            )
            .await;
        if result.is_err()
            && self
                .profile_runtime
                .active_profiles()
                .await
                .is_ok_and(|active| !active.contains(&cleanup_profile_id))
        {
            // Cover failures before the sync transaction has a StopReceipt
            // (for example key/manifest preflight or proxy shutdown). Never
            // clear state while the browser is still active.
            let _ = self.profile_runtime.complete_stop(cleanup_profile_id).await;
        }
        result
    }

    async fn stop_profile_inner(
        &self,
        profile_id: ProfileId,
        idempotency_key: String,
        correlation_id: String,
        confirmed_recipients: Option<Vec<ConfirmedKeyRotationRecipient>>,
        preacquired_lease: Option<LeaseGrant>,
        mut preheld_stop: Option<StopReceipt>,
    ) -> Result<Value, Problem> {
        let binding = self
            .client
            .profile_binding(&profile_id, &correlation_id)
            .await?;
        if binding.archive_state != ProfileArchiveState::Active || binding.profile_id != profile_id
        {
            return Err(Problem::new(
                "managed_profile_binding_required",
                "Managed profile requires an active binding",
                409,
                correlation_id,
            ));
        }
        // Authenticate and validate the managed binding first, then stop the local
        // browser before doing manifest, key-rotation, or object-storage work.
        // A cloud sync failure must not leave the user's browser process running.
        if preheld_stop.is_none()
            && self
                .profile_runtime
                .running_profiles()
                .await?
                .iter()
                .any(|running| running == &profile_id)
        {
            preheld_stop = Some(
                self.profile_runtime
                    .stop_for_sync(profile_id.clone())
                    .await
                    .map_err(|mut error| {
                        error.correlation_id = correlation_id.clone();
                        error
                    })?,
            );
        }
        let auth = self.auth(&correlation_id)?;
        let scope = CoordinationScope {
            workspace_id: &binding.workspace_id,
            auth: &auth,
        };
        let (previous, stale_proxy_dependency) = match self
            .client
            .current_manifest(scope.clone(), &binding.resource_id)
            .await
        {
            Ok(Some(manifest)) => (manifest, false),
            Err(error) if error.code == "manifest_dependency_stale" => (
                self.client
                    .current_manifest_for_proxy_repair(
                        scope.clone(),
                        &binding.resource_id,
                        &correlation_id,
                    )
                    .await?
                    .ok_or_else(|| {
                        Problem::new(
                            "production_manifest_not_found",
                            "Production profile manifest not found for proxy repair",
                            409,
                            &correlation_id,
                        )
                    })?,
                true,
            ),
            Ok(None) => {
                return Err(Problem::new(
                    "production_manifest_not_found",
                    "Production profile manifest not found",
                    409,
                    &correlation_id,
                ));
            }
            Err(error) => return Err(error),
        };
        if let Some(stopped) = preheld_stop.as_ref().filter(|stopped| stopped.was_launched) {
            let lease = stopped.lease.as_ref().ok_or_else(|| {
                Problem::new(
                    "production_write_authority_required",
                    "Production write authority is required",
                    409,
                    &correlation_id,
                )
            })?;
            self.client
                .runtime_stopped(
                    &profile_id,
                    auth.actor_id(),
                    previous.generation,
                    &previous.root_checksum,
                    lease,
                    &runtime_projection_idempotency_key(
                        &idempotency_key,
                        "process-stopped-preflight",
                    ),
                    &correlation_id,
                )
                .await?;
        }
        // Publish the stopped state before device-key enrollment and key-rotation
        // checks. Those can reject the sync after the browser is already closed;
        // the remote profile must not remain marked Running in that case.
        self.ensure_device_key_registered(&correlation_id).await?;
        let rotation_recipients = match self
            .client
            .profile_key_rotation_recipients(&profile_id, &correlation_id)
            .await
        {
            Ok(recipients) => Some(recipients),
            Err(error) if error.code == "sync_key_rotation_not_pending" || error.status == 404 => {
                if confirmed_recipients.is_some() {
                    return Err(Problem::new(
                        "profile_key_rotation_snapshot_stale",
                        "The profile no longer has the reviewed pending key rotation",
                        409,
                        &correlation_id,
                    ));
                }
                None
            }
            Err(error) if error.code == "profile_forbidden" => {
                if confirmed_recipients.is_some() {
                    return Err(error);
                }
                None
            }
            Err(error) if error.code == "profile_key_enrollment_required" => return Err(error),
            Err(error) => return Err(error),
        };
        let profile = self.client.profile(&profile_id, &correlation_id).await?;
        let initial_owner_key_enrollment = rotation_recipients.as_ref().is_some_and(|recipients| {
            previous.generation == 1
                && previous.objects.is_empty()
                && previous.payload_encryption.is_none()
                && recipients.len() == 1
                && recipients[0].device_keys.len() == 1
                && recipients[0].device_keys[0].device_id == self.device_id
        });
        if let Some(recipients) = rotation_recipients.as_ref() {
            if initial_owner_key_enrollment {
                let recipient = &recipients[0].device_keys[0];
                let private_key = self
                    .device_key_vault
                    .load_or_create(&self.device_id, &correlation_id)?;
                let public_key = private_key.public_key()?;
                let public_key_hex = encode_hex(public_key.as_bytes());
                let fingerprint = format!("{:x}", Sha256::digest(public_key.as_bytes()));
                if recipient.public_key != public_key_hex || recipient.fingerprint != fingerprint {
                    return Err(Problem::new(
                        "recipient_device_key_invalid",
                        "The registered owner device key does not match this device",
                        409,
                        &correlation_id,
                    ));
                }
            } else {
                let supplied = confirmed_recipients.as_ref().ok_or_else(|| Problem::new(
                    "profile_key_rotation_confirmation_required",
                    "Review and confirm every active recipient device fingerprint before syncing this profile",
                    409,
                    &correlation_id,
                ))?;
                validate_key_rotation_confirmation(supplied, recipients, &correlation_id)?;
            }
        }
        let pending_proxy_assignment = self
            .client
            .profile_proxy_assignment(&binding.workspace_id, &profile_id, &correlation_id)
            .await?;
        let running_profiles = self.profile_runtime.running_profiles().await?;
        let browser_was_running = running_profiles
            .iter()
            .any(|running| running == &profile_id);
        let dirty_local_path = if browser_was_running {
            None
        } else {
            self.projection
                .dirty_local_generation(&profile_id, &previous)?
        };
        let proxy_refresh_required = stale_proxy_dependency || pending_proxy_assignment.is_some();
        if preheld_stop.is_some()
            && !browser_was_running
            && dirty_local_path.is_none()
            && !proxy_refresh_required
            && rotation_recipients.is_none()
        {
            self.profile_runtime
                .complete_stop(profile_id.clone())
                .await
                .map_err(|mut error| {
                    error.correlation_id = correlation_id.clone();
                    error
                })?;
            return Ok(json!({
                "profileId": profile_id,
                "generation": previous.generation,
                "rootChecksum": previous.root_checksum,
                "keyRotated": false,
                "alreadySynced": true
            }));
        }
        let local_sync_path = if browser_was_running {
            None
        } else if proxy_refresh_required || preacquired_lease.is_some() {
            self.projection
                .existing_local_generation(&profile_id, &previous)?
                .or(dirty_local_path)
        } else {
            dirty_local_path
        };
        let mut stopped = if let Some(stopped) = preheld_stop {
            stopped
        } else if browser_was_running && preacquired_lease.is_some() {
            if let Some(lease) = preacquired_lease.as_ref() {
                let _ = self
                    .client
                    .release_lease(scope.clone(), &binding.resource_id, &lease.owner_token)
                    .await;
            }
            return Err(Problem::new(
                "cookie_profile_running",
                "Stop the profile before applying cookie changes",
                409,
                &correlation_id,
            ));
        } else if browser_was_running {
            self.profile_runtime
                .stop_for_sync(profile_id.clone())
                .await
                .map_err(|mut error| {
                    error.correlation_id = correlation_id.clone();
                    error
                })?
        } else if let Some(profile_path) = local_sync_path {
            let lease = match preacquired_lease.clone() {
                Some(lease) => lease,
                None => {
                    self.client
                        .acquire_lease_idempotent(
                            scope.clone(),
                            &binding.resource_id,
                            Duration::from_secs(120),
                            &lease_idempotency_key(&idempotency_key, previous.generation),
                        )
                        .await?
                }
            };
            match self
                .profile_runtime
                .hold_idle_profile_for_sync(
                    profile_id.clone(),
                    profile_path.clone(),
                    LeaseScope {
                        workspace_id: binding.workspace_id.clone(),
                        resource_id: binding.resource_id.clone(),
                    },
                    lease.clone(),
                )
                .await
            {
                Ok(receipt) => receipt,
                Err(error) => {
                    let _ = self
                        .client
                        .release_lease(scope.clone(), &binding.resource_id, &lease.owner_token)
                        .await;
                    return Err(error);
                }
            }
        } else if let Some(lease) = preacquired_lease.as_ref() {
            let _ = self
                .client
                .release_lease(scope.clone(), &binding.resource_id, &lease.owner_token)
                .await;
            return Err(Problem::new(
                "cookie_profile_projection_unavailable",
                "A local profile snapshot is required before cookie sync",
                409,
                &correlation_id,
            ));
        } else if rotation_recipients.is_some() || proxy_refresh_required {
            self.prepare_idle_profile_for_key_rotation(
                &binding,
                &profile_id,
                &scope,
                &idempotency_key,
                &correlation_id,
            )
            .await?
        } else {
            return Err(Problem::new(
                "profile_not_running",
                "Profile is not running",
                409,
                &correlation_id,
            ));
        };
        let sync_result = async {
            if stopped.was_launched
                && stopped
                    .lease
                    .as_ref()
                    .is_some_and(|lease| lease.fencing_token <= previous.fencing_token)
            {
                let was_launched = stopped.was_launched;
                let profile_path = stopped.profile_path.clone();
                // A browser can outlive its lease after sleep, process crash, or a
                // second device taking over. Never snapshot against the manifest
                // with that stale fence: release local runtime bookkeeping and
                // acquire a fresh lease before reconciling the stopped tree.
                self.profile_runtime
                    .complete_stop(profile_id.clone())
                    .await?;
                let refreshed_lease_key = lease_idempotency_key(
                    &format!("{idempotency_key}:fence-refresh:{}", previous.fencing_token),
                    previous.generation,
                );
                let refreshed_lease = self
                    .client
                    .acquire_lease_idempotent(
                        scope.clone(),
                        &binding.resource_id,
                        Duration::from_secs(120),
                        &refreshed_lease_key,
                    )
                    .await?;
                stopped = self
                    .profile_runtime
                    .hold_idle_profile_for_sync(
                        profile_id.clone(),
                        profile_path,
                        LeaseScope {
                            workspace_id: binding.workspace_id.clone(),
                            resource_id: binding.resource_id.clone(),
                        },
                        refreshed_lease,
                    )
                    .await?;
                stopped.was_launched = was_launched;
                if was_launched {
                    let refreshed_lease = stopped.lease.as_ref().ok_or_else(|| {
                        Problem::new(
                            "production_write_authority_required",
                            "Production write authority is required",
                            409,
                            &correlation_id,
                        )
                    })?;
                    self.client
                        .runtime_stopped(
                            &profile_id,
                            auth.actor_id(),
                            previous.generation,
                            &previous.root_checksum,
                            refreshed_lease,
                            &runtime_projection_idempotency_key(
                                &idempotency_key,
                                "process-stopped-fence-refresh",
                            ),
                            &correlation_id,
                        )
                        .await?;
                }
            }
            let lease = stopped.lease.as_ref().ok_or_else(|| {
                Problem::new(
                    "production_write_authority_required",
                    "Production write authority is required",
                    409,
                    &correlation_id,
                )
            })?;
            if lease.workspace_id != binding.workspace_id
                || lease.resource_id != binding.resource_id
                || lease.owner_id != auth.actor_id()
            {
                return Err(Problem::new(
                    "production_write_authority_mismatch",
                    "Production write authority does not match binding",
                    409,
                    correlation_id,
                ));
            }
            if pending_proxy_assignment
                .as_ref()
                .is_some_and(|assignment| assignment.expected_profile_revision != profile.revision)
            {
                return Err(Problem::new(
                    "profile_proxy_assignment_stale",
                    "Profile proxy assignment is stale; refresh the profile before syncing",
                    409,
                    &correlation_id,
                ));
            }
            let selected_proxy_id = pending_proxy_assignment
                .as_ref()
                .map(|assignment| assignment.proxy_id.clone())
                .unwrap_or_else(|| profile.value.proxy_id.clone());
            let workspace_keys = if let Some(encryption) = previous.payload_encryption.as_ref() {
                let key_version = encryption.key_version.as_deref().ok_or_else(|| {
                    Problem::new(
                        "profile_key_version_missing",
                        "Profile sync manifest has no active resource key version",
                        409,
                        &correlation_id,
                    )
                })?;
                match self.sync_key_vault.load_for_manifest(
                    &binding.workspace_id,
                    &binding.resource_id,
                    key_version,
                    &correlation_id,
                ) {
                    Ok(Some(keys)) => keys,
                    Ok(None) => {
                        self.recover_profile_key_envelope(
                            &binding,
                            &profile_id,
                            key_version,
                            &correlation_id,
                        )
                        .await?
                    }
                    Err(error) if error.code == "profile_key_version_stale" => {
                        self.recover_profile_key_envelope(
                            &binding,
                            &profile_id,
                            key_version,
                            &correlation_id,
                        )
                        .await?
                    }
                    Err(error) => return Err(error),
                }
            } else {
                self.sync_key_vault.load_or_create(
                    &binding.workspace_id,
                    &binding.resource_id,
                    &correlation_id,
                )?
            };
            let rotation_keys = if rotation_recipients.is_some() && !initial_owner_key_enrollment {
                Some(self.sync_key_vault.stage_rotation(
                    &binding.workspace_id,
                    &binding.resource_id,
                    &correlation_id,
                )?)
            } else {
                None
            };
            let keys_for_snapshot = rotation_keys.as_ref().unwrap_or(&workspace_keys);
            let rotation_envelopes = rotation_recipients
                .map(|recipients| {
                    let key_version =
                        format!("{:x}", Sha256::digest(keys_for_snapshot.client.as_bytes()));
                    self.profile_key_rotation_envelopes(
                        &binding,
                        recipients,
                        keys_for_snapshot,
                        &key_version,
                        &correlation_id,
                    )
                })
                .transpose()?;
            let proxy_snapshot = self
                .proxy_snapshot_for_sync(
                    &binding.workspace_id,
                    selected_proxy_id.as_ref(),
                    &previous,
                    &correlation_id,
                )
                .await?;
            let (descriptor, files, manifest) = self.projection.snapshot_with_proxy_encryption(
                &stopped.profile_path,
                &previous,
                lease.fencing_token,
                proxy_snapshot,
                &keys_for_snapshot.client,
                &keys_for_snapshot.recovery,
            )?;
            self.upload_profile_snapshot(
                scope.clone(),
                &binding.resource_id,
                &lease.owner_token,
                lease.fencing_token,
                &idempotency_key,
                &descriptor,
                files,
                &manifest,
                &correlation_id,
            )
            .await?;
            self.projection
                .prepare_sync_commit(&profile_id, &previous, &manifest)?;
            if let Some(envelopes) = rotation_envelopes.as_ref() {
                self.client
                    .commit_manifest_with_key_rotation_reconciliation(
                        scope,
                        &binding.resource_id,
                        &lease.owner_token,
                        pending_proxy_assignment.as_ref().map(|_| profile.revision),
                        manifest.clone(),
                        envelopes,
                        &correlation_id,
                    )
                    .await?;
            } else {
                self.client
                    .commit_manifest_with_reconciliation(
                        scope,
                        &binding.resource_id,
                        &lease.owner_token,
                        pending_proxy_assignment.as_ref().map(|_| profile.revision),
                        manifest.clone(),
                        &correlation_id,
                    )
                    .await?;
            }
            self.projection.clear_active_session(&profile_id)?;
            let key_rotation_error = if rotation_envelopes.is_some() {
                if rotation_keys.is_some() {
                    self.sync_key_vault
                        .activate_staged_rotation(
                            &binding.workspace_id,
                            &binding.resource_id,
                            &correlation_id,
                        )
                        .err()
                } else {
                    None
                }
            } else {
                None
            };
            if stopped.was_launched {
                self.client
                    .runtime_stopped(
                        &profile_id,
                        auth.actor_id(),
                        manifest.generation,
                        &manifest.root_checksum,
                        lease,
                        &runtime_projection_idempotency_key(&idempotency_key, "stopped"),
                        &correlation_id,
                    )
                    .await?;
            }
            self.profile_runtime
                .complete_stop(profile_id.clone())
                .await
                .map_err(|mut error| {
                    error.correlation_id = correlation_id;
                    error
                })?;
            if let Some(error) = key_rotation_error {
                return Err(error);
            }
            Ok(json!({
                "profileId": profile_id,
                "generation": manifest.generation,
                "rootChecksum": manifest.root_checksum,
                "keyRotated": rotation_keys.is_some()
            }))
        }
        .await;
        self.profile_runtime
            .finish_sync_result(profile_id, sync_result)
            .await
    }

    async fn import_legacy_profile_data_tree(
        &self,
        source_root: PathBuf,
        idempotency_key: String,
        correlation_id: String,
    ) -> Result<Value, Problem> {
        let source_metadata = fs::symlink_metadata(&source_root).map_err(|_| {
            Problem::new(
                "migration_source_invalid",
                "Prepared legacy source directory is unavailable",
                400,
                &correlation_id,
            )
        })?;
        if !source_metadata.is_dir() || source_metadata.file_type().is_symlink() {
            return Err(Problem::new(
                "migration_source_invalid",
                "Prepared legacy source directory is invalid",
                400,
                &correlation_id,
            ));
        }
        let source_root = source_root.canonicalize().map_err(|_| {
            Problem::new(
                "migration_source_invalid",
                "Prepared legacy source directory is unavailable",
                400,
                &correlation_id,
            )
        })?;
        let version: LegacyPreparedSourceVersion = serde_json::from_slice(
            &read_migration_source_file(&source_root, "version.json", 64 * 1024, &correlation_id)?,
        )
        .map_err(|_| {
            Problem::new(
                "migration_source_identity_invalid",
                "Prepared legacy source identity is invalid",
                422,
                &correlation_id,
            )
        })?;
        if version.instance_id.trim().is_empty()
            || version.instance_id.len() > 128
            || version.instance_id.chars().any(char::is_control)
        {
            return Err(Problem::new(
                "migration_source_identity_invalid",
                "Prepared legacy source identity is invalid",
                422,
                &correlation_id,
            ));
        }
        let inventory: LegacyProfileDataInventory =
            serde_json::from_slice(&read_migration_source_file(
                &source_root,
                "control-plane-export.json",
                64 * 1024 * 1024,
                &correlation_id,
            )?)
            .map_err(|_| {
                Problem::new(
                    "migration_control_export_invalid",
                    "Prepared legacy control export is invalid",
                    422,
                    &correlation_id,
                )
            })?;
        let profiles_root = source_root.join("profiles");
        let profiles_metadata = fs::symlink_metadata(&profiles_root).map_err(|_| {
            Problem::new(
                "migration_profile_source_missing",
                "Prepared legacy profile data is missing",
                422,
                &correlation_id,
            )
        })?;
        if !profiles_metadata.is_dir() || profiles_metadata.file_type().is_symlink() {
            return Err(Problem::new(
                "migration_profile_source_invalid",
                "Prepared legacy profile directory is invalid",
                422,
                &correlation_id,
            ));
        }

        let mut profile_sources = BTreeMap::new();
        let mut source_proxy_ids = BTreeSet::new();
        for profile in inventory.profiles {
            let local_id = buglogin_migration::legacy::legacy_local_profile_id(&profile.id)
                .map_err(|_| {
                    Problem::new(
                        "migration_control_export_invalid",
                        "Prepared legacy profile identity is invalid",
                        422,
                        &correlation_id,
                    )
                })?;
            let profile_id = ProfileId::new(
                buglogin_domain::ids::migration_uuid(
                    "buglogin-legacy",
                    &version.instance_id,
                    "profile",
                    &local_id,
                )
                .to_string(),
            )
            .map_err(|_| {
                Problem::new(
                    "migration_profile_id_invalid",
                    "Mapped legacy profile ID is invalid",
                    422,
                    &correlation_id,
                )
            })?;
            let proxy_id = profile.proxy_id;
            if let Some(proxy_id) = proxy_id.as_deref() {
                if proxy_id.trim().is_empty()
                    || proxy_id.len() > 128
                    || !proxy_id.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-')
                    })
                {
                    return Err(Problem::new(
                        "migration_control_export_invalid",
                        "Prepared legacy profile proxy identity is invalid",
                        422,
                        &correlation_id,
                    ));
                }
                source_proxy_ids.insert(proxy_id.to_owned());
            }
            if profile_sources
                .insert(local_id, (profile.id, profile_id, proxy_id))
                .is_some()
            {
                return Err(Problem::new(
                    "migration_control_export_invalid",
                    "Prepared legacy control export contains duplicate profiles",
                    422,
                    &correlation_id,
                ));
            }
        }

        let mut imported = Vec::new();
        let mut already_imported = Vec::new();
        let mut preserve_only = Vec::new();
        let mut failed = Vec::new();
        let proxy_export = if source_proxy_ids.is_empty() {
            None
        } else {
            Some(
                buglogin_migration::legacy::map_legacy_proxies_for_ids(
                    &source_root,
                    &version.instance_id,
                    &source_proxy_ids,
                )
                .map_err(|_| {
                    Problem::new(
                        "migration_proxy_source_invalid",
                        "A proxy referenced by the legacy profiles is missing or invalid",
                        422,
                        &correlation_id,
                    )
                })?,
            )
        };
        let mut processed = 0usize;
        let total_profiles = profile_sources.len();
        let mut imported_proxy_scopes = BTreeSet::new();
        let mut imported_proxy_count = 0usize;
        for (local_id, (source_profile_id, profile_id, source_proxy_id)) in profile_sources {
            processed += 1;
            let progress_operation_id =
                operation_id("migration.profile.data.import", &idempotency_key);
            let progress = 0.1 + 0.8 * processed as f64 / total_profiles as f64;
            if let Ok(mut operations) = self.operations.write() {
                let _ = operations.transition(
                    &progress_operation_id,
                    OperationStatus::Running,
                    Some(progress.min(0.9)),
                    None,
                    None,
                );
            }
            let expected_proxy_id = source_proxy_id
                .as_deref()
                .map(|source_proxy_id| {
                    ProxyId::new(
                        buglogin_domain::ids::migration_uuid(
                            "buglogin-legacy",
                            &version.instance_id,
                            "proxy",
                            source_proxy_id,
                        )
                        .to_string(),
                    )
                    .map_err(|_| invalid_input("proxyId", &correlation_id))
                })
                .transpose()?;
            let target_profile = match self.client.profile(&profile_id, &correlation_id).await {
                Ok(profile) => profile,
                Err(error) if error.status >= 500 || error.status == 408 || error.status == 429 => {
                    return Err(error);
                }
                Err(error) => {
                    failed.push(json!({
                        "sourceProfileId": source_profile_id,
                        "profileId": profile_id,
                        "code": error.code
                    }));
                    continue;
                }
            };
            if target_profile.value.id != profile_id
                || target_profile.value.proxy_id != expected_proxy_id
            {
                failed.push(json!({
                    "sourceProfileId": source_profile_id,
                    "profileId": profile_id,
                    "code": "migration_profile_control_mismatch"
                }));
                continue;
            }
            if let Some(source_proxy_id) = source_proxy_id.as_deref() {
                let binding = match self
                    .client
                    .profile_binding(&profile_id, &correlation_id)
                    .await
                {
                    Ok(binding) => binding,
                    Err(error)
                        if error.status >= 500 || error.status == 408 || error.status == 429 =>
                    {
                        return Err(error);
                    }
                    Err(error) => {
                        failed.push(json!({
                            "sourceProfileId": source_profile_id,
                            "profileId": profile_id,
                            "code": error.code
                        }));
                        continue;
                    }
                };
                if binding.profile_id != profile_id
                    || binding.archive_state != ProfileArchiveState::Active
                {
                    failed.push(json!({
                        "sourceProfileId": source_profile_id,
                        "profileId": profile_id,
                        "code": "managed_profile_binding_required"
                    }));
                    continue;
                }
                let scope_key = format!("{}:{source_proxy_id}", binding.workspace_id.as_str());
                if !imported_proxy_scopes.contains(&scope_key) {
                    let export = proxy_export.as_ref().ok_or_else(|| {
                        Problem::new(
                            "migration_proxy_source_invalid",
                            "A profile references a proxy that is missing from the prepared source",
                            422,
                            &correlation_id,
                        )
                    })?;
                    let mapped_proxy = export
                        .records
                        .iter()
                        .find(|record| record.source_ref.id == *source_proxy_id)
                        .ok_or_else(|| {
                            Problem::new(
                                "migration_proxy_source_invalid",
                                "A referenced legacy proxy could not be mapped",
                                422,
                                &correlation_id,
                            )
                        })?;
                    let credential = export.credentials.iter().find(|secret| {
                        mapped_proxy.data.credential_ref.as_deref()
                            == Some(secret.credential_id.as_str())
                    });
                    let imported_proxy = self
                        .ensure_legacy_proxy(
                            &binding.workspace_id,
                            mapped_proxy,
                            credential,
                            &version.instance_id,
                            &correlation_id,
                        )
                        .await;
                    if let Err(error) = imported_proxy {
                        if error.status >= 500 || error.status == 408 || error.status == 429 {
                            return Err(error);
                        }
                        failed.push(json!({
                            "sourceProfileId": source_profile_id,
                            "profileId": profile_id,
                            "code": error.code
                        }));
                        continue;
                    }
                    imported_proxy_scopes.insert(scope_key);
                    imported_proxy_count += 1;
                }
            }
            let profile_root = profiles_root.join(&local_id);
            let profile_root_metadata = match fs::symlink_metadata(&profile_root) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    preserve_only.push(json!({
                        "sourceProfileId": source_profile_id,
                        "profileId": profile_id,
                        "reason": "browser_data_missing"
                    }));
                    continue;
                }
                Err(_) => {
                    return Err(unavailable(
                        "migration_profile_source_unavailable",
                        &correlation_id,
                    ));
                }
            };
            if !profile_root_metadata.is_dir() || profile_root_metadata.file_type().is_symlink() {
                failed.push(json!({
                    "sourceProfileId": source_profile_id,
                    "profileId": profile_id,
                    "code": "migration_profile_source_invalid"
                }));
                continue;
            }
            let data_root = profile_root.join("profile");
            let profile_metadata = match fs::symlink_metadata(&data_root) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    preserve_only.push(json!({
                        "sourceProfileId": source_profile_id,
                        "profileId": profile_id,
                        "reason": "browser_data_missing"
                    }));
                    continue;
                }
                Err(_) => {
                    return Err(unavailable(
                        "migration_profile_source_unavailable",
                        &correlation_id,
                    ));
                }
            };
            if !profile_metadata.is_dir() || profile_metadata.file_type().is_symlink() {
                failed.push(json!({
                    "sourceProfileId": source_profile_id,
                    "profileId": profile_id,
                    "code": "migration_profile_source_invalid"
                }));
                continue;
            }
            let profile_key = format!(
                "migration_{:x}",
                Sha256::digest(format!("{idempotency_key}:{}", profile_id.as_str()).as_bytes())
            );
            match self
                .import_legacy_profile_data(
                    profile_id.clone(),
                    data_root,
                    profile_key,
                    correlation_id.clone(),
                )
                .await
            {
                Ok(receipt) => {
                    if receipt.get("status").and_then(Value::as_str) == Some("already_imported") {
                        already_imported.push(receipt);
                    } else {
                        imported.push(receipt);
                    }
                }
                Err(error) if error.code == "migration_profile_source_empty" => {
                    preserve_only.push(json!({
                        "sourceProfileId": source_profile_id,
                        "profileId": profile_id,
                        "reason": "browser_data_empty"
                    }));
                }
                Err(error) if error.status >= 500 || error.status == 408 || error.status == 429 => {
                    return Err(error);
                }
                Err(error) => failed.push(json!({
                    "sourceProfileId": source_profile_id,
                    "profileId": profile_id,
                    "code": error.code
                })),
            }
        }
        Ok(json!({
            "status": if failed.is_empty() { "completed" } else { "completed_with_errors" },
            "sourceProfileCount": processed,
            "proxiesImportedOrReconciled": imported_proxy_count,
            "imported": imported,
            "alreadyImported": already_imported,
            "preserveOnly": preserve_only,
            "failed": failed
        }))
    }

    async fn ensure_legacy_proxy(
        &self,
        workspace_id: &WorkspaceId,
        proxy: &buglogin_migration::legacy::NeutralProxyRecord,
        credential: Option<&buglogin_migration::encrypted_secrets::ProxyCredentialSecret>,
        source_instance_id: &str,
        correlation_id: &str,
    ) -> Result<(), Problem> {
        let proxy_id =
            ProxyId::new(proxy.id.clone()).map_err(|_| invalid_input("proxyId", correlation_id))?;
        let created_at = proxy
            .created_at
            .as_deref()
            .unwrap_or("1970-01-01T00:00:00.000Z");
        let updated_at = proxy.updated_at.as_deref().unwrap_or(created_at);
        let proxy_metadata = json!({
            "id": proxy_id,
            "name": proxy.data.name,
            "protocol": proxy.data.protocol,
            "host": proxy.data.host,
            "port": proxy.data.port,
            "bypassRules": proxy.data.bypass_rules,
            "createdAt": created_at,
            "updatedAt": updated_at
        });
        let credential_payload = match credential {
            Some(credential) => {
                let (Some(username), Some(password)) =
                    (credential.username.as_ref(), credential.password.as_ref())
                else {
                    return Err(Problem::new(
                        "migration_proxy_credential_invalid",
                        "A referenced legacy proxy has incomplete credentials and cannot be migrated",
                        422,
                        correlation_id,
                    ));
                };
                if username.is_empty() || password.is_empty() {
                    return Err(Problem::new(
                        "migration_proxy_credential_invalid",
                        "A referenced legacy proxy has incomplete credentials and cannot be migrated",
                        422,
                        correlation_id,
                    ));
                }
                Some(SensitiveJson::new(json!({
                    "username": username,
                    "password": password
                })))
            }
            None => None,
        };
        self.reconcile_pending_proxy_mutation(workspace_id, &proxy_id, correlation_id)
            .await?;
        let current = self
            .client
            .invoke(
                "proxy_get",
                correlation_id,
                None,
                json!({
                    "workspaceId": workspace_id,
                    "proxyId": proxy_id
                }),
            )
            .await;
        let existing = match current {
            Ok(current) => Some(current.data),
            Err(error) if error.status == 404 => None,
            Err(error) => return Err(error),
        };
        let (command, expected_revision) = if let Some(existing) = existing {
            let record = existing.get("proxy").ok_or_else(|| {
                Problem::new(
                    "migration_proxy_conflict",
                    "An existing proxy with the mapped ID has an invalid API record",
                    502,
                    correlation_id,
                )
            })?;
            let same_metadata = record.get("id").and_then(Value::as_str) == Some(proxy.id.as_str())
                && record.get("name").and_then(Value::as_str) == proxy.data.name.as_deref()
                && record.get("protocol").and_then(Value::as_str)
                    == Some(proxy.data.protocol.as_str())
                && record.get("host").and_then(Value::as_str) == Some(proxy.data.host.as_str())
                && record.get("port").and_then(Value::as_u64) == Some(u64::from(proxy.data.port))
                && record
                    .get("bypassRules")
                    .and_then(Value::as_array)
                    .is_some_and(|rules| {
                        rules.len() == proxy.data.bypass_rules.len()
                            && rules.iter().all(Value::is_string)
                            && rules.iter().filter_map(Value::as_str).eq(proxy
                                .data
                                .bypass_rules
                                .iter()
                                .map(String::as_str))
                    });
            if !same_metadata {
                return Err(Problem::new(
                    "migration_proxy_conflict",
                    "An existing proxy with the mapped ID differs from the legacy source; it was preserved",
                    409,
                    correlation_id,
                ));
            }
            let revision = existing
                .get("revision")
                .and_then(Value::as_u64)
                .ok_or_else(|| {
                    Problem::new(
                        "migration_proxy_conflict",
                        "An existing proxy has no valid revision",
                        502,
                        correlation_id,
                    )
                })?;
            let has_credential = record.get("hasCredential").and_then(Value::as_bool);
            if credential_payload.is_none() && has_credential == Some(true) {
                return Err(Problem::new(
                    "migration_proxy_conflict",
                    "An existing proxy has credentials absent from the legacy source; it was preserved",
                    409,
                    correlation_id,
                ));
            }
            if credential_payload.is_none() && has_credential == Some(false) {
                return Ok(());
            }
            if let Some(credential) = credential {
                let username = credential
                    .username
                    .as_deref()
                    .unwrap_or_default()
                    .to_owned();
                let password = credential
                    .password
                    .as_deref()
                    .unwrap_or_default()
                    .to_owned();
                let encoded = serde_json::to_vec(&ProxyCredentialInput { username, password })
                    .map_err(|_| serialization(correlation_id))?;
                let expected =
                    SecretValue::new(encoded).map_err(|_| serialization(correlation_id))?;
                if has_credential == Some(true)
                    && self
                        .proxy_credential_vault
                        .load(workspace_id, &proxy_id, revision, correlation_id)?
                        .is_some_and(|stored| stored.as_bytes() == expected.as_bytes())
                {
                    return Ok(());
                }
            }
            ("proxy_update", Some(revision))
        } else {
            ("proxy_create", None)
        };
        let digest = Sha256::digest(
            format!(
                "legacy-proxy:{source_instance_id}:{}:{}:{}",
                proxy.source_ref.id,
                workspace_id.as_str(),
                expected_revision.unwrap_or_default()
            )
            .as_bytes(),
        );
        let idempotency_key = format!("migration_proxy_{:x}", digest);
        let mut payload = if command == "proxy_create" {
            json!({
                "workspaceId": workspace_id,
                "proxy": proxy_metadata
            })
        } else {
            json!({
                "workspaceId": workspace_id,
                "proxyId": proxy_id,
                "expectedRevision": expected_revision,
                "proxy": proxy_metadata
            })
        };
        if let Some(credential) = credential_payload {
            payload["credential"] = credential.into_value();
        }
        let result = self
            .invoke_proxy_mutation(command, correlation_id, Some(&idempotency_key), payload)
            .await?;
        if result.data.get("deduplicated").and_then(Value::as_bool) == Some(true) {
            return Err(Problem::new(
                "migration_proxy_conflict",
                "A different proxy already uses the legacy proxy endpoint; the profile was not imported",
                409,
                correlation_id,
            ));
        }
        Ok(())
    }

    async fn import_legacy_profile_data(
        &self,
        profile_id: ProfileId,
        source_root: PathBuf,
        idempotency_key: String,
        correlation_id: String,
    ) -> Result<Value, Problem> {
        let binding = self
            .client
            .profile_binding(&profile_id, &correlation_id)
            .await?;
        if binding.archive_state != ProfileArchiveState::Active || binding.profile_id != profile_id
        {
            return Err(Problem::new(
                "managed_profile_binding_required",
                "Legacy profile import requires an active managed profile binding",
                409,
                &correlation_id,
            ));
        }
        let profile = self.client.profile(&profile_id, &correlation_id).await?;
        let auth = self.auth(&correlation_id)?;
        let scope = CoordinationScope {
            workspace_id: &binding.workspace_id,
            auth: &auth,
        };
        let current = self
            .client
            .current_manifest(scope.clone(), &binding.resource_id)
            .await?
            .ok_or_else(|| {
                Problem::new(
                    "production_manifest_not_found",
                    "Profile migration requires its provisioned genesis manifest",
                    409,
                    &correlation_id,
                )
            })?;
        if manifest_has_profile_data(&current) {
            return Ok(json!({
                "profileId": profile_id,
                "status": "already_imported",
                "generation": current.generation,
                "rootChecksum": current.root_checksum
            }));
        }
        if profile.value.state != ProfileState::Created {
            return Err(Problem::new(
                "migration_profile_state_invalid",
                "Only a newly imported profile without browser data can receive legacy profile files",
                409,
                &correlation_id,
            ));
        }
        if self
            .profile_runtime
            .running_profiles()
            .await?
            .iter()
            .any(|running| running == &profile_id)
        {
            return Err(Problem::new(
                "migration_profile_running",
                "Stop this profile before importing its legacy browser data",
                409,
                &correlation_id,
            ));
        }
        let lease = self
            .client
            .acquire_lease_idempotent(
                scope.clone(),
                &binding.resource_id,
                Duration::from_secs(120),
                &lease_idempotency_key(&idempotency_key, current.generation),
            )
            .await?;
        if let Err(error) =
            self.projection
                .stage_legacy_profile_data(&profile_id, &current, &source_root)
        {
            let _ = self
                .client
                .release_lease(scope.clone(), &binding.resource_id, &lease.owner_token)
                .await;
            return Err(error);
        }
        self.stop_profile(
            profile_id.clone(),
            idempotency_key,
            correlation_id.clone(),
            None,
            Some(lease),
            None,
        )
        .await?;
        let active = self
            .client
            .current_manifest(scope, &binding.resource_id)
            .await?
            .ok_or_else(|| {
                Problem::new(
                    "production_manifest_not_found",
                    "Profile sync did not publish an active migration manifest",
                    503,
                    &correlation_id,
                )
            })?;
        if !manifest_has_profile_data(&active) {
            return Err(Problem::new(
                "migration_profile_sync_incomplete",
                "Legacy profile data was staged but its encrypted sync manifest is not active",
                503,
                &correlation_id,
            )
            .retryable(true));
        }
        Ok(json!({
            "profileId": profile_id,
            "status": "completed",
            "generation": active.generation,
            "rootChecksum": active.root_checksum
        }))
    }

    async fn prepare_idle_profile_for_key_rotation(
        &self,
        binding: &buglogin_domain::profile::ProfileResource,
        profile_id: &ProfileId,
        scope: &CoordinationScope<'_>,
        idempotency_key: &str,
        correlation_id: &str,
    ) -> Result<StopReceipt, Problem> {
        let (previous, stale_proxy_dependency) = match self
            .client
            .current_manifest(scope.clone(), &binding.resource_id)
            .await
        {
            Ok(Some(manifest)) => (manifest, false),
            Err(error) if error.code == "manifest_dependency_stale" => (
                self.client
                    .current_manifest_for_proxy_repair(
                        scope.clone(),
                        &binding.resource_id,
                        correlation_id,
                    )
                    .await?
                    .ok_or_else(|| {
                        Problem::new(
                            "production_manifest_not_found",
                            "Production profile manifest not found for proxy repair",
                            409,
                            correlation_id,
                        )
                    })?,
                true,
            ),
            Ok(None) => {
                return Err(Problem::new(
                    "production_manifest_not_found",
                    "Production profile manifest not found",
                    409,
                    correlation_id,
                ));
            }
            Err(error) => return Err(error),
        };
        let keys = if let Some(encryption) = previous.payload_encryption.as_ref() {
            let version = encryption.key_version.as_deref().ok_or_else(|| {
                Problem::new(
                    "profile_key_version_missing",
                    "Profile sync manifest has no active resource key version",
                    409,
                    correlation_id,
                )
            })?;
            match self.sync_key_vault.load_for_manifest(
                &binding.workspace_id,
                &binding.resource_id,
                version,
                correlation_id,
            ) {
                Ok(Some(keys)) => keys,
                Ok(None) => {
                    self.recover_profile_key_envelope(binding, profile_id, version, correlation_id)
                        .await?
                }
                Err(error) if error.code == "profile_key_version_stale" => {
                    self.recover_profile_key_envelope(binding, profile_id, version, correlation_id)
                        .await?
                }
                Err(error) => return Err(error),
            }
        } else {
            self.sync_key_vault.load_or_create(
                &binding.workspace_id,
                &binding.resource_id,
                correlation_id,
            )?
        };
        let hydration = if stale_proxy_dependency {
            self.projection
                .hydrate_encrypted_with_proxy_credentials_for_repair(
                    &self.client,
                    scope.auth,
                    profile_id,
                    &previous,
                    Some(&keys.client),
                    Some(&keys.recovery),
                    correlation_id,
                )
                .await
        } else {
            self.projection
                .hydrate_encrypted_with_proxy_credentials(
                    &self.client,
                    scope.auth,
                    profile_id,
                    &previous,
                    Some(&keys.client),
                    Some(&keys.recovery),
                    correlation_id,
                )
                .await
        };
        let (projection, proxy_credentials) = hydration?;
        for credential in proxy_credentials {
            self.proxy_credential_vault.store(
                &binding.workspace_id,
                &credential.proxy_id,
                credential.revision,
                credential.secret,
                correlation_id,
            )?;
        }
        let lease = self
            .client
            .acquire_lease_idempotent(
                scope.clone(),
                &binding.resource_id,
                Duration::from_secs(120),
                &lease_idempotency_key(idempotency_key, previous.generation),
            )
            .await?;
        let latest = match self
            .client
            .current_manifest(scope.clone(), &binding.resource_id)
            .await
        {
            Ok(Some(manifest)) => manifest,
            Err(error) if error.code == "manifest_dependency_stale" => match self
                .client
                .current_manifest_for_proxy_repair(
                    scope.clone(),
                    &binding.resource_id,
                    correlation_id,
                )
                .await
            {
                Ok(Some(manifest)) => manifest,
                Ok(None) => {
                    let _ = self
                        .client
                        .release_lease(scope.clone(), &binding.resource_id, &lease.owner_token)
                        .await;
                    return Err(Problem::new(
                        "production_manifest_not_found",
                        "Production profile manifest not found for proxy repair",
                        409,
                        correlation_id,
                    ));
                }
                Err(error) => {
                    let _ = self
                        .client
                        .release_lease(scope.clone(), &binding.resource_id, &lease.owner_token)
                        .await;
                    return Err(error);
                }
            },
            Ok(None) => {
                let _ = self
                    .client
                    .release_lease(scope.clone(), &binding.resource_id, &lease.owner_token)
                    .await;
                return Err(Problem::new(
                    "production_manifest_not_found",
                    "Production profile manifest not found",
                    409,
                    correlation_id,
                ));
            }
            Err(error) => {
                let _ = self
                    .client
                    .release_lease(scope.clone(), &binding.resource_id, &lease.owner_token)
                    .await;
                return Err(error);
            }
        };
        if latest != previous {
            let _ = self
                .client
                .release_lease(scope.clone(), &binding.resource_id, &lease.owner_token)
                .await;
            return Err(Problem::new(
                "production_manifest_changed",
                "Profile data changed while preparing key rotation; retry after refreshing",
                409,
                correlation_id,
            )
            .retryable(true));
        }
        let lease_scope = LeaseScope {
            workspace_id: binding.workspace_id.clone(),
            resource_id: binding.resource_id.clone(),
        };
        match self
            .profile_runtime
            .hold_idle_profile_for_sync(
                profile_id.clone(),
                projection.path,
                lease_scope,
                lease.clone(),
            )
            .await
        {
            Ok(receipt) => Ok(receipt),
            Err(error) => {
                let _ = self
                    .client
                    .release_lease(scope.clone(), &binding.resource_id, &lease.owner_token)
                    .await;
                Err(error)
            }
        }
    }

    async fn upload_profile_snapshot(
        &self,
        scope: CoordinationScope<'_>,
        resource_id: &buglogin_domain::ids::ResourceId,
        owner_token: &str,
        fencing_token: u64,
        idempotency_key: &str,
        descriptor: &PayloadDescriptor,
        files: Vec<SnapshotBlob>,
        manifest: &buglogin_sync::manifest::SyncManifest,
        correlation_id: &str,
    ) -> Result<(), Problem> {
        if descriptor.workspace_id != *scope.workspace_id
            || descriptor.resource_id != *resource_id
            || descriptor.generation != manifest.generation
            || descriptor.root_checksum != manifest.root_checksum
        {
            return Err(Problem::new(
                "profile_snapshot_incomplete",
                "Profile snapshot does not match the manifest being committed",
                409,
                correlation_id,
            ));
        }
        let object_map: Arc<BTreeMap<String, buglogin_sync::payload::PayloadObject>> = Arc::new(
            descriptor
                .objects
                .iter()
                .cloned()
                .map(|object| (object.path.clone(), object))
                .collect(),
        );
        let reused_object_keys: Arc<BTreeSet<String>> = Arc::new(
            manifest
                .objects
                .iter()
                .filter(|object| object.source_generation.is_some())
                .map(|object| object.key.clone())
                .collect(),
        );
        let client = self.client.clone();
        let resource_id = resource_id.clone();
        let generation = manifest.generation;
        let owner_token = owner_token.to_owned();
        let correlation_id = correlation_id.to_owned();
        let idempotency_key = idempotency_key.to_owned();
        stream::iter(files.into_iter().map(|blob| {
            let client = client.clone();
            let scope = scope.clone();
            let owner_token = owner_token.clone();
            let correlation_id = correlation_id.clone();
            let idempotency_key = idempotency_key.clone();
            let object_map = Arc::clone(&object_map);
            let reused_object_keys = Arc::clone(&reused_object_keys);
            let resource_id = resource_id.clone();
            async move {
                if reused_object_keys.contains(&blob.key) {
                    return Ok(());
                }
                let object = object_map.get(&blob.key).ok_or_else(|| {
                    Problem::new(
                        "profile_snapshot_incomplete",
                        "Profile snapshot is incomplete",
                        409,
                        &correlation_id,
                    )
                })?;
                if blob.size != object.size || blob.checksum != object.checksum {
                    return Err(Problem::new(
                        "profile_snapshot_incomplete",
                        "Staged profile object does not match its manifest descriptor",
                        409,
                        &correlation_id,
                    ));
                }
                let mut presence_attempt = 0;
                let present = loop {
                    match client
                        .blob_present(
                            scope.clone(),
                            &resource_id,
                            &blob.key,
                            generation,
                            &object.checksum,
                            &owner_token,
                            fencing_token,
                            &correlation_id,
                        )
                        .await
                    {
                        Ok(present) => break present,
                        Err(problem) if problem.retryable && presence_attempt == 0 => {
                            presence_attempt += 1;
                            tokio::time::sleep(Duration::from_millis(250)).await;
                        }
                        Err(problem) => return Err(problem),
                    }
                };
                if present {
                    return Ok(());
                }

                let upload_idempotency_key =
                    blob_idempotency_key(&idempotency_key, &blob.key, &object.checksum);
                let mut upload_attempt = 0;
                loop {
                    let bytes = blob.read_bytes()?;
                    match client
                        .upload_blob(
                            scope.clone(),
                            &resource_id,
                            &blob.key,
                            generation,
                            &object.checksum,
                            &owner_token,
                            fencing_token,
                            &upload_idempotency_key,
                            bytes,
                            &correlation_id,
                        )
                        .await
                    {
                        Ok(()) => return Ok(()),
                        Err(problem)
                            if problem.retryable
                                && upload_attempt
                                    < match problem.code.as_str() {
                                        "sync_object_cleanup_in_progress" => 5,
                                        "large_transfer_capacity_exhausted" => 6,
                                        _ => 1,
                                    } =>
                        {
                            let delay_ms = if matches!(
                                problem.code.as_str(),
                                "sync_object_cleanup_in_progress"
                                    | "large_transfer_capacity_exhausted"
                            ) {
                                250_u64.saturating_mul(1_u64 << upload_attempt.min(4))
                            } else {
                                250
                            };
                            upload_attempt += 1;
                            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                        }
                        Err(problem) => return Err(problem),
                    }
                }
            }
        }))
        .buffer_unordered(MAX_CONCURRENT_PROFILE_BLOB_UPLOADS)
        .try_collect::<Vec<()>>()
        .await?;
        Ok(())
    }

    async fn run_operation<F, Fut>(
        &self,
        command_id: &'static str,
        idempotency_key: &str,
        payload_hash: &str,
        correlation_id: &str,
        action: F,
    ) -> Result<Value, Problem>
    where
        F: FnOnce(ProductionDesktopState) -> Fut + Send + 'static,
        Fut: Future<Output = Result<Value, Problem>> + Send + 'static,
    {
        let mut gate = self
            .operation_gate
            .lock()
            .map_err(|_| unavailable("operation_gate_unavailable", correlation_id))?;
        if gate.draining {
            return Err(Problem::new(
                "desktop_draining",
                "Desktop is draining active operations",
                503,
                correlation_id,
            )
            .retryable(true));
        }
        let operation_id = operation_id(command_id, idempotency_key);
        let accepted =
            json!({ "operationId": operation_id, "commandId": command_id, "status": "accepted" });
        {
            let mut records = self
                .idempotency
                .lock()
                .map_err(|_| unavailable("idempotency_unavailable", correlation_id))?;
            if let Some(record) = records.get(idempotency_key) {
                if record.command_id != command_id || record.payload_hash != payload_hash {
                    return Err(Problem::new(
                        "idempotency_conflict",
                        "Idempotency key conflict",
                        409,
                        correlation_id,
                    ));
                }
                return Ok(record.accepted.clone());
            }
            let mut idempotency_order = self
                .idempotency_order
                .lock()
                .map_err(|_| unavailable("idempotency_unavailable", correlation_id))?;
            make_idempotency_room(&mut records, &mut idempotency_order, correlation_id)?;
            self.operations
                .write()
                .map_err(|_| unavailable("operation_unavailable", correlation_id))?
                .insert(Operation::queued(
                    &operation_id,
                    command_id,
                    ResourceScope::global(),
                )?)?;
            records.insert(
                idempotency_key.to_owned(),
                IdempotencyRecord {
                    command_id,
                    payload_hash: payload_hash.to_owned(),
                    operation_id: operation_id.clone(),
                    accepted: accepted.clone(),
                    completed: false,
                },
            );
            idempotency_order.push_back(idempotency_key.to_owned());
        }
        let state = self.clone();
        let worker_id = operation_id.clone();
        let worker_idempotency_key = idempotency_key.to_owned();
        gate.in_flight += 1;
        let operation_guard = OperationGuard {
            gate: Arc::clone(&self.operation_gate),
            drained: Arc::clone(&self.operation_drained),
        };
        drop(gate);
        tokio::spawn(async move {
            let _operation_guard = operation_guard;
            if let Ok(mut operations) = state.operations.write() {
                let _ = operations.transition(
                    &worker_id,
                    OperationStatus::Running,
                    Some(0.1),
                    None,
                    None,
                );
            }
            let outcome = action(state.clone()).await;
            let completion_recorded = if let Ok(mut operations) = state.operations.write() {
                match outcome {
                    Ok(value) => operations
                        .transition(
                            &worker_id,
                            OperationStatus::Completed,
                            Some(1.0),
                            Some(value),
                            None,
                        )
                        .is_ok(),
                    Err(error) => operations
                        .transition(
                            &worker_id,
                            OperationStatus::Failed,
                            Some(1.0),
                            None,
                            Some(error),
                        )
                        .is_ok(),
                }
            } else {
                false
            };
            if completion_recorded {
                if let Ok(mut records) = state.idempotency.lock()
                    && let Some(record) = records.get_mut(&worker_idempotency_key)
                    && record.operation_id == worker_id
                {
                    record.completed = true;
                }
            }
        });
        Ok(accepted)
    }

    async fn shutdown(&self) -> Result<(), Problem> {
        loop {
            let notified = self.operation_drained.notified();
            let in_flight = {
                let mut gate = self.operation_gate.lock().map_err(|_| {
                    unavailable("operation_gate_unavailable", "desktop-production-shutdown")
                })?;
                gate.draining = true;
                gate.in_flight
            };
            if in_flight == 0 {
                break;
            }
            notified.await;
        }
        let profiles = self.profile_runtime.running_profiles().await?;
        let mut first_error = None;
        for profile_id in profiles {
            // Key the stop to this browser session so a later quit with the same
            // profile never collides with the stored record of an earlier one.
            let session = match self
                .profile_runtime
                .write_authority(profile_id.clone())
                .await
            {
                Ok(Some(authority)) => authority.fencing_token,
                _ => std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|elapsed| elapsed.as_millis() as u64)
                    .unwrap_or_default(),
            };
            let idempotency_key = shutdown_idempotency_key(&profile_id, session);
            let _profile_operation = self
                .profile_operations
                .lock_for(&profile_id)
                .lock_owned()
                .await;
            if let Err(error) = self
                .stop_profile(
                    profile_id,
                    idempotency_key,
                    "desktop-production-shutdown".to_owned(),
                    None,
                    None,
                    None,
                )
                .await
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
        if let Some(error) = first_error {
            if let Ok(mut gate) = self.operation_gate.lock() {
                gate.draining = false;
            }
            return Err(error);
        }
        if let Err(error) = self.profile_runtime.shutdown().await {
            if let Ok(mut gate) = self.operation_gate.lock() {
                gate.draining = false;
            }
            return Err(error);
        }
        Ok(())
    }
}

fn production_transport(
    api_base_url: &str,
    token: Option<String>,
) -> Result<(RemoteApiClient, Option<CoordinationAuth>), Problem> {
    if let Some(token) = token.map(Zeroizing::new) {
        let client = RemoteApiClient::new(api_base_url, token.to_string())?;
        Ok((client, None))
    } else {
        Ok((RemoteApiClient::new_unauthenticated(api_base_url)?, None))
    }
}

fn local_cookie_principal() -> Principal {
    Principal {
        roles: BTreeSet::from([buglogin_domain::permissions::Role::LocalOwner]),
        ..Principal::default()
    }
}

fn runtime_state_unavailable(correlation_id: &str) -> Problem {
    Problem::new(
        "runtime_state_unavailable",
        "Desktop runtime state is unavailable",
        503,
        correlation_id,
    )
}

fn desktop_state_problem(correlation_id: &str) -> Problem {
    Problem::new(
        "desktop_state_unavailable",
        "Desktop state unavailable",
        503,
        correlation_id,
    )
}

fn auth_not_applicable(correlation_id: &str) -> Problem {
    Problem::new(
        "auth_not_applicable",
        "Authentication is not applicable in local mode",
        409,
        correlation_id,
    )
}

fn definitive_auth_error(problem: &Problem) -> bool {
    matches!(
        problem.code.as_str(),
        "invalid_session" | "session_expired" | "session_revoked"
    )
}

fn preserve_refresh_credentials(problem: &Problem) -> bool {
    problem.retryable && !definitive_auth_error(problem)
}

fn ambiguous_proxy_mutation_error(problem: &Problem) -> bool {
    problem.retryable
        || matches!(
            problem.code.as_str(),
            "idempotency_in_progress" | "idempotency_claim_conflict"
        )
}

fn proxy_mutation_requires_recovery(problem: &Problem) -> bool {
    ambiguous_proxy_mutation_error(problem)
        || matches!(
            problem.code.as_str(),
            "idempotency_conflict"
                | "proxy_mutation_recovery_invalid"
                | "proxy_mutation_recovery_version_unsupported"
        )
}

fn invalid_proxy_mutation_response(correlation_id: &str) -> Problem {
    Problem::new(
        "proxy_mutation_response_invalid",
        "BugLogin API returned a proxy change that does not match the pending request",
        502,
        correlation_id,
    )
}

impl DesktopState {
    pub async fn submit_feedback(
        &self,
        submission: FeedbackSubmission,
        idempotency_key: &str,
        correlation_id: &str,
    ) -> Result<FeedbackReceipt, Problem> {
        match self {
            Self::Production(state) => {
                state.refresh_session_if_needed(correlation_id).await?;
                if !state.client.has_token()? {
                    return Err(Problem::new(
                        "authentication_required",
                        "Sign in to send feedback",
                        401,
                        correlation_id,
                    ));
                }
                state
                    .client
                    .submit_feedback(submission, idempotency_key, correlation_id)
                    .await
            }
            Self::Local(_) => Err(Problem::new(
                "feedback_hosted_authority_required",
                "Feedback submission requires the signed-in BugLogin API",
                409,
                correlation_id,
            )),
            Self::Unavailable(problem) => Err(problem.clone()),
        }
    }

    pub async fn integration_status(
        &self,
        correlation_id: &str,
    ) -> Result<DesktopIntegrationStatus, Problem> {
        match self {
            Self::Local(_) => Ok(DesktopIntegrationStatus {
                deployment_mode: "local",
                api_endpoint: None,
                api_reachable: false,
                api_service: None,
                mcp_endpoint: None,
                mcp_enabled: Some(false),
            }),
            Self::Production(state) => {
                let status = state.client.integration_status(correlation_id).await?;
                Ok(DesktopIntegrationStatus {
                    deployment_mode: "production",
                    api_endpoint: Some(status.api_endpoint),
                    api_reachable: status.api_reachable,
                    api_service: status.api_service,
                    mcp_endpoint: Some(status.mcp_endpoint),
                    mcp_enabled: status.mcp_enabled,
                })
            }
            Self::Unavailable(problem) => Err(problem.clone()),
        }
    }

    pub async fn list_api_tokens(&self, correlation_id: &str) -> Result<ApiTokenPage, Problem> {
        match self {
            Self::Production(state) => state.client.list_api_tokens(correlation_id).await,
            Self::Local(_) => Err(Problem::new(
                "api_token_hosted_authority_required",
                "Scoped API tokens require the signed-in BugLogin API authority",
                409,
                correlation_id,
            )),
            Self::Unavailable(problem) => Err(problem.clone()),
        }
    }

    pub async fn create_api_token(
        &self,
        name: &str,
        scopes: BTreeSet<String>,
        expires_at: u64,
        correlation_id: &str,
    ) -> Result<CreatedApiToken, Problem> {
        match self {
            Self::Production(state) => {
                state
                    .client
                    .create_api_token(name, scopes, expires_at, correlation_id)
                    .await
            }
            Self::Local(_) => Err(Problem::new(
                "api_token_hosted_authority_required",
                "Scoped API tokens require the signed-in BugLogin API authority",
                409,
                correlation_id,
            )),
            Self::Unavailable(problem) => Err(problem.clone()),
        }
    }

    pub async fn revoke_api_token(
        &self,
        token_id: &str,
        correlation_id: &str,
    ) -> Result<(), Problem> {
        match self {
            Self::Production(state) => {
                state
                    .client
                    .revoke_api_token(token_id, correlation_id)
                    .await
            }
            Self::Local(_) => Err(Problem::new(
                "api_token_hosted_authority_required",
                "Scoped API tokens require the signed-in BugLogin API authority",
                409,
                correlation_id,
            )),
            Self::Unavailable(problem) => Err(problem.clone()),
        }
    }

    pub(crate) async fn register_runtime_target(
        &self,
        runtime: Runtime,
        target: RuntimeTarget,
        correlation_id: &str,
    ) -> Result<(), Problem> {
        let Self::Production(state) = self else {
            return Err(Problem::new(
                "runtime_install_hosted_only",
                "Verified browser runtimes require BugLogin hosted authority",
                409,
                correlation_id,
            ));
        };
        if runtime != Runtime::Wayfern {
            return Err(Problem::new(
                "runtime_not_supported",
                "Runtime has no approved desktop adapter",
                400,
                correlation_id,
            ));
        }
        state
            .profile_runtime
            .approve_executable(target.binary.path.clone())
            .await?;
        let mut targets = state
            .runtime_targets
            .write()
            .map_err(|_| runtime_state_unavailable(correlation_id))?;
        targets.insert(runtime, target);
        Ok(())
    }

    pub(crate) fn runtime_installed_versions_get(
        &self,
        runtime: &Runtime,
        correlation_id: &str,
    ) -> Result<Vec<String>, Problem> {
        match self {
            Self::Production(state) => {
                let targets = state
                    .runtime_targets
                    .read()
                    .map_err(|_| runtime_state_unavailable(correlation_id))?;
                Ok(targets.versions(runtime))
            }
            Self::Local(state) => Ok(state.runtime_installed_versions_get(runtime)),
            Self::Unavailable(problem) => Err(problem.clone()),
        }
    }

    pub(crate) fn wayfern_executable_path_get(
        &self,
        correlation_id: &str,
    ) -> Result<PathBuf, Problem> {
        let Self::Production(state) = self else {
            return Err(Problem::new(
                "runtime_not_configured",
                "Wayfern runtime is not configured",
                409,
                correlation_id,
            ));
        };
        let targets = state
            .runtime_targets
            .read()
            .map_err(|_| runtime_state_unavailable(correlation_id))?;
        targets
            .latest_target(&Runtime::Wayfern)
            .map(|target| target.binary.path.clone())
            .ok_or_else(|| {
                Problem::new(
                    "runtime_not_configured",
                    "Wayfern runtime is not configured",
                    409,
                    correlation_id,
                )
            })
    }

    pub async fn runtime_catalog_get(
        &self,
        correlation_id: &str,
    ) -> Result<RuntimeCatalog, Problem> {
        let verifier = crate::runtime_config::pinned_runtime_verifier(correlation_id)?;
        match self {
            Self::Production(state) => {
                match state
                    .client
                    .verified_runtime_catalog(&verifier, correlation_id)
                    .await
                {
                    Ok(catalog) => Ok(catalog),
                    Err(_) => {
                        crate::runtime_config::fallback_runtime_catalog(&verifier, correlation_id)
                    }
                }
            }
            Self::Local(_) => {
                crate::runtime_config::fallback_runtime_catalog(&verifier, correlation_id)
            }
            Self::Unavailable(problem) => Err(problem.clone()),
        }
    }

    pub fn selected_workspace_id(&self, account_id: Option<&str>) -> Option<String> {
        match self {
            Self::Production(state) => {
                let selections = state.workspace_selections.lock().ok()?;
                account_id
                    .and_then(|id| selections.accounts.get(id).cloned())
                    .or_else(|| selections.legacy_workspace_id.clone())
            }
            Self::Local(_) | Self::Unavailable(_) => None,
        }
    }

    pub fn set_selected_workspace_id(
        &self,
        account_id: &str,
        workspace_id: String,
    ) -> Result<(), Problem> {
        match self {
            Self::Production(state) => {
                AccountId::new(account_id.to_owned()).map_err(|_| {
                    Problem::new(
                        "invalid_account_id",
                        "Account identifier is invalid",
                        400,
                        "desktop-workspace-selection",
                    )
                })?;
                WorkspaceId::new(workspace_id.clone()).map_err(|_| {
                    Problem::new(
                        "invalid_workspace_id",
                        "Workspace identifier is invalid",
                        400,
                        "desktop-workspace-selection",
                    )
                })?;
                let mut selections = state.workspace_selections.lock().map_err(|_| {
                    Problem::new(
                        "workspace_selection_unavailable",
                        "Workspace selection is unavailable",
                        500,
                        "desktop-workspace-selection",
                    )
                })?;
                if selections.accounts.get(account_id) == Some(&workspace_id)
                    && selections.legacy_workspace_id.is_none()
                {
                    return Ok(());
                }
                let previous = selections.clone();
                selections
                    .accounts
                    .insert(account_id.to_owned(), workspace_id);
                selections.legacy_workspace_id = None;
                if selections.persist(&state.selected_workspace_path).is_err() {
                    *selections = previous;
                    return Err(Problem::new(
                        "workspace_selection_unavailable",
                        "Workspace selection could not be saved",
                        500,
                        "desktop-workspace-selection",
                    ));
                }
                Ok(())
            }
            Self::Local(_) => Err(Problem::new(
                "hosted_workspace_required",
                "Sign in to select a hosted workspace",
                409,
                "desktop-workspace-selection",
            )),
            Self::Unavailable(problem) => Err(problem.clone()),
        }
    }

    pub async fn event_stream_client(
        &self,
        correlation_id: &str,
    ) -> Result<RemoteApiClient, Problem> {
        match self {
            Self::Production(state) => {
                state.refresh_session_if_needed(correlation_id).await?;
                if !state.client.has_token()? {
                    return Err(Problem::new(
                        "authentication_required",
                        "Sign in to BugLogin",
                        401,
                        correlation_id,
                    ));
                }
                Ok(state.client.clone())
            }
            Self::Local(_) => Err(Problem::new(
                "hosted_event_stream_required",
                "Workspace events require hosted production authority",
                409,
                correlation_id,
            )),
            Self::Unavailable(problem) => Err(problem.clone()),
        }
    }

    pub fn local(state: AdapterState) -> Self {
        Self::Local(state)
    }

    pub fn unavailable(problem: Problem) -> Self {
        Self::Unavailable(problem)
    }

    pub fn profiles_root(&self) -> Option<&Path> {
        match self {
            Self::Local(state) => Some(state.profiles_root()),
            Self::Production(state) => Some(state.profiles_root()),
            Self::Unavailable(_) => None,
        }
    }

    pub fn auth_status(&self) -> Result<DesktopAuthStatus, Problem> {
        match self {
            Self::Local(_) => Ok(DesktopAuthStatus::AuthNotApplicable),
            Self::Production(state) => state.auth_status(),
            Self::Unavailable(problem) => Err(problem.clone()),
        }
    }

    pub fn start_background_auth_refresh(&self) {
        if let Self::Production(state) = self {
            state.start_background_auth_refresh();
        }
    }

    pub async fn restore_auth(&self, correlation_id: &str) -> Result<(), Problem> {
        if let Self::Production(state) = self {
            state.refresh_session_if_needed(correlation_id).await?;
            state
                .ensure_authenticated_coordination(correlation_id)
                .await?;
            if state.client.has_token()? {
                state.ensure_device_key_registered(correlation_id).await?;
            }
        }
        Ok(())
    }

    pub async fn login_with_credentials(
        &self,
        identifier: Zeroizing<String>,
        password: Zeroizing<String>,
        correlation_id: &str,
    ) -> Result<DesktopAuthStatus, Problem> {
        match self {
            Self::Local(_) => Err(auth_not_applicable(correlation_id)),
            Self::Production(state) => {
                state
                    .login_with_credentials(identifier, password, correlation_id)
                    .await
            }
            Self::Unavailable(problem) => Err(problem.clone()),
        }
    }

    pub async fn start_buglab_sso(
        &self,
        correlation_id: &str,
    ) -> Result<BugLabSsoStartResponse, Problem> {
        match self {
            Self::Local(_) => Err(auth_not_applicable(correlation_id)),
            Self::Production(state) => state.start_buglab_sso(correlation_id).await,
            Self::Unavailable(problem) => Err(problem.clone()),
        }
    }

    pub async fn complete_buglab_sso(
        &self,
        state: Zeroizing<String>,
        token: Zeroizing<String>,
        correlation_id: &str,
    ) -> Result<DesktopAuthStatus, Problem> {
        match self {
            Self::Local(_) => Err(auth_not_applicable(correlation_id)),
            Self::Production(production) => {
                production
                    .complete_buglab_sso(state, token, correlation_id)
                    .await
            }
            Self::Unavailable(problem) => Err(problem.clone()),
        }
    }

    pub async fn logout(&self, correlation_id: &str) -> Result<DesktopAuthStatus, Problem> {
        match self {
            Self::Local(_) => Err(auth_not_applicable(correlation_id)),
            Self::Production(state) => state.logout(correlation_id).await,
            Self::Unavailable(problem) => Err(problem.clone()),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn production(
        api_base_url: &str,
        token: Option<String>,
        device_id: String,
        profiles_root: PathBuf,
        runtime_targets: RuntimeTargets,
    ) -> Result<Self, Problem> {
        Ok(Self::Production(ProductionDesktopState::open(
            api_base_url,
            token,
            device_id,
            profiles_root,
            runtime_targets,
        )?))
    }

    pub async fn invoke(
        &self,
        invoke_name: &str,
        correlation_id: &str,
        idempotency_key: Option<&str>,
        payload: Value,
    ) -> Result<CommandResult<Value>, Problem> {
        match self {
            Self::Local(_) if invoke_name == "migration_profile_data_import" => Err(Problem::new(
                "hosted_profile_required",
                "Legacy profile data import requires an authenticated hosted profile",
                409,
                correlation_id,
            )),
            Self::Local(_) if invoke_name == "resource_lease_get" => Err(Problem::new(
                "hosted_coordination_required",
                "Team lock status requires an authenticated workspace API",
                409,
                correlation_id,
            )),
            Self::Local(_) if invoke_name == "profile_runtime_get" => Err(Problem::new(
                "hosted_coordination_required",
                "Managed profile runtime status requires the authenticated workspace API",
                409,
                correlation_id,
            )),
            Self::Local(state) => {
                state
                    .invoke(invoke_name, correlation_id, idempotency_key, payload)
                    .await
            }
            Self::Production(state) => {
                if matches!(invoke_name, "migration_inspect" | "migration_import") {
                    return Err(Problem::new(
                        "production_local_migration_forbidden",
                        "Production cannot run local filesystem migration",
                        409,
                        correlation_id,
                    ));
                }
                state
                    .invoke(invoke_name, correlation_id, idempotency_key, payload)
                    .await
            }
            Self::Unavailable(problem) => Err(problem.clone()),
        }
    }

    /// Stops the local browser process while retaining the lease for its final
    /// encrypted snapshot. `profile_stop` publishes the stopped runtime state
    /// before beginning that sync.
    pub async fn stop_browser_process_locally(
        &self,
        profile_id: &str,
        correlation_id: &str,
    ) -> Result<bool, Problem> {
        let Self::Production(state) = self else {
            return Ok(false);
        };
        let profile_id = parse_profile_id(profile_id, correlation_id)?;
        if !state
            .profile_runtime
            .running_profiles()
            .await?
            .iter()
            .any(|running| running == &profile_id)
        {
            return Ok(false);
        }
        state
            .profile_runtime
            .stop_for_sync(profile_id)
            .await
            .map_err(|mut error| {
                error.correlation_id = correlation_id.to_owned();
                error
            })?;
        Ok(true)
    }

    pub async fn active_browser_profiles(&self) -> Result<Vec<String>, Problem> {
        match self {
            Self::Local(state) => state.active_profiles().await,
            Self::Production(state) => Ok(state
                .profile_runtime
                .active_profiles()
                .await?
                .into_iter()
                .map(|profile_id| profile_id.as_str().to_owned())
                .collect()),
            Self::Unavailable(problem) => Err(problem.clone()),
        }
    }

    pub fn runtime_device_id(&self) -> Option<String> {
        match self {
            Self::Production(state) => Some(state.device_id.clone()),
            Self::Local(_) | Self::Unavailable(_) => None,
        }
    }

    pub fn list_proxies(&self) -> Result<Vec<ImportedProxy>, Problem> {
        match self {
            Self::Local(state) => state.list_proxies(),
            Self::Production(_) => Err(production_sync_not_ready()),
            Self::Unavailable(problem) => Err(problem.clone()),
        }
    }

    pub fn import_proxies(
        &self,
        rows: Vec<ProxyRow>,
        name_prefix: Option<&str>,
    ) -> Result<ProxyImportReport, Problem> {
        match self {
            Self::Local(state) => state.import_proxies(rows, name_prefix),
            Self::Production(_) => Err(production_sync_not_ready()),
            Self::Unavailable(problem) => Err(problem.clone()),
        }
    }

    pub fn import_named_proxies(
        &self,
        rows: Vec<(ProxyRow, String)>,
    ) -> Result<ProxyImportReport, Problem> {
        match self {
            Self::Local(state) => state.import_named_proxies(rows),
            Self::Production(_) => Err(production_sync_not_ready()),
            Self::Unavailable(problem) => Err(problem.clone()),
        }
    }

    pub async fn shutdown(&self) -> Result<(), Problem> {
        match self {
            Self::Local(state) => state.shutdown().await,
            Self::Production(state) => state.shutdown().await,
            Self::Unavailable(_) => Ok(()),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ConfirmedDeviceFingerprint {
    device_id: String,
    fingerprint: String,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ConfirmedKeyRotationRecipient {
    member_id: String,
    device_id: String,
    fingerprint: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProfileKeyRotationExecuteInput {
    profile_id: String,
    confirmed_recipients: Vec<ConfirmedKeyRotationRecipient>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProfileLaunchInput {
    profile_id: String,
    #[serde(default)]
    headless: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProfileStopInput {
    profile_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MigrationProfileDataImportInput {
    source_root: PathBuf,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacyPreparedSourceVersion {
    instance_id: String,
}

#[derive(Deserialize)]
struct LegacyProfileDataInventory {
    #[serde(default)]
    profiles: Vec<LegacyProfileDataInventoryRecord>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacyProfileDataInventoryRecord {
    id: String,
    #[serde(default)]
    proxy_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProfileRecoveryCodeInput {
    profile_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProfileSyncRecoveryInput {
    profile_id: String,
    recovery_code: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OperationIdInput {
    operation_id: String,
}

fn parse_payload<T: for<'de> Deserialize<'de>>(
    payload: Value,
    correlation_id: &str,
) -> Result<T, Problem> {
    serde_json::from_value(payload).map_err(|_| {
        Problem::new(
            "invalid_command_input",
            "Invalid command input",
            400,
            correlation_id,
        )
    })
}

fn validate_key_rotation_confirmation(
    supplied: &[ConfirmedKeyRotationRecipient],
    recipients: &[buglogin_api::services::profile_hosted::ProfileKeyRotationRecipientDevices],
    correlation_id: &str,
) -> Result<(), Problem> {
    const MAX_CONFIRMED_DEVICES: usize = 16_384;
    if supplied.len() > MAX_CONFIRMED_DEVICES {
        return Err(invalid_input("confirmedRecipients", correlation_id));
    }
    let mut confirmed = BTreeSet::new();
    for item in supplied {
        if item.fingerprint.len() != 64
            || !item
                .fingerprint
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
            || !confirmed.insert((
                item.member_id.as_str(),
                item.device_id.as_str(),
                item.fingerprint.to_ascii_lowercase(),
            ))
        {
            return Err(invalid_input("confirmedRecipients", correlation_id));
        }
    }
    let current = recipients
        .iter()
        .flat_map(|member| {
            member.device_keys.iter().map(move |device| {
                (
                    member.member_id.as_str(),
                    device.device_id.as_str(),
                    device.fingerprint.to_ascii_lowercase(),
                )
            })
        })
        .collect::<BTreeSet<_>>();
    if confirmed != current {
        return Err(Problem::new(
            "profile_key_rotation_snapshot_stale",
            "Recipient devices changed; review the current fingerprints and confirm again",
            409,
            correlation_id,
        ));
    }
    Ok(())
}

fn parse_profile_id(value: &str, correlation_id: &str) -> Result<ProfileId, Problem> {
    ProfileId::new(value).map_err(|_| {
        Problem::new(
            "invalid_profile_id",
            "Invalid profile ID",
            400,
            correlation_id,
        )
    })
}

fn validate_idempotency_key<'a>(
    key: Option<&'a str>,
    correlation_id: &str,
) -> Result<&'a str, Problem> {
    key.filter(|value| {
        (16..=128).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_graphic())
    })
    .ok_or_else(|| {
        Problem::new(
            "idempotency_key_required",
            "Valid idempotency key required",
            400,
            correlation_id,
        )
    })
}

fn payload_hash(payload: &Value) -> Result<String, Problem> {
    struct Sha256Writer(Sha256);

    impl std::io::Write for Sha256Writer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.update(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let mut writer = Sha256Writer(Sha256::new());
    serde_json::to_writer(&mut writer, payload).map_err(|_| serialization("desktop-production"))?;
    Ok(format!("{:x}", writer.0.finalize()))
}

fn encode_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(DIGITS[(byte >> 4) as usize] as char);
        encoded.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    encoded
}

fn parse_device_public_key(
    value: &str,
    correlation_id: &str,
) -> Result<buglogin_sync::crypto::DevicePublicKey, Problem> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(Problem::new(
            "recipient_device_key_invalid",
            "Recipient device public key is invalid",
            409,
            correlation_id,
        ));
    }
    let mut bytes = [0_u8; 32];
    for (index, byte) in bytes.iter_mut().enumerate() {
        let offset = index * 2;
        *byte = u8::from_str_radix(&value[offset..offset + 2], 16).map_err(|_| {
            Problem::new(
                "recipient_device_key_invalid",
                "Recipient device public key is invalid",
                409,
                correlation_id,
            )
        })?;
    }
    Ok(buglogin_sync::crypto::DevicePublicKey::from_bytes(bytes))
}

fn operation_id(command_id: &str, idempotency_key: &str) -> String {
    let digest = format!(
        "{:x}",
        Sha256::digest(format!("{command_id}:{idempotency_key}").as_bytes())
    );
    format!("op_{}", &digest[..24])
}

fn manifest_has_profile_data(manifest: &buglogin_sync::manifest::SyncManifest) -> bool {
    manifest.payload_encryption.is_some()
        && (manifest
            .file_chunks
            .iter()
            .any(|file| !file.path.starts_with(".buglogin/"))
            || manifest
                .objects
                .iter()
                .any(|object| !object.key.starts_with(".buglogin/")))
}

fn read_migration_source_file(
    source_root: &PathBuf,
    name: &str,
    max_bytes: u64,
    correlation_id: &str,
) -> Result<Vec<u8>, Problem> {
    let missing_code = if name == "version.json" {
        "migration_source_identity_missing"
    } else {
        "migration_control_export_missing"
    };
    let invalid_code = if name == "version.json" {
        "migration_source_identity_invalid"
    } else {
        "migration_control_export_invalid"
    };
    let path = source_root.join(name);
    let metadata = fs::symlink_metadata(&path).map_err(|_| {
        Problem::new(
            missing_code,
            "Required prepared legacy source metadata is missing",
            422,
            correlation_id,
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > max_bytes {
        return Err(Problem::new(
            invalid_code,
            "Prepared legacy source metadata is invalid",
            422,
            correlation_id,
        ));
    }
    let bytes = fs::read(path).map_err(|_| {
        Problem::new(
            missing_code,
            "Required prepared legacy source metadata is unavailable",
            422,
            correlation_id,
        )
    })?;
    if bytes.len() as u64 != metadata.len() {
        return Err(Problem::new(
            "migration_source_changed",
            "Prepared legacy source metadata changed while it was being read",
            409,
            correlation_id,
        ));
    }
    Ok(bytes)
}

fn lease_idempotency_key(idempotency_key: &str, generation: u64) -> String {
    let digest = format!(
        "{:x}",
        Sha256::digest(format!("{idempotency_key}:{generation}").as_bytes())
    );
    format!("lease_{}", &digest[..32])
}

fn blob_idempotency_key(idempotency_key: &str, path: &str, checksum: &str) -> String {
    let digest = format!(
        "{:x}",
        Sha256::digest(format!("{idempotency_key}:{path}:{checksum}").as_bytes())
    );
    format!("blob_{}", &digest[..32])
}

fn runtime_projection_idempotency_key(idempotency_key: &str, transition: &str) -> String {
    let digest = format!(
        "{:x}",
        Sha256::digest(format!("{idempotency_key}:{transition}").as_bytes())
    );
    format!("runtime_{}", &digest[..32])
}

/// Retryable runtime dispatch failures are retried on later inbox polls with a
/// fresh local operation key, then reported as failed after this many attempts.
const RUNTIME_DISPATCH_MAX_ATTEMPTS: u32 = 5;
const RUNTIME_DISPATCH_POLL_INTERVAL: Duration = Duration::from_secs(5);

fn runtime_dispatch_operation_key(job_id: &str, attempt: u32) -> String {
    let digest = format!(
        "{:x}",
        Sha256::digest(format!("{job_id}|{attempt}").as_bytes())
    );
    format!("runtime_dispatch_{}", &digest[..32])
}

fn stable_runtime_failure_code(code: &str) -> String {
    let normalized = code
        .bytes()
        .map(|byte| {
            if byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_' {
                byte as char
            } else {
                '_'
            }
        })
        .take(96)
        .collect::<String>();
    if normalized.is_empty() {
        "runtime_operation_failed".to_owned()
    } else {
        normalized
    }
}

fn shutdown_idempotency_key(profile_id: &ProfileId, session: u64) -> String {
    let digest = format!(
        "{:x}",
        Sha256::digest(format!("{}|{session}", profile_id.as_str()).as_bytes())
    );
    format!("shutdown_{}", &digest[..32])
}

fn launch_state_allows_start(state: &ProfileState, archived: bool, locally_running: bool) -> bool {
    if archived || locally_running {
        return false;
    }

    // The profile row is a cloud projection and may lag behind a crashed or
    // closed desktop process. The local process registry and the acquired
    // resource lease are authoritative for whether this device may launch.
    matches!(state, ProfileState::Ready | ProfileState::Running)
}

fn result(command_id: &str, correlation_id: &str, data: Value) -> CommandResult<Value> {
    CommandResult {
        command_id: command_id.to_owned(),
        correlation_id: correlation_id.to_owned(),
        data,
    }
}

fn local_proxy_parse(
    payload: Value,
    correlation_id: &str,
) -> Result<CommandResult<Value>, Problem> {
    let input: ProxyParseInput =
        serde_json::from_value(payload).map_err(|_| invalid_input("text", correlation_id))?;
    if input.text.len() > 1_048_576 {
        return Err(Problem::new(
            "proxy_input_too_large",
            "Proxy input exceeds the maximum size",
            413,
            correlation_id,
        ));
    }
    let report = parse_lines(&input.text);
    let mut rows = report
        .rows
        .iter()
        .map(|row| {
            json!({
                "protocol": row.protocol,
                "endpoint": format!("{}:{}", row.host, row.port),
                "valid": true
            })
        })
        .collect::<Vec<_>>();
    rows.extend(
        report
            .errors
            .iter()
            .map(|error| json!({ "line": error.line, "valid": false, "error": error.code })),
    );
    Ok(result(
        "proxy.parse",
        correlation_id,
        json!({
            "validCount": report.rows.len(),
            "invalidCount": report.errors.len(),
            "duplicateCount": report.duplicates,
            "rows": rows
        }),
    ))
}

fn unavailable(code: &'static str, correlation_id: &str) -> Problem {
    Problem::new(
        code,
        "Desktop production service unavailable",
        503,
        correlation_id,
    )
}

fn serialization(correlation_id: &str) -> Problem {
    Problem::new(
        "serialization_failed",
        "Response serialization failed",
        500,
        correlation_id,
    )
}

fn invalid_input(field: &'static str, correlation_id: &str) -> Problem {
    Problem::new(
        "invalid_command_input",
        "Invalid command input",
        400,
        correlation_id,
    )
    .with_field("field", field)
}

fn production_sync_not_ready() -> Problem {
    Problem::new(
        "production_data_sync_not_ready",
        "Production data sync is not ready",
        503,
        "desktop-production",
    )
    .retryable(true)
}

#[cfg(test)]
mod tests {
    use super::{
        ConfirmedKeyRotationRecipient, DesktopState, ProxyCredentialInput, ProxyImportInput,
        WorkspaceSelectionStore, launch_state_allows_start, local_proxy_parse,
        production_sync_not_ready, production_transport, runtime_dispatch_operation_key,
        shutdown_idempotency_key, validate_key_rotation_confirmation,
    };
    use buglogin_api::services::{
        device_key_directory::DeviceEncryptionKey,
        profile_hosted::ProfileKeyRotationRecipientDevices,
    };
    use buglogin_domain::entities::ProfileState;
    use buglogin_domain::errors::Problem;
    use buglogin_domain::ids::{AccountId, MemberId};
    use serde_json::json;
    use sha2::Digest;

    #[test]
    fn stale_running_projection_does_not_block_local_start_but_local_process_does() {
        assert!(launch_state_allows_start(
            &ProfileState::Running,
            false,
            false
        ));
        assert!(!launch_state_allows_start(
            &ProfileState::Running,
            false,
            true
        ));
        assert!(!launch_state_allows_start(
            &ProfileState::Ready,
            true,
            false
        ));
        assert!(!launch_state_allows_start(
            &ProfileState::Launching,
            false,
            false
        ));
        assert!(!launch_state_allows_start(
            &ProfileState::Stopping,
            false,
            false
        ));
        assert!(!launch_state_allows_start(
            &ProfileState::Failed,
            false,
            false
        ));
    }

    #[test]
    fn streaming_payload_hash_matches_the_existing_json_digest() {
        let payload = json!({"credential":{"username":"proxy-user","password":"secret"}});
        let expected = format!(
            "{:x}",
            sha2::Sha256::digest(serde_json::to_vec(&payload).unwrap())
        );
        assert_eq!(super::payload_hash(&payload).unwrap(), expected);
    }

    #[test]
    fn workspace_selection_restores_independently_for_each_account() {
        let mut selections = WorkspaceSelectionStore::default();
        selections
            .accounts
            .insert("acct_a".into(), "ws_a_last".into());

        assert_eq!(
            selections.resolve_for_account("acct_a", &["ws_a_first".into(), "ws_a_last".into()]),
            Some("ws_a_last".into())
        );
        assert_eq!(
            selections.resolve_for_account("acct_b", &["ws_b_zeta".into(), "ws_b_alpha".into()]),
            Some("ws_b_alpha".into())
        );
        assert_eq!(
            selections.resolve_for_account("acct_a", &["ws_a_first".into(), "ws_a_last".into()]),
            Some("ws_a_last".into())
        );
    }

    #[test]
    fn workspace_selection_persists_across_restart_and_revalidates_authority() {
        use std::{fs, time::SystemTime};

        let unique = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "buglogin-workspace-selection-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("workspace-selection.json");

        let mut selections = WorkspaceSelectionStore::default();
        assert_eq!(
            selections.resolve_for_account("acct_a", &["ws_a_first".into(), "ws_a_last".into()]),
            Some("ws_a_first".into())
        );
        assert_eq!(
            selections.resolve_for_account("acct_b", &["ws_b_last".into()]),
            Some("ws_b_last".into())
        );
        selections
            .accounts
            .insert("acct_a".into(), "ws_a_last".into());
        selections.persist(&path).unwrap();

        let mut reopened = WorkspaceSelectionStore::load(&path);
        assert_eq!(
            reopened.resolve_for_account("acct_a", &["ws_a_first".into(), "ws_a_last".into()]),
            Some("ws_a_last".into())
        );
        assert_eq!(
            reopened.resolve_for_account("acct_b", &["ws_b_last".into()]),
            Some("ws_b_last".into())
        );
        assert_eq!(
            reopened.resolve_for_account("acct_a", &["ws_a_authorized".into()]),
            Some("ws_a_authorized".into()),
            "a remembered workspace outside the current authorized set must not be selected"
        );
        assert_eq!(
            reopened.accounts.get("acct_a").map(String::as_str),
            Some("ws_a_authorized")
        );
        reopened.persist(&path).unwrap();
        let after_authority_change = WorkspaceSelectionStore::load(&path);
        assert_eq!(
            after_authority_change
                .accounts
                .get("acct_a")
                .map(String::as_str),
            Some("ws_a_authorized")
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_workspace_selection_migrates_only_when_authorized() {
        let mut authorized = WorkspaceSelectionStore::from_contents("ws_legacy");
        assert_eq!(
            authorized.resolve_for_account("acct_a", &["ws_legacy".into(), "ws_other".into()]),
            Some("ws_legacy".into())
        );
        assert_eq!(
            authorized.accounts.get("acct_a").map(String::as_str),
            Some("ws_legacy")
        );
        assert_eq!(authorized.legacy_workspace_id, None);

        let mut unauthorized = WorkspaceSelectionStore::from_contents("ws_revoked");
        assert_eq!(
            unauthorized.resolve_for_account("acct_b", &["ws_b_zeta".into(), "ws_b_alpha".into()]),
            Some("ws_b_alpha".into())
        );
        assert_eq!(
            unauthorized.accounts.get("acct_b").map(String::as_str),
            Some("ws_b_alpha")
        );
        assert_eq!(unauthorized.legacy_workspace_id, None);
    }

    #[test]
    fn sensitive_journal_bytes_keep_the_existing_json_shape() {
        let value = serde_json::to_value(super::SensitiveBytes::new(vec![1, 2, 3])).unwrap();
        assert_eq!(value, json!([1, 2, 3]));
    }

    #[test]
    fn proxy_import_secret_buffers_are_zeroized() {
        let mut input = ProxyImportInput {
            workspace_id: "ws_test".to_owned(),
            text: "proxy.example:8080:user:password-secret".to_owned(),
            created_at: "2026-10-17T00:00:00Z".to_owned(),
        };
        input.zeroize_text();
        assert!(input.text.is_empty());

        let mut credential = ProxyCredentialInput {
            username: "user-secret".to_owned(),
            password: "password-secret".to_owned(),
        };
        credential.zeroize_credentials();
        assert!(credential.username.is_empty());
        assert!(credential.password.is_empty());

        let mut payload = json!({
            "credential": {"username": "payload-user", "password": "payload-secret"},
            "nested": ["another-secret"]
        });
        super::zeroize_json_strings(&mut payload);
        assert_eq!(
            payload,
            json!({
                "credential": {"username": "", "password": ""},
                "nested": [""]
            })
        );
    }

    #[test]
    fn refresh_credentials_are_preserved_only_for_retryable_non_auth_failures() {
        let transient =
            Problem::new("api_unavailable", "API unavailable", 503, "test").retryable(true);
        assert!(super::preserve_refresh_credentials(&transient));

        let permanent = Problem::new("api_unavailable", "API unavailable", 503, "test");
        assert!(!super::preserve_refresh_credentials(&permanent));

        let rejected =
            Problem::new("session_revoked", "Session revoked", 401, "test").retryable(true);
        assert!(!super::preserve_refresh_credentials(&rejected));
        assert!(super::definitive_auth_error(&rejected));
        assert!(!super::definitive_auth_error(&transient));
    }

    #[tokio::test]
    async fn unavailable_state_fails_closed_without_local_authority() {
        let state = DesktopState::unavailable(Problem::new(
            "runtime_not_configured",
            "Wayfern runtime is not configured",
            503,
            "desktop-bootstrap",
        ));
        let error = state
            .invoke("system_health_get", "correlation-1", None, json!({}))
            .await
            .expect_err("unavailable desktop must not execute commands");
        assert_eq!(error.code, "runtime_not_configured");
        assert!(state.list_proxies().is_err());
        state
            .shutdown()
            .await
            .expect("unavailable shutdown is safe");
    }

    #[test]
    fn production_transport_allows_desktop_login_without_token() {
        let (_, auth) = production_transport("http://127.0.0.1:1", None).unwrap();
        assert!(auth.is_none());
    }

    #[test]
    fn production_proxy_sync_fails_closed_without_an_exact_remote_contract() {
        let error = production_sync_not_ready();
        assert_eq!(error.code, "production_data_sync_not_ready");
        assert_eq!(error.status, 503);
        assert!(error.retryable);
        assert_eq!(error.correlation_id, "desktop-production");
    }

    #[test]
    fn production_proxy_parse_stays_local_and_redacts_credentials() {
        let result = local_proxy_parse(
            json!({
                "text": "http://proxy-user:proxy-secret@127.0.0.1:8080\ninvalid"
            }),
            "cor_proxy_parse_local",
        )
        .unwrap();
        let serialized = serde_json::to_string(&result).unwrap();
        assert_eq!(result.data["validCount"], 1);
        assert_eq!(result.data["invalidCount"], 1);
        assert_eq!(result.data["rows"][0]["endpoint"], "127.0.0.1:8080");
        assert!(!serialized.contains("proxy-user"));
        assert!(!serialized.contains("proxy-secret"));
    }

    #[test]
    fn key_rotation_confirmation_requires_the_exact_member_device_fingerprint_set() {
        let fingerprint = "ab".repeat(32);
        let recipients = vec![ProfileKeyRotationRecipientDevices {
            member_id: MemberId::new("mem_rotation_recipient").unwrap(),
            device_keys: vec![DeviceEncryptionKey {
                account_id: AccountId::new("acct_rotation_recipient").unwrap(),
                device_id: "device-1".to_owned(),
                schema_version: 1,
                public_key: "01".repeat(32),
                fingerprint: fingerprint.clone(),
                registered_at: 1,
            }],
        }];
        let confirmed = vec![ConfirmedKeyRotationRecipient {
            member_id: "mem_rotation_recipient".to_owned(),
            device_id: "device-1".to_owned(),
            fingerprint: fingerprint.clone(),
        }];
        assert!(validate_key_rotation_confirmation(&confirmed, &recipients, "test").is_ok());

        let stale = vec![ConfirmedKeyRotationRecipient {
            fingerprint: "cd".repeat(32),
            ..confirmed[0].clone()
        }];
        assert_eq!(
            validate_key_rotation_confirmation(&stale, &recipients, "test")
                .unwrap_err()
                .code,
            "profile_key_rotation_snapshot_stale"
        );

        let duplicate = vec![confirmed[0].clone(), confirmed[0].clone()];
        assert_eq!(
            validate_key_rotation_confirmation(&duplicate, &recipients, "test")
                .unwrap_err()
                .code,
            "invalid_command_input"
        );
    }

    #[test]
    fn shutdown_idempotency_key_is_unique_per_browser_session() {
        let profile = buglogin_domain::ids::ProfileId::new("prf_shutdown_key").unwrap();
        assert_eq!(
            shutdown_idempotency_key(&profile, 7),
            shutdown_idempotency_key(&profile, 7)
        );
        assert_ne!(
            shutdown_idempotency_key(&profile, 7),
            shutdown_idempotency_key(&profile, 8)
        );
    }

    #[tokio::test]
    async fn profile_operations_are_serialized_per_profile() {
        let locks = super::ProfileOperationLocks::default();
        let first = buglogin_domain::ids::ProfileId::new("prf_serial_a").unwrap();
        let second = buglogin_domain::ids::ProfileId::new("prf_serial_b").unwrap();

        let held = locks.lock_for(&first).lock_owned().await;
        assert!(locks.lock_for(&first).try_lock_owned().is_err());
        assert!(locks.lock_for(&second).try_lock_owned().is_ok());
        assert!(locks.is_busy(&first));
        drop(held);
        assert!(!locks.is_busy(&first));
        assert!(locks.lock_for(&first).try_lock_owned().is_ok());
    }

    #[test]
    fn exit_reconcile_retries_back_off_and_reset() {
        let mut backoff = super::ExitReconcileBackoff::default();
        let profile = buglogin_domain::ids::ProfileId::new("prf_backoff").unwrap();
        let start = std::time::Instant::now();

        assert!(backoff.ready(&profile, start));
        backoff.record_attempt(&profile, start);
        assert!(!backoff.ready(&profile, start + std::time::Duration::from_secs(2)));
        assert!(backoff.ready(&profile, start + std::time::Duration::from_secs(3)));
        backoff.record_attempt(&profile, start + std::time::Duration::from_secs(3));
        assert!(!backoff.ready(&profile, start + std::time::Duration::from_secs(8)));
        assert!(backoff.ready(&profile, start + std::time::Duration::from_secs(9)));
        for attempt in 0..20 {
            backoff.record_attempt(&profile, start + std::time::Duration::from_secs(attempt));
        }
        assert!(backoff.ready(&profile, start + std::time::Duration::from_secs(19 + 300)));

        backoff.retain_exited(&[]);
        assert!(backoff.ready(&profile, start));
    }

    #[test]
    fn runtime_dispatch_retries_use_a_fresh_operation_key() {
        assert_eq!(
            runtime_dispatch_operation_key("runtime_job", 1),
            runtime_dispatch_operation_key("runtime_job", 1)
        );
        assert_ne!(
            runtime_dispatch_operation_key("runtime_job", 1),
            runtime_dispatch_operation_key("runtime_job", 2)
        );
    }
}
