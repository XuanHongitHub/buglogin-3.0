pub mod transport;

use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicU64, Ordering},
};

use buglogin_domain::{
    entities::{Profile, Proxy, ProxyProtocol},
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
use serde_json::Value;

use crate::commands::{Adapter, CommandMetadata, CommandRegistry};
use crate::services::{operations::reject_secrets, profile_hosted::HostedProfileService};
use buglogin_network::proxy::{
    ProxyAssignmentRepository, ProxyDelete, ProxyRecord, ProxyUpdate, RepositoryError,
};

pub const ENABLED_BY_DEFAULT: bool = false;
static AUDIT_SEQUENCE: AtomicU64 = AtomicU64::new(1);

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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionRequest<'a> {
    pub command: &'a CommandMetadata,
    pub correlation_id: String,
    pub principal: Principal,
    pub scope: ResourceScope,
    pub input: Value,
    pub idempotency_key: Option<String>,
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

    fn complete_idempotency(
        &mut self,
        _principal: &Principal,
        _command: &CommandMetadata,
        _result: &Result<Value, Problem>,
    ) -> Result<(), Problem> {
        Ok(())
    }
}

pub trait McpExecutor {
    fn execute(&mut self, request: ExecutionRequest<'_>) -> Result<Value, Problem>;
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
        if !self.enabled {
            return Err(problem("mcp_disabled", "MCP execution is disabled", 503));
        }

        let command = dispatch_metadata(&request.tool_name)
            .map_err(|error| dispatch_problem(error, &request.correlation_id))?
            .command;
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

        let result = self.executor.execute(ExecutionRequest {
            command,
            correlation_id: request.correlation_id.clone(),
            principal: principal.clone(),
            scope: scope.clone(),
            input: request.input,
            idempotency_key,
        });
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
                self.policy
                    .complete_idempotency(&principal, command, &Ok(value.clone()))?;
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
                self.policy
                    .complete_idempotency(&principal, command, &Err(error.clone()))?;
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
    }
}

