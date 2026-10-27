use std::collections::BTreeMap;

use axum::{Json, extract::State, http::HeaderMap, response::Response};
use buglogin_domain::{
    account::{
        Account, AccountIdentity, IdentityVerification, canonical_email, canonical_username,
    },
    errors::Problem,
    ids::{AccountId, DeviceId},
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
    if linked.subject_hash != subject_hash || linked.email != email {
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
        user.old_account
            .as_deref()
            .map(canonical_username)
            .transpose()?
            .map(|username| state.identity_persistence.find_by_username(&username))
            .transpose()?
            .flatten()
    } else {
        None
    };
    if by_email
        .as_ref()
        .is_some_and(|identity| identity.verification != IdentityVerification::Verified)
    {
        return Err(ambiguous_identity());
    }
    if let (Some(email_identity), Some(legacy_identity)) = (&by_email, &by_legacy)
        && email_identity.account_id != legacy_identity.account_id
    {
        return Err(ambiguous_identity());
    }
    let existing = by_email.or(by_legacy);
    let account_id = if let Some(identity) = existing {
        if identity.verification != IdentityVerification::Verified {
            return Err(ambiguous_identity());
        }
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
        let generated_username = format!("buglab_{}", &subject_hash[..16]);
        let candidate_username = if user.app == app_code {
            user.old_account
                .as_deref()
                .and_then(|value| canonical_username(value).ok())
                .unwrap_or(generated_username)
        } else {
            generated_username
        };
        if state
            .identity_persistence
            .find_by_username(&candidate_username)?
            .is_some()
        {
            return Err(ambiguous_identity());
        }
        let account_id = AccountId::new(super::random_bootstrap_id("acct")?)
            .map_err(|_| sso_mapping_invalid())?;
        let created_at = super::migration_timestamp()?;
        let identity = AccountIdentity::new(
            account_id.clone(),
            candidate_username,
            email,
            created_at.clone(),
        )?
        .verify(1, created_at.clone())?;
        let account = Account::new(account_id.clone(), created_at);
        let mut puts = vec![ControlPut {
            tenant_id: "control".into(),
            record_type: "account".into(),
            record_id: account_id.as_str().into(),
            expected_revision: Some(0),
            value: serde_json::to_value(account).map_err(|_| sso_mapping_invalid())?,
        }];
        puts.extend(state.identity_persistence.prepare_create_puts(&identity)?);
        let linked = linked_identity(account_id.clone(), subject_hash, email);
        puts.push(link_record(subject_hash, &linked)?);
        state.control_persistence.execute(ExecuteControlCommand {
            tenant_id: SSO_TENANT.into(),
            command_id: "identity.buglab_sso.provision".into(),
            idempotency_key: format!("buglab-sso-provision-{subject_hash}"),
            expected_revisions: BTreeMap::new(),
            puts,
            deletes: vec![],
            audit: ControlAuditEntry {
                tenant_id: SSO_TENANT.into(),
                command_id: "identity.buglab_sso.provision".into(),
                actor_id: "buglab_id_provider".into(),
                allowed: true,
                detail: json!({
                    "providerSubjectHash": subject_hash,
                    "tenantAccessGranted": false,
                }),
            },
            response: HttpResponse {
                status: 201,
                headers: Default::default(),
                body: json!({"accountId": account_id}),
            },
        })?;
        account_id
    };

    let linked = linked_identity(account_id.clone(), subject_hash, email);
    let revision = state
        .control_persistence
        .revision(SSO_TENANT, SSO_IDENTITY, subject_hash)?
        .unwrap_or(0);
    if revision == 0 {
        state.control_persistence.execute(ExecuteControlCommand {
            tenant_id: SSO_TENANT.into(),
            command_id: "identity.buglab_sso.link".into(),
            idempotency_key: format!("buglab-sso-link-{subject_hash}"),
            expected_revisions: BTreeMap::new(),
            puts: vec![link_record(subject_hash, &linked)?],
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

fn linked_identity(account_id: AccountId, subject_hash: &str, email: &str) -> LinkedBugLabIdentity {
    LinkedBugLabIdentity {
        account_id,
        subject_hash: subject_hash.into(),
        email: email.into(),
    }
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

    use super::{SsoTransaction, SsoTransactionStatus, issue_sso_session};
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
