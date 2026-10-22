use std::{
    collections::BTreeMap,
    fs,
    future::Future,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicU64, Ordering},
    },
};

use buglogin_api::{
    commands::{Adapter, CommandKind, CommandRegistry, CommandResult},
    services::{
        operations::{Operation, OperationService, OperationStatus},
        profile_creation::ProfileCreationService,
    },
};
use buglogin_browser::{
    cookies::{
        CookieInput, CookieSameSite, CookieScope,
        authority::{
            CookieAuthority, CookieStageReceipt, IdempotencyKey, ProfileRuntimeState,
            StagedImportId,
        },
        wayfern::ProfileCookieState,
    },
    process::{ApprovedExecutable, ProcessController, SystemProcessController},
    profile::{ProfileBindingService, ProfileService},
    runtime::{RuntimeAdapter, RuntimeBinary, bugmini::BugminiAdapter, wayfern::WayfernAdapter},
};
use buglogin_domain::{
    entities::{
        ExtensionGroup, Profile, ProfileCreateInput, ProfileRecord, ProfileState, Proxy,
        ProxyProtocol, Runtime, SyncMode,
    },
    errors::Problem,
    ids::{GrantId, ProfileId, ProxyId, ResourceId, WorkspaceId},
    lease::Lease,
    permissions::{Action, Capability, Effect, Grant, Principal, ResourceScope},
    profile::ProfileArchiveState,
};
use buglogin_network::proxy::parse_lines;
use buglogin_storage::{
    local::{LocalRepository, Versioned},
    secrets::SecretValue,
    vault::NativeVault,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::clock::{unix_time_millis, unix_time_nanos, unix_time_seconds};
use crate::migration_commands::{
    MigrationCommandRunner, MigrationImportInput, MigrationInspectInput, migration_import,
    migration_inspect,
};
use crate::profile_runtime::{
    LaunchRequest, LeaseScope, ManagedCoordination, ProfileRuntimeHandle, spawn_profile_runtime,
};
use crate::proxy_bridge::{ProxyBridgeHandle, check_proxy_connectivity};
use crate::proxy_store::{DesktopProxyStore, ImportedProxy, ProxyImportReport};
use crate::wayfern_fingerprint::{
    apply_or_generate as apply_wayfern_fingerprint, clear_stale_cdp_marker,
    configure_local_cdp_args, target_from_runtime_config,
};

pub const INVOKE_NAMESPACE: &str = "buglogin";
const SUPPORTED_COMMANDS: &[&str] = &[
    "system.health.get",
    "organization.list",
    "organization.get",
    "organization.create",
    "organization.update",
    "workspace.list",
    "workspace.get",
    "workspace.create",
    "workspace.update",
    "operation.get",
    "migration.inspect",
    "migration.import",
    "profile.list",
    "extension.group.list",
    "extension.group.get",
    "extension.group.create",
    "extension.group.update",
    "extension.group.delete",
    "profile.extension.group.set",
    "profile.binding.get",
    "profile.proxy.assignment.get",
    "group.share.list",
    "group.share.create",
    "group.share.revoke",
    "member.list",
    "member.get",
    "member.invite",
    "member.invitation.create",
    "member.invitation.redeem",
    "member.invitation.revoke",
    "member.activate",
    "member.suspend",
    "member.role.update",
    "team.list",
    "team.get",
    "team.create",
    "team.update",
    "resource.list",
    "resource.get",
    "resource.register",
    "grant.list.page",
    "profile.create",
    "profile.archive",
    "profile.restore",
    "profile.launch",
    "profile.stop",
    "cookie.list",
    "cookie.stage",
    "cookie.import",
    "cookie.copy",
    "proxy.parse",
    "proxy.import",
    "proxy.check",
    "proxy.list",
    "proxy.get",
    "proxy.create",
    "proxy.update",
    "proxy.delete",
];
static PROFILE_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdapterCommandMetadata {
    pub invoke: String,
    pub command_id: &'static str,
    pub kind: CommandKind,
    pub capability: &'static str,
    pub audit_required: bool,
    pub auth_required: bool,
    pub idempotency_required: bool,
    pub timeout_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Health {
    pub status: &'static str,
    pub service: &'static str,
    pub version: &'static str,
}

#[derive(Clone, Debug)]
pub struct RuntimeTarget {
    pub binary: RuntimeBinary,
    pub extra_args: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct RuntimeTargets {
    pub wayfern: Option<RuntimeTarget>,
    pub bugmini: Option<RuntimeTarget>,
}

impl RuntimeTargets {
    pub fn wayfern(binary: RuntimeBinary, extra_args: Vec<String>) -> Self {
        Self {
            wayfern: Some(RuntimeTarget { binary, extra_args }),
            bugmini: None,
        }
    }

    pub fn supports(&self, runtime: &Runtime) -> bool {
        match runtime {
            Runtime::Wayfern => self.wayfern.is_some(),
            Runtime::Bugmini => self.bugmini.is_some(),
            _ => false,
        }
    }

    pub(crate) fn approved_executables(&self) -> Result<Vec<ApprovedExecutable>, Problem> {
        self.wayfern
            .iter()
            .chain(self.bugmini.iter())
            .map(|target| ApprovedExecutable::new(&target.binary.path))
            .collect()
    }

    pub fn profile_path(
        &self,
        runtime: &Runtime,
        root: &PathBuf,
        profile_id: &ProfileId,
    ) -> Result<PathBuf, Problem> {
        match runtime {
            Runtime::Wayfern if self.wayfern.is_some() => {
                WayfernAdapter.profile_path(root, profile_id)
            }
            Runtime::Bugmini if self.bugmini.is_some() => {
                BugminiAdapter.profile_path(root, profile_id)
            }
            _ => Err(runtime_not_configured()),
        }
    }

    pub fn launch_specification(
        &self,
        profile: &Profile,
        profile_path: &PathBuf,
        headless: bool,
    ) -> Result<buglogin_browser::runtime::LaunchSpecification, Problem> {
        let mut specification = match &profile.runtime {
            Runtime::Wayfern => launch_with_adapter(
                WayfernAdapter,
                self.wayfern.as_ref().ok_or_else(runtime_not_configured)?,
                profile_path,
                headless,
            )?,
            Runtime::Bugmini => launch_with_adapter(
                BugminiAdapter,
                self.bugmini.as_ref().ok_or_else(runtime_not_configured)?,
                profile_path,
                headless,
            )?,
            _ => return Err(runtime_not_configured()),
        };
        if profile.runtime == Runtime::Wayfern {
            append_wayfern_profile_chrome_args(
                &mut specification.args,
                profile.name.as_str(),
                profile.id.as_str(),
            );
        }
        let fingerprint_target =
            target_from_runtime_config(profile.runtime_config.as_ref(), "runtime")?;
        if fingerprint_target.is_some() {
            if profile.runtime != Runtime::Wayfern {
                return Err(Problem::new(
                    "fingerprint_runtime_unsupported",
                    "Fingerprint targets are supported only by the Wayfern runtime",
                    409,
                    "runtime",
                ));
            }
        }
        if profile.runtime == Runtime::Bugmini {
            append_bugmini_profile_args(&mut specification.args, profile);
        }
        Ok(specification)
    }
}

fn launch_with_adapter<A: RuntimeAdapter>(
    adapter: A,
    target: &RuntimeTarget,
    profile_path: &PathBuf,
    headless: bool,
) -> Result<buglogin_browser::runtime::LaunchSpecification, Problem> {
    let validated = adapter.validate_binary(&target.binary)?;
    let mut extra_args = target.extra_args.clone();
    if headless {
        extra_args.push("--headless".to_owned());
    }
    adapter.launch_specification(&validated, profile_path, &extra_args)
}

fn append_bugmini_profile_args(args: &mut Vec<String>, profile: &Profile) {
    let digest = Sha256::digest(profile.id.as_str().as_bytes());
    let seed = u32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]]) % 90_000 + 10_000;
    push_or_replace_arg(args, format!("--fingerprint={seed}"));
    push_or_replace_arg(
        args,
        format!(
            "--fingerprint-platform={}",
            if cfg!(target_os = "macos") {
                "macos"
            } else {
                "windows"
            }
        ),
    );
    push_or_replace_arg(args, "--fingerprint-brand=Chrome".to_owned());
    if let Some(version) = profile.runtime_version.as_deref() {
        push_or_replace_arg(args, format!("--fingerprint-brand-version={version}"));
    }
    if cfg!(target_os = "windows") {
        push_or_replace_arg(args, "--ignore-gpu-blocklist".to_owned());
    }
    if let Some(fingerprint) = profile.fingerprint.as_ref() {
        if let Some(timezone) = fingerprint.get("timezone").and_then(Value::as_str) {
            push_or_replace_arg(args, format!("--fingerprint-timezone={timezone}"));
        }
        let locale = fingerprint
            .get("language")
            .and_then(Value::as_str)
            .or_else(|| {
                fingerprint
                    .get("languages")
                    .and_then(Value::as_array)
                    .and_then(|values| values.first())
                    .and_then(Value::as_str)
            });
        if let Some(locale) = locale.filter(|value| !value.trim().is_empty()) {
            let locale = locale.replace('_', "-");
            push_or_replace_arg(args, format!("--lang={locale}"));
            push_or_replace_arg(args, format!("--fingerprint-locale={locale}"));
        }
        if let Some(user_agent) = fingerprint.get("userAgent").and_then(Value::as_str) {
            push_or_replace_arg(args, format!("--user-agent={user_agent}"));
        }
    }
    let payload = profile
        .runtime_config
        .as_ref()
        .and_then(|config| config.get("payload"));
    if payload
        .and_then(|value| value.get("block_images"))
        .and_then(Value::as_bool)
        == Some(true)
    {
        push_or_replace_arg(args, "--blink-settings=imagesEnabled=false".to_owned());
    }
    if payload
        .and_then(|value| value.get("block_webgl"))
        .and_then(Value::as_bool)
        == Some(true)
    {
        push_or_replace_arg(args, "--disable-webgl".to_owned());
    }
    if payload
        .and_then(|value| value.get("block_webrtc"))
        .and_then(Value::as_bool)
        == Some(true)
    {
        push_or_replace_arg(
            args,
            "--force-webrtc-ip-handling-policy=disable_non_proxied_udp".to_owned(),
        );
    }
}

fn append_wayfern_profile_chrome_args(
    args: &mut Vec<String>,
    profile_name: &str,
    profile_id: &str,
) {
    if !profile_name.trim().is_empty() {
        push_or_replace_arg(
            args,
            format!("--wayfern-profile-label={}", profile_name.trim()),
        );
    }
    let mut hash = 2_166_136_261_u32;
    for byte in profile_id.as_bytes() {
        hash = (hash ^ u32::from(*byte)).wrapping_mul(16_777_619);
    }
    let (red, green, blue) = hsl_to_rgb(f64::from(hash % 360), 0.6, 0.8);
    push_or_replace_arg(
        args,
        format!("--wayfern-profile-color={red:02x}{green:02x}{blue:02x}"),
    );
}

fn hsl_to_rgb(hue: f64, saturation: f64, lightness: f64) -> (u8, u8, u8) {
    let chroma = (1.0 - (2.0 * lightness - 1.0).abs()) * saturation;
    let hue_sector = hue / 60.0;
    let secondary = chroma * (1.0 - (hue_sector % 2.0 - 1.0).abs());
    let (red, green, blue) = match hue_sector as u32 {
        0 => (chroma, secondary, 0.0),
        1 => (secondary, chroma, 0.0),
        2 => (0.0, chroma, secondary),
        3 => (0.0, secondary, chroma),
        4 => (secondary, 0.0, chroma),
        _ => (chroma, 0.0, secondary),
    };
    let offset = lightness - chroma / 2.0;
    let channel = |value: f64| ((value + offset) * 255.0).round().clamp(0.0, 255.0) as u8;
    (channel(red), channel(green), channel(blue))
}

pub(crate) fn push_or_replace_arg(args: &mut Vec<String>, replacement: String) {
    let key = replacement
        .split_once('=')
        .map(|(key, _)| key)
        .unwrap_or(replacement.as_str());
    if let Some(existing) = args.iter_mut().find(|value| {
        value
            .split_once('=')
            .map(|(existing, _)| existing == key)
            .unwrap_or(value.as_str() == key)
    }) {
        *existing = replacement;
    } else {
        args.push(replacement);
    }
}

fn runtime_not_configured() -> Problem {
    Problem::new(
        "runtime_not_configured",
        "Profile runtime is not configured",
        503,
        "desktop-runtime",
    )
}

#[derive(Clone, Debug)]
pub struct RuntimeConfiguration {
    pub repository_root: PathBuf,
    pub profiles_root: PathBuf,
    pub targets: RuntimeTargets,
    pub migration_executable: Option<PathBuf>,
}

#[derive(Clone, Debug)]
struct IdempotencyRecord {
    command_id: &'static str,
    payload_hash: String,
    data: Value,
}

#[derive(Clone)]
pub struct AdapterState {
    profiles: ProfileService,
    bindings: ProfileBindingService,
    operations: Arc<RwLock<OperationService>>,
    profile_runtime: ProfileRuntimeHandle,
    runtime: RuntimeConfiguration,
    idempotency: Arc<Mutex<BTreeMap<String, IdempotencyRecord>>>,
    proxy_store: DesktopProxyStore,
    cookies: Arc<Mutex<CookieAuthority<NativeVault>>>,
}

impl AdapterState {
    pub fn profiles_root(&self) -> &Path {
        &self.runtime.profiles_root
    }
    pub fn open(
        repository_root: impl Into<PathBuf>,
        profiles_root: impl Into<PathBuf>,
        binary: RuntimeBinary,
        extra_args: Vec<String>,
    ) -> Result<Self, Problem> {
        Self::open_with_targets(
            repository_root,
            profiles_root,
            RuntimeTargets::wayfern(binary, extra_args),
        )
    }

    pub fn open_with_targets(
        repository_root: impl Into<PathBuf>,
        profiles_root: impl Into<PathBuf>,
        targets: RuntimeTargets,
    ) -> Result<Self, Problem> {
        let approved = targets.approved_executables()?;
        if approved.is_empty() {
            return Err(runtime_not_configured());
        }
        let repository_root = repository_root.into();
        let repository = LocalRepository::open(&repository_root)?;
        Self::from_parts(
            ProfileService::new(repository.clone()),
            ProfileBindingService::new(repository),
            RuntimeConfiguration {
                repository_root,
                profiles_root: profiles_root.into(),
                targets,
                migration_executable: std::env::var_os("BUGLOGIN_MIGRATION_BINARY")
                    .map(PathBuf::from),
            },
            SystemProcessController::new(approved),
        )
    }

    pub fn open_with_targets_and_migration(
        repository_root: impl Into<PathBuf>,
        profiles_root: impl Into<PathBuf>,
        targets: RuntimeTargets,
        migration_executable: Option<PathBuf>,
    ) -> Result<Self, Problem> {
        let approved = targets.approved_executables()?;
        if approved.is_empty() {
            return Err(runtime_not_configured());
        }
        let repository_root = repository_root.into();
        let repository = LocalRepository::open(&repository_root)?;
        Self::from_parts(
            ProfileService::new(repository.clone()),
            ProfileBindingService::new(repository),
            RuntimeConfiguration {
                repository_root,
                profiles_root: profiles_root.into(),
                targets,
                migration_executable,
            },
            SystemProcessController::new(approved),
        )
    }
}

