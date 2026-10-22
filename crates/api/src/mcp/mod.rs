pub mod transport;

use std::{
    cell::Cell,
    collections::BTreeMap,
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};

use async_trait::async_trait;
use buglogin_domain::{
    entities::{Profile, ProfileCreateInput, ProfileRecord, Proxy, ProxyProtocol},
    errors::Problem,
    ids::{
        AccountId, GrantId, MemberId, OrganizationId, ProfileId, ResourceId, TeamId, WorkspaceId,
    },
    organization::{Organization, OrganizationPolicy},
    permissions::{Action, Capability, Principal, ResourceScope},
    profile::{ProfileCloneInput, ProfileShareInput, ProfileShareRevokeInput},
    team::Team,
    workspace::Workspace,
};
use buglogin_sync::coordination::CoordinationAuth;
use serde_json::Value;

use crate::clock::unix_time_seconds;
use crate::commands::{Adapter, CommandMetadata, CommandRegistry};
use crate::services::{
    operations::reject_secrets,
    profile_creation::ProfileCreationService,
    profile_deletion::ProfileDeletionService,
    profile_hosted::HostedProfileService,
    proxy_credentials::{credential_version, next_proxy_revision},
};
use buglogin_network::proxy::{
    ProxyAssignmentRepository, ProxyDelete, ProxyRecord, ProxyUpdate, RepositoryError,
};

pub const ENABLED_BY_DEFAULT: bool = false;
static AUDIT_SEQUENCE: AtomicU64 = AtomicU64::new(1);
static MCP_INPUT_VALIDATORS: OnceLock<
    Result<BTreeMap<&'static str, jsonschema::Validator>, String>,
