use std::{
    cell::{Cell, RefCell},
    fs,
    path::PathBuf,
    rc::Rc,
    sync::{
        Arc, Barrier,
        atomic::{AtomicU64, Ordering},
    },
};

use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use buglogin_api::mcp::{
    AuditOutcome, AuditRecord, ExecutionRequest, IdempotencyClaim, McpDispatcher,
    McpExecutionPolicy, McpExecutor, McpRequest,
};
use buglogin_domain::{
    account::Account,
    entities::{Profile, ProfileState, Proxy, ProxyProtocol, Runtime, SyncMode},
    errors::Problem,
    grant::ResourceGrant,
    ids::{
        AccountId, GrantId, MemberId, OrganizationId, ProfileId, ProxyId, ResourceId, WorkspaceId,
    },
    member::{Member, MemberRole, MemberStatus},
    organization::{Organization, OrganizationPolicy},
    permissions::{Action, Capability, Effect, Grant, Principal, ResourceScope},
    profile::{ProfileArchiveState, ProfileResource},
    resource::{Resource, ResourceKind, ResourceState},
    workspace::{Workspace, WorkspacePolicy},
};
use buglogin_storage::control::{ControlRecordStore, MemoryControlRecordStore, PutControlRecord};
use buglogin_sync::coordination::CoordinationBackend;
use serde_json::{Value, json};
use tower::ServiceExt;

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Default)]
struct FakePolicy {
    authenticated: bool,
    authorized: bool,
    replay: Option<Value>,
    calls: Rc<RefCell<Vec<&'static str>>>,
    audits: Rc<RefCell<Vec<AuditRecord>>>,
    completed: Rc<RefCell<Vec<Result<Value, Problem>>>>,
    abandoned: Rc<Cell<bool>>,
    audit_fails: bool,
}

impl McpExecutionPolicy for FakePolicy {
    fn authenticate(
        &mut self,
        _bearer_token: Option<&str>,
        required: bool,
        _correlation_id: &str,
    ) -> Result<Principal, Problem> {
        self.calls.borrow_mut().push("authenticate");
        if required && !self.authenticated {
            Err(problem("unauthorized", "Authentication required", 401))
        } else {
            Ok(Principal::default())
        }
    }

    fn resolve_scope(
        &mut self,
        _principal: &Principal,
        _command: &buglogin_api::commands::CommandMetadata,
        _input: &Value,
    ) -> Result<ResourceScope, Problem> {
        self.calls.borrow_mut().push("scope");
        Ok(ResourceScope::global())
    }

    fn authorize(
        &mut self,
        _principal: &Principal,
        _command: &buglogin_api::commands::CommandMetadata,
        _scope: &ResourceScope,
    ) -> Result<(), Problem> {
        self.calls.borrow_mut().push("authorize");
        if self.authorized {
            Ok(())
        } else {
            Err(problem("forbidden", "Permission denied", 403))
        }
    }

    fn claim_idempotency(
        &mut self,
        _principal: &Principal,
        _command: &buglogin_api::commands::CommandMetadata,
        _scope: &ResourceScope,
        _key: &str,
        _input: &Value,
    ) -> Result<IdempotencyClaim, Problem> {
        self.calls.borrow_mut().push("idempotency");
        Ok(self
            .replay
            .take()
            .map_or(IdempotencyClaim::Execute, IdempotencyClaim::Replay))
    }

    fn audit(&mut self, record: AuditRecord) -> Result<(), Problem> {
        self.calls.borrow_mut().push("audit");
        self.audits.borrow_mut().push(record);
        if self.audit_fails {
            Err(problem("audit_unavailable", "Audit unavailable", 503))
        } else {
            Ok(())
        }
    }

    fn complete_idempotency(
        &mut self,
        _principal: &Principal,
        _command: &buglogin_api::commands::CommandMetadata,
        result: &Result<Value, Problem>,
    ) -> Result<(), Problem> {
        self.completed.borrow_mut().push(result.clone());
        Ok(())
    }

    fn abandon_idempotency(&mut self) -> Result<(), Problem> {
        self.abandoned.set(true);
        Ok(())
    }
}

struct FakeExecutor {
    calls: usize,
    result: Result<Value, Problem>,
}

impl McpExecutor for FakeExecutor {
    fn execute(&mut self, _request: ExecutionRequest<'_>) -> Result<Value, Problem> {
        self.calls += 1;
        self.result.clone()
    }
}

fn request(tool_name: &str, idempotency_key: Option<&str>) -> McpRequest {
    McpRequest {
        tool_name: tool_name.to_owned(),
        correlation_id: "cor_test".to_owned(),
        bearer_token: Some("token".to_owned()),
        idempotency_key: idempotency_key.map(str::to_owned),
        input: json!({"name": "MCP test profile", "runtime": "wayfern"}),
    }
}

#[tokio::test]
async fn signed_migration_package_activation_is_exposed_to_mcp_with_package_id_input() {
    let audits = Rc::new(RefCell::new(Vec::new()));
    let calls = Rc::new(RefCell::new(Vec::new()));
    let policy = FakePolicy {
        authenticated: true,
        authorized: true,
        calls: calls.clone(),
        audits: audits.clone(),
        ..FakePolicy::default()
    };
    let executor = FakeExecutor {
        calls: 0,
        result: Ok(json!({
            "packageId": "pkg_rehearsal_01",
            "state": "active",
            "replayed": false,
            "controlImport": null,
            "syncImport": null
        })),
    };
    let mut dispatcher = McpDispatcher::new(policy, executor, true);
    let result = dispatcher
        .dispatch_async(McpRequest {
            tool_name: "migration_package_activate".into(),
            correlation_id: "cor_mcp_package_activation".into(),
            bearer_token: Some("test-token".into()),
            idempotency_key: Some("mcp-package-activate-01".into()),
            input: json!({"packageId": "pkg_rehearsal_01"}),
        })
        .await
        .unwrap();

    assert_eq!(result["packageId"], "pkg_rehearsal_01");
    assert!(calls.borrow().contains(&"authenticate"));
    assert!(calls.borrow().contains(&"authorize"));
    assert!(calls.borrow().contains(&"idempotency"));
    assert_eq!(audits.borrow().len(), 1);
    assert_eq!(audits.borrow()[0].outcome, AuditOutcome::Succeeded);
}

#[tokio::test]
async fn signed_migration_package_activation_replays_before_running_activation() {
    let replay = json!({
        "packageId": "pkg_rehearsal_02",
        "state": "active",
        "replayed": true,
        "controlImport": null,
        "syncImport": null
    });
    let policy = FakePolicy {
        authenticated: true,
        authorized: true,
        replay: Some(replay.clone()),
        ..FakePolicy::default()
    };
    let executor = FakeExecutor {
        calls: 0,
        result: Ok(Value::Null),
    };
    let mut dispatcher = McpDispatcher::new(policy, executor, true);
    let result = dispatcher
        .dispatch_async(McpRequest {
            tool_name: "migration_package_activate".into(),
            correlation_id: "cor_mcp_package_replay".into(),
            bearer_token: Some("test-token".into()),
            idempotency_key: Some("mcp-package-activate-02".into()),
            input: json!({"packageId": "pkg_rehearsal_02"}),
        })
        .await
        .unwrap();
    let (policy, executor) = dispatcher.into_parts();

    assert_eq!(result, replay);
    assert_eq!(executor.calls, 0);
    assert!(policy.calls.borrow().contains(&"idempotency"));
    assert_eq!(
        policy.audits.borrow()[0].metadata.get("idempotency"),
        Some(&"replay".to_owned())
    );
}

fn problem(code: &str, title: &str, status: u16) -> Problem {
    Problem::new(code, title, status, "test")
}

fn temp_root(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "buglogin-mcp-{label}-{}-{}",
        std::process::id(),
        TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ))
}

fn profile(id: &str, name: &str) -> Profile {
    Profile {
        id: ProfileId::from_str(id).unwrap(),
        name: name.into(),
        runtime: Runtime::Wayfern,
        runtime_version: Some("1.0.0".into()),
        runtime_config: None,
        state: ProfileState::Created,
        archived: false,
        fingerprint: None,
        note: None,
        proxy_id: None,
        vpn_id: None,
        group_ids: vec![],
        tags: vec!["mcp".into()],
        extension_group_id: None,
        sync_mode: Some(SyncMode::Local),
        created_at: "2026-07-21T00:00:00Z".into(),
        updated_at: "2026-07-21T00:00:00Z".into(),
    }
}

fn production_state(label: &str) -> (PathBuf, buglogin_api::http::ApiState) {
    let root = temp_root(label);
    let repository = buglogin_storage::local::LocalRepository::open(&root).unwrap();
    let profiles = buglogin_browser::profile::ProfileService::new(repository);
    (root, buglogin_api::http::ApiState::local(profiles))
}

fn hosted_state(account_id: &str) -> buglogin_api::http::ApiState {
    let account_id = AccountId::from_str(account_id).unwrap();
    let mut store = MemoryControlRecordStore::default();
    let account =
        serde_json::to_value(Account::new(account_id.clone(), "2026-07-22T00:00:00Z")).unwrap();
    store
        .put(PutControlRecord {
            tenant_id: "control",
            record_type: "account",
            record_id: account_id.as_str(),
            expected_revision: Some(0),
            idempotency_key: "seed-mcp-hosted-account",
            value: &account,
        })
        .unwrap();
    buglogin_api::http::ApiState::hosted_with_control_store(
        buglogin_browser::profile::ProfileService::disabled(),
        store,
    )
    .unwrap()
}

fn seed_workspace(
    state: &buglogin_api::http::ApiState,
    account_id: &str,
    workspace_id: &str,
    maximum_profiles: u32,
) {
    let organization = Organization {
        id: OrganizationId::from_str(&format!("org_{workspace_id}")).unwrap(),
        account_id: AccountId::from_str(account_id).unwrap(),
        name: "MCP hosted profile test".into(),
        created_at: "2026-07-22T00:00:00Z".into(),
    };
    let mut service = buglogin_api::services::workspaces::WorkspaceService::new(
        organization.clone(),
        OrganizationPolicy {
            max_workspaces: 1,
            offline_entitled: false,
        },
    )
    .unwrap();
    let workspace_id = WorkspaceId::from_str(workspace_id).unwrap();
    service
        .create_workspace(
            &Principal {
                roles: std::collections::BTreeSet::from([
                    buglogin_domain::permissions::Role::PlatformAdmin,
                ]),
                ..Principal::default()
            },
            &[],
            &organization.id,
            Workspace {
                id: workspace_id,
                organization_id: organization.id.clone(),
                name: "MCP hosted profile test".into(),
                policy: WorkspacePolicy {
                    max_profiles: maximum_profiles,
                    max_members: 10,
                    max_storage_bytes: 1024 * 1024,
                    offline_entitled: false,
                    default_extension_group_id: None,
                },
                created_at: "2026-07-22T00:00:00Z".into(),
            },
        )
        .unwrap();
    *state.workspaces.write().unwrap() = service;
}

fn production_dispatcher(
    state: &buglogin_api::http::ApiState,
) -> McpDispatcher<buglogin_api::mcp::ProductionMcpPolicy, buglogin_api::mcp::ProductionMcpExecutor>
{
    McpDispatcher::new(
        buglogin_api::mcp::ProductionMcpPolicy::new(state.clone()),
        buglogin_api::mcp::ProductionMcpExecutor::new(state.clone()),
        true,
    )
}

fn production_request(
    tool_name: &str,
    correlation_id: &str,
    bearer_token: Option<&str>,
    idempotency_key: Option<&str>,
    input: Value,
) -> McpRequest {
    McpRequest {
        tool_name: tool_name.into(),
        correlation_id: correlation_id.into(),
        bearer_token: bearer_token.map(str::to_owned),
        idempotency_key: idempotency_key.map(str::to_owned),
        input,
    }
}

fn mcp_token(state: &buglogin_api::http::ApiState, account_id: &str) -> String {
    state.issue_token(account_id).unwrap()
}