impl AdapterState {
    pub fn from_parts<C>(
        profiles: ProfileService,
        bindings: ProfileBindingService,
        runtime: RuntimeConfiguration,
        controller: C,
    ) -> Result<Self, Problem>
    where
        C: ProcessController + 'static,
    {
        Self::from_parts_with_coordination(profiles, bindings, runtime, controller, None)
    }

    pub fn from_parts_with_coordination<C>(
        profiles: ProfileService,
        bindings: ProfileBindingService,
        runtime: RuntimeConfiguration,
        controller: C,
        coordination: Option<ManagedCoordination>,
    ) -> Result<Self, Problem>
    where
        C: ProcessController + 'static,
    {
        fs::create_dir_all(&runtime.profiles_root).map_err(|_| {
            problem(
                "profiles_root_unavailable",
                "Profiles root unavailable",
                503,
                "desktop",
            )
        })?;
        let proxy_store =
            DesktopProxyStore::native(LocalRepository::open(&runtime.repository_root)?);
        let cookies =
            CookieAuthority::open(NativeVault::new(), runtime.repository_root.join("cookies"))
                .map_err(|error| {
                    problem(
                        "cookie_authority_unavailable",
                        &error.to_string(),
                        503,
                        "desktop",
                    )
                })?;
        Ok(Self {
            profiles,
            bindings,
            operations: Arc::new(RwLock::new(OperationService::default())),
            profile_runtime: spawn_profile_runtime(controller, coordination),
            runtime,
            idempotency: Arc::new(Mutex::new(BTreeMap::new())),
            proxy_store,
            cookies: Arc::new(Mutex::new(cookies)),
        })
    }

    pub async fn invoke(
        &self,
        invoke_name: &str,
        correlation_id: &str,
        idempotency_key: Option<&str>,
        payload: Value,
    ) -> Result<CommandResult<Value>, Problem> {
        validate_correlation_id(correlation_id)?;
        if matches!(
            invoke_name,
            "profile_clone" | "profile_share" | "profile_share_revoke"
        ) {
            return Err(Problem::new(
                "hosted_profile_authority_required",
                "Profile clone and sharing require hosted production authority",
                409,
                correlation_id,
            )
            .with_field("invoke", invoke_name));
        }
        if matches!(
            invoke_name,
            "account_list"
                | "account_status_update"
                | "organization_get"
                | "organization_record_get"
                | "organization_create"
                | "organization_update"
                | "workspace_list"
                | "workspace_get"
                | "workspace_record_get"
                | "workspace_create"
                | "workspace_update"
                | "team_list"
                | "team_get"
                | "team_create"
                | "team_update"
                | "resource_list"
                | "resource_get"
                | "resource_register"
                | "grant_list_page"
                | "group_list"
                | "group_get"
                | "group_create"
                | "group_update"
                | "group_delete"
                | "group_share_list"
                | "group_share_create"
                | "group_share_revoke"
                | "member_list"
                | "member_get"
                | "member_invite"
                | "member_invitation_create"
                | "member_invitation_redeem"
                | "member_invitation_revoke"
                | "member_activate"
                | "member_suspend"
                | "member_role_update"
                | "profile_binding_get"
                | "profile_proxy_assignment_get"
                | "proxy_import"
                | "profile_groups_replace"
                | "profile_proxy_assignment_set"
                | "profile_update"
        ) {
            return Err(Problem::new(
                "hosted_workspace_authority_required",
                "Workspace-scoped membership, profile, group, and proxy operations require hosted API authority",
                409,
                correlation_id,
            )
            .with_field("invoke", invoke_name));
        }
        let command = supported_command(invoke_name).ok_or_else(|| {
            problem(
                "command_not_found",
                "Command not found",
                404,
                correlation_id,
            )
            .with_field("invoke", invoke_name)
        })?;
        let payload_hash = payload_hash(&payload)?;
        if command.idempotency_required {
            let key = validate_idempotency_key(idempotency_key, correlation_id)?;
            if let Some(data) =
                self.replay(key, command.id.as_str(), &payload_hash, correlation_id)?
            {
                return Ok(result(command.id.as_str(), correlation_id, data));
            }
        }
        let data = match command.id.as_str() {
            "system.health.get" => self.health(payload, correlation_id),
            "organization.list" => self.organization_list(payload, correlation_id),
            "workspace.list" => self.workspace_list(payload, correlation_id),
            "operation.get" => self.operation_get(payload, correlation_id),
            "migration.inspect" => {
                let key = validate_idempotency_key(idempotency_key, correlation_id)?;
                let input: MigrationInspectInput = serde_json::from_value(payload.clone())
                    .map_err(|_| {
                        problem(
                            "invalid_input",
                            "Invalid migration inspection input",
                            400,
                            correlation_id,
                        )
                    })?;
                let executable = self.runtime.migration_executable.clone().ok_or_else(|| {
                    problem(
                        "migration_executable_missing",
                        "Migration executable is not configured",
                        503,
                        correlation_id,
                    )
                })?;
                let operation_correlation = correlation_id.to_owned();
                let worker_correlation = operation_correlation.clone();
                self.run_operation(
                    "migration.inspect",
                    key,
                    &payload_hash,
                    correlation_id,
                    move |_state| async move {
                        let process_correlation = worker_correlation.clone();
                        let error_correlation = worker_correlation.clone();
                        tokio::task::spawn_blocking(move || {
                            let runner = MigrationCommandRunner::new(executable)?;
                            let result = migration_inspect(&runner, input)?;
                            serde_json::to_value(result.data).map_err(|_| {
                                problem(
                                    "migration_result_invalid",
                                    "Migration result invalid",
                                    500,
                                    &process_correlation,
                                )
                            })
                        })
                        .await
                        .map_err(|_| {
                            problem(
                                "migration_worker_failed",
                                "Migration worker failed",
                                500,
                                &error_correlation,
                            )
                        })?
                    },
                )
                .await
            }
            "migration.import" => {
                let key = validate_idempotency_key(idempotency_key, correlation_id)?;
                let input: MigrationImportInput =
                    serde_json::from_value(payload.clone()).map_err(|_| {
                        problem(
                            "invalid_input",
                            "Invalid migration import input",
                            400,
                            correlation_id,
                        )
                    })?;
                let expected_destination = fs::canonicalize(&self.runtime.repository_root)
                    .map_err(|_| {
                        problem(
                            "migration_destination_invalid",
                            "Active local repository is unavailable",
                            503,
                            correlation_id,
                        )
                    })?;
                let requested_destination =
                    fs::canonicalize(&input.destination_root).map_err(|_| {
                        problem(
                            "migration_destination_invalid",
                            "Migration destination is unavailable",
                            400,
                            correlation_id,
                        )
                    })?;
                if requested_destination != expected_destination {
                    return Err(problem(
                        "migration_destination_mismatch",
                        "Migration destination must match the active local repository",
                        409,
                        correlation_id,
                    ));
                }
                let executable = self.runtime.migration_executable.clone().ok_or_else(|| {
                    problem(
                        "migration_executable_missing",
                        "Migration executable is not configured",
                        503,
                        correlation_id,
                    )
                })?;
                let worker_correlation = correlation_id.to_owned();
                self.run_operation(
                    "migration.import",
                    key,
                    &payload_hash,
                    correlation_id,
                    move |_state| async move {
                        let process_correlation = worker_correlation.clone();
                        let error_correlation = worker_correlation.clone();
                        tokio::task::spawn_blocking(move || {
                            let runner = MigrationCommandRunner::new(executable)?;
                            let result = migration_import(&runner, input)?;
                            serde_json::to_value(result.data).map_err(|_| {
                                problem(
                                    "migration_result_invalid",
                                    "Migration result invalid",
                                    500,
                                    &process_correlation,
                                )
                            })
                        })
                        .await
                        .map_err(|_| {
                            problem(
                                "migration_worker_failed",
                                "Migration worker failed",
                                500,
                                &error_correlation,
                            )
                        })?
                    },
                )
                .await
            }
            "profile.list" => self.profile_list(payload, correlation_id),
            "extension.group.list" => self.extension_group_list(payload, correlation_id),
            "extension.group.get" => self.extension_group_get(payload, correlation_id),
            "extension.group.create" => self.extension_group_create(payload, correlation_id),
            "extension.group.update" => self.extension_group_update(payload, correlation_id),
            "extension.group.delete" => self.extension_group_delete(payload, correlation_id),
            "profile.extension.group.set" => {
                self.profile_extension_group_set(payload, correlation_id)
            }
            "profile.create" => self.profile_create(payload, correlation_id),
            "profile.archive" => self.profile_archive_restore(payload, correlation_id, true),
            "profile.restore" => self.profile_archive_restore(payload, correlation_id, false),
            "profile.launch" => {
                self.profile_launch(payload, idempotency_key, &payload_hash, correlation_id)
                    .await
            }
            "profile.stop" => {
                self.profile_stop(payload, idempotency_key, &payload_hash, correlation_id)
                    .await
            }
            "cookie.list" => self.cookie_list(payload, correlation_id),
            "cookie.stage" => {
                let key = validate_idempotency_key(idempotency_key, correlation_id)?;
                self.cookie_stage(payload, key, correlation_id)
            }
            "cookie.import" => {
                let key = validate_idempotency_key(idempotency_key, correlation_id)?;
                self.cookie_import(payload, key, correlation_id)
            }
            "cookie.copy" => {
                let key = validate_idempotency_key(idempotency_key, correlation_id)?;
                self.cookie_copy(payload, key, correlation_id)
            }
            "proxy.parse" => self.proxy_parse(payload, correlation_id),
            "proxy.check" => {
                let input =
                    parse_payload::<ProxyCheckInput>(payload, "proxy.check", correlation_id)?;
                let proxy_id = parse_proxy_id(&input.proxy_id, correlation_id)?;
                parse_workspace_id(&input.workspace_id, correlation_id)?;
                let (spec, revision) = self.proxy_store.connectivity_check_spec(&proxy_id)?;
                let latency_ms = check_proxy_connectivity(spec).await?;
                Ok(json!({
                    "proxyId": proxy_id,
                    "revision": revision,
                    "status": "healthy",
                    "latencyMs": latency_ms
                }))
            }
            "proxy.list" => self.proxy_list(payload, correlation_id),
            "proxy.get" => self.proxy_get(payload, correlation_id),
            "proxy.create" => self.proxy_create(payload, correlation_id),
            "proxy.update" => self.proxy_update(payload, correlation_id),
            "proxy.delete" => self.proxy_delete(payload, correlation_id),
            _ => unreachable!(),
        }?;
        if command.idempotency_required {
            self.remember(
                idempotency_key.expect("validated idempotency key"),
                command.id.as_str(),
                payload_hash,
                data.clone(),
                correlation_id,
            )?;
        }
        Ok(result(command.id.as_str(), correlation_id, data))
    }

    pub fn import_proxies(
        &self,
        rows: Vec<buglogin_network::proxy::ProxyRow>,
        name_prefix: Option<&str>,
    ) -> Result<ProxyImportReport, Problem> {
        self.proxy_store.import(rows, name_prefix)
    }

    pub fn import_named_proxies(
        &self,
        rows: Vec<(buglogin_network::proxy::ProxyRow, String)>,
    ) -> Result<ProxyImportReport, Problem> {
        self.proxy_store.import_named(rows)
    }

    pub fn list_proxies(&self) -> Result<Vec<ImportedProxy>, Problem> {
        self.proxy_store.list()
    }
    pub async fn active_profiles(&self) -> Result<Vec<String>, Problem> {
        Ok(self
            .profile_runtime
            .active_profiles()
            .await?
            .into_iter()
            .map(|profile_id| profile_id.as_str().to_owned())
            .collect())
    }
    pub async fn shutdown(&self) -> Result<(), Problem> {
        self.profile_runtime.shutdown().await
    }

    fn health(&self, payload: Value, correlation_id: &str) -> Result<Value, Problem> {
        parse_payload::<Empty>(payload, "system.health.get", correlation_id)?;
        serde_json::to_value(Health {
            status: "ok",
            service: "buglogin-desktop",
            version: env!("CARGO_PKG_VERSION"),
        })
        .map_err(|_| serialization_problem(correlation_id))
    }

    fn organization_list(&self, payload: Value, correlation_id: &str) -> Result<Value, Problem> {
        parse_payload::<Empty>(payload, "organization.list", correlation_id)?;
        Err(Problem::new(
            "hosted_organization_authority_required",
            "Organization records require the hosted BugLogin API",
            409,
            correlation_id,
        ))
    }

    fn workspace_list(&self, payload: Value, correlation_id: &str) -> Result<Value, Problem> {
        parse_payload::<WorkspaceListInput>(payload, "workspace.list", correlation_id)?;
        Err(Problem::new(
            "hosted_workspace_authority_required",
            "Workspace records require the hosted BugLogin API",
            409,
            correlation_id,
        ))
    }

    fn profile_list(&self, payload: Value, correlation_id: &str) -> Result<Value, Problem> {
        let input = parse_payload::<ProfileListInput>(payload, "profile.list", correlation_id)?;
        let limit = input.limit.unwrap_or(100);
        if !(1..=100).contains(&limit) {
            return Err(problem(
                "invalid_page_limit",
                "Profile page limit must be between 1 and 100",
                400,
                correlation_id,
            ));
        }
        let cursor = input
            .cursor
            .as_deref()
            .map(ProfileId::from_str)
            .transpose()
            .map_err(|_| {
                problem(
                    "invalid_page_cursor",
                    "Invalid page cursor",
                    400,
                    correlation_id,
                )
            })?;
        let mut scan_cursor = cursor.map(|cursor| cursor.as_str().to_owned());
        let mut profiles = Vec::with_capacity(limit + 1);
        loop {
            let batch = self
                .profiles
                .list_after(scan_cursor.as_deref(), 100)
                .map_err(|error| correlate(error, correlation_id))?;
            let batch_len = batch.len();
            if batch_len == 0 {
                break;
            }
            let last_scanned = batch
                .last()
                .map(|profile| profile.value.id.as_str().to_owned());
            for profile in batch {
                if input.include_archived || !profile.value.archived {
                    profiles.push(profile);
                    if profiles.len() > limit {
                        break;
                    }
                }
            }
            if profiles.len() > limit || batch_len < 100 {
                break;
            }
            scan_cursor = last_scanned;
        }
        let has_more = profiles.len() > limit;
        profiles.truncate(limit);
        let next_cursor = has_more
            .then(|| {
                profiles
                    .last()
                    .map(|profile| profile.value.id.as_str().to_owned())
            })
            .flatten();
        let items = profiles
            .into_iter()
            .map(|profile| profile_list_item(profile, correlation_id))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(json!({ "items": items, "nextCursor": next_cursor }))
    }

    fn profile_create(&self, payload: Value, correlation_id: &str) -> Result<Value, Problem> {
        let input = parse_payload::<ProfileCreateInput>(payload, "profile.create", correlation_id)?;
        if input.name.trim().is_empty() {
            return Err(problem(
                "profile_name_required",
                "Profile name is required",
                400,
                correlation_id,
            ));
        }
        if !self.runtime.targets.supports(&input.runtime) {
            return Err(correlate(runtime_not_configured(), correlation_id));
        }
        let proxy_id = input
            .proxy_id
            .as_deref()
            .map(|value| parse_proxy_id(value, correlation_id))
            .transpose()?;
        if let Some(proxy_id) = proxy_id.as_ref() {
            self.proxy_repository(correlation_id)?
                .get::<Proxy>(proxy_id.as_str())
                .map_err(|error| correlate(error, correlation_id))?
                .ok_or_else(|| proxy_not_found(correlation_id))?;
        }
        let profile_id = match input.id.as_deref() {
            Some(id) => parse_profile_id(id, correlation_id)?,
            None => generated_profile_id(correlation_id)?,
        };
        let profile = ProfileCreationService::build(input, profile_id, false)
            .map_err(|error| correlate(error, correlation_id))?;
        let created = self
            .profiles
            .create(profile)
            .map_err(|error| correlate(error, correlation_id))?;
        serde_json::to_value(ProfileRecord::new(created.value, created.revision))
            .map_err(|_| serialization_problem(correlation_id))
    }