fn reject_mcp_output_secrets(command: &CommandMetadata, value: &Value) -> Result<(), Problem> {
    if !command.id.as_str().starts_with("proxy.") {
        return reject_secrets(value);
    }
    let mut checked = value.clone();
    remove_proxy_secret_metadata(&mut checked);
    reject_secrets(&checked)
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

fn serialization_problem(error: serde_json::Error) -> Problem {
    Problem::new("serialization_failed", error.to_string(), 400, "mcp")
}

fn sha256_hash(value: &serde_json::Value) -> String {
    use sha2::{Digest, Sha256};
    let bytes = serde_json::to_vec(value).unwrap_or_default();
    format!("{:x}", Sha256::digest(&bytes))
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
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

fn resolve_scope(
    principal: &Principal,
    command: &CommandMetadata,
    input: &Value,
) -> Result<ResourceScope, Problem> {
    match command.id.as_str() {
        "system.health.get" => Ok(ResourceScope::global()),
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
        "organization.get" | "organization.update" => Ok(ResourceScope {
            account_id: principal.account_id.clone(),
            organization_id: Some(required_id(
                input,
                "organizationId",
                OrganizationId::from_str,
            )?),
            ..ResourceScope::default()
        }),
        "workspace.list" => Ok(ResourceScope {
            organization_id: Some(required_id(
                input,
                "organizationId",
                OrganizationId::from_str,
            )?),
            ..ResourceScope::default()
        }),
        "workspace.create" | "workspace.get" | "workspace.update" => Ok(ResourceScope {
            organization_id: Some(required_id(
                input,
                "organizationId",
                OrganizationId::from_str,
            )?),
            workspace_id: Some(required_id(input, "workspaceId", WorkspaceId::from_str)?),
            ..ResourceScope::default()
        }),
        "team.list" | "member.list" | "member.invite" | "resource.list" | "resource.register"
        | "grant.list" => Ok(ResourceScope::workspace(required_id(
            input,
            "workspaceId",
            WorkspaceId::from_str,
        )?)),
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
            let profile: Profile =
                serde_json::from_value(input.clone()).map_err(serialization_problem)?;
            Ok(ResourceScope {
                profile_id: Some(profile.id),
                ..ResourceScope::default()
            })
        }
        "profile.get" | "profile.launch" | "profile.stop" | "profile.delete"
        | "profile.archive" | "profile.restore" => Ok(ResourceScope {
            profile_id: Some(required_id(input, "profileId", ProfileId::from_str)?),
            ..ResourceScope::default()
        }),
        "operation.get" => Ok(ResourceScope::global()),
        "events.stream" => {
            serde_json::from_value(input.get("scope").cloned().unwrap_or(Value::Null))
                .map_err(serialization_problem)
        }
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
}

impl McpExecutionPolicy for ProductionMcpPolicy {
    fn authenticate(
        &mut self,
        bearer_token: Option<&str>,
        required: bool,
        _correlation_id: &str,
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
        self.state.authenticate_token(token, now())
    }

    fn resolve_scope(
        &mut self,
        principal: &Principal,
        command: &CommandMetadata,
        input: &Value,
    ) -> Result<ResourceScope, Problem> {
        if command.id.as_str() == "operation.get" {
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
        let mut scope = resolve_scope(principal, command, input)?;
        if !self.state.local_authority_enabled()
            && matches!(command.id.as_str(), "profile.archive" | "profile.restore")
        {
            let profile_id = scope
                .profile_id
                .as_ref()
                .ok_or_else(|| problem("profile_missing", "Profile is missing from scope", 400))?;
            let binding = self
                .state
                .profile_resource_record(profile_id)?
                .ok_or_else(|| problem("binding_not_found", "Profile binding not found", 404))?;
            scope.workspace_id = Some(binding.workspace_id);
            scope.resource_id = Some(binding.resource_id);
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
        let Some(claim) = self.claim.take() else {
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
            true,
            matches!(
                command.kind,
                crate::commands::CommandKind::Command | crate::commands::CommandKind::Operation
            ),
            serde_json::json!({ "adapter": "mcp" }),
            status,
            body,
        )?;
        Ok(())
    }

    fn audit(&mut self, record: AuditRecord) -> Result<(), Problem> {
        let outcome = match record.outcome {
            AuditOutcome::Succeeded => buglogin_domain::audit::AuditOutcome::Succeeded,
            AuditOutcome::Failed => buglogin_domain::audit::AuditOutcome::Failed,
            AuditOutcome::Denied => buglogin_domain::audit::AuditOutcome::Denied,
        };
        let event = buglogin_domain::audit::AuditEvent::new(
            format!(
                "evt_{}",
                &sha256_hash(&serde_json::json!({
                    "correlation": record.correlation_id,
                    "command": record.command_id,
                    "outcome": format!("{:?}", record.outcome),
                    "sequence": AUDIT_SEQUENCE.fetch_add(1, Ordering::Relaxed),
                }))[..24]
            ),
            now(),
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
        Ok(())
    }
}

pub struct ProductionMcpExecutor {
    state: crate::http::ApiState,
}

impl ProductionMcpExecutor {
    pub fn new(state: crate::http::ApiState) -> Self {
        Self { state }
    }
}

impl McpExecutor for ProductionMcpExecutor {
    fn execute(&mut self, request: ExecutionRequest<'_>) -> Result<Value, Problem> {
        let state = &self.state;
        let input = request.input;
        let principal = request.principal;
        let grants = state.grants()?;

        let is_mutation = request.command.kind == crate::commands::CommandKind::Command
            || request.command.kind == crate::commands::CommandKind::Operation;
        let _mutation_lock = if is_mutation {
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
            "operation.get" => {
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
            "events.stream" => {
                let after = input.get("after").and_then(Value::as_u64);
                let limit = input.get("limit").and_then(Value::as_u64).unwrap_or(100) as usize;
                let events = state
                    .events
                    .read()
                    .map_err(|_| problem("event_unavailable", "Event service unavailable", 503))?
                    .read_after(&principal, &grants, &request.scope, after, limit)?
                    .into_iter()
                    .cloned()
                    .collect::<Vec<_>>();
                Ok(serde_json::json!({ "items": events }))
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
                proxy.credential_ref = match has_credential {
                    None => current.proxy.credential_ref,
                    Some(false) => None,
                    Some(true) => Some(proxy_credential_id(&proxy.id)?),
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
                if state
                    .list_profile_records()?
                    .iter()
                    .any(|record| record.value.proxy_id.as_ref() == Some(&proxy_id))
                {
                    return Err(problem(
                        "proxy_assigned",
                        "Proxy is assigned to a profile",
                        409,
                    ));
                }
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
                Ok(serde_json::to_value(organization).map_err(serialization_problem)?)
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
                Ok(serde_json::to_value(record).map_err(serialization_problem)?)
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
                Ok(serde_json::to_value(record).map_err(serialization_problem)?)
            }
            "team.create" => {
                let ws_id: WorkspaceId = serde_json::from_value(
                    input.get("workspaceId").cloned().unwrap_or(Value::Null),
                )
                .map_err(serialization_problem)?;
                let team: Team =
                    serde_json::from_value(input.get("team").cloned().unwrap_or(Value::Null))
                        .map_err(serialization_problem)?;
                state
                    .teams
                    .write()
                    .map_err(|_| {
                        Problem::new("team_unavailable", "Team service unavailable", 503, "mcp")
                    })?
                    .create(&principal, &grants, &ws_id, team.clone())?;
                Ok(serde_json::to_value(team).map_err(serialization_problem)?)
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
                Ok(serde_json::to_value(record.team).map_err(serialization_problem)?)
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
                let items: Vec<Team> = list.into_iter().map(|t| t.team).collect();
                Ok(serde_json::json!({ "items": items, "nextCursor": Value::Null }))
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
                    .invite(&principal, &grants, member.clone(), now())?;
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
                let list = state
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
                    .list(&principal, &grants, &ws_id)?;
                Ok(serde_json::json!({ "items": list, "nextCursor": Value::Null }))
            }
            "member.activate" => {
                let id: MemberId =
                    serde_json::from_value(input.get("memberId").cloned().unwrap_or(Value::Null))
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
                    .activate(&principal, &grants, &id, now())?;
                Ok(serde_json::json!({ "memberId": id, "status": "active" }))
            }
            "member.suspend" => {
                let id: MemberId =
                    serde_json::from_value(input.get("memberId").cloned().unwrap_or(Value::Null))
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
                    .suspend(&principal, &grants, &id)?;
                Ok(serde_json::json!({ "memberId": id, "status": "suspended" }))
            }
            "member.role.update" => {
                let id: MemberId =
                    serde_json::from_value(input.get("memberId").cloned().unwrap_or(Value::Null))
                        .map_err(serialization_problem)?;
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
                    .set_role(&principal, &grants, &id, role.clone())?;
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
                let list = state
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
                    .list(&principal, &grants, &ws_id)?;
                Ok(serde_json::json!({ "items": list, "nextCursor": Value::Null }))
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
                Ok(serde_json::to_value(grant).map_err(serialization_problem)?)
            }
            "grant.revoke" => {
                let id: GrantId =
                    serde_json::from_value(input.get("grantId").cloned().unwrap_or(Value::Null))
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
                    .revoke(&principal, &grants, &id)?;
                Ok(serde_json::json!({ "grantId": id, "revoked": true }))
            }
            "grant.list" => {
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
                let list = state
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
                Ok(serde_json::json!({ "items": list, "nextCursor": Value::Null }))
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
                Ok(serde_json::to_value(profile.value).map_err(serialization_problem)?)
            }
            "profile.create" => (|| {
                let profile: Profile =
                    serde_json::from_value(input.clone()).map_err(serialization_problem)?;
                let workspace_id = input
                    .get("workspaceId")
                    .and_then(Value::as_str)
                    .map(WorkspaceId::from_str)
                    .transpose()
                    .map_err(|_| problem("invalid_scope", "workspaceId is invalid", 400))?;
                validate_profile_proxy(
                    state,
                    &profile,
                    workspace_id.as_ref(),
                    &principal,
                    &grants,
                )?;

                if state.local_authority_enabled() {
                    let created = state.create_profile_record(profile)?;
                    return serde_json::to_value(created.value).map_err(serialization_problem);
                }
                if profile.sync_mode != Some(buglogin_domain::entities::SyncMode::Managed) {
                    return Err(problem(
                        "production_managed_profile_required",
                        "Production profiles must use hosted managed authority",
                        422,
                    ));
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
                serde_json::to_value(created.value).map_err(serialization_problem)
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
                let list = state.list_profile_records()?;
                let items: Vec<Profile> = list
                    .into_iter()
                    .filter(|value| {
                        Capability::new("profile.read").is_ok_and(|capability| {
                            buglogin_domain::permissions::is_allowed(
                                &principal,
                                &capability,
                                Action::Read,
                                &ResourceScope {
                                    profile_id: Some(value.value.id.clone()),
                                    ..ResourceScope::default()
                                },
                                &grants,
                            )
                        })
                    })
                    .map(|value| value.value)
                    .collect();
                Ok(serde_json::json!({ "items": items, "nextCursor": Value::Null }))
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
                Ok(serde_json::to_value(updated.value).map_err(serialization_problem)?)
            }
            "profile.delete" => {
                if !state.local_authority_enabled() {
                    return Err(problem(
                        "production_profile_mcp_mutation_not_ready",
                        "Hosted profile MCP mutation is not ready",
                        503,
                    ));
                }
                if input.get("confirmation").and_then(Value::as_str)
                    != Some("QUARANTINE_PROFILE_DATA")
                {
                    return Err(problem(
                        "delete_not_confirmed",
                        "Profile deletion is not confirmed",
                        409,
                    ));
                }
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
                let current = state.get_profile_record(&id)?.ok_or_else(|| {
                    Problem::new("profile_not_found", "Profile not found", 404, "mcp")
                })?;
                let quarantine_path = state.profiles.delete(revision, &current.value)?;
                Ok(serde_json::json!({
                    "id": id,
                    "deleted": true,
                    "recoverable": true,
                    "quarantinePath": quarantine_path,
                }))
            }
            "profile.launch" => {
                if !state.local_authority_enabled() {
                    return Err(problem(
                        "production_runtime_dispatch_required",
                        "Production profile runtime must be dispatched through the desktop API client",
                        503,
                    ));
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
                let operation_id = format!(
                    "op_{}",
                    &sha256_hash(&serde_json::json!({
                        "correlation": request.correlation_id,
                        "command": request.command.id.as_str(),
                        "profileId": profile_id,
                    }))[..24]
                );
                let operation = crate::services::operations::Operation::queued(
                    operation_id.clone(),
                    request.command.id.as_str(),
                    request.scope.clone(),
                )?;
                state
                    .operations
                    .write()
                    .map_err(|_| {
                        problem(
                            "operation_unavailable",
                            "Operation service unavailable",
                            503,
                        )
                    })?
                    .insert(operation)?;
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
                let mut operations = state.operations.write().map_err(|_| {
                    problem(
                        "operation_unavailable",
                        "Operation service unavailable",
                        503,
                    )
                })?;
                match result {
                    Ok(value) => {
                        let completed = operations.transition(
                            &operation_id,
                            crate::services::operations::OperationStatus::Completed,
                            Some(1.0),
                            Some(value.clone()),
                            None,
                        )?;
                        Ok(
                            serde_json::json!({ "operationId": operation_id, "result": completed.result }),
                        )
                    }
                    Err(error) => {
                        operations.transition(
                            &operation_id,
                            crate::services::operations::OperationStatus::Failed,
                            Some(1.0),
                            None,
                            Some(error.clone()),
                        )?;
                        Err(error)
                    }
                }
            }
            "profile.stop" => {
                if !state.local_authority_enabled() {
                    return Err(problem(
                        "production_runtime_dispatch_required",
                        "Production profile runtime must be dispatched through the desktop API client",
                        503,
                    ));
                }
                let profile_id = request.scope.profile_id.clone().ok_or_else(|| {
                    Problem::new(
                        "profile_missing",
                        "Profile is missing from scope",
                        400,
                        "mcp",
                    )
                })?;
                let operation_id = format!(
                    "op_{}",
                    &sha256_hash(&serde_json::json!({
                        "correlation": request.correlation_id,
                        "command": request.command.id.as_str(),
                        "profileId": profile_id,
                    }))[..24]
                );
                let operation = crate::services::operations::Operation::queued(
                    operation_id.clone(),
                    request.command.id.as_str(),
                    request.scope.clone(),
                )?;
                state
                    .operations
                    .write()
                    .map_err(|_| {
                        problem(
                            "operation_unavailable",
                            "Operation service unavailable",
                            503,
                        )
                    })?
                    .insert(operation)?;
                let lifecycle_service = crate::http::profile_lifecycle::ProfileLifecycleService;
                let result = lifecycle_service.stop(&state.profiles, &state.lifecycles, profile_id);
                let mut operations = state.operations.write().map_err(|_| {
                    problem(
                        "operation_unavailable",
                        "Operation service unavailable",
                        503,
                    )
                })?;
                match result {
                    Ok(value) => {
                        let completed = operations.transition(
                            &operation_id,
                            crate::services::operations::OperationStatus::Completed,
                            Some(1.0),
                            Some(value),
                            None,
                        )?;
                        Ok(
                            serde_json::json!({ "operationId": operation_id, "result": completed.result }),
                        )
                    }
                    Err(error) => {
                        operations.transition(
                            &operation_id,
                            crate::services::operations::OperationStatus::Failed,
                            Some(1.0),
                            None,
                            Some(error.clone()),
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
                if let Some(before) = before_state {
                    state.replace_control_aggregate(before)?;
                }
            }
        }

        result
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
            Some(proxy_credential_id(&self.id)?)
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

fn proxy_credential_id(
    proxy_id: &buglogin_domain::ids::ProxyId,
) -> Result<buglogin_domain::ids::CredentialId, Problem> {
    buglogin_domain::ids::CredentialId::new(format!("credential-{}", proxy_id.as_str())).map_err(
        |_| {
            problem(
                "invalid_proxy_credential_id",
                "Proxy credential identifier is invalid",
                400,
            )
        },
    )
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
    match error {
        RepositoryError::Forbidden => problem("forbidden", "Proxy access denied", 403),
        RepositoryError::InvalidProxy => problem("invalid_proxy", "Proxy is invalid", 400),
        RepositoryError::ProxyIdConflict => problem(
            "proxy_conflict",
            "Proxy conflicts with existing inventory",
            409,
        ),
        RepositoryError::ProxyNotFound | RepositoryError::WorkspaceMismatch => {
            problem("proxy_not_found", "Proxy not found", 404)
        }
        RepositoryError::RevisionConflict { expected, actual } => problem(
            "storage_revision_conflict",
            "Storage revision conflict",
            409,
        )
        .with_field("expectedRevision", expected)
        .with_field("actualRevision", actual),
        RepositoryError::RevisionExhausted | RepositoryError::ProxyAssigned => {
            problem("proxy_conflict", "Proxy cannot be changed", 409)
        }
        RepositoryError::SnapshotVersionUnsupported { .. }
        | RepositoryError::SnapshotInvalidWorkspace
        | RepositoryError::SnapshotInvalidRevision
        | RepositoryError::SnapshotDuplicateProxyId
        | RepositoryError::SnapshotDuplicateEndpoint => {
            problem("proxy_state_invalid", "Proxy state is invalid", 500)
        }
    }
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct OrganizationInput {
    organization: Organization,
    policy: OrganizationPolicy,
}