fn grant_profile(
    state: &buglogin_api::http::ApiState,
    account_id: &str,
    profile_id: &str,
    capability: &str,
    action: Action,
) {
    state.grants.write().unwrap().push(Grant {
        id: GrantId::from_str(&format!(
            "grant_{}_{}_{}",
            capability.replace('.', "_"),
            profile_id,
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ))
        .unwrap(),
        account_id: Some(AccountId::from_str(account_id).unwrap()),
        member_id: None,
        capability: Capability::new(capability).unwrap(),
        action,
        scope: ResourceScope {
            profile_id: Some(ProfileId::from_str(profile_id).unwrap()),
            ..ResourceScope::default()
        },
        effect: Effect::Allow,
    });
}

fn grant_proxy(
    state: &buglogin_api::http::ApiState,
    account_id: &str,
    workspace_id: &str,
    action: Action,
) {
    state.grants.write().unwrap().push(Grant {
        id: GrantId::from_str(&format!(
            "grant_proxy_{}_{}",
            workspace_id,
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ))
        .unwrap(),
        account_id: Some(AccountId::from_str(account_id).unwrap()),
        member_id: None,
        capability: Capability::new("CAP-NET-001").unwrap(),
        action,
        scope: ResourceScope::workspace(WorkspaceId::from_str(workspace_id).unwrap()),
        effect: Effect::Allow,
    });
}

fn grant_workspace_capability(
    state: &buglogin_api::http::ApiState,
    account_id: &str,
    workspace_id: &str,
    capability: &str,
    action: Action,
) {
    state.grants.write().unwrap().push(Grant {
        id: GrantId::from_str(&format!(
            "grant_{}_{}_{}",
            capability.replace('.', "_"),
            workspace_id,
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ))
        .unwrap(),
        account_id: Some(AccountId::from_str(account_id).unwrap()),
        member_id: None,
        capability: Capability::new(capability).unwrap(),
        action,
        scope: ResourceScope::workspace(WorkspaceId::from_str(workspace_id).unwrap()),
        effect: Effect::Allow,
    });
}

fn seed_proxy(
    state: &buglogin_api::http::ApiState,
    account_id: &str,
    workspace_id: &str,
    proxy_id: &str,
) {
    grant_proxy(state, account_id, workspace_id, Action::Create);
    let principal = Principal {
        account_id: Some(AccountId::from_str(account_id).unwrap()),
        ..Principal::default()
    };
    let grants = state.grants().unwrap();
    state
        .proxies
        .write()
        .unwrap()
        .create(
            WorkspaceId::from_str(workspace_id).unwrap(),
            Proxy {
                id: ProxyId::from_str(proxy_id).unwrap(),
                name: Some("Profile proxy".into()),
                protocol: ProxyProtocol::Http,
                host: "profile-proxy.example.com".into(),
                port: 8080,
                credential_ref: None,
                rotation_url: None,
                bypass_rules: vec![],
                created_at: "2026-07-22T00:00:00Z".into(),
                updated_at: "2026-07-22T00:00:00Z".into(),
            },
            &principal,
            &grants,
        )
        .unwrap();
}

fn profile_create_input(mut profile: Profile, workspace_id: &str, proxy_id: &str) -> Value {
    profile.proxy_id = Some(ProxyId::from_str(proxy_id).unwrap());
    let mut input = serde_json::to_value(profile).unwrap();
    input["workspaceId"] = json!(workspace_id);
    input
}

fn proxy_input(workspace_id: &str, proxy_id: &str, name: &str) -> Value {
    json!({
        "workspaceId": workspace_id,
        "proxy": {
            "id": proxy_id,
            "name": name,
            "protocol": "http",
            "host": "Proxy.Example.COM",
            "port": 8080,
            "bypassRules": ["localhost"],
            "createdAt": "2026-07-22T00:00:00Z",
            "updatedAt": "2026-07-22T00:00:00Z"
        }
    })
}

#[test]
fn execution_runs_policy_before_fake_executor_and_audits_success() {
    let policy = FakePolicy {
        authenticated: true,
        authorized: true,
        ..FakePolicy::default()
    };
    let calls = policy.calls.clone();
    let audits = policy.audits.clone();
    let executor = FakeExecutor {
        calls: 0,
        result: Ok(json!({"status": "ok"})),
    };
    let mut dispatcher = McpDispatcher::new(policy, executor, true);

    assert_eq!(
        dispatcher.dispatch(request("profile_create", Some("key-123456789012"))),
        Ok(json!({"status": "ok"}))
    );
    assert_eq!(
        calls.take(),
        vec!["authenticate", "scope", "authorize", "idempotency", "audit"]
    );
    assert_eq!(audits.borrow()[0].outcome, AuditOutcome::Succeeded);
    assert_eq!(dispatcher.into_parts().1.calls, 1);
}

#[test]
fn audit_failure_after_execution_does_not_leave_idempotency_unsettled() {
    let policy = FakePolicy {
        authenticated: true,
        authorized: true,
        audit_fails: true,
        ..FakePolicy::default()
    };
    let completed = policy.completed.clone();
    let mut dispatcher = McpDispatcher::new(
        policy,
        FakeExecutor {
            calls: 0,
            result: Ok(json!({"created": true})),
        },
        true,
    );

    let error = dispatcher
        .dispatch(request("profile_create", Some("key-123456789012")))
        .unwrap_err();

    assert_eq!(error.code, "audit_unavailable");
    assert_eq!(
        completed.borrow().as_slice(),
        &[Ok(json!({"created": true}))]
    );
    assert_eq!(dispatcher.into_parts().1.calls, 1);
}

#[test]
fn execution_rejects_invalid_contract_before_auth_or_execution() {
    let invalid_inputs = [
        json!({"name": "Missing runtime"}),
        json!({"name": 42, "runtime": "wayfern"}),
        json!({"name": "Unknown field", "runtime": "wayfern", "password": "credential-marker-secret"}),
        json!({"name": "x".repeat(201), "runtime": "wayfern"}),
    ];
    for input in invalid_inputs {
        let policy = FakePolicy::default();
        let calls = policy.calls.clone();
        let mut dispatcher = McpDispatcher::new(
            policy,
            FakeExecutor {
                calls: 0,
                result: Ok(json!(null)),
            },
            true,
        );
        let mut request = request("profile_create", Some("key-123456789012"));
        request.input = input;

        let error = dispatcher.dispatch(request).unwrap_err();
        assert_eq!(error.code, "mcp_invalid_input");
        assert!(
            !error
                .detail
                .as_deref()
                .unwrap_or_default()
                .contains("credential-marker-secret")
        );
        assert!(calls.borrow().is_empty());
        assert_eq!(dispatcher.into_parts().1.calls, 0);
    }
}

#[test]
fn execution_denies_auth_and_scope_without_calling_executor() {
    let policy = FakePolicy::default();
    let calls = policy.calls.clone();
    let mut dispatcher = McpDispatcher::new(
        policy,
        FakeExecutor {
            calls: 0,
            result: Ok(json!(null)),
        },
        true,
    );

    let error = dispatcher
        .dispatch(request("profile_create", Some("key-123456789012")))
        .unwrap_err();
    assert_eq!(error.code, "unauthorized");
    assert_eq!(calls.take(), vec!["authenticate", "audit"]);
    assert_eq!(dispatcher.into_parts().1.calls, 0);

    let policy = FakePolicy {
        authenticated: true,
        authorized: false,
        ..FakePolicy::default()
    };
    let calls = policy.calls.clone();
    let mut dispatcher = McpDispatcher::new(
        policy,
        FakeExecutor {
            calls: 0,
            result: Ok(json!(null)),
        },
        true,
    );
    let error = dispatcher
        .dispatch(request("profile_create", Some("key-123456789012")))
        .unwrap_err();
    assert_eq!(error.code, "forbidden");
    assert_eq!(
        calls.take(),
        vec!["authenticate", "scope", "authorize", "audit"]
    );
    assert_eq!(dispatcher.into_parts().1.calls, 0);
}

#[test]
fn execution_requires_idempotency_replays_without_execution_and_rejects_secret_results() {
    let policy = FakePolicy {
        authenticated: true,
        authorized: true,
        ..FakePolicy::default()
    };
    let mut dispatcher = McpDispatcher::new(
        policy,
        FakeExecutor {
            calls: 0,
            result: Ok(json!({"ok": true})),
        },
        true,
    );
    let error = dispatcher
        .dispatch(request("profile_create", None))
        .unwrap_err();
    assert_eq!(error.code, "idempotency_key_required");
    assert_eq!(dispatcher.into_parts().1.calls, 0);

    let policy = FakePolicy {
        authenticated: true,
        authorized: true,
        replay: Some(json!({"status": "already-complete"})),
        ..FakePolicy::default()
    };
    let mut dispatcher = McpDispatcher::new(
        policy,
        FakeExecutor {
            calls: 0,
            result: Ok(json!(null)),
        },
        true,
    );
    assert_eq!(
        dispatcher.dispatch(request("profile_create", Some("key-123456789012"))),
        Ok(json!({"status": "already-complete"}))
    );
    let (policy, executor) = dispatcher.into_parts();
    assert_eq!(executor.calls, 0);
    assert_eq!(policy.audits.borrow()[0].outcome, AuditOutcome::Succeeded);
    assert_eq!(policy.audits.borrow()[0].metadata["idempotency"], "replay");

    let policy = FakePolicy {
        authenticated: true,
        authorized: true,
        ..FakePolicy::default()
    };
    let mut dispatcher = McpDispatcher::new(
        policy,
        FakeExecutor {
            calls: 0,
            result: Ok(json!({"token": "secret"})),
        },
        true,
    );
    let error = dispatcher
        .dispatch(request("profile_create", Some("key-123456789012")))
        .unwrap_err();
    assert_eq!(error.code, "secret_material_rejected");
}

#[test]
fn execution_is_opt_in_and_non_mcp_tools_are_denied() {
    let mut dispatcher = McpDispatcher::new(
        FakePolicy::default(),
        FakeExecutor {
            calls: 0,
            result: Ok(json!(null)),
        },
        false,
    );
    assert_eq!(
        dispatcher
            .dispatch(request("profile_create", Some("key-123456789012")))
            .unwrap_err()
            .code,
        "mcp_disabled"
    );

    let mut dispatcher = McpDispatcher::new(
        FakePolicy {
            authenticated: true,
            authorized: true,
            ..FakePolicy::default()
        },
        FakeExecutor {
            calls: 0,
            result: Ok(json!(null)),
        },
        true,
    );
    let error = dispatcher
        .dispatch(request("migration_import", Some("key-123456789012")))
        .unwrap_err();
    assert_eq!(error.code, "mcp_tool_unavailable");
}

#[test]
fn production_mcp_policy_and_executor_integration() {
    let temp_dir = std::env::temp_dir().join(format!("mcp-test-{}", std::process::id()));
    let repository = buglogin_storage::local::LocalRepository::open(&temp_dir).unwrap();
    let profiles = buglogin_browser::profile::ProfileService::new(repository);
    let state = buglogin_api::http::ApiState::local(profiles);

    let policy = buglogin_api::mcp::ProductionMcpPolicy::new(state.clone());
    let executor = buglogin_api::mcp::ProductionMcpExecutor::new(state.clone());
    let mut dispatcher = buglogin_api::mcp::McpDispatcher::new(policy, executor, true);

    let req = buglogin_api::mcp::McpRequest {
        tool_name: "account_get".to_owned(),
        correlation_id: "cor_mcp_1".to_owned(),
        bearer_token: None,
        idempotency_key: None,
        input: serde_json::json!({}),
    };
    let err = dispatcher.dispatch(req).unwrap_err();
    assert_eq!(err.code, "authentication_required");

    let _ = std::fs::remove_dir_all(temp_dir);
}