    fn profile_archive_restore(
        &self,
        payload: Value,
        correlation_id: &str,
        archive: bool,
    ) -> Result<Value, Problem> {
        let input = parse_payload::<ProfileRevisionInput>(
            payload,
            if archive {
                "profile.archive"
            } else {
                "profile.restore"
            },
            correlation_id,
        )?;
        let profile_id = parse_profile_id(&input.profile_id, correlation_id)?;
        let current = self
            .profiles
            .get(&profile_id)
            .map_err(|error| correlate(error, correlation_id))?
            .ok_or_else(|| {
                problem(
                    "profile_not_found",
                    "Profile not found",
                    404,
                    correlation_id,
                )
            })?;
        if archive && current.value.state == ProfileState::Running {
            return Err(problem(
                "profile_running",
                "Running profile must be stopped before archive",
                409,
                correlation_id,
            ));
        }
        let updated = if archive {
            self.profiles.archive(input.revision, current.value)
        } else {
            self.profiles.restore(input.revision, current.value)
        }
        .map_err(|error| correlate(error, correlation_id))?;
        serde_json::to_value(ProfileRecord::new(updated.value, updated.revision))
            .map_err(|_| serialization_problem(correlation_id))
    }

    fn cookie_list(&self, payload: Value, correlation_id: &str) -> Result<Value, Problem> {
        let input = parse_payload::<CookieListInput>(payload, "cookie.list", correlation_id)?;
        let workspace_id = WorkspaceId::from_str(&input.workspace_id).map_err(|_| {
            problem(
                "invalid_workspace_id",
                "Invalid workspace ID",
                400,
                correlation_id,
            )
        })?;
        let profile_id = parse_profile_id(&input.profile_id, correlation_id)?;
        let profile = self
            .profiles
            .get(&profile_id)
            .map_err(|error| correlate(error, correlation_id))?
            .ok_or_else(|| profile_not_found(correlation_id))?;
        let runtime_state = match profile.value.state {
            ProfileState::Running | ProfileState::Launching | ProfileState::Stopping => {
                ProfileRuntimeState::Running
            }
            _ => ProfileRuntimeState::Stopped,
        };
        let scope = CookieScope {
            workspace_id,
            profile_id,
        };
        let mut authority = self.cookies.lock().map_err(|_| {
            problem(
                "cookie_authority_unavailable",
                "Cookie authority unavailable",
                503,
                correlation_id,
            )
        })?;
        authority.set_profile_state(scope.clone(), runtime_state);
        self.reconcile_cookie_scope(&mut authority, &scope, correlation_id)?;
        let principal = Principal {
            roles: std::collections::BTreeSet::from([
                buglogin_domain::permissions::Role::LocalOwner,
            ]),
            ..Principal::default()
        };
        let result = authority
            .list(&scope, &principal, &[])
            .map_err(|error| error.as_problem(correlation_id))?;
        serde_json::to_value(result).map_err(|_| serialization_problem(correlation_id))
    }

    fn cookie_stage(
        &self,
        payload: Value,
        _idempotency_key: &str,
        correlation_id: &str,
    ) -> Result<Value, Problem> {
        let input = parse_payload::<CookieStageInput>(payload, "cookie.stage", correlation_id)?;
        let scope = cookie_scope(&input.workspace_id, &input.profile_id, correlation_id)?;
        let cookies = input
            .cookies
            .into_iter()
            .map(|cookie| cookie.into_cookie(correlation_id))
            .collect::<Result<Vec<_>, _>>()?;
        let mut authority = self.cookie_authority(correlation_id)?;
        self.set_cookie_profile_state(&mut authority, &scope, correlation_id)?;
        let stage_id = authority
            .stage_import(scope, cookies)
            .map_err(|error| error.as_problem(correlation_id))?;
        serde_json::to_value(CookieStageReceipt {
            stage_id: stage_id.as_str().to_owned(),
        })
        .map_err(|_| serialization_problem(correlation_id))
    }

    fn cookie_import(
        &self,
        payload: Value,
        idempotency_key: &str,
        correlation_id: &str,
    ) -> Result<Value, Problem> {
        let input = parse_payload::<CookieImportInput>(payload, "cookie.import", correlation_id)?;
        let stage_id = StagedImportId::from_opaque(input.stage_id)
            .map_err(|error| error.as_problem(correlation_id))?;
        let scope = cookie_scope(&input.workspace_id, &input.profile_id, correlation_id)?;
        let request_key = IdempotencyKey::new(idempotency_key)
            .map_err(|error| error.as_problem(correlation_id))?;
        let mut authority = self.cookie_authority(correlation_id)?;
        self.set_cookie_profile_state(&mut authority, &scope, correlation_id)?;
        if !authority.has_idempotency_receipt(&request_key) {
            self.preflight_cookie_scope(&scope, correlation_id)?;
        }
        let lease = local_cookie_lease(&scope, correlation_id)?;
        authority
            .set_current_lease(scope.clone(), lease.clone())
            .map_err(|error| error.as_problem(correlation_id))?;
        let result = authority
            .import(
                &stage_id,
                input.expected_generation,
                &lease,
                unix_time_millis(correlation_id)?,
                &local_principal(),
                &[],
                request_key,
            )
            .map_err(|error| error.as_problem(correlation_id))?;
        self.reconcile_cookie_scope(&mut authority, &scope, correlation_id)?;
        serde_json::to_value(result).map_err(|_| serialization_problem(correlation_id))
    }

    fn cookie_copy(
        &self,
        payload: Value,
        idempotency_key: &str,
        correlation_id: &str,
    ) -> Result<Value, Problem> {
        let input = parse_payload::<CookieCopyInput>(payload, "cookie.copy", correlation_id)?;
        let source = cookie_scope(
            &input.source_workspace_id,
            &input.source_profile_id,
            correlation_id,
        )?;
        let destination = cookie_scope(
            &input.destination_workspace_id,
            &input.destination_profile_id,
            correlation_id,
        )?;
        let request_key = IdempotencyKey::new(idempotency_key)
            .map_err(|error| error.as_problem(correlation_id))?;
        let mut authority = self.cookie_authority(correlation_id)?;
        self.set_cookie_profile_state(&mut authority, &source, correlation_id)?;
        self.set_cookie_profile_state(&mut authority, &destination, correlation_id)?;
        if !authority.has_idempotency_receipt(&request_key) {
            self.preflight_cookie_scope(&destination, correlation_id)?;
        }
        let lease = local_cookie_lease(&destination, correlation_id)?;
        authority
            .set_current_lease(destination.clone(), lease.clone())
            .map_err(|error| error.as_problem(correlation_id))?;
        let result = authority
            .copy(
                &source,
                destination.clone(),
                input.expected_generation,
                &lease,
                unix_time_millis(correlation_id)?,
                &local_principal(),
                &[],
                request_key,
            )
            .map_err(|error| error.as_problem(correlation_id))?;
        self.reconcile_cookie_scope(&mut authority, &destination, correlation_id)?;
        serde_json::to_value(result).map_err(|_| serialization_problem(correlation_id))
    }

