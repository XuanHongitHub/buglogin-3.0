use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
    path::PathBuf,
    sync::{Arc, Mutex, RwLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use buglogin_api::{
    commands::CommandResult,
    services::operations::{Operation, OperationService, OperationStatus},
};
use buglogin_browser::process::SystemProcessController;
use buglogin_domain::{
    entities::{ProfileState, SyncMode},
    errors::Problem,
    ids::{GrantId, ProfileId, ProxyId, WorkspaceId},
    permissions::{Action, Capability, Effect, Grant, Principal, ResourceScope},
    profile::ProfileArchiveState,
};
use buglogin_network::proxy::ProxyRow;
use buglogin_storage::secrets::SecretVersion;
use buglogin_sync::coordination::{CoordinationAuth, CoordinationBackend, CoordinationScope};
use futures_util::{StreamExt, TryStreamExt, stream};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::Notify;

use crate::{
    adapter::{AdapterState, RuntimeTargets, push_or_replace_arg},
    auth_vault::AuthVault,
    profile_projection::{HydratedProxyCredential, ProfileProjection},
    profile_runtime::{
        LaunchRequest, LeaseScope, ManagedCoordination, ProfileRuntimeHandle, spawn_profile_runtime,
    },
    proxy_bridge::{ProxyBridgeHandle, ProxyBridgeSpec},
    proxy_credential_vault::ProxyCredentialVault,
    proxy_store::{ImportedProxy, ProxyImportReport},
    remote_api::{DeviceAuthorizationStart, RemoteApiClient},
    sync_key_vault::SyncKeyVault,
};

pub enum DesktopState {
    Local(AdapterState),
    Production(ProductionDesktopState),
    Unavailable(Problem),
}

#[derive(Clone)]
pub struct ProductionDesktopState {
    client: RemoteApiClient,
    auth: Arc<RwLock<Option<CoordinationAuth>>>,
    device_id: String,
    pending_authorization: Arc<Mutex<Option<PendingDeviceAuthorization>>>,
    auth_vault: Arc<Mutex<AuthVault>>,
    auth_vault_version: Arc<Mutex<Option<SecretVersion>>>,
    projection: ProfileProjection,
    sync_key_vault: Arc<SyncKeyVault>,
    proxy_credential_vault: Arc<ProxyCredentialVault>,
    profile_runtime: ProfileRuntimeHandle,
    runtime_targets: RuntimeTargets,
    operations: Arc<RwLock<OperationService>>,
    idempotency: Arc<Mutex<BTreeMap<String, IdempotencyRecord>>>,
    operation_gate: Arc<Mutex<OperationGate>>,
    operation_drained: Arc<Notify>,
}

struct PendingDeviceAuthorization {
    device_code: String,
    user_code: String,
    verification_uri: String,
    expires_at: u64,
    interval: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProxyCredentialInput {
    username: String,
    password: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum DesktopAuthStatus {
    AuthNotApplicable,
    Unauthenticated,
    Authorizing {
        user_code: String,
        verification_uri: String,
        expires_at: u64,
        interval: u64,
    },
    Authenticated,
}

#[derive(Clone)]
struct IdempotencyRecord {
    command_id: &'static str,
    payload_hash: String,
    accepted: Value,
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
        let auth_vault = AuthVault::native();
        let stored = auth_vault.load_bearer(&device_id, "desktop-bootstrap")?;
        let auth_vault_version = stored.as_ref().map(|value| value.version());
        let token = token.or_else(|| stored.map(|value| value.bearer().to_owned()));
        let (client, auth) = production_transport(api_base_url, token, device_id.clone())?;
        let approved = runtime_targets.approved_executables()?;
        if approved.is_empty() {
            return Err(Problem::new(
                "runtime_not_configured",
                "At least one desktop runtime is required",
                503,
                "desktop-bootstrap",
            ));
        }
        let coordination = auth.clone().map(|auth| ManagedCoordination {
            backend: Arc::new(client.clone()),
            auth,
            lease_ttl: Duration::from_secs(120),
            heartbeat_interval: Duration::from_secs(30),
        });
        let profile_runtime =
            spawn_profile_runtime(SystemProcessController::new(approved), coordination);
        Ok(Self {
            client,
            auth: Arc::new(RwLock::new(auth)),
            device_id,
            pending_authorization: Arc::new(Mutex::new(None)),
            auth_vault: Arc::new(Mutex::new(auth_vault)),
            auth_vault_version: Arc::new(Mutex::new(auth_vault_version)),
            projection: ProfileProjection::open(profiles_root)?,
            sync_key_vault: Arc::new(SyncKeyVault::native()),
            proxy_credential_vault: Arc::new(ProxyCredentialVault::native()),
            profile_runtime,
            runtime_targets,
            operations: Arc::new(RwLock::new(OperationService::default())),
            idempotency: Arc::new(Mutex::new(BTreeMap::new())),
            operation_gate: Arc::new(Mutex::new(OperationGate::default())),
            operation_drained: Arc::new(Notify::new()),
        })
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

    pub fn auth_status(&self) -> Result<DesktopAuthStatus, Problem> {
        if self.client.has_token()? {
            return Ok(DesktopAuthStatus::Authenticated);
        }
        let pending = self
            .pending_authorization
            .lock()
            .map_err(|_| desktop_state_problem("desktop-auth"))?;
        Ok(pending
            .as_ref()
            .map(|value| DesktopAuthStatus::Authorizing {
                user_code: value.user_code.clone(),
                verification_uri: value.verification_uri.clone(),
                expires_at: value.expires_at,
                interval: value.interval,
            })
            .unwrap_or(DesktopAuthStatus::Unauthenticated))
    }

    pub async fn start_device_authorization(
        &self,
        correlation_id: &str,
    ) -> Result<DesktopAuthStatus, Problem> {
        if self.client.has_token()? {
            return Ok(DesktopAuthStatus::Authenticated);
        }
        let started: DeviceAuthorizationStart = self
            .client
            .device_authorization_start(&self.device_id, "BugLogin Desktop", correlation_id)
            .await?;
        let expires_at = now_seconds().saturating_add(started.expires_in);
        *self
            .pending_authorization
            .lock()
            .map_err(|_| desktop_state_problem(correlation_id))? =
            Some(PendingDeviceAuthorization {
                device_code: started.device_code,
                user_code: started.user_code,
                verification_uri: started.verification_uri,
                expires_at,
                interval: started.interval,
            });
        self.auth_status()
    }

    pub async fn poll_device_authorization(
        &self,
        correlation_id: &str,
    ) -> Result<DesktopAuthStatus, Problem> {
        let pending = self
            .pending_authorization
            .lock()
            .map_err(|_| desktop_state_problem(correlation_id))?
            .as_ref()
            .map(|value| {
                (
                    value.device_code.clone(),
                    value.user_code.clone(),
                    value.verification_uri.clone(),
                    value.expires_at,
                    value.interval,
                )
            })
            .ok_or_else(|| {
                Problem::new(
                    "device_authorization_not_started",
                    "Device authorization has not started",
                    409,
                    correlation_id,
                )
            })?;
        let token = match self
            .client
            .device_authorization_token(&pending.0, correlation_id)
            .await
        {
            Ok(token) => token,
            Err(error) if matches!(error.code.as_str(), "authorization_pending" | "slow_down") => {
                return self.auth_status();
            }
            Err(error) => {
                if matches!(
                    error.code.as_str(),
                    "expired_token" | "invalid_grant" | "invalid_user_code"
                ) {
                    *self
                        .pending_authorization
                        .lock()
                        .map_err(|_| desktop_state_problem(correlation_id))? = None;
                }
                return Err(error);
            }
        };
        let version = self.persist_bearer(&token.access_token, correlation_id)?;
        self.client.set_token(token.access_token.clone())?;
        let auth = CoordinationAuth::new(self.device_id.clone(), token.access_token.clone())?;
        let coordination = ManagedCoordination {
            backend: Arc::new(self.client.clone()),
            auth: auth.clone(),
            lease_ttl: Duration::from_secs(120),
            heartbeat_interval: Duration::from_secs(30),
        };
        if let Err(error) = self
            .profile_runtime
            .set_coordination(Some(coordination))
            .await
        {
            let _ = self.client.clear_token();
            let _ = self.remove_persisted_bearer(version, correlation_id);
            return Err(error);
        }
        *self
            .auth
            .write()
            .map_err(|_| desktop_state_problem(correlation_id))? = Some(auth);
        *self
            .pending_authorization
            .lock()
            .map_err(|_| desktop_state_problem(correlation_id))? = None;
        self.auth_status()
    }

    pub async fn logout(&self, correlation_id: &str) -> Result<DesktopAuthStatus, Problem> {
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
        *self
            .pending_authorization
            .lock()
            .map_err(|_| desktop_state_problem(correlation_id))? = None;
        self.auth_status()
    }

    fn persist_bearer(&self, bearer: &str, correlation_id: &str) -> Result<SecretVersion, Problem> {
        let mut vault = self
            .auth_vault
            .lock()
            .map_err(|_| desktop_state_problem(correlation_id))?;
        let mut version = self
            .auth_vault_version
            .lock()
            .map_err(|_| desktop_state_problem(correlation_id))?;
        let next = match *version {
            Some(current) => {
                vault.replace_bearer(&self.device_id, current, bearer, correlation_id)?
            }
            None => vault.store_bearer(&self.device_id, bearer, correlation_id)?,
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

    async fn invoke(
        &self,
        invoke_name: &str,
        correlation_id: &str,
        idempotency_key: Option<&str>,
        payload: Value,
    ) -> Result<CommandResult<Value>, Problem> {
        match invoke_name {
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
                let profile_id = parse_profile_id(&input.id, correlation_id)?;
                let action_correlation = correlation_id.to_owned();
                let sync_idempotency_key = key.to_owned();
                let data = self
                    .run_operation(
                        "profile.stop",
                        key,
                        &hash,
                        correlation_id,
                        move |state| async move {
                            state
                                .stop_profile(profile_id, sync_idempotency_key, action_correlation)
                                .await
                        },
                    )
                    .await?;
                Ok(result("profile.stop", correlation_id, data))
            }
            "operation_get" => self.operation_get(payload, correlation_id),
            _ => {
                self.client
                    .invoke(invoke_name, correlation_id, idempotency_key, payload)
                    .await
            }
        }
    }

    async fn invoke_proxy_mutation(
        &self,
        invoke_name: &str,
        correlation_id: &str,
        idempotency_key: Option<&str>,
        mut payload: Value,
    ) -> Result<CommandResult<Value>, Problem> {
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
                let encoded = serde_json::to_vec(&json!({
                    "username": input.username,
                    "password": input.password
                }))
                .map_err(|_| serialization(correlation_id))?;
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
        if let Some(secret) = secret.as_ref() {
            self.proxy_credential_vault.store(
                &workspace_id,
                &proxy_id,
                0,
                secret.clone(),
                correlation_id,
            )?;
        }
        let result = self
            .client
            .invoke(invoke_name, correlation_id, idempotency_key, payload)
            .await;
        match result {
            Ok(result) => {
                if invoke_name == "proxy_delete" || clear_credential {
                    if let Some(revision) = previous_revision {
                        self.proxy_credential_vault.remove(
                            &workspace_id,
                            &proxy_id,
                            revision,
                            correlation_id,
                        )?;
                    }
                } else {
                    let revision = if invoke_name == "proxy_create" {
                        result
                            .data
                            .pointer("/record/revision")
                            .and_then(Value::as_u64)
                    } else {
                        result.data.get("revision").and_then(Value::as_u64)
                    }
                    .ok_or_else(|| {
                        Problem::new(
                            "proxy_revision_missing",
                            "BugLogin API did not return the updated proxy revision",
                            502,
                            correlation_id,
                        )
                    })?;
                    if let Some(secret) = secret.as_ref().or(previous.as_ref()) {
                        self.proxy_credential_vault.store(
                            &workspace_id,
                            &proxy_id,
                            revision,
                            secret.clone(),
                            correlation_id,
                        )?;
                    }
                    if let Some(previous_revision) = previous_revision {
                        if previous_revision != revision {
                            self.proxy_credential_vault.remove(
                                &workspace_id,
                                &proxy_id,
                                previous_revision,
                                correlation_id,
                            )?;
                        }
                    }
                }
                if secret.is_some() {
                    self.proxy_credential_vault.remove(
                        &workspace_id,
                        &proxy_id,
                        0,
                        correlation_id,
                    )?;
                }
                Ok(result)
            }
            Err(error) => {
                if secret.is_some() {
                    self.proxy_credential_vault.remove(
                        &workspace_id,
                        &proxy_id,
                        0,
                        correlation_id,
                    )?;
                }
                Err(error)
            }
        }
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
        self.sync_key_vault.store_recovered(
            &binding.workspace_id,
            &binding.resource_id,
            client,
            recovery,
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
        let profile = self.client.profile(&profile_id, &correlation_id).await?;
        if profile.value.archived || profile.value.state != ProfileState::Ready {
            return Err(Problem::new(
                "profile_not_launchable",
                "Profile cannot be launched",
                409,
                correlation_id,
            ));
        }
        if !self.runtime_targets.supports(&profile.value.runtime) {
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
        let proxy_launch = match profile.value.proxy_id.as_ref() {
            Some(proxy_id) => Some(
                self.client
                    .proxy_for_launch(&binding.workspace_id, proxy_id, &correlation_id)
                    .await?,
            ),
            None => None,
        };
        let scope = CoordinationScope {
            workspace_id: &binding.workspace_id,
            auth: &auth,
        };
        let mut manifest = self
            .client
            .current_manifest(scope.clone(), &binding.resource_id)
            .await?
            .ok_or_else(|| {
                Problem::new(
                    "production_manifest_not_found",
                    "Production profile manifest not found",
                    409,
                    &correlation_id,
                )
            })?;
        let workspace_keys = if manifest.payload_encryption.is_some() {
            Some(
                self.sync_key_vault
                    .load(&binding.workspace_id, &binding.resource_id, &correlation_id)?
                    .ok_or_else(|| {
                        Problem::new(
                            "sync_key_recovery_required",
                            "Enter this profile's recovery code on this device to open it",
                            409,
                            &correlation_id,
                        )
                    })?,
            )
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
                    Duration::from_secs(120),
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
            let mut specification = match self.runtime_targets.launch_specification(
                &profile.value,
                &projection.path,
                headless,
            ) {
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
            let runtime_key = runtime_projection_idempotency_key(&idempotency_key, "started");
            let lease_for_projection = lease.clone();
            let receipt = self
                .profile_runtime
                .launch(LaunchRequest {
                    profile_id: profile_id.clone(),
                    idempotency_key: idempotency_key.clone(),
                    profile_path: projection.path,
                    specification,
                    lease_scope: Some(LeaseScope {
                        workspace_id: binding.workspace_id.clone(),
                        resource_id: binding.resource_id.clone(),
                    }),
                    preacquired_lease: Some(lease),
                    proxy_bridge,
                })
                .await
                .map_err(|mut error| {
                    error.correlation_id = correlation_id.clone();
                    error
                })?;
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

    async fn stop_profile(
        &self,
        profile_id: ProfileId,
        idempotency_key: String,
        correlation_id: String,
    ) -> Result<Value, Problem> {
        let auth = self.auth(&correlation_id)?;
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
        let stopped = self
            .profile_runtime
            .stop_for_sync(profile_id.clone())
            .await
            .map_err(|mut error| {
                error.correlation_id = correlation_id.clone();
                error
            })?;
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
        let scope = CoordinationScope {
            workspace_id: &binding.workspace_id,
            auth: &auth,
        };
        let profile = self.client.profile(&profile_id, &correlation_id).await?;
        let proxy_snapshot = match profile.value.proxy_id.as_ref() {
            Some(proxy_id) => {
                let proxy_info = self
                    .client
                    .proxy_for_launch(&binding.workspace_id, proxy_id, &correlation_id)
                    .await?;
                let credential = self.proxy_credential_vault.load(
                    &binding.workspace_id,
                    proxy_id,
                    proxy_info.revision,
                    &correlation_id,
                )?;
                Some(
                    self.client
                        .proxy_for_sync(
                            &binding.workspace_id,
                            proxy_id,
                            proxy_info.revision,
                            credential,
                            &correlation_id,
                        )
                        .await?,
                )
            }
            None => None,
        };
        let previous = self
            .client
            .current_manifest(scope.clone(), &binding.resource_id)
            .await?
            .ok_or_else(|| {
                Problem::new(
                    "production_manifest_not_found",
                    "Production profile manifest not found",
                    409,
                    &correlation_id,
                )
            })?;
        let workspace_keys = self.sync_key_vault.load_or_create(
            &binding.workspace_id,
            &binding.resource_id,
            &correlation_id,
        )?;
        let (descriptor, files, manifest) = self.projection.snapshot_with_proxy_encryption(
            &stopped.profile_path,
            &previous,
            lease.fencing_token,
            proxy_snapshot,
            &workspace_keys.client,
            &workspace_keys.recovery,
        )?;
        let object_map: BTreeMap<_, _> = descriptor
            .objects
            .iter()
            .map(|object| (object.path.as_str(), object))
            .collect();
        let reused_object_keys: BTreeSet<_> = manifest
            .objects
            .iter()
            .filter(|object| object.source_generation.is_some())
            .map(|object| object.key.as_str())
            .collect();
        let client = self.client.clone();
        let resource_id = &binding.resource_id;
        let generation = manifest.generation;
        let owner_token = lease.owner_token.clone();
        let fencing_token = lease.fencing_token;
        let correlation_id = correlation_id.clone();
        stream::iter(files.into_iter().map(|(path, bytes)| {
            let client = client.clone();
            let scope = scope.clone();
            let owner_token = owner_token.clone();
            let correlation_id = correlation_id.clone();
            let object_map = &object_map;
            let reused_object_keys = &reused_object_keys;
            let idempotency_key = &idempotency_key;
            async move {
                if reused_object_keys.contains(path.as_str()) {
                    return Ok(());
                }
                let object = object_map.get(path.as_str()).ok_or_else(|| {
                    Problem::new(
                        "profile_snapshot_incomplete",
                        "Profile snapshot is incomplete",
                        409,
                        &correlation_id,
                    )
                })?;
                if client
                    .blob_present(
                        scope.clone(),
                        resource_id,
                        &path,
                        generation,
                        &object.checksum,
                        &owner_token,
                        fencing_token,
                        &correlation_id,
                    )
                    .await?
                {
                    return Ok(());
                }
                client
                    .upload_blob(
                        scope,
                        resource_id,
                        &path,
                        generation,
                        &object.checksum,
                        &owner_token,
                        fencing_token,
                        &blob_idempotency_key(&idempotency_key, &path, &object.checksum),
                        bytes,
                        &correlation_id,
                    )
                    .await
            }
        }))
        .buffer_unordered(4)
        .try_collect::<Vec<()>>()
        .await?;
        self.client
            .commit_manifest(
                scope,
                &binding.resource_id,
                &lease.owner_token,
                manifest.clone(),
            )
            .await?;
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
        self.profile_runtime
            .complete_stop(profile_id.clone())
            .await
            .map_err(|mut error| {
                error.correlation_id = correlation_id;
                error
            })?;
        Ok(json!({
            "profileId": profile_id,
            "generation": manifest.generation,
            "rootChecksum": manifest.root_checksum
        }))
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
                    accepted: accepted.clone(),
                },
            );
        }
        let state = self.clone();
        let worker_id = operation_id.clone();
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
            if let Ok(mut operations) = state.operations.write() {
                let _ = match outcome {
                    Ok(value) => operations.transition(
                        &worker_id,
                        OperationStatus::Completed,
                        Some(1.0),
                        Some(value),
                        None,
                    ),
                    Err(error) => operations.transition(
                        &worker_id,
                        OperationStatus::Failed,
                        Some(1.0),
                        None,
                        Some(error),
                    ),
                };
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
            let idempotency_key = shutdown_idempotency_key(&profile_id);
            if let Err(error) = self
                .stop_profile(
                    profile_id,
                    idempotency_key,
                    "desktop-production-shutdown".to_owned(),
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
    device_id: String,
) -> Result<(RemoteApiClient, Option<CoordinationAuth>), Problem> {
    if let Some(token) = token {
        let client = RemoteApiClient::new(api_base_url, token.clone())?;
        let auth = CoordinationAuth::new(device_id, token)?;
        Ok((client, Some(auth)))
    } else {
        Ok((RemoteApiClient::new_unauthenticated(api_base_url)?, None))
    }
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
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

impl DesktopState {
    pub fn local(state: AdapterState) -> Self {
        Self::Local(state)
    }

    pub fn unavailable(problem: Problem) -> Self {
        Self::Unavailable(problem)
    }

    pub fn auth_status(&self) -> Result<DesktopAuthStatus, Problem> {
        match self {
            Self::Local(_) => Ok(DesktopAuthStatus::AuthNotApplicable),
            Self::Production(state) => state.auth_status(),
            Self::Unavailable(problem) => Err(problem.clone()),
        }
    }

    pub async fn start_device_authorization(
        &self,
        correlation_id: &str,
    ) -> Result<DesktopAuthStatus, Problem> {
        match self {
            Self::Local(_) => Err(auth_not_applicable(correlation_id)),
            Self::Production(state) => state.start_device_authorization(correlation_id).await,
            Self::Unavailable(problem) => Err(problem.clone()),
        }
    }

    pub async fn poll_device_authorization(
        &self,
        correlation_id: &str,
    ) -> Result<DesktopAuthStatus, Problem> {
        match self {
            Self::Local(_) => Err(auth_not_applicable(correlation_id)),
            Self::Production(state) => state.poll_device_authorization(correlation_id).await,
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
struct ProfileLaunchInput {
    profile_id: String,
    #[serde(default)]
    headless: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileStopInput {
    id: String,
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
    serde_json::to_vec(payload)
        .map(|bytes| format!("{:x}", Sha256::digest(bytes)))
        .map_err(|_| serialization("desktop-production"))
}

fn operation_id(command_id: &str, idempotency_key: &str) -> String {
    let digest = format!(
        "{:x}",
        Sha256::digest(format!("{command_id}:{idempotency_key}").as_bytes())
    );
    format!("op_{}", &digest[..24])
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

fn shutdown_idempotency_key(profile_id: &ProfileId) -> String {
    let digest = format!("{:x}", Sha256::digest(profile_id.as_str().as_bytes()));
    format!("shutdown_{}", &digest[..32])
}

fn result(command_id: &str, correlation_id: &str, data: Value) -> CommandResult<Value> {
    CommandResult {
        command_id: command_id.to_owned(),
        correlation_id: correlation_id.to_owned(),
        data,
    }
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
    use super::{DesktopState, production_sync_not_ready, production_transport};
    use buglogin_domain::errors::Problem;
    use serde_json::json;

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
    fn production_transport_allows_device_authorization_without_token() {
        let (_, auth) =
            production_transport("http://127.0.0.1:1", None, "desktop-1".to_owned()).unwrap();
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
}