> = OnceLock::new();

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolMetadata {
    pub name: String,
    pub command_id: &'static str,
    pub capability: &'static str,
    pub action: Action,
    pub auth_required: bool,
    pub audit_required: bool,
    pub idempotency_required: bool,
    pub timeout_ms: u64,
    pub input_schema: Value,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DispatchMetadata {
    pub command: &'static CommandMetadata,
    pub scope_required: bool,
    pub secrets_returned: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DispatchError {
    UnknownTool,
    AdapterNotEnabled,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct McpRequest {
    pub tool_name: String,
    pub correlation_id: String,
    pub bearer_token: Option<String>,
    pub idempotency_key: Option<String>,
    pub input: Value,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AuditEventsListInput {
    workspace_id: WorkspaceId,
    cursor: Option<String>,
    limit: Option<usize>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionRequest<'a> {
    pub command: &'a CommandMetadata,
    pub correlation_id: String,
    pub principal: Principal,
    pub scope: ResourceScope,
    pub input: Value,
    pub idempotency_key: Option<String>,
    pub coordination_auth: Option<CoordinationAuth>,
    pub bearer_token: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IdempotencyClaim {
    Execute,
    Pending(Value),
    Replay(Value),
    ReplayError(Problem),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuditOutcome {
    Succeeded,
    Failed,
    Denied,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuditRecord {
    pub actor_id: Option<String>,
    pub command_id: &'static str,
    pub correlation_id: String,
    pub scope: ResourceScope,
    pub outcome: AuditOutcome,
    pub metadata: BTreeMap<String, String>,
}

pub trait McpExecutionPolicy {
    fn authenticate(
        &mut self,
        bearer_token: Option<&str>,
        required: bool,
        correlation_id: &str,
    ) -> Result<Principal, Problem>;

    fn resolve_scope(
        &mut self,
        principal: &Principal,
        command: &CommandMetadata,
        input: &Value,
    ) -> Result<ResourceScope, Problem>;

    fn authorize(
        &mut self,
        principal: &Principal,
        command: &CommandMetadata,
        scope: &ResourceScope,
    ) -> Result<(), Problem>;

    fn claim_idempotency(
        &mut self,
        principal: &Principal,
        command: &CommandMetadata,
        scope: &ResourceScope,
        key: &str,
        input: &Value,
    ) -> Result<IdempotencyClaim, Problem>;

    fn audit(&mut self, record: AuditRecord) -> Result<(), Problem>;

    fn append_audit_event(&mut self, _record: AuditRecord) -> Result<String, Problem> {
        Err(Problem::new(
            "audit_unavailable",
            "Audit event persistence is unavailable",
            503,
            "mcp",
        ))
    }

    fn complete_idempotency(
        &mut self,
        principal: &Principal,
        command: &CommandMetadata,
        result: &Result<Value, Problem>,
    ) -> Result<(), Problem>;

    fn abandon_idempotency(&mut self) -> Result<(), Problem>;

    fn complete_idempotency_with_audit(
        &mut self,
        principal: &Principal,
        command: &CommandMetadata,
        result: &Result<Value, Problem>,
        audit: AuditRecord,
    ) -> Result<(), Problem> {
        self.complete_idempotency(principal, command, result)?;
        self.audit(audit)
    }

    fn abandon_idempotency_with_audit(&mut self, audit: AuditRecord) -> Result<(), Problem> {
        self.abandon_idempotency()?;
        self.audit(audit)
    }

    fn execute_and_complete<E, F>(
        &mut self,
        executor: &mut E,
        request: ExecutionRequest<'_>,
        complete: F,
    ) -> Result<Value, Problem>
    where
        Self: Sized,
        E: McpExecutor,
        F: FnOnce(&mut Self, Result<Value, Problem>) -> Result<Value, Problem>,
    {
        complete(self, executor.execute(request))
    }
}

#[async_trait]
pub trait McpExecutor: Send {
    fn execute(&mut self, request: ExecutionRequest<'_>) -> Result<Value, Problem>;

    fn execute_in_transaction(&mut self, request: ExecutionRequest<'_>) -> Result<Value, Problem> {
        self.execute(request)
    }

    async fn execute_async(&mut self, request: ExecutionRequest<'_>) -> Result<Value, Problem> {
        self.execute(request)
    }
}

pub struct McpDispatcher<P, E> {
    policy: P,
    executor: E,
    enabled: bool,
}

impl<P, E> McpDispatcher<P, E>
where
    P: McpExecutionPolicy,
    E: McpExecutor,
{
    pub fn new(policy: P, executor: E, enabled: bool) -> Self {
        Self {
            policy,
            executor,
            enabled,
        }
    }

    pub fn dispatch(&mut self, request: McpRequest) -> Result<Value, Problem> {
        let correlation_id = request.correlation_id.clone();
        self.dispatch_inner(request)
            .map_err(|error| with_correlation_id(error, &correlation_id))
    }

    fn dispatch_inner(&mut self, request: McpRequest) -> Result<Value, Problem> {
        if !self.enabled {
            return Err(problem("mcp_disabled", "MCP execution is disabled", 503));
        }

        let command = dispatch_metadata(&request.tool_name)
            .map_err(|error| dispatch_problem(error, &request.correlation_id))?
            .command;
        validate_mcp_input(command, &request.input)?;
        let principal = match self.policy.authenticate(
            request.bearer_token.as_deref(),
            command.auth_required,
            &request.correlation_id,
        ) {
            Ok(principal) => principal,
            Err(error) => {
                self.audit_denied(
                    command,
                    &request.correlation_id,
                    ResourceScope::global(),
                    None,
                );
                return Err(error);
            }
        };
        let actor_id = principal
            .account_id
            .as_ref()
            .map(|account_id| account_id.as_str().to_owned());
        let scope = match self
            .policy
            .resolve_scope(&principal, command, &request.input)
        {
            Ok(scope) => scope,
            Err(error) => {
                self.audit_denied(
                    command,
                    &request.correlation_id,
                    ResourceScope::global(),
                    actor_id.clone(),
                );
                return Err(error);
            }
        };
        if let Err(error) = self.policy.authorize(&principal, command, &scope) {
            self.audit_denied(command, &request.correlation_id, scope, actor_id.clone());
            return Err(error);
        }

        let idempotency_key = match (
            command.idempotency_required,
            request.idempotency_key.as_deref(),
        ) {
            (true, None) => {
                let error = problem("idempotency_key_required", "Idempotency key required", 400);
                self.audit_denied(command, &request.correlation_id, scope, actor_id.clone());
                return Err(error);
            }
            (true, Some(key)) => {
                if !valid_idempotency_key(key) {
                    let error = problem("invalid_idempotency_key", "Invalid idempotency key", 400);
                    self.audit_denied(command, &request.correlation_id, scope, actor_id.clone());
                    return Err(error);
                }
                Some(key.to_owned())
            }
            (false, key) => key.map(str::to_owned),
        };

        let claim = match idempotency_key.as_deref() {
            Some(key) => {
                self.policy
                    .claim_idempotency(&principal, command, &scope, key, &request.input)
            }
            None => Ok(IdempotencyClaim::Execute),
        };
        let claim = match claim {
            Ok(claim) => claim,
            Err(error) => {
                self.audit_denied(command, &request.correlation_id, scope, actor_id.clone());
                return Err(error);
            }
        };
        if let IdempotencyClaim::Replay(value) = claim {
            reject_mcp_output_secrets(command, &value)?;
            self.policy.audit(AuditRecord {
                actor_id,
                command_id: command.id.as_str(),
                correlation_id: request.correlation_id,
                scope,
                outcome: AuditOutcome::Succeeded,
                metadata: BTreeMap::from([("idempotency".to_owned(), "replay".to_owned())]),
            })?;
            return Ok(value);
        }
        if let IdempotencyClaim::Pending(value) = claim {
            reject_mcp_output_secrets(command, &value)?;
            return Ok(value);
        }
        if let IdempotencyClaim::ReplayError(error) = claim {
            self.policy.audit(AuditRecord {
                actor_id,
                command_id: command.id.as_str(),
                correlation_id: request.correlation_id,
                scope,
                outcome: AuditOutcome::Failed,
                metadata: BTreeMap::from([("idempotency".to_owned(), "replay".to_owned())]),
            })?;
            return Err(error);
        }

        let correlation_id = request.correlation_id.clone();
        let execution_request = ExecutionRequest {
            command,
            correlation_id: correlation_id.clone(),
            principal: principal.clone(),
            scope: scope.clone(),
            input: request.input,
            idempotency_key,
            coordination_auth: None,
            bearer_token: request.bearer_token.clone(),
        };
        let policy = &mut self.policy;
        let executor = &mut self.executor;
        let error_correlation_id = correlation_id.clone();
        policy.execute_and_complete(executor, execution_request, |policy, result| match result {
            Ok(value) => {
                if let Err(error) = reject_mcp_output_secrets(command, &value) {
                    policy.complete_idempotency_with_audit(
                        &principal,
                        command,
                        &Err(error.clone()),
                        AuditRecord {
                            actor_id: actor_id.clone(),
                            command_id: command.id.as_str(),
                            correlation_id: correlation_id.clone(),
                            scope: scope.clone(),
                            outcome: AuditOutcome::Denied,
                            metadata: BTreeMap::from([(
                                "output_policy".to_owned(),
                                "rejected".to_owned(),
                            )]),
                        },
                    )?;
                    return Err(error);
                }
                policy.complete_idempotency_with_audit(
                    &principal,
                    command,
                    &Ok(value.clone()),
                    AuditRecord {
                        actor_id,
                        command_id: command.id.as_str(),
                        correlation_id,
                        scope,
                        outcome: AuditOutcome::Succeeded,
                        metadata: BTreeMap::new(),
                    },
                )?;
                Ok(value)
            }
            Err(error) => {
                let audit = AuditRecord {
                    actor_id,
                    command_id: command.id.as_str(),
                    correlation_id: error_correlation_id,
                    scope,
                    outcome: AuditOutcome::Failed,
                    metadata: BTreeMap::new(),
                };
                if should_abandon_mcp_claim(&error) {
                    policy.abandon_idempotency_with_audit(audit)?;
                } else {
                    policy.complete_idempotency_with_audit(
                        &principal,
                        command,
                        &Err(error.clone()),
                        audit,
                    )?;
                }
                Err(error)
            }
        })
    }

    pub async fn dispatch_async(&mut self, request: McpRequest) -> Result<Value, Problem> {
        let correlation_id = request.correlation_id.clone();
        self.dispatch_async_inner(request)
            .await
            .map_err(|error| with_correlation_id(error, &correlation_id))
    }

    async fn dispatch_async_inner(&mut self, request: McpRequest) -> Result<Value, Problem> {
        let command = dispatch_metadata(&request.tool_name)
            .map_err(|error| dispatch_problem(error, &request.correlation_id))?
            .command;
        if command.id.as_str() == "migration.package.activate" {
            return self.dispatch_package_activation(request, command).await;
        }
        if !matches!(command.id.as_str(), "events.stream" | "resource.lease.get")
            || command.idempotency_required
        {
            return self.dispatch(request);
        }
        if !self.enabled {
            return Err(problem("mcp_disabled", "MCP execution is disabled", 503));
        }
        validate_mcp_input(command, &request.input)?;
        let principal = match self.policy.authenticate(
            request.bearer_token.as_deref(),
            command.auth_required,
            &request.correlation_id,
        ) {
            Ok(principal) => principal,
            Err(error) => {
                self.audit_denied(
                    command,
                    &request.correlation_id,
                    ResourceScope::global(),
                    None,
                );
                return Err(error);
            }
        };
        let actor_id = principal
            .account_id
            .as_ref()
            .map(|account_id| account_id.as_str().to_owned());
        let scope = match self
            .policy
            .resolve_scope(&principal, command, &request.input)
        {
            Ok(scope) => scope,
            Err(error) => {
                self.audit_denied(
                    command,
                    &request.correlation_id,
                    ResourceScope::global(),
                    actor_id,
                );
                return Err(error);
            }
        };
        if let Err(error) = self.policy.authorize(&principal, command, &scope) {
            self.audit_denied(command, &request.correlation_id, scope, actor_id);
            return Err(error);
        }
        let account_id = principal
            .account_id
            .as_ref()
            .ok_or_else(|| problem("authentication_required", "Authentication required", 401))?;
        let bearer = request
            .bearer_token
            .as_deref()
            .ok_or_else(|| problem("authentication_required", "Authentication required", 401))?;
        let coordination_auth = match CoordinationAuth::new(account_id.as_str(), bearer) {
            Ok(auth) => auth,
            Err(error) => {
                self.audit_denied(command, &request.correlation_id, scope, actor_id);
                return Err(error);
            }
        };
        let result = self
            .executor
            .execute_async(ExecutionRequest {
                command,
                correlation_id: request.correlation_id.clone(),
                principal: principal.clone(),
                scope: scope.clone(),
                input: request.input,
                idempotency_key: request.idempotency_key,
                coordination_auth: Some(coordination_auth),
                bearer_token: request.bearer_token,
            })
            .await;
        match result {
            Ok(value) => {
                if let Err(error) = reject_mcp_output_secrets(command, &value) {
                    self.audit_denied(command, &request.correlation_id, scope, actor_id.clone());
                    return Err(error);
                }
                self.policy.audit(AuditRecord {
                    actor_id,
                    command_id: command.id.as_str(),
                    correlation_id: request.correlation_id,
                    scope,
                    outcome: AuditOutcome::Succeeded,
                    metadata: BTreeMap::new(),
                })?;
                Ok(value)
            }
            Err(error) => {
                self.policy.audit(AuditRecord {
                    actor_id,
                    command_id: command.id.as_str(),
                    correlation_id: request.correlation_id,
                    scope,
                    outcome: AuditOutcome::Failed,
                    metadata: BTreeMap::new(),
                })?;
                Err(error)
            }
        }
    }

    async fn dispatch_package_activation(
        &mut self,
        request: McpRequest,
        command: &'static CommandMetadata,
    ) -> Result<Value, Problem> {
        if !self.enabled {
            return Err(problem("mcp_disabled", "MCP execution is disabled", 503));
        }
        validate_mcp_input(command, &request.input)?;
        let principal = match self.policy.authenticate(
            request.bearer_token.as_deref(),
            command.auth_required,
            &request.correlation_id,
        ) {
            Ok(principal) => principal,
            Err(error) => {
                self.audit_denied(
                    command,
                    &request.correlation_id,
                    ResourceScope::global(),
                    None,
                );
                return Err(error);
            }
        };
        let actor_id = principal
            .account_id
            .as_ref()
            .map(|id| id.as_str().to_owned());
        let scope = match self
            .policy
            .resolve_scope(&principal, command, &request.input)
        {
            Ok(scope) => scope,
            Err(error) => {
                self.audit_denied(
                    command,
                    &request.correlation_id,
                    ResourceScope::global(),
                    actor_id,
                );
                return Err(error);
            }
        };
        if let Err(error) = self.policy.authorize(&principal, command, &scope) {
            self.audit_denied(command, &request.correlation_id, scope, actor_id);
            return Err(error);
        }
        let idempotency_key = match request.idempotency_key.as_deref() {
            Some(key) if valid_idempotency_key(key) => key.to_owned(),
            _ => {
                self.audit_denied(command, &request.correlation_id, scope, actor_id);
                return Err(problem(
                    "idempotency_key_required",
                    "A valid idempotency key is required",
                    400,
                ));
            }
        };
        let claim = match self.policy.claim_idempotency(
            &principal,
            command,
            &scope,
            &idempotency_key,
            &request.input,
        ) {
            Ok(claim) => claim,
            Err(error) => {
                self.audit_denied(command, &request.correlation_id, scope, actor_id);
                return Err(error);
            }
        };
        match claim {
            IdempotencyClaim::Replay(value) => {
                reject_mcp_output_secrets(command, &value)?;
                self.policy.audit(AuditRecord {
                    actor_id,
                    command_id: command.id.as_str(),
                    correlation_id: request.correlation_id,
                    scope,
                    outcome: AuditOutcome::Succeeded,
                    metadata: BTreeMap::from([("idempotency".into(), "replay".into())]),
                })?;
                return Ok(value);
            }
            IdempotencyClaim::Pending(value) => {
                reject_mcp_output_secrets(command, &value)?;
                return Ok(value);
            }
            IdempotencyClaim::ReplayError(error) => {
                self.policy.audit(AuditRecord {
                    actor_id,
                    command_id: command.id.as_str(),
                    correlation_id: request.correlation_id,
                    scope,
                    outcome: AuditOutcome::Failed,
                    metadata: BTreeMap::from([("idempotency".into(), "replay".into())]),
                })?;
                return Err(error);
            }
            IdempotencyClaim::Execute => {}
        }
        let result = self
            .executor
            .execute_async(ExecutionRequest {
                command,
                correlation_id: request.correlation_id.clone(),
                principal: principal.clone(),
                scope: scope.clone(),
                input: request.input,
                idempotency_key: Some(idempotency_key),
                coordination_auth: None,
                bearer_token: request.bearer_token,
            })
            .await;
        match result {
            Ok(value) => {
                if let Err(error) = reject_mcp_output_secrets(command, &value) {
                    self.policy.complete_idempotency_with_audit(
                        &principal,
                        command,
                        &Err(error.clone()),
                        AuditRecord {
                            actor_id: actor_id.clone(),
                            command_id: command.id.as_str(),
                            correlation_id: request.correlation_id.clone(),
                            scope: scope.clone(),
                            outcome: AuditOutcome::Denied,
                            metadata: BTreeMap::from([(
                                "output_policy".to_owned(),
                                "rejected".to_owned(),
                            )]),
                        },
                    )?;
                    return Err(error);
                }
                self.policy.complete_idempotency_with_audit(
                    &principal,
                    command,
                    &Ok(value.clone()),
                    AuditRecord {
                        actor_id,
                        command_id: command.id.as_str(),
                        correlation_id: request.correlation_id,
                        scope,
                        outcome: AuditOutcome::Succeeded,
                        metadata: BTreeMap::new(),
                    },
                )?;
                Ok(value)
            }
            Err(error) => {
                let audit = AuditRecord {
                    actor_id,
                    command_id: command.id.as_str(),
                    correlation_id: request.correlation_id,
                    scope,
                    outcome: AuditOutcome::Failed,
                    metadata: BTreeMap::new(),
                };
                if should_abandon_mcp_claim(&error) {
                    self.policy.abandon_idempotency_with_audit(audit)?;
                } else {
                    self.policy.complete_idempotency_with_audit(
                        &principal,
                        command,
                        &Err(error.clone()),
                        audit,
                    )?;
                }
                Err(error)
            }
        }
    }

    pub fn into_parts(self) -> (P, E) {
        (self.policy, self.executor)
    }

    fn audit_denied(
        &mut self,
        command: &CommandMetadata,
        correlation_id: &str,
        scope: ResourceScope,
        actor_id: Option<String>,
    ) {
        let _ = self.policy.audit(AuditRecord {
            actor_id,
            command_id: command.id.as_str(),
            correlation_id: correlation_id.to_owned(),
            scope,
            outcome: AuditOutcome::Denied,
            metadata: BTreeMap::new(),
        });
    }
}

fn should_abandon_mcp_claim(error: &Problem) -> bool {
    matches!(error.status, 401 | 403) || error.code == "authorization_state_changed"
}

pub fn tools() -> Vec<ToolMetadata> {
    CommandRegistry::all()
        .iter()
        .filter(|command| command.adapters.contains(&Adapter::Mcp))
        .map(tool_metadata)
        .collect()
}

pub fn dispatch_metadata(name: &str) -> Result<DispatchMetadata, DispatchError> {
    let command_id = name.replace('_', ".");
    let command = CommandRegistry::get(&command_id).ok_or(DispatchError::UnknownTool)?;
    if !command.adapters.contains(&Adapter::Mcp) {
        return Err(DispatchError::AdapterNotEnabled);
    }
    Ok(DispatchMetadata {
        command,
        scope_required: command.auth_required,
        secrets_returned: false,
    })
}

fn tool_metadata(command: &CommandMetadata) -> ToolMetadata {
    ToolMetadata {
        name: command.id.as_str().replace('.', "_"),
        command_id: command.id.as_str(),
        capability: command.capability,
        action: command.action,
        auth_required: command.auth_required,
        audit_required: command.audit_required,
        idempotency_required: command.idempotency_required,
        timeout_ms: command.timeout_ms,
        input_schema: mcp_input_schema_for_command(command)
            .expect("generated MCP input schema must exist for every MCP command"),
    }
}

fn mcp_input_schema_for_command(command: &CommandMetadata) -> Option<Value> {
    if command.id.as_str() == "profile.launch" {
        return Some(serde_json::json!({
            "type": "object",
            "required": ["profileId"],
            "properties": {
                "profileId": {
                    "type": "string",
                    "minLength": 1,
                    "maxLength": 128,
                    "pattern": "^[A-Za-z0-9][A-Za-z0-9._:-]*$"
                },
                "targetDeviceId": {
                    "type": "string",
                    "minLength": 8,
                    "maxLength": 128,
                    "pattern": "^[A-Za-z0-9][A-Za-z0-9._-]*$"
                },
                "profilesRoot": { "type": "string", "minLength": 1 },
                "extraArgs": { "type": "array", "items": { "type": "string" } }
            },
            "additionalProperties": false
        }));
    }
    if command.id.as_str() == "profile.stop" {
        return Some(serde_json::json!({
            "type": "object",
            "required": ["profileId"],
            "properties": {
                "profileId": {
                    "type": "string",
                    "minLength": 1,
                    "maxLength": 128,
                    "pattern": "^[A-Za-z0-9][A-Za-z0-9._:-]*$"
                },
                "targetDeviceId": {
                    "type": "string",
                    "minLength": 8,
                    "maxLength": 128,
                    "pattern": "^[A-Za-z0-9][A-Za-z0-9._-]*$"
                }
            },
            "additionalProperties": false
        }));
    }
    crate::commands::registry::mcp_input_schema(command.input_schema)
}

fn mcp_validator_key(command: &CommandMetadata) -> &'static str {
    if matches!(command.id.as_str(), "profile.launch" | "profile.stop") {
        command.id.as_str()
    } else {
        command.input_schema.as_str()
    }
}

fn validate_mcp_input(command: &CommandMetadata, input: &Value) -> Result<(), Problem> {
    let validators = MCP_INPUT_VALIDATORS.get_or_init(|| {
        let mut validators = BTreeMap::new();
        for command in CommandRegistry::all()
            .iter()
            .filter(|command| command.adapters.contains(&Adapter::Mcp))
        {
            let key = mcp_validator_key(command);
            if validators.contains_key(key) {
                continue;
            }
            let schema_value = mcp_input_schema_for_command(command)
                .ok_or_else(|| format!("missing generated schema for {}", command.id.as_str()))?;
            let validator = jsonschema::validator_for(&schema_value).map_err(|error| {
                format!(
                    "invalid generated schema for {}: {error}",
                    command.id.as_str()
                )
            })?;
            validators.insert(key, validator);
        }
        Ok(validators)
    });
    let validators = validators.as_ref().map_err(|error| {
        problem(
            "mcp_schema_unavailable",
            "MCP input schema is unavailable",
            500,
        )
        .with_detail(error.clone())
    })?;
    let validator = validators.get(mcp_validator_key(command)).ok_or_else(|| {
        problem(
            "mcp_schema_unavailable",
            "MCP input schema is unavailable",
            500,
        )
    })?;
    if let Err(error) = validator.validate(input) {
        let field = error.instance_path().to_string();
        return Err(problem(
            "mcp_invalid_input",
            "MCP input does not match the command contract",
            400,
        )
        .with_detail(format!(
            "Invalid input at {}",
            if field.is_empty() { "/" } else { &field }
        )));
    }
    Ok(())
}

fn reject_mcp_output_secrets(command: &CommandMetadata, value: &Value) -> Result<(), Problem> {
    if command.id.as_str() == "profile.manifest.get" {
        let mut checked = value.clone();
        remove_safe_manifest_metadata(&mut checked);
        return reject_secrets(&checked);
    }
    if !command.id.as_str().starts_with("proxy.") {
        if command.id.as_str() == "resource.lease.get" {
            let mut checked = value.clone();
            remove_lease_fencing_metadata(&mut checked);
            return reject_secrets(&checked);
        }
        return reject_secrets(value);
    }
    let mut checked = value.clone();
    remove_proxy_secret_metadata(&mut checked);
    reject_secrets(&checked)
}

fn remove_safe_manifest_metadata(value: &mut Value) {
    match value {
        Value::Object(values) => {
            if values
                .get("fencingToken")
                .is_some_and(|value| value.as_u64().is_some())
            {
                values.remove("fencingToken");
            }
            if values
                .get("credentialVersion")
                .and_then(Value::as_str)
                .is_some_and(|value| buglogin_domain::ids::CredentialId::from_str(value).is_ok())
            {
                values.remove("credentialVersion");
            }
            if values
                .get("credentialEnvelopeObject")
                .and_then(|value| {
                    serde_json::from_value::<buglogin_sync::manifest::ManifestObjectReference>(
                        value.clone(),
                    )
                    .ok()
                })
                .is_some()
            {
                values.remove("credentialEnvelopeObject");
            }
            for value in values.values_mut() {
                remove_safe_manifest_metadata(value);
            }
        }
        Value::Array(values) => {
            for value in values {
                remove_safe_manifest_metadata(value);
            }
        }
        _ => {}
    }
}

fn remove_lease_fencing_metadata(value: &mut Value) {
    match value {
        Value::Object(values) => {
            values.remove("fencingToken");
            for value in values.values_mut() {
                remove_lease_fencing_metadata(value);
            }
        }
        Value::Array(values) => {
            for value in values {
                remove_lease_fencing_metadata(value);
            }
        }
        _ => {}
    }
}

fn remove_proxy_secret_metadata(value: &mut Value) {
    match value {
        Value::Object(values) => {
            if values.get("hasCredential").is_some_and(Value::is_boolean) {
                values.remove("hasCredential");
            }
            if values
                .get("rotationConfigured")
                .is_some_and(Value::is_boolean)
            {
                values.remove("rotationConfigured");
            }
            for value in values.values_mut() {
                remove_proxy_secret_metadata(value);
            }
        }
        Value::Array(values) => {
            for value in values {
                remove_proxy_secret_metadata(value);
            }
        }
        _ => {}
    }
}

fn valid_idempotency_key(key: &str) -> bool {
    (16..=128).contains(&key.len()) && key.is_ascii()
}

fn dispatch_problem(error: DispatchError, correlation_id: &str) -> Problem {
    match error {
        DispatchError::UnknownTool => {
            problem("mcp_tool_not_found", "MCP tool not found", 404).with_detail("Unknown MCP tool")
        }
        DispatchError::AdapterNotEnabled => {
            problem("mcp_tool_unavailable", "MCP tool unavailable", 404)
        }
    }
    .with_instance(correlation_id)
}

fn problem(code: &str, title: &str, status: u16) -> Problem {
    Problem::new(code, title, status, "mcp")
}

fn with_correlation_id(mut error: Problem, correlation_id: &str) -> Problem {
    error.correlation_id = correlation_id.to_owned();
    error
}

fn runtime_operation_id(
    command_id: &str,
    actor_id: &str,
    workspace_id: Option<&WorkspaceId>,
    profile_id: &ProfileId,
    idempotency_key: Option<&str>,
) -> Result<String, Problem> {
    let idempotency_key = idempotency_key
        .ok_or_else(|| problem("idempotency_key_required", "Idempotency key required", 400))?;
    let digest = sha256_hash(&serde_json::json!({
        "actorId": actor_id,
        "workspaceId": workspace_id.map(WorkspaceId::as_str),
        "profileId": profile_id.as_str(),
        "commandId": command_id,
        "idempotencyKey": idempotency_key,
    }));
    Ok(format!("op_{}", &digest[..24]))
}

fn enqueue_runtime_dispatch(
    state: &crate::http::ApiState,
    principal: &Principal,
    scope: &ResourceScope,
    input: &Value,
    idempotency_key: Option<&str>,
    command_id: &str,
    correlation_id: &str,
) -> Result<Value, Problem> {
    let account_id = principal
        .account_id
        .as_ref()
        .ok_or_else(|| problem("authentication_required", "Authentication required", 401))?;
    let workspace_id = scope
        .workspace_id
        .as_ref()
        .ok_or_else(|| problem("workspace_missing", "Workspace is required", 400))?;
    let profile_id = scope
        .profile_id
        .as_ref()
        .ok_or_else(|| problem("profile_missing", "Profile is required", 400))?;
    let idempotency_key = idempotency_key
        .ok_or_else(|| problem("idempotency_key_required", "Idempotency key required", 400))?;
    let now = crate::clock::unix_time_seconds(correlation_id)?;
    let devices = state
        .auth
        .read()
        .map_err(|_| problem("auth_unavailable", "Authentication unavailable", 503))?
        .active_device_ids(account_id, now)?;
    let requested_device_id = input.get("targetDeviceId").and_then(Value::as_str);
    let device_id = if let Some(device_id) = requested_device_id {
        if !devices.contains(device_id) {
            return Err(problem(
                "device_not_available",
                "The selected desktop device is not active for this account",
                403,
            ));
        }
        device_id.to_owned()
    } else {
        match devices.len() {
        0 => {
            return Err(problem(
                "device_offline",
                "No active desktop device is signed in for this account",
                503,
            )
            .retryable(true));
        }
        1 => devices.into_iter().next().expect("one active device"),
        _ => {
            return Err(problem(
                "device_selection_required",
                "This account has multiple active desktop devices; select a target device",
                409,
            ));
        }
        }
    };
    let expires_at = now.checked_add(120).ok_or_else(|| {
        problem(
            "runtime_job_input_invalid",
            "Runtime dispatch expiry is invalid",
            400,
        )
    })?;
    let job = state.runtime_dispatch.enqueue(
        crate::services::runtime_dispatch::EnqueueRuntimeDispatch {
            command_id: command_id.into(),
            workspace_id: workspace_id.as_str().into(),
            device_id,
            profile_id: profile_id.as_str().into(),
            requested_by: account_id.as_str().into(),
            idempotency_key: idempotency_key.into(),
            now,
            expires_at,
            input: serde_json::json!({}),
        },
        None,
    )?;
    Ok(serde_json::json!({
        "operationId": job.operation_id,
        "jobId": job.job_id,
        "status": job.state,
    }))
}

fn serialization_problem(error: serde_json::Error) -> Problem {
    Problem::new("serialization_failed", error.to_string(), 400, "mcp")
}

fn sha256_hash(value: &serde_json::Value) -> String {
    use sha2::{Digest, Sha256};
    let bytes = serde_json::to_vec(value).unwrap_or_default();
    format!("{:x}", Sha256::digest(&bytes))
}

fn required_id<T, E>(
    input: &Value,
    field: &str,
    parse: impl FnOnce(&str) -> Result<T, E>,
) -> Result<T, Problem> {
    let value = input.get(field).and_then(Value::as_str).ok_or_else(|| {
        Problem::new("scope_required", format!("{field} is required"), 400, "mcp")
    })?;
    parse(value)
        .map_err(|_| Problem::new("invalid_scope", format!("{field} is invalid"), 400, "mcp"))
}

enum ParsedProfileCreate {
    Command(ProfileCreateInput),
    Legacy(Profile),
}

fn parse_profile_create_request(input: &Value) -> Result<ParsedProfileCreate, Problem> {
    if let Ok(command) = serde_json::from_value::<ProfileCreateInput>(input.clone()) {
        return Ok(ParsedProfileCreate::Command(command));
    }
    serde_json::from_value::<Profile>(input.clone())
        .map(ParsedProfileCreate::Legacy)
        .map_err(serialization_problem)
}

fn resolve_scope(
    principal: &Principal,
    command: &CommandMetadata,
    input: &Value,
) -> Result<ResourceScope, Problem> {
    match command.id.as_str() {
        "system.health.get" | "runtime.catalog.get" => Ok(ResourceScope::global()),
        "account.get" | "organization.list" | "profile.list" | "proxy.parse" => principal
            .account_id
            .clone()
            .map(ResourceScope::account)
            .ok_or_else(|| problem("account_missing", "Account is missing", 401)),
        "proxy.list" | "proxy.get" | "proxy.create" | "proxy.update" | "proxy.delete" => Ok(
            ResourceScope::workspace(required_id(input, "workspaceId", WorkspaceId::from_str)?),
        ),
        "cookie.list" | "cookie.stage" | "cookie.import" => Ok(ResourceScope {
            workspace_id: Some(required_id(input, "workspaceId", WorkspaceId::from_str)?),
            profile_id: Some(required_id(input, "profileId", ProfileId::from_str)?),
            ..ResourceScope::default()
        }),
        "cookie.copy" => Ok(ResourceScope {
            workspace_id: Some(required_id(
                input,
                "destinationWorkspaceId",
                WorkspaceId::from_str,
            )?),
            profile_id: Some(required_id(
                input,
                "destinationProfileId",
                ProfileId::from_str,
            )?),
            ..ResourceScope::default()
        }),
        "organization.create" => {
            let parsed: OrganizationInput =
                serde_json::from_value(input.clone()).map_err(serialization_problem)?;
            Ok(ResourceScope {
                account_id: Some(parsed.organization.account_id),
                organization_id: Some(parsed.organization.id),
                ..ResourceScope::default()
            })
        }
        "organization.get" | "organization.record.get" | "organization.update" => {
            Ok(ResourceScope {
                account_id: principal.account_id.clone(),
                organization_id: Some(required_id(
                    input,
                    "organizationId",
                    OrganizationId::from_str,
                )?),
                ..ResourceScope::default()
            })
        }
        "workspace.list" => Ok(ResourceScope {
            organization_id: Some(required_id(
                input,
                "organizationId",
                OrganizationId::from_str,
            )?),
            ..ResourceScope::default()
        }),
        "workspace.create" => {
            let workspace_input = input.get("workspace").ok_or_else(|| {
                Problem::new("scope_required", "workspace is required", 400, "mcp")
            })?;
            Ok(ResourceScope {
                organization_id: Some(required_id(
                    input,
                    "organizationId",
                    OrganizationId::from_str,
                )?),
                workspace_id: Some(required_id(workspace_input, "id", WorkspaceId::from_str)?),
                ..ResourceScope::default()
            })
        }
        "workspace.get" | "workspace.record.get" | "workspace.update" => Ok(ResourceScope {
            organization_id: Some(required_id(
                input,
                "organizationId",
                OrganizationId::from_str,
            )?),
            workspace_id: Some(required_id(input, "workspaceId", WorkspaceId::from_str)?),
            ..ResourceScope::default()
        }),
        "group.list"
        | "group.create"
        | "extension.group.list"
        | "extension.group.create"
        | "group.share.list"
        | "group.share.create"
        | "group.share.revoke"
        | "team.list"
        | "team.create"
        | "member.list"
        | "member.invite"
        | "resource.list"
        | "grant.list"
        | "grant.list.page" => Ok(ResourceScope::workspace(required_id(
            input,
            "workspaceId",
            WorkspaceId::from_str,
        )?)),
        "group.get"
        | "group.update"
        | "group.delete"
        | "extension.group.get"
        | "extension.group.update"
        | "extension.group.delete" => Ok(ResourceScope::workspace(required_id(
            input,
            "workspaceId",
            WorkspaceId::from_str,
        )?)),
        "profile.groups.replace" | "profile.extension.group.set" => Ok(ResourceScope {
            workspace_id: Some(required_id(input, "workspaceId", WorkspaceId::from_str)?),
            profile_id: Some(required_id(input, "profileId", ProfileId::from_str)?),
            ..ResourceScope::default()
        }),
        "profile.update" => Ok(ResourceScope {
            workspace_id: Some(required_id(input, "workspaceId", WorkspaceId::from_str)?),
            profile_id: Some(required_id(input, "profileId", ProfileId::from_str)?),
            ..ResourceScope::default()
        }),
        "profile.proxy.assignment.get" | "profile.proxy.assignment.set" => Ok(ResourceScope {
            workspace_id: Some(required_id(input, "workspaceId", WorkspaceId::from_str)?),
            profile_id: Some(required_id(input, "profileId", ProfileId::from_str)?),
            ..ResourceScope::default()
        }),
        "resource.register" => Ok(ResourceScope::workspace(required_id(
            input.get("resource").unwrap_or(&Value::Null),
            "workspaceId",
            WorkspaceId::from_str,
        )?)),
        "audit.events.list" => {
            let query: AuditEventsListInput =
                serde_json::from_value(input.clone()).map_err(serialization_problem)?;
            Ok(ResourceScope::workspace(query.workspace_id))
        }
        "team.get" | "team.update" => Ok(ResourceScope {
            workspace_id: Some(required_id(input, "workspaceId", WorkspaceId::from_str)?),
            team_id: Some(required_id(input, "teamId", TeamId::from_str)?),
            ..ResourceScope::default()
        }),
        "member.get" | "member.activate" | "member.suspend" | "member.role.update" => {
            Ok(ResourceScope {
                workspace_id: Some(required_id(input, "workspaceId", WorkspaceId::from_str)?),
                member_id: Some(required_id(input, "memberId", MemberId::from_str)?),
                ..ResourceScope::default()
            })
        }
        "resource.get" => Ok(ResourceScope {
            workspace_id: Some(required_id(input, "workspaceId", WorkspaceId::from_str)?),
            resource_id: Some(required_id(input, "resourceId", ResourceId::from_str)?),
            ..ResourceScope::default()
        }),
        "resource.lease.get" | "profile.manifest.get" => Ok(ResourceScope::resource(
            required_id(input, "workspaceId", WorkspaceId::from_str)?,
            required_id(input, "resourceId", ResourceId::from_str)?,
        )),
        "grant.create" => {
            let grant: buglogin_domain::grant::ResourceGrant =
                serde_json::from_value(input.clone()).map_err(serialization_problem)?;
            Ok(ResourceScope::resource(
                grant.workspace_id,
                grant.resource_id,
            ))
        }
        "grant.revoke" => Ok(ResourceScope::workspace(required_id(
            input,
            "workspaceId",
            WorkspaceId::from_str,
        )?)),
        "profile.create" => {
            let (requested_id, workspace_id) = match parse_profile_create_request(input)? {
                ParsedProfileCreate::Command(input) => (
                    input
                        .id
                        .as_deref()
                        .map(ProfileId::from_str)
                        .transpose()
                        .map_err(|_| problem("invalid_scope", "profile ID is invalid", 400))?,
                    input
                        .workspace_id
                        .as_deref()
                        .map(WorkspaceId::from_str)
                        .transpose()
                        .map_err(|_| problem("invalid_scope", "workspaceId is invalid", 400))?
                        .or_else(|| {
                            (principal.workspace_ids.len() == 1)
                                .then(|| principal.workspace_ids.iter().next().cloned())
                                .flatten()
                        }),
                ),
                ParsedProfileCreate::Legacy(profile) => (
                    Some(profile.id),
                    input
                        .get("workspaceId")
                        .and_then(Value::as_str)
                        .map(WorkspaceId::from_str)
                        .transpose()
                        .map_err(|_| problem("invalid_scope", "workspaceId is invalid", 400))?
                        .or_else(|| {
                            (principal.workspace_ids.len() == 1)
                                .then(|| principal.workspace_ids.iter().next().cloned())
                                .flatten()
                        }),
                ),
            };
            Ok(ResourceScope {
                workspace_id,
                profile_id: requested_id,
                ..ResourceScope::default()
            })
        }
        "profile.clone" => Ok(ResourceScope {
            profile_id: Some(required_id(input, "sourceProfileId", ProfileId::from_str)?),
            ..ResourceScope::default()
        }),
        "profile.share" | "profile.share.revoke" => Ok(ResourceScope {
            profile_id: Some(required_id(input, "profileId", ProfileId::from_str)?),
            ..ResourceScope::default()
        }),
        "profile.get" | "profile.launch" | "profile.stop" | "profile.delete"
        | "profile.archive" | "profile.restore" => Ok(ResourceScope {
            profile_id: Some(required_id(input, "profileId", ProfileId::from_str)?),
            ..ResourceScope::default()
        }),
        "operation.get" => Ok(ResourceScope::global()),
        "events.stream" => Ok(ResourceScope::workspace(required_id(
            input,
            "workspaceId",
            WorkspaceId::from_str,
        )?)),
        _ => Ok(ResourceScope::global()),
    }
}

use buglogin_domain::member::Member;
use buglogin_domain::resource::Resource;

pub struct ProductionMcpPolicy {
    state: crate::http::ApiState,
    claim: Option<buglogin_storage::control::ControlIdempotencyClaimToken>,
}

impl ProductionMcpPolicy {
    pub fn new(state: crate::http::ApiState) -> Self {
        Self { state, claim: None }
    }

    fn persist_idempotency_result(
        &mut self,
        principal: &Principal,
        command: &CommandMetadata,
        result: &Result<Value, Problem>,
        audit_allowed: bool,
        detail: Value,
    ) -> Result<(), Problem> {
        let Some(claim) = self.claim.as_ref().cloned() else {
            return Ok(());
        };
        let actor = principal
            .account_id
            .as_ref()
            .map(|id| id.as_str().to_owned())
            .unwrap_or_else(|| "anonymous".to_owned());
        let (status, body) = match result {
            Ok(value) => (200, value.clone()),
            Err(error) => (
                error.status,
                serde_json::to_value(error).map_err(serialization_problem)?,
            ),
        };
        self.state.complete_mcp_idempotency(
            claim,
            command.id.as_str(),
            actor,
            audit_allowed,
            matches!(
                command.kind,
                crate::commands::CommandKind::Command | crate::commands::CommandKind::Operation
            ),
            detail,
            status,
            body,
        )?;
        self.claim = None;
        Ok(())
    }
}

impl McpExecutionPolicy for ProductionMcpPolicy {
    fn execute_and_complete<E, F>(
        &mut self,
        executor: &mut E,
        request: ExecutionRequest<'_>,
        complete: F,
    ) -> Result<Value, Problem>
    where
        Self: Sized,
        E: McpExecutor,
        F: FnOnce(&mut Self, Result<Value, Problem>) -> Result<Value, Problem>,
    {
        let is_mutation = matches!(
            request.command.kind,
            crate::commands::CommandKind::Command | crate::commands::CommandKind::Operation
        );
        if !is_mutation {
            let snapshot_gate = std::sync::Arc::clone(&self.state.control_snapshot_gate);
            let snapshot = match snapshot_gate.try_read() {
                Ok(guard) => guard,
                Err(_) => {
                    let _ = self.abandon_idempotency();
                    return Err(Problem::new(
                        "control_snapshot_busy",
                        "A control snapshot is being updated; retry the command",
                        503,
                        "mcp",
                    )
                    .retryable(true));
                }
            };
            if let Err(error) = self.state.refresh_control_aggregate() {
                drop(snapshot);
                return complete(self, Err(error));
            }
            let result = executor.execute(request);
            drop(snapshot);
            return complete(self, result);
        }

        let snapshot_gate = std::sync::Arc::clone(&self.state.control_snapshot_gate);
        let _snapshot_gate = match snapshot_gate.try_write() {
            Ok(guard) => guard,
            Err(_) => {
                let _ = self.abandon_idempotency();
                return Err(Problem::new(
                    "control_snapshot_busy",
                    "A profile snapshot is in progress; retry the command",
                    503,
                    "mcp",
                )
                .retryable(true));
            }
        };
        let mutation_lock = std::sync::Arc::clone(self.state.control_mutation());
        let mutation_lock_result = mutation_lock.lock();
        let _mutation_lock = match mutation_lock_result {
            Ok(guard) => guard,
            Err(error) => {
                drop(error.into_inner());
                let _ = self.abandon_idempotency();
                return Err(Problem::new(
                    "control_unavailable",
                    "Control state unavailable",
                    503,
                    "mcp",
                ));
            }
        };

        complete(self, executor.execute_in_transaction(request))
    }

    fn authenticate(
        &mut self,
        bearer_token: Option<&str>,
        required: bool,
        correlation_id: &str,
    ) -> Result<Principal, Problem> {
        let token = match bearer_token {
            Some(t) => t,
            None => {
                if required {
                    return Err(Problem::new(
                        "authentication_required",
                        "Authentication required",
                        401,
                        "mcp",
                    ));
                } else {
                    return Ok(Principal::default());
                }
            }
        };
        self.state
            .authenticate_token(token, unix_time_seconds(correlation_id)?)
    }

    fn resolve_scope(
        &mut self,
        principal: &Principal,
        command: &CommandMetadata,
        input: &Value,
    ) -> Result<ResourceScope, Problem> {
        if command.id.as_str() == "operation.get" {
            self.state.refresh_control_aggregate()?;
            let operation_id = input
                .get("operationId")
                .and_then(Value::as_str)
                .ok_or_else(|| problem("scope_required", "operationId is required", 400))?;
            return self
                .state
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
                .map(|operation| operation.scope.clone())
                .ok_or_else(|| problem("operation_not_found", "Operation not found", 404));
        }
        if command.id.as_str() == "grant.revoke" {
            let grant_id = required_id(input, "grantId", GrantId::from_str)?;
            let grants = self.state.grants()?;
            let grant = self
                .state
                .resources
                .read()
                .map_err(|_| problem("resource_unavailable", "Resource service unavailable", 503))?
                .get_grant(principal, &grants, &grant_id)?;
            return Ok(ResourceScope::resource(
                grant.workspace_id,
                grant.resource_id,
            ));
        }
        if matches!(
            command.id.as_str(),
            "profile.groups.replace" | "profile.extension.group.set"
        ) {
            let workspace_id = required_id(input, "workspaceId", WorkspaceId::from_str)?;
            let profile_id = required_id(input, "profileId", ProfileId::from_str)?;
            let binding = self
                .state
                .profile_resource_record(&profile_id)?
                .filter(|binding| binding.workspace_id == workspace_id)
                .ok_or_else(|| problem("binding_not_found", "Profile binding not found", 404))?;
            return Ok(ResourceScope {
                workspace_id: Some(workspace_id),
                resource_id: Some(binding.resource_id),
                profile_id: Some(profile_id),
                ..ResourceScope::default()
            });
        }
        let mut scope = resolve_scope(principal, command, input)?;
        if command.id.as_str() == "profile.manifest.get"
            && let (Some(workspace_id), Some(resource_id)) =
                (scope.workspace_id.as_ref(), scope.resource_id.as_ref())
            && let Some(binding) = self
                .state
                .profile_resource_for_resource(workspace_id, resource_id)?
        {
            scope.profile_id = Some(binding.profile_id);
        }
        if !self.state.local_authority_enabled()
            && matches!(
                command.id.as_str(),
                "profile.get"
                    | "profile.launch"
                    | "profile.stop"
                    | "profile.delete"
                    | "profile.archive"
                    | "profile.restore"
                    | "profile.clone"
                    | "profile.share"
                    | "profile.share.revoke"
            )
        {
            let profile_id = scope
                .profile_id
                .as_ref()
                .ok_or_else(|| problem("profile_missing", "Profile is missing from scope", 400))?;
            if let Some(binding) = self.state.profile_resource_record(profile_id)? {
                scope.workspace_id = Some(binding.workspace_id);
                scope.resource_id = Some(binding.resource_id);
            } else if matches!(command.id.as_str(), "profile.archive" | "profile.restore") {
                return Err(problem(
                    "binding_not_found",
                    "Profile binding not found",
                    404,
                ));
            }
        }
        Ok(scope)
    }

    fn authorize(
        &mut self,
        principal: &Principal,
        command: &CommandMetadata,
        scope: &ResourceScope,
    ) -> Result<(), Problem> {
        if !command.auth_required || command.id.as_str() == "profile.list" {
            return Ok(());
        }
        let grants = self.state.grants()?;
        let capability = Capability::new(command.capability)
            .map_err(|_| Problem::new("invalid_capability", "Invalid capability", 500, "mcp"))?;
        if !self.state.local_authority_enabled()
            && command.id.as_str().starts_with("profile.")
            && let Some(profile_id) = scope.profile_id.as_ref()
        {
            return if crate::services::profile_hosted::HostedProfileService::authorized(
                &self.state,
                principal,
                &grants,
                profile_id,
                command.action,
                command.capability,
            )? {
                Ok(())
            } else {
                Err(Problem::new("forbidden", "Permission denied", 403, "mcp"))
            };
        }
        if buglogin_domain::permissions::is_allowed(
            principal,
            &capability,
            command.action,
            scope,
            &grants,
        ) {
            Ok(())
        } else {
            Err(Problem::new("forbidden", "Permission denied", 403, "mcp"))
        }
    }

    fn claim_idempotency(
        &mut self,
        principal: &Principal,
        command: &CommandMetadata,
        scope: &ResourceScope,
        key: &str,
        input: &Value,
    ) -> Result<IdempotencyClaim, Problem> {
        let actor = principal
            .account_id
            .as_ref()
            .map(|id| id.as_str())
            .unwrap_or("anonymous");
        let input_hash = sha256_hash(input);
        let operation_id = format!(
            "mcp_{}",
            &sha256_hash(&serde_json::json!({
                "actor": actor,
                "scope": scope,
                "command": command.id.as_str(),
                "key": key,
            }))[..24]
        );
        match self.state.claim_mcp_idempotency(
            actor,
            scope,
            command.id.as_str(),
            key,
            &input_hash,
            &operation_id,
        )? {
            buglogin_storage::control::ControlIdempotencyClaim::Claimed { claim } => {
                self.claim = Some(claim);
                Ok(IdempotencyClaim::Execute)
            }
            buglogin_storage::control::ControlIdempotencyClaim::Pending { operation_id } => {
                self.claim = None;
                Ok(IdempotencyClaim::Pending(serde_json::json!({
                    "status": "in_progress",
                    "operationId": operation_id,
                })))
            }
            buglogin_storage::control::ControlIdempotencyClaim::Replay { result } => {
                self.claim = None;
                if result.response.status >= 400 {
                    let error = serde_json::from_value(result.response.body).map_err(|_| {
                        problem(
                            "idempotency_corrupt",
                            "Stored idempotency result is invalid",
                            500,
                        )
                    })?;
                    Ok(IdempotencyClaim::ReplayError(error))
                } else {
                    Ok(IdempotencyClaim::Replay(result.response.body))
                }
            }
        }
    }

    fn complete_idempotency(
        &mut self,
        principal: &Principal,
        command: &CommandMetadata,
        result: &Result<Value, Problem>,
    ) -> Result<(), Problem> {
        self.persist_idempotency_result(
            principal,
            command,
            result,
            true,
            serde_json::json!({ "adapter": "mcp" }),
        )
    }

    fn complete_idempotency_with_audit(
        &mut self,
        principal: &Principal,
        command: &CommandMetadata,
        result: &Result<Value, Problem>,
        audit: AuditRecord,
    ) -> Result<(), Problem> {
        let detail = serde_json::json!({
            "adapter": "mcp",
            "correlationId": audit.correlation_id.clone(),
            "scope": audit.scope.clone(),
            "outcome": format!("{:?}", audit.outcome),
            "metadata": audit.metadata.clone(),
        });
        let audit_allowed = audit.outcome != AuditOutcome::Denied;
        let apply_mutations = matches!(
            command.kind,
            crate::commands::CommandKind::Command | crate::commands::CommandKind::Operation
        );
        if !apply_mutations {
            if let Err(error) = self.audit(audit) {
                self.abandon_idempotency()?;
                return Err(error);
            }
            return self.persist_idempotency_result(
                principal,
                command,
                result,
                audit_allowed,
                detail,
            );
        }
        // The audit service is part of ControlAggregate. Append the event
        // before completing the idempotency transaction so both are written
        // by the same durable aggregate commit.
        if let Err(error) = self.append_audit_event(audit) {
            self.state.reload_control_aggregate()?;
            return Err(error);
        }
        self.persist_idempotency_result(principal, command, result, audit_allowed, detail)
    }

    fn abandon_idempotency(&mut self) -> Result<(), Problem> {
        let Some(claim) = self.claim.as_ref() else {
            return Ok(());
        };
        self.state.abandon_mcp_idempotency(claim)?;
        self.claim = None;
        Ok(())
    }

    fn abandon_idempotency_with_audit(&mut self, audit: AuditRecord) -> Result<(), Problem> {
        self.abandon_idempotency()?;
        let actor_id = audit
            .actor_id
            .clone()
            .unwrap_or_else(|| "anonymous".to_owned());
        let detail = serde_json::json!({
            "adapter": "mcp",
            "correlationId": audit.correlation_id.clone(),
            "scope": audit.scope.clone(),
            "outcome": format!("{:?}", audit.outcome),
            "metadata": audit.metadata.clone(),
        });
        let input_hash = sha256_hash(&detail);
        let event_id = match self.append_audit_event(audit) {
            Ok(event_id) => event_id,
            Err(error) => {
                self.state.reload_control_aggregate()?;
                return Err(error);
            }
        };
        let audit_key = format!("mcp-audit-{event_id}");
        if let Err(error) = self.state.persist_control_aggregate(
            "mcp.audit.append",
            audit_key,
            actor_id,
            true,
            &input_hash,
            detail,
            200,
            serde_json::json!({"recorded": true}),
        ) {
            self.state.reload_control_aggregate()?;
            return Err(error);
        }
        Ok(())
    }

    fn audit(&mut self, record: AuditRecord) -> Result<(), Problem> {
        let snapshot_gate = std::sync::Arc::clone(&self.state.control_snapshot_gate);
        let _snapshot_gate = snapshot_gate.try_write().map_err(|_| {
            Problem::new(
                "control_snapshot_busy",
                "A profile snapshot is in progress; retry the command",
                503,
                "mcp",
            )
            .retryable(true)
        })?;
        let mutation_lock = std::sync::Arc::clone(self.state.control_mutation());
        let mutation_lock_result = mutation_lock.lock();
        let _mutation_lock = match mutation_lock_result {
            Ok(guard) => guard,
            Err(error) => {
                drop(error.into_inner());
                return Err(Problem::new(
                    "control_unavailable",
                    "Control state unavailable",
                    503,
                    "mcp",
                ));
            }
        };
        self.state.refresh_control_aggregate_locked()?;

        let actor_id = record
            .actor_id
            .clone()
            .unwrap_or_else(|| "anonymous".to_owned());
        let detail = serde_json::json!({
            "adapter": "mcp",
            "correlationId": record.correlation_id.clone(),
            "scope": record.scope.clone(),
            "outcome": format!("{:?}", record.outcome),
            "metadata": record.metadata.clone(),
        });
        let input_hash = sha256_hash(&detail);
        let event_id = self.append_audit_event(record)?;
        if let Err(error) = self.state.persist_control_aggregate(
            "mcp.audit.append",
            format!("mcp-audit-{event_id}"),
            actor_id,
            true,
            &input_hash,
            detail,
            200,
            serde_json::json!({"recorded": true}),
        ) {
            self.state.reload_control_aggregate()?;
            return Err(error);
        }
        Ok(())
    }

    fn append_audit_event(&mut self, record: AuditRecord) -> Result<String, Problem> {
        let outcome = match record.outcome {
            AuditOutcome::Succeeded => buglogin_domain::audit::AuditOutcome::Succeeded,
            AuditOutcome::Failed => buglogin_domain::audit::AuditOutcome::Failed,
            AuditOutcome::Denied => buglogin_domain::audit::AuditOutcome::Denied,
        };
        let event_id = format!(
            "evt_{}",
            &sha256_hash(&serde_json::json!({
                "correlation": record.correlation_id,
                "command": record.command_id,
                "outcome": format!("{:?}", record.outcome),
                "sequence": AUDIT_SEQUENCE.fetch_add(1, Ordering::Relaxed),
            }))[..24]
        );
        let event = buglogin_domain::audit::AuditEvent::new(
            event_id.clone(),
            unix_time_seconds(&record.correlation_id)?,
            AccountId::from_str(record.actor_id.as_deref().unwrap_or("anonymous"))
                .unwrap_or_else(|_| AccountId::new("anonymous").unwrap()),
            buglogin_domain::audit::AuditResource {
                resource_type: "mcp".into(),
                resource_id: record.command_id.into(),
                scope: record.scope.clone(),
            },
            record.command_id,
            record.correlation_id,
            outcome,
            record.metadata.into_iter().collect(),
        )?;
        self.state
            .audit
            .write()
            .map_err(|_| {
                Problem::new("audit_unavailable", "Audit service unavailable", 503, "mcp")
            })?
            .append(event)?;
        Ok(event_id)
    }
}

pub struct ProductionMcpExecutor {
    state: crate::http::ApiState,
    transaction_guarded: Cell<bool>,
}

impl ProductionMcpExecutor {
    pub fn new(state: crate::http::ApiState) -> Self {
        Self {
            state,
            transaction_guarded: Cell::new(false),
        }
    }
}

#[async_trait]
impl McpExecutor for ProductionMcpExecutor {
    fn execute(&mut self, request: ExecutionRequest<'_>) -> Result<Value, Problem> {
        let state = &self.state;
        let input = request.input;
        let principal = request.principal;

        let is_mutation = request.command.kind == crate::commands::CommandKind::Command
            || request.command.kind == crate::commands::CommandKind::Operation;
        let transaction_guarded = self.transaction_guarded.get();
        let _snapshot_gate = if is_mutation && !transaction_guarded {
            Some(state.control_snapshot_gate.try_write().map_err(|_| {
                Problem::new(
                    "control_snapshot_busy",
                    "A profile snapshot is in progress; retry the command",
                    503,
                    "mcp",
                )
                .retryable(true)
            })?)
        } else {
            None
        };
        let _mutation_lock = if is_mutation && !transaction_guarded {
            Some(state.control_mutation().lock().map_err(|_| {
                Problem::new(
                    "control_unavailable",
                    "Control state unavailable",
                    503,
                    "mcp",
                )
            })?)
        } else {
            None
        };

        if is_mutation {
            state.refresh_control_aggregate_locked()?;
        } else if matches!(
            request.command.id.as_str(),
            "profile.manifest.get" | "resource.lease.get"
        ) {
            state.refresh_control_aggregate()?;
        }

        if let Some(token) = request.bearer_token.as_deref() {
            let current_principal =
                state.authenticate_token(token, unix_time_seconds(&request.correlation_id)?)?;
            if current_principal != principal {
                return Err(Problem::new(
                    "authorization_state_changed",
                    "Authentication changed; retry the command",
                    409,
                    "mcp",
                )
                .retryable(true));
            }
        }

        let mut current_policy = ProductionMcpPolicy::new(state.clone());
        let current_scope = current_policy.resolve_scope(&principal, request.command, &input)?;
        if current_scope != request.scope {
            return Err(Problem::new(
                "authorization_state_changed",
                "Command scope changed; retry the command",
                409,
                "mcp",
            )
            .retryable(true));
        }
        current_policy.authorize(&principal, request.command, &current_scope)?;
        let grants = state.grants()?;

        let before_state = if is_mutation {
            Some(state.control_aggregate()?)
        } else {
            None
        };

        let result: Result<Value, Problem> = match request.command.id.as_str() {
            "system.health.get" => Ok(serde_json::json!({
                "status": "ok",
                "service": "buglogin-api"
            })),
            "runtime.catalog.get" => match state.published_runtime_catalog() {
                Some(catalog) => {
                    serde_json::to_value(catalog.catalog()).map_err(serialization_problem)
                }
                None => Err(problem(
                    "runtime_catalog_unavailable",
                    "Signed runtime catalog is not configured",
                    503,
                )
                .retryable(true)),
            },
            "operation.get" => {
                state.refresh_control_aggregate()?;
                let operation_id = input
                    .get("operationId")
                    .and_then(Value::as_str)
                    .ok_or_else(|| problem("invalid_input", "operationId is required", 400))?;
                let operation = state
                    .operations
                    .read()
                    .map_err(|_| {
                        problem(
                            "operation_unavailable",
                            "Operation service unavailable",
                            503,
                        )
                    })?
                    .get(&principal, &grants, operation_id)?
                    .clone();
                Ok(serde_json::to_value(operation).map_err(serialization_problem)?)
            }
            "events.stream" => Err(problem(
                "async_executor_required",
                "Event stream must use the asynchronous executor",
                503,
            )),
            "audit.events.list" => {
                let query: AuditEventsListInput =
                    serde_json::from_value(input.clone()).map_err(serialization_problem)?;
                let workspace_id = request.scope.workspace_id.as_ref().ok_or_else(|| {
                    problem("workspace_missing", "Workspace is missing from scope", 400)
                })?;
                if workspace_id != &query.workspace_id {
                    return Err(problem(
                        "invalid_scope",
                        "Workspace scope does not match",
                        400,
                    ));
                }
                let page = state
                    .audit
                    .read()
                    .map_err(|_| problem("audit_unavailable", "Audit service unavailable", 503))?
                    .read_page(
                        &principal,
                        &grants,
                        &ResourceScope::workspace(workspace_id.clone()),
                        query.cursor.as_deref(),
                        query.limit.unwrap_or(50),
                    )?;
                Ok(serde_json::to_value(page).map_err(serialization_problem)?)
            }
            "proxy.parse" => {
                let text = input
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or_else(|| problem("invalid_input", "text is required", 400))?;
                Ok(proxy_parse_view(buglogin_network::proxy::parse_lines(text)))
            }
            "cookie.list" => state.execute_local_cookie(
                request.command.id.as_str(),
                input.clone(),
                &principal,
                &grants,
                request.idempotency_key.as_deref(),
                &request.correlation_id,
            ),
            "cookie.stage" | "cookie.import" | "cookie.copy" => Err(problem(
                "production_cookie_device_dispatch_required",
                "Cookie mutation requires a connected authorized device",
                503,
            )
            .retryable(true)),
            "proxy.list" => {
                let workspace_id = request.scope.workspace_id.clone().ok_or_else(|| {
                    problem("workspace_missing", "Workspace is missing from scope", 400)
                })?;
                let records = state
                    .proxies
                    .read()
                    .map_err(|_| problem("proxy_unavailable", "Proxy service unavailable", 503))?
                    .list(&workspace_id, &principal, &grants)
                    .map_err(proxy_problem)?;
                Ok(serde_json::json!({
                    "items": records.into_iter().map(proxy_record_view).collect::<Vec<_>>()
                }))
            }
            "proxy.get" => {
                let workspace_id = request.scope.workspace_id.clone().ok_or_else(|| {
                    problem("workspace_missing", "Workspace is missing from scope", 400)
                })?;
                let proxy_id: buglogin_domain::ids::ProxyId =
                    serde_json::from_value(input.get("proxyId").cloned().unwrap_or(Value::Null))
                        .map_err(serialization_problem)?;
                let record = state
                    .proxies
                    .read()
                    .map_err(|_| problem("proxy_unavailable", "Proxy service unavailable", 503))?
                    .get(&workspace_id, &proxy_id, &principal, &grants)
                    .map_err(proxy_problem)?;
                Ok(proxy_record_view(record))
            }
            "proxy.create" => {
                let workspace_id = request.scope.workspace_id.clone().ok_or_else(|| {
                    problem("workspace_missing", "Workspace is missing from scope", 400)
                })?;
                let input_proxy: McpProxyMetadata =
                    serde_json::from_value(input.get("proxy").cloned().unwrap_or(Value::Null))
                        .map_err(serialization_problem)?;
                let proxy = input_proxy.into_proxy()?;
                let result = state
                    .proxies
                    .write()
                    .map_err(|_| problem("proxy_unavailable", "Proxy service unavailable", 503))?
                    .create(workspace_id, proxy, &principal, &grants)
                    .map_err(proxy_problem)?;
                Ok(serde_json::json!({
                    "record": proxy_record_view(result.record),
                    "deduplicated": result.deduplicated
                }))
            }
            "proxy.update" => {
                let workspace_id = request.scope.workspace_id.clone().ok_or_else(|| {
                    problem("workspace_missing", "Workspace is missing from scope", 400)
                })?;
                let proxy_id: buglogin_domain::ids::ProxyId =
                    serde_json::from_value(input.get("proxyId").cloned().unwrap_or(Value::Null))
                        .map_err(serialization_problem)?;
                let expected_revision = input
                    .get("expectedRevision")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| problem("invalid_input", "expectedRevision is required", 400))?;
                let input_proxy: McpProxyMetadata =
                    serde_json::from_value(input.get("proxy").cloned().unwrap_or(Value::Null))
                        .map_err(serialization_problem)?;
                let has_credential = input_proxy.has_credential;
                let mut proxy = input_proxy.into_proxy()?;
                if proxy.id != proxy_id {
                    return Err(problem("invalid_proxy", "Proxy identifier mismatch", 400));
                }
                let current = state
                    .proxies
                    .read()
                    .map_err(|_| problem("proxy_unavailable", "Proxy service unavailable", 503))?
                    .get(&workspace_id, &proxy_id, &principal, &grants)
                    .map_err(proxy_problem)?;
                crate::services::profile_sync::ensure_proxy_update_allowed(
                    &state,
                    &workspace_id,
                    &proxy_id,
                    &request.correlation_id,
                )?;
                proxy.credential_ref = match has_credential {
                    None => current.proxy.credential_ref,
                    Some(false) => None,
                    Some(true) => Some(credential_version(
                        &proxy.id,
                        next_proxy_revision(expected_revision)?,
                    )?),
                };
                proxy.rotation_url = current.proxy.rotation_url;
                let record = state
                    .proxies
                    .write()
                    .map_err(|_| problem("proxy_unavailable", "Proxy service unavailable", 503))?
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
                    .map_err(proxy_problem)?;
                Ok(proxy_record_view(record))
            }
            "proxy.delete" => {
                let workspace_id = request.scope.workspace_id.clone().ok_or_else(|| {
                    problem("workspace_missing", "Workspace is missing from scope", 400)
                })?;
                let proxy_id: buglogin_domain::ids::ProxyId =
                    serde_json::from_value(input.get("proxyId").cloned().unwrap_or(Value::Null))
                        .map_err(serialization_problem)?;
                let expected_revision = input
                    .get("expectedRevision")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| problem("invalid_input", "expectedRevision is required", 400))?;
                crate::services::profile_sync::ensure_proxy_unassigned(
                    state,
                    &workspace_id,
                    &proxy_id,
                    &request.correlation_id,
                )?;
                let record = state
                    .proxies
                    .write()
                    .map_err(|_| problem("proxy_unavailable", "Proxy service unavailable", 503))?
                    .delete(
                        &workspace_id,
                        &proxy_id,
                        ProxyDelete { expected_revision },
                        &ProxyAssignmentRepository::default(),
                        &principal,
                        &grants,
                    )
                    .map_err(proxy_problem)?;
                Ok(proxy_record_view(record))
            }
            "account.get" => {
                let account_id = principal.account_id.clone().ok_or_else(|| {
                    Problem::new("account_missing", "Account is missing", 401, "identity")
                })?;
                let account = state.account(&account_id)?.ok_or_else(|| {
                    Problem::new("account_not_found", "Account not found", 404, "identity")
                })?;
                Ok(serde_json::to_value(account).map_err(serialization_problem)?)
            }
            "organization.create" => {
                let parsed: OrganizationInput =
                    serde_json::from_value(input.clone()).map_err(serialization_problem)?;
                state
                    .workspaces
                    .write()
                    .map_err(|_| {
                        Problem::new(
                            "workspace_unavailable",
                            "Workspace service unavailable",
                            503,
                            "mcp",
                        )
                    })?
                    .add_organization(
                        &principal,
                        &grants,
                        parsed.organization.clone(),
                        parsed.policy,
                    )?;
                Ok(serde_json::to_value(parsed.organization).map_err(serialization_problem)?)
            }
            "organization.get" => {
                let org_id: OrganizationId = serde_json::from_value(
                    input.get("organizationId").cloned().unwrap_or(Value::Null),
                )
                .map_err(serialization_problem)?;
                let organization = state
                    .workspaces
                    .read()
                    .map_err(|_| {
                        Problem::new(
                            "workspace_unavailable",
                            "Workspace service unavailable",
                            503,
                            "mcp",
                        )
                    })?
                    .get_organization(&principal, &grants, &org_id)?;
                Ok(serde_json::to_value(organization.organization)
                    .map_err(serialization_problem)?)
            }
            "organization.record.get" => {
                let org_id: OrganizationId = serde_json::from_value(
                    input.get("organizationId").cloned().unwrap_or(Value::Null),
                )
                .map_err(serialization_problem)?;
                let record = state
                    .workspaces
                    .read()
                    .map_err(|_| {
                        Problem::new(
                            "workspace_unavailable",
                            "Workspace service unavailable",
                            503,
                            "mcp",
                        )
                    })?
                    .get_organization(&principal, &grants, &org_id)?;
                Ok(serde_json::to_value(record).map_err(serialization_problem)?)
            }
            "organization.list" => {
                let list = state
                    .workspaces
                    .read()
                    .map_err(|_| {
                        Problem::new(
                            "workspace_unavailable",
                            "Workspace service unavailable",
                            503,
                            "mcp",
                        )
                    })?
                    .list_organizations(&principal, &grants)?;
                Ok(serde_json::json!({ "items": list, "nextCursor": Value::Null }))
            }
            "organization.update" => {
                let org_id: OrganizationId = serde_json::from_value(
                    input.get("organizationId").cloned().unwrap_or(Value::Null),
                )
                .map_err(serialization_problem)?;
                let update: crate::services::workspaces::OrganizationUpdate =
                    serde_json::from_value(input.get("update").cloned().unwrap_or(Value::Null))
                        .map_err(serialization_problem)?;
                let record = state
                    .workspaces
                    .write()
                    .map_err(|_| {
                        Problem::new(
                            "workspace_unavailable",
                            "Workspace service unavailable",
                            503,
                            "mcp",
                        )
                    })?
                    .update_organization(&principal, &grants, &org_id, update)?;
                Ok(serde_json::to_value(record.organization).map_err(serialization_problem)?)
            }
            "workspace.create" => {
                let org_id: OrganizationId = serde_json::from_value(
                    input.get("organizationId").cloned().unwrap_or(Value::Null),
                )
                .map_err(serialization_problem)?;
                let workspace: Workspace =
                    serde_json::from_value(input.get("workspace").cloned().unwrap_or(Value::Null))
                        .map_err(serialization_problem)?;
                state
                    .workspaces
                    .write()
                    .map_err(|_| {
                        Problem::new(
                            "workspace_unavailable",
                            "Workspace service unavailable",
                            503,
                            "mcp",
                        )
                    })?
                    .create_workspace(&principal, &grants, &org_id, workspace.clone())?;
                Ok(serde_json::to_value(workspace).map_err(serialization_problem)?)
            }
            "workspace.get" => {
                let org_id: OrganizationId = serde_json::from_value(
                    input.get("organizationId").cloned().unwrap_or(Value::Null),
                )
                .map_err(serialization_problem)?;
                let ws_id: WorkspaceId = serde_json::from_value(
                    input.get("workspaceId").cloned().unwrap_or(Value::Null),
                )
                .map_err(serialization_problem)?;
                let ws = state
                    .workspaces
                    .read()
                    .map_err(|_| {
                        Problem::new(
                            "workspace_unavailable",
                            "Workspace service unavailable",
                            503,
                            "mcp",
                        )
                    })?
                    .get_workspace(&principal, &grants, &org_id, &ws_id)?;
                Ok(serde_json::to_value(ws).map_err(serialization_problem)?)
            }
            "workspace.record.get" => {
                let org_id: OrganizationId = serde_json::from_value(
                    input.get("organizationId").cloned().unwrap_or(Value::Null),
                )
                .map_err(serialization_problem)?;
                let ws_id: WorkspaceId = serde_json::from_value(
                    input.get("workspaceId").cloned().unwrap_or(Value::Null),
                )
                .map_err(serialization_problem)?;
                let record = state
                    .workspaces
                    .read()
                    .map_err(|_| {
                        Problem::new(
                            "workspace_unavailable",
                            "Workspace service unavailable",
                            503,
                            "mcp",
                        )
                    })?
                    .get_workspace_record(&principal, &grants, &org_id, &ws_id)?;
                Ok(serde_json::to_value(record).map_err(serialization_problem)?)
            }
            "workspace.list" => {
                let org_id: OrganizationId = serde_json::from_value(
                    input.get("organizationId").cloned().unwrap_or(Value::Null),
                )
                .map_err(serialization_problem)?;
                let list = state
                    .workspaces
                    .read()
                    .map_err(|_| {
                        Problem::new(
                            "workspace_unavailable",
                            "Workspace service unavailable",
                            503,
                            "mcp",
                        )
                    })?
                    .list_workspaces(&principal, &grants, &org_id)?;
                Ok(serde_json::json!({ "items": list, "nextCursor": Value::Null }))
            }
            "workspace.update" => {
                let org_id: OrganizationId = serde_json::from_value(
                    input.get("organizationId").cloned().unwrap_or(Value::Null),
                )
                .map_err(serialization_problem)?;
                let ws_id: WorkspaceId = serde_json::from_value(
                    input.get("workspaceId").cloned().unwrap_or(Value::Null),
                )
                .map_err(serialization_problem)?;
                let update: crate::services::workspaces::WorkspaceUpdate =
                    serde_json::from_value(input.get("update").cloned().unwrap_or(Value::Null))
                        .map_err(serialization_problem)?;
                let record = state
                    .workspaces
                    .write()
                    .map_err(|_| {
                        Problem::new(
                            "workspace_unavailable",
                            "Workspace service unavailable",
                            503,
                            "mcp",
                        )
                    })?
                    .update_workspace(&principal, &grants, &org_id, &ws_id, update)?;
                Ok(serde_json::to_value(record.workspace).map_err(serialization_problem)?)
            }
            "group.create" => {
                let input: crate::services::groups::GroupCreateInput =
                    serde_json::from_value(input.clone()).map_err(serialization_problem)?;
                if request.scope.workspace_id.as_ref() != Some(&input.workspace_id) {
                    return Err(problem(
                        "invalid_scope",
                        "Workspace scope does not match",
                        400,
                    ));
                }
                let workspace_id = input.workspace_id.clone();
                let record = state
                    .groups
                    .write()
                    .map_err(|_| problem("group_unavailable", "Group service unavailable", 503))?
                    .create(&principal, &grants, &workspace_id, input)?;
                Ok(serde_json::to_value(record).map_err(serialization_problem)?)
            }
            "group.list" => {
                let input: crate::services::groups::GroupListInput =
                    serde_json::from_value(input.clone()).map_err(serialization_problem)?;
                if request.scope.workspace_id.as_ref() != Some(&input.workspace_id) {
                    return Err(problem(
                        "invalid_scope",
                        "Workspace scope does not match",
                        400,
                    ));
                }
                let records = state
                    .groups
                    .read()
                    .map_err(|_| problem("group_unavailable", "Group service unavailable", 503))?
                    .list(&principal, &grants, &input.workspace_id)?;
                Ok(serde_json::to_value(records).map_err(serialization_problem)?)
            }
            "group.get" => {
                let input: crate::services::groups::GroupIdInput =
                    serde_json::from_value(input.clone()).map_err(serialization_problem)?;
                if request.scope.workspace_id.as_ref() != Some(&input.workspace_id) {
                    return Err(problem(
                        "invalid_scope",
                        "Workspace scope does not match",
                        400,
                    ));
                }
                let record = state
                    .groups
                    .read()
                    .map_err(|_| problem("group_unavailable", "Group service unavailable", 503))?
                    .get(&principal, &grants, &input.workspace_id, &input.group_id)?;
                Ok(serde_json::to_value(record).map_err(serialization_problem)?)
            }
            "group.update" => {
                let input: crate::services::groups::GroupUpdateInput =
                    serde_json::from_value(input.clone()).map_err(serialization_problem)?;
                if request.scope.workspace_id.as_ref() != Some(&input.workspace_id) {
                    return Err(problem(
                        "invalid_scope",
                        "Workspace scope does not match",
                        400,
                    ));
                }
                let workspace_id = input.workspace_id.clone();
                let group_id = input.group_id.clone();
                let record = state
                    .groups
                    .write()
                    .map_err(|_| problem("group_unavailable", "Group service unavailable", 503))?
                    .update(&principal, &grants, &workspace_id, &group_id, input)?;
                Ok(serde_json::to_value(record).map_err(serialization_problem)?)
            }
            "group.delete" => {
                let input: crate::services::groups::GroupDeleteInput =
                    serde_json::from_value(input.clone()).map_err(serialization_problem)?;
                if request.scope.workspace_id.as_ref() != Some(&input.workspace_id) {
                    return Err(problem(
                        "invalid_scope",
                        "Workspace scope does not match",
                        400,
                    ));
                }
                let workspace_id = input.workspace_id.clone();
                let group_id = input.group_id.clone();
                let in_use = state.group_is_in_use(&workspace_id, &group_id)?;
                let record = state
                    .groups
                    .write()
                    .map_err(|_| problem("group_unavailable", "Group service unavailable", 503))?
                    .delete(&principal, &grants, &workspace_id, &group_id, input, in_use)?;
                Ok(serde_json::to_value(record).map_err(serialization_problem)?)
            }
            "extension.group.list" => {
                let input: crate::services::extension_groups::ExtensionGroupListInput =
                    serde_json::from_value(input.clone()).map_err(serialization_problem)?;
                if request.scope.workspace_id.as_ref() != Some(&input.workspace_id) {
                    return Err(problem(
                        "invalid_scope",
                        "Workspace scope does not match",
                        400,
                    ));
                }
                let records = state
                    .extension_groups
                    .read()
                    .map_err(|_| {
                        problem(
                            "extension_group_unavailable",
                            "Extension group service unavailable",
                            503,
                        )
                    })?
                    .list(&principal, &grants, &input.workspace_id)?;
                Ok(serde_json::to_value(records).map_err(serialization_problem)?)
            }
            "extension.group.get" => {
                let input: crate::services::extension_groups::ExtensionGroupIdInput =
                    serde_json::from_value(input.clone()).map_err(serialization_problem)?;
                if request.scope.workspace_id.as_ref() != Some(&input.workspace_id) {
                    return Err(problem(
                        "invalid_scope",
                        "Workspace scope does not match",
                        400,
                    ));
                }
                let record = state
                    .extension_groups
                    .read()
                    .map_err(|_| {
                        problem(
                            "extension_group_unavailable",
                            "Extension group service unavailable",
                            503,
                        )
                    })?
                    .get(
                        &principal,
                        &grants,
                        &input.workspace_id,
                        &input.extension_group_id,
                    )?;
                Ok(serde_json::to_value(record).map_err(serialization_problem)?)
            }
            "extension.group.create" => {
                let input: crate::services::extension_groups::ExtensionGroupCreateInput =
                    serde_json::from_value(input.clone()).map_err(serialization_problem)?;
                if request.scope.workspace_id.as_ref() != Some(&input.workspace_id) {
                    return Err(problem(
                        "invalid_scope",
                        "Workspace scope does not match",
                        400,
                    ));
                }
                let record = state
                    .extension_groups
                    .write()
                    .map_err(|_| {
                        problem(
                            "extension_group_unavailable",
                            "Extension group service unavailable",
                            503,
                        )
                    })?
                    .create(&principal, &grants, &input.workspace_id.clone(), input)?;
                Ok(serde_json::to_value(record).map_err(serialization_problem)?)
            }
            "extension.group.update" => {
                let input: crate::services::extension_groups::ExtensionGroupUpdateInput =
                    serde_json::from_value(input.clone()).map_err(serialization_problem)?;
                if request.scope.workspace_id.as_ref() != Some(&input.workspace_id) {
                    return Err(problem(
                        "invalid_scope",
                        "Workspace scope does not match",
                        400,
                    ));
                }
                let record = state
                    .extension_groups
                    .write()
                    .map_err(|_| {
                        problem(
                            "extension_group_unavailable",
                            "Extension group service unavailable",
                            503,
                        )
                    })?
                    .update(&principal, &grants, &input.workspace_id.clone(), input)?;
                Ok(serde_json::to_value(record).map_err(serialization_problem)?)
            }
            "extension.group.delete" => {
                let input: crate::services::extension_groups::ExtensionGroupDeleteInput =
                    serde_json::from_value(input.clone()).map_err(serialization_problem)?;
                if request.scope.workspace_id.as_ref() != Some(&input.workspace_id) {
                    return Err(problem(
                        "invalid_scope",
                        "Workspace scope does not match",
                        400,
                    ));
                }
                let in_use = state
                    .extension_group_is_in_use(&input.workspace_id, &input.extension_group_id)?;
                let record = state
                    .extension_groups
                    .write()
                    .map_err(|_| {
                        problem(
                            "extension_group_unavailable",
                            "Extension group service unavailable",
                            503,
                        )
                    })?
                    .delete(
                        &principal,
                        &grants,
                        &input.workspace_id.clone(),
                        input,
                        in_use,
                    )?;
                Ok(serde_json::to_value(record).map_err(serialization_problem)?)
            }
            "profile.extension.group.set" => {
                let input: crate::services::extension_groups::ProfileExtensionGroupSetInput =
                    serde_json::from_value(input.clone()).map_err(serialization_problem)?;
                if request.scope.workspace_id.as_ref() != Some(&input.workspace_id)
                    || request.scope.profile_id.as_ref() != Some(&input.profile_id)
                {
                    return Err(problem(
                        "invalid_scope",
                        "Profile scope does not match",
                        400,
                    ));
                }
                let binding = state
                    .profile_resource_record(&input.profile_id)?
                    .filter(|binding| binding.workspace_id == input.workspace_id)
                    .ok_or_else(|| {
                        problem("binding_not_found", "Profile binding not found", 404)
                    })?;
                if request.scope.resource_id.as_ref() != Some(&binding.resource_id) {
                    return Err(problem(
                        "invalid_scope",
                        "Profile resource scope does not match",
                        400,
                    ));
                }
                let record = state.set_hosted_profile_extension_group(input)?;
                Ok(
                    serde_json::to_value(buglogin_domain::entities::ProfileRecord::new(
                        record.value,
                        record.revision,
                    ))
                    .map_err(serialization_problem)?,
                )
            }
            "group.share.list" => {
                let input: crate::services::groups::GroupIdInput =
                    serde_json::from_value(input.clone()).map_err(serialization_problem)?;
                if request.scope.workspace_id.as_ref() != Some(&input.workspace_id) {
                    return Err(problem(
                        "invalid_scope",
                        "Workspace scope does not match",
                        400,
                    ));
                }
                let records = state
                    .groups
                    .read()
                    .map_err(|_| problem("group_unavailable", "Group service unavailable", 503))?
                    .list_share_grants(
                        &principal,
                        &grants,
                        &input.workspace_id,
                        Some(&input.group_id),
                        None,
                    )?;
                Ok(serde_json::to_value(records).map_err(serialization_problem)?)
            }
            "group.share.create" => {
                let input: crate::services::groups::GroupShareCreateInput =
                    serde_json::from_value(input.clone()).map_err(serialization_problem)?;
                if request.scope.workspace_id.as_ref() != Some(&input.workspace_id) {
                    return Err(problem(
                        "invalid_scope",
                        "Workspace scope does not match",
                        400,
                    ));
                }
                let record =
                    HostedProfileService::create_group_share(state, &principal, &grants, input)?;
                Ok(serde_json::to_value(record).map_err(serialization_problem)?)
            }
            "group.share.revoke" => {
                let input: crate::services::groups::GroupShareRevokeInput =
                    serde_json::from_value(input.clone()).map_err(serialization_problem)?;
                if request.scope.workspace_id.as_ref() != Some(&input.workspace_id) {
                    return Err(problem(
                        "invalid_scope",
                        "Workspace scope does not match",
                        400,
                    ));
                }
                let record =
                    HostedProfileService::revoke_group_share(state, &principal, &grants, input)?;
                Ok(serde_json::to_value(record).map_err(serialization_problem)?)
            }
            "profile.groups.replace" => {
                let input: crate::services::groups::ProfileGroupsReplaceInput =
                    serde_json::from_value(input.clone()).map_err(serialization_problem)?;
                if request.scope.workspace_id.as_ref() != Some(&input.workspace_id)
                    || request.scope.profile_id.as_ref() != Some(&input.profile_id)
                {
                    return Err(problem(
                        "invalid_scope",
                        "Profile scope does not match",
                        400,
                    ));
                }
                let binding = state
                    .profile_resource_record(&input.profile_id)?
                    .filter(|binding| binding.workspace_id == input.workspace_id)
                    .ok_or_else(|| {
                        problem("binding_not_found", "Profile binding not found", 404)
                    })?;
                if request.scope.resource_id.as_ref() != Some(&binding.resource_id) {
                    return Err(problem(
                        "invalid_scope",
                        "Profile resource scope does not match",
                        400,
                    ));
                }
                let record = HostedProfileService::replace_profile_groups(state, input)?;
                Ok(
                    serde_json::to_value(ProfileRecord::new(record.value, record.revision))
                        .map_err(serialization_problem)?,
                )
            }
            "profile.proxy.assignment.get" => {
                let input: crate::services::profile_proxy_assignment::ProfileProxyAssignmentGetInput =
                    serde_json::from_value(input.clone()).map_err(serialization_problem)?;
                if request.scope.workspace_id.as_ref() != Some(&input.workspace_id)
                    || request.scope.profile_id.as_ref() != Some(&input.profile_id)
                {
                    return Err(problem(
                        "invalid_scope",
                        "Profile scope does not match",
                        400,
                    ));
                }
                let binding = state
                    .profile_resource_record(&input.profile_id)?
                    .filter(|binding| binding.workspace_id == input.workspace_id)
                    .ok_or_else(|| {
                        problem("binding_not_found", "Profile binding not found", 404)
                    })?;
                let _ = binding;
                let _profile = state
                    .get_profile_record(&input.profile_id)?
                    .ok_or_else(|| problem("profile_not_found", "Profile not found", 404))?;
                let assignment = state
                    .profile_proxy_assignments
                    .read()
                    .map_err(|_| {
                        problem(
                            "profile_proxy_assignment_unavailable",
                            "Profile proxy assignment service unavailable",
                            503,
                        )
                    })?
                    .get(&input.workspace_id, &input.profile_id)?;
                Ok(serde_json::to_value(assignment).map_err(serialization_problem)?)
            }
            "profile.proxy.assignment.set" => {
                let input: crate::services::profile_proxy_assignment::ProfileProxyAssignmentSetInput =
                    serde_json::from_value(input.clone()).map_err(serialization_problem)?;
                if request.scope.workspace_id.as_ref() != Some(&input.workspace_id)
                    || request.scope.profile_id.as_ref() != Some(&input.profile_id)
                {
                    return Err(problem(
                        "invalid_scope",
                        "Profile scope does not match",
                        400,
                    ));
                }
                let binding = state
                    .profile_resource_record(&input.profile_id)?
                    .filter(|binding| binding.workspace_id == input.workspace_id)
                    .ok_or_else(|| {
                        problem("binding_not_found", "Profile binding not found", 404)
                    })?;
                let _ = binding;
                let profile = state
                    .get_profile_record(&input.profile_id)?
                    .ok_or_else(|| problem("profile_not_found", "Profile not found", 404))?;
                if state.local_authority_enabled() {
                    return Err(problem(
                        "hosted_proxy_assignment_required",
                        "Managed proxy reassignment requires hosted profile sync",
                        503,
                    ));
                }
                let pending_assignment = state
                    .profile_proxy_assignments
                    .read()
                    .map_err(|_| {
                        problem(
                            "profile_proxy_assignment_unavailable",
                            "Profile proxy assignment service unavailable",
                            503,
                        )
                    })?
                    .get_optional(&input.workspace_id, &input.profile_id);
                crate::services::profile_proxy_assignment::ProfileProxyAssignmentService::validate_change(
                    &profile.value,
                    profile.revision,
                    &input,
                    pending_assignment.as_ref(),
                )?;
                if let Some(proxy_id) = input.proxy_id.as_ref() {
                    state
                        .proxies
                        .read()
                        .map_err(|_| {
                            problem("proxy_unavailable", "Proxy service unavailable", 503)
                        })?
                        .get(&input.workspace_id, proxy_id, &principal, &grants)
                        .map_err(proxy_problem)?;
                }
                let workspace_id = input.workspace_id.clone();
                let assignment = state
                    .profile_proxy_assignments
                    .write()
                    .map_err(|_| {
                        problem(
                            "profile_proxy_assignment_unavailable",
                            "Profile proxy assignment service unavailable",
                            503,
                        )
                    })?
                    .set(&workspace_id, input)?;
                Ok(serde_json::to_value(assignment).map_err(serialization_problem)?)
            }
            "team.create" => {
                let ws_id: WorkspaceId = serde_json::from_value(
                    input.get("workspaceId").cloned().unwrap_or(Value::Null),
                )
                .map_err(serialization_problem)?;
                let team: Team =
                    serde_json::from_value(input.get("team").cloned().unwrap_or(Value::Null))
                        .map_err(serialization_problem)?;
                let record = state
                    .teams
                    .write()
                    .map_err(|_| {
                        Problem::new("team_unavailable", "Team service unavailable", 503, "mcp")
                    })?
                    .create(&principal, &grants, &ws_id, team.clone())?;
                Ok(serde_json::to_value(record).map_err(serialization_problem)?)
            }
            "team.get" => {
                let id: TeamId =
                    serde_json::from_value(input.get("teamId").cloned().unwrap_or(Value::Null))
                        .map_err(serialization_problem)?;
                let ws_id = request.scope.workspace_id.clone().ok_or_else(|| {
                    Problem::new(
                        "workspace_missing",
                        "Workspace is missing from scope",
                        400,
                        "mcp",
                    )
                })?;
                let record = state
                    .teams
                    .read()
                    .map_err(|_| {
                        Problem::new("team_unavailable", "Team service unavailable", 503, "mcp")
                    })?
                    .get(&principal, &grants, &ws_id, &id)?;
                Ok(serde_json::to_value(record).map_err(serialization_problem)?)
            }
            "team.list" => {
                let ws_id: WorkspaceId = serde_json::from_value(
                    input.get("workspaceId").cloned().unwrap_or(Value::Null),
                )
                .map_err(serialization_problem)?;
                let list = state
                    .teams
                    .read()
                    .map_err(|_| {
                        Problem::new("team_unavailable", "Team service unavailable", 503, "mcp")
                    })?
                    .list(&principal, &grants, &ws_id)?;
                serde_json::to_value(list).map_err(serialization_problem)
            }
            "team.update" => {
                let ws_id: WorkspaceId = serde_json::from_value(
                    input.get("workspaceId").cloned().unwrap_or(Value::Null),
                )
                .map_err(serialization_problem)?;
                let team_id: TeamId =
                    serde_json::from_value(input.get("teamId").cloned().unwrap_or(Value::Null))
                        .map_err(serialization_problem)?;
                let update: crate::services::teams::TeamUpdate =
                    serde_json::from_value(input.get("update").cloned().unwrap_or(Value::Null))
                        .map_err(serialization_problem)?;
                let record = state
                    .teams
                    .write()
                    .map_err(|_| {
                        Problem::new("team_unavailable", "Team service unavailable", 503, "mcp")
                    })?
                    .update(&principal, &grants, &ws_id, &team_id, update)?;
                Ok(serde_json::to_value(record).map_err(serialization_problem)?)
            }
            "member.invite" => {
                let _ws_id: WorkspaceId = serde_json::from_value(
                    input.get("workspaceId").cloned().unwrap_or(Value::Null),
                )
                .map_err(serialization_problem)?;
                let member: Member =
                    serde_json::from_value(input.get("member").cloned().unwrap_or(Value::Null))
                        .map_err(serialization_problem)?;
                state
                    .members
                    .write()
                    .map_err(|_| {
                        Problem::new(
                            "member_unavailable",
                            "Member service unavailable",
                            503,
                            "mcp",
                        )
                    })?
                    .invite(
                        &principal,
                        &grants,
                        member.clone(),
                        unix_time_seconds(&request.correlation_id)?,
                    )?;
                Ok(serde_json::to_value(member).map_err(serialization_problem)?)
            }
            "member.get" => {
                let id: MemberId =
                    serde_json::from_value(input.get("memberId").cloned().unwrap_or(Value::Null))
                        .map_err(serialization_problem)?;
                let ws_id = request.scope.workspace_id.clone().ok_or_else(|| {
                    Problem::new(
                        "workspace_missing",
                        "Workspace is missing from scope",
                        400,
                        "mcp",
                    )
                })?;
                let member = state
                    .members
                    .read()
                    .map_err(|_| {
                        Problem::new(
                            "member_unavailable",
                            "Member service unavailable",
                            503,
                            "mcp",
                        )
                    })?
                    .get_in_workspace(&principal, &grants, &id, &ws_id)?;
                Ok(serde_json::to_value(member).map_err(serialization_problem)?)
            }
            "member.list" => {
                let ws_id: WorkspaceId = serde_json::from_value(
                    input.get("workspaceId").cloned().unwrap_or(Value::Null),
                )
                .map_err(serialization_problem)?;
                let cursor: Option<MemberId> = input
                    .get("cursor")
                    .filter(|value| !value.is_null())
                    .cloned()
                    .map(serde_json::from_value)
                    .transpose()
                    .map_err(serialization_problem)?;
                let limit = input
                    .get("limit")
                    .and_then(Value::as_u64)
                    .map_or(Ok(100_usize), |value| usize::try_from(value))
                    .map_err(|_| problem("invalid_page_limit", "Page limit is invalid", 400))?;
                let (items, next_cursor) = state
                    .members
                    .read()
                    .map_err(|_| {
                        Problem::new(
                            "member_unavailable",
                            "Member service unavailable",
                            503,
                            "mcp",
                        )
                    })?
                    .list_page(&principal, &grants, &ws_id, cursor.as_ref(), limit)?;
                Ok(serde_json::json!({ "items": items, "nextCursor": next_cursor }))
            }
            "member.activate" => {
                let id: MemberId =
                    serde_json::from_value(input.get("memberId").cloned().unwrap_or(Value::Null))
                        .map_err(serialization_problem)?;
                let workspace_id = request.scope.workspace_id.as_ref().ok_or_else(|| {
                    problem("workspace_missing", "Workspace scope is required", 400)
                })?;
                let member = HostedProfileService::activate_member(
                    state,
                    &principal,
                    &grants,
                    workspace_id,
                    &id,
                )?;
                Ok(serde_json::to_value(member).map_err(serialization_problem)?)
            }
            "member.suspend" => {
                let id: MemberId =
                    serde_json::from_value(input.get("memberId").cloned().unwrap_or(Value::Null))
                        .map_err(serialization_problem)?;
                let workspace_id = request.scope.workspace_id.as_ref().ok_or_else(|| {
                    problem("workspace_missing", "Workspace scope is required", 400)
                })?;
                let member = HostedProfileService::suspend_member(
                    state,
                    &principal,
                    &grants,
                    workspace_id,
                    &id,
                )?;
                Ok(serde_json::to_value(member).map_err(serialization_problem)?)
            }
            "member.role.update" => {
                let id: MemberId =
                    serde_json::from_value(input.get("memberId").cloned().unwrap_or(Value::Null))
                        .map_err(serialization_problem)?;
                let workspace_id = request.scope.workspace_id.as_ref().ok_or_else(|| {
                    problem("workspace_missing", "Workspace scope is required", 400)
                })?;
                let role: buglogin_domain::member::MemberRole =
                    serde_json::from_value(input.get("role").cloned().unwrap_or(Value::Null))
                        .map_err(serialization_problem)?;
                state
                    .members
                    .write()
                    .map_err(|_| {
                        Problem::new(
                            "member_unavailable",
                            "Member service unavailable",
                            503,
                            "mcp",
                        )
                    })?
                    .set_role(&principal, &grants, workspace_id, &id, role.clone())?;
                Ok(serde_json::json!({ "memberId": id, "role": role }))
            }
            "resource.register" => {
                let resource: Resource =
                    serde_json::from_value(input.get("resource").cloned().unwrap_or(Value::Null))
                        .map_err(serialization_problem)?;
                state
                    .resources
                    .write()
                    .map_err(|_| {
                        Problem::new(
                            "resource_unavailable",
                            "Resource service unavailable",
                            503,
                            "mcp",
                        )
                    })?
                    .register(&principal, &grants, resource.clone())?;
                Ok(serde_json::to_value(resource).map_err(serialization_problem)?)
            }
            "resource.get" => {
                let id: ResourceId =
                    serde_json::from_value(input.get("resourceId").cloned().unwrap_or(Value::Null))
                        .map_err(serialization_problem)?;
                let ws_id = request.scope.workspace_id.clone().ok_or_else(|| {
                    Problem::new(
                        "workspace_missing",
                        "Workspace is missing from scope",
                        400,
                        "mcp",
                    )
                })?;
                let resource = state
                    .resources
                    .read()
                    .map_err(|_| {
                        Problem::new(
                            "resource_unavailable",
                            "Resource service unavailable",
                            503,
                            "mcp",
                        )
                    })?
                    .get(&principal, &grants, &id, &ws_id)?;
                Ok(serde_json::to_value(resource).map_err(serialization_problem)?)
            }
            "resource.list" => {
                let ws_id: WorkspaceId = serde_json::from_value(
                    input.get("workspaceId").cloned().unwrap_or(Value::Null),
                )
                .map_err(serialization_problem)?;
                let cursor: Option<ResourceId> = input
                    .get("cursor")
                    .filter(|value| !value.is_null())
                    .cloned()
                    .map(serde_json::from_value)
                    .transpose()
                    .map_err(serialization_problem)?;
                let limit = input
                    .get("limit")
                    .and_then(Value::as_u64)
                    .map_or(Ok(100_usize), |value| usize::try_from(value))
                    .map_err(|_| problem("invalid_page_limit", "Page limit is invalid", 400))?;
                let (items, next_cursor) = state
                    .resources
                    .read()
                    .map_err(|_| {
                        Problem::new(
                            "resource_unavailable",
                            "Resource service unavailable",
                            503,
                            "mcp",
                        )
                    })?
                    .list_page(&principal, &grants, &ws_id, cursor.as_ref(), limit)?;
                Ok(serde_json::json!({ "items": items, "nextCursor": next_cursor }))
            }
            "grant.create" => {
                let grant: buglogin_domain::grant::ResourceGrant =
                    serde_json::from_value(input.clone()).map_err(serialization_problem)?;
                state
                    .resources
                    .write()
                    .map_err(|_| {
                        Problem::new(
                            "resource_unavailable",
                            "Resource service unavailable",
                            503,
                            "mcp",
                        )
                    })?
                    .grant(&principal, &grants, grant.clone())?;
                if matches!(grant.action, Action::Read | Action::Execute) {
                    HostedProfileService::mark_resource_key_rotation_if_encrypted(
                        state,
                        &grant.workspace_id,
                        &grant.resource_id,
                    )?;
                }
                Ok(serde_json::to_value(grant).map_err(serialization_problem)?)
            }
            "grant.revoke" => {
                let id: GrantId =
                    serde_json::from_value(input.get("grantId").cloned().unwrap_or(Value::Null))
                        .map_err(serialization_problem)?;
                let existing = state
                    .resources
                    .read()
                    .map_err(|_| {
                        Problem::new(
                            "resource_unavailable",
                            "Resource service unavailable",
                            503,
                            "mcp",
                        )
                    })?
                    .get_grant(&principal, &grants, &id)?;
                state
                    .resources
                    .write()
                    .map_err(|_| {
                        Problem::new(
                            "resource_unavailable",
                            "Resource service unavailable",
                            503,
                            "mcp",
                        )
                    })?
                    .revoke(&principal, &grants, &id)?;
                if matches!(existing.action, Action::Read | Action::Execute) {
                    HostedProfileService::mark_resource_key_rotation_if_encrypted(
                        state,
                        &existing.workspace_id,
                        &existing.resource_id,
                    )?;
                }
                Ok(serde_json::json!({ "grantId": id, "revoked": true }))
            }
            "grant.list" | "grant.list.page" => {
                let workspace_id = request.scope.workspace_id.clone().ok_or_else(|| {
                    Problem::new(
                        "workspace_missing",
                        "Workspace is missing from scope",
                        400,
                        "mcp",
                    )
                })?;
                let resource_id = input
                    .get("resourceId")
                    .and_then(Value::as_str)
                    .map(ResourceId::from_str)
                    .transpose()
                    .map_err(|_| {
                        Problem::new("invalid_scope", "resourceId is invalid", 400, "mcp")
                    })?;
                let member_id = input
                    .get("memberId")
                    .and_then(Value::as_str)
                    .map(MemberId::from_str)
                    .transpose()
                    .map_err(|_| {
                        Problem::new("invalid_scope", "memberId is invalid", 400, "mcp")
                    })?;
                if request.command.id.as_str() == "grant.list" {
                    let items = state
                        .resources
                        .read()
                        .map_err(|_| {
                            Problem::new(
                                "resource_unavailable",
                                "Resource service unavailable",
                                503,
                                "mcp",
                            )
                        })?
                        .list_grants(
                            &principal,
                            &grants,
                            &workspace_id,
                            resource_id.as_ref(),
                            member_id.as_ref(),
                        )?;
                    Ok(serde_json::json!({ "items": items, "nextCursor": Value::Null }))
                } else {
                    let cursor = input
                        .get("cursor")
                        .and_then(Value::as_str)
                        .map(GrantId::from_str)
                        .transpose()
                        .map_err(|_| {
                            Problem::new("invalid_scope", "cursor is invalid", 400, "mcp")
                        })?;
                    let limit = input
                        .get("limit")
                        .and_then(Value::as_u64)
                        .map_or(Ok(100usize), |value| usize::try_from(value).map_err(|_| ()))
                        .map_err(|_| {
                            Problem::new(
                                "invalid_page_limit",
                                "Page limit must be between 1 and 100",
                                400,
                                "mcp",
                            )
                        })?;
                    let (items, next_cursor) = state
                        .resources
                        .read()
                        .map_err(|_| {
                            Problem::new(
                                "resource_unavailable",
                                "Resource service unavailable",
                                503,
                                "mcp",
                            )
                        })?
                        .list_grants_page(
                            &principal,
                            &grants,
                            &workspace_id,
                            resource_id.as_ref(),
                            member_id.as_ref(),
                            cursor.as_ref(),
                            limit,
                        )?;
                    Ok(serde_json::json!({ "items": items, "nextCursor": next_cursor }))
                }
            }
            "profile.get" => {
                let id = request.scope.profile_id.clone().ok_or_else(|| {
                    Problem::new(
                        "profile_missing",
                        "Profile is missing from scope",
                        400,
                        "mcp",
                    )
                })?;
                let profile = state.get_profile_record(&id)?.ok_or_else(|| {
                    Problem::new("profile_not_found", "Profile not found", 404, "mcp")
                })?;
                Ok(
                    serde_json::to_value(ProfileRecord::new(profile.value, profile.revision))
                        .map_err(serialization_problem)?,
                )
            }
            "resource.lease.get" => Err(problem(
                "async_execution_required",
                "Lease status requires asynchronous coordination",
                501,
            )),
            "profile.manifest.get" => {
                let workspace_id = required_id(&input, "workspaceId", WorkspaceId::from_str)?;
                let resource_id = required_id(&input, "resourceId", ResourceId::from_str)?;
                if request.scope.workspace_id.as_ref() != Some(&workspace_id)
                    || request.scope.resource_id.as_ref() != Some(&resource_id)
                {
                    return Err(problem(
                        "invalid_scope",
                        "Manifest resource scope does not match request",
                        400,
                    ));
                }
                let binding = state.profile_resource_for_resource(&workspace_id, &resource_id)?;
                crate::services::profile_sync::authorize_resource_access(
                    state,
                    &principal,
                    &grants,
                    &workspace_id,
                    &resource_id,
                    request.command.capability,
                    request.command.action,
                )?;
                let manifest = state.current_sync_manifest(&workspace_id, &resource_id)?;
                if let Some(manifest) = manifest.as_ref() {
                    if binding.is_some() {
                        if !(manifest.generation == 1
                            && manifest.objects.is_empty()
                            && manifest.tombstones.is_empty()
                            && manifest.dependencies.is_empty())
                        {
                            let _ = crate::services::profile_sync::validate_proxy_dependency(
                                state,
                                &principal,
                                &grants,
                                &workspace_id,
                                &resource_id,
                                manifest,
                                false,
                                false,
                            )?;
                        }
                    } else if !manifest.dependencies.is_empty() {
                        return Err(problem(
                            "manifest_dependency_profile_missing",
                            "Proxy dependencies require a bound profile",
                            409,
                        ));
                    }
                }
                Ok(serde_json::to_value(manifest).map_err(serialization_problem)?)
            }
            "profile.update" => {
                let update: crate::services::profile_hosted::ProfileUpdateInput =
                    serde_json::from_value(input.clone()).map_err(serialization_problem)?;
                if request.scope.workspace_id.as_ref() != Some(&update.workspace_id)
                    || request.scope.profile_id.as_ref() != Some(&update.profile_id)
                {
                    return Err(problem(
                        "invalid_scope",
                        "Profile update scope does not match request",
                        400,
                    ));
                }
                let updated =
                    HostedProfileService::update_metadata(state, &principal, &grants, update)?;
                Ok(
                    serde_json::to_value(ProfileRecord::new(updated.value, updated.revision))
                        .map_err(serialization_problem)?,
                )
            }
            "profile.create" => (|| {
                let key = request.idempotency_key.as_deref().ok_or_else(|| {
                    problem(
                        "idempotency_key_required",
                        "Idempotency key is required",
                        400,
                    )
                })?;
                let parsed = parse_profile_create_request(&input)?;
                let (create_input, legacy_profile) = match parsed {
                    ParsedProfileCreate::Command(input) => (input, None),
                    ParsedProfileCreate::Legacy(profile) => {
                        let workspace = input.get("workspaceId").and_then(Value::as_str);
                        (
                            ProfileCreationService::legacy_input(&profile, workspace),
                            Some(profile),
                        )
                    }
                };
                if let Some(legacy) = legacy_profile.as_ref() {
                    ProfileCreationService::validate_legacy_profile_data(
                        &legacy.runtime,
                        legacy.runtime_config.as_ref(),
                        legacy.fingerprint.as_ref(),
                    )?;
                }
                let managed = !state.local_authority_enabled();
                let workspace_id = ProfileCreationService::resolve_workspace(
                    create_input.workspace_id.as_deref(),
                    None,
                    &principal,
                    managed,
                )?;
                let profile_id = ProfileCreationService::profile_id(
                    create_input.id.as_deref(),
                    workspace_id.as_ref().map(WorkspaceId::as_str),
                    key,
                )?;
                let mut profile = ProfileCreationService::build(create_input, profile_id, managed)?;
                if let Some(legacy) = legacy_profile {
                    profile.runtime_config = legacy.runtime_config;
                    profile.fingerprint = legacy.fingerprint;
                    profile.vpn_id = legacy.vpn_id;
                    profile.group_ids = legacy.group_ids;
                    profile.tags = legacy.tags;
                    profile.extension_group_id = legacy.extension_group_id;
                }
                validate_profile_proxy(
                    state,
                    &profile,
                    workspace_id.as_ref(),
                    &principal,
                    &grants,
                )?;

                if state.local_authority_enabled() {
                    let created = state.create_profile_record(profile)?;
                    return serde_json::to_value(ProfileRecord::new(
                        created.value,
                        created.revision,
                    ))
                    .map_err(serialization_problem);
                }
                let workspace_id = workspace_id
                    .ok_or_else(|| problem("workspace_required", "workspaceId is required", 400))?;
                let resource_id = HostedProfileService::managed_resource_id(&profile.id)?;
                HostedProfileService::authorize_resource_creation(
                    &principal,
                    &grants,
                    &workspace_id,
                    &resource_id,
                )?;
                let plan = HostedProfileService::prepare_provision(profile, workspace_id)?;
                let created = HostedProfileService::provision(state, &principal, &grants, &plan)?;
                serde_json::to_value(ProfileRecord::new(created.value, created.revision))
                    .map_err(serialization_problem)
            })(),
            "profile.clone" => {
                let parsed: ProfileCloneInput =
                    serde_json::from_value(input.clone()).map_err(serialization_problem)?;
                if request.scope.profile_id.as_ref() != Some(&parsed.source_profile_id) {
                    return Err(problem(
                        "profile_scope_mismatch",
                        "Profile scope does not match source profile",
                        400,
                    ));
                }
                let receipt =
                    HostedProfileService::clone_metadata(state, &principal, &grants, parsed)?;
                serde_json::to_value(receipt).map_err(serialization_problem)
            }
            "profile.share" => {
                let parsed: ProfileShareInput =
                    serde_json::from_value(input.clone()).map_err(serialization_problem)?;
                if request.scope.profile_id.as_ref() != Some(&parsed.profile_id) {
                    return Err(problem(
                        "profile_scope_mismatch",
                        "Profile scope does not match profile",
                        400,
                    ));
                }
                let receipt = HostedProfileService::share(state, &principal, &grants, parsed)?;
                serde_json::to_value(receipt).map_err(serialization_problem)
            }
            "profile.share.revoke" => {
                let parsed: ProfileShareRevokeInput =
                    serde_json::from_value(input.clone()).map_err(serialization_problem)?;
                if request.scope.profile_id.as_ref() != Some(&parsed.profile_id) {
                    return Err(problem(
                        "profile_scope_mismatch",
                        "Profile scope does not match profile",
                        400,
                    ));
                }
                let receipt =
                    HostedProfileService::revoke_share(state, &principal, &grants, parsed)?;
                serde_json::to_value(receipt).map_err(serialization_problem)
            }
            "profile.list" => {
                let include_archived = input
                    .get("includeArchived")
                    .map(|value| {
                        value.as_bool().ok_or_else(|| {
                            problem(
                                "invalid_include_archived",
                                "includeArchived must be a boolean",
                                400,
                            )
                        })
                    })
                    .transpose()?
                    .unwrap_or(false);
                let workspace_id = input
                    .get("workspaceId")
                    .map(|value| {
                        value.as_str().ok_or_else(|| {
                            problem("invalid_workspace_id", "Invalid workspace ID", 400)
                        })
                    })
                    .transpose()?;
                let limit = input
                    .get("limit")
                    .map(|value| {
                        value
                            .as_u64()
                            .filter(|limit| *limit <= usize::MAX as u64)
                            .ok_or_else(|| {
                                problem("invalid_page_limit", "Profile page limit is invalid", 400)
                            })
                    })
                    .transpose()?
                    .map(|limit| limit as usize);
                let cursor = input
                    .get("cursor")
                    .map(|value| {
                        value.as_str().ok_or_else(|| {
                            problem("invalid_page_cursor", "Invalid page cursor", 400)
                        })
                    })
                    .transpose()?;
                let query = crate::services::profile_listing::ProfileListQuery::parse(
                    workspace_id,
                    cursor,
                    limit,
                    include_archived,
                )?;
                let page =
                    crate::services::profile_listing::list(state, &principal, &grants, query)?;
                let items = page
                    .items
                    .into_iter()
                    .map(|profile| ProfileRecord::new(profile.value, profile.revision))
                    .collect::<Vec<_>>();
                Ok(serde_json::json!({ "items": items, "nextCursor": page.next_cursor }))
            }
            "profile.archive" | "profile.restore" => {
                let id = request.scope.profile_id.clone().ok_or_else(|| {
                    Problem::new(
                        "profile_missing",
                        "Profile is missing from scope",
                        400,
                        "mcp",
                    )
                })?;
                let revision = input
                    .get("revision")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| problem("invalid_input", "revision is required", 400))?;
                let updated = if state.local_authority_enabled() {
                    let current = state.get_profile_record(&id)?.ok_or_else(|| {
                        Problem::new("profile_not_found", "Profile not found", 404, "mcp")
                    })?;
                    if request.command.id.as_str() == "profile.archive" {
                        state.archive_profile_record(revision, current.value)?
                    } else {
                        state.restore_profile_record(revision, current.value)?
                    }
                } else {
                    state.mutate_hosted_profile_archive_state(
                        &id,
                        revision,
                        request.command.id.as_str() == "profile.archive",
                    )?
                };
                Ok(
                    serde_json::to_value(ProfileRecord::new(updated.value, updated.revision))
                        .map_err(serialization_problem)?,
                )
            }
            "profile.delete" => {
                let id = request.scope.profile_id.clone().ok_or_else(|| {
                    Problem::new(
                        "profile_missing",
                        "Profile is missing from scope",
                        400,
                        "mcp",
                    )
                })?;
                let revision = input
                    .get("revision")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| problem("invalid_input", "revision is required", 400))?;
                let confirmation = input
                    .get("confirmation")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if state.local_authority_enabled() {
                    serde_json::to_value(ProfileDeletionService::delete_local(
                        state,
                        &id,
                        revision,
                        confirmation,
                    )?)
                    .map_err(serialization_problem)
                } else {
                    let deleted = HostedProfileService::delete_to_quarantine(
                        state,
                        &id,
                        revision,
                        confirmation,
                        principal.account_id.clone(),
                    )?;
                    Ok(serde_json::json!({
                        "id": deleted.id,
                        "deleted": deleted.deleted,
                        "recoverable": true,
                    }))
                }
            }
            "profile.launch" => {
                if !state.local_authority_enabled() {
                    return enqueue_runtime_dispatch(
                        state,
                        &principal,
                        &request.scope,
                        &input,
                        request.idempotency_key.as_deref(),
                        "profile.launch",
                        &request.correlation_id,
                    );
                }
                let profile_id = request.scope.profile_id.clone().ok_or_else(|| {
                    Problem::new(
                        "profile_missing",
                        "Profile is missing from scope",
                        400,
                        "mcp",
                    )
                })?;
                let profiles_root = input
                    .get("profilesRoot")
                    .and_then(Value::as_str)
                    .ok_or_else(|| problem("invalid_input", "profilesRoot is required", 400))?;
                let extra_args = input
                    .get("extraArgs")
                    .cloned()
                    .map(serde_json::from_value)
                    .transpose()
                    .map_err(serialization_problem)?
                    .unwrap_or_default();
                let actor_id = principal
                    .account_id
                    .as_ref()
                    .map_or("local", buglogin_domain::ids::AccountId::as_str);
                let operation_id = runtime_operation_id(
                    request.command.id.as_str(),
                    actor_id,
                    request.scope.workspace_id.as_ref(),
                    &profile_id,
                    request.idempotency_key.as_deref(),
                )?;
                state.mutate_operation_locked(
                    &operation_id,
                    request.command.id.as_str(),
                    "started",
                    actor_id,
                    |operations| {
                        operations.insert(crate::services::operations::Operation::queued(
                            operation_id.clone(),
                            request.command.id.as_str(),
                            request.scope.clone(),
                        )?)?;
                        operations.transition(
                            &operation_id,
                            crate::services::operations::OperationStatus::Running,
                            Some(0.1),
                            None,
                            None,
                        )?;
                        Ok(())
                    },
                )?;
                let lifecycle_service = crate::http::profile_lifecycle::ProfileLifecycleService;
                let result = lifecycle_service.launch(
                    &state.profiles,
                    &state.lifecycles,
                    &state.runtime_binaries,
                    crate::http::profile_lifecycle::ProfileLifecycleCommand {
                        profile_id,
                        profiles_root: profiles_root.into(),
                        extra_args,
                    },
                );
                match result {
                    Ok(value) => {
                        let completed = state.mutate_operation_locked(
                            &operation_id,
                            "operation.complete",
                            "completed",
                            actor_id,
                            |operations| {
                                operations.transition(
                                    &operation_id,
                                    crate::services::operations::OperationStatus::Completed,
                                    Some(1.0),
                                    Some(value.clone()),
                                    None,
                                )?;
                                Ok(())
                            },
                        )?;
                        Ok(
                            serde_json::json!({ "operationId": operation_id, "result": completed.result }),
                        )
                    }
                    Err(error) => {
                        state.mutate_operation_locked(
                            &operation_id,
                            "operation.fail",
                            "failed",
                            actor_id,
                            |operations| {
                                operations.transition(
                                    &operation_id,
                                    crate::services::operations::OperationStatus::Failed,
                                    Some(1.0),
                                    None,
                                    Some(error.clone()),
                                )?;
                                Ok(())
                            },
                        )?;
                        Err(error)
                    }
                }
            }
            "profile.stop" => {
                if !state.local_authority_enabled() {
                    return enqueue_runtime_dispatch(
                        state,
                        &principal,
                        &request.scope,
                        &input,
                        request.idempotency_key.as_deref(),
                        "profile.stop",
                        &request.correlation_id,
                    );
                }
                let profile_id = request.scope.profile_id.clone().ok_or_else(|| {
                    Problem::new(
                        "profile_missing",
                        "Profile is missing from scope",
                        400,
                        "mcp",
                    )
                })?;
                let actor_id = principal
                    .account_id
                    .as_ref()
                    .map_or("local", buglogin_domain::ids::AccountId::as_str);
                let operation_id = runtime_operation_id(
                    request.command.id.as_str(),
                    actor_id,
                    request.scope.workspace_id.as_ref(),
                    &profile_id,
                    request.idempotency_key.as_deref(),
                )?;
                state.mutate_operation_locked(
                    &operation_id,
                    request.command.id.as_str(),
                    "started",
                    actor_id,
                    |operations| {
                        operations.insert(crate::services::operations::Operation::queued(
                            operation_id.clone(),
                            request.command.id.as_str(),
                            request.scope.clone(),
                        )?)?;
                        operations.transition(
                            &operation_id,
                            crate::services::operations::OperationStatus::Running,
                            Some(0.1),
                            None,
                            None,
                        )?;
                        Ok(())
                    },
                )?;
                let lifecycle_service = crate::http::profile_lifecycle::ProfileLifecycleService;
                let result = lifecycle_service.stop(&state.profiles, &state.lifecycles, profile_id);
                match result {
                    Ok(value) => {
                        let completed = state.mutate_operation_locked(
                            &operation_id,
                            "operation.complete",
                            "completed",
                            actor_id,
                            |operations| {
                                operations.transition(
                                    &operation_id,
                                    crate::services::operations::OperationStatus::Completed,
                                    Some(1.0),
                                    Some(value.clone()),
                                    None,
                                )?;
                                Ok(())
                            },
                        )?;
                        Ok(
                            serde_json::json!({ "operationId": operation_id, "result": completed.result }),
                        )
                    }
                    Err(error) => {
                        state.mutate_operation_locked(
                            &operation_id,
                            "operation.fail",
                            "failed",
                            actor_id,
                            |operations| {
                                operations.transition(
                                    &operation_id,
                                    crate::services::operations::OperationStatus::Failed,
                                    Some(1.0),
                                    None,
                                    Some(error.clone()),
                                )?;
                                Ok(())
                            },
                        )?;
                        Err(error)
                    }
                }
            }
            _ => Err(Problem::new(
                "unsupported_mcp_command",
                "MCP command is unsupported by executor",
                501,
                "mcp",
            )),
        };

        if is_mutation {
            if result.is_err() {
                if let Some(mut before) = before_state {
                    // Operation outcomes are durable records in the same
                    // control aggregate; retain a failed operation while
                    // rolling back any other in-memory mutation from this call.
                    before.operations = state.control_aggregate()?.operations;
                    state.replace_control_aggregate(before)?;
                }
            }
        }

        result
    }

    fn execute_in_transaction(&mut self, request: ExecutionRequest<'_>) -> Result<Value, Problem> {
        let previous = self.transaction_guarded.replace(true);
        let result = self.execute(request);
        self.transaction_guarded.set(previous);
        result
    }

    async fn execute_async(&mut self, request: ExecutionRequest<'_>) -> Result<Value, Problem> {
        if request.command.id.as_str() == "migration.package.activate" {
            let package_id = request
                .input
                .get("packageId")
                .and_then(Value::as_str)
                .ok_or_else(|| problem("invalid_input", "packageId is required", 400))?;
            let bearer = request.bearer_token.as_deref().ok_or_else(|| {
                problem("authentication_required", "Authentication required", 401)
            })?;
            let idempotency_key = request.idempotency_key.as_deref().ok_or_else(|| {
                problem("idempotency_key_required", "Idempotency key required", 400)
            })?;
            let mut headers = axum::http::HeaderMap::new();
            headers.insert(
                axum::http::header::AUTHORIZATION,
                axum::http::HeaderValue::from_str(&format!("Bearer {bearer}")).map_err(|_| {
                    problem("authentication_required", "Authentication required", 401)
                })?,
            );
            headers.insert(
                "x-correlation-id",
                axum::http::HeaderValue::from_str(&request.correlation_id).map_err(|_| {
                    problem("invalid_correlation_id", "Correlation ID is invalid", 400)
                })?,
            );
            headers.insert(
                "idempotency-key",
                axum::http::HeaderValue::from_str(idempotency_key).map_err(|_| {
                    problem("invalid_idempotency_key", "Idempotency key is invalid", 400)
                })?,
            );
            let response = crate::http::package_upload::activate_package_upload(
                axum::extract::State(self.state.clone()),
                axum::extract::Path(package_id.to_owned()),
                headers,
            )
            .await;
            let response = match response {
                Ok(response) => response,
                Err(response) => {
                    return Err(crate::http::package_upload::response_problem(response).await);
                }
            };
            let status = response.status();
            let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
                .await
                .map_err(|_| {
                    problem(
                        "migration_package_activation_failed",
                        "Migration package activation failed",
                        500,
                    )
                })?;
            if !status.is_success() {
                return Err(serde_json::from_slice(&bytes).unwrap_or_else(|_| {
                    problem(
                        "migration_package_activation_failed",
                        "Migration package activation failed",
                        status.as_u16(),
                    )
                }));
            }
            return serde_json::from_slice(&bytes).map_err(serialization_problem);
        }
        if request.command.id.as_str() == "resource.lease.get" {
            let workspace_id = required_id(&request.input, "workspaceId", WorkspaceId::from_str)?;
            let resource_id = required_id(&request.input, "resourceId", ResourceId::from_str)?;
            if request.scope.workspace_id.as_ref() != Some(&workspace_id)
                || request.scope.resource_id.as_ref() != Some(&resource_id)
            {
                return Err(problem(
                    "invalid_scope",
                    "Lease resource scope does not match request",
                    400,
                ));
            }
            let grants = self.state.grants()?;
            crate::services::profile_sync::authorize_resource_access(
                &self.state,
                &request.principal,
                &grants,
                &workspace_id,
                &resource_id,
                request.command.capability,
                request.command.action,
            )?;
            let auth = request.coordination_auth.as_ref().ok_or_else(|| {
                problem("authentication_required", "Authentication required", 401)
            })?;
            let status = self
                .state
                .coordination()?
                .lease_status(
                    buglogin_sync::coordination::CoordinationScope {
                        workspace_id: &workspace_id,
                        auth,
                    },
                    &resource_id,
                )
                .await?;
            return serde_json::to_value(status).map_err(serialization_problem);
        }
        if request.command.id.as_str() != "events.stream" {
            return self.execute(request);
        }
        let workspace_id =
            request.scope.workspace_id.as_ref().ok_or_else(|| {
                problem("workspace_missing", "Workspace is missing from scope", 400)
            })?;
        let auth = request
            .coordination_auth
            .as_ref()
            .ok_or_else(|| problem("authentication_required", "Authentication required", 401))?;
        let after = request.input.get("after").and_then(Value::as_str);
        let limit = request
            .input
            .get("limit")
            .and_then(Value::as_u64)
            .and_then(|value| usize::try_from(value).ok())
            .unwrap_or(100);
        crate::http::read_workspace_events(&self.state, auth, workspace_id, after, limit).await
    }
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct McpProxyMetadata {
    id: buglogin_domain::ids::ProxyId,
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

impl McpProxyMetadata {
    fn into_proxy(self) -> Result<Proxy, Problem> {
        let credential_ref = if self.has_credential.unwrap_or(false) {
            Some(credential_version(&self.id, 1)?)
        } else {
            None
        };
        Ok(Proxy {
            id: self.id,
            name: self.name,
            protocol: self.protocol,
            host: self.host,
            port: self.port,
            credential_ref,
            rotation_url: None,
            bypass_rules: self.bypass_rules,
            created_at: self.created_at,
            updated_at: self.updated_at,
        })
    }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct McpProxyView {
    id: buglogin_domain::ids::ProxyId,
    name: Option<String>,
    protocol: ProxyProtocol,
    host: String,
    port: u16,
    has_credential: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    credential_version: Option<buglogin_domain::ids::CredentialId>,
    rotation_configured: bool,
    bypass_rules: Vec<String>,
    created_at: String,
    updated_at: String,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct McpProxyRecordView {
    workspace_id: WorkspaceId,
    proxy: McpProxyView,
    revision: u64,
}

fn proxy_record_view(record: ProxyRecord) -> Value {
    serde_json::to_value(McpProxyRecordView {
        workspace_id: record.workspace_id,
        proxy: McpProxyView {
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
    })
    .expect("proxy view serialization")
}

fn proxy_parse_view(report: buglogin_network::proxy::ParseReport) -> Value {
    let valid_count = report.rows.len();
    let invalid_count = report.errors.len();
    let mut rows = report
        .rows
        .into_iter()
        .map(|row| {
            serde_json::json!({
                "protocol": format!("{:?}", row.protocol).to_ascii_lowercase(),
                "endpoint": format!("{}:{}", row.host, row.port),
                "valid": true,
                "line": Value::Null,
                "error": Value::Null
            })
        })
        .collect::<Vec<_>>();
    rows.extend(report.errors.into_iter().map(|error| {
        serde_json::json!({
            "protocol": Value::Null,
            "endpoint": Value::Null,
            "valid": false,
            "line": error.line,
            "error": error.code
        })
    }));
    serde_json::json!({
        "validCount": valid_count,
        "invalidCount": invalid_count,
        "duplicateCount": report.duplicates,
        "rows": rows
    })
}

fn validate_profile_proxy(
    state: &crate::http::ApiState,
    profile: &Profile,
    workspace_id: Option<&WorkspaceId>,
    principal: &Principal,
    grants: &[buglogin_domain::permissions::Grant],
) -> Result<(), Problem> {
    let Some(proxy_id) = profile.proxy_id.as_ref() else {
        return Ok(());
    };
    let workspace_id = workspace_id
        .ok_or_else(|| problem("workspace_required", "workspaceId is required", 400))?;
    let repository = state
        .proxies
        .read()
        .map_err(|_| problem("proxy_unavailable", "Proxy service unavailable", 503))?;
    if state.local_authority_enabled() {
        if repository.snapshot().records.iter().any(|record| {
            record.workspace_id == workspace_id.as_str() && record.proxy.id == *proxy_id
        }) {
            Ok(())
        } else {
            Err(problem("proxy_not_found", "Proxy not found", 404))
        }
    } else {
        repository
            .get(workspace_id, proxy_id, principal, grants)
            .map(|_| ())
            .map_err(proxy_problem)
    }
}

fn proxy_problem(error: RepositoryError) -> Problem {
    crate::services::profile_sync::proxy_repository_problem(error)
}

#[cfg(test)]
mod scope_tests {
    use super::*;
    use crate::commands::CommandRegistry;
    use buglogin_domain::{
        audit::{AuditOutcome, AuditResource},
        ids::{AccountId, ResourceId},
        permissions::{Effect, Grant, Role},
        profile::{ProfileArchiveState, ProfileResource},
    };

    #[test]
    fn mcp_boundary_applies_request_correlation_to_service_problems() {
        let error = Problem::new("proxy_in_use", "Proxy is in use", 409, "proxy")
            .with_instance("service-instance");

        let error = with_correlation_id(error, "mcp-request-0001");

        assert_eq!(error.correlation_id, "mcp-request-0001");
        assert_eq!(error.instance.as_deref(), Some("service-instance"));
        assert_eq!(error.code, "proxy_in_use");
    }

    #[test]
    fn lease_status_allows_fencing_counter_but_rejects_owner_token() {
        let command = CommandRegistry::get("resource.lease.get").unwrap();
        assert!(
            reject_mcp_output_secrets(
                command,
                &serde_json::json!({"ownerId":"acct_1","fencingToken":7})
            )
            .is_ok()
        );
        assert_eq!(
            reject_mcp_output_secrets(
                command,
                &serde_json::json!({"ownerId":"acct_1","ownerToken":"secret"})
            )
            .unwrap_err()
            .code,
            "secret_material_rejected"
        );
    }

    #[test]
    fn profile_manifest_allows_fencing_counters_and_encrypted_object_references() {
        let command = CommandRegistry::get("profile.manifest.get").unwrap();
        let safe_manifest_metadata = serde_json::json!({
            "fencingToken": 7,
            "dependencies": [{
                "credentialVersion": "cred_version_1",
                "credentialEnvelopeObject": {
                    "key": "profile/proxy-credential-envelope",
                    "size": 80,
                    "checksum": "a".repeat(64),
                },
            }],
        });
        assert!(reject_mcp_output_secrets(&command, &safe_manifest_metadata).is_ok());

        let malformed_reference = serde_json::json!({
            "dependencies": [{"credentialEnvelopeObject": "password=plaintext"}],
        });
        assert_eq!(
            reject_mcp_output_secrets(&command, &malformed_reference)
                .unwrap_err()
                .code,
            "secret_material_rejected"
        );
    }

    #[test]
    fn public_runtime_catalog_has_global_scope_without_workspace_authority() {
        let command = CommandRegistry::get("runtime.catalog.get").unwrap();
        let principal = Principal::default();

        assert!(!command.auth_required);
        assert_eq!(
            resolve_scope(&principal, command, &Value::Null).unwrap(),
            ResourceScope::global()
        );
    }

    #[test]
    fn runtime_operation_identity_uses_actor_scope_and_idempotency_key() {
        let workspace = WorkspaceId::from_str("ws_1").unwrap();
        let profile = ProfileId::from_str("prf_1").unwrap();
        let first = runtime_operation_id(
            "profile.launch",
            "acc_1",
            Some(&workspace),
            &profile,
            Some("launch-request-0001"),
        )
        .unwrap();
        let replay = runtime_operation_id(
            "profile.launch",
            "acc_1",
            Some(&workspace),
            &profile,
            Some("launch-request-0001"),
        )
        .unwrap();
        let distinct_request = runtime_operation_id(
            "profile.launch",
            "acc_1",
            Some(&workspace),
            &profile,
            Some("launch-request-0002"),
        )
        .unwrap();
        let other_actor = runtime_operation_id(
            "profile.launch",
            "acc_2",
            Some(&workspace),
            &profile,
            Some("launch-request-0001"),
        )
        .unwrap();
        let other_command = runtime_operation_id(
            "profile.stop",
            "acc_1",
            Some(&workspace),
            &profile,
            Some("launch-request-0001"),
        )
        .unwrap();
        let other_workspace = WorkspaceId::from_str("ws_2").unwrap();
        let other_scope = runtime_operation_id(
            "profile.launch",
            "acc_1",
            Some(&other_workspace),
            &profile,
            Some("launch-request-0001"),
        )
        .unwrap();

        assert_eq!(first, replay);
        assert_ne!(first, distinct_request);
        assert_ne!(first, other_actor);
        assert_ne!(first, other_command);
        assert_ne!(first, other_scope);
        assert!(
            runtime_operation_id("profile.launch", "acc_1", Some(&workspace), &profile, None,)
                .is_err()
        );
    }

    #[test]
    fn only_pre_side_effect_authentication_failures_release_mcp_claims() {
        assert!(should_abandon_mcp_claim(&problem(
            "authorization_state_changed",
            "Authorization changed",
            409,
        )));
        assert!(should_abandon_mcp_claim(&problem(
            "authentication_required",
            "Authentication required",
            401,
        )));
        assert!(should_abandon_mcp_claim(&problem(
            "permission_denied",
            "Permission denied",
            403,
        )));
        assert!(!should_abandon_mcp_claim(
            &problem("storage_unavailable", "Storage unavailable", 503).retryable(true)
        ));
        assert!(!should_abandon_mcp_claim(&problem(
            "profile_launch_failed",
            "Profile launch failed",
            503,
        )));
    }

    #[test]
    fn workspace_create_scope_uses_the_nested_workspace_and_organization_ids() {
        let command = CommandRegistry::get("workspace.create").unwrap();
        let input = serde_json::json!({
            "organizationId": "org_scope_create",
            "workspace": { "id": "ws_scope_create" }
        });

        let scope = resolve_scope(&Principal::default(), command, &input).unwrap();

        assert_eq!(
            scope.organization_id.as_ref().map(OrganizationId::as_str),
            Some("org_scope_create")
        );
        assert_eq!(
            scope.workspace_id.as_ref().map(WorkspaceId::as_str),
            Some("ws_scope_create")
        );
    }

    #[test]
    fn audit_events_list_uses_the_workspace_scope_and_shared_page_service() {
        let workspace_id = WorkspaceId::from_str("ws_mcp_audit").unwrap();
        let account_id = AccountId::from_str("acct_mcp_audit").unwrap();
        let state =
            crate::http::ApiState::local(buglogin_browser::profile::ProfileService::disabled());
        state.grants.write().unwrap().push(Grant {
            id: GrantId::from_str("grant_mcp_audit_read").unwrap(),
            account_id: Some(account_id.clone()),
            member_id: None,
            capability: Capability::new("CAP-AUD-001").unwrap(),
            action: Action::Read,
            scope: ResourceScope::workspace(workspace_id.clone()),
            effect: Effect::Allow,
        });
        state
            .audit
            .write()
            .unwrap()
            .append(
                buglogin_domain::audit::AuditEvent::new(
                    "evt_mcp_audit",
                    100,
                    account_id.clone(),
                    AuditResource {
                        resource_type: "workspace".into(),
                        resource_id: workspace_id.as_str().into(),
                        scope: ResourceScope::workspace(workspace_id.clone()),
                    },
                    "workspace.update",
                    "cor_mcp_audit",
                    AuditOutcome::Succeeded,
                    Default::default(),
                )
                .unwrap(),
            )
            .unwrap();

        let principal = Principal {
            account_id: Some(account_id),
            workspace_ids: [workspace_id.clone()].into_iter().collect(),
            roles: [Role::WorkspaceMember].into_iter().collect(),
            ..Principal::default()
        };
        let command = CommandRegistry::get("audit.events.list").unwrap();
        let input = serde_json::json!({"workspaceId": workspace_id, "limit": 10});
        let mut policy = ProductionMcpPolicy::new(state.clone());
        let scope = policy.resolve_scope(&principal, command, &input).unwrap();
        policy.authorize(&principal, command, &scope).unwrap();
        let result = ProductionMcpExecutor::new(state)
            .execute(ExecutionRequest {
                command,
                correlation_id: "cor_mcp_audit_query".into(),
                principal,
                scope,
                input,
                idempotency_key: None,
                coordination_auth: None,
                bearer_token: None,
            })
            .unwrap();

        assert_eq!(result["items"].as_array().unwrap().len(), 1);
        assert_eq!(result["items"][0]["eventId"], "evt_mcp_audit");
        assert_eq!(result["nextCursor"], serde_json::Value::Null);
    }

    #[test]
    fn direct_resource_grants_through_mcp_fence_encrypted_profiles() {
        use buglogin_domain::{
            entities::{Profile, ProfileState, Runtime, SyncMode},
            profile::ProfileResource,
            resource::{Resource, ResourceKind, ResourceState},
        };
        use buglogin_sync::manifest::{
            ManifestObject, ManifestObjectReference, ManifestPayloadEncryption,
            PayloadEncryptionAlgorithm, SyncManifest,
        };

        let workspace_id = WorkspaceId::from_str("ws_mcp_grant_fence").unwrap();
        let profile_id = ProfileId::from_str("prf_mcp_grant_fence").unwrap();
        let resource_id = ResourceId::from_str("res_mcp_grant_fence").unwrap();
        let owner_id = AccountId::from_str("acct_mcp_grant_fence").unwrap();
        let member_id = MemberId::from_str("mem_mcp_grant_fence").unwrap();
        let state = crate::http::ApiState::hosted_with_control_store(
            buglogin_browser::profile::ProfileService::disabled(),
            buglogin_storage::control::MemoryControlRecordStore::default(),
        )
        .unwrap()
        .with_profile_resource(ProfileResource {
            profile_id: profile_id.clone(),
            resource_id: resource_id.clone(),
            workspace_id: workspace_id.clone(),
            archive_state: ProfileArchiveState::Active,
        });
        let principal = Principal {
            account_id: Some(owner_id.clone()),
            workspace_ids: [workspace_id.clone()].into_iter().collect(),
            roles: [Role::WorkspaceAdmin].into_iter().collect(),
            ..Principal::default()
        };
        let grants = vec![
            Grant {
                id: GrantId::from_str("grant_mcp_fence_resource_create").unwrap(),
                account_id: Some(owner_id.clone()),
                member_id: None,
                capability: Capability::new("CAP-RES-001").unwrap(),
                action: Action::Create,
                scope: ResourceScope::workspace(workspace_id.clone()),
                effect: Effect::Allow,
            },
            Grant {
                id: GrantId::from_str("grant_mcp_fence_share_manage").unwrap(),
                account_id: Some(owner_id.clone()),
                member_id: None,
                capability: Capability::new("CAP-SHARE-001").unwrap(),
                action: Action::Manage,
                scope: ResourceScope::resource(workspace_id.clone(), resource_id.clone()),
                effect: Effect::Allow,
            },
        ];
        state.grants.write().unwrap().extend(grants.clone());
        state
            .create_profile_record(Profile {
                id: profile_id,
                name: "Encrypted MCP grant fence".into(),
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
                created_at: "2026-10-15T00:00:00Z".into(),
                updated_at: "2026-10-15T00:00:00Z".into(),
            })
            .unwrap();
        let object = ManifestObject::new(
            buglogin_sync::manifest::RESOURCE_KEY_ENVELOPE_KEY,
            b"encrypted-profile-key-envelope",
        )
        .unwrap();
        let mut manifest = SyncManifest::new(
            workspace_id.clone(),
            resource_id.clone(),
            1,
            1,
            None,
            b"mcp-grant-fence",
            vec![object.clone()],
        )
        .unwrap();
        manifest.payload_encryption = Some(ManifestPayloadEncryption {
            schema_version: 1,
            algorithm: PayloadEncryptionAlgorithm::Aes256GcmChunkV1,
            key_version: None,
            resource_key_envelope: ManifestObjectReference::from(&object),
        });
        state.replace_sync_manifest(manifest).unwrap();
        state
            .resources
            .write()
            .unwrap()
            .register(
                &principal,
                &grants,
                Resource {
                    id: resource_id.clone(),
                    workspace_id: workspace_id.clone(),
                    kind: ResourceKind::Profile,
                    state: ResourceState::Active,
                    display_name: "Encrypted MCP grant fence".into(),
                },
            )
            .unwrap();

        let grant = buglogin_domain::grant::ResourceGrant {
            id: GrantId::from_str("grant_mcp_fence_read").unwrap(),
            workspace_id: workspace_id.clone(),
            resource_id: resource_id.clone(),
            member_id,
            action: Action::Read,
            effect: Effect::Allow,
        };
        let create = CommandRegistry::get("grant.create").unwrap();
        ProductionMcpExecutor::new(state.clone())
            .execute(ExecutionRequest {
                command: create,
                correlation_id: "cor_mcp_grant_fence_create".into(),
                principal: principal.clone(),
                scope: ResourceScope::resource(workspace_id.clone(), resource_id.clone()),
                input: serde_json::to_value(&grant).unwrap(),
                idempotency_key: Some("mcp-grant-fence-create-0001".into()),
                coordination_auth: None,
                bearer_token: None,
            })
            .unwrap();
        assert!(
            state
                .resource_key_rotation_pending(&workspace_id, &resource_id)
                .unwrap()
        );

        state
            .clear_resource_key_rotation_pending(&workspace_id, &resource_id)
            .unwrap();
        let revoke = CommandRegistry::get("grant.revoke").unwrap();
        ProductionMcpExecutor::new(state.clone())
            .execute(ExecutionRequest {
                command: revoke,
                correlation_id: "cor_mcp_grant_fence_revoke".into(),
                principal,
                scope: ResourceScope::resource(workspace_id.clone(), resource_id.clone()),
                input: serde_json::json!({
                    "workspaceId": workspace_id,
                    "grantId": grant.id,
                }),
                idempotency_key: Some("mcp-grant-fence-revoke-0001".into()),
                coordination_auth: None,
                bearer_token: None,
            })
            .unwrap();
        assert!(
            state
                .resource_key_rotation_pending(&workspace_id, &resource_id)
                .unwrap()
        );
    }

    #[test]
    fn hosted_profile_get_uses_canonical_workspace_scope_for_authorization() {
        let workspace_id = WorkspaceId::from_str("ws_mcp_scope").unwrap();
        let profile_id = ProfileId::from_str("prf_mcp_scope").unwrap();
        let account_id = AccountId::from_str("acct_mcp_scope").unwrap();
        let state = crate::http::ApiState::hosted_with_control_store(
            buglogin_browser::profile::ProfileService::disabled(),
            buglogin_storage::control::MemoryControlRecordStore::default(),
        )
        .unwrap()
        .with_profile_resource(ProfileResource {
            profile_id: profile_id.clone(),
            resource_id: ResourceId::from_str("res_mcp_scope").unwrap(),
            workspace_id: workspace_id.clone(),
            archive_state: ProfileArchiveState::Active,
        });
        state.grants.write().unwrap().push(Grant {
            id: GrantId::from_str("grant_mcp_workspace_profile_read").unwrap(),
            account_id: Some(account_id.clone()),
            member_id: None,
            capability: Capability::new("profile.read").unwrap(),
            action: Action::Read,
            scope: ResourceScope::workspace(workspace_id.clone()),
            effect: Effect::Allow,
        });
        state.grants.write().unwrap().push(Grant {
            id: GrantId::from_str("grant_mcp_workspace_team_create").unwrap(),
            account_id: Some(AccountId::from_str("acct_mcp_scope").unwrap()),
            member_id: None,
            capability: Capability::new("CAP-TEAM-001").unwrap(),
            action: Action::Create,
            scope: ResourceScope::workspace(workspace_id.clone()),
            effect: Effect::Allow,
        });
        let principal = Principal {
            account_id: Some(account_id),
            workspace_ids: [workspace_id.clone()].into_iter().collect(),
            roles: [Role::WorkspaceMember].into_iter().collect(),
            ..Principal::default()
        };
        let command = CommandRegistry::get("profile.get").unwrap();
        let mut policy = ProductionMcpPolicy::new(state);

        let scope = policy
            .resolve_scope(
                &principal,
                command,
                &serde_json::json!({ "profileId": profile_id }),
            )
            .unwrap();

        assert_eq!(scope.workspace_id, Some(workspace_id.clone()));
        assert_eq!(
            scope.resource_id.as_ref().unwrap().as_str(),
            "res_mcp_scope"
        );
        policy.authorize(&principal, command, &scope).unwrap();

        let team_command = CommandRegistry::get("team.create").unwrap();
        let team_scope = policy
            .resolve_scope(
                &principal,
                team_command,
                &serde_json::json!({ "workspaceId": workspace_id.clone() }),
            )
            .unwrap();
        assert_eq!(team_scope.workspace_id, Some(workspace_id.clone()));
        policy
            .authorize(&principal, team_command, &team_scope)
            .unwrap();

        let grants = policy.state.grants.read().unwrap();
        assert!(
            crate::services::profile_hosted::HostedProfileService::authorized(
                &policy.state,
                &principal,
                &grants,
                &ProfileId::from_str("prf_mcp_scope").unwrap(),
                Action::Read,
                "profile.read",
            )
            .unwrap()
        );
        assert!(
            !crate::services::profile_hosted::HostedProfileService::authorized(
                &policy.state,
                &principal,
                &grants,
                &ProfileId::from_str("prf_mcp_unrelated").unwrap(),
                Action::Read,
                "profile.read",
            )
            .unwrap()
        );
        drop(grants);
        policy.state.grants.write().unwrap().push(Grant {
            id: GrantId::from_str("grant_mcp_profile_read_deny").unwrap(),
            account_id: principal.account_id.clone(),
            member_id: None,
            capability: Capability::new("profile.read").unwrap(),
            action: Action::Read,
            scope: ResourceScope::resource(
                workspace_id,
                ResourceId::from_str("res_mcp_scope").unwrap(),
            ),
            effect: Effect::Deny,
        });
        let grants = policy.state.grants.read().unwrap();
        assert!(
            !crate::services::profile_hosted::HostedProfileService::authorized(
                &policy.state,
                &principal,
                &grants,
                &ProfileId::from_str("prf_mcp_scope").unwrap(),
                Action::Read,
                "profile.read",
            )
            .unwrap()
        );
    }
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct OrganizationInput {
    organization: Organization,
    policy: OrganizationPolicy,
}

#[cfg(test)]
mod input_fixture_tests {
    use super::*;

    #[test]
    fn every_mcp_input_schema_has_a_valid_fixture_and_rejects_null() {
        let fixtures: BTreeMap<String, Value> = serde_json::from_str(include_str!(
            "../../../../tests/contracts/mcp-input-fixtures.json"
        ))
        .expect("MCP schema fixtures must be valid JSON");
        let commands = CommandRegistry::all()
            .iter()
            .filter(|command| command.adapters.contains(&Adapter::Mcp))
            .collect::<Vec<_>>();
        let schemas = commands
            .iter()
            .map(|command| command.input_schema.as_str().to_owned())
            .collect::<std::collections::BTreeSet<_>>();

        assert_eq!(
            fixtures
                .keys()
                .cloned()
                .collect::<std::collections::BTreeSet<_>>(),
            schemas,
            "MCP input-schema fixtures must exactly match active MCP inputs"
        );
        for schema_name in schemas {
            let fixture = fixtures
                .get(&schema_name)
                .unwrap_or_else(|| panic!("missing fixture for MCP input schema {schema_name}"));
            let command = commands
                .iter()
                .find(|command| command.input_schema.as_str() == schema_name.as_str())
                .expect("schema is used by an MCP command");

            validate_mcp_input(command, fixture)
                .unwrap_or_else(|error| panic!("invalid fixture for {schema_name}: {error}"));
            assert!(
                validate_mcp_input(command, &Value::Null).is_err(),
                "{schema_name} unexpectedly accepts null"
            );
        }
    }

    #[test]
    fn profile_launch_mcp_schema_requires_profile_scope_without_host_paths() {
        let command = CommandRegistry::get("profile.launch").expect("profile launch command");
        let input = serde_json::json!({
            "profileId": "prf_mcp_launch",
            "targetDeviceId": "dev_0123456789abcdef0123456789abcdef",
        });

        validate_mcp_input(command, &input)
            .expect("hosted launch needs a profile scope and may target an owned device");
        assert!(validate_mcp_input(command, &serde_json::json!({})).is_err());
        assert!(validate_mcp_input(
            command,
            &serde_json::json!({"profileId": "prf_mcp_launch", "unknown": true})
        )
        .is_err());

        let stop = CommandRegistry::get("profile.stop").expect("profile stop command");
        validate_mcp_input(
            stop,
            &serde_json::json!({"profileId": "prf_mcp_launch", "targetDeviceId": "dev_0123456789abcdef0123456789abcdef"}),
        )
        .expect("stop can target the same owned desktop device");
    }
}