    fn cookie_authority(
        &self,
        correlation_id: &str,
    ) -> Result<std::sync::MutexGuard<'_, CookieAuthority<NativeVault>>, Problem> {
        self.cookies.lock().map_err(|_| {
            problem(
                "cookie_authority_unavailable",
                "Cookie authority unavailable",
                503,
                correlation_id,
            )
        })
    }

    fn extension_group_repository(&self) -> Result<LocalRepository, Problem> {
        LocalRepository::open(&self.runtime.repository_root)
    }

    fn extension_group_list(&self, payload: Value, correlation_id: &str) -> Result<Value, Problem> {
        use buglogin_api::services::extension_groups::ExtensionGroupListInput;
        let input = parse_payload::<ExtensionGroupListInput>(
            payload,
            "extension.group.list",
            correlation_id,
        )?;
        let workspace_id = parse_workspace_id(input.workspace_id.as_str(), correlation_id)?;
        let repo = self.extension_group_repository()?;
        let rows = repo
            .list::<ExtensionGroup>()?
            .into_iter()
            .map(|row| {
                json!({
                    "group": row.value,
                    "workspaceId": workspace_id,
                    "revision": row.revision
                })
            })
            .collect::<Vec<_>>();
        Ok(json!(rows))
    }

    fn extension_group_get(&self, payload: Value, correlation_id: &str) -> Result<Value, Problem> {
        use buglogin_api::services::extension_groups::ExtensionGroupIdInput;
        let input =
            parse_payload::<ExtensionGroupIdInput>(payload, "extension.group.get", correlation_id)?;
        let workspace_id = parse_workspace_id(input.workspace_id.as_str(), correlation_id)?;
        let row = self
            .extension_group_repository()?
            .get::<ExtensionGroup>(input.extension_group_id.as_str())?
            .ok_or_else(|| {
                problem(
                    "extension_group_not_found",
                    "Extension group not found",
                    404,
                    correlation_id,
                )
            })?;
        Ok(json!({ "group": row.value, "workspaceId": workspace_id, "revision": row.revision }))
    }

    fn extension_group_create(
        &self,
        payload: Value,
        correlation_id: &str,
    ) -> Result<Value, Problem> {
        use buglogin_api::services::extension_groups::ExtensionGroupCreateInput;
        let input = parse_payload::<ExtensionGroupCreateInput>(
            payload,
            "extension.group.create",
            correlation_id,
        )?;
        let workspace_id = parse_workspace_id(input.workspace_id.as_str(), correlation_id)?;
        validate_extension_group_fields(&input.name, &input.extension_ids, correlation_id)?;
        let now = time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(|_| {
                problem(
                    "clock_unavailable",
                    "System clock unavailable",
                    503,
                    correlation_id,
                )
            })?;
        let group = ExtensionGroup {
            id: input.extension_group_id,
            name: input.name.trim().to_owned(),
            extension_ids: input.extension_ids,
            sync_mode: input.sync_mode,
            created_at: now.clone(),
            updated_at: now,
        };
        let row = self.extension_group_repository()?.create(group)?;
        Ok(json!({ "group": row.value, "workspaceId": workspace_id, "revision": row.revision }))
    }

    fn extension_group_update(
        &self,
        payload: Value,
        correlation_id: &str,
    ) -> Result<Value, Problem> {
        use buglogin_api::services::extension_groups::ExtensionGroupUpdateInput;
        let input = parse_payload::<ExtensionGroupUpdateInput>(
            payload,
            "extension.group.update",
            correlation_id,
        )?;
        let workspace_id = parse_workspace_id(input.workspace_id.as_str(), correlation_id)?;
        validate_extension_group_fields(&input.name, &input.extension_ids, correlation_id)?;
        let repo = self.extension_group_repository()?;
        let mut current = repo
            .get::<ExtensionGroup>(input.extension_group_id.as_str())?
            .ok_or_else(|| {
                problem(
                    "extension_group_not_found",
                    "Extension group not found",
                    404,
                    correlation_id,
                )
            })?;
        let now = time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(|_| {
                problem(
                    "clock_unavailable",
                    "System clock unavailable",
                    503,
                    correlation_id,
                )
            })?;
        current.value.name = input.name.trim().to_owned();
        current.value.extension_ids = input.extension_ids;
        current.value.sync_mode = input.sync_mode;
        current.value.updated_at = now;
        let updated = repo.update(input.expected_revision, current.value)?;
        Ok(
            json!({ "group": updated.value, "workspaceId": workspace_id, "revision": updated.revision }),
        )
    }

    fn extension_group_delete(
        &self,
        payload: Value,
        correlation_id: &str,
    ) -> Result<Value, Problem> {
        use buglogin_api::services::extension_groups::ExtensionGroupDeleteInput;
        let input = parse_payload::<ExtensionGroupDeleteInput>(
            payload,
            "extension.group.delete",
            correlation_id,
        )?;
        let workspace_id = parse_workspace_id(input.workspace_id.as_str(), correlation_id)?;
        let repo = self.extension_group_repository()?;
        let row = repo
            .get::<ExtensionGroup>(input.extension_group_id.as_str())?
            .ok_or_else(|| {
                problem(
                    "extension_group_not_found",
                    "Extension group not found",
                    404,
                    correlation_id,
                )
            })?;
        if repo.list::<Profile>()?.iter().any(|profile| {
            profile.value.extension_group_id.as_ref() == Some(&input.extension_group_id)
        }) {
            return Err(problem(
                "extension_group_in_use",
                "Extension group is assigned to profiles",
                409,
                correlation_id,
            ));
        }
        repo.quarantine::<ExtensionGroup>(
            input.expected_revision,
            input.extension_group_id.as_str(),
        )?;
        Ok(json!({ "group": row.value, "workspaceId": workspace_id, "revision": row.revision }))
    }

    fn profile_extension_group_set(
        &self,
        payload: Value,
        correlation_id: &str,
    ) -> Result<Value, Problem> {
        use buglogin_api::services::extension_groups::ProfileExtensionGroupSetInput;
        let input = parse_payload::<ProfileExtensionGroupSetInput>(
            payload,
            "profile.extension.group.set",
            correlation_id,
        )?;
        let workspace_id = parse_workspace_id(input.workspace_id.as_str(), correlation_id)?;
        let repo = self.extension_group_repository()?;
        if let Some(group_id) = &input.extension_group_id {
            repo.get::<ExtensionGroup>(group_id.as_str())?
                .ok_or_else(|| {
                    problem(
                        "extension_group_not_found",
                        "Extension group not found",
                        404,
                        correlation_id,
                    )
                })?;
        }
        let mut profile = self.profiles.get(&input.profile_id)?.ok_or_else(|| {
            problem(
                "profile_not_found",
                "Profile not found",
                404,
                correlation_id,
            )
        })?;
        profile.value.extension_group_id = input.extension_group_id;
        let updated = self
            .profiles
            .update(input.expected_revision, profile.value)?;
        let _ = workspace_id;
        serde_json::to_value(ProfileRecord::new(updated.value, updated.revision))
            .map_err(|_| serialization_problem(correlation_id))
    }

    fn reconcile_profile_cookie_materializations(
        &self,
        profile_id: &ProfileId,
        correlation_id: &str,
    ) -> Result<(), Problem> {
        let mut authority = self.cookie_authority(correlation_id)?;
        let pending = authority
            .pending_materializations()
            .map_err(|error| error.as_problem(correlation_id))?
            .into_iter()
            .filter(|(scope, _)| scope.profile_id == *profile_id)
            .collect::<Vec<_>>();
        if pending.len() > 1 {
            return Err(cookie_recovery_pending(correlation_id));
        }
        for (scope, _) in pending {
            self.reconcile_cookie_scope(&mut authority, &scope, correlation_id)?;
        }
        Ok(())
    }

    fn reconcile_cookie_scope(
        &self,
        authority: &mut CookieAuthority<NativeVault>,
        scope: &CookieScope,
        correlation_id: &str,
    ) -> Result<(), Problem> {
        let pending = authority
            .pending_materializations()
            .map_err(|error| error.as_problem(correlation_id))?
            .into_iter()
            .filter(|(pending_scope, _)| pending_scope == scope)
            .collect::<Vec<_>>();
        if pending.is_empty() {
            return Ok(());
        }
        let profile = self
            .profiles
            .get(&scope.profile_id)
            .map_err(|error| correlate(error, correlation_id))?
            .ok_or_else(|| profile_not_found(correlation_id))?;
        if profile.value.runtime != Runtime::Wayfern
            || matches!(
                profile.value.state,
                ProfileState::Running | ProfileState::Launching | ProfileState::Stopping
            )
        {
            return Err(cookie_recovery_pending(correlation_id));
        }
        let profile_root = self
            .runtime
            .targets
            .profile_path(
                &profile.value.runtime,
                &self.runtime.profiles_root,
                &scope.profile_id,
            )
            .map_err(|_| cookie_recovery_pending(correlation_id))?;
        authority.set_profile_state(scope.clone(), ProfileRuntimeState::Stopped);
        let cookies = authority
            .materialization_inputs(scope, &local_principal(), &[])
            .map_err(|_| cookie_recovery_pending(correlation_id))?;
        let now_unix = unix_time_seconds(correlation_id)?.min(i64::MAX as u64) as i64;
        buglogin_browser::cookies::wayfern::materialize_from_inputs(
            &profile_root,
            ProfileCookieState::Stopped,
            &cookies,
            now_unix,
        )
        .map_err(|_| cookie_recovery_pending(correlation_id))?;
        for (_, idempotency_key) in pending {
            authority
                .complete_materialization(scope, &idempotency_key)
                .map_err(|_| cookie_recovery_pending(correlation_id))?;
        }
        Ok(())
    }

    fn preflight_cookie_scope(
        &self,
        scope: &CookieScope,
        correlation_id: &str,
    ) -> Result<(), Problem> {
        let profile = self
            .profiles
            .get(&scope.profile_id)
            .map_err(|error| correlate(error, correlation_id))?
            .ok_or_else(|| profile_not_found(correlation_id))?;
        if profile.value.runtime != Runtime::Wayfern
            || matches!(
                profile.value.state,
                ProfileState::Running | ProfileState::Launching | ProfileState::Stopping
            )
        {
            return Err(problem(
                "cookie_runtime_unsupported",
                "Cookie import requires a stopped Wayfern profile",
                409,
                correlation_id,
            ));
        }
        let profile_root = self
            .runtime
            .targets
            .profile_path(
                &profile.value.runtime,
                &self.runtime.profiles_root,
                &scope.profile_id,
            )
            .map_err(|_| {
                problem(
                    "cookie_store_unavailable",
                    "The native cookie database is not initialized",
                    409,
                    correlation_id,
                )
            })?;
        let now_unix = unix_time_seconds(correlation_id)?.min(i64::MAX as u64) as i64;
        buglogin_browser::cookies::wayfern::materialize_from_inputs(
            &profile_root,
            ProfileCookieState::Stopped,
            &[],
            now_unix,
        )
        .map_err(|_| {
            problem(
                "cookie_store_unavailable",
                "The native cookie database or encryption key is unavailable",
                409,
                correlation_id,
            )
        })?;
        Ok(())
    }

    fn set_cookie_profile_state(
        &self,
        authority: &mut CookieAuthority<NativeVault>,
        scope: &CookieScope,
        correlation_id: &str,
    ) -> Result<(), Problem> {
        let profile = self
            .profiles
            .get(&scope.profile_id)
            .map_err(|error| correlate(error, correlation_id))?
            .ok_or_else(|| profile_not_found(correlation_id))?;
        let state = match profile.value.state {
            ProfileState::Running | ProfileState::Launching | ProfileState::Stopping => {
                ProfileRuntimeState::Running
            }
            _ => ProfileRuntimeState::Stopped,
        };
        authority.set_profile_state(scope.clone(), state);
        Ok(())
    }

    fn proxy_parse(&self, payload: Value, correlation_id: &str) -> Result<Value, Problem> {
        let input = parse_payload::<ProxyParseInput>(payload, "proxy.parse", correlation_id)?;
        let report = parse_lines(&input.text);
        let mut rows: Vec<Value> = report
            .rows
            .iter()
            .map(|row| {
                json!({
                    "protocol": row.protocol,
                    "endpoint": format!("{}:{}", row.host, row.port),
                    "valid": true
                })
            })
            .collect();
        rows.extend(
            report
                .errors
                .iter()
                .map(|error| json!({ "line": error.line, "valid": false, "error": error.code })),
        );
        Ok(json!({
            "validCount": report.rows.len(),
            "invalidCount": report.errors.len(),
            "duplicateCount": report.duplicates,
            "rows": rows
        }))
    }

    fn proxy_list(&self, payload: Value, correlation_id: &str) -> Result<Value, Problem> {
        let input = parse_payload::<ProxyListInput>(payload, "proxy.list", correlation_id)?;
        let workspace_id = parse_workspace_id(&input.workspace_id, correlation_id)?;
        let repository = self.proxy_repository(correlation_id)?;
        let items = repository
            .list::<Proxy>()
            .map_err(|error| correlate(error, correlation_id))?
            .into_iter()
            .map(|proxy| proxy_record(&workspace_id, proxy, correlation_id))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(json!({ "items": items }))
    }

    fn proxy_get(&self, payload: Value, correlation_id: &str) -> Result<Value, Problem> {
        let input = parse_payload::<ProxyIdInput>(payload, "proxy.get", correlation_id)?;
        let workspace_id = parse_workspace_id(&input.workspace_id, correlation_id)?;
        let proxy_id = parse_proxy_id(&input.proxy_id, correlation_id)?;
        let repository = self.proxy_repository(correlation_id)?;
        let proxy = repository
            .get::<Proxy>(proxy_id.as_str())
            .map_err(|error| correlate(error, correlation_id))?
            .ok_or_else(|| proxy_not_found(correlation_id))?;
        proxy_record(&workspace_id, proxy, correlation_id)
    }

    fn proxy_create(&self, payload: Value, correlation_id: &str) -> Result<Value, Problem> {
        let input = parse_payload::<ProxyCreateInput>(payload, "proxy.create", correlation_id)?;
        let workspace_id = parse_workspace_id(&input.workspace_id, correlation_id)?;
        let proxy = input.proxy.into_proxy(correlation_id)?;
        let repository = self.proxy_repository(correlation_id)?;
        let existing = repository
            .list::<Proxy>()
            .map_err(|error| correlate(error, correlation_id))?;
        if let Some(existing) = existing
            .iter()
            .find(|item| same_endpoint(&item.value, &proxy))
        {
            return Ok(json!({
                "record": proxy_record(&workspace_id, (*existing).clone(), correlation_id)?,
                "deduplicated": true
            }));
        }
        if repository
            .get::<Proxy>(proxy.id.as_str())
            .map_err(|error| correlate(error, correlation_id))?
            .is_some()
        {
            return Err(problem(
                "proxy_id_conflict",
                "Proxy ID already exists",
                409,
                correlation_id,
            ));
        }
        let created = repository
            .create(proxy)
            .map_err(|error| correlate(error, correlation_id))?;
        Ok(json!({
            "record": proxy_record(&workspace_id, created, correlation_id)?,
            "deduplicated": false
        }))
    }

    fn proxy_update(&self, payload: Value, correlation_id: &str) -> Result<Value, Problem> {
        let input = parse_payload::<ProxyUpdateInput>(payload, "proxy.update", correlation_id)?;
        let workspace_id = parse_workspace_id(&input.workspace_id, correlation_id)?;
        let proxy_id = parse_proxy_id(&input.proxy_id, correlation_id)?;
        let repository = self.proxy_repository(correlation_id)?;
        let current = repository
            .get::<Proxy>(proxy_id.as_str())
            .map_err(|error| correlate(error, correlation_id))?
            .ok_or_else(|| proxy_not_found(correlation_id))?;
        if current.revision != input.expected_revision {
            return Err(revision_conflict(
                input.expected_revision,
                current.revision,
                correlation_id,
            ));
        }
        let mut proxy = input.proxy.into_proxy(correlation_id)?;
        if proxy.id != proxy_id {
            return Err(problem(
                "proxy_id_mismatch",
                "Proxy ID does not match update path",
                400,
                correlation_id,
            ));
        }
        if proxy.credential_ref.is_none() {
            proxy.credential_ref = current.value.credential_ref.clone();
        }
        proxy.rotation_url = current.value.rotation_url.clone();
        let duplicate = repository
            .list::<Proxy>()
            .map_err(|error| correlate(error, correlation_id))?
            .into_iter()
            .any(|item| item.value.id != proxy_id && same_endpoint(&item.value, &proxy));
        if duplicate {
            return Err(problem(
                "proxy_endpoint_conflict",
                "Proxy endpoint already exists",
                409,
                correlation_id,
            ));
        }
        let updated = repository
            .update(input.expected_revision, proxy)
            .map_err(|error| correlate(error, correlation_id))?;
        proxy_record(&workspace_id, updated, correlation_id)
    }

    fn proxy_delete(&self, payload: Value, correlation_id: &str) -> Result<Value, Problem> {
        let input = parse_payload::<ProxyDeleteInput>(payload, "proxy.delete", correlation_id)?;
        let workspace_id = parse_workspace_id(&input.workspace_id, correlation_id)?;
        let proxy_id = parse_proxy_id(&input.proxy_id, correlation_id)?;
        let repository = self.proxy_repository(correlation_id)?;
        let current = repository
            .get::<Proxy>(proxy_id.as_str())
            .map_err(|error| correlate(error, correlation_id))?
            .ok_or_else(|| proxy_not_found(correlation_id))?;
        if current.revision != input.expected_revision {
            return Err(revision_conflict(
                input.expected_revision,
                current.revision,
                correlation_id,
            ));
        }
        if repository
            .list::<Profile>()
            .map_err(|error| correlate(error, correlation_id))?
            .iter()
            .any(|profile| profile.value.proxy_id.as_ref() == Some(&proxy_id))
        {
            return Err(problem(
                "proxy_assigned",
                "Proxy is assigned to a profile",
                409,
                correlation_id,
            ));
        }
        repository
            .quarantine::<Proxy>(input.expected_revision, proxy_id.as_str())
            .map_err(|error| correlate(error, correlation_id))?;
        proxy_record(&workspace_id, current, correlation_id)
    }

    fn proxy_repository(&self, correlation_id: &str) -> Result<LocalRepository, Problem> {
        LocalRepository::open(self.runtime.repository_root.clone())
            .map_err(|error| correlate(error, correlation_id))
    }

    async fn profile_launch(
        &self,
        payload: Value,
        idempotency_key: Option<&str>,
        payload_hash: &str,
        correlation_id: &str,
    ) -> Result<Value, Problem> {
        let input = parse_payload::<ProfileLaunchInput>(payload, "profile.launch", correlation_id)?;
        let profile_id = parse_profile_id(&input.profile_id, correlation_id)?;
        let operation_correlation_id = correlation_id.to_owned();
        let action_correlation_id = operation_correlation_id.clone();
        let launch_idempotency_key = idempotency_key
            .expect("validated idempotency key")
            .to_owned();
        let runtime_idempotency_key = launch_idempotency_key.clone();
        self.run_operation(
            "profile.launch",
            &launch_idempotency_key,
            payload_hash,
            &operation_correlation_id,
            move |state| async move {
                state
                    .launch_profile(
                        &profile_id,
                        input.headless,
                        &runtime_idempotency_key,
                        &action_correlation_id,
                    )
                    .await
            },
        )
        .await
    }

    async fn profile_stop(
        &self,
        payload: Value,
        idempotency_key: Option<&str>,
        payload_hash: &str,
        correlation_id: &str,
    ) -> Result<Value, Problem> {
        let input = parse_payload::<ProfileIdInput>(payload, "profile.stop", correlation_id)?;
        let profile_id = parse_profile_id(&input.profile_id, correlation_id)?;
        let operation_correlation_id = correlation_id.to_owned();
        let action_correlation_id = operation_correlation_id.clone();
        self.run_operation(
            "profile.stop",
            idempotency_key.expect("validated idempotency key"),
            payload_hash,
            &operation_correlation_id,
            move |state| async move {
                state
                    .stop_profile(&profile_id, &action_correlation_id)
                    .await
            },
        )
        .await
    }

    fn operation_get(&self, payload: Value, correlation_id: &str) -> Result<Value, Problem> {
        let input = parse_payload::<OperationIdInput>(payload, "operation.get", correlation_id)?;
        let grants = [Grant {
            id: GrantId::from_str("grant_desktop_operation_read")
                .map_err(|_| serialization_problem(correlation_id))?,
            account_id: None,
            member_id: None,
            capability: Capability::new("operation.read")
                .map_err(|_| serialization_problem(correlation_id))?,
            action: Action::Read,
            scope: ResourceScope::global(),
            effect: Effect::Allow,
        }];
        let operations = self
            .operations
            .read()
            .map_err(|_| operation_unavailable(correlation_id))?;
        serde_json::to_value(
            operations
                .get(&Principal::default(), &grants, &input.operation_id)
                .map_err(|error| correlate(error, correlation_id))?,
        )
        .map_err(|_| serialization_problem(correlation_id))
    }

    async fn launch_profile(
        &self,
        profile_id: &ProfileId,
        headless: bool,
        idempotency_key: &str,
        correlation_id: &str,
    ) -> Result<Value, Problem> {
        let current = self
            .profiles
            .get(profile_id)
            .map_err(|error| correlate(error, correlation_id))?
            .ok_or_else(|| profile_not_found(correlation_id))?;
        let fingerprint_target =
            target_from_runtime_config(current.value.runtime_config.as_ref(), correlation_id)?;
        if fingerprint_target.is_some() && current.value.runtime != Runtime::Wayfern {
            return Err(problem(
                "fingerprint_runtime_unsupported",
                "Fingerprint targets are supported only by the Wayfern runtime",
                409,
                correlation_id,
            ));
        }
        if current.value.archived || current.value.state != ProfileState::Ready {
            return Err(problem(
                "profile_not_launchable",
                "Profile cannot be launched",
                409,
                correlation_id,
            ));
        }
        if !self.runtime.targets.supports(&current.value.runtime) {
            return Err(correlate(runtime_not_configured(), correlation_id));
        }
        self.reconcile_profile_cookie_materializations(profile_id, correlation_id)?;
        let binding = self
            .bindings
            .get(profile_id)
            .map_err(|error| correlate(error, correlation_id))?;
        let lease_scope = if matches!(current.value.sync_mode.as_ref(), Some(SyncMode::Managed)) {
            let binding = binding
                .filter(|binding| binding.value.archive_state == ProfileArchiveState::Active)
                .ok_or_else(|| {
                    problem(
                        "managed_profile_binding_required",
                        "Managed profile requires an active binding",
                        409,
                        correlation_id,
                    )
                })?;
            Some(LeaseScope {
                workspace_id: binding.value.workspace_id,
                resource_id: binding.value.resource_id,
            })
        } else {
            if binding.is_some() {
                return Err(problem(
                    "profile_binding_mismatch",
                    "Non-managed profile cannot have a binding",
                    409,
                    correlation_id,
                ));
            }
            None
        };
        let profile_path = self
            .runtime
            .targets
            .profile_path(
                &current.value.runtime,
                &self.runtime.profiles_root,
                profile_id,
            )
            .map_err(|error| correlate(error, correlation_id))?;
        fs::create_dir_all(&profile_path).map_err(|_| {
            problem(
                "profile_path_unavailable",
                "Profile path unavailable",
                503,
                correlation_id,
            )
        })?;
        let mut specification = self
            .runtime
            .targets
            .launch_specification(&current.value, &profile_path, headless)
            .map_err(|error| correlate(error, correlation_id))?;
        let mut proxy_bridge: Option<ProxyBridgeHandle> = None;
        if let Some(proxy_id) = current.value.proxy_id.as_ref() {
            let proxy = self
                .proxy_store
                .launch(proxy_id)
                .map_err(|error| correlate(error, correlation_id))?;
            let server = match proxy.bridge {
                Some(bridge_spec) => {
                    let bridge = ProxyBridgeHandle::start(bridge_spec)
                        .await
                        .map_err(|error| correlate(error, correlation_id))?;
                    let server = bridge.proxy_url().to_owned();
                    proxy_bridge = Some(bridge);
                    server
                }
                None => proxy.server,
            };
            push_or_replace_arg(&mut specification.args, format!("--proxy-server={server}"));
            if !proxy.bypass_rules.is_empty() {
                push_or_replace_arg(
                    &mut specification.args,
                    format!("--proxy-bypass-list={}", proxy.bypass_rules.join(";")),
                );
            }
        }
        let fingerprint_cdp_port = if fingerprint_target.is_some() {
            let port = match configure_local_cdp_args(&mut specification.args, correlation_id) {
                Ok(port) => port,
                Err(error) => {
                    if let Some(bridge) = proxy_bridge.take() {
                        let _ = bridge.stop().await;
                    }
                    return Err(correlate(error, correlation_id));
                }
            };
            if let Err(error) = clear_stale_cdp_marker(&profile_path, correlation_id) {
                if let Some(bridge) = proxy_bridge.take() {
                    let _ = bridge.stop().await;
                }
                return Err(correlate(error, correlation_id));
            }
            Some(port)
        } else {
            None
        };
        let runtime_version = specification.version.clone();
        let mut launching_profile = current.value;
        launching_profile.state = launching_profile
            .state
            .clone()
            .transition(ProfileState::Launching)
            .map_err(|error| correlate(error, correlation_id))?;
        launching_profile.updated_at = now(correlation_id)?;
        let launching = {
            let authority = self.cookie_authority(correlation_id)?;
            if authority.has_pending_materialization_for_profile(profile_id.as_str()) {
                return Err(cookie_recovery_pending(correlation_id));
            }
            self.profiles
                .update(current.revision, launching_profile)
                .map_err(|error| correlate(error, correlation_id))?
        };
        let receipt = match self
            .profile_runtime
            .launch(LaunchRequest {
                profile_id: profile_id.clone(),
                idempotency_key: idempotency_key.to_owned(),
                profile_path: profile_path.clone(),
                specification,
                lease_scope,
                preacquired_lease: None,
                proxy_bridge,
            })
            .await
        {
            Ok(receipt) => receipt,
            Err(error) => {
                self.persist_failed(launching, correlation_id)?;
                return Err(correlate(error, correlation_id));
            }
        };
        if let Some(target) = fingerprint_target.as_ref() {
            let Some(cdp_port) = fingerprint_cdp_port else {
                if let Err(stop_error) = self.profile_runtime.stop(profile_id.clone()).await {
                    let _ = self.persist_failed(launching, correlation_id);
                    return Err(correlate(stop_error, correlation_id));
                }
                self.persist_failed(launching, correlation_id)?;
                return Err(problem(
                    "wayfern_cdp_unavailable",
                    "Wayfern local control port was not configured",
                    503,
                    correlation_id,
                ));
            };
            if let Err(error) = apply_wayfern_fingerprint(
                &profile_path,
                target,
                &runtime_version,
                cdp_port,
                correlation_id,
            )
            .await
            {
                if let Err(stop_error) = self.profile_runtime.stop(profile_id.clone()).await {
                    let _ = self.persist_failed(launching, correlation_id);
                    return Err(correlate(stop_error, correlation_id));
                }
                self.persist_failed(launching, correlation_id)?;
                return Err(correlate(error, correlation_id));
            }
        }
        let mut running_profile = launching.value.clone();
        running_profile.state = running_profile
            .state
            .clone()
            .transition(ProfileState::Running)
            .map_err(|error| correlate(error, correlation_id))?;
        running_profile.runtime_version = Some(runtime_version);
        running_profile.updated_at = now(correlation_id)?;
        let updated = match self.profiles.update(launching.revision, running_profile) {
            Ok(updated) => updated,
            Err(error) => {
                let _ = self.profile_runtime.stop(profile_id.clone()).await;
                let _ = self.persist_failed(launching, correlation_id);
                return Err(correlate(error, correlation_id));
            }
        };
        Ok(json!({
            "profileId": profile_id,
            "revision": updated.revision,
            "processId": receipt.process_id
        }))
    }

    async fn stop_profile(
        &self,
        profile_id: &ProfileId,
        correlation_id: &str,
    ) -> Result<Value, Problem> {
        let current = self
            .profiles
            .get(profile_id)
            .map_err(|error| correlate(error, correlation_id))?
            .ok_or_else(|| profile_not_found(correlation_id))?;
        if current.value.state != ProfileState::Running {
            return Err(problem(
                "profile_not_running",
                "Profile is not running",
                409,
                correlation_id,
            ));
        }
        let mut stopping_profile = current.value;
        stopping_profile.state = stopping_profile
            .state
            .clone()
            .transition(ProfileState::Stopping)
            .map_err(|error| correlate(error, correlation_id))?;
        stopping_profile.updated_at = now(correlation_id)?;
        let stopping = self
            .profiles
            .update(current.revision, stopping_profile)
            .map_err(|error| correlate(error, correlation_id))?;
        if let Err(error) = self.profile_runtime.stop(profile_id.clone()).await {
            self.persist_failed(stopping, correlation_id)?;
            return Err(correlate(error, correlation_id));
        }
        let mut ready_profile = stopping.value;
        ready_profile.state = ready_profile
            .state
            .clone()
            .transition(ProfileState::Ready)
            .map_err(|error| correlate(error, correlation_id))?;
        ready_profile.updated_at = now(correlation_id)?;
        let updated = self
            .profiles
            .update(stopping.revision, ready_profile)
            .map_err(|error| correlate(error, correlation_id))?;
        Ok(json!({ "profileId": profile_id, "revision": updated.revision }))
    }

    fn persist_failed(
        &self,
        current: Versioned<Profile>,
        correlation_id: &str,
    ) -> Result<(), Problem> {
        let mut failed_profile = current.value;
        failed_profile.state = failed_profile
            .state
            .clone()
            .transition(ProfileState::Failed)
            .map_err(|error| correlate(error, correlation_id))?;
        failed_profile.updated_at = now(correlation_id)?;
        self.profiles
            .update(current.revision, failed_profile)
            .map(|_| ())
            .map_err(|error| correlate(error, correlation_id))
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
        F: FnOnce(AdapterState) -> Fut + Send + 'static,
        Fut: Future<Output = Result<Value, Problem>> + Send + 'static,
    {
        let operation_id = operation_id(command_id, idempotency_key);
        let accepted =
            json!({ "operationId": operation_id, "commandId": command_id, "status": "accepted" });
        {
            let mut records = self
                .idempotency
                .lock()
                .map_err(|_| idempotency_unavailable(correlation_id))?;
            if let Some(record) = records.get(idempotency_key) {
                if record.command_id != command_id || record.payload_hash != payload_hash {
                    return Err(problem(
                        "idempotency_conflict",
                        "Idempotency key conflict",
                        409,
                        correlation_id,
                    ));
                }
                return Ok(record.data.clone());
            }
            let mut operations = self
                .operations
                .write()
                .map_err(|_| operation_unavailable(correlation_id))?;
            operations
                .insert(
                    Operation::queued(&operation_id, command_id, ResourceScope::global())
                        .map_err(|error| correlate(error, correlation_id))?,
                )
                .map_err(|error| correlate(error, correlation_id))?;
            records.insert(
                idempotency_key.to_owned(),
                IdempotencyRecord {
                    command_id,
                    payload_hash: payload_hash.to_owned(),
                    data: accepted.clone(),
                },
            );
        }
        let state = self.clone();
        let worker_operation_id = operation_id.clone();
        let worker_correlation_id = correlation_id.to_owned();
        tokio::spawn(async move {
            let started = match state.operations.write() {
                Ok(mut operations) => operations
                    .transition(
                        &worker_operation_id,
                        OperationStatus::Running,
                        Some(0.1),
                        None,
                        None,
                    )
                    .is_ok(),
                Err(_) => false,
            };
            if !started {
                return;
            }
            let worker_state = state.clone();
            let outcome = match tokio::spawn(async move { action(worker_state).await }).await {
                Ok(outcome) => outcome,
                Err(_) => Err(problem(
                    "operation_worker_failed",
                    "Operation worker failed",
                    500,
                    &worker_correlation_id,
                )),
            };
            let Ok(mut operations) = state.operations.write() else {
                return;
            };
            let _ = match outcome {
                Ok(value) => operations.transition(
                    &worker_operation_id,
                    OperationStatus::Completed,
                    Some(1.0),
                    Some(value),
                    None,
                ),
                Err(error) => operations.transition(
                    &worker_operation_id,
                    OperationStatus::Failed,
                    Some(1.0),
                    None,
                    Some(error),
                ),
            };
        });
        Ok(accepted)
    }

    fn replay(
        &self,
        key: &str,
        command_id: &'static str,
        payload_hash: &str,
        correlation_id: &str,
    ) -> Result<Option<Value>, Problem> {
        let records = self
            .idempotency
            .lock()
            .map_err(|_| idempotency_unavailable(correlation_id))?;
        let Some(record) = records.get(key) else {
            return Ok(None);
        };
        if record.command_id != command_id || record.payload_hash != payload_hash {
            return Err(problem(
                "idempotency_conflict",
                "Idempotency key conflict",
                409,
                correlation_id,
            ));
        }
        Ok(Some(record.data.clone()))
    }

    fn remember(
        &self,
        key: &str,
        command_id: &'static str,
        payload_hash: String,
        data: Value,
        correlation_id: &str,
    ) -> Result<(), Problem> {
        self.idempotency
            .lock()
            .map_err(|_| idempotency_unavailable(correlation_id))?
            .insert(
                key.to_owned(),
                IdempotencyRecord {
                    command_id,
                    payload_hash,
                    data,
                },
            );
        Ok(())
    }
}

