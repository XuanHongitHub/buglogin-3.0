use std::{
    cell::RefCell,
    fs,
    path::PathBuf,
    rc::Rc,
    sync::{
        Arc, Barrier,
        atomic::{AtomicU64, Ordering},
    },
};

use buglogin_api::mcp::{
    AuditOutcome, AuditRecord, ExecutionRequest, IdempotencyClaim, McpDispatcher,
    McpExecutionPolicy, McpExecutor, McpRequest,
};
use buglogin_domain::{
    account::Account,
    entities::{Profile, ProfileState, Proxy, ProxyProtocol, Runtime, SyncMode},
    errors::Problem,
    ids::{AccountId, GrantId, ProfileId, ProxyId, WorkspaceId},
    permissions::{Action, Capability, Effect, Grant, Principal, ResourceScope},
};
use buglogin_storage::control::{ControlRecordStore, MemoryControlRecordStore, PutControlRecord};
use serde_json::{Value, json};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Default)]
struct FakePolicy {
    authenticated: bool,
    authorized: bool,
    replay: Option<Value>,
    calls: Rc<RefCell<Vec<&'static str>>>,
    audits: Rc<RefCell<Vec<AuditRecord>>>,
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
        input: json!({"value": "safe"}),
    }
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
    let input = serde_json::to_value(profile("prf_mcp_replay", "Replay profile")).unwrap();
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
    create_input["proxy"]["hasCredential"] = json!(true);
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
    assert_eq!(created["record"]["proxy"]["hasCredential"], true);
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
    assert_eq!(deleted["revision"], 2);
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