#[test]
fn production_health_is_available_anonymously() {
    let (root, state) = production_state("health-anonymous");
    let result = production_dispatcher(&state)
        .dispatch(production_request(
            "system_health_get",
            "cor_health_anonymous",
            None,
            None,
            json!({}),
        ))
        .unwrap();

    assert_eq!(result, json!({"status": "ok", "service": "buglogin-api"}));
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn production_mcp_event_read_uses_hosted_coordination_and_fails_closed_without_it() {
    let (root, state) = production_state("events-require-coordination");
    let token = mcp_token(&state, "acct_mcp_events");
    grant_workspace_capability(
        &state,
        "acct_mcp_events",
        "ws_mcp_events",
        "events.read",
        Action::Read,
    );
    let error = production_dispatcher(&state)
        .dispatch_async(production_request(
            "events_stream",
            "cor_events_stream",
            Some(&token),
            None,
            json!({
                "workspaceId": "ws_mcp_events",
                "after": null,
                "limit": 25
            }),
        ))
        .await
        .unwrap_err();

    assert_eq!(error.code, "coordination_unavailable");
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn production_mcp_lease_status_is_scoped_and_never_returns_owner_token() {
    let (root, state) = production_state("lease-status-redaction");
    let account_id = "acct_mcp_lease_status";
    let workspace_id = WorkspaceId::from_str("ws_mcp_lease_status").unwrap();
    let resource_id = ResourceId::from_str("res_mcp_lease_status").unwrap();
    let token = mcp_token(&state, account_id);
    grant_workspace_capability(
        &state,
        account_id,
        workspace_id.as_str(),
        "profile.read",
        Action::Read,
    );
    let coordination = buglogin_sync::coordination::LocalCoordination::new();
    let auth =
        buglogin_sync::coordination::CoordinationAuth::new(account_id, token.clone()).unwrap();
    coordination
        .acquire_lease_idempotent(
            buglogin_sync::coordination::CoordinationScope {
                workspace_id: &workspace_id,
                auth: &auth,
            },
            &resource_id,
            std::time::Duration::from_secs(30),
            "mcp-lease-status-0001",
        )
        .await
        .unwrap();
    let state = state.with_coordination(Arc::new(coordination));
    let status = production_dispatcher(&state)
        .dispatch_async(production_request(
            "resource_lease_get",
            "cor_mcp_lease_status",
            Some(&token),
            None,
            json!({
                "workspaceId": workspace_id,
                "resourceId": resource_id
            }),
        ))
        .await
        .unwrap();

    assert_eq!(status["ownerId"], account_id);
    assert!(status.get("ownerToken").is_none());

    let absent = production_dispatcher(&state)
        .dispatch_async(production_request(
            "resource_lease_get",
            "cor_mcp_lease_status_absent",
            Some(&token),
            None,
            json!({
                "workspaceId": workspace_id,
                "resourceId": "res_mcp_lease_absent"
            }),
        ))
        .await
        .unwrap();
    assert!(absent.is_null());

    let forbidden = production_dispatcher(&state)
        .dispatch_async(production_request(
            "resource_lease_get",
            "cor_mcp_lease_status_forbidden",
            Some(&token),
            None,
            json!({
                "workspaceId": "ws_mcp_lease_other",
                "resourceId": resource_id
            }),
        ))
        .await
        .unwrap_err();
    assert_eq!(forbidden.status, 403);
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn production_mcp_reads_revision_aware_org_and_workspace_records() {
    let account_id = "acct_mcp_admin_records";
    let workspace_id = "ws_mcp_admin_records";
    let organization_id = format!("org_{workspace_id}");
    let state = hosted_state(account_id);
    let token = mcp_token(&state, account_id);
    seed_workspace(&state, account_id, workspace_id, 25);
    grant_workspace_capability(&state, account_id, workspace_id, "CAP-WS-001", Action::Read);
    state.grants.write().unwrap().push(Grant {
        id: GrantId::from_str("grant_mcp_admin_record_org_read").unwrap(),
        account_id: Some(AccountId::from_str(account_id).unwrap()),
        member_id: None,
        capability: Capability::new("CAP-ORG-001").unwrap(),
        action: Action::Read,
        scope: ResourceScope {
            account_id: Some(AccountId::from_str(account_id).unwrap()),
            organization_id: Some(OrganizationId::from_str(&organization_id).unwrap()),
            ..ResourceScope::default()
        },
        effect: Effect::Allow,
    });

    let workspace_record = production_dispatcher(&state)
        .dispatch_async(production_request(
            "workspace_record_get",
            "cor_mcp_workspace_record",
            Some(&token),
            None,
            json!({ "organizationId": organization_id.clone(), "workspaceId": workspace_id }),
        ))
        .await
        .unwrap();
    assert_eq!(workspace_record["workspace"]["id"], workspace_id);
    assert_eq!(workspace_record["revision"], 1);

    let organization_record = production_dispatcher(&state)
        .dispatch_async(production_request(
            "organization_record_get",
            "cor_mcp_organization_record",
            Some(&token),
            None,
            json!({ "organizationId": organization_id.clone() }),
        ))
        .await
        .unwrap();
    assert_eq!(organization_record["organization"]["id"], organization_id);
    assert_eq!(organization_record["revision"], 1);
}

#[test]
fn production_profile_create_replays_exact_result() {
    let (root, state) = production_state("profile-create-replay");
    let token = mcp_token(&state, "acct_mcp_replay");
    grant_profile(
        &state,
        "acct_mcp_replay",
        "prf_mcp_replay",
        "profile.create",
        Action::Create,
    );
    let input = json!({
        "id": "prf_mcp_replay",
        "name": "Replay profile",
        "runtime": "wayfern",
        "runtimeVersion": "152.0"
    });
    let mut first_dispatcher = production_dispatcher(&state);
    let first = first_dispatcher
        .dispatch(production_request(
            "profile_create",
            "cor_profile_create_first",
            Some(&token),
            Some("profile-create-replay-0001"),
            input.clone(),
        ))
        .unwrap();
    let replay = production_dispatcher(&state)
        .dispatch(production_request(
            "profile_create",
            "cor_profile_create_replay",
            Some(&token),
            Some("profile-create-replay-0001"),
            input,
        ))
        .unwrap();

    assert_eq!(replay, first);
    assert_eq!(state.profiles.list().unwrap().len(), 1);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn production_profile_create_rejects_same_key_with_different_input() {
    let (root, state) = production_state("profile-create-conflict");
    let token = mcp_token(&state, "acct_mcp_conflict");
    grant_profile(
        &state,
        "acct_mcp_conflict",
        "prf_mcp_conflict",
        "profile.create",
        Action::Create,
    );
    let mut dispatcher = production_dispatcher(&state);
    dispatcher
        .dispatch(production_request(
            "profile_create",
            "cor_profile_conflict_first",
            Some(&token),
            Some("profile-create-conflict-0001"),
            serde_json::to_value(profile("prf_mcp_conflict", "Original name")).unwrap(),
        ))
        .unwrap();
    let error = production_dispatcher(&state)
        .dispatch(production_request(
            "profile_create",
            "cor_profile_conflict_second",
            Some(&token),
            Some("profile-create-conflict-0001"),
            serde_json::to_value(profile("prf_mcp_conflict", "Changed name")).unwrap(),
        ))
        .unwrap_err();

    assert_eq!(error.status, 409);
    assert_eq!(error.code, "idempotency_conflict");
    assert_eq!(state.profiles.list().unwrap().len(), 1);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn production_mcp_grant_list_page_continues_and_legacy_list_keeps_all_items() {
    let (root, state) = production_state("grant-list-page");
    let account_id = "acct_mcp_grant_page";
    let workspace_id = "ws_mcp_grant_page";
    let resource_id = "res_mcp_grant_page";
    let token = mcp_token(&state, account_id);
    for (capability, action) in [
        ("CAP-RES-001", Action::Create),
        ("CAP-SHARE-001", Action::Manage),
        ("CAP-SHARE-001", Action::Read),
    ] {
        grant_workspace_capability(&state, account_id, workspace_id, capability, action);
    }

    let principal = Principal {
        account_id: Some(AccountId::from_str(account_id).unwrap()),
        ..Principal::default()
    };
    let grants = state.grants().unwrap();
    let workspace = WorkspaceId::from_str(workspace_id).unwrap();
    let resource = ResourceId::from_str(resource_id).unwrap();
    state
        .resources
        .write()
        .unwrap()
        .register(
            &principal,
            &grants,
            Resource {
                id: resource.clone(),
                workspace_id: workspace.clone(),
                kind: ResourceKind::Profile,
                state: ResourceState::Active,
                display_name: "Grant page fixture".into(),
            },
        )
        .unwrap();
    for (grant_id, member_id) in [
        ("grant_page_a", "mem_mcp_page_a"),
        ("grant_page_b", "mem_mcp_page_b"),
        ("grant_page_c", "mem_mcp_page_c"),
    ] {
        state
            .resources
            .write()
            .unwrap()
            .grant(
                &principal,
                &grants,
                ResourceGrant {
                    id: GrantId::from_str(grant_id).unwrap(),
                    workspace_id: workspace.clone(),
                    resource_id: resource.clone(),
                    member_id: MemberId::from_str(member_id).unwrap(),
                    action: Action::Read,
                    effect: Effect::Allow,
                },
            )
            .unwrap();
    }

    let first_page = production_dispatcher(&state)
        .dispatch(production_request(
            "grant.list.page",
            "cor_grant_page_first",
            Some(&token),
            None,
            json!({"workspaceId": workspace_id, "limit": 2}),
        ))
        .unwrap();
    assert_eq!(first_page["items"].as_array().unwrap().len(), 2);
    assert_eq!(first_page["items"][0]["id"], "grant_page_a");
    assert_eq!(first_page["items"][1]["id"], "grant_page_b");
    assert_eq!(first_page["nextCursor"], "grant_page_b");

    let second_page = production_dispatcher(&state)
        .dispatch(production_request(
            "grant.list.page",
            "cor_grant_page_second",
            Some(&token),
            None,
            json!({"workspaceId": workspace_id, "limit": 2, "cursor": "grant_page_b"}),
        ))
        .unwrap();
    assert_eq!(second_page["items"].as_array().unwrap().len(), 1);
    assert_eq!(second_page["items"][0]["id"], "grant_page_c");
    assert!(second_page["nextCursor"].is_null());

    let compatibility = production_dispatcher(&state)
        .dispatch(production_request(
            "grant.list",
            "cor_grant_list_compatibility",
            Some(&token),
            None,
            json!({"workspaceId": workspace_id}),
        ))
        .unwrap();
    assert_eq!(compatibility["items"].as_array().unwrap().len(), 3);
    assert!(compatibility["nextCursor"].is_null());
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn platform_admin_mcp_api_key_can_manage_accounts_and_basic_role_is_denied() {
    use buglogin_api::http::{ApiState, FirstOwnerBootstrapInput, router};
    use buglogin_domain::permissions::Role;
    use buglogin_storage::control::MemoryControlRecordStore;

    let state = ApiState::hosted_with_control_store(
        buglogin_browser::profile::ProfileService::disabled(),
        MemoryControlRecordStore::default(),
    )
    .unwrap();
    let owner = state
        .bootstrap_first_owner(FirstOwnerBootstrapInput {
            account_id: Some("acct_mcp_platform_owner".into()),
            username: "mcp-platform-owner".into(),
            email: "mcp-platform-owner@example.test".into(),
            operator_attested_email: true,
            password: zeroize::Zeroizing::new("Mcp platform owner 2026!".into()),
            organization_name: "MCP Admin Org".into(),
            workspace_name: "MCP Admin Workspace".into(),
            max_workspaces: 4,
            max_profiles: 50,
            max_members: 10,
            max_storage_bytes: 1_048_576,
        })
        .unwrap();
    let app = router(state.clone());
    let login = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/auth/login")
                .header("content-type", "application/json")
                .header("origin", "http://127.0.0.1:3000")
                .header("x-correlation-id", "mcp-admin-login")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "identifier": "mcp-platform-owner",
                        "password": "Mcp platform owner 2026!"
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(login.status(), StatusCode::OK);
    let set_cookies = login
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|value| value.to_str().unwrap())
        .collect::<Vec<_>>();
    let session_cookie = set_cookies
        .iter()
        .find(|value| value.starts_with("__Host-buglogin_session="))
        .unwrap()
        .split(';')
        .next()
        .unwrap();
    let csrf_cookie = set_cookies
        .iter()
        .find(|value| value.starts_with("__Host-buglogin_csrf="))
        .unwrap()
        .split(';')
        .next()
        .unwrap();
    let cookies = format!("{session_cookie}; {csrf_cookie}");
    let csrf = csrf_cookie.split_once('=').unwrap().1;
    let expires_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 3600;
    let created_key = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/auth/api-tokens")
                .header("content-type", "application/json")
                .header("origin", "http://127.0.0.1:3000")
                .header("cookie", &cookies)
                .header("x-csrf-token", csrf)
                .header("x-correlation-id", "mcp-admin-api-key-create")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "name": "MCP platform admin",
                        "scopes": [
                            "CAP-ADM-001:*",
                            "CAP-ORG-001:*",
                            "CAP-WS-001:*",
                            "CAP-MEM-001:*",
                            "CAP-PROF-003:*",
                            "profile.read:*",
                            "profile.create:*",
                            "profile.update:*",
                            "profile.delete:*"
                        ],
                        "expiresAt": expires_at
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created_key.status(), StatusCode::CREATED);
    let key_body: Value = serde_json::from_slice(
        &axum::body::to_bytes(created_key.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    let token = key_body["token"].as_str().unwrap().to_owned();
    assert!(token.starts_with("blapi_"));
    let mut dispatcher = production_dispatcher(&state);
    let account_input = json!({
        "accountId": "acct_mcp_pending_child",
        "username": "mcp-pending-child",
        "email": "mcp-pending-child@example.test"
    });
    let pending_account = dispatcher
        .dispatch(production_request(
            "account_create",
            "cor_mcp_platform_account_create",
            Some(&token),
            Some("mcp-platform-account-create-01"),
            account_input.clone(),
        ))
        .unwrap();
    assert_eq!(pending_account["account"]["id"], "acct_mcp_pending_child");
    assert_eq!(pending_account["identity"]["verification"], "pending");
    assert_eq!(pending_account["onboardingStatus"], "pending_verification");
    assert_eq!(pending_account["loginEnabled"], false);
    assert!(pending_account.get("password").is_none());

    let replayed_account = dispatcher
        .dispatch(production_request(
            "account_create",
            "cor_mcp_platform_account_create_replay",
            Some(&token),
            Some("mcp-platform-account-create-01"),
            account_input.clone(),
        ))
        .unwrap();
    assert_eq!(replayed_account, pending_account);

    let conflicting_replay = dispatcher
        .dispatch(production_request(
            "account_create",
            "cor_mcp_platform_account_create_conflict",
            Some(&token),
            Some("mcp-platform-account-create-01"),
            json!({
                "accountId": "acct_mcp_other_child",
                "username": "mcp-other-child",
                "email": "mcp-other-child@example.test"
            }),
        ))
        .unwrap_err();
    assert_eq!(conflicting_replay.status, 409);

    let pending_login = router(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/auth/login")
                .header("content-type", "application/json")
                .header("origin", "http://127.0.0.1:3000")
                .header("x-correlation-id", "mcp-pending-account-login")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "identifier": "mcp-pending-child",
                        "password": "Mcp platform owner 2026!"
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(pending_login.status(), StatusCode::UNAUTHORIZED);
    let pending_login_body: Value = serde_json::from_slice(
        &axum::body::to_bytes(pending_login.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(pending_login_body["code"], "invalid_credentials");

    let restricted_key_response = router(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/auth/api-tokens")
                .header("content-type", "application/json")
                .header("origin", "http://127.0.0.1:3000")
                .header("cookie", &cookies)
                .header("x-csrf-token", csrf)
                .header("x-correlation-id", "mcp-admin-restricted-key-create")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "name": "MCP account manager without create",
                        "scopes": ["CAP-ADM-001:manage"],
                        "expiresAt": expires_at
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(restricted_key_response.status(), StatusCode::CREATED);
    let restricted_key_body: Value = serde_json::from_slice(
        &axum::body::to_bytes(restricted_key_response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    let scoped_admin_token = restricted_key_body["token"].as_str().unwrap();
    let scoped_admin_created_account = production_dispatcher(&state)
        .dispatch(production_request(
            "account_create",
            "cor_mcp_platform_account_create_scoped_admin",
            Some(scoped_admin_token),
            Some("mcp-platform-account-create-scoped-admin-01"),
            json!({
                "accountId": "acct_mcp_scope_denied",
                "username": "mcp-scope-denied",
                "email": "mcp-scope-denied@example.test"
            }),
        ))
        .unwrap();
    assert_eq!(
        scoped_admin_created_account["account"]["id"],
        "acct_mcp_scope_denied"
    );

    let accounts = dispatcher
        .dispatch(production_request(
            "account_list",
            "cor_mcp_platform_account_list",
            Some(&token),
            None,
            json!({}),
        ))
        .unwrap();

    assert_eq!(accounts["items"].as_array().unwrap().len(), 3);
    assert!(
        accounts["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|account| account["id"] == owner.account_id.as_str())
    );
    assert!(
        accounts["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|account| account["id"] == "acct_mcp_pending_child")
    );
    let owner_revision = accounts["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|account| account["id"] == owner.account_id.as_str())
        .unwrap()["revision"]
        .as_u64()
        .unwrap();

    // Platform-admin authority comes from the account role. API-token scopes
    // restrict regular principals, while explicit DENY grants still apply.
    let mut policy = buglogin_api::mcp::ProductionMcpPolicy::new(state.clone());
    let platform_principal = policy
        .authenticate(Some(&token), true, "cor_mcp_platform_authz")
        .unwrap();
    assert!(platform_principal.roles.contains(&Role::PlatformAdmin));
    for (command_id, scope) in [
        ("account.create", ResourceScope::global()),
        ("account.list", ResourceScope::global()),
        (
            "organization.create",
            ResourceScope {
                account_id: Some(owner.account_id.clone()),
                organization_id: Some(OrganizationId::from_str("org_mcp_admin_authz").unwrap()),
                ..ResourceScope::default()
            },
        ),
        (
            "workspace.create",
            ResourceScope {
                organization_id: Some(OrganizationId::from_str("org_mcp_admin_authz").unwrap()),
                workspace_id: Some(WorkspaceId::from_str("ws_mcp_admin_authz").unwrap()),
                ..ResourceScope::default()
            },
        ),
        (
            "member.role.update",
            ResourceScope {
                workspace_id: Some(WorkspaceId::from_str("ws_mcp_admin_authz").unwrap()),
                member_id: Some(MemberId::from_str("mem_mcp_admin_authz").unwrap()),
                ..ResourceScope::default()
            },
        ),
        (
            "group.create",
            ResourceScope::workspace(WorkspaceId::from_str("ws_mcp_admin_authz").unwrap()),
        ),
        (
            "profile.create",
            ResourceScope {
                workspace_id: Some(WorkspaceId::from_str("ws_mcp_admin_authz").unwrap()),
                profile_id: Some(ProfileId::from_str("prf_mcp_admin_authz").unwrap()),
                ..ResourceScope::default()
            },
        ),
    ] {
        let command = buglogin_api::commands::CommandRegistry::get(command_id).unwrap();
        policy
            .authorize(&platform_principal, command, &scope)
            .unwrap_or_else(|error| panic!("platform admin denied {command_id}: {error:?}"));
    }

    let lower_role = Principal {
        account_id: Some(owner.account_id.clone()),
        workspace_ids: [WorkspaceId::from_str("ws_mcp_admin_authz").unwrap()]
            .into_iter()
            .collect(),
        roles: [Role::WorkspaceAdmin].into_iter().collect(),
        token_scopes: platform_principal.token_scopes.clone(),
        ..Principal::default()
    };
    state.grants.write().unwrap().push(Grant {
        id: GrantId::from_str("grant_mcp_workspace_profile_create").unwrap(),
        account_id: Some(owner.account_id.clone()),
        member_id: None,
        capability: Capability::new("profile.create").unwrap(),
        action: Action::Create,
        scope: ResourceScope::workspace(WorkspaceId::from_str("ws_mcp_admin_authz").unwrap()),
        effect: Effect::Allow,
    });
    for (command_id, scope) in [
        (
            "group.create",
            ResourceScope::workspace(WorkspaceId::from_str("ws_mcp_admin_authz").unwrap()),
        ),
        (
            "member.role.update",
            ResourceScope {
                workspace_id: Some(WorkspaceId::from_str("ws_mcp_admin_authz").unwrap()),
                member_id: Some(MemberId::from_str("mem_mcp_admin_authz").unwrap()),
                ..ResourceScope::default()
            },
        ),
        (
            "profile.create",
            ResourceScope {
                workspace_id: Some(WorkspaceId::from_str("ws_mcp_admin_authz").unwrap()),
                profile_id: Some(ProfileId::from_str("prf_mcp_admin_authz").unwrap()),
                ..ResourceScope::default()
            },
        ),
    ] {
        let command = buglogin_api::commands::CommandRegistry::get(command_id).unwrap();
        policy
            .authorize(&lower_role, command, &scope)
            .unwrap_or_else(|error| {
                panic!(
                    "workspace admin denied equivalent workspace permission {command_id}: {error:?}"
                )
            });
    }
    let admin_only_key = Principal {
        token_scopes: Some(["CAP-ADM-001:manage".to_owned()].into_iter().collect()),
        ..platform_principal.clone()
    };
    assert!(
        policy
            .authorize(
                &admin_only_key,
                buglogin_api::commands::CommandRegistry::get("organization.create").unwrap(),
                &ResourceScope::global(),
            )
            .is_ok()
    );
    let admin_manage_without_create = Principal {
        token_scopes: Some(["CAP-ADM-001:manage".to_owned()].into_iter().collect()),
        ..platform_principal.clone()
    };
    assert!(
        policy
            .authorize(
                &admin_manage_without_create,
                buglogin_api::commands::CommandRegistry::get("account.create").unwrap(),
                &ResourceScope::global(),
            )
            .is_ok()
    );

    let command = buglogin_api::commands::CommandRegistry::get("account.list").unwrap();
    let principal = Principal {
        account_id: Some(owner.account_id.clone()),
        roles: [Role::Account].into_iter().collect(),
        ..Principal::default()
    };
    let mut policy = buglogin_api::mcp::ProductionMcpPolicy::new(state.clone());
    assert_eq!(
        policy
            .authorize(&principal, command, &ResourceScope::global())
            .unwrap_err()
            .status,
        403
    );
    assert_eq!(
        policy
            .authorize(
                &principal,
                buglogin_api::commands::CommandRegistry::get("account.create").unwrap(),
                &ResourceScope::global(),
            )
            .unwrap_err()
            .status,
        403,
        "account role cannot create accounts without the platform grant"
    );
    let status_command =
        buglogin_api::commands::CommandRegistry::get("account.status.update").unwrap();
    assert_eq!(
        policy
            .authorize(
                &principal,
                status_command,
                &ResourceScope::account(owner.account_id.clone()),
            )
            .unwrap_err()
            .status,
        403
    );

    let created_at = "2026-10-23T00:00:00Z";
    let organization_id = OrganizationId::from_str("org_mcp_api_key_crud").unwrap();
    let workspace_id = WorkspaceId::from_str("ws_mcp_api_key_crud").unwrap();
    let organization = Organization {
        id: organization_id.clone(),
        account_id: owner.account_id.clone(),
        name: "MCP API Key Organization".into(),
        created_at: created_at.into(),
    };
    let organization_policy = OrganizationPolicy {
        max_workspaces: 4,
        offline_entitled: false,
    };
    let created_organization = dispatcher
        .dispatch(production_request(
            "organization_create",
            "cor_mcp_api_key_org_create",
            Some(&token),
            Some("mcp-api-key-org-create-0001"),
            json!({
                "organization": organization,
                "policy": organization_policy
            }),
        ))
        .unwrap();
    assert_eq!(created_organization["id"], organization_id.as_str());

    let renamed_organization = Organization {
        name: "MCP API Key Organization Updated".into(),
        ..organization.clone()
    };
    let updated_organization = dispatcher
        .dispatch(production_request(
            "organization_update",
            "cor_mcp_api_key_org_update",
            Some(&token),
            Some("mcp-api-key-org-update-0001"),
            json!({
                "organizationId": organization_id,
                "update": {
                    "organization": renamed_organization,
                    "policy": organization_policy,
                    "expectedRevision": 1
                }
            }),
        ))
        .unwrap();
    assert_eq!(
        updated_organization["name"],
        "MCP API Key Organization Updated"
    );

    let workspace = Workspace {
        id: workspace_id.clone(),
        organization_id: organization_id.clone(),
        name: "MCP API Key Workspace".into(),
        policy: WorkspacePolicy {
            max_profiles: 10,
            max_members: 10,
            max_storage_bytes: 1_048_576,
            offline_entitled: false,
            default_extension_group_id: None,
        },
        created_at: created_at.into(),
    };
    let created_workspace = dispatcher
        .dispatch(production_request(
            "workspace_create",
            "cor_mcp_api_key_workspace_create",
            Some(&token),
            Some("mcp-api-key-workspace-create-0001"),
            json!({
                "organizationId": organization_id,
                "workspace": workspace
            }),
        ))
        .unwrap();
    assert_eq!(created_workspace["id"], workspace_id.as_str());

    let renamed_workspace = Workspace {
        name: "MCP API Key Workspace Updated".into(),
        ..workspace.clone()
    };
    let updated_workspace = dispatcher
        .dispatch(production_request(
            "workspace_update",
            "cor_mcp_api_key_workspace_update",
            Some(&token),
            Some("mcp-api-key-workspace-update-0001"),
            json!({
                "organizationId": organization_id,
                "workspaceId": workspace_id,
                "update": {
                    "workspace": renamed_workspace,
                    "expectedRevision": 1
                }
            }),
        ))
        .unwrap();
    assert_eq!(updated_workspace["name"], "MCP API Key Workspace Updated");

    let target_account = AccountId::from_str("acct_mcp_api_key_member").unwrap();
    let member_id = MemberId::from_str("mem_mcp_api_key_member").unwrap();
    let grants = state.grants.read().unwrap().clone();
    state
        .members
        .write()
        .unwrap()
        .invite(
            &platform_principal,
            &grants,
            Member {
                id: member_id.clone(),
                account_id: target_account.clone(),
                workspace_id: workspace_id.clone(),
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
                account_id: Some(target_account),
                ..Principal::default()
            },
            &[],
            &workspace_id,
            &member_id,
            2,
        )
        .unwrap();
    let role_updated = dispatcher
        .dispatch(production_request(
            "member_role_update",
            "cor_mcp_api_key_member_role_update",
            Some(&token),
            Some("mcp-api-key-member-role-update-0001"),
            json!({
                "workspaceId": workspace_id,
                "memberId": member_id,
                "role": "operator"
            }),
        ))
        .unwrap();
    assert_eq!(role_updated["role"], "operator");

    let group_id = "grp_mcp_api_key_crud";
    let created_group = dispatcher
        .dispatch(production_request(
            "group_create",
            "cor_mcp_api_key_group_create",
            Some(&token),
            Some("mcp-api-key-group-create-0001"),
            json!({
                "workspaceId": workspace_id,
                "groupId": group_id,
                "name": "MCP API Key Group",
                "appearance": {"color": "blue"}
            }),
        ))
        .unwrap();
    assert_eq!(created_group["group"]["id"], group_id);
    let updated_group = dispatcher
        .dispatch(production_request(
            "group_update",
            "cor_mcp_api_key_group_update",
            Some(&token),
            Some("mcp-api-key-group-update-0001"),
            json!({
                "workspaceId": workspace_id,
                "groupId": group_id,
                "expectedRevision": 1,
                "name": "MCP API Key Group Updated",
                "appearance": null
            }),
        ))
        .unwrap();
    assert_eq!(updated_group["group"]["name"], "MCP API Key Group Updated");
    let deleted_group = dispatcher
        .dispatch(production_request(
            "group_delete",
            "cor_mcp_api_key_group_delete",
            Some(&token),
            Some("mcp-api-key-group-delete-0001"),
            json!({"workspaceId": workspace_id, "groupId": group_id, "expectedRevision": 2}),
        ))
        .unwrap();
    assert_eq!(deleted_group["group"]["id"], group_id);
    assert!(
        dispatcher
            .dispatch(production_request(
                "group_list",
                "cor_mcp_api_key_group_list_after_delete",
                Some(&token),
                None,
                json!({"workspaceId": workspace_id}),
            ))
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty()
    );

    let profile_id = "prf_mcp_api_key_crud";
    let created_profile = dispatcher
        .dispatch(production_request(
            "profile_create",
            "cor_mcp_api_key_profile_create",
            Some(&token),
            Some("mcp-api-key-profile-create-0001"),
            json!({
                "id": profile_id,
                "workspaceId": workspace_id,
                "name": "MCP API Key Profile",
                "runtime": "wayfern",
                "runtimeVersion": "154.0.8037.57"
            }),
        ))
        .unwrap();
    assert_eq!(created_profile["id"], profile_id);
    let updated_profile = dispatcher
        .dispatch(production_request(
            "profile_update",
            "cor_mcp_api_key_profile_update",
            Some(&token),
            Some("mcp-api-key-profile-update-0001"),
            json!({
                "workspaceId": workspace_id,
                "profileId": profile_id,
                "expectedRevision": 1,
                "name": "MCP API Key Profile Updated",
                "note": null,
                "tags": []
            }),
        ))
        .unwrap();
    assert_eq!(updated_profile["name"], "MCP API Key Profile Updated");
    let deleted_profile = dispatcher
        .dispatch(production_request(
            "profile_delete",
            "cor_mcp_api_key_profile_delete",
            Some(&token),
            Some("mcp-api-key-profile-delete-0001"),
            json!({
                "profileId": profile_id,
                "revision": 2,
                "confirmation": "QUARANTINE_PROFILE_DATA"
            }),
        ))
        .unwrap();
    assert_eq!(deleted_profile["deleted"], true);

    let workspace_admin = Principal {
        account_id: Some(owner.account_id.clone()),
        workspace_ids: [workspace_id.clone()].into_iter().collect(),
        roles: [Role::WorkspaceAdmin].into_iter().collect(),
        ..Principal::default()
    };
    for (command_id, scope) in [
        (
            "workspace.update",
            ResourceScope {
                organization_id: Some(organization_id.clone()),
                workspace_id: Some(workspace_id.clone()),
                ..ResourceScope::default()
            },
        ),
        (
            "member.role.update",
            ResourceScope {
                workspace_id: Some(workspace_id.clone()),
                member_id: Some(member_id.clone()),
                ..ResourceScope::default()
            },
        ),
        (
            "group.create",
            ResourceScope::workspace(workspace_id.clone()),
        ),
        (
            "group.update",
            ResourceScope::workspace(workspace_id.clone()),
        ),
        (
            "group.delete",
            ResourceScope::workspace(workspace_id.clone()),
        ),
    ] {
        policy
            .authorize(
                &workspace_admin,
                buglogin_api::commands::CommandRegistry::get(command_id).unwrap(),
                &scope,
            )
            .unwrap_or_else(|error| {
                panic!("WorkspaceAdmin denied same-workspace command {command_id}: {error:?}")
            });
    }
    for (command_id, scope) in [
        ("account.list", ResourceScope::global()),
        ("account.create", ResourceScope::global()),
        (
            "organization.create",
            ResourceScope {
                account_id: Some(owner.account_id.clone()),
                organization_id: Some(organization_id.clone()),
                ..ResourceScope::default()
            },
        ),
        (
            "organization.update",
            ResourceScope {
                account_id: Some(owner.account_id.clone()),
                organization_id: Some(organization_id.clone()),
                ..ResourceScope::default()
            },
        ),
    ] {
        assert_eq!(
            policy
                .authorize(
                    &workspace_admin,
                    buglogin_api::commands::CommandRegistry::get(command_id).unwrap(),
                    &scope,
                )
                .unwrap_err()
                .status,
            403,
            "WorkspaceAdmin must not use global/account/org command {command_id}"
        );
    }

    let updated = dispatcher
        .dispatch(production_request(
            "account_status_update",
            "cor_mcp_platform_account_status_update",
            Some(&token),
            Some("mcp-platform-account-status-update-01"),
            json!({
                "accountId": owner.account_id.as_str(),
                "status": "suspended",
                "expectedRevision": owner_revision,
            }),
        ))
        .unwrap();
    assert_eq!(updated["status"], "suspended");
    assert!(updated["revision"].as_u64().unwrap() > owner_revision);
}

#[test]
fn production_dispatchers_share_atomic_profile_create_claim() {
    let (root, state) = production_state("profile-create-concurrent");
    let token = mcp_token(&state, "acct_mcp_concurrent");
    grant_profile(
        &state,
        "acct_mcp_concurrent",
        "prf_mcp_concurrent",
        "profile.create",
        Action::Create,
    );
    let input = serde_json::to_value(profile("prf_mcp_concurrent", "Concurrent profile")).unwrap();
    let barrier = Arc::new(Barrier::new(3));
    let handles: Vec<_> = (0..2)
        .map(|index| {
            let state = state.clone();
            let token = token.clone();
            let input = input.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let mut dispatcher = production_dispatcher(&state);
                barrier.wait();
                dispatcher.dispatch(production_request(
                    "profile_create",
                    &format!("cor_profile_concurrent_{index}"),
                    Some(&token),
                    Some("profile-create-concurrent-0001"),
                    input,
                ))
            })
        })
        .collect();
    barrier.wait();
    let results: Vec<_> = handles
        .into_iter()
        .map(|handle| handle.join().unwrap().unwrap())
        .collect();

    assert_eq!(results.len(), 2);
    assert!(results.iter().all(|result| {
        result["id"] == "prf_mcp_concurrent" || result["status"] == "in_progress"
    }));
    assert_eq!(state.profiles.list().unwrap().len(), 1);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn production_profile_list_filters_unauthorized_profiles() {
    let (root, state) = production_state("profile-list-filter");
    let token = mcp_token(&state, "acct_mcp_reader");
    state
        .profiles
        .create(profile("prf_mcp_visible", "Visible profile"))
        .unwrap();
    state
        .profiles
        .create(profile("prf_mcp_hidden", "Hidden profile"))
        .unwrap();
    grant_profile(
        &state,
        "acct_mcp_reader",
        "prf_mcp_visible",
        "profile.read",
        Action::Read,
    );

    let result = production_dispatcher(&state)
        .dispatch(production_request(
            "profile_list",
            "cor_profile_list_filter",
            Some(&token),
            None,
            json!({}),
        ))
        .unwrap();
    let items = result["items"].as_array().unwrap();

    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], "prf_mcp_visible");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn production_profile_archive_and_restore_use_revision_and_replay() {
    let (root, state) = production_state("profile-archive-restore");
    let token = mcp_token(&state, "acct_mcp_archive");
    state
        .profiles
        .create(profile("prf_mcp_archive", "Archive profile"))
        .unwrap();
    grant_profile(
        &state,
        "acct_mcp_archive",
        "prf_mcp_archive",
        "profile.archive",
        Action::Update,
    );
    grant_profile(
        &state,
        "acct_mcp_archive",
        "prf_mcp_archive",
        "profile.restore",
        Action::Update,
    );

    let archived = production_dispatcher(&state)
        .dispatch(production_request(
            "profile_archive",
            "cor_profile_archive",
            Some(&token),
            Some("profile-archive-0001"),
            json!({ "profileId": "prf_mcp_archive", "revision": 1 }),
        ))
        .unwrap();
    assert_eq!(archived["archived"], true);
    assert_eq!(archived["revision"], 2);
    assert_eq!(
        state
            .profiles
            .get(&ProfileId::from_str("prf_mcp_archive").unwrap())
            .unwrap()
            .unwrap()
            .revision,
        2
    );

    let replay = production_dispatcher(&state)
        .dispatch(production_request(
            "profile_archive",
            "cor_profile_archive_replay",
            Some(&token),
            Some("profile-archive-0001"),
            json!({ "profileId": "prf_mcp_archive", "revision": 1 }),
        ))
        .unwrap();
    assert_eq!(replay, archived);

    let restored = production_dispatcher(&state)
        .dispatch(production_request(
            "profile_restore",
            "cor_profile_restore",
            Some(&token),
            Some("profile-restore-0001"),
            json!({ "profileId": "prf_mcp_archive", "revision": 2 }),
        ))
        .unwrap();
    assert_eq!(restored["archived"], false);
    assert_eq!(
        state
            .profiles
            .get(&ProfileId::from_str("prf_mcp_archive").unwrap())
            .unwrap()
            .unwrap()
            .revision,
        3
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn production_profile_archive_rejects_stale_revision_without_mutation() {
    let (root, state) = production_state("profile-archive-revision");
    let token = mcp_token(&state, "acct_mcp_archive_revision");
    state
        .profiles
        .create(profile("prf_mcp_archive_revision", "Archive revision"))
        .unwrap();
    grant_profile(
        &state,
        "acct_mcp_archive_revision",
        "prf_mcp_archive_revision",
        "profile.archive",
        Action::Update,
    );

    let error = production_dispatcher(&state)
        .dispatch(production_request(
            "profile_archive",
            "cor_profile_archive_revision",
            Some(&token),
            Some("profile-archive-revision-0001"),
            json!({ "profileId": "prf_mcp_archive_revision", "revision": 99 }),
        ))
        .unwrap_err();
    assert_eq!(error.status, 409);
    assert_eq!(error.code, "storage_revision_conflict");
    let stored = state
        .profiles
        .get(&ProfileId::from_str("prf_mcp_archive_revision").unwrap())
        .unwrap()
        .unwrap();
    assert!(!stored.value.archived);
    assert_eq!(stored.revision, 1);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn production_profile_delete_uses_shared_service_and_idempotent_replay() {
    let (root, state) = production_state("profile-delete-replay");
    let account_id = "acct_mcp_profile_delete";
    let profile_id = "prf_mcp_delete";
    let token = mcp_token(&state, account_id);
    state
        .profiles
        .create(profile(profile_id, "Delete profile"))
        .unwrap();
    grant_profile(
        &state,
        account_id,
        profile_id,
        "profile.delete",
        Action::Delete,
    );

    let mut dispatcher = production_dispatcher(&state);
    let deleted = dispatcher
        .dispatch(production_request(
            "profile_delete",
            "cor_profile_delete",
            Some(&token),
            Some("profile-delete-mcp-0001"),
            json!({
                "profileId": profile_id,
                "revision": 1,
                "confirmation": "QUARANTINE_PROFILE_DATA"
            }),
        ))
        .unwrap();
    assert_eq!(deleted["id"], profile_id);
    assert_eq!(deleted["deleted"], true);
    assert_eq!(deleted["recoverable"], true);
    assert!(
        state
            .profiles
            .get(&ProfileId::from_str(profile_id).unwrap())
            .unwrap()
            .is_none()
    );

    let replay = dispatcher
        .dispatch(production_request(
            "profile_delete",
            "cor_profile_delete_replay",
            Some(&token),
            Some("profile-delete-mcp-0001"),
            json!({
                "profileId": profile_id,
                "revision": 1,
                "confirmation": "QUARANTINE_PROFILE_DATA"
            }),
        ))
        .unwrap();
    assert_eq!(replay, deleted);

    let conflict = dispatcher
        .dispatch(production_request(
            "profile_delete",
            "cor_profile_delete_conflict",
            Some(&token),
            Some("profile-delete-mcp-0001"),
            json!({
                "profileId": profile_id,
                "revision": 2,
                "confirmation": "QUARANTINE_PROFILE_DATA"
            }),
        ))
        .unwrap_err();
    assert_eq!(conflict.code, "idempotency_conflict");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn production_local_profile_create_requires_existing_selected_proxy() {
    let (root, state) = production_state("profile-proxy-local");
    let account_id = "acct_mcp_profile_proxy_local";
    let workspace_id = "ws_mcp_profile_proxy_local";
    let proxy_id = "pxy_mcp_profile_proxy_local";
    let token = mcp_token(&state, account_id);
    grant_profile(
        &state,
        account_id,
        "prf_mcp_profile_proxy_local",
        "profile.create",
        Action::Create,
    );
    let missing = production_dispatcher(&state)
        .dispatch(production_request(
            "profile_create",
            "cor_profile_proxy_local_missing",
            Some(&token),
            Some("profile-proxy-local-missing"),
            profile_create_input(
                profile("prf_mcp_profile_proxy_local", "Local proxy profile"),
                workspace_id,
                proxy_id,
            ),
        ))
        .unwrap_err();
    assert_eq!(missing.code, "proxy_not_found");
    assert!(state.profiles.list().unwrap().is_empty());

    seed_proxy(&state, account_id, workspace_id, proxy_id);
    let created = production_dispatcher(&state)
        .dispatch(production_request(
            "profile_create",
            "cor_profile_proxy_local_created",
            Some(&token),
            Some("profile-proxy-local-created"),
            profile_create_input(
                profile("prf_mcp_profile_proxy_local", "Local proxy profile"),
                workspace_id,
                proxy_id,
            ),
        ))
        .unwrap();
    assert_eq!(created["proxyId"], proxy_id);
    assert_eq!(
        state.profiles.list().unwrap()[0]
            .value
            .proxy_id
            .as_ref()
            .unwrap()
            .as_str(),
        proxy_id
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn production_hosted_profile_create_requires_proxy_read_before_writes() {
    let account_id = "acct_mcp_profile_proxy_hosted_guard";
    let workspace_id = "ws_mcp_profile_proxy_hosted_guard";
    let proxy_id = "pxy_mcp_profile_proxy_hosted_guard";
    let profile_id = "prf_mcp_profile_proxy_hosted_guard";
    let state = hosted_state(account_id);
    seed_workspace(&state, account_id, workspace_id, 10);
    let token = mcp_token(&state, account_id);
    seed_proxy(&state, account_id, workspace_id, proxy_id);
    grant_profile(
        &state,
        account_id,
        profile_id,
        "profile.create",
        Action::Create,
    );
    grant_profile(&state, account_id, profile_id, "profile.read", Action::Read);
    grant_workspace_capability(
        &state,
        account_id,
        workspace_id,
        "CAP-RES-001",
        Action::Create,
    );
    let mut managed = profile(profile_id, "Hosted guarded profile");
    managed.sync_mode = Some(SyncMode::Managed);

    let denied = production_dispatcher(&state)
        .dispatch(production_request(
            "profile_create",
            "cor_profile_proxy_hosted_denied",
            Some(&token),
            Some("profile-proxy-hosted-denied"),
            profile_create_input(managed, workspace_id, proxy_id),
        ))
        .unwrap_err();
    assert_eq!(denied.code, "forbidden");
    let missing = production_dispatcher(&state)
        .dispatch(production_request(
            "profile_get",
            "cor_profile_proxy_hosted_denied_get",
            Some(&token),
            None,
            json!({ "profileId": profile_id }),
        ))
        .unwrap_err();
    assert_eq!(missing.code, "profile_not_found");
}

#[test]
fn production_profile_proxy_assignment_uses_canonical_workspace_revision_contract() {
    let account_id = "acct_mcp_proxy_assignment";
    let workspace_id = "ws_mcp_proxy_assignment";
    let proxy_id = "pxy_mcp_proxy_assignment";
    let profile_id = "prf_mcp_proxy_assignment";
    let state = hosted_state(account_id);
    seed_workspace(&state, account_id, workspace_id, 10);
    let token = mcp_token(&state, account_id);
    seed_proxy(&state, account_id, workspace_id, proxy_id);
    grant_proxy(&state, account_id, workspace_id, Action::Read);
    grant_profile(
        &state,
        account_id,
        profile_id,
        "profile.create",
        Action::Create,
    );
    grant_profile(&state, account_id, profile_id, "profile.read", Action::Read);
    grant_profile(
        &state,
        account_id,
        profile_id,
        "profile.update",
        Action::Update,
    );
    grant_workspace_capability(
        &state,
        account_id,
        workspace_id,
        "CAP-RES-001",
        Action::Create,
    );
    let mut managed = profile(profile_id, "MCP proxy assignment");
    managed.sync_mode = Some(SyncMode::Managed);
    let created = production_dispatcher(&state)
        .dispatch(production_request(
            "profile_create",
            "cor_mcp_proxy_assignment_create",
            Some(&token),
            Some("mcp-proxy-assignment-create-01"),
            profile_create_input(managed, workspace_id, proxy_id),
        ))
        .unwrap();
    assert_eq!(created["proxyId"], proxy_id);
    assert_eq!(created["revision"], 1);

    let input = json!({
        "workspaceId": workspace_id,
        "profileId": profile_id,
        "expectedProfileRevision": 1,
        "proxyId": null
    });
    let staged = production_dispatcher(&state)
        .dispatch(production_request(
            "profile_proxy_assignment_set",
            "cor_mcp_proxy_assignment_set",
            Some(&token),
            Some("mcp-proxy-assignment-set-0001"),
            input.clone(),
        ))
        .unwrap();
    assert_eq!(staged["proxyId"], Value::Null);
    assert_eq!(staged["expectedProfileRevision"], 1);
    let replayed = production_dispatcher(&state)
        .dispatch(production_request(
            "profile_proxy_assignment_set",
            "cor_mcp_proxy_assignment_replay",
            Some(&token),
            Some("mcp-proxy-assignment-set-0001"),
            input,
        ))
        .unwrap();
    assert_eq!(replayed, staged);

    let pending = production_dispatcher(&state)
        .dispatch(production_request(
            "profile_proxy_assignment_get",
            "cor_mcp_proxy_assignment_get",
            Some(&token),
            None,
            json!({ "workspaceId": workspace_id, "profileId": profile_id }),
        ))
        .unwrap();
    assert_eq!(pending, staged);
    let active = production_dispatcher(&state)
        .dispatch(production_request(
            "profile_get",
            "cor_mcp_proxy_assignment_profile_get",
            Some(&token),
            None,
            json!({ "profileId": profile_id }),
        ))
        .unwrap();
    assert_eq!(active["proxyId"], proxy_id);
    assert_eq!(active["revision"], 1);

    let pending_update = production_dispatcher(&state)
        .dispatch(production_request(
            "profile_update",
            "cor_mcp_proxy_assignment_metadata_update",
            Some(&token),
            Some("mcp-proxy-assignment-metadata-update-01"),
            json!({
                "workspaceId": workspace_id,
                "profileId": profile_id,
                "expectedRevision": 1,
                "name": "MCP proxy assignment renamed",
                "note": null,
                "tags": []
            }),
        ))
        .unwrap_err();
    assert_eq!(pending_update.code, "profile_proxy_assignment_pending");
    let stale_pending = production_dispatcher(&state)
        .dispatch(production_request(
            "profile_proxy_assignment_get",
            "cor_mcp_proxy_assignment_stale_get",
            Some(&token),
            None,
            json!({ "workspaceId": workspace_id, "profileId": profile_id }),
        ))
        .unwrap();
    assert_eq!(stale_pending, staged);
}

#[test]
fn production_hosted_profile_create_rejects_missing_proxy_before_writes() {
    let account_id = "acct_mcp_profile_proxy_hosted_missing";
    let workspace_id = "ws_mcp_profile_proxy_hosted_missing";
    let proxy_id = "pxy_mcp_profile_proxy_hosted_missing";
    let profile_id = "prf_mcp_profile_proxy_hosted_missing";
    let state = hosted_state(account_id);
    let token = mcp_token(&state, account_id);
    grant_proxy(&state, account_id, workspace_id, Action::Read);
    grant_profile(
        &state,
        account_id,
        profile_id,
        "profile.create",
        Action::Create,
    );
    grant_profile(&state, account_id, profile_id, "profile.read", Action::Read);
    grant_workspace_capability(
        &state,
        account_id,
        workspace_id,
        "CAP-RES-001",
        Action::Create,
    );
    let mut managed = profile(profile_id, "Hosted missing proxy profile");
    managed.sync_mode = Some(SyncMode::Managed);

    let missing_proxy = production_dispatcher(&state)
        .dispatch(production_request(
            "profile_create",
            "cor_profile_proxy_hosted_missing",
            Some(&token),
            Some("profile-proxy-hosted-missing"),
            profile_create_input(managed, workspace_id, proxy_id),
        ))
        .unwrap_err();
    assert_eq!(missing_proxy.code, "proxy_not_found");
    let missing_profile = production_dispatcher(&state)
        .dispatch(production_request(
            "profile_get",
            "cor_profile_proxy_hosted_missing_get",
            Some(&token),
            None,
            json!({ "profileId": profile_id }),
        ))
        .unwrap_err();
    assert_eq!(missing_profile.code, "profile_not_found");
}

#[test]
fn production_hosted_profile_create_persists_selected_proxy() {
    let account_id = "acct_mcp_profile_proxy_hosted";
    let workspace_id = "ws_mcp_profile_proxy_hosted";
    let proxy_id = "pxy_mcp_profile_proxy_hosted";
    let profile_id = "prf_mcp_profile_proxy_hosted";
    let state = hosted_state(account_id);
    seed_workspace(&state, account_id, workspace_id, 10);
    let token = mcp_token(&state, account_id);
    seed_proxy(&state, account_id, workspace_id, proxy_id);
    grant_proxy(&state, account_id, workspace_id, Action::Read);
    grant_profile(
        &state,
        account_id,
        profile_id,
        "profile.create",
        Action::Create,
    );
    grant_profile(&state, account_id, profile_id, "profile.read", Action::Read);
    grant_workspace_capability(
        &state,
        account_id,
        workspace_id,
        "CAP-RES-001",
        Action::Create,
    );
    let mut managed = profile(profile_id, "Hosted proxy profile");
    managed.sync_mode = Some(SyncMode::Managed);

    let created = production_dispatcher(&state)
        .dispatch(production_request(
            "profile_create",
            "cor_profile_proxy_hosted_created",
            Some(&token),
            Some("profile-proxy-hosted-created"),
            profile_create_input(managed, workspace_id, proxy_id),
        ))
        .unwrap();
    assert_eq!(created["state"], "ready");
    assert_eq!(created["proxyId"], proxy_id);
    assert_eq!(created["revision"], 1);
    let fetched = production_dispatcher(&state)
        .dispatch(production_request(
            "profile_get",
            "cor_profile_proxy_hosted_get",
            Some(&token),
            None,
            json!({ "profileId": profile_id }),
        ))
        .unwrap();
    assert_eq!(fetched["proxyId"], proxy_id);
    assert_eq!(fetched["revision"], 1);
    let serialized = serde_json::to_string(&json!([created, fetched])).unwrap();
    for forbidden in ["credentialRef", "rotationUrl", "username", "password"] {
        assert!(!serialized.contains(forbidden));
    }
}

#[test]
fn production_proxy_parse_normalizes_and_redacts_credentials() {
    let (root, state) = production_state("proxy-parse");
    let account_id = "acct_mcp_proxy_parse";
    let token = mcp_token(&state, account_id);
    state.grants.write().unwrap().push(Grant {
        id: GrantId::from_str("grant_proxy_parse").unwrap(),
        account_id: Some(AccountId::from_str(account_id).unwrap()),
        member_id: None,
        capability: Capability::new("CAP-NET-001").unwrap(),
        action: Action::Read,
        scope: ResourceScope::account(AccountId::from_str(account_id).unwrap()),
        effect: Effect::Allow,
    });
    let result = production_dispatcher(&state)
        .dispatch(production_request(
            "proxy_parse",
            "cor_proxy_parse",
            Some(&token),
            None,
            json!({
                "text": "http://proxy-user:proxy-secret@127.0.0.1:8080\ninvalid\nhttp://proxy-user:proxy-secret@127.0.0.1:8080"
            }),
        ))
        .unwrap();

    assert_eq!(result["validCount"], 1);
    assert_eq!(result["invalidCount"], 1);
    assert_eq!(result["duplicateCount"], 1);
    assert_eq!(result["rows"][0]["protocol"], "http");
    assert_eq!(result["rows"][0]["endpoint"], "127.0.0.1:8080");
    assert_eq!(result["rows"][1]["line"], 2);
    assert_eq!(result["rows"][1]["error"], "invalid_port");
    let serialized = serde_json::to_string(&result).unwrap();
    assert!(!serialized.contains("proxy-user"));
    assert!(!serialized.contains("proxy-secret"));
    assert!(!serialized.contains("username"));
    assert!(!serialized.contains("password"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn production_proxy_crud_replays_and_redacts_inventory() {
    let (root, state) = production_state("proxy-crud");
    let account_id = "acct_mcp_proxy_crud";
    let workspace_id = "ws_mcp_proxy_crud";
    let proxy_id = "pxy_mcp_proxy_crud";
    let token = mcp_token(&state, account_id);
    for action in [Action::Read, Action::Create, Action::Update, Action::Delete] {
        grant_proxy(&state, account_id, workspace_id, action);
    }
    let mut create_input = proxy_input(workspace_id, proxy_id, "Primary");
    create_input["proxy"]["hasCredential"] = json!(false);
    let created = production_dispatcher(&state)
        .dispatch(production_request(
            "proxy_create",
            "cor_proxy_create",
            Some(&token),
            Some("proxy-create-key-0001"),
            create_input.clone(),
        ))
        .unwrap();
    let replay = production_dispatcher(&state)
        .dispatch(production_request(
            "proxy_create",
            "cor_proxy_create_replay",
            Some(&token),
            Some("proxy-create-key-0001"),
            create_input,
        ))
        .unwrap();
    assert_eq!(replay, created);
    assert_eq!(created["record"]["revision"], 1);
    assert_eq!(created["record"]["proxy"]["host"], "proxy.example.com");
    assert_eq!(created["record"]["proxy"]["hasCredential"], false);
    assert_eq!(created["record"]["proxy"]["rotationConfigured"], false);

    let listed = production_dispatcher(&state)
        .dispatch(production_request(
            "proxy_list",
            "cor_proxy_list",
            Some(&token),
            None,
            json!({ "workspaceId": workspace_id }),
        ))
        .unwrap();
    assert_eq!(listed["items"].as_array().unwrap().len(), 1);
    let fetched = production_dispatcher(&state)
        .dispatch(production_request(
            "proxy_get",
            "cor_proxy_get",
            Some(&token),
            None,
            json!({ "workspaceId": workspace_id, "proxyId": proxy_id }),
        ))
        .unwrap();
    assert_eq!(fetched, created["record"]);

    let mut update_input = proxy_input(workspace_id, proxy_id, "Updated");
    update_input["proxyId"] = json!(proxy_id);
    update_input["expectedRevision"] = json!(1);
    update_input["proxy"]["updatedAt"] = json!("2026-07-22T01:00:00Z");
    let updated = production_dispatcher(&state)
        .dispatch(production_request(
            "proxy_update",
            "cor_proxy_update",
            Some(&token),
            Some("proxy-update-key-0001"),
            update_input,
        ))
        .unwrap();
    assert_eq!(updated["revision"], 2);
    assert_eq!(updated["proxy"]["name"], "Updated");

    let stale = production_dispatcher(&state)
        .dispatch(production_request(
            "proxy_delete",
            "cor_proxy_delete_stale",
            Some(&token),
            Some("proxy-delete-key-stale"),
            json!({
                "workspaceId": workspace_id,
                "proxyId": proxy_id,
                "expectedRevision": 1
            }),
        ))
        .unwrap_err();
    assert_eq!(stale.code, "storage_revision_conflict");
    assert_eq!(stale.status, 409);

    let deleted = production_dispatcher(&state)
        .dispatch(production_request(
            "proxy_delete",
            "cor_proxy_delete",
            Some(&token),
            Some("proxy-delete-key-0001"),
            json!({
                "workspaceId": workspace_id,
                "proxyId": proxy_id,
                "expectedRevision": 2
            }),
        ))
        .unwrap();
    let serialized =
        serde_json::to_string(&json!([created, listed, fetched, updated, deleted])).unwrap();
    for forbidden in ["credentialRef", "rotationUrl", "username", "password"] {
        assert!(!serialized.contains(forbidden));
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn production_proxy_create_enforces_idempotency_and_workspace_acl() {
    let (root, state) = production_state("proxy-guards");
    let account_id = "acct_mcp_proxy_guards";
    let workspace_id = "ws_mcp_proxy_allowed";
    let token = mcp_token(&state, account_id);
    grant_proxy(&state, account_id, workspace_id, Action::Create);
    production_dispatcher(&state)
        .dispatch(production_request(
            "proxy_create",
            "cor_proxy_guard_create",
            Some(&token),
            Some("proxy-guard-key-0001"),
            proxy_input(workspace_id, "pxy_mcp_proxy_guard", "First"),
        ))
        .unwrap();

    let conflict = production_dispatcher(&state)
        .dispatch(production_request(
            "proxy_create",
            "cor_proxy_guard_conflict",
            Some(&token),
            Some("proxy-guard-key-0001"),
            proxy_input(workspace_id, "pxy_mcp_proxy_guard", "Changed"),
        ))
        .unwrap_err();
    assert_eq!(conflict.code, "idempotency_conflict");

    let forbidden = production_dispatcher(&state)
        .dispatch(production_request(
            "proxy_create",
            "cor_proxy_guard_forbidden",
            Some(&token),
            Some("proxy-guard-key-0002"),
            proxy_input("ws_mcp_proxy_denied", "pxy_mcp_proxy_denied", "Denied"),
        ))
        .unwrap_err();
    assert_eq!(forbidden.code, "forbidden");
    assert_eq!(forbidden.status, 403);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn production_group_commands_share_workspace_acl_revision_and_idempotency_contracts() {
    let (root, state) = production_state("group-crud");
    let account_id = "acct_mcp_group_crud";
    let workspace_id = "ws_mcp_group_crud";
    let group_id = "grp_mcp_group_crud";
    let token = mcp_token(&state, account_id);
    for action in [Action::Read, Action::Create, Action::Update, Action::Delete] {
        grant_workspace_capability(&state, account_id, workspace_id, "CAP-PROF-003", action);
    }
    grant_workspace_capability(
        &state,
        account_id,
        workspace_id,
        "CAP-PROF-003",
        Action::Share,
    );
    grant_workspace_capability(
        &state,
        account_id,
        workspace_id,
        "CAP-MEM-001",
        Action::Create,
    );
    grant_workspace_capability(
        &state,
        account_id,
        workspace_id,
        "profile.update",
        Action::Update,
    );

    let profile_id = ProfileId::from_str("prf_mcp_group_membership").unwrap();
    state
        .profiles
        .create(profile(profile_id.as_str(), "Grouped profile"))
        .unwrap();
    let state = state.with_profile_resource(ProfileResource {
        profile_id: profile_id.clone(),
        resource_id: ResourceId::from_str("res_mcp_group_membership").unwrap(),
        workspace_id: WorkspaceId::from_str(workspace_id).unwrap(),
        archive_state: ProfileArchiveState::Active,
    });
    let member_id = MemberId::from_str("mem_mcp_group_share_target").unwrap();
    let target_account = AccountId::from_str("acct_mcp_group_share_target").unwrap();
    state
        .members
        .write()
        .unwrap()
        .invite(
            &Principal {
                account_id: Some(AccountId::from_str(account_id).unwrap()),
                roles: [buglogin_domain::permissions::Role::PlatformAdmin]
                    .into_iter()
                    .collect(),
                ..Principal::default()
            },
            &state.grants.read().unwrap(),
            Member {
                id: member_id.clone(),
                account_id: target_account.clone(),
                workspace_id: WorkspaceId::from_str(workspace_id).unwrap(),
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
                account_id: Some(target_account.clone()),
                ..Principal::default()
            },
            &[],
            &WorkspaceId::from_str(workspace_id).unwrap(),
            &member_id,
            2,
        )
        .unwrap();
    let mut dispatcher = production_dispatcher(&state);
    let create_input = json!({
        "workspaceId": workspace_id,
        "groupId": group_id,
        "name": "  Operations  ",
        "appearance": {"color": "blue"}
    });
    let created = dispatcher
        .dispatch(production_request(
            "group_create",
            "cor_group_create",
            Some(&token),
            Some("group-create-mcp-0001"),
            create_input.clone(),
        ))
        .unwrap();
    assert_eq!(created["group"]["name"], "Operations");
    assert_eq!(created["revision"], 1);
    let replay = dispatcher
        .dispatch(production_request(
            "group_create",
            "cor_group_create_replay",
            Some(&token),
            Some("group-create-mcp-0001"),
            create_input,
        ))
        .unwrap();
    assert_eq!(replay, created);

    let listed = dispatcher
        .dispatch(production_request(
            "group_list",
            "cor_group_list",
            Some(&token),
            None,
            json!({"workspaceId": workspace_id}),
        ))
        .unwrap();
    assert_eq!(listed.as_array().unwrap().len(), 1);

    let updated = dispatcher
        .dispatch(production_request(
            "group_update",
            "cor_group_update",
            Some(&token),
            Some("group-update-mcp-0001"),
            json!({
                "workspaceId": workspace_id,
                "groupId": group_id,
                "expectedRevision": 1,
                "name": "Paid",
                "appearance": null
            }),
        ))
        .unwrap();
    assert_eq!(updated["revision"], 2);

    let assigned_groups = dispatcher
        .dispatch(production_request(
            "profile_groups_replace",
            "cor_profile_groups_replace",
            Some(&token),
            Some("profile-groups-replace-mcp-0001"),
            json!({
                "workspaceId": workspace_id,
                "profileId": profile_id,
                "expectedRevision": 1,
                "groupIds": [group_id]
            }),
        ))
        .unwrap();
    assert_eq!(assigned_groups["revision"], 2);
    assert_eq!(assigned_groups["groupIds"], json!([group_id]));

    let share_input = json!({
        "workspaceId": workspace_id,
        "groupId": group_id,
        "memberId": member_id,
        "action": "read",
        "effect": "allow"
    });
    let created_share = dispatcher
        .dispatch(production_request(
            "group_share_create",
            "cor_group_share_create",
            Some(&token),
            Some("group-share-create-mcp-0001"),
            share_input.clone(),
        ))
        .unwrap();
    assert_eq!(created_share["revision"], 1);
    let share_id = created_share["id"].as_str().unwrap().to_owned();
    let recipient = Principal {
        account_id: Some(target_account),
        workspace_ids: [WorkspaceId::from_str(workspace_id).unwrap()]
            .into_iter()
            .collect(),
        ..Principal::default()
    };
    assert!(
        buglogin_api::services::profile_hosted::HostedProfileService::authorized(
            &state,
            &recipient,
            &[],
            &profile_id,
            Action::Read,
            "profile.read",
        )
        .unwrap()
    );
    let listed_shares = dispatcher
        .dispatch(production_request(
            "group_share_list",
            "cor_group_share_list",
            Some(&token),
            None,
            json!({"workspaceId": workspace_id, "groupId": group_id}),
        ))
        .unwrap();
    assert_eq!(listed_shares.as_array().unwrap().len(), 1);
    let revoked_share = dispatcher
        .dispatch(production_request(
            "group_share_revoke",
            "cor_group_share_revoke",
            Some(&token),
            Some("group-share-revoke-mcp-0001"),
            json!({
                "workspaceId": workspace_id,
                "groupId": group_id,
                "grantId": share_id,
                "expectedRevision": 1
            }),
        ))
        .unwrap();
    assert_eq!(revoked_share["revision"], 2);
    assert!(
        !buglogin_api::services::profile_hosted::HostedProfileService::authorized(
            &state,
            &recipient,
            &[],
            &profile_id,
            Action::Read,
            "profile.read",
        )
        .unwrap()
    );

    let assigned_groups_replay = dispatcher
        .dispatch(production_request(
            "profile_groups_replace",
            "cor_profile_groups_replace_replay",
            Some(&token),
            Some("profile-groups-replace-mcp-0001"),
            json!({
                "workspaceId": workspace_id,
                "profileId": profile_id,
                "expectedRevision": 1,
                "groupIds": [group_id]
            }),
        ))
        .unwrap();
    assert_eq!(assigned_groups_replay, assigned_groups);

    let cleared_groups = dispatcher
        .dispatch(production_request(
            "profile_groups_replace",
            "cor_profile_groups_clear",
            Some(&token),
            Some("profile-groups-clear-mcp-0001"),
            json!({
                "workspaceId": workspace_id,
                "profileId": profile_id,
                "expectedRevision": 2,
                "groupIds": []
            }),
        ))
        .unwrap();
    assert_eq!(cleared_groups["revision"], 3);
    assert_eq!(cleared_groups["groupIds"], json!([]));

    let profile_update_input = json!({
        "workspaceId": workspace_id,
        "profileId": profile_id,
        "expectedRevision": 3,
        "name": "  Renamed profile  ",
        "note": null,
        "tags": ["automation"]
    });
    let profile_updated = dispatcher
        .dispatch(production_request(
            "profile_update",
            "cor_profile_update_mcp",
            Some(&token),
            Some("profile-update-mcp-0001"),
            profile_update_input.clone(),
        ))
        .unwrap();
    assert_eq!(profile_updated["revision"], 4);
    assert_eq!(profile_updated["name"], "Renamed profile");
    assert_eq!(profile_updated["note"], Value::Null);
    assert_eq!(profile_updated["tags"], json!(["automation"]));

    let profile_update_replay = dispatcher
        .dispatch(production_request(
            "profile_update",
            "cor_profile_update_mcp_replay",
            Some(&token),
            Some("profile-update-mcp-0001"),
            profile_update_input,
        ))
        .unwrap();
    assert_eq!(profile_update_replay, profile_updated);

    // The REST PUT adapter delegates to HostedProfileService::update_metadata
    // too, so both transports must reject a stale profile revision the same way.
    let stale_profile_update = dispatcher
        .dispatch(production_request(
            "profile_update",
            "cor_profile_update_mcp_stale",
            Some(&token),
            Some("profile-update-mcp-stale-0001"),
            json!({
                "workspaceId": workspace_id,
                "profileId": profile_id,
                "expectedRevision": 3,
                "name": "Stale overwrite",
                "note": null,
                "tags": ["automation"]
            }),
        ))
        .unwrap_err();
    assert_eq!(stale_profile_update.code, "revision_conflict");
    assert_eq!(stale_profile_update.status, 409);

    let stale = dispatcher
        .dispatch(production_request(
            "group_delete",
            "cor_group_delete_stale",
            Some(&token),
            Some("group-delete-mcp-stale"),
            json!({"workspaceId": workspace_id, "groupId": group_id, "expectedRevision": 1}),
        ))
        .unwrap_err();
    assert_eq!(stale.code, "revision_conflict");
    assert_eq!(stale.status, 409);

    let retained_history = dispatcher
        .dispatch(production_request(
            "group_delete",
            "cor_group_delete",
            Some(&token),
            Some("group-delete-mcp-0001"),
            json!({"workspaceId": workspace_id, "groupId": group_id, "expectedRevision": 2}),
        ))
        .unwrap_err();
    assert_eq!(retained_history.code, "group_share_history_retained");
    assert_eq!(retained_history.status, 409);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn production_extension_group_mcp_commands_enforce_workspace_acl_and_revisions() {
    let (root, state) = production_state("extension-group-crud");
    let account_id = "acct_mcp_extension_group";
    let workspace_id = "ws_mcp_extension_group";
    let group_id = "extgrp_mcp_extension_group";
    seed_workspace(&state, account_id, workspace_id, 50);
    let token = mcp_token(&state, account_id);
    for action in [Action::Read, Action::Create, Action::Update, Action::Delete] {
        grant_workspace_capability(&state, account_id, workspace_id, "CAP-EXT-002", action);
    }

    let create_input = json!({
        "workspaceId": workspace_id,
        "extensionGroupId": group_id,
        "name": "Automation",
        "extensionIds": [],
        "syncMode": "local"
    });
    let created = production_dispatcher(&state)
        .dispatch(production_request(
            "extension.group.create",
            "cor_mcp_extension_group_create",
            Some(&token),
            Some("extension-group-create-mcp-0001"),
            create_input.clone(),
        ))
        .unwrap();
    assert_eq!(created["group"]["id"], group_id);
    assert_eq!(created["revision"], 1);

    let mut dispatcher = production_dispatcher(&state);
    let replay = dispatcher
        .dispatch(production_request(
            "extension.group.create",
            "cor_mcp_extension_group_create_replay",
            Some(&token),
            Some("extension-group-create-mcp-0001"),
            create_input,
        ))
        .unwrap();
    assert_eq!(replay, created);

    let listed = dispatcher
        .dispatch(production_request(
            "extension.group.list",
            "cor_mcp_extension_group_list",
            Some(&token),
            None,
            json!({"workspaceId": workspace_id}),
        ))
        .unwrap();
    assert_eq!(listed.as_array().unwrap().len(), 1);

    let updated = dispatcher
        .dispatch(production_request(
            "extension.group.update",
            "cor_mcp_extension_group_update",
            Some(&token),
            Some("extension-group-update-mcp-0001"),
            json!({
                "workspaceId": workspace_id,
                "extensionGroupId": group_id,
                "expectedRevision": 1,
                "name": "Managed automation",
                "extensionIds": [],
                "syncMode": "managed"
            }),
        ))
        .unwrap();
    assert_eq!(updated["revision"], 2);
    assert_eq!(updated["group"]["name"], "Managed automation");

    let stale = dispatcher
        .dispatch(production_request(
            "extension.group.update",
            "cor_mcp_extension_group_stale",
            Some(&token),
            Some("extension-group-update-mcp-stale-0001"),
            json!({
                "workspaceId": workspace_id,
                "extensionGroupId": group_id,
                "expectedRevision": 1,
                "name": "Stale overwrite",
                "extensionIds": [],
                "syncMode": "managed"
            }),
        ))
        .unwrap_err();
    assert_eq!(stale.code, "revision_conflict");
    assert_eq!(stale.status, 409);

    let forbidden = dispatcher
        .dispatch(production_request(
            "extension.group.list",
            "cor_mcp_extension_group_cross_workspace",
            Some(&token),
            None,
            json!({"workspaceId": "ws_mcp_extension_group_other"}),
        ))
        .unwrap_err();
    assert_eq!(forbidden.code, "forbidden");
    assert_eq!(forbidden.status, 403);

    let deleted = dispatcher
        .dispatch(production_request(
            "extension.group.delete",
            "cor_mcp_extension_group_delete",
            Some(&token),
            Some("extension-group-delete-mcp-0001"),
            json!({
                "workspaceId": workspace_id,
                "extensionGroupId": group_id,
                "expectedRevision": 2
            }),
        ))
        .unwrap();
    assert_eq!(deleted["revision"], 2);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn production_team_commands_share_versioned_rest_record_shapes() {
    let account_id = "acct_mcp_team_records";
    let workspace_id = "ws_mcp_team_records";
    let team_id = "team_mcp_team_records";
    let state = hosted_state(account_id);
    let token = mcp_token(&state, account_id);
    for action in [Action::Read, Action::Create] {
        grant_workspace_capability(&state, account_id, workspace_id, "CAP-TEAM-001", action);
    }

    let created = production_dispatcher(&state)
        .dispatch(production_request(
            "team_create",
            "cor_team_create_mcp",
            Some(&token),
            Some("team-create-mcp-0001"),
            json!({
                "workspaceId": workspace_id,
                "team": {
                    "id": team_id,
                    "workspaceId": workspace_id,
                    "name": "Operators",
                    "memberIds": []
                }
            }),
        ))
        .unwrap();
    assert_eq!(created["team"]["id"], team_id);
    assert_eq!(created["revision"], 1);

    let fetched = production_dispatcher(&state)
        .dispatch(production_request(
            "team_get",
            "cor_team_get_mcp",
            Some(&token),
            None,
            json!({ "workspaceId": workspace_id, "teamId": team_id }),
        ))
        .unwrap();
    assert_eq!(fetched, created);

    let listed = production_dispatcher(&state)
        .dispatch(production_request(
            "team_list",
            "cor_team_list_mcp",
            Some(&token),
            None,
            json!({ "workspaceId": workspace_id }),
        ))
        .unwrap();
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert_eq!(listed[0], created);
}