pub fn registry() -> Vec<AdapterCommandMetadata> {
    CommandRegistry::all()
        .iter()
        .filter(|command| {
            command.adapters.contains(&Adapter::Tauri)
                && SUPPORTED_COMMANDS.contains(&command.id.as_str())
        })
        .map(|command| AdapterCommandMetadata {
            invoke: command.id.as_str().replace('.', "_"),
            command_id: command.id.as_str(),
            kind: command.kind,
            capability: command.capability,
            audit_required: command.audit_required,
            auth_required: command.auth_required,
            idempotency_required: command.idempotency_required,
            timeout_ms: command.timeout_ms,
        })
        .collect()
}

fn supported_command(
    invoke_name: &str,
) -> Option<&'static buglogin_api::commands::CommandMetadata> {
    CommandRegistry::all().iter().find(|command| {
        command.adapters.contains(&Adapter::Tauri)
            && SUPPORTED_COMMANDS.contains(&command.id.as_str())
            && command.id.as_str().replace('.', "_") == invoke_name
    })
}

fn profile_list_item(profile: Versioned<Profile>, correlation_id: &str) -> Result<Value, Problem> {
    serde_json::to_value(ProfileRecord::new(profile.value, profile.revision))
        .map_err(|_| serialization_problem(correlation_id))
}

fn proxy_record(
    workspace_id: &WorkspaceId,
    proxy: Versioned<Proxy>,
    correlation_id: &str,
) -> Result<Value, Problem> {
    let view = ProxyView::from_proxy(&proxy.value);
    Ok(json!({
        "workspaceId": workspace_id,
        "proxy": serde_json::to_value(view).map_err(|_| serialization_problem(correlation_id))?,
        "revision": proxy.revision
    }))
}

fn same_endpoint(left: &Proxy, right: &Proxy) -> bool {
    protocol_name(&left.protocol) == protocol_name(&right.protocol)
        && left.host.trim().eq_ignore_ascii_case(right.host.trim())
        && left.port == right.port
}

fn validate_extension_group_fields(
    name: &str,
    extension_ids: &[buglogin_domain::ids::ExtensionId],
    correlation_id: &str,
) -> Result<(), Problem> {
    let unique = extension_ids
        .iter()
        .collect::<std::collections::BTreeSet<_>>();
    if name.trim().is_empty() || name.chars().count() > 120 || unique.len() != extension_ids.len() {
        return Err(problem(
            "extension_group_invalid",
            "Extension group fields are invalid",
            400,
            correlation_id,
        ));
    }
    Ok(())
}

fn protocol_name(protocol: &ProxyProtocol) -> &'static str {
    match protocol {
        ProxyProtocol::Http => "http",
        ProxyProtocol::Https => "https",
        ProxyProtocol::Socks4 => "socks4",
        ProxyProtocol::Socks5 => "socks5",
    }
}

fn parse_workspace_id(value: &str, correlation_id: &str) -> Result<WorkspaceId, Problem> {
    WorkspaceId::from_str(value).map_err(|_| {
        problem(
            "invalid_workspace_id",
            "Invalid workspace ID",
            400,
            correlation_id,
        )
    })
}

fn parse_proxy_id(value: &str, correlation_id: &str) -> Result<ProxyId, Problem> {
    ProxyId::from_str(value)
        .map_err(|_| problem("invalid_proxy_id", "Invalid proxy ID", 400, correlation_id))
}

fn proxy_not_found(correlation_id: &str) -> Problem {
    problem("proxy_not_found", "Proxy not found", 404, correlation_id)
}

