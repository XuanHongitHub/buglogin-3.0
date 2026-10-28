use std::collections::BTreeMap;

use axum::{Json, extract::State, http::HeaderMap, response::Response};
use buglogin_domain::{
    account::{
        Account, AccountIdentity, IdentityVerification, canonical_email, canonical_username,
    },
    errors::Problem,
    ids::{AccountId, DeviceId, MemberId},
    member::{Member, MemberRole, MemberStatus},
    permissions::{Principal, Role},
};
use buglogin_storage::control::{
    ControlAuditEntry, ControlPut, ControlRecordKey, ExecuteControlCommand, HttpResponse,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, Zeroizing};

use super::{ApiState, correlation, problem, request_now, response, sensitive_json};
use crate::auth::AuthService;
use crate::services::buglab_sso::{BugLabSsoClient, BugLabSsoConfig, BugLabUser};

const SSO_TENANT: &str = "identity";
const SSO_TRANSACTION: &str = "buglab_sso_transaction";
const SSO_IDENTITY: &str = "buglab_sso_identity";
const SSO_TRANSACTION_SECONDS: u64 = 5 * 60;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct StartInput {
    device_id: String,
}

impl Drop for StartInput {
    fn drop(&mut self) {
        self.device_id.zeroize();
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct CompleteInput {
    state: String,
    token: String,
}

impl Drop for CompleteInput {
    fn drop(&mut self) {
        self.state.zeroize();
        self.token.zeroize();
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StartResponse {
    authorization_url: String,
    expires_in: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CompleteResponse {
    access_token: String,
    refresh_token: String,
    token_type: &'static str,
    expires_at: u64,
}

impl Drop for CompleteResponse {
    fn drop(&mut self) {
        self.access_token.zeroize();
        self.refresh_token.zeroize();
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SsoTransaction {
    state_hash: String,
    device_id: String,
    status: SsoTransactionStatus,
    expires_at: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum SsoTransactionStatus {
    Started,
    Exchanging,
    Complete,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LinkedBugLabIdentity {
    account_id: AccountId,
    subject_hash: String,
    email: String,
}

pub(super) async fn start(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(mut input): Json<StartInput>,
) -> Result<Response, Response> {
    let _correlation_id = correlation(&headers)?;
    let config = configured_provider()?;
    let device_id = DeviceId::new(input.device_id.clone()).map_err(|_| {
        response(problem(
            "invalid_device_id",
            "Desktop device identity is invalid",
            400,
        ))
    })?;
    input.device_id.zeroize();
    let current = request_now(&headers)?;
    let expires_at = current
        .checked_add(SSO_TRANSACTION_SECONDS)
        .ok_or_else(|| {
            response(problem(
                "sso_expiry_invalid",
                "Sign-in could not start",
                500,
            ))
        })?;
    let mut raw_state = random_state().map_err(response)?;
    let state_hash = hash(&raw_state);
    let transaction = SsoTransaction {
        state_hash: state_hash.clone(),
        device_id: device_id.as_str().to_owned(),
        status: SsoTransactionStatus::Started,
        expires_at,
    };
    let value = serde_json::to_value(&transaction).map_err(|_| {
        response(problem(
            "sso_transaction_invalid",
            "Sign-in could not start",
            500,
        ))
    })?;
    state
        .control_persistence
        .execute(ExecuteControlCommand {
            tenant_id: SSO_TENANT.into(),
            command_id: "identity.buglab_sso.start".into(),
            idempotency_key: format!("buglab-sso-start-{state_hash}"),
            expected_revisions: BTreeMap::new(),
            puts: vec![ControlPut {
                tenant_id: SSO_TENANT.into(),
                record_type: SSO_TRANSACTION.into(),
                record_id: state_hash.clone(),
                expected_revision: Some(0),
                value,
            }],
            deletes: vec![],
            audit: ControlAuditEntry {
                tenant_id: SSO_TENANT.into(),
                command_id: "identity.buglab_sso.start".into(),
                actor_id: "anonymous_sso_start".into(),
                allowed: true,
                detail: json!({"provider": "buglab_id", "deviceId": device_id.as_str()}),
            },
            response: HttpResponse {
                status: 201,
                headers: Default::default(),
                body: json!({"stateHash": state_hash}),
            },
        })
        .map_err(response)?;
    let mut authorization_url = config.authorization_url(&raw_state).map_err(response)?;
    raw_state.zeroize();
    // Keep callback payloads out of browser referrers. The callback page is a native deep link.
    authorization_url.set_fragment(None);
    Ok(super::no_store(sensitive_json(StartResponse {
        authorization_url: authorization_url.to_string(),
        expires_in: SSO_TRANSACTION_SECONDS,
    })))
}

pub(super) async fn complete(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(mut input): Json<CompleteInput>,
) -> Result<Response, Response> {
    let correlation_id = correlation(&headers)?;
    let config = configured_provider()?;
    if !valid_opaque(&input.state) || !valid_provider_token(&input.token) {
        return Err(response(problem(
            "invalid_sso_callback",
            "BugLab ID sign-in response is invalid",
            400,
        )));
    }
    let raw_state = Zeroizing::new(std::mem::take(&mut input.state));
    let token = Zeroizing::new(std::mem::take(&mut input.token));
    let state_hash = hash(raw_state.as_str());
    let current = request_now(&headers)?;
    let (transaction, revision) = state
        .control_persistence
        .load_versioned::<SsoTransaction>(SSO_TENANT, SSO_TRANSACTION, &state_hash)
        .map_err(response)?
        .ok_or_else(invalid_callback)?;
    if transaction.state_hash != state_hash
        || transaction.status != SsoTransactionStatus::Started
        || transaction.expires_at <= current
    {
        return Err(invalid_callback());
    }
    let mut exchanging = transaction.clone();
    exchanging.status = SsoTransactionStatus::Exchanging;
    transition_transaction(
        &state,
        &state_hash,
        revision,
        exchanging,
        "exchange-started",
    )
    .map_err(response)?;

    let provider = BugLabSsoClient::new().map_err(response)?;
    let user = provider
        .exchange(token.as_str(), &config.app_code)
        .await
        .map_err(response)?;
    let account_id =
        resolve_or_create_identity(&state, &config.app_code, &user).map_err(response)?;
    let (_, account_revision) = state
        .account_persistence
        .load_active_versioned(&account_id)
        .map_err(response)?
        .ok_or_else(invalid_callback)?;
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
    let mut tokens = issue_sso_session(
        &mut auth,
        account_id.clone(),
        &transaction,
        Some(account_revision),
        current,
        expires_at,
        refresh_expires_at,
    )
    .map_err(response)?;
    drop(auth);
    mark_transaction_complete(&state, &state_hash).map_err(response)?;
    Ok(super::no_store(sensitive_json(CompleteResponse {
        access_token: std::mem::take(&mut tokens.access_token),
        refresh_token: std::mem::take(&mut tokens.refresh_token),
        token_type: "Bearer",
        expires_at,
    })))
}

fn issue_sso_session(
    auth: &mut AuthService,
    account_id: AccountId,
    transaction: &SsoTransaction,
    account_revision: Option<u64>,
    current: u64,
    expires_at: u64,
    refresh_expires_at: u64,
) -> Result<crate::auth::DeviceSessionTokens, Problem> {
    auth.issue_device_session_with_account_revision_at(
        account_id,
        expires_at,
        refresh_expires_at,
        transaction.device_id.clone(),
        None,
        account_revision,
        current,
    )
}

fn resolve_or_create_identity(
    state: &ApiState,
    app_code: &str,
    user: &BugLabUser,
) -> Result<AccountId, buglogin_domain::errors::Problem> {
    let email = canonical_email(&user.email)?;
    let subject_hash = hash_parts(&["buglab-id/v1", &user.user_id]);
    let Some((linked, _)) = state
        .control_persistence
        .load_versioned::<LinkedBugLabIdentity>(SSO_TENANT, SSO_IDENTITY, &subject_hash)?
    else {
        return create_or_link_identity(state, app_code, user, &email, &subject_hash);
    };
    // BugLab's stable subject is the account-link authority. Email is mutable
    // and can change at the provider; do not strand an already-linked account
    // or silently remap it to a different BugLogin identity by email.
    if !linked_identity_matches_subject(&linked, &subject_hash) {
        return Err(Problem::new(
            "sso_identity_changed",
            "BugLab ID identity needs administrator review",
            409,
            "identity",
        ));
    }
    let Some((account, _)) = state
        .account_persistence
        .load_active_versioned(&linked.account_id)?
    else {
        return Err(Problem::new(
            "account_inactive",
            "BugLogin account is not active",
            403,
            "identity",
        ));
    };
    if account.id != linked.account_id {
        return Err(sso_mapping_invalid());
    }
    Ok(linked.account_id)
}

fn create_or_link_identity(
    state: &ApiState,
    app_code: &str,
    user: &BugLabUser,
    email: &str,
    subject_hash: &str,
) -> Result<AccountId, buglogin_domain::errors::Problem> {
    let by_email = state.identity_persistence.find_by_email(email)?;
    let by_legacy = if user.app == app_code {
        legacy_username(user.old_account.as_deref())
            .map(|username| state.identity_persistence.find_by_username(&username))
            .transpose()?
            .flatten()
    } else {
        None
    };
    // BugLab attests the canonical email. The legacy username may be stale or
    // reused; only link it when the attested email independently confirms it.
    let existing = prefer_buglab_attested_email_identity(by_email, by_legacy, email);
    let account_id = if let Some(identity) = existing {
        if identity.canonical_email != email {
            return Err(ambiguous_identity());
        }
        state
            .account_persistence
            .load_active_versioned(&identity.account_id)?
            .ok_or_else(|| {
                Problem::new(
                    "account_inactive",
                    "BugLogin account is not active",
                    403,
                    "identity",
                )
            })?;
        identity.account_id
    } else {
        return provision_buglab_identity_in_default_workspace(state, user, email, subject_hash);
    };

    let linked = linked_identity(account_id.clone(), subject_hash, email);
    let revision = state
        .control_persistence
        .revision(SSO_TENANT, SSO_IDENTITY, subject_hash)?
        .unwrap_or(0);
    if revision == 0 {
        let mut puts = vec![link_record(subject_hash, &linked)?];
        let mut expected_revisions = BTreeMap::new();
        expected_revisions.insert(
            ControlRecordKey {
                tenant_id: SSO_TENANT.into(),
                record_type: SSO_IDENTITY.into(),
                record_id: subject_hash.into(),
            },
            0,
        );
        let (identity, identity_revision) = state
            .identity_persistence
            .load_versioned(&account_id)?
            .ok_or_else(ambiguous_identity)?;
        if identity.canonical_email != email {
            return Err(ambiguous_identity());
        }
        expected_revisions.insert(
            ControlRecordKey {
                tenant_id: "identity".into(),
                record_type: "account_identity".into(),
                record_id: account_id.as_str().into(),
            },
            identity_revision,
        );
        if identity.verification != IdentityVerification::Verified {
            let verified = verify_buglab_attested_identity(identity, email)?;
            puts.push(ControlPut {
                tenant_id: "identity".into(),
                record_type: "account_identity".into(),
                record_id: account_id.as_str().into(),
                expected_revision: Some(identity_revision),
                value: serde_json::to_value(&verified).map_err(|_| sso_mapping_invalid())?,
            });
        }
        state.control_persistence.execute(ExecuteControlCommand {
            tenant_id: SSO_TENANT.into(),
            command_id: "identity.buglab_sso.link".into(),
            idempotency_key: format!("buglab-sso-link-{subject_hash}"),
            expected_revisions,
            puts,
            deletes: vec![],
            audit: ControlAuditEntry {
                tenant_id: SSO_TENANT.into(),
                command_id: "identity.buglab_sso.link".into(),
                actor_id: "buglab_id_provider".into(),
                allowed: true,
                detail: json!({
                    "providerSubjectHash": subject_hash,
                    "tenantAccessGranted": false,
                }),
            },
            response: HttpResponse {
                status: 200,
                headers: Default::default(),
                body: json!({"accountId": account_id}),
            },
        })?;
    }
    Ok(account_id)
}

fn prefer_buglab_attested_email_identity(
    by_email: Option<AccountIdentity>,
    by_legacy: Option<AccountIdentity>,
    attested_email: &str,
) -> Option<AccountIdentity> {
    by_email.or_else(|| by_legacy.filter(|identity| identity.canonical_email == attested_email))
}

fn provision_buglab_identity_in_default_workspace(
    state: &ApiState,
    user: &BugLabUser,
    email: &str,
    subject_hash: &str,
) -> Result<AccountId, Problem> {
    let _mutation = state.control_mutation.lock().map_err(|_| {
        Problem::new(
            "control_unavailable",
            "Control state unavailable",
            503,
            "identity",
        )
    })?;
    state.refresh_control_aggregate_locked()?;
    let mut aggregate = state.control_aggregate()?;
    let platform = Principal {
        roles: std::collections::BTreeSet::from([Role::PlatformAdmin]),
        ..Principal::default()
    };
    let organizations = aggregate.workspaces.list_organizations(&platform, &[])?;
    let organization = organizations
        .into_iter()
        .find(|organization| organization.name.trim().eq_ignore_ascii_case("BugMedia"))
        .ok_or_else(|| {
            Problem::new(
                "sso_default_organization_missing",
                "BugMedia organization is not configured for BugLab sign-in",
                503,
                "identity",
            )
        })?;
    let workspaces = aggregate
        .workspaces
        .list_workspaces(&platform, &[], &organization.id)?;
    let workspace = select_default_workspace(&workspaces)
        .cloned()
        .ok_or_else(|| {
            Problem::new(
                "sso_default_workspace_missing",
                "BugMedia has no common workspace for BugLab sign-in",
                503,
                "identity",
            )
        })?;

    let account_id = AccountId::new(format!("acct_bl_{}", &subject_hash[..24]))
        .map_err(|_| sso_mapping_invalid())?;
    let preferred_username = user
        .old_account
        .as_deref()
        .and_then(|candidate| canonical_username(candidate).ok())
        .or_else(|| {
            email
                .split('@')
                .next()
                .and_then(|candidate| canonical_username(candidate).ok())
        });
    let username = match preferred_username {
        Some(candidate)
            if state
                .identity_persistence
                .find_by_username(&candidate)?
                .is_none() =>
        {
            candidate
        }
        _ => format!("bl{}", &subject_hash[..24]),
    };
    let now = crate::clock::rfc3339("buglab-sso-provision")?;
    let now_unix = crate::clock::unix_time_seconds("buglab-sso-provision")?;
    let mut identity = AccountIdentity::new(account_id.clone(), &username, email, now.clone())?
        .verify(1, now.clone())?;
    if let Some(name) = user.name.as_deref() {
        if name.chars().count() > 120 || name.chars().any(char::is_control) {
            return Err(sso_mapping_invalid());
        }
        identity.display_name = name.trim().to_owned();
    }
    // BugLab ID attests the identity. New SSO identities join only the shared
    // BugMedia workspace, with the ordinary Member role and no elevated grants.
    let mut puts = state.identity_persistence.prepare_create_puts(&identity)?;
    puts.push(ControlPut {
        tenant_id: "control".into(),
        record_type: "account".into(),
        record_id: account_id.as_str().into(),
        expected_revision: Some(0),
        value: serde_json::to_value(Account::new(account_id.clone(), now.clone()))
            .map_err(|_| sso_mapping_invalid())?,
    });

    let member_id = MemberId::new(format!("mem_bl_{}", &subject_hash[..24]))
        .map_err(|_| sso_mapping_invalid())?;
    let invite_expires_at = now_unix.checked_add(600).ok_or_else(sso_mapping_invalid)?;
    let membership = Member {
        id: member_id.clone(),
        account_id: account_id.clone(),
        workspace_id: workspace.id.clone(),
        team_id: None,
        role: MemberRole::Member,
        status: MemberStatus::Invited,
        invite_expires_at: Some(invite_expires_at),
    };
    aggregate
        .members
        .invite(&platform, &[], membership, now_unix)?;
    aggregate
        .members
        .activate(&platform, &[], &workspace.id, &member_id, now_unix)?;
    aggregate
        .organization_members
        .create_active(organization.id.clone(), account_id.clone())?;

    let identity_revision = state
        .control_persistence
        .revision("identity", "buglab_sso_identity", subject_hash)?
        .unwrap_or(0);
    if identity_revision != 0 {
        return Err(sso_mapping_invalid());
    }
    let linked = linked_identity(account_id.clone(), subject_hash, email);
    puts.push(link_record(subject_hash, &linked)?);
    state.replace_control_aggregate(aggregate)?;
    let result = state.persist_control_aggregate_with_puts(
        "identity.buglab_sso.provision",
        format!("buglab-sso-provision-{subject_hash}"),
        "buglab_id_provider".into(),
        true,
        &hash_parts(&["buglab-sso-provision/v1", subject_hash]),
        json!({"providerSubjectHash": subject_hash, "organizationId": organization.id, "workspaceId": workspace.id, "role": "member"}),
        201,
        json!({"accountId": account_id}),
        puts,
    );
    if let Err(error) = result {
        let _ = state.reload_control_aggregate();
        return Err(error);
    }
    state.reload_control_aggregate()?;
    Ok(account_id)
}

fn select_default_workspace(
    workspaces: &[buglogin_domain::workspace::Workspace],
) -> Option<&buglogin_domain::workspace::Workspace> {
    let candidates = workspaces
        .iter()
        .filter(|workspace| {
            let name = workspace.name.trim().to_ascii_lowercase();
            [
                "common",
                "shared",
                "general",
                "default",
                "chung",
                "main workspace",
            ]
            .iter()
            .any(|candidate| name.contains(candidate))
        })
        .map(|workspace| workspace.id.clone())
        .collect::<Vec<_>>();
    match candidates.as_slice() {
        [id] => workspaces.iter().find(|workspace| &workspace.id == id),
        [] if workspaces.len() == 1 => workspaces.first(),
        _ => None,
    }
}

// BugLab's legacy account field is optional compatibility data, not an
// authority for the verified SSO identity. Older provider records can contain
// values that no longer satisfy BugLogin's username rules; they must not make
// a valid email/subject sign-in fail with `invalid_username`.
fn legacy_username(value: Option<&str>) -> Option<String> {
    value.and_then(|value| canonical_username(value).ok())
}

fn linked_identity(account_id: AccountId, subject_hash: &str, email: &str) -> LinkedBugLabIdentity {
    LinkedBugLabIdentity {
        account_id,
        subject_hash: subject_hash.into(),
        email: email.into(),
    }
}

fn linked_identity_matches_subject(linked: &LinkedBugLabIdentity, subject_hash: &str) -> bool {
    linked.subject_hash == subject_hash
}

fn verify_buglab_attested_identity(
    identity: AccountIdentity,
    provider_email: &str,
) -> Result<AccountIdentity, Problem> {
    if identity.canonical_email != provider_email {
        return Err(ambiguous_identity());
    }
    identity.verify(
        identity.revision,
        crate::clock::rfc3339("buglab-sso-verify").map_err(|_| sso_mapping_invalid())?,
    )
}

fn link_record(
    subject_hash: &str,
    linked: &LinkedBugLabIdentity,
) -> Result<ControlPut, buglogin_domain::errors::Problem> {
    Ok(ControlPut {
        tenant_id: SSO_TENANT.into(),
        record_type: SSO_IDENTITY.into(),
        record_id: subject_hash.into(),
        expected_revision: Some(0),
        value: serde_json::to_value(linked).map_err(|_| sso_mapping_invalid())?,
    })
}

fn transition_transaction(
    state: &ApiState,
    state_hash: &str,
    revision: u64,
    transaction: SsoTransaction,
    operation: &str,
) -> Result<(), buglogin_domain::errors::Problem> {
    let command_id = format!("identity.buglab_sso.{operation}");
    state.control_persistence.execute(ExecuteControlCommand {
        tenant_id: SSO_TENANT.into(),
        command_id: command_id.clone(),
        idempotency_key: format!("buglab-sso-{operation}-{state_hash}"),
        expected_revisions: BTreeMap::from([(
            ControlRecordKey {
                tenant_id: SSO_TENANT.into(),
                record_type: SSO_TRANSACTION.into(),
                record_id: state_hash.into(),
            },
            revision,
        )]),
        puts: vec![ControlPut {
            tenant_id: SSO_TENANT.into(),
            record_type: SSO_TRANSACTION.into(),
            record_id: state_hash.into(),
            expected_revision: Some(revision),
            value: serde_json::to_value(transaction).map_err(|_| sso_mapping_invalid())?,
        }],
        deletes: vec![],
        audit: ControlAuditEntry {
            tenant_id: SSO_TENANT.into(),
            command_id,
            actor_id: "anonymous_sso_callback".into(),
            allowed: true,
            detail: json!({"stateHash": state_hash, "operation": operation}),
        },
        response: HttpResponse {
            status: 200,
            headers: Default::default(),
            body: json!({"accepted": true}),
        },
    })?;
    Ok(())
}

fn mark_transaction_complete(
    state: &ApiState,
    state_hash: &str,
) -> Result<(), buglogin_domain::errors::Problem> {
    let (mut transaction, revision) = state
        .control_persistence
        .load_versioned::<SsoTransaction>(SSO_TENANT, SSO_TRANSACTION, state_hash)?
        .ok_or_else(invalid_callback_problem)?;
    if transaction.status != SsoTransactionStatus::Exchanging {
        return Err(invalid_callback_problem());
    }
    transaction.status = SsoTransactionStatus::Complete;
    transition_transaction(
        state,
        state_hash,
        revision,
        transaction,
        "exchange-complete",
    )
}

fn configured_provider() -> Result<BugLabSsoConfig, Response> {
    BugLabSsoConfig::from_env()
        .map_err(response)?
        .ok_or_else(|| {
            response(problem(
                "identity_provider_not_configured",
                "BugLab ID sign-in is not enabled on this server",
                503,
            ))
        })
}

fn random_state() -> Result<String, buglogin_domain::errors::Problem> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| {
        problem(
            "sso_randomness_unavailable",
            "BugLab ID sign-in is temporarily unavailable",
            503,
        )
    })?;
    Ok(hex(&bytes))
}

fn valid_opaque(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn valid_provider_token(value: &str) -> bool {
    (16..=4096).contains(&value.len())
        && value.is_ascii()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

fn hash(value: &str) -> String {
    hex(Sha256::digest(value.as_bytes()).as_slice())
}

fn hash_parts(parts: &[&str]) -> String {
    let mut digest = Sha256::new();
    for part in parts {
        digest.update((part.len() as u64).to_be_bytes());
        digest.update(part.as_bytes());
    }
    hex(digest.finalize().as_slice())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn invalid_callback() -> Response {
    response(invalid_callback_problem())
}

fn invalid_callback_problem() -> buglogin_domain::errors::Problem {
    problem(
        "invalid_sso_callback",
        "BugLab ID sign-in response is expired or already used",
        401,
    )
}

fn ambiguous_identity() -> buglogin_domain::errors::Problem {
    problem(
        "sso_identity_mapping_ambiguous",
        "BugLab ID identity needs administrator review",
        409,
    )
}

fn sso_mapping_invalid() -> buglogin_domain::errors::Problem {
    problem(
        "sso_identity_mapping_invalid",
        "BugLab ID account mapping is invalid",
        409,
    )
}

#[cfg(test)]
mod tests {
    use buglogin_domain::ids::AccountId;
    use buglogin_storage::control::MemoryControlRecordStore;

    use super::{
        IdentityVerification, LinkedBugLabIdentity, SsoTransaction, SsoTransactionStatus,
        issue_sso_session, legacy_username, linked_identity_matches_subject,
        prefer_buglab_attested_email_identity, select_default_workspace,
        verify_buglab_attested_identity,
    };
    use crate::{
        auth::AuthService,
        services::{
            control_persistence::ControlPersistence, session_persistence::SessionPersistence,
        },
    };

    fn transaction(device_id: &str) -> SsoTransaction {
        SsoTransaction {
            state_hash: "test-state".into(),
            device_id: device_id.into(),
            status: SsoTransactionStatus::Exchanging,
            expires_at: 10_000,
        }
    }

    #[test]
    fn invalid_legacy_username_does_not_block_buglab_sign_in() {
        assert_eq!(
            legacy_username(Some("legacy.user")),
            Some("legacy.user".into())
        );
        assert_eq!(legacy_username(Some("old user")), None);
        assert_eq!(legacy_username(Some("ab")), None);
        assert_eq!(legacy_username(None), None);
    }

    #[test]
    fn attested_email_match_wins_over_stale_legacy_account_mapping() {
        let by_email = buglogin_domain::account::AccountIdentity::new(
            AccountId::new("acct_email_match").unwrap(),
            "current_user",
            "person@example.test",
            "2026-10-28T00:00:00Z",
        )
        .unwrap();
        let by_legacy = buglogin_domain::account::AccountIdentity::new(
            AccountId::new("acct_legacy").unwrap(),
            "old_user",
            "old@example.test",
            "2026-10-28T00:00:00Z",
        )
        .unwrap();

        assert_eq!(
            prefer_buglab_attested_email_identity(
                Some(by_email),
                Some(by_legacy),
                "person@example.test",
            )
            .unwrap()
            .account_id
            .as_str(),
            "acct_email_match"
        );
    }

    #[test]
    fn stale_legacy_username_with_different_email_does_not_block_buglab_provisioning() {
        let stale_legacy = buglogin_domain::account::AccountIdentity::new(
            AccountId::new("acct_stale_legacy").unwrap(),
            "old_user",
            "old@example.test",
            "2026-10-28T00:00:00Z",
        )
        .unwrap();

        assert!(prefer_buglab_attested_email_identity(
            None,
            Some(stale_legacy),
            "current@example.test",
        )
        .is_none());
    }

    #[test]
    fn linked_buglab_subject_survives_provider_email_change_without_remapping_account() {
        let linked = LinkedBugLabIdentity {
            account_id: AccountId::new("acct_linked_subject").unwrap(),
            subject_hash: "stable-subject-hash".into(),
            email: "old@example.test".into(),
        };

        assert!(linked_identity_matches_subject(
            &linked,
            "stable-subject-hash"
        ));
        assert!(!linked_identity_matches_subject(
            &linked,
            "different-subject-hash"
        ));
    }

    #[test]
    fn buglab_attestation_verifies_pending_identity_and_requires_matching_email() {
        let mut identity = buglogin_domain::account::AccountIdentity::admin_provisioned(
            AccountId::new("acct_admin_link").unwrap(),
            "adminlink",
            "admin-link@example.test",
            "2026-10-28T00:00:00Z",
        )
        .unwrap();
        let verified =
            verify_buglab_attested_identity(identity.clone(), "admin-link@example.test").unwrap();
        assert_eq!(verified.verification, IdentityVerification::Verified);
        identity.verification = IdentityVerification::Pending;
        let verified =
            verify_buglab_attested_identity(identity.clone(), "admin-link@example.test").unwrap();
        assert_eq!(verified.verification, IdentityVerification::Verified);
        assert_eq!(
            verify_buglab_attested_identity(identity, "different@example.test")
                .unwrap_err()
                .code,
            "sso_identity_mapping_ambiguous"
        );
    }

    #[test]
    fn new_buglab_identity_uses_common_workspace_or_only_unambiguous_single_workspace() {
        use buglogin_domain::{ids::OrganizationId, workspace::WorkspacePolicy};

        let make_workspace = |id: &str, name: &str| buglogin_domain::workspace::Workspace {
            id: buglogin_domain::ids::WorkspaceId::new(id).unwrap(),
            organization_id: OrganizationId::new("org_bugmedia").unwrap(),
            name: name.into(),
            policy: WorkspacePolicy {
                max_profiles: 100,
                max_members: 30,
                max_storage_bytes: 1_000_000,
                offline_entitled: false,
                default_extension_group_id: None,
            },
            created_at: "2026-10-28T00:00:00Z".into(),
        };
        let team = make_workspace("ws_team", "Team Lam");
        let common = make_workspace("ws_common", "Workspace chung");

        assert_eq!(
            select_default_workspace(&[team.clone(), common.clone()]).map(|ws| &ws.id),
            Some(&common.id)
        );
        assert_eq!(
            select_default_workspace(std::slice::from_ref(&team)).map(|ws| &ws.id),
            Some(&team.id)
        );
        assert!(
            select_default_workspace(&[team, common, make_workspace("ws_shared", "Shared")])
                .is_none()
        );
    }

    #[test]
    fn repeated_sso_login_replaces_the_same_device_session_only() {
        let persistence =
            SessionPersistence::new(ControlPersistence::new(MemoryControlRecordStore::default()));
        let account_id = AccountId::new("acct_sso_device_replacement").unwrap();
        let mut auth = AuthService::persistent(persistence).unwrap();

        let first_device_session = issue_sso_session(
            &mut auth,
            account_id.clone(),
            &transaction("device-sso-a"),
            None,
            100,
            1_000,
            10_000,
        )
        .unwrap();
        let other_device_session = issue_sso_session(
            &mut auth,
            account_id.clone(),
            &transaction("device-sso-b"),
            None,
            101,
            1_000,
            10_000,
        )
        .unwrap();
        let replacement_session = issue_sso_session(
            &mut auth,
            account_id.clone(),
            &transaction("device-sso-a"),
            None,
            200,
            1_000,
            10_000,
        )
        .unwrap();

        assert_eq!(
            auth.authenticate(&first_device_session.access_token, 201)
                .unwrap_err()
                .code,
            "session_revoked"
        );
        assert_eq!(
            auth.authenticate(&other_device_session.access_token, 201)
                .unwrap(),
            account_id
        );
        assert_eq!(
            auth.authenticate(&replacement_session.access_token, 201)
                .unwrap(),
            account_id
        );
        let active = auth
            .list_sessions(&account_id, Some(&replacement_session.access_token))
            .unwrap()
            .into_iter()
            .filter(|session| session.revoked_at.is_none())
            .count();
        assert_eq!(active, 2, "one active SSO session per stable device ID");
    }
}
