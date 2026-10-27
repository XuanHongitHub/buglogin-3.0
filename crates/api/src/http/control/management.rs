//! Account self service and bounded, resumable administrative batch execution.
//! Child commands pass through their existing REST handlers and authorization.
use super::*;
use buglogin_domain::account::canonical_username;
use buglogin_storage::control::{ControlAuditEntry, ExecuteControlCommand, HttpResponse};
use std::collections::BTreeSet;
use tower::ServiceExt;

pub(super) fn routes() -> Router<ApiState> {
    non_batch_routes().route("/v1/management/batch", post(batch))
}

fn non_batch_routes() -> Router<ApiState> {
    Router::new()
        .route("/v1/account/identity", get(own_identity))
        .route(
            "/v1/accounts/{accountId}/identity",
            get(identity).patch(update_identity),
        )
        .route("/v1/account/password", put(change_password))
        .route("/v1/accounts/{accountId}/sessions", get(sessions))
        .route(
            "/v1/accounts/{accountId}/sessions/{sessionId}",
            delete(revoke_session),
        )
        .route("/v1/management/account-drafts", post(generate_drafts))
}

pub(super) fn set_provisioned_display_name(
    identity: &mut AccountIdentity,
    puts: &mut [ControlPut],
    display_name: &str,
) -> Result<(), ResponseProblem> {
    validate_display_name(display_name)?;
    identity.display_name = display_name.trim().into();
    for put in puts
        .iter_mut()
        .filter(|put| put.record_type == "account_identity")
    {
        put.value = serde_json::to_value(&*identity).map_err(|_| {
            response_problem(Problem::new(
                "serialization_failed",
                "Serialization failed",
                500,
                "management",
            ))
        })?;
    }
    Ok(())
}

fn validate_display_name(name: &str) -> Result<(), ResponseProblem> {
    if name.trim().chars().count() > 120 || name.chars().any(char::is_control) {
        return Err(response_problem(Problem::new(
            "invalid_display_name",
            "Display name must be at most 120 characters",
            400,
            "management",
        )));
    }
    Ok(())
}

fn own_or_platform(
    state: &ApiState,
    headers: &HeaderMap,
    account_id: &AccountId,
    mutation: bool,
) -> Result<Principal, Response> {
    let principal = authenticate(state, headers)?;
    super::super::reject_api_token_principal(&principal)?;
    if mutation {
        super::super::authenticated_account_for_mutation(state, headers)?;
    }
    if principal.account_id.as_ref() == Some(account_id) {
        return Ok(principal);
    }
    let grants = grants(state)?;
    authorize_control(
        &principal,
        &grants,
        "CAP-ADM-001",
        if mutation {
            Action::Manage
        } else {
            Action::Read
        },
        &ResourceScope::global(),
        "account_forbidden",
        "Account access denied",
    )?;
    Ok(principal)
}