fn revision_conflict(expected: u64, actual: u64, correlation_id: &str) -> Problem {
    Problem::new(
        "storage_revision_conflict",
        "Storage revision conflict",
        409,
        correlation_id,
    )
    .with_field("entity", "proxies")
    .with_field("expectedRevision", expected)
    .with_field("actualRevision", actual)
}
fn result(command_id: &str, correlation_id: &str, data: Value) -> CommandResult<Value> {
    CommandResult {
        command_id: command_id.to_owned(),
        correlation_id: correlation_id.to_owned(),
        data,
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProfileListInput {
    #[serde(default)]
    include_archived: bool,
    limit: Option<usize>,
    cursor: Option<String>,
    #[serde(default)]
    workspace_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProfileRevisionInput {
    profile_id: String,
    revision: u64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WorkspaceListInput {
    organization_id: String,
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
struct ResourceIdInput {
    id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProfileIdInput {
    profile_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OperationIdInput {
    operation_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProxyParseInput {
    text: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProxyCheckInput {
    workspace_id: String,
    proxy_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CookieListInput {
    workspace_id: String,
    profile_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CookieStageInput {
    workspace_id: String,
    profile_id: String,
    cookies: Vec<CookieWireInput>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CookieImportInput {
    workspace_id: String,
    profile_id: String,
    stage_id: String,
    expected_generation: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CookieCopyInput {
    source_workspace_id: String,
    source_profile_id: String,
    destination_workspace_id: String,
    destination_profile_id: String,
    expected_generation: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CookieWireInput {
    name: String,
    domain: String,
    path: String,
    secure: bool,
    http_only: bool,
    expires_at: Option<i64>,
    same_site: CookieSameSite,
    value: String,
}

impl CookieWireInput {
    fn into_cookie(self, correlation_id: &str) -> Result<CookieInput, Problem> {
        let value = SecretValue::new(self.value.into_bytes()).map_err(|_| {
            problem(
                "invalid_cookie_value",
                "Cookie value is invalid",
                400,
                correlation_id,
            )
        })?;
        Ok(CookieInput {
            name: self.name,
            domain: self.domain,
            path: self.path,
            secure: self.secure,
            http_only: self.http_only,
            expires_at: self.expires_at,
            same_site: self.same_site,
            value,
        })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProxyListInput {
    workspace_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProxyIdInput {
    workspace_id: String,
    proxy_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProxyCreateInput {
    workspace_id: String,
    proxy: ProxyMetadataInput,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProxyUpdateInput {
    workspace_id: String,
    proxy_id: String,
    expected_revision: u64,
    proxy: ProxyMetadataInput,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProxyDeleteInput {
    workspace_id: String,
    proxy_id: String,
    expected_revision: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProxyMetadataInput {
    id: String,
    name: Option<String>,
    protocol: ProxyProtocol,
    host: String,
    port: u16,
    bypass_rules: Vec<String>,
    created_at: String,
    updated_at: String,
}

impl ProxyMetadataInput {
    fn into_proxy(self, correlation_id: &str) -> Result<Proxy, Problem> {
        let id = parse_proxy_id(&self.id, correlation_id)?;
        let host = self.host.trim().to_owned();
        if host.is_empty() || host.len() > 255 || self.port == 0 {
            return Err(problem(
                "invalid_proxy",
                "Invalid proxy metadata",
                400,
                correlation_id,
            ));
        }
        if self.created_at.trim().is_empty() || self.updated_at.trim().is_empty() {
            return Err(problem(
                "invalid_proxy",
                "Proxy timestamps are required",
                400,
                correlation_id,
            ));
        }
        Ok(Proxy {
            id,
            name: self.name,
            protocol: self.protocol,
            host,
            port: self.port,
            credential_ref: None,
            rotation_url: None,
            bypass_rules: self.bypass_rules,
            created_at: self.created_at,
            updated_at: self.updated_at,
        })
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProxyView<'a> {
    id: &'a ProxyId,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: &'a Option<String>,
    protocol: &'static str,
    host: &'a str,
    port: u16,
    has_credential: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    credential_version: Option<&'a buglogin_domain::ids::CredentialId>,
    rotation_configured: bool,
    bypass_rules: &'a [String],
    created_at: &'a str,
    updated_at: &'a str,
}

impl<'a> ProxyView<'a> {
    fn from_proxy(proxy: &'a Proxy) -> Self {
        Self {
            id: &proxy.id,
            name: &proxy.name,
            protocol: protocol_name(&proxy.protocol),
            host: &proxy.host,
            port: proxy.port,
            has_credential: proxy.credential_ref.is_some(),
            credential_version: proxy.credential_ref.as_ref(),
            rotation_configured: proxy.rotation_url.is_some(),
            bypass_rules: &proxy.bypass_rules,
            created_at: &proxy.created_at,
            updated_at: &proxy.updated_at,
        }
    }
}

fn parse_payload<T: for<'de> Deserialize<'de>>(
    payload: Value,
    command_id: &str,
    correlation_id: &str,
) -> Result<T, Problem> {
    serde_json::from_value(payload).map_err(|_| {
        problem(
            "invalid_command_input",
            "Invalid command input",
            400,
            correlation_id,
        )
        .with_field("commandId", command_id)
    })
}

fn validate_correlation_id(correlation_id: &str) -> Result<(), Problem> {
    if correlation_id.trim().is_empty() {
        return Err(problem(
            "missing_correlation_id",
            "Correlation ID is required",
            400,
            "adapter",
        ));
    }
    Ok(())
}

fn validate_idempotency_key<'a>(
    key: Option<&'a str>,
    correlation_id: &str,
) -> Result<&'a str, Problem> {
    key.filter(|key| {
        (16..=128).contains(&key.len()) && key.bytes().all(|byte| byte.is_ascii_graphic())
    })
    .ok_or_else(|| {
        problem(
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
        .map_err(|_| serialization_problem("adapter"))
}

fn operation_id(command_id: &str, idempotency_key: &str) -> String {
    let digest = format!(
        "{:x}",
        Sha256::digest(format!("{command_id}:{idempotency_key}").as_bytes())
    );
    format!("op_{}", &digest[..24])
}

fn generated_profile_id(correlation_id: &str) -> Result<ProfileId, Problem> {
    let seed = format!(
        "{}:{}",
        unix_time_nanos(correlation_id)?,
        PROFILE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    );
    let digest = format!("{:x}", Sha256::digest(seed.as_bytes()));
    ProfileId::from_str(&format!("prf_{}", &digest[..24]))
        .map_err(|_| serialization_problem("desktop"))
}

fn parse_profile_id(value: &str, correlation_id: &str) -> Result<ProfileId, Problem> {
    ProfileId::from_str(value).map_err(|_| {
        problem(
            "invalid_profile_id",
            "Invalid profile ID",
            400,
            correlation_id,
        )
    })
}

fn cookie_scope(
    workspace_id: &str,
    profile_id: &str,
    correlation_id: &str,
) -> Result<CookieScope, Problem> {
    Ok(CookieScope {
        workspace_id: WorkspaceId::from_str(workspace_id).map_err(|_| {
            problem(
                "invalid_workspace_id",
                "Invalid workspace ID",
                400,
                correlation_id,
            )
        })?,
        profile_id: parse_profile_id(profile_id, correlation_id)?,
    })
}

fn local_principal() -> Principal {
    Principal {
        roles: std::collections::BTreeSet::from([buglogin_domain::permissions::Role::LocalOwner]),
        ..Principal::default()
    }
}

fn local_cookie_lease(scope: &CookieScope, correlation_id: &str) -> Result<Lease, Problem> {
    let now = unix_time_millis(correlation_id)?;
    Ok(Lease {
        lease_id: format!("local-cookie-{}", scope.profile_id.as_str()),
        fencing_token: 1,
        resource_id: ResourceId::from_str(&format!("res-cookie-{}", scope.profile_id.as_str()))
            .expect("canonical local cookie resource ID"),
        workspace_id: scope.workspace_id.clone(),
        owner_id: "desktop-local".into(),
        token: format!("local-cookie-token-{}", scope.profile_id.as_str()),
        issued_at: now,
        expires_at: crate::clock::checked_add_millis(now, 300_000, correlation_id)?,
    })
}

fn now(correlation_id: &str) -> Result<String, Problem> {
    Ok(unix_time_seconds(correlation_id)?.to_string())
}

fn correlate(mut error: Problem, correlation_id: &str) -> Problem {
    error.correlation_id = correlation_id.to_owned();
    error
}
fn profile_not_found(correlation_id: &str) -> Problem {
    problem(
        "profile_not_found",
        "Profile not found",
        404,
        correlation_id,
    )
}
fn operation_unavailable(correlation_id: &str) -> Problem {
    problem(
        "operation_unavailable",
        "Operation service unavailable",
        503,
        correlation_id,
    )
}
fn idempotency_unavailable(correlation_id: &str) -> Problem {
    problem(
        "idempotency_unavailable",
        "Idempotency service unavailable",
        503,
        correlation_id,
    )
}
fn serialization_problem(correlation_id: &str) -> Problem {
    problem(
        "adapter_serialization_failed",
        "Adapter serialization failed",
        500,
        correlation_id,
    )
}
fn problem(code: &str, title: &str, status: u16, correlation_id: &str) -> Problem {
    Problem::new(code, title, status, correlation_id)
}

fn cookie_recovery_pending(correlation_id: &str) -> Problem {
    Problem::new(
        "cookie_recovery_pending",
        "Cookie data recovery is still pending",
        503,
        correlation_id,
    )
    .retryable(true)
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeSet, HashMap};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use std::time::{SystemTime, UNIX_EPOCH};

    use buglogin_browser::{
        process::{ManagedProcess, ProcessIdentity, ProcessStatus},
        runtime::LaunchSpecification,
    };
    use buglogin_storage::{
        blob::checksum,
        secrets::{OsVault, RotationReceipt, SecretError, SecretKey, SecretValue, SecretVersion},
    };

    use super::*;

    #[derive(Clone, Default)]
    struct TestController {
        launches: Arc<AtomicUsize>,
        stops: Arc<AtomicUsize>,
        specifications: Arc<Mutex<Vec<LaunchSpecification>>>,
    }

    impl ProcessController for TestController {
        fn launch(
            &self,
            specification: &LaunchSpecification,
        ) -> Result<Box<dyn ManagedProcess>, Problem> {
            self.launches.fetch_add(1, Ordering::SeqCst);
            self.specifications
                .lock()
                .unwrap()
                .push(specification.clone());
            Ok(Box::new(TestProcess {
                identity: ProcessIdentity {
                    pid: 42,
                    executable: specification.executable.clone(),
                    token: "test-process-token".to_owned(),
                },
                stops: Arc::clone(&self.stops),
            }))
        }
        fn inspect(&self, _identity: &ProcessIdentity) -> ProcessStatus {
            ProcessStatus::Running
        }
        fn terminate(&self, _identity: &ProcessIdentity) -> Result<(), Problem> {
            self.stops.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[derive(Clone, Default)]
    struct AdapterTestVault {
        values: Arc<Mutex<HashMap<SecretKey, (SecretValue, SecretVersion)>>>,
    }

    impl OsVault for AdapterTestVault {
        fn read(&self, key: &SecretKey) -> Result<(SecretValue, SecretVersion), SecretError> {
            self.values
                .lock()
                .unwrap()
                .get(key)
                .cloned()
                .ok_or(SecretError::NotFound)
        }

        fn create(
            &mut self,
            key: &SecretKey,
            value: SecretValue,
        ) -> Result<SecretVersion, SecretError> {
            let mut values = self.values.lock().unwrap();
            if values.contains_key(key) {
                return Err(SecretError::VersionConflict);
            }
            let version = SecretVersion::new(1).unwrap();
            values.insert(key.clone(), (value, version));
            Ok(version)
        }

        fn rotate(
            &mut self,
            _key: &SecretKey,
            _expected: SecretVersion,
            _replacement: SecretValue,
        ) -> Result<RotationReceipt, SecretError> {
            Err(SecretError::OperationFailed)
        }

        fn remove(&mut self, key: &SecretKey, expected: SecretVersion) -> Result<(), SecretError> {
            let mut values = self.values.lock().unwrap();
            if values.get(key).map(|(_, version)| *version) != Some(expected) {
                return Err(SecretError::VersionConflict);
            }
            values.remove(key);
            Ok(())
        }
    }

    struct TestProcess {
        identity: ProcessIdentity,
        stops: Arc<AtomicUsize>,
    }
    impl ManagedProcess for TestProcess {
        fn identity(&self) -> &ProcessIdentity {
            &self.identity
        }
        fn status(&mut self) -> Result<ProcessStatus, Problem> {
            Ok(ProcessStatus::Running)
        }
        fn stop(&mut self) -> Result<(), Problem> {
            self.stops.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    fn state(name: &str) -> (AdapterState, TestController, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "buglogin-desktop-{name}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let repository_root = root.join("repository");
        let profiles_root = root.join("profiles");
        let executable = root.join("wayfern-test.exe");
        fs::create_dir_all(&root).unwrap();
        fs::write(&executable, b"test runtime").unwrap();
        let controller = TestController::default();
        let repository = LocalRepository::open(repository_root).unwrap();
        let state = AdapterState::from_parts(
            ProfileService::new(repository.clone()),
            ProfileBindingService::new(repository),
            RuntimeConfiguration {
                repository_root: root.join("repository"),
                profiles_root,
                targets: RuntimeTargets::wayfern(
                    RuntimeBinary {
                        path: executable,
                        version: "1.0.0".to_owned(),
                        expected_checksum: checksum(b"test runtime"),
                    },
                    Vec::new(),
                ),
                migration_executable: None,
            },
            controller.clone(),
        )
        .unwrap();
        (state, controller, root)
    }

    #[tokio::test]
    async fn local_authority_never_reports_fake_empty_organization_or_workspace_lists() {
        let (state, _, root) = state("hosted-only-lists");
        let organization = state
            .invoke("organization_list", "cor_org_list", None, json!({}))
            .await
            .unwrap_err();
        assert_eq!(organization.code, "hosted_organization_authority_required");
        assert_eq!(organization.correlation_id, "cor_org_list");

        let workspace = state
            .invoke(
                "workspace_list",
                "cor_workspace_list",
                None,
                json!({ "organizationId": "org_test" }),
            )
            .await
            .unwrap_err();
        assert_eq!(workspace.code, "hosted_workspace_authority_required");
        assert_eq!(workspace.correlation_id, "cor_workspace_list");
        fs::remove_dir_all(root).unwrap();
    }

    async fn create_profile(state: &AdapterState, key: &str) -> String {
        let profile_id = state
            .invoke(
                "profile_create",
                "cor_create",
                Some(key),
                json!({ "name": "Primary", "runtime": "wayfern", "runtimeVersion": "1.0.0" }),
            )
            .await
            .unwrap()
            .data["id"]
            .as_str()
            .unwrap()
            .to_owned();
        profile_id
    }

    fn initialize_supported_cookie_store(state: &AdapterState, profile_id: &str) {
        let profile_id = ProfileId::from_str(profile_id).unwrap();
        let profile_root = state
            .runtime
            .targets
            .profile_path(&Runtime::Wayfern, &state.runtime.profiles_root, &profile_id)
            .unwrap();
        fs::create_dir_all(&profile_root).unwrap();
        fs::write(
            profile_root.join("os_crypt_key"),
            b"adapter-test-os-crypt-key",
        )
        .unwrap();

        let database_path = buglogin_browser::cookies::wayfern::database_path(&profile_root);
        fs::create_dir_all(database_path.parent().unwrap()).unwrap();
        rusqlite::Connection::open(database_path)
            .unwrap()
            .execute_batch(
                "CREATE TABLE meta(key LONGVARCHAR NOT NULL UNIQUE PRIMARY KEY, value LONGVARCHAR);
                 INSERT INTO meta(key, value) VALUES('version', '24');
                 INSERT INTO meta(key, value) VALUES('last_compatible_version', '24');
                 CREATE TABLE cookies(
                     creation_utc INTEGER NOT NULL,
                     host_key TEXT NOT NULL,
                     top_frame_site_key TEXT NOT NULL,
                     name TEXT NOT NULL,
                     value TEXT NOT NULL,
                     encrypted_value BLOB NOT NULL,
                     path TEXT NOT NULL,
                     expires_utc INTEGER NOT NULL,
                     is_secure INTEGER NOT NULL,
                     is_httponly INTEGER NOT NULL,
                     last_access_utc INTEGER NOT NULL,
                     has_expires INTEGER NOT NULL,
                     is_persistent INTEGER NOT NULL,
                     priority INTEGER NOT NULL,
                     samesite INTEGER NOT NULL,
                     source_scheme INTEGER NOT NULL,
                     source_port INTEGER NOT NULL,
                     last_update_utc INTEGER NOT NULL,
                     source_type INTEGER NOT NULL,
                     has_cross_site_ancestor INTEGER NOT NULL
                 );
                 CREATE UNIQUE INDEX cookies_unique_index ON cookies(
                     host_key, top_frame_site_key, has_cross_site_ancestor,
                     name, path, source_scheme, source_port
                 );",
            )
            .unwrap();
    }

    async fn wait_operation(state: &AdapterState, operation_id: &Value) -> CommandResult<Value> {
        for _ in 0..100 {
            let operation = state
                .invoke(
                    "operation_get",
                    "cor_operation_wait",
                    None,
                    json!({ "operationId": operation_id }),
                )
                .await
                .unwrap();
            if matches!(
                operation.data["status"].as_str(),
                Some("completed" | "failed")
            ) {
                return operation;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("operation did not reach a terminal state");
    }

    fn proxy_metadata(id: &str, host: &str, port: u16) -> Value {
        json!({
            "id": id,
            "name": "Primary proxy",
            "protocol": "http",
            "host": host,
            "port": port,
            "bypassRules": ["localhost"],
            "createdAt": "2026-07-22T00:00:00Z",
            "updatedAt": "2026-07-22T00:00:00Z"
        })
    }

    #[test]
    fn runtime_targets_select_bugmini_and_apply_migrated_fingerprint() {
        let root = std::env::temp_dir().join(format!(
            "buglogin-bugmini-targets-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let executable = root.join("bugmini.exe");
        fs::write(&executable, b"bugmini runtime").unwrap();
        let targets = RuntimeTargets {
            wayfern: None,
            bugmini: Some(RuntimeTarget {
                binary: RuntimeBinary {
                    path: executable.clone(),
                    version: "146.0.7680.177.4".into(),
                    expected_checksum: checksum(b"bugmini runtime"),
                },
                extra_args: vec!["--lang=vi-VN".into()],
            }),
        };
        let profile = Profile {
            id: ProfileId::from_str("prf_bugmini_runtime").unwrap(),
            name: "Migrated Bugmini".into(),
            runtime: Runtime::Bugmini,
            runtime_version: Some("146.0.7680.177.4".into()),
            runtime_config: Some(json!({
                "schema_key": "chromium.runtime-config.v1",
                "schema_version": 1,
                "payload": {
                    "block_images": true,
                    "block_webgl": true,
                    "block_webrtc": true
                }
            })),
            state: ProfileState::Ready,
            archived: false,
            fingerprint: Some(json!({
                "language": "en_US",
                "timezone": "America/New_York",
                "userAgent": "Bugmini Test Agent"
            })),
            note: Some("legacy note".into()),
            proxy_id: None,
            vpn_id: None,
            group_ids: Vec::new(),
            tags: Vec::new(),
            extension_group_id: None,
            sync_mode: Some(SyncMode::Local),
            created_at: "2026-07-23T00:00:00Z".into(),
            updated_at: "2026-07-23T00:00:00Z".into(),
        };
        let profile_path = targets
            .profile_path(&profile.runtime, &root, &profile.id)
            .unwrap();
        let specification = targets
            .launch_specification(&profile, &profile_path, true)
            .unwrap();

        assert_eq!(specification.runtime, Runtime::Bugmini);
        assert_eq!(specification.executable, executable);
        assert!(specification.args.iter().any(|arg| arg == "--headless"));
        assert!(
            specification
                .args
                .iter()
                .any(|arg| arg == "--fingerprint-timezone=America/New_York")
        );
        assert!(specification.args.iter().any(|arg| arg == "--lang=en-US"));
        assert!(
            specification
                .args
                .iter()
                .any(|arg| arg == "--blink-settings=imagesEnabled=false")
        );
        assert_eq!(
            targets
                .profile_path(&Runtime::Wayfern, &root, &profile.id)
                .unwrap_err()
                .code,
            "runtime_not_configured"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn registry_contains_only_real_handlers() {
        let actual: BTreeSet<_> = registry()
            .iter()
            .map(|command| command.command_id)
            .collect();
        let expected: BTreeSet<_> = SUPPORTED_COMMANDS.iter().copied().collect();
        assert_eq!(actual, expected);
        assert!(
            registry()
                .iter()
                .all(|command| command.invoke == command.command_id.replace('.', "_"))
        );
    }

    #[tokio::test]
    async fn profile_create_list_and_idempotency_are_real() {
        let (state, _, root) = state("profiles");
        let key = "idem_profile_create_0001";
        let profile_id = create_profile(&state, key).await;
        let replayed = create_profile(&state, key).await;
        let listed = state
            .invoke("profile_list", "cor_list", None, json!({}))
            .await
            .unwrap();
        assert_eq!(profile_id, replayed);
        assert_eq!(listed.data["items"].as_array().unwrap().len(), 1);
        assert_eq!(listed.data["items"][0]["id"], profile_id);
        assert_eq!(listed.data["items"][0]["state"], "ready");
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn cookie_list_is_local_authority_backed_and_redacts_values() {
        let (state, _, root) = state("cookie-list");
        let profile_id = create_profile(&state, "idem_cookie_profile_create_0001").await;
        let result = state
            .invoke(
                "cookie_list",
                "cor_cookie_list",
                None,
                json!({
                    "workspaceId": "ws_cookie",
                    "profileId": profile_id
                }),
            )
            .await
            .unwrap();
        assert_eq!(result.data["generation"], 0);
        assert_eq!(result.data["cookies"].as_array().unwrap().len(), 0);
        assert!(!result.data.to_string().contains("value"));
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn cookie_stage_returns_opaque_id_without_echoing_secret() {
        let (state, _, root) = state("cookie-stage");
        let profile_id = create_profile(&state, "idem_cookie_stage_profile_0001").await;
        let result = state
            .invoke(
                "cookie_stage",
                "cor_cookie_stage",
                Some("idem_cookie_stage_0001"),
                json!({
                    "workspaceId": "ws_cookie",
                    "profileId": profile_id,
                    "cookies": [{
                        "name": "sid",
                        "domain": ".example.com",
                        "path": "/",
                        "secure": true,
                        "httpOnly": true,
                        "expiresAt": null,
                        "sameSite": "lax",
                        "value": "secret-value"
                    }]
                }),
            )
            .await
            .unwrap();
        let stage_id = result.data["stageId"].as_str().unwrap();
        assert_eq!(stage_id.len(), 64);
        assert!(!result.data.to_string().contains("secret-value"));
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn cookie_import_persists_generation_and_rejects_stale_cas() {
        let (state, _, root) = state("cookie-import");
        let profile_id = create_profile(&state, "idem_cookie_import_profile_0001").await;
        initialize_supported_cookie_store(&state, &profile_id);
        let stage = state
            .invoke(
                "cookie_stage",
                "cor_cookie_import_stage",
                Some("idem_cookie_import_stage_0001"),
                json!({
                    "workspaceId": "ws_cookie_import",
                    "profileId": profile_id,
                    "cookies": [{
                        "name": "sid",
                        "domain": ".example.com",
                        "path": "/",
                        "secure": true,
                        "httpOnly": true,
                        "expiresAt": null,
                        "sameSite": "lax",
                        "value": "secret-value"
                    }]
                }),
            )
            .await
            .unwrap();
        let stage_id = stage.data["stageId"].as_str().unwrap();
        let payload = json!({
            "workspaceId": "ws_cookie_import",
            "profileId": profile_id,
            "stageId": stage_id,
            "expectedGeneration": 0
        });
        let imported = state
            .invoke(
                "cookie_import",
                "cor_cookie_import",
                Some("idem_cookie_import_0001"),
                payload.clone(),
            )
            .await
            .unwrap();
        assert_eq!(imported.data["generation"], 1);
        assert_eq!(imported.data["report"]["inserted"], 1);
        assert_eq!(imported.data["replayed"], false);

        let listed = state
            .invoke(
                "cookie_list",
                "cor_cookie_import_list",
                None,
                json!({
                    "workspaceId": "ws_cookie_import",
                    "profileId": profile_id
                }),
            )
            .await
            .unwrap();
        assert_eq!(listed.data["generation"], 1);
        assert_eq!(listed.data["cookies"][0]["name"], "sid");
        assert_eq!(listed.data["cookies"][0]["valuePresent"], true);
        assert!(!listed.data.to_string().contains("secret-value"));

        let replay = state
            .invoke(
                "cookie_import",
                "cor_cookie_import_replay",
                Some("idem_cookie_import_0001"),
                payload,
            )
            .await
            .unwrap();
        assert_eq!(replay.data, imported.data);

        let second_stage = state
            .invoke(
                "cookie_stage",
                "cor_cookie_import_stage_two",
                Some("idem_cookie_import_stage_0002"),
                json!({
                    "workspaceId": "ws_cookie_import",
                    "profileId": profile_id,
                    "cookies": [{
                        "name": "sid-2",
                        "domain": ".example.com",
                        "path": "/",
                        "secure": true,
                        "httpOnly": true,
                        "expiresAt": null,
                        "sameSite": "lax",
                        "value": "second-secret"
                    }]
                }),
            )
            .await
            .unwrap();
        let second_stage_id = second_stage.data["stageId"].as_str().unwrap();

        let stale = state
            .invoke(
                "cookie_import",
                "cor_cookie_import_stale",
                Some("idem_cookie_import_0002"),
                json!({
                    "workspaceId": "ws_cookie_import",
                    "profileId": profile_id,
                    "stageId": second_stage_id,
                    "expectedGeneration": 0
                }),
            )
            .await
            .unwrap_err();
        assert_eq!(stale.code, "cookie_generation_mismatch");
        assert_eq!(stale.title, "Cookie operation conflicts with current state");
        assert_eq!(stale.correlation_id, "cor_cookie_import_stale");
        assert!(!stale.to_string().contains("secret-value"));
        drop(state);
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn cookie_copy_uses_local_authority_and_destination_generation() {
        let (state, _, root) = state("cookie-copy");
        let source_profile = create_profile(&state, "idem_cookie_copy_source_0001").await;
        let destination_profile = create_profile(&state, "idem_cookie_copy_destination_0001").await;
        initialize_supported_cookie_store(&state, &source_profile);
        initialize_supported_cookie_store(&state, &destination_profile);
        let stage = state
            .invoke(
                "cookie_stage",
                "cor_cookie_copy_stage",
                Some("idem_cookie_copy_stage_0001"),
                json!({
                    "workspaceId": "ws_cookie_copy",
                    "profileId": source_profile,
                    "cookies": [{
                        "name": "sid",
                        "domain": ".example.com",
                        "path": "/",
                        "secure": false,
                        "httpOnly": true,
                        "expiresAt": null,
                        "sameSite": "strict",
                        "value": "copy-secret"
                    }]
                }),
            )
            .await
            .unwrap();
        let stage_id = stage.data["stageId"].as_str().unwrap();
        state
            .invoke(
                "cookie_import",
                "cor_cookie_copy_import",
                Some("idem_cookie_copy_import_0001"),
                json!({
                    "workspaceId": "ws_cookie_copy",
                    "profileId": source_profile,
                    "stageId": stage_id,
                    "expectedGeneration": 0
                }),
            )
            .await
            .unwrap();
        let copied = state
            .invoke(
                "cookie_copy",
                "cor_cookie_copy",
                Some("idem_cookie_copy_0001"),
                json!({
                    "sourceWorkspaceId": "ws_cookie_copy",
                    "sourceProfileId": source_profile,
                    "destinationWorkspaceId": "ws_cookie_copy",
                    "destinationProfileId": destination_profile,
                    "expectedGeneration": 0
                }),
            )
            .await
            .unwrap();
        assert_eq!(copied.data["generation"], 1);
        assert_eq!(copied.data["report"]["copied"], 1);
        assert_eq!(copied.data["replayed"], false);

        let listed = state
            .invoke(
                "cookie_list",
                "cor_cookie_copy_list",
                None,
                json!({
                    "workspaceId": "ws_cookie_copy",
                    "profileId": destination_profile
                }),
            )
            .await
            .unwrap();
        assert_eq!(listed.data["generation"], 1);
        assert_eq!(listed.data["cookies"][0]["valuePresent"], true);
        assert!(!listed.data.to_string().contains("copy-secret"));
        drop(state);
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn profile_create_assigns_existing_proxy() {
        let (state, _, root) = state("profile-create-proxy");
        state
            .invoke(
                "proxy_create",
                "cor_profile_proxy_create",
                Some("idem_profile_proxy_create_0001"),
                json!({
                    "workspaceId": "ws_profile_create_proxy",
                    "proxy": proxy_metadata("prx_profile_create", "proxy.example", 8080)
                }),
            )
            .await
            .unwrap();

        let created = state
            .invoke(
                "profile_create",
                "cor_profile_create_with_proxy",
                Some("idem_profile_create_with_proxy_0001"),
                json!({
                    "name": "Proxied",
                    "runtime": "wayfern",
                    "runtimeVersion": "1.0.0",
                    "proxyId": "prx_profile_create"
                }),
            )
            .await
            .unwrap();

        assert_eq!(created.data["proxyId"], "prx_profile_create");
        assert_eq!(created.data["revision"], 1);
        let profile_id = ProfileId::from_str(created.data["id"].as_str().unwrap()).unwrap();
        let stored = state.profiles.get(&profile_id).unwrap().unwrap();
        assert_eq!(
            stored.value.proxy_id,
            Some(ProxyId::from_str("prx_profile_create").unwrap())
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn profile_create_rejects_missing_proxy_without_partial_write() {
        let (state, _, root) = state("profile-create-missing-proxy");
        let error = state
            .invoke(
                "profile_create",
                "cor_profile_create_missing_proxy",
                Some("idem_profile_create_missing_proxy_0001"),
                json!({
                    "name": "Missing proxy",
                    "runtime": "wayfern",
                    "runtimeVersion": "1.0.0",
                    "proxyId": "prx_missing_profile_create"
                }),
            )
            .await
            .unwrap_err();

        assert_eq!(error.code, "proxy_not_found");
        assert!(state.profiles.list().unwrap().is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn profile_archive_restore_are_real_and_revision_checked() {
        let (state, _, root) = state("profile-archive-restore");
        let profile_id = create_profile(&state, "idem_profile_archive_create_0001").await;
        let archived = state
            .invoke(
                "profile_archive",
                "cor_profile_archive",
                Some("idem_profile_archive_0001"),
                json!({ "profileId": profile_id, "revision": 1 }),
            )
            .await
            .unwrap();
        assert_eq!(archived.data["archived"], true);
        assert_eq!(archived.data["revision"], 2);
        let active_only = state
            .invoke("profile_list", "cor_list_active", None, json!({}))
            .await
            .unwrap();
        assert!(active_only.data["items"].as_array().unwrap().is_empty());
        let including_archived = state
            .invoke(
                "profile_list",
                "cor_list_archived",
                None,
                json!({ "includeArchived": true }),
            )
            .await
            .unwrap();
        assert_eq!(including_archived.data["items"][0]["archived"], true);
        assert_eq!(including_archived.data["items"][0]["revision"], 2);
        let restored = state
            .invoke(
                "profile_restore",
                "cor_profile_restore",
                Some("idem_profile_restore_0001"),
                json!({ "profileId": profile_id, "revision": 2 }),
            )
            .await
            .unwrap();
        assert_eq!(restored.data["archived"], false);
        assert_eq!(restored.data["revision"], 3);
        let stale = state
            .invoke(
                "profile_archive",
                "cor_profile_archive_stale",
                Some("idem_profile_archive_stale_0001"),
                json!({ "profileId": profile_id, "revision": 1 }),
            )
            .await
            .unwrap_err();
        assert_eq!(stale.code, "storage_revision_conflict");
        fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test]
    async fn proxy_parse_delegates_and_redacts_credentials() {
        let (state, _, root) = state("proxy");
        let result = state
            .invoke(
                "proxy_parse",
                "cor_proxy",
                None,
                json!({ "text": "http://user:secret@127.0.0.1:8080\ninvalid" }),
            )
            .await
            .unwrap();
        let serialized = serde_json::to_string(&result).unwrap();
        assert_eq!(result.data["validCount"], 1);
        assert_eq!(result.data["invalidCount"], 1);
        assert!(!serialized.contains("secret"));
        assert!(!serialized.contains("user"));
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn proxy_check_runs_real_connectivity_and_fails_closed() {
        let (state, _, root) = state("proxy-connectivity-check");
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let workspace_id = "ws_proxy_check";
        let proxy_id = "prx_proxy_check";
        state
            .invoke(
                "proxy_create",
                "cor_proxy_check_create",
                Some("idem_proxy_check_create_0001"),
                json!({
                    "workspaceId": workspace_id,
                    "proxy": proxy_metadata(proxy_id, "127.0.0.1", port)
                }),
            )
            .await
            .unwrap();
        let error = state
            .invoke(
                "proxy_check",
                "cor_proxy_check",
                None,
                json!({ "workspaceId": workspace_id, "proxyId": proxy_id }),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, "proxy_check_connection_failed");
        assert!(!error.title.contains("127.0.0.1"));
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn proxy_inventory_crud_deduplicates_and_enforces_revision() {
        let (state, _, root) = state("proxy-inventory-crud");
        let workspace_id = "ws_local_proxy";
        let created = state
            .invoke(
                "proxy_create",
                "cor_proxy_create",
                Some("idem_proxy_create_0001"),
                json!({
                    "workspaceId": workspace_id,
                    "proxy": proxy_metadata("prx_local_one", " Proxy.Example ", 8080)
                }),
            )
            .await
            .unwrap();
        assert_eq!(created.data["deduplicated"], false);
        assert_eq!(created.data["record"]["revision"], 1);
        assert_eq!(created.data["record"]["proxy"]["host"], "Proxy.Example");
        assert_eq!(created.data["record"]["proxy"]["hasCredential"], false);

        let duplicate = state
            .invoke(
                "proxy_create",
                "cor_proxy_duplicate",
                Some("idem_proxy_create_0002"),
                json!({
                    "workspaceId": workspace_id,
                    "proxy": proxy_metadata("prx_local_duplicate", "proxy.example", 8080)
                }),
            )
            .await
            .unwrap();
        assert_eq!(duplicate.data["deduplicated"], true);
        assert_eq!(duplicate.data["record"]["proxy"]["id"], "prx_local_one");

        let listed = state
            .invoke(
                "proxy_list",
                "cor_proxy_list",
                None,
                json!({ "workspaceId": workspace_id }),
            )
            .await
            .unwrap();
        assert_eq!(listed.data["items"].as_array().unwrap().len(), 1);
        let fetched = state
            .invoke(
                "proxy_get",
                "cor_proxy_get",
                None,
                json!({ "workspaceId": workspace_id, "proxyId": "prx_local_one" }),
            )
            .await
            .unwrap();
        assert_eq!(fetched.data["proxy"]["id"], "prx_local_one");

        let updated = state
            .invoke(
                "proxy_update",
                "cor_proxy_update",
                Some("idem_proxy_update_0001"),
                json!({
                    "workspaceId": workspace_id,
                    "proxyId": "prx_local_one",
                    "expectedRevision": 1,
                    "proxy": proxy_metadata("prx_local_one", "updated.example", 9090)
                }),
            )
            .await
            .unwrap();
        assert_eq!(updated.data["revision"], 2);
        assert_eq!(updated.data["proxy"]["host"], "updated.example");

        let stale = state
            .invoke(
                "proxy_update",
                "cor_proxy_update_stale",
                Some("idem_proxy_update_0002"),
                json!({
                    "workspaceId": workspace_id,
                    "proxyId": "prx_local_one",
                    "expectedRevision": 1,
                    "proxy": proxy_metadata("prx_local_one", "stale.example", 9091)
                }),
            )
            .await
            .unwrap_err();
        assert_eq!(stale.code, "storage_revision_conflict");

        let deleted = state
            .invoke(
                "proxy_delete",
                "cor_proxy_delete",
                Some("idem_proxy_delete_0001"),
                json!({
                    "workspaceId": workspace_id,
                    "proxyId": "prx_local_one",
                    "expectedRevision": 2
                }),
            )
            .await
            .unwrap();
        assert_eq!(deleted.data["proxy"]["id"], "prx_local_one");
        let missing = state
            .invoke(
                "proxy_get",
                "cor_proxy_missing",
                None,
                json!({ "workspaceId": workspace_id, "proxyId": "prx_local_one" }),
            )
            .await
            .unwrap_err();
        assert_eq!(missing.code, "proxy_not_found");
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn proxy_delete_blocks_assigned_profile_and_inputs_reject_secrets() {
        let (state, _, root) = state("proxy-assignment-delete");
        let workspace_id = "ws_local_assignment";
        state
            .invoke(
                "proxy_create",
                "cor_proxy_create_assigned",
                Some("idem_proxy_create_0003"),
                json!({
                    "workspaceId": workspace_id,
                    "proxy": proxy_metadata("prx_local_assigned", "assigned.example", 8080)
                }),
            )
            .await
            .unwrap();
        let profile_id = create_profile(&state, "idem_profile_proxy_0001").await;
        let profile_id = ProfileId::from_str(&profile_id).unwrap();
        let current = state.profiles.get(&profile_id).unwrap().unwrap();
        let mut profile = current.value;
        profile.proxy_id = Some(ProxyId::from_str("prx_local_assigned").unwrap());
        state.profiles.update(current.revision, profile).unwrap();

        let assigned = state
            .invoke(
                "proxy_delete",
                "cor_proxy_delete_assigned",
                Some("idem_proxy_delete_0002"),
                json!({
                    "workspaceId": workspace_id,
                    "proxyId": "prx_local_assigned",
                    "expectedRevision": 1
                }),
            )
            .await
            .unwrap_err();
        assert_eq!(assigned.code, "proxy_assigned");

        let secret_input = state
            .invoke(
                "proxy_create",
                "cor_proxy_secret_rejected",
                Some("idem_proxy_create_0004"),
                json!({
                    "workspaceId": workspace_id,
                    "proxy": {
                        "id": "prx_secret_rejected",
                        "name": "Secret",
                        "protocol": "http",
                        "host": "secret.example",
                        "port": 8080,
                        "username": "alice",
                        "password": "never-store-this",
                        "bypassRules": [],
                        "createdAt": "2026-07-22T00:00:00Z",
                        "updatedAt": "2026-07-22T00:00:00Z"
                    }
                }),
            )
            .await
            .unwrap_err();
        assert_eq!(secret_input.code, "invalid_command_input");
        assert!(
            !serde_json::to_string(&assigned)
                .unwrap()
                .contains("never-store-this")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn operation_returns_before_completion_and_same_key_runs_once() {
        let (state, _, root) = state("background-operation");
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let calls = Arc::new(AtomicUsize::new(0));
        let worker_started = Arc::clone(&started);
        let worker_release = Arc::clone(&release);
        let worker_calls = Arc::clone(&calls);
        let accepted = state
            .run_operation(
                "profile.launch",
                "idem_background_operation_0001",
                "payload-hash",
                "cor_background",
                move |_| async move {
                    worker_calls.fetch_add(1, Ordering::SeqCst);
                    worker_started.notify_one();
                    worker_release.notified().await;
                    Ok(json!({ "done": true }))
                },
            )
            .await
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), started.notified())
            .await
            .unwrap();
        let replayed = state
            .run_operation(
                "profile.launch",
                "idem_background_operation_0001",
                "payload-hash",
                "cor_background_replay",
                |_| async { panic!("idempotent replay must not execute") },
            )
            .await
            .unwrap();
        assert_eq!(accepted, replayed);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        release.notify_one();
        let operation = wait_operation(&state, &accepted["operationId"]).await;
        assert_eq!(operation.data["status"], "completed");
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn authenticated_proxy_uses_loopback_bridge_and_stops_with_profile() {
        let (mut state, controller, root) = state("authenticated-proxy-bridge");
        let repository = LocalRepository::open(root.join("repository")).unwrap();
        state.proxy_store =
            DesktopProxyStore::with_vault(repository, Box::new(AdapterTestVault::default()));
        let imported = state
            .import_proxies(
                vec![buglogin_network::proxy::ProxyRow {
                    protocol: buglogin_network::proxy::ProxyProtocol::Http,
                    host: "127.0.0.1".into(),
                    port: 6553,
                    username: Some("alice".into()),
                    password: Some("secret".into()),
                }],
                None,
            )
            .unwrap();
        let proxy_id = imported.proxies[0].id.clone();
        let created = state
            .invoke(
                "profile_create",
                "cor_auth_proxy_create",
                Some("idem_auth_proxy_create_0001"),
                json!({
                    "name": "Authenticated proxy",
                    "runtime": "wayfern",
                    "runtimeVersion": "1.0.0",
                    "proxyId": proxy_id
                }),
            )
            .await
            .unwrap();
        let profile_id = created.data["id"].as_str().unwrap().to_owned();
        let launched = state
            .invoke(
                "profile_launch",
                "cor_auth_proxy_launch",
                Some("idem_auth_proxy_launch_0001"),
                json!({"profileId": profile_id, "headless": true}),
            )
            .await
            .unwrap();
        let operation = wait_operation(&state, &launched.data["operationId"]).await;
        assert_eq!(operation.data["status"], "completed");
        let specifications = controller.specifications.lock().unwrap();
        let proxy_argument = specifications[0]
            .args
            .iter()
            .find(|argument| argument.starts_with("--proxy-server="))
            .unwrap()
            .clone();
        drop(specifications);
        assert!(proxy_argument.starts_with("--proxy-server=http://127.0.0.1:"));
        assert!(!proxy_argument.contains("alice"));
        assert!(!proxy_argument.contains("secret"));
        let bridge_address = proxy_argument
            .strip_prefix("--proxy-server=http://")
            .unwrap()
            .to_owned();
        assert!(
            tokio::net::TcpStream::connect(&bridge_address)
                .await
                .is_ok()
        );
        let stopped = state
            .invoke(
                "profile_stop",
                "cor_auth_proxy_stop",
                Some("idem_auth_proxy_stop_00001"),
                json!({"profileId": profile_id}),
            )
            .await
            .unwrap();
        let operation = wait_operation(&state, &stopped.data["operationId"]).await;
        assert_eq!(operation.data["status"], "completed");
        assert!(
            tokio::net::TcpStream::connect(&bridge_address)
                .await
                .is_err()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn launch_stop_and_operation_get_use_canonical_services() {
        let (state, controller, root) = state("lifecycle");
        let profile_id = create_profile(&state, "idem_profile_create_0002").await;
        let launch = state
            .invoke(
                "profile_launch",
                "cor_launch",
                Some("idem_profile_launch_0001"),
                json!({ "profileId": profile_id, "headless": true }),
            )
            .await
            .unwrap();
        let launch_operation = wait_operation(&state, &launch.data["operationId"]).await;
        let stop = state
            .invoke(
                "profile_stop",
                "cor_stop",
                Some("idem_profile_stop_00001"),
                json!({ "profileId": profile_id }),
            )
            .await
            .unwrap();
        let stop_operation = wait_operation(&state, &stop.data["operationId"]).await;
        let relaunch = state
            .invoke(
                "profile_launch",
                "cor_relaunch",
                Some("idem_profile_relaunch_0001"),
                json!({ "profileId": profile_id, "headless": true }),
            )
            .await
            .unwrap();
        let relaunch_operation = wait_operation(&state, &relaunch.data["operationId"]).await;
        let second_stop = state
            .invoke(
                "profile_stop",
                "cor_second_stop",
                Some("idem_profile_stop_00002"),
                json!({ "profileId": profile_id }),
            )
            .await
            .unwrap();
        let second_stop_operation = wait_operation(&state, &second_stop.data["operationId"]).await;
        let listed = state
            .invoke("profile_list", "cor_list", None, json!({}))
            .await
            .unwrap();
        assert_eq!(launch_operation.data["status"], "completed");
        assert_eq!(stop_operation.data["status"], "completed");
        assert_eq!(relaunch_operation.data["status"], "completed");
        assert_eq!(second_stop_operation.data["status"], "completed");
        assert_eq!(listed.data["items"][0]["state"], "ready");
        assert_eq!(controller.launches.load(Ordering::SeqCst), 2);
        assert_eq!(controller.stops.load(Ordering::SeqCst), 2);
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn managed_profile_without_binding_never_spawns_process() {
        let (state, controller, root) = state("managed-missing-binding");
        let profile_id = create_profile(&state, "idem_profile_create_0003").await;
        let id = ProfileId::from_str(&profile_id).unwrap();
        let current = state.profiles.get(&id).unwrap().unwrap();
        let mut profile = current.value;
        profile.sync_mode = Some(SyncMode::Managed);
        profile.updated_at = now("test").unwrap();
        state.profiles.update(current.revision, profile).unwrap();

        let launch = state
            .invoke(
                "profile_launch",
                "cor_managed_launch",
                Some("idem_profile_launch_0002"),
                json!({ "profileId": profile_id, "headless": true }),
            )
            .await
            .unwrap();
        let operation = wait_operation(&state, &launch.data["operationId"]).await;

        assert_eq!(operation.data["status"], "failed");
        assert_eq!(
            operation.data["error"]["code"],
            "managed_profile_binding_required"
        );
        assert_eq!(controller.launches.load(Ordering::SeqCst), 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn unsupported_registered_command_is_not_advertised_or_invocable() {
        let (state, _, root) = state("unsupported");
        let error = state
            .invoke("profile_delete", "cor_delete", None, json!({}))
            .await
            .unwrap_err();
        assert_eq!(error.code, "command_not_found");
        assert!(
            !registry()
                .iter()
                .any(|command| command.command_id == "profile.delete")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn hosted_profile_commands_fail_closed_in_local_mode() {
        let (state, _, root) = state("hosted-profile-authority");
        for invoke in ["profile_clone", "profile_share", "profile_share_revoke"] {
            let error = state
                .invoke(
                    invoke,
                    "cor_hosted_profile",
                    Some("idem_hosted_profile"),
                    json!({}),
                )
                .await
                .unwrap_err();
            assert_eq!(error.code, "hosted_profile_authority_required");
            assert_eq!(error.status, 409);
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn hosted_workspace_mutations_fail_closed_in_local_mode() {
        let (state, _, root) = state("hosted-workspace-authority");
        for invoke in [
            "group_list",
            "group_get",
            "group_create",
            "group_update",
            "group_delete",
            "profile_groups_replace",
            "profile_proxy_assignment_set",
            "profile_update",
        ] {
            let error = state
                .invoke(
                    invoke,
                    "cor_hosted_workspace",
                    Some("idem_hosted_workspace"),
                    json!({}),
                )
                .await
                .unwrap_err();
            assert_eq!(error.code, "hosted_workspace_authority_required");
            assert_eq!(error.status, 409);
        }
        fs::remove_dir_all(root).unwrap();
    }
}