async fn own_identity(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Response, Response> {
    let principal = authenticate(&state, &headers)?;
    let account = principal.account_id.ok_or_else(|| {
        problem_response(
            "authentication_required",
            "Authentication required",
            401,
            correlation(&headers),
        )
    })?;
    identity(State(state), Path(account.as_str().into()), headers).await
}

async fn identity(
    State(state): State<ApiState>,
    Path(account_id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, Response> {
    let account_id = AccountId::from_str(&account_id).map_err(|_| bad_id("account_id"))?;
    own_or_platform(&state, &headers, &account_id, false)?;
    let value = state
        .identity_persistence
        .load(&account_id)
        .map_err(response_problem)?
        .ok_or_else(|| {
            problem_response(
                "account_identity_not_found",
                "Account identity was not found",
                404,
                correlation(&headers),
            )
        })?;
    Ok(super::super::sensitive_json(value))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct IdentityUpdateInput {
    username: String,
    email: String,
    display_name: String,
    expected_revision: u64,
    #[serde(default)]
    current_password: Option<String>,
}
impl Drop for IdentityUpdateInput {
    fn drop(&mut self) {
        if let Some(password) = self.current_password.as_mut() {
            password.zeroize();
        }
    }
}

async fn update_identity(
    State(state): State<ApiState>,
    Path(account_id): Path<String>,
    headers: HeaderMap,
    Json(input): Json<IdentityUpdateInput>,
) -> Result<Response, Response> {
    let account_id = AccountId::from_str(&account_id).map_err(|_| bad_id("account_id"))?;
    let principal = own_or_platform(&state, &headers, &account_id, true)?;
    let actor = principal
        .account_id
        .as_ref()
        .ok_or_else(|| bad_id("actor"))?;
    let current = state
        .identity_persistence
        .load(&account_id)
        .map_err(response_problem)?
        .ok_or_else(|| {
            problem_response(
                "account_identity_not_found",
                "Account identity was not found",
                404,
                correlation(&headers),
            )
        })?;
    // Changing verified email needs proof of control of the new address. Do not
    // silently mark a replacement email verified or disrupt current login.
    if current.email != input.email.trim() && current.verification == IdentityVerification::Verified
    {
        return Err(problem_response(
            "email_verification_required",
            "Changing a verified email requires the email verification flow",
            409,
            correlation(&headers),
        ));
    }
    if principal.account_id.as_ref() == Some(&account_id)
        && (current.username != input.username.trim() || current.email != input.email.trim())
    {
        let password = input.current_password.as_deref().ok_or_else(|| {
            problem_response(
                "current_password_required",
                "Confirm your current password before changing login details",
                400,
                correlation(&headers),
            )
        })?;
        let authenticated = state
            .credentials
            .authenticate(
                &current.username,
                password,
                super::super::request_now(&headers)?,
            )
            .map_err(response_problem)?;
        if authenticated != account_id {
            return Err(problem_response(
                "invalid_credentials",
                "Current password is invalid",
                401,
                correlation(&headers),
            ));
        }
    }
    let key = require_idempotency(&headers)?;
    let next = state
        .identity_persistence
        .update(
            &account_id,
            &input.username,
            &input.email,
            &input.display_name,
            input.expected_revision,
            actor,
            key,
            &crate::clock::rfc3339(&correlation(&headers)).map_err(response_problem)?,
        )
        .map_err(response_problem)?;
    Ok(super::super::sensitive_json(next))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PasswordChangeInput {
    current_password: String,
    new_password: String,
}
impl Drop for PasswordChangeInput {
    fn drop(&mut self) {
        self.current_password.zeroize();
        self.new_password.zeroize();
    }
}

async fn change_password(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(input): Json<PasswordChangeInput>,
) -> Result<Response, Response> {
    let account_id = super::super::authenticated_account_for_mutation(&state, &headers)?;
    let identity = state
        .identity_persistence
        .load(&account_id)
        .map_err(response_problem)?
        .ok_or_else(|| {
            problem_response(
                "account_identity_not_found",
                "Account identity was not found",
                404,
                correlation(&headers),
            )
        })?;
    let now = super::super::request_now(&headers)?;
    let verified = state
        .credentials
        .authenticate(&identity.username, &input.current_password, now)
        .map_err(response_problem)?;
    if verified != account_id {
        return Err(problem_response(
            "invalid_credentials",
            "Current password is invalid",
            401,
            correlation(&headers),
        ));
    }
    let key = require_idempotency(&headers)?;
    let password_put = state
        .credentials
        .password_reset_put(&account_id, &input.new_password, now)
        .map_err(response_problem)?;
    let mut puts = vec![password_put];
    puts.extend(
        state
            .auth
            .read()
            .map_err(|_| service_unavailable("auth"))?
            .account_session_revocation_puts(&account_id, now)
            .map_err(response_problem)?,
    );
    puts.extend(
        state
            .api_tokens
            .account_revocation_puts(&account_id, now)
            .map_err(response_problem)?,
    );
    state
        .control_persistence
        .execute(ExecuteControlCommand {
            tenant_id: "identity".into(),
            command_id: "account.password.change".into(),
            idempotency_key: format!(
                "password-change-{:x}",
                Sha256::digest(format!("{}:{key}", account_id.as_str()))
            ),
            expected_revisions: Default::default(),
            puts,
            deletes: vec![],
            audit: ControlAuditEntry {
                tenant_id: "identity".into(),
                command_id: "account.password.change".into(),
                actor_id: account_id.as_str().into(),
                allowed: true,
                detail: json!({"revokedSessions":true,"revokedApiTokens":true}),
            },
            response: HttpResponse {
                status: 200,
                headers: Default::default(),
                body: json!({"changed":true}),
            },
        })
        .map_err(response_problem)?;
    state
        .auth
        .write()
        .map_err(|_| service_unavailable("auth"))?
        .mark_account_sessions_revoked(&account_id, now);
    Ok(super::super::sensitive_json(json!({"changed":true})))
}

async fn sessions(
    State(state): State<ApiState>,
    Path(account_id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, Response> {
    let principal = authenticate(&state, &headers)?;
    let account_id = if account_id == "me" {
        principal.account_id.ok_or_else(|| bad_id("account_id"))?
    } else {
        AccountId::from_str(&account_id).map_err(|_| bad_id("account_id"))?
    };
    own_or_platform(&state, &headers, &account_id, false)?;
    let raw = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    let sessions = state
        .auth
        .read()
        .map_err(|_| service_unavailable("auth"))?
        .list_sessions(&account_id, raw)
        .map_err(response_problem)?;
    let sessions: Vec<Value> = sessions
        .into_iter()
        .map(|session| {
            let mut value = serde_json::to_value(&session).unwrap_or(Value::Null);
            if let Some(object) = value.as_object_mut() {
                object.insert("revoked".into(), json!(session.revoked_at.is_some()));
            }
            value
        })
        .collect();
    Ok(super::super::sensitive_json(json!({"sessions":sessions})))
}

async fn revoke_session(
    State(state): State<ApiState>,
    Path((account_id, session_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, Response> {
    let principal = authenticate(&state, &headers)?;
    let account_id = if account_id == "me" {
        principal.account_id.ok_or_else(|| bad_id("account_id"))?
    } else {
        AccountId::from_str(&account_id).map_err(|_| bad_id("account_id"))?
    };
    own_or_platform(&state, &headers, &account_id, true)?;
    let now = super::super::request_now(&headers)?;
    let session = state
        .auth
        .read()
        .map_err(|_| service_unavailable("auth"))?
        .list_sessions(&account_id, None)
        .map_err(response_problem)?
        .into_iter()
        .find(|session| session.id == session_id)
        .ok_or_else(|| {
            problem_response(
                "session_not_found",
                "Session was not found",
                404,
                correlation(&headers),
            )
        })?;
    if let Some(device_id) = session.device_id {
        for (workspace, resource) in
            encrypted_profiles_with_device_envelopes(&state, &account_id, &device_id)
                .map_err(Response::from)?
        {
            state
                .mark_resource_key_rotation_pending(&workspace, &resource)
                .map_err(response_problem)?;
        }
    }
    state
        .auth
        .write()
        .map_err(|_| service_unavailable("auth"))?
        .revoke_session_with_idempotency(
            &account_id,
            &session_id,
            require_idempotency(&headers)?,
            now,
        )
        .map_err(response_problem)?;
    Ok(super::super::sensitive_json(json!({"revoked":true})))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DraftInput {
    count: usize,
    prefix: String,
    start: u32,
    display_name_template: String,
}

async fn generate_drafts(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(input): Json<DraftInput>,
) -> Result<Response, Response> {
    let principal = authenticate(&state, &headers)?;
    super::super::reject_api_token_principal(&principal)?;
    if !(1..=100).contains(&input.count)
        || input.prefix.len() > 48
        || input.display_name_template.chars().count() > 100
    {
        return Err(problem_response(
            "invalid_account_drafts",
            "Generate between 1 and 100 accounts with bounded names",
            400,
            correlation(&headers),
        ));
    }
    let mut items = Vec::with_capacity(input.count);
    for index in 0..input.count {
        let number = input
            .start
            .checked_add(index as u32)
            .ok_or_else(|| bad_id("start"))?;
        let username = format!("{}{:03}", input.prefix, number);
        canonical_username(&username).map_err(response_problem)?;
        let mut random = [0u8; 32];
        getrandom::fill(&mut random).map_err(|_| {
            problem_response(
                "secure_random_unavailable",
                "Secure account generation is unavailable",
                503,
                correlation(&headers),
            )
        })?;
        let password = format!(
            "Aa1!{}",
            random[..12]
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        );
        let id = format!(
            "acct_{}",
            random[12..28]
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        );
        random.zeroize();
        let display_name = input
            .display_name_template
            .replace("{n}", &number.to_string());
        items.push(json!({"accountId":id,"username":username,"displayName":display_name,"initialPassword":password}));
    }
    Ok(super::super::sensitive_json(json!({"items":items})))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BatchInput {
    items: Vec<BatchItem>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BatchItem {
    key: String,
    command_id: String,
    input: Value,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BatchStored {
    input_hash: String,
    result: Value,
}

async fn batch(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(input): Json<BatchInput>,
) -> Result<Response, Response> {
    let principal = authenticate(&state, &headers)?;
    let actor = principal
        .account_id
        .as_ref()
        .ok_or_else(|| bad_id("actor"))?;
    let batch_key = require_idempotency(&headers)?;
    if input.items.is_empty() || input.items.len() > 100 {
        return Err(problem_response(
            "invalid_batch_size",
            "A batch must contain 1–100 items",
            400,
            correlation(&headers),
        ));
    }
    let mut seen = BTreeSet::new();
    // Validate the entire dispatch allowlist before executing the first item.
    for item in &input.items {
        if item.key.is_empty() || item.key.len() > 128 || !seen.insert(item.key.clone()) {
            return Err(problem_response(
                "invalid_batch_key",
                "Batch item keys must be unique and at most 128 characters",
                400,
                correlation(&headers),
            ));
        }
        batch_request(item)?;
    }
    let mut results = Vec::new();
    let mut all_replayed = true;
    for item in input.items {
        // Fresh authentication is performed by each inner handler. Never trust the
        // account or capability snapshot from the initial batch request.
        let digest = format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(&json!({"commandId":item.command_id,"input":item.input}))
                    .map_err(internal)?
            )
        );
        let record_id = format!(
            "{:x}",
            Sha256::digest(format!("{}:{batch_key}:{}", actor.as_str(), item.key))
        );
        let stored = state
            .control_persistence
            .load_versioned::<BatchStored>("management", "batch_item", &record_id)
            .map_err(response_problem)?;
        if let Some((stored, _)) = &stored {
            if stored.input_hash != digest {
                return Err(problem_response(
                    "idempotency_conflict",
                    "Batch key was reused with different input",
                    409,
                    correlation(&headers),
                ));
            }
        }
        all_replayed &= stored.as_ref().is_some_and(|(stored, _)| {
            stored.result.get("status").and_then(Value::as_str) == Some("succeeded")
        });
        let (method, path, body) = batch_request(&item)?;
        let mut request = axum::http::Request::builder().method(method).uri(path);
        *request.headers_mut().ok_or_else(|| bad_id("headers"))? = headers.clone();
        let child_key = format!("batch-{record_id}");
        let request_headers = request.headers_mut().ok_or_else(|| bad_id("headers"))?;
        request_headers.remove("content-length");
        request_headers.insert(
            "content-type",
            "application/json"
                .parse()
                .map_err(|_| bad_id("content-type"))?,
        );
        request_headers.insert(
            "idempotency-key",
            child_key.parse().map_err(|_| bad_id("idempotency-key"))?,
        );
        let request = request
            .body(Body::from(serde_json::to_vec(&body).map_err(internal)?))
            .map_err(|_| bad_id("request"))?;
        let response = super::routes_core()
            .merge(non_batch_routes())
            .with_state(state.clone())
            .oneshot(request)
            .await
            .map_err(|_| service_unavailable("batch"))?;
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .map_err(|_| service_unavailable("batch"))?;
        let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        let result = if status.is_success() {
            json!({"key":item.key,"status":"succeeded","data":body})
        } else {
            json!({"key":item.key,"status":"failed","error":{"code":body.get("code").and_then(Value::as_str).unwrap_or("batch_item_failed"),"title":body.get("title").and_then(Value::as_str).unwrap_or("Operation failed"),"status":status.as_u16()}})
        };
        let revision = stored.map(|(_, revision)| revision).unwrap_or(0);
        let receipt = BatchStored {
            input_hash: digest,
            result: result.clone(),
        };
        state
            .control_persistence
            .execute(ExecuteControlCommand {
                tenant_id: "management".into(),
                command_id: "management.batch.item".into(),
                idempotency_key: format!("batch-receipt-{record_id}-{revision}"),
                expected_revisions: Default::default(),
                puts: vec![ControlPut {
                    tenant_id: "management".into(),
                    record_type: "batch_item".into(),
                    record_id: record_id.clone(),
                    expected_revision: Some(revision),
                    value: serde_json::to_value(receipt).map_err(internal)?,
                }],
                deletes: vec![],
                audit: ControlAuditEntry {
                    tenant_id: "management".into(),
                    command_id: item.command_id.clone(),
                    actor_id: actor.as_str().into(),
                    allowed: status.is_success(),
                    detail: json!({"batchItem":record_id,"status":status.as_u16()}),
                },
                response: HttpResponse {
                    status: 200,
                    headers: Default::default(),
                    body: result.clone(),
                },
            })
            .map_err(response_problem)?;
        results.push(result);
    }
    Ok(super::super::sensitive_json(
        json!({"items":results,"replayed":all_replayed}),
    ))
}

fn arg(input: &Value, key: &str) -> Result<String, Response> {
    input
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 256
                && value.chars().all(|character| {
                    character.is_ascii_alphanumeric() || matches!(character, '_' | '-')
                })
        })
        .map(str::to_owned)
        .ok_or_else(|| bad_id(key))
}
fn batch_request(item: &BatchItem) -> Result<(Method, String, Value), Response> {
    let input = &item.input;
    let mut body = input.clone();
    let route = match item.command_id.as_str() {
        "organization.create" => (Method::POST, "/v1/organizations".into()),
        "organization.update" => {
            body = input
                .get("update")
                .cloned()
                .ok_or_else(|| bad_id("update"))?;
            (
                Method::PATCH,
                format!("/v1/organizations/{}", arg(input, "organizationId")?),
            )
        }
        "workspace.create" => {
            body = input
                .get("workspace")
                .cloned()
                .ok_or_else(|| bad_id("workspace"))?;
            (
                Method::POST,
                format!(
                    "/v1/organizations/{}/workspaces",
                    arg(input, "organizationId")?
                ),
            )
        }
        "workspace.update" => {
            body = input
                .get("update")
                .cloned()
                .ok_or_else(|| bad_id("update"))?;
            (
                Method::PATCH,
                format!(
                    "/v1/organizations/{}/workspaces/{}",
                    arg(input, "organizationId")?,
                    arg(input, "workspaceId")?
                ),
            )
        }
        "account.create.scoped" => {
            let workspace = arg(input, "workspaceId")?;
            if let Some(object) = body.as_object_mut() {
                object.remove("workspaceId");
            }
            (Method::POST, format!("/v1/workspaces/{workspace}/accounts"))
        }
        "organization.account.create" => (
            Method::POST,
            format!(
                "/v1/organizations/{}/accounts",
                arg(input, "organizationId")?
            ),
        ),
        "organization.member.workspace.assign" => (
            Method::POST,
            format!(
                "/v1/organizations/{}/workspaces/{}/members",
                arg(input, "organizationId")?,
                arg(input, "workspaceId")?
            ),
        ),
        "organization.member.status.update" => (
            Method::PATCH,
            format!(
                "/v1/organizations/{}/members/{}",
                arg(input, "organizationId")?,
                arg(input, "accountId")?
            ),
        ),
        "account.identity.update" => {
            let account = arg(input, "accountId")?;
            if let Some(object) = body.as_object_mut() {
                object.remove("accountId");
            }
            (Method::PATCH, format!("/v1/accounts/{account}/identity"))
        }
        "account.status.update" => {
            let account = arg(input, "accountId")?;
            if let Some(object) = body.as_object_mut() {
                object.remove("accountId");
            }
            (Method::PUT, format!("/v1/accounts/{account}/status"))
        }
        "member.role.update" => {
            body = json!({"role":input.get("role").ok_or_else(||bad_id("role"))?});
            (
                Method::PUT,
                format!(
                    "/v1/workspaces/{}/members/{}/role",
                    arg(input, "workspaceId")?,
                    arg(input, "memberId")?
                ),
            )
        }
        "member.activate" => (
            Method::PUT,
            format!(
                "/v1/workspaces/{}/members/{}/activate",
                arg(input, "workspaceId")?,
                arg(input, "memberId")?
            ),
        ),
        "member.suspend" => (
            Method::PUT,
            format!(
                "/v1/workspaces/{}/members/{}/suspend",
                arg(input, "workspaceId")?,
                arg(input, "memberId")?
            ),
        ),
        "member.remove" => (
            Method::DELETE,
            format!(
                "/v1/workspaces/{}/members/{}",
                arg(input, "workspaceId")?,
                arg(input, "memberId")?
            ),
        ),
        "member.invitation.create" => {
            let workspace = arg(input, "workspaceId")?;
            if let Some(object) = body.as_object_mut() {
                object.remove("workspaceId");
            }
            (
                Method::POST,
                format!("/v1/workspaces/{workspace}/invitations"),
            )
        }
        "group.create" => (
            Method::POST,
            format!("/v1/workspaces/{}/groups", arg(input, "workspaceId")?),
        ),
        "group.update" => (
            Method::PUT,
            format!(
                "/v1/workspaces/{}/groups/{}",
                arg(input, "workspaceId")?,
                arg(input, "groupId")?
            ),
        ),
        "group.delete" => (
            Method::DELETE,
            format!(
                "/v1/workspaces/{}/groups/{}",
                arg(input, "workspaceId")?,
                arg(input, "groupId")?
            ),
        ),
        "group.share.create" => (
            Method::POST,
            format!(
                "/v1/workspaces/{}/groups/{}/shares",
                arg(input, "workspaceId")?,
                arg(input, "groupId")?
            ),
        ),
        "group.share.revoke" => (
            Method::DELETE,
            format!(
                "/v1/workspaces/{}/groups/{}/shares/{}",
                arg(input, "workspaceId")?,
                arg(input, "groupId")?,
                arg(input, "grantId")?
            ),
        ),
        "profile.groups.replace" => (
            Method::PUT,
            format!("/v1/profiles/{}/groups", arg(input, "profileId")?),
        ),
        "profile.update" => (
            Method::PUT,
            format!(
                "/v1/workspaces/{}/profiles/{}",
                arg(input, "workspaceId")?,
                arg(input, "profileId")?
            ),
        ),
        "resource.grant" => (
            Method::POST,
            format!("/v1/workspaces/{}/grants", arg(input, "workspaceId")?),
        ),
        "resource.grant.revoke" => (
            Method::DELETE,
            format!("/v1/resource-grants/{}", arg(input, "grantId")?),
        ),
        _ => {
            return Err(problem_response(
                "batch_command_forbidden",
                "This command is not allowed in a management batch",
                400,
                "management".into(),
            ));
        }
    };
    Ok((route.0, route.1, body))
}

#[cfg(test)]
mod batch_route_tests {
    use super::*;

    #[test]
    fn organization_member_status_batch_uses_scoped_status_route() {
        let item = BatchItem {
            key: "account_123".into(),
            command_id: "organization.member.status.update".into(),
            input: json!({
                "organizationId": "org_123",
                "accountId": "account_123",
                "status": "suspended",
                "expectedRevision": 7
            }),
        };

        let (method, path, body) = batch_request(&item).expect("route maps");

        assert_eq!(method, Method::PATCH);
        assert_eq!(path, "/v1/organizations/org_123/members/account_123");
        assert_eq!(body, item.input);
    }
}
