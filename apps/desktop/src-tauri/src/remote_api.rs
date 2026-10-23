use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    sync::{Arc, RwLock},
    time::Duration,
};

use crate::clock::unix_time_seconds;
use crate::profile_runtime::DesktopSyncBackend;
use crate::runtime_config::url_authority_has_userinfo;
use buglogin_api::services::connected_cookie_jobs::{
    ConnectedCookieFailureCode, ConnectedCookieJob, ConnectedCookieReceipt,
};
use buglogin_api::services::device_key_directory::DeviceEncryptionKey;
use buglogin_api::services::profile_hosted::ProfileKeyRotationRecipientDevices;
use buglogin_api::services::profile_proxy_assignment::ProfileProxyAssignment;
use buglogin_api::services::runtime_dispatch::RuntimeDispatchJob;
use buglogin_browser::runtime::catalog::{RuntimeCatalog, RuntimeSignatureVerifier};
use buglogin_domain::{
    entities::{Profile, ProfileCreateInput, ProfileRecord, ProfileState, ProxyProtocol, SyncMode},
    errors::Problem,
    ids::{
        AccountId, CredentialId, GrantId, MemberId, ProfileId, ProxyId, ResourceId, WorkspaceId,
    },
    profile::{ProfileKeyRotationEnvelopeUpdate, ProfileKeyShareEnvelope, ProfileResource},
};
use buglogin_storage::local::Versioned;
use buglogin_storage::secrets::SecretValue;
use buglogin_sync::{
    coordination::{CoordinationAuth, CoordinationScope, LeaseGrant},
    manifest::{MAX_PROFILE_SYNC_OBJECT_BYTES, SyncManifest},
};
use futures_util::StreamExt;
use reqwest::{Method, StatusCode, Url, header};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, Zeroizing};

const MAX_RUNTIME_CATALOG_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const MAX_AUTH_RESPONSE_BYTES: usize = 16 * 1024;

enum AuthResponseBodyError {
    TooLarge,
    Interrupted,
}

async fn read_bounded_auth_response(
    response: reqwest::Response,
) -> Result<Vec<u8>, AuthResponseBodyError> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_AUTH_RESPONSE_BYTES as u64)
    {
        return Err(AuthResponseBodyError::TooLarge);
    }

    let mut body = Vec::with_capacity(
        response
            .content_length()
            .unwrap_or_default()
            .min(MAX_AUTH_RESPONSE_BYTES as u64) as usize,
    );
    let mut chunks = response.bytes_stream();
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk.map_err(|_| AuthResponseBodyError::Interrupted)?;
        if body.len().saturating_add(chunk.len()) > MAX_AUTH_RESPONSE_BYTES {
            return Err(AuthResponseBodyError::TooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[derive(Clone)]
pub struct RemoteApiClient {
    base_url: Url,
    http: reqwest::Client,
    tokens: Arc<RwLock<ApiTokenState>>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IntegrationApiStatus {
    pub api_endpoint: String,
    pub api_reachable: bool,
    pub api_service: Option<String>,
    pub mcp_endpoint: String,
    pub mcp_enabled: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiTokenSummary {
    pub id: String,
    pub name: String,
    pub scopes: BTreeSet<String>,
    pub created_at: u64,
    pub expires_at: u64,
    pub revoked_at: Option<u64>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatedApiToken {
    pub token: String,
    pub token_record: ApiTokenSummary,
}

impl Drop for CreatedApiToken {
    fn drop(&mut self) {
        self.token.zeroize();
    }
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiTokenPage {
    pub tokens: Vec<ApiTokenSummary>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ApiHealthResponse {
    status: String,
    service: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct McpStatusResponse {
    enabled: bool,
}

#[derive(Default)]
struct ApiTokenState {
    current: Option<Zeroizing<String>>,
    refreshed: Option<Zeroizing<String>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BlobPresence {
    present: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct VersionedProfilePage {
    items: Vec<Versioned<Profile>>,
    #[serde(default)]
    next_cursor: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CookieJobInboxResponse {
    items: Vec<ConnectedCookieJob>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RuntimeDispatchInboxResponse {
    items: Vec<RuntimeDispatchJob>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RuntimeDispatchJobResponse {
    job: RuntimeDispatchJob,
}

impl fmt::Debug for RemoteApiClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RemoteApiClient")
            .field("base_url", &self.base_url)
            .field("token", &"[REDACTED]")
            .finish()
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeviceSessionTokens {
    pub access_token: String,
    pub refresh_token: String,
    pub token_type: String,
    pub expires_at: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DesktopLoginRequest {
    identifier: String,
    password: String,
    device_id: String,
}

impl Drop for DesktopLoginRequest {
    fn drop(&mut self) {
        self.identifier.zeroize();
        self.password.zeroize();
        self.device_id.zeroize();
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LegacySessionRefresh {
    pub access_token: String,
    pub token_type: String,
    pub expires_at: u64,
}

impl Drop for DeviceSessionTokens {
    fn drop(&mut self) {
        self.access_token.zeroize();
        self.refresh_token.zeroize();
    }
}

impl Drop for LegacySessionRefresh {
    fn drop(&mut self) {
        self.access_token.zeroize();
    }
}

impl fmt::Debug for DeviceSessionTokens {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeviceSessionTokens")
            .field("access_token", &"[REDACTED]")
            .field("refresh_token", &"[REDACTED]")
            .field("token_type", &self.token_type)
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProxyRecordView {
    workspace_id: WorkspaceId,
    proxy: ProxyView,
    revision: u64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProxyView {
    id: ProxyId,
    protocol: ProxyProtocol,
    host: String,
    port: u16,
    has_credential: bool,
    #[serde(default)]
    credential_version: Option<CredentialId>,
    bypass_rules: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct RemoteProxyLaunch {
    pub server: String,
    pub bypass_rules: Vec<String>,
    pub proxy_id: ProxyId,
    pub revision: u64,
    pub protocol: ProxyProtocol,
    pub host: String,
    pub port: u16,
    pub has_credential: bool,
    pub credential_version: Option<CredentialId>,
}

#[derive(Clone, Debug)]
pub struct RemoteProxySyncSnapshot {
    pub proxy_id: ProxyId,
    pub revision: u64,
    pub credential_version: Option<CredentialId>,
    /// Plaintext exists only in Rust memory until ProfileProjection seals it
    /// with the selected profile resource key.
    pub credential: Option<SecretValue>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiProblem {
    #[serde(rename = "type")]
    problem_type: Option<String>,
    code: String,
    title: String,
    status: u16,
    instance: Option<String>,
    correlation_id: Option<String>,
    detail: Option<String>,
    retryable: Option<bool>,
    fields: Option<BTreeMap<String, Value>>,
}

impl RemoteApiClient {
    pub fn new(base_url: &str, token: String) -> Result<Self, Problem> {
        Self::build(base_url, Some(token))
    }

    pub fn new_unauthenticated(base_url: &str) -> Result<Self, Problem> {
        Self::build(base_url, None)
    }

    fn build(base: &str, token: Option<String>) -> Result<Self, Problem> {
        let token = token.map(Zeroizing::new);
        let mut base_url = Url::parse(base).map_err(|_| client_problem("api_url_invalid", 400))?;
        let local_http = base_url.scheme() == "http"
            && matches!(base_url.host_str(), Some("127.0.0.1" | "localhost" | "::1"));
        if (base_url.scheme() != "https" && !local_http)
            || base_url.host_str().is_none()
            || url_authority_has_userinfo(base)
            || !base_url.username().is_empty()
            || base_url.password().is_some()
            || base_url.query().is_some()
            || base_url.fragment().is_some()
            || !matches!(base_url.path(), "/" | "")
            || token.as_ref().is_some_and(|value| !valid_bearer(value))
        {
            return Err(client_problem("api_url_invalid", 400));
        }
        base_url.set_path("/");
        let http = reqwest::Client::builder()
            .https_only(!local_http)
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(|_| client_problem("api_client_unavailable", 503))?;
        Ok(Self {
            base_url,
            http,
            tokens: Arc::new(RwLock::new(ApiTokenState {
                current: token,
                refreshed: None,
            })),
        })
    }

    pub fn has_token(&self) -> Result<bool, Problem> {
        self.tokens
            .read()
            .map(|tokens| tokens.current.is_some())
            .map_err(|_| client_problem("api_client_unavailable", 503))
    }

    pub fn set_token(&self, token: String) -> Result<(), Problem> {
        let token = Zeroizing::new(token);
        if !valid_bearer(&token) {
            return Err(client_problem("api_token_invalid", 400));
        }
        let mut tokens = self
            .tokens
            .write()
            .map_err(|_| client_problem("api_client_unavailable", 503))?;
        tokens.current = Some(token);
        tokens.refreshed = None;
        Ok(())
    }

    pub fn replace_refreshed_token(&self, token: String) -> Result<(), Problem> {
        let token = Zeroizing::new(token);
        if !valid_bearer(&token) {
            return Err(client_problem("api_token_invalid", 400));
        }
        let mut tokens = self
            .tokens
            .write()
            .map_err(|_| client_problem("api_client_unavailable", 503))?;
        tokens.current = Some(token.clone());
        tokens.refreshed = Some(token);
        Ok(())
    }

    pub fn clear_token(&self) -> Result<(), Problem> {
        let mut tokens = self
            .tokens
            .write()
            .map_err(|_| client_problem("api_client_unavailable", 503))?;
        tokens.current = None;
        tokens.refreshed = None;
        Ok(())
    }

    pub async fn integration_status(
        &self,
        correlation: &str,
    ) -> Result<IntegrationApiStatus, Problem> {
        let health = self
            .send(Method::GET, "v1/health", None, correlation, None, None, &[])
            .await;
        let api_reachable = health
            .as_ref()
            .is_ok_and(|health: &ApiHealthResponse| health.status == "ok");
        let api_service = health.ok().map(|health| health.service);
        let mcp_enabled = if api_reachable {
            self.send::<McpStatusResponse>(
                Method::GET,
                "v1/mcp/status",
                None,
                correlation,
                None,
                None,
                &[],
            )
            .await
            .ok()
            .map(|status| status.enabled)
        } else {
            None
        };
        Ok(IntegrationApiStatus {
            api_endpoint: self.base_url.as_str().trim_end_matches('/').to_owned(),
            api_reachable,
            api_service,
            mcp_endpoint: format!(
                "{}{}",
                self.base_url.as_str().trim_end_matches('/'),
                "/v1/mcp"
            ),
            mcp_enabled,
        })
    }

    pub async fn list_api_tokens(&self, correlation: &str) -> Result<ApiTokenPage, Problem> {
        self.send(
            Method::GET,
            "v1/auth/api-tokens",
            None,
            correlation,
            None,
            None,
            &[],
        )
        .await
    }

    pub async fn create_api_token(
        &self,
        name: &str,
        scopes: BTreeSet<String>,
        expires_at: u64,
        correlation: &str,
    ) -> Result<CreatedApiToken, Problem> {
        self.send(
            Method::POST,
            "v1/auth/api-tokens",
            Some(json!({ "name": name, "scopes": scopes, "expiresAt": expires_at })),
            correlation,
            Some(&format!("mcp-token-create-{correlation}")),
            None,
            &[],
        )
        .await
    }

    pub async fn revoke_api_token(&self, token_id: &str, correlation: &str) -> Result<(), Problem> {
        self.send::<Value>(
            Method::DELETE,
            &format!("v1/auth/api-tokens/{token_id}"),
            None,
            correlation,
            Some(&format!("mcp-token-revoke-{correlation}")),
            None,
            &[],
        )
        .await?;
        Ok(())
    }

    pub async fn open_event_stream(
        &self,
        workspace: &WorkspaceId,
        after: Option<&str>,
        limit: usize,
        correlation: &str,
    ) -> Result<reqwest::Response, Problem> {
        let url = self
            .base_url
            .join("v1/events")
            .map_err(|_| client_problem("api_url_invalid", 400))?;
        let token = self.preferred_token()?;
        let token = token.ok_or_else(|| {
            Problem::new(
                "authentication_required",
                "Sign in to BugLogin",
                401,
                correlation,
            )
        })?;
        let mut request = self
            .http
            .get(url)
            .query(&[("workspaceId", workspace.as_str())])
            .query(&[("limit", limit)])
            .bearer_auth(token.as_str())
            .header("x-correlation-id", correlation)
            .header(header::ACCEPT, "text/event-stream");
        if let Some(after) = after {
            request = request
                .query(&[("after", after)])
                .header("last-event-id", after);
        }
        let response = request.send().await.map_err(|error| {
            Problem::new(
                "api_unavailable",
                "BugLogin event stream is unavailable",
                503,
                correlation,
            )
            .retryable(error.is_connect() || error.is_timeout())
        })?;
        if !response.status().is_success() {
            return Err(read_problem(response, correlation, true).await);
        }
        Ok(response)
    }

    async fn send<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        correlation: &str,
        idempotency: Option<&str>,
        auth: Option<&CoordinationAuth>,
        extra: &[(&str, String)],
    ) -> Result<T, Problem> {
        let url = self
            .base_url
            .join(path.trim_start_matches('/'))
            .map_err(|_| client_problem("api_url_invalid", 400))?;
        let retry_safe = request_retry_safe(&method, path, idempotency.is_some());
        let mut request = self
            .http
            .request(method, url)
            .header("x-correlation-id", correlation);
        let token = self.preferred_token()?;
        let token = match (token, auth) {
            (Some(token), _) => Some(token),
            (None, Some(auth)) => Some(Zeroizing::new(auth.bearer_token().to_owned())),
            (None, None) => None,
        };
        if let Some(token) = token {
            request = request.bearer_auth(token.as_str());
        }
        if let Some(key) = idempotency {
            request = request.header("Idempotency-Key", key);
        }
        for (name, value) in extra {
            request = request.header(*name, value);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.map_err(|error| {
            Problem::new(
                "api_unavailable",
                "BugLogin API is unavailable",
                503,
                correlation,
            )
            .retryable(error.is_connect() || (retry_safe && error.is_timeout()))
        })?;
        let status = response.status();
        if !status.is_success() {
            return Err(read_problem(response, correlation, retry_safe).await);
        }
        if status == StatusCode::NO_CONTENT {
            return serde_json::from_value(Value::Null)
                .map_err(|_| client_problem("api_response_invalid", 502));
        }
        response.json().await.map_err(|error| {
            Problem::new(
                "api_response_invalid",
                "BugLogin API response is incomplete or invalid",
                502,
                correlation,
            )
            .retryable(retry_safe && (error.is_body() || error.is_timeout() || error.is_decode()))
        })
    }

    async fn send_bytes(
        &self,
        method: Method,
        path: &str,
        bytes: Vec<u8>,
        correlation: &str,
        idempotency: &str,
        auth: &CoordinationAuth,
        extra: &[(&str, String)],
    ) -> Result<Value, Problem> {
        let url = self
            .base_url
            .join(path.trim_start_matches('/'))
            .map_err(|_| client_problem("api_url_invalid", 400))?;
        let token = self
            .preferred_token()?
            .unwrap_or_else(|| Zeroizing::new(auth.bearer_token().to_owned()));
        let mut request = self
            .http
            .request(method, url)
            .bearer_auth(token.as_str())
            .header("x-correlation-id", correlation)
            .header("Idempotency-Key", idempotency)
            .header(header::CONTENT_TYPE, "application/octet-stream");
        for (name, value) in extra {
            request = request.header(*name, value);
        }
        let response = request.body(bytes).send().await.map_err(|error| {
            Problem::new(
                "api_unavailable",
                "BugLogin API is unavailable",
                503,
                correlation,
            )
            .retryable(error.is_connect() || error.is_timeout())
        })?;
        if !response.status().is_success() {
            return Err(read_problem(response, correlation, true).await);
        }
        response.json().await.map_err(|_| {
            Problem::new(
                "api_response_invalid",
                "BugLogin API response is incomplete or invalid",
                502,
                correlation,
            )
            .retryable(true)
        })
    }

    async fn send_text(
        &self,
        path: &str,
        body: String,
        correlation: &str,
    ) -> Result<Value, Problem> {
        let url = self
            .base_url
            .join(path.trim_start_matches('/'))
            .map_err(|_| client_problem("api_url_invalid", 400))?;
        let token = self.token_value()?;
        let response = self
            .http
            .post(url)
            .bearer_auth(token.as_str())
            .header("x-correlation-id", correlation)
            .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
            .body(body)
            .send()
            .await
            .map_err(|error| {
                Problem::new(
                    "api_unavailable",
                    "BugLogin API is unavailable",
                    503,
                    correlation,
                )
                .retryable(error.is_connect())
            })?;
        parse_response(response, correlation).await
    }

    async fn get<T: DeserializeOwned>(&self, path: &str, correlation: &str) -> Result<T, Problem> {
        self.send(Method::GET, path, None, correlation, None, None, &[])
            .await
    }

    pub async fn authenticated_account_id(&self, correlation: &str) -> Result<AccountId, Problem> {
        let account: Value = self.get("/v1/account", correlation).await?;
        let account_id = account
            .get("accountId")
            .and_then(Value::as_str)
            .ok_or_else(|| bad_input("accountId", correlation))?;
        AccountId::from_str(account_id).map_err(|_| bad_input("accountId", correlation))
    }

    pub async fn authenticated_coordination_auth(
        &self,
        correlation: &str,
    ) -> Result<CoordinationAuth, Problem> {
        let account_id = self.authenticated_account_id(correlation).await?;
        let bearer = self.token_value()?;
        CoordinationAuth::new(account_id.as_str(), bearer.to_string())
    }

    async fn get_public_bounded<T: DeserializeOwned>(
        &self,
        path: &str,
        correlation: &str,
    ) -> Result<T, Problem> {
        let url = self
            .base_url
            .join(path.trim_start_matches('/'))
            .map_err(|_| client_problem("api_url_invalid", 400))?;
        let response = self
            .http
            .get(url)
            .header("x-correlation-id", correlation)
            .send()
            .await
            .map_err(|error| {
                Problem::new(
                    "api_unavailable",
                    "BugLogin API is unavailable",
                    503,
                    correlation,
                )
                .retryable(error.is_connect() || error.is_timeout())
            })?;
        if !response.status().is_success() {
            return Err(read_problem(response, correlation, true).await);
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_RUNTIME_CATALOG_RESPONSE_BYTES as u64)
        {
            return Err(Problem::new(
                "runtime_catalog_size_invalid",
                "BugLogin runtime catalog exceeds the permitted size",
                502,
                correlation,
            ));
        }
        let mut stream = response.bytes_stream();
        let mut body = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| {
                Problem::new(
                    "api_unavailable",
                    "BugLogin API response was interrupted",
                    503,
                    correlation,
                )
                .retryable(error.is_body() || error.is_timeout())
            })?;
            if body.len().saturating_add(chunk.len()) > MAX_RUNTIME_CATALOG_RESPONSE_BYTES {
                return Err(Problem::new(
                    "runtime_catalog_size_invalid",
                    "BugLogin runtime catalog exceeds the permitted size",
                    502,
                    correlation,
                ));
            }
            body.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&body).map_err(|_| {
            Problem::new(
                "api_response_invalid",
                "BugLogin API response is incomplete or invalid",
                502,
                correlation,
            )
        })
    }

    pub async fn verified_runtime_catalog(
        &self,
        verifier: &(impl RuntimeSignatureVerifier + Sync),
        correlation: &str,
    ) -> Result<RuntimeCatalog, Problem> {
        let catalog: RuntimeCatalog = self
            .get_public_bounded("/v1/runtime/catalog", correlation)
            .await?;
        catalog
            .verify(verifier)
            .map_err(|error| Problem::new(error.code, error.title, error.status, correlation))?;
        Ok(catalog)
    }

    pub async fn desktop_login(
        &self,
        identifier: String,
        password: String,
        device_id: &str,
        correlation: &str,
    ) -> Result<DeviceSessionTokens, Problem> {
        let request = DesktopLoginRequest {
            identifier,
            password,
            device_id: device_id.to_owned(),
        };
        let url = self
            .base_url
            .join("v1/auth/desktop/login")
            .map_err(|_| client_problem("api_url_invalid", 400))?;
        let response = self
            .http
            .post(url)
            .header("x-correlation-id", correlation)
            .json(&request)
            .send()
            .await
            .map_err(|error| {
                Problem::new(
                    "api_unavailable",
                    "BugLogin API is unavailable",
                    503,
                    correlation,
                )
                .retryable(error.is_connect() || error.is_timeout())
            })?;
        drop(request);
        if !response.status().is_success() {
            return Err(read_problem(response, correlation, false).await);
        }
        let body = read_bounded_auth_response(response)
            .await
            .map_err(|error| match error {
                AuthResponseBodyError::TooLarge => Problem::new(
                    "api_auth_response_invalid",
                    "BugLogin returned an invalid sign-in response",
                    502,
                    correlation,
                ),
                AuthResponseBodyError::Interrupted => Problem::new(
                    "api_auth_response_invalid",
                    "BugLogin sign-in response was interrupted",
                    502,
                    correlation,
                ),
            })?;
        let token: DeviceSessionTokens = serde_json::from_slice(&body).map_err(|_| {
            Problem::new(
                "api_auth_response_invalid",
                "BugLogin returned an invalid sign-in response",
                502,
                correlation,
            )
        })?;
        let now = unix_time_seconds(correlation)?;
        if token.access_token.is_empty()
            || token.refresh_token.is_empty()
            || token.token_type != "Bearer"
            || token.expires_at <= now
        {
            return Err(Problem::new(
                "api_auth_response_invalid",
                "BugLogin returned an invalid sign-in response",
                502,
                correlation,
            ));
        }
        Ok(token)
    }

    pub async fn register_device_encryption_key(
        &self,
        device_id: &str,
        public_key: &str,
        fingerprint: &str,
        idempotency_key: &str,
        correlation: &str,
    ) -> Result<(), Problem> {
        let registered: DeviceEncryptionKey = self
            .send(
                Method::PUT,
                "/v1/auth/device/encryption-key",
                Some(json!({"publicKey":public_key})),
                correlation,
                Some(idempotency_key),
                None,
                &[],
            )
            .await?;
        if registered.schema_version != 1
            || registered.device_id != device_id
            || registered.public_key != public_key
            || registered.fingerprint != fingerprint
        {
            return Err(Problem::new(
                "api_device_key_response_invalid",
                "BugLogin returned an unexpected device encryption key record",
                502,
                correlation,
            ));
        }
        Ok(())
    }

    pub async fn device_session_refresh(
        &self,
        refresh_token: &str,
        device_id: &str,
        correlation: &str,
    ) -> Result<DeviceSessionTokens, Problem> {
        // The refresh credential is already a high-entropy secret. A
        // domain-separated digest gives every rotation a stable opaque key,
        // so retries (including after a lost response) replay the same server
        // result without persisting a second secret in the desktop vault.
        let mut key_hasher = Sha256::new();
        key_hasher.update(b"buglogin-device-session-refresh-v1\0");
        key_hasher.update(device_id.as_bytes());
        key_hasher.update([0]);
        key_hasher.update(refresh_token.as_bytes());
        let idempotency_key = format!("device-refresh-{:x}", key_hasher.finalize());
        let url = self
            .base_url
            .join("v1/auth/device/refresh")
            .map_err(|_| client_problem("api_url_invalid", 400))?;
        let body = json!({"refreshToken":refresh_token,"deviceId":device_id});
        let mut response_body = None;
        let mut last_problem = None;
        for attempt in 0..2 {
            let response = match self
                .http
                .post(url.clone())
                .header("x-correlation-id", correlation)
                .header("idempotency-key", &idempotency_key)
                .json(&body)
                .send()
                .await
            {
                Ok(value) => value,
                Err(_) => {
                    last_problem = Some(
                        Problem::new(
                            "api_unavailable",
                            "BugLogin API is unavailable",
                            503,
                            correlation,
                        )
                        .retryable(true),
                    );
                    continue;
                }
            };
            if !response.status().is_success() {
                let retryable_server_error = response.status().is_server_error();
                let retryable_refresh_response = retryable_server_error
                    || matches!(
                        response.status(),
                        StatusCode::TOO_MANY_REQUESTS | StatusCode::REQUEST_TIMEOUT
                    );
                let mut problem = read_problem(response, correlation, false).await;
                if retryable_refresh_response {
                    // The server may have committed rotation before the
                    // response failed. The stable idempotency key makes a
                    // same-token retry safe and lets the desktop preserve
                    // its vault credentials.
                    problem.retryable = true;
                }
                if retryable_server_error && attempt == 0 {
                    last_problem = Some(problem);
                    continue;
                }
                return Err(problem);
            }
            match read_bounded_auth_response(response).await {
                Ok(bytes) => {
                    response_body = Some(bytes);
                    break;
                }
                Err(AuthResponseBodyError::TooLarge) => {
                    let problem = Problem::new(
                        "api_auth_response_invalid",
                        "BugLogin returned an oversized refreshed device session",
                        502,
                        correlation,
                    )
                    .retryable(true);
                    if attempt == 0 {
                        last_problem = Some(problem);
                        continue;
                    }
                    return Err(problem);
                }
                Err(AuthResponseBodyError::Interrupted) => {
                    last_problem = Some(
                        Problem::new(
                            "api_unavailable",
                            "BugLogin API is unavailable",
                            503,
                            correlation,
                        )
                        .retryable(true),
                    );
                }
            }
        }
        let response_body = response_body.ok_or_else(|| {
            last_problem.unwrap_or_else(|| {
                // Both attempts may have reached the server. Do not let callers
                // reuse the old credential after the bounded replay window; the
                // desktop will fail closed and require a fresh authorization.
                Problem::new(
                    "api_unavailable",
                    "BugLogin API is unavailable",
                    503,
                    correlation,
                )
                .retryable(true)
            })
        })?;
        let token: DeviceSessionTokens = serde_json::from_slice(&response_body)
            .map_err(|_| client_problem("api_response_invalid", 502).retryable(true))?;
        let now = unix_time_seconds(correlation)?;
        if token.access_token.is_empty()
            || token.refresh_token.is_empty()
            || token.token_type != "Bearer"
            || token.expires_at <= now
        {
            return Err(Problem::new(
                "api_auth_response_invalid",
                "BugLogin returned an invalid refreshed device session",
                502,
                correlation,
            )
            .retryable(true));
        }
        Ok(token)
    }

    pub async fn legacy_session_refresh(
        &self,
        correlation: &str,
    ) -> Result<LegacySessionRefresh, Problem> {
        let token: LegacySessionRefresh = self
            .send(
                Method::POST,
                "/v1/auth/refresh",
                None,
                correlation,
                None,
                None,
                &[],
            )
            .await?;
        let now = unix_time_seconds(correlation)?;
        if token.access_token.is_empty() || token.token_type != "Bearer" || token.expires_at <= now
        {
            return Err(Problem::new(
                "api_auth_response_invalid",
                "BugLogin returned an invalid refreshed session",
                502,
                correlation,
            ));
        }
        Ok(token)
    }

    pub async fn logout(&self, correlation: &str) -> Result<(), Problem> {
        let _: Value = self
            .send(
                Method::POST,
                "/v1/auth/logout",
                Some(json!({})),
                correlation,
                None,
                None,
                &[],
            )
            .await?;
        Ok(())
    }

    pub async fn cookie_job_inbox(
        &self,
        workspace: &WorkspaceId,
        correlation: &str,
    ) -> Result<Vec<ConnectedCookieJob>, Problem> {
        let response: CookieJobInboxResponse = self
            .get(
                &format!("/v1/workspaces/{}/cookie-jobs/inbox", workspace.as_str()),
                correlation,
            )
            .await?;
        Ok(response.items)
    }

    pub async fn cookie_job_inbox_all(
        &self,
        correlation: &str,
    ) -> Result<Vec<ConnectedCookieJob>, Problem> {
        let response: CookieJobInboxResponse = self
            .get("/v1/device/cookie-jobs/inbox", correlation)
            .await?;
        Ok(response.items)
    }

    pub async fn runtime_dispatch_inbox(
        &self,
        correlation: &str,
    ) -> Result<Vec<RuntimeDispatchJob>, Problem> {
        let response: RuntimeDispatchInboxResponse = self
            .get("/v1/device/runtime-dispatch/inbox", correlation)
            .await?;
        Ok(response.items)
    }

    pub async fn claim_runtime_dispatch(
        &self,
        workspace: &WorkspaceId,
        job_id: &str,
        correlation: &str,
    ) -> Result<RuntimeDispatchJob, Problem> {
        validate_runtime_dispatch_job_id(job_id)?;
        let response: RuntimeDispatchJobResponse = self
            .send(
                Method::POST,
                &format!(
                    "/v1/workspaces/{}/runtime-dispatch/{job_id}/claim",
                    workspace.as_str()
                ),
                None,
                correlation,
                None,
                None,
                &[],
            )
            .await?;
        Ok(response.job)
    }

    pub async fn complete_runtime_dispatch(
        &self,
        workspace: &WorkspaceId,
        job_id: &str,
        receipt: Value,
        correlation: &str,
    ) -> Result<RuntimeDispatchJob, Problem> {
        validate_runtime_dispatch_job_id(job_id)?;
        let response: RuntimeDispatchJobResponse = self
            .send(
                Method::POST,
                &format!(
                    "/v1/workspaces/{}/runtime-dispatch/{job_id}/complete",
                    workspace.as_str()
                ),
                Some(receipt),
                correlation,
                None,
                None,
                &[],
            )
            .await?;
        Ok(response.job)
    }

    pub async fn fail_runtime_dispatch(
        &self,
        workspace: &WorkspaceId,
        job_id: &str,
        failure_code: &str,
        correlation: &str,
    ) -> Result<RuntimeDispatchJob, Problem> {
        validate_runtime_dispatch_job_id(job_id)?;
        let response: RuntimeDispatchJobResponse = self
            .send(
                Method::POST,
                &format!(
                    "/v1/workspaces/{}/runtime-dispatch/{job_id}/fail",
                    workspace.as_str()
                ),
                Some(json!({"failureCode": failure_code})),
                correlation,
                None,
                None,
                &[],
            )
            .await?;
        Ok(response.job)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn enqueue_cookie_job(
        &self,
        command_id: &str,
        workspace: &WorkspaceId,
        profile: &ProfileId,
        device_id: &str,
        expected_generation: u64,
        staged_import_ref: Option<&str>,
        source: Option<(&WorkspaceId, &ProfileId)>,
        idempotency_key: &str,
        correlation: &str,
    ) -> Result<ConnectedCookieJob, Problem> {
        if !matches!(command_id, "cookie.import" | "cookie.copy")
            || device_id.is_empty()
            || device_id.len() > 256
            || staged_import_ref.is_some_and(|value| value.is_empty() || value.len() > 256)
        {
            return Err(client_problem("cookie_job_input_invalid", 400));
        }
        let mut body = json!({
            "commandId": command_id,
            "workspaceId": workspace.as_str(),
            "profileId": profile.as_str(),
            "deviceId": device_id,
            "expectedGeneration": expected_generation,
        });
        if let Some(stage) = staged_import_ref {
            body["stagedImportRef"] = json!(stage);
        }
        if let Some((source_workspace, source_profile)) = source {
            body["sourceWorkspaceId"] = json!(source_workspace.as_str());
            body["sourceProfileId"] = json!(source_profile.as_str());
            body["sourceDeviceId"] = json!(device_id);
        }
        self.send(
            Method::POST,
            "/v1/cookie-jobs",
            Some(body),
            correlation,
            Some(idempotency_key),
            None,
            &[],
        )
        .await
    }

    pub async fn claim_cookie_job(
        &self,
        workspace: &WorkspaceId,
        job_id: &str,
        correlation: &str,
    ) -> Result<ConnectedCookieJob, Problem> {
        validate_cookie_job_id(job_id)?;
        self.send(
            Method::POST,
            &format!(
                "/v1/cookie-jobs/{job_id}/claim?workspaceId={}",
                workspace.as_str()
            ),
            None,
            correlation,
            None,
            None,
            &[],
        )
        .await
    }

    pub async fn acknowledge_cookie_job(
        &self,
        workspace: &WorkspaceId,
        job_id: &str,
        lease: &LeaseGrant,
        correlation: &str,
    ) -> Result<ConnectedCookieJob, Problem> {
        validate_cookie_job_id(job_id)?;
        let extra = [
            ("x-buglogin-owner-id", lease.owner_id.clone()),
            ("x-buglogin-owner-token", lease.owner_token.clone()),
            ("x-buglogin-fencing-token", lease.fencing_token.to_string()),
        ];
        self.send(
            Method::POST,
            &format!(
                "/v1/cookie-jobs/{job_id}/ack?workspaceId={}",
                workspace.as_str()
            ),
            None,
            correlation,
            None,
            None,
            &extra,
        )
        .await
    }

    pub async fn complete_cookie_job(
        &self,
        workspace: &WorkspaceId,
        job_id: &str,
        receipt: &ConnectedCookieReceipt,
        correlation: &str,
    ) -> Result<ConnectedCookieJob, Problem> {
        validate_cookie_job_id(job_id)?;
        self.send(
            Method::POST,
            &format!(
                "/v1/cookie-jobs/{job_id}/complete?workspaceId={}",
                workspace.as_str()
            ),
            Some(
                serde_json::to_value(receipt)
                    .map_err(|_| client_problem("cookie_job_receipt_invalid", 400))?,
            ),
            correlation,
            None,
            None,
            &[],
        )
        .await
    }

    pub async fn fail_cookie_job(
        &self,
        workspace: &WorkspaceId,
        job_id: &str,
        failure_code: ConnectedCookieFailureCode,
        correlation: &str,
    ) -> Result<ConnectedCookieJob, Problem> {
        validate_cookie_job_id(job_id)?;
        self.send(
            Method::POST,
            &format!(
                "/v1/cookie-jobs/{job_id}/fail?workspaceId={}",
                workspace.as_str()
            ),
            Some(serde_json::json!({ "failureCode": failure_code })),
            correlation,
            None,
            None,
            &[],
        )
        .await
    }

    pub async fn cancel_cookie_job(
        &self,
        workspace: &WorkspaceId,
        job_id: &str,
        correlation: &str,
    ) -> Result<ConnectedCookieJob, Problem> {
        validate_cookie_job_id(job_id)?;
        self.send(
            Method::POST,
            &format!(
                "/v1/cookie-jobs/{job_id}/cancel?workspaceId={}",
                workspace.as_str()
            ),
            None,
            correlation,
            None,
            None,
            &[],
        )
        .await
    }

    pub async fn profile(
        &self,
        id: &ProfileId,
        correlation: &str,
    ) -> Result<Versioned<Profile>, Problem> {
        self.get(&format!("/v1/profiles/{}", id.as_str()), correlation)
            .await
    }

    pub async fn profile_binding(
        &self,
        id: &ProfileId,
        correlation: &str,
    ) -> Result<ProfileResource, Problem> {
        self.get(
            &format!("/v1/profiles/{}/binding", id.as_str()),
            correlation,
        )
        .await
    }

    pub async fn profile_key_share_envelope(
        &self,
        id: &ProfileId,
        correlation: &str,
    ) -> Result<ProfileKeyShareEnvelope, Problem> {
        self.get(
            &format!("/v1/profiles/{}/key-envelope", id.as_str()),
            correlation,
        )
        .await
    }

    pub async fn profile_share_recipient_device_keys(
        &self,
        profile_id: &ProfileId,
        member_id: &buglogin_domain::ids::MemberId,
        correlation: &str,
    ) -> Result<Vec<DeviceEncryptionKey>, Problem> {
        self.get(
            &format!(
                "/v1/profiles/{}/shares/{}/device-keys",
                profile_id.as_str(),
                member_id.as_str()
            ),
            correlation,
        )
        .await
    }

    pub async fn profile_key_rotation_recipients(
        &self,
        profile_id: &ProfileId,
        correlation: &str,
    ) -> Result<Vec<ProfileKeyRotationRecipientDevices>, Problem> {
        self.get(
            &format!(
                "/v1/profiles/{}/key-rotation-recipients",
                profile_id.as_str()
            ),
            correlation,
        )
        .await
    }

    pub async fn proxy_for_launch(
        &self,
        workspace: &WorkspaceId,
        id: &ProxyId,
        correlation: &str,
    ) -> Result<RemoteProxyLaunch, Problem> {
        let record: ProxyRecordView = self
            .get(
                &format!(
                    "/v1/workspaces/{}/proxies/{}",
                    workspace.as_str(),
                    id.as_str()
                ),
                correlation,
            )
            .await?;
        if record.workspace_id != *workspace || record.proxy.id != *id {
            return Err(client_problem("api_scope_mismatch", 502));
        }
        let protocol = match record.proxy.protocol {
            ProxyProtocol::Http => "http",
            ProxyProtocol::Https => "https",
            ProxyProtocol::Socks4 => "socks4",
            ProxyProtocol::Socks5 => "socks5",
        };
        Ok(RemoteProxyLaunch {
            server: format!("{protocol}://{}:{}", record.proxy.host, record.proxy.port),
            bypass_rules: record.proxy.bypass_rules,
            proxy_id: record.proxy.id,
            revision: record.revision,
            protocol: record.proxy.protocol,
            host: record.proxy.host,
            port: record.proxy.port,
            has_credential: record.proxy.has_credential,
            credential_version: record.proxy.credential_version,
        })
    }

    pub async fn proxy_for_sync(
        &self,
        workspace: &WorkspaceId,
        id: &ProxyId,
        expected_revision: u64,
        credential: Option<SecretValue>,
        correlation: &str,
    ) -> Result<RemoteProxySyncSnapshot, Problem> {
        let record: ProxyRecordView = self
            .get(
                &format!(
                    "/v1/workspaces/{}/proxies/{}",
                    workspace.as_str(),
                    id.as_str()
                ),
                correlation,
            )
            .await?;
        if record.workspace_id != *workspace || record.proxy.id != *id {
            return Err(client_problem("api_scope_mismatch", 502));
        }
        if record.revision != expected_revision {
            return Err(Problem::new(
                "proxy_revision_changed",
                "Proxy changed while its profile snapshot was being prepared",
                409,
                correlation,
            )
            .retryable(true));
        }
        if record.proxy.has_credential != record.proxy.credential_version.is_some() {
            return Err(Problem::new(
                "proxy_credential_version_missing",
                "BugLogin API returned inconsistent proxy credential version metadata",
                502,
                correlation,
            ));
        }
        if record.proxy.has_credential && credential.is_none() {
            return Err(Problem::new(
                "proxy_secret_sync_unavailable",
                "This device does not have the selected proxy credentials in its operating system vault",
                409,
                correlation,
            ));
        }
        if !record.proxy.has_credential && credential.is_some() {
            return Err(Problem::new(
                "proxy_credential_state_mismatch",
                "Proxy credential state does not match the API record",
                409,
                correlation,
            ));
        }
        Ok(RemoteProxySyncSnapshot {
            proxy_id: id.clone(),
            revision: record.revision,
            credential_version: record.proxy.credential_version,
            credential,
        })
    }

    pub async fn upload_blob(
        &self,
        scope: CoordinationScope<'_>,
        resource: &ResourceId,
        key: &str,
        generation: u64,
        checksum: &str,
        owner_token: &str,
        fence: u64,
        idem: &str,
        bytes: Vec<u8>,
        correlation: &str,
    ) -> Result<(), Problem> {
        let path = format!(
            "/v1/workspaces/{}/resources/{}/blobs?key={}",
            scope.workspace_id.as_str(),
            resource.as_str(),
            urlencoding(key)
        );
        let extra = [
            ("x-buglogin-owner-id", scope.auth.actor_id().to_owned()),
            ("x-buglogin-owner-token", owner_token.to_owned()),
            ("x-buglogin-fencing-token", fence.to_string()),
            ("x-buglogin-generation", generation.to_string()),
            ("x-content-sha256", checksum.to_owned()),
        ];
        let _: Value = self
            .send_bytes(
                Method::PUT,
                &path,
                bytes,
                correlation,
                idem,
                scope.auth,
                &extra,
            )
            .await?;
        Ok(())
    }

    pub async fn blob_present(
        &self,
        scope: CoordinationScope<'_>,
        resource: &ResourceId,
        key: &str,
        generation: u64,
        checksum: &str,
        owner_token: &str,
        fence: u64,
        correlation: &str,
    ) -> Result<bool, Problem> {
        let path = format!(
            "/v1/workspaces/{}/resources/{}/blobs/check?key={}&generation={generation}&checksum={checksum}",
            scope.workspace_id.as_str(),
            resource.as_str(),
            urlencoding(key)
        );
        let extra = [
            ("x-buglogin-owner-id", scope.auth.actor_id().to_owned()),
            ("x-buglogin-owner-token", owner_token.to_owned()),
            ("x-buglogin-fencing-token", fence.to_string()),
        ];
        let response: BlobPresence = self
            .send(
                Method::GET,
                &path,
                None,
                correlation,
                None,
                Some(scope.auth),
                &extra,
            )
            .await?;
        Ok(response.present)
    }

    pub async fn download_blob(
        &self,
        scope: CoordinationScope<'_>,
        resource: &ResourceId,
        key: &str,
        generation: u64,
        checksum: &str,
        expected_size: u64,
        correlation: &str,
    ) -> Result<Vec<u8>, Problem> {
        self.download_blob_inner(
            scope,
            resource,
            key,
            generation,
            checksum,
            expected_size,
            correlation,
            false,
        )
        .await
    }

    pub async fn download_blob_for_proxy_repair(
        &self,
        scope: CoordinationScope<'_>,
        resource: &ResourceId,
        key: &str,
        generation: u64,
        checksum: &str,
        expected_size: u64,
        correlation: &str,
    ) -> Result<Vec<u8>, Problem> {
        self.download_blob_inner(
            scope,
            resource,
            key,
            generation,
            checksum,
            expected_size,
            correlation,
            true,
        )
        .await
    }

    async fn download_blob_inner(
        &self,
        scope: CoordinationScope<'_>,
        resource: &ResourceId,
        key: &str,
        generation: u64,
        checksum: &str,
        expected_size: u64,
        correlation: &str,
        repair_stale_proxy: bool,
    ) -> Result<Vec<u8>, Problem> {
        if expected_size > MAX_PROFILE_SYNC_OBJECT_BYTES {
            return Err(client_problem("api_blob_size_invalid", 400));
        }
        let path = format!(
            "/v1/workspaces/{}/resources/{}/blobs?key={}{}",
            scope.workspace_id.as_str(),
            resource.as_str(),
            urlencoding(key),
            if repair_stale_proxy {
                "&repairStaleProxy=true"
            } else {
                ""
            }
        );
        let url = self
            .base_url
            .join(path.trim_start_matches('/'))
            .map_err(|_| client_problem("api_url_invalid", 400))?;
        let response = self
            .http
            .get(url)
            .bearer_auth(scope.auth.bearer_token())
            .header("x-correlation-id", correlation)
            .header("x-buglogin-actor-id", scope.auth.actor_id())
            .header("x-buglogin-generation", generation)
            .header("x-content-sha256", checksum)
            .send()
            .await
            .map_err(|_| {
                Problem::new(
                    "api_unavailable",
                    "BugLogin API is unavailable",
                    503,
                    correlation,
                )
                .retryable(true)
            })?;
        if !response.status().is_success() {
            return Err(read_problem(response, correlation, true).await);
        }
        if response
            .content_length()
            .is_some_and(|content_length| content_length > expected_size)
        {
            return Err(client_problem("api_blob_integrity_failed", 502));
        }
        let capacity = usize::try_from(expected_size)
            .map_err(|_| client_problem("api_blob_size_invalid", 400))?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(capacity)
            .map_err(|_| client_problem("api_blob_capacity_exceeded", 503))?;
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| {
                Problem::new(
                    "api_response_invalid",
                    "BugLogin API response is incomplete or invalid",
                    502,
                    correlation,
                )
                .retryable(true)
            })?;
            let new_len = bytes
                .len()
                .checked_add(chunk.len())
                .ok_or_else(|| client_problem("api_blob_integrity_failed", 502))?;
            if new_len > capacity {
                return Err(client_problem("api_blob_integrity_failed", 502));
            }
            bytes.extend_from_slice(&chunk);
        }
        if bytes.len() as u64 != expected_size || sha256_hex(&bytes) != checksum {
            return Err(client_problem("api_blob_integrity_failed", 502));
        }
        Ok(bytes)
    }

    pub async fn runtime_started(
        &self,
        profile: &ProfileId,
        device_id: &str,
        generation: u64,
        checksum: &str,
        lease: &LeaseGrant,
        idem: &str,
        correlation: &str,
    ) -> Result<(), Problem> {
        self.runtime_projection(
            "started",
            profile,
            device_id,
            generation,
            checksum,
            lease,
            idem,
            correlation,
        )
        .await
    }
    pub async fn runtime_stopped(
        &self,
        profile: &ProfileId,
        device_id: &str,
        generation: u64,
        checksum: &str,
        lease: &LeaseGrant,
        idem: &str,
        correlation: &str,
    ) -> Result<(), Problem> {
        self.runtime_projection(
            "stopped",
            profile,
            device_id,
            generation,
            checksum,
            lease,
            idem,
            correlation,
        )
        .await
    }
    async fn runtime_projection(
        &self,
        state: &str,
        profile: &ProfileId,
        device_id: &str,
        generation: u64,
        checksum: &str,
        lease: &LeaseGrant,
        idem: &str,
        correlation: &str,
    ) -> Result<(), Problem> {
        let path = format!("/v1/profiles/{}/runtime/{}", profile.as_str(), state);
        let extra = [
            ("x-buglogin-owner-id", lease.owner_id.clone()),
            ("x-buglogin-owner-token", lease.owner_token.clone()),
            ("x-buglogin-fencing-token", lease.fencing_token.to_string()),
        ];
        let _: Value = self
            .send(
                Method::PUT,
                &path,
                Some(json!({"deviceId":device_id,"generation":generation,"rootChecksum":checksum})),
                correlation,
                Some(idem),
                None,
                &extra,
            )
            .await?;
        Ok(())
    }

    pub async fn invoke(
        &self,
        name: &str,
        correlation: &str,
        idem: Option<&str>,
        payload: Value,
    ) -> Result<buglogin_api::commands::CommandResult<Value>, Problem> {
        let result = match name {
            "system_health_get" => self.get::<Value>("/v1/health",correlation).await?,
            "account_get" => self.get::<Value>("/v1/account",correlation).await?,
            "account_list" => json!({"items":self.get::<Vec<Value>>("/v1/accounts",correlation).await?,"nextCursor":Value::Null}),
            "account_status_update" => {
                let account_id = AccountId::from_str(&path_arg(&payload, "accountId", correlation)?)
                    .map_err(|_| bad_input("accountId", correlation))?;
                let body = json!({
                    "status": payload.get("status").cloned().ok_or_else(|| bad_input("status", correlation))?,
                    "expectedRevision": payload.get("expectedRevision").and_then(Value::as_u64).ok_or_else(|| bad_input("expectedRevision", correlation))?,
                });
                self.send::<Value>(Method::PUT, &format!("/v1/accounts/{}/status", account_id.as_str()), Some(body), correlation, idem, None, &[]).await?
            },
            "organization_list" => json!({"items":self.get::<Vec<Value>>("/v1/organizations",correlation).await?,"nextCursor":Value::Null}),
            "organization_get" => { let id=path_arg(&payload,"organizationId",correlation)?; self.get::<Value>(&format!("/v1/organizations/{id}"),correlation).await? },
            "organization_record_get" => { let id=path_arg(&payload,"organizationId",correlation)?; self.get::<Value>(&format!("/v1/organizations/{id}/record"),correlation).await? },
            "organization_create" => self.send::<Value>(Method::POST,"/v1/organizations",Some(payload),correlation,idem,None,&[]).await?,
            "organization_update" => { let id=path_arg(&payload,"organizationId",correlation)?; let body=payload.get("update").cloned().ok_or_else(||bad_input("update",correlation))?; self.send::<Value>(Method::PATCH,&format!("/v1/organizations/{id}"),Some(body),correlation,idem,None,&[]).await? },
            "workspace_list" => { let org=path_arg(&payload,"organizationId",correlation)?; json!({"items":self.get::<Vec<Value>>(&format!("/v1/organizations/{org}/workspaces"),correlation).await?,"nextCursor":Value::Null}) },
            "workspace_get" => { let org=path_arg(&payload,"organizationId",correlation)?; let id=path_arg(&payload,"workspaceId",correlation)?; self.get::<Value>(&format!("/v1/organizations/{org}/workspaces/{id}"),correlation).await? },
            "workspace_record_get" => { let org=path_arg(&payload,"organizationId",correlation)?; let id=path_arg(&payload,"workspaceId",correlation)?; self.get::<Value>(&format!("/v1/organizations/{org}/workspaces/{id}/record"),correlation).await? },
            "workspace_create" => { let org=path_arg(&payload,"organizationId",correlation)?; let body=payload.get("workspace").cloned().ok_or_else(||bad_input("workspace",correlation))?; self.send::<Value>(Method::POST,&format!("/v1/organizations/{org}/workspaces"),Some(body),correlation,idem,None,&[]).await? },
            "workspace_update" => {
                let org = path_arg(&payload, "organizationId", correlation)?;
                let id = path_arg(&payload, "workspaceId", correlation)?;
                let update = payload
                    .get("update")
                    .ok_or_else(|| bad_input("update", correlation))?;
                let body = if update.get("workspace").is_some()
                    || update.get("expectedRevision").is_some()
                {
                    canonical_workspace_update(update, correlation)?
                } else {
                    let record = self
                        .get::<Value>(
                            &format!("/v1/organizations/{org}/workspaces/{id}/record"),
                            correlation,
                        )
                        .await?;
                    legacy_workspace_update(update, &record, &org, &id, correlation)?
                };
                self.send::<Value>(
                    Method::PATCH,
                    &format!("/v1/organizations/{org}/workspaces/{id}"),
                    Some(body),
                    correlation,
                    idem,
                    None,
                    &[],
                )
                .await?
            },
            "group_list" => { let ws=path_arg(&payload,"workspaceId",correlation)?; json!({"items":self.get::<Vec<Value>>(&format!("/v1/workspaces/{ws}/groups"),correlation).await?,"nextCursor":Value::Null}) },
            "group_get" => { let ws=path_arg(&payload,"workspaceId",correlation)?; let id=path_arg(&payload,"groupId",correlation)?; self.get::<Value>(&format!("/v1/workspaces/{ws}/groups/{id}"),correlation).await? },
            "group_create" => { let ws=path_arg(&payload,"workspaceId",correlation)?; self.send::<Value>(Method::POST,&format!("/v1/workspaces/{ws}/groups"),Some(payload),correlation,idem,None,&[]).await? },
            "group_update" => { let ws=path_arg(&payload,"workspaceId",correlation)?; let id=path_arg(&payload,"groupId",correlation)?; self.send::<Value>(Method::PUT,&format!("/v1/workspaces/{ws}/groups/{id}"),Some(payload),correlation,idem,None,&[]).await? },
            "group_delete" => { let ws=path_arg(&payload,"workspaceId",correlation)?; let id=path_arg(&payload,"groupId",correlation)?; self.send::<Value>(Method::DELETE,&format!("/v1/workspaces/{ws}/groups/{id}"),Some(payload),correlation,idem,None,&[]).await? },
            "extension_group_list" => { let ws=path_arg(&payload,"workspaceId",correlation)?; json!(self.get::<Vec<Value>>(&format!("/v1/workspaces/{ws}/extension-groups"),correlation).await?) },
            "extension_group_get" => { let ws=path_arg(&payload,"workspaceId",correlation)?; let id=path_arg(&payload,"extensionGroupId",correlation)?; self.get::<Value>(&format!("/v1/workspaces/{ws}/extension-groups/{id}"),correlation).await? },
            "extension_group_create" => { let ws=path_arg(&payload,"workspaceId",correlation)?; self.send::<Value>(Method::POST,&format!("/v1/workspaces/{ws}/extension-groups"),Some(payload),correlation,idem,None,&[]).await? },
            "extension_group_update" => { let ws=path_arg(&payload,"workspaceId",correlation)?; let id=path_arg(&payload,"extensionGroupId",correlation)?; self.send::<Value>(Method::PUT,&format!("/v1/workspaces/{ws}/extension-groups/{id}"),Some(payload),correlation,idem,None,&[]).await? },
            "extension_group_delete" => { let ws=path_arg(&payload,"workspaceId",correlation)?; let id=path_arg(&payload,"extensionGroupId",correlation)?; self.send::<Value>(Method::DELETE,&format!("/v1/workspaces/{ws}/extension-groups/{id}"),Some(payload),correlation,idem,None,&[]).await? },
            "profile_extension_group_set" => { let id=path_arg(&payload,"profileId",correlation)?; self.send::<Value>(Method::PUT,&format!("/v1/profiles/{id}/extension-group"),Some(payload),correlation,idem,None,&[]).await? },
            "group_share_list" => { let ws=path_arg(&payload,"workspaceId",correlation)?; let id=path_arg(&payload,"groupId",correlation)?; self.get::<Value>(&format!("/v1/workspaces/{ws}/groups/{id}/shares"),correlation).await? },
            "group_share_create" => { let ws=path_arg(&payload,"workspaceId",correlation)?; let id=path_arg(&payload,"groupId",correlation)?; self.send::<Value>(Method::POST,&format!("/v1/workspaces/{ws}/groups/{id}/shares"),Some(payload),correlation,idem,None,&[]).await? },
            "group_share_revoke" => { let ws=path_arg(&payload,"workspaceId",correlation)?; let id=path_arg(&payload,"groupId",correlation)?; let grant=path_arg(&payload,"grantId",correlation)?; self.send::<Value>(Method::DELETE,&format!("/v1/workspaces/{ws}/groups/{id}/shares/{grant}"),Some(payload),correlation,idem,None,&[]).await? },
            "profile_groups_replace" => { let id=path_arg(&payload,"profileId",correlation)?; self.send::<Value>(Method::PUT,&format!("/v1/profiles/{id}/groups"),Some(payload),correlation,idem,None,&[]).await? },
            "operation_get" => { let id=path_arg(&payload,"operationId",correlation)?; self.get::<Value>(&format!("/v1/operations/{id}"),correlation).await? },
            "profile_binding_get" => { let id=path_arg(&payload,"profileId",correlation)?; self.get::<Value>(&format!("/v1/profiles/{id}/binding"),correlation).await? },
            "profile_proxy_assignment_get" => { let ws=path_arg(&payload,"workspaceId",correlation)?; let id=path_arg(&payload,"profileId",correlation)?; self.get::<Value>(&format!("/v1/workspaces/{ws}/profiles/{id}/proxy-assignment"),correlation).await? },
            "profile_proxy_assignment_set" => { let ws=path_arg(&payload,"workspaceId",correlation)?; let id=path_arg(&payload,"profileId",correlation)?; self.send::<Value>(Method::PUT,&format!("/v1/workspaces/{ws}/profiles/{id}/proxy-assignment"),Some(payload),correlation,idem,None,&[]).await? },
            "profile_list" => {
                let include_archived = optional_bool(&payload, "includeArchived", false, correlation)?;
                let limit = optional_page_limit(&payload, correlation)? as usize;
                let workspace_id = payload
                    .get("workspaceId")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                if workspace_id
                    .as_deref()
                    .is_some_and(|workspace_id| WorkspaceId::from_str(workspace_id).is_err())
                {
                    return Err(bad_input("workspaceId", correlation));
                }
                let mut scan_cursor = optional_cursor(&payload, correlation)?;
                if scan_cursor
                    .as_deref()
                    .is_some_and(|cursor| ProfileId::from_str(cursor).is_err())
                {
                    return Err(bad_input("cursor", correlation));
                }
                let mut seen_cursors = BTreeSet::new();
                let mut profiles = Vec::with_capacity(limit + 1);
                loop {
                    let mut path = match scan_cursor.as_deref() {
                        Some(cursor) => format!("/v1/profiles?limit=100&cursor={}", urlencoding(cursor)),
                        None => "/v1/profiles?limit=100".to_owned(),
                    };
                    if let Some(workspace_id) = workspace_id.as_deref() {
                        path.push_str("&workspaceId=");
                        path.push_str(&urlencoding(workspace_id));
                    }
                    if include_archived {
                        path.push_str("&includeArchived=true");
                    }
                    let page: VersionedProfilePage = self.get(&path, correlation).await?;
                    let next_scan_cursor = page.next_cursor;
                    for profile in page.items {
                        profiles.push(profile);
                        if profiles.len() > limit {
                            break;
                        }
                    }
                    if profiles.len() > limit {
                        break;
                    }
                    match next_scan_cursor {
                        Some(next)
                            if next.len() <= 512 && seen_cursors.insert(next.clone()) =>
                        {
                            scan_cursor = Some(next)
                        }
                        Some(_) => return Err(client_problem("api_profile_cursor_invalid", 502)),
                        None => break,
                    }
                }
                let has_more = profiles.len() > limit;
                profiles.truncate(limit);
                let next_cursor = has_more
                    .then(|| profiles.last().map(|profile| profile.value.id.as_str().to_owned()))
                    .flatten();
                let items = profiles
                    .into_iter()
                    .map(profile_record_value)
                    .collect::<Result<Vec<_>, Problem>>()?;
                json!({"items":items,"nextCursor":next_cursor})
            },
            "profile_get" => { let id=path_arg(&payload,"profileId",correlation)?; let record:Versioned<Profile>=self.get(&format!("/v1/profiles/{id}"),correlation).await?; profile_record_value(record)? },
            "profile_create" => self.create_managed_profile(payload,correlation,idem).await?,
            "profile_update" => { let ws=path_arg(&payload,"workspaceId",correlation)?; let id=path_arg(&payload,"profileId",correlation)?; self.send::<Value>(Method::PUT,&format!("/v1/workspaces/{ws}/profiles/{id}"),Some(payload),correlation,idem,None,&[]).await? },
            "profile_delete" => {
                let id = path_arg(&payload, "profileId", correlation)?;
                let revision = payload
                    .get("revision")
                    .and_then(Value::as_u64)
                    .filter(|revision| *revision > 0)
                    .ok_or_else(|| bad_input("revision", correlation))?;
                let confirmation = required_string(&payload, "confirmation", correlation)?;
                if confirmation != "QUARANTINE_PROFILE_DATA" {
                    return Err(bad_input("confirmation", correlation));
                }
                let body = json!({
                    "revision": revision,
                    "confirmation": confirmation,
                });
                self.send::<Value>(
                    Method::DELETE,
                    &format!("/v1/profiles/{id}"),
                    Some(body),
                    correlation,
                    idem,
                    None,
                    &[],
                )
                .await?
            },
            "profile_archive" | "profile_restore" => { let id=path_arg(&payload,"profileId",correlation)?; let action=if name=="profile_archive" {"archive"} else {"restore"}; let body=json!({"revision":payload.get("revision").and_then(Value::as_u64).ok_or_else(||bad_input("revision",correlation))?}); let updated:Versioned<Profile>=self.send(Method::PUT,&format!("/v1/profiles/{id}/{action}"),Some(body),correlation,idem,None,&[]).await?; profile_record_value(updated)? },
            "profile_clone" => { let id=path_arg(&payload,"sourceProfileId",correlation)?; self.send::<Value>(Method::POST,&format!("/v1/profiles/{id}/clone"),Some(payload),correlation,idem,None,&[]).await? },
            "profile_share" => { let id=path_arg(&payload,"profileId",correlation)?; self.send::<Value>(Method::POST,&format!("/v1/profiles/{id}/shares"),Some(payload),correlation,idem,None,&[]).await? },
            "profile_share_revoke" => { let id=path_arg(&payload,"profileId",correlation)?; let member=path_arg(&payload,"memberId",correlation)?; self.send::<Value>(Method::DELETE,&format!("/v1/profiles/{id}/shares/{member}"),Some(payload),correlation,idem,None,&[]).await? },
            "profile_share_recipientdevicekeys_list" => { let id=path_arg(&payload,"profileId",correlation)?; let member=path_arg(&payload,"memberId",correlation)?; json!(self.get::<Vec<DeviceEncryptionKey>>(&format!("/v1/profiles/{id}/shares/{member}/device-keys"),correlation).await?) },
            "profile_keyrotation_recipients_list" => { let id=path_arg(&payload,"profileId",correlation)?; json!(self.get::<Vec<ProfileKeyRotationRecipientDevices>>(&format!("/v1/profiles/{id}/key-rotation-recipients"),correlation).await?) },
            "proxy_parse" => { let text=payload.get("text").and_then(Value::as_str).ok_or_else(||bad_input("text",correlation))?; self.send_text("/v1/proxies/parse",text.to_owned(),correlation).await? },
            "proxy_list" => { let ws=path_arg(&payload,"workspaceId",correlation)?; self.get::<Value>(&format!("/v1/workspaces/{ws}/proxies"),correlation).await? },
            "proxy_get" => { let ws=path_arg(&payload,"workspaceId",correlation)?; let id=path_arg(&payload,"proxyId",correlation)?; self.get::<Value>(&format!("/v1/workspaces/{ws}/proxies/{id}"),correlation).await? },
            "proxy_create" => { let ws=path_arg(&payload,"workspaceId",correlation)?; self.send::<Value>(Method::POST,&format!("/v1/workspaces/{ws}/proxies"),Some(payload),correlation,idem,None,&[]).await? },
            "proxy_update" => { let ws=path_arg(&payload,"workspaceId",correlation)?; let id=path_arg(&payload,"proxyId",correlation)?; self.send::<Value>(Method::PATCH,&format!("/v1/workspaces/{ws}/proxies/{id}"),Some(payload),correlation,idem,None,&[]).await? },
            "proxy_delete" => { let ws=path_arg(&payload,"workspaceId",correlation)?; let id=path_arg(&payload,"proxyId",correlation)?; self.send::<Value>(Method::DELETE,&format!("/v1/workspaces/{ws}/proxies/{id}"),Some(payload),correlation,idem,None,&[]).await? },
            "team_list" => { let ws=path_arg(&payload,"workspaceId",correlation)?; self.get::<Value>(&format!("/v1/workspaces/{ws}/teams"),correlation).await? },
            "team_get" => { let ws=path_arg(&payload,"workspaceId",correlation)?; let id=path_arg(&payload,"teamId",correlation)?; self.get::<Value>(&format!("/v1/workspaces/{ws}/teams/{id}"),correlation).await? },
            "team_create" => { let ws=path_arg(&payload,"workspaceId",correlation)?; let body=payload.get("team").cloned().ok_or_else(||bad_input("team",correlation))?; self.send::<Value>(Method::POST,&format!("/v1/workspaces/{ws}/teams"),Some(body),correlation,idem,None,&[]).await? },
            "team_update" => { let ws=path_arg(&payload,"workspaceId",correlation)?; let id=path_arg(&payload,"teamId",correlation)?; let body=payload.get("update").cloned().ok_or_else(||bad_input("update",correlation))?; self.send::<Value>(Method::PUT,&format!("/v1/workspaces/{ws}/teams/{id}"),Some(body),correlation,idem,None,&[]).await? },
            "team_delete" => { let ws=path_arg(&payload,"workspaceId",correlation)?; let id=path_arg(&payload,"teamId",correlation)?; let body=json!({"expectedRevision":payload.get("expectedRevision").and_then(Value::as_u64).ok_or_else(||bad_input("expectedRevision",correlation))?}); self.send::<Value>(Method::DELETE,&format!("/v1/workspaces/{ws}/teams/{id}"),Some(body),correlation,idem,None,&[]).await? },
            "member_list" => {
                let ws = path_arg(&payload, "workspaceId", correlation)?;
                let limit = optional_page_limit(&payload, correlation)?;
                let mut path = format!("/v1/workspaces/{ws}/members/page?limit={limit}");
                if let Some(cursor) = optional_cursor(&payload, correlation)? {
                    MemberId::from_str(&cursor).map_err(|_| bad_input("cursor", correlation))?;
                    path.push_str("&cursor=");
                    path.push_str(&urlencoding(&cursor));
                }
                self.get::<Value>(&path, correlation).await?
            },
            "member_get" => { let ws=path_arg(&payload,"workspaceId",correlation)?; let id=path_arg(&payload,"memberId",correlation)?; self.get::<Value>(&format!("/v1/workspaces/{ws}/members/{id}"),correlation).await? },
            "member_invite" => { let ws=path_arg(&payload,"workspaceId",correlation)?; let body=payload.get("member").cloned().ok_or_else(||bad_input("member",correlation))?; self.send::<Value>(Method::POST,&format!("/v1/workspaces/{ws}/members"),Some(body),correlation,idem,None,&[]).await? },
            "member_invitation_create" => { let ws=path_arg(&payload,"workspaceId",correlation)?; let body=json!({"email":payload.get("email").and_then(Value::as_str).ok_or_else(||bad_input("email",correlation))?,"role":payload.get("role").cloned().ok_or_else(||bad_input("role",correlation))?,"expiresAt":payload.get("expiresAt").and_then(Value::as_u64).ok_or_else(||bad_input("expiresAt",correlation))?}); self.send::<Value>(Method::POST,&format!("/v1/workspaces/{ws}/invitations"),Some(body),correlation,idem,None,&[]).await? },
            "member_invitation_redeem" => { let id=path_arg(&payload,"invitationId",correlation)?; let body=json!({"code":payload.get("code").and_then(Value::as_str).ok_or_else(||bad_input("code",correlation))?}); self.send::<Value>(Method::POST,&format!("/v1/workspace-invitations/{id}/redeem"),Some(body),correlation,idem,None,&[]).await? },
            "member_invitation_revoke" => { let ws=path_arg(&payload,"workspaceId",correlation)?; let id=path_arg(&payload,"invitationId",correlation)?; self.send::<Value>(Method::DELETE,&format!("/v1/workspaces/{ws}/invitations/{id}"),None,correlation,idem,None,&[]).await? },
            "member_activate" | "member_suspend" => { let ws=path_arg(&payload,"workspaceId",correlation)?; let id=path_arg(&payload,"memberId",correlation)?; let action=if name=="member_activate" {"activate"} else {"suspend"}; self.send::<Value>(Method::PUT,&format!("/v1/workspaces/{ws}/members/{id}/{action}"),None,correlation,idem,None,&[]).await? },
            "member_role_update" => { let ws=path_arg(&payload,"workspaceId",correlation)?; let id=path_arg(&payload,"memberId",correlation)?; let body=json!({"role":payload.get("role").cloned().ok_or_else(||bad_input("role",correlation))?}); self.send::<Value>(Method::PUT,&format!("/v1/workspaces/{ws}/members/{id}/role"),Some(body),correlation,idem,None,&[]).await? },
            "resource_list" => {
                let ws = path_arg(&payload, "workspaceId", correlation)?;
                let limit = optional_page_limit(&payload, correlation)?;
                let mut path = format!("/v1/workspaces/{ws}/resources/page?limit={limit}");
                if let Some(cursor) = optional_cursor(&payload, correlation)? {
                    ResourceId::from_str(&cursor).map_err(|_| bad_input("cursor", correlation))?;
                    path.push_str("&cursor=");
                    path.push_str(&urlencoding(&cursor));
                }
                self.get::<Value>(&path, correlation).await?
            },
            "resource_get" => { let ws=path_arg(&payload,"workspaceId",correlation)?; let id=path_arg(&payload,"resourceId",correlation)?; self.get::<Value>(&format!("/v1/workspaces/{ws}/resources/{id}"),correlation).await? },
            "resource_lease_get" => {
                let ws = path_arg(&payload, "workspaceId", correlation)?;
                let id = path_arg(&payload, "resourceId", correlation)?;
                let account: Value = self.get("/v1/account", correlation).await?;
                let account_id = account
                    .get("accountId")
                    .and_then(Value::as_str)
                    .ok_or_else(|| bad_input("accountId", correlation))?;
                let account_id = AccountId::from_str(account_id)
                    .map_err(|_| bad_input("accountId", correlation))?;
                let token = self.token_value()?;
                let auth = CoordinationAuth::new(account_id.as_str(), token.to_string())?;
                self.send::<Value>(
                    Method::GET,
                    &format!("/v1/workspaces/{ws}/resources/{id}/lease"),
                    None,
                    correlation,
                    None,
                    Some(&auth),
                    &[("x-buglogin-actor-id", auth.actor_id().to_owned())],
                )
                .await?
            }
            "profile_runtime_get" => {
                let profile_id = path_arg(&payload, "profileId", correlation)?;
                self.get::<Value>(
                    &format!("/v1/profiles/{profile_id}/runtime"),
                    correlation,
                )
                .await?
            }
            "resource_register" => { let resource=payload.get("resource").cloned().ok_or_else(||bad_input("resource",correlation))?; let ws=resource.get("workspaceId").and_then(Value::as_str).filter(|value|!value.is_empty()).ok_or_else(||bad_input("resource.workspaceId",correlation))?; let ws=path_arg(&json!({"workspaceId":ws}),"workspaceId",correlation)?; self.send::<Value>(Method::POST,&format!("/v1/workspaces/{ws}/resources"),Some(resource),correlation,idem,None,&[]).await? },
            "cookie_list" => { let id=path_arg(&payload,"profileId",correlation)?; let ws=path_arg(&payload,"workspaceId",correlation)?; self.get::<Value>(&format!("/v1/profiles/{id}/cookies?workspaceId={ws}"),correlation).await? },
            "cookie_stage" | "cookie_import" | "cookie_copy" => {
                return Err(Problem::new(
                    "production_cookie_device_dispatch_required",
                    "Cookie mutations must execute on the authorized connected device",
                    503,
                    correlation,
                ).retryable(true));
            }
            "grant_list" => {
                let ws = path_arg(&payload, "workspaceId", correlation)?;
                let mut path = format!("/v1/workspaces/{ws}/grants");
                let mut query = Vec::new();
                for key in ["resourceId", "memberId"] {
                    if let Some(value) = payload.get(key).and_then(Value::as_str) {
                        query.push(format!("{key}={}", urlencoding(value)));
                    }
                }
                if !query.is_empty() {
                    path.push('?');
                    path.push_str(&query.join("&"));
                }
                self.get::<Value>(&path, correlation).await?
            },
            "grant_list_page" => {
                let ws = path_arg(&payload, "workspaceId", correlation)?;
                let limit = optional_page_limit(&payload, correlation)?;
                let mut path = format!("/v1/workspaces/{ws}/grants/page?limit={limit}");
                let mut query = Vec::new();
                for key in ["resourceId", "memberId"] {
                    if let Some(value) = payload.get(key).and_then(Value::as_str) {
                        query.push(format!("{key}={}", urlencoding(value)));
                    }
                }
                if let Some(cursor) = optional_cursor(&payload, correlation)? {
                    GrantId::from_str(&cursor).map_err(|_| bad_input("cursor", correlation))?;
                    query.push(format!("cursor={}", urlencoding(&cursor)));
                }
                if !query.is_empty() {
                    path.push('&');
                    path.push_str(&query.join("&"));
                }
                self.get::<Value>(&path, correlation).await?
            },
            "grant_create" => { let ws=path_arg(&payload,"workspaceId",correlation)?; self.send(Method::POST,&format!("/v1/workspaces/{ws}/grants"),Some(payload),correlation,idem,None,&[]).await? },
            "grant_revoke" => { let ws=path_arg(&payload,"workspaceId",correlation)?;let id=path_arg(&payload,"grantId",correlation)?;self.send(Method::DELETE,&format!("/v1/workspaces/{ws}/grants/{id}"),None,correlation,idem,None,&[]).await? },
            "migration_inspect" | "migration_import" => return Err(Problem::new("production_migration_device_dispatch_required","Migration commands must execute against an authorized local device, not the API server filesystem",503,correlation).retryable(true)),
            _ => return Err(Problem::new("api_command_unsupported","This command has no verified REST mapping",501,correlation).with_field("command",name)),
        };
        Ok(buglogin_api::commands::CommandResult {
            command_id: command_id(name).to_owned(),
            correlation_id: correlation.to_owned(),
            data: result,
        })
    }

    fn token_value(&self) -> Result<Zeroizing<String>, Problem> {
        self.tokens
            .read()
            .map_err(|_| client_problem("api_client_unavailable", 503))?
            .current
            .clone()
            .ok_or_else(|| {
                Problem::new(
                    "authentication_required",
                    "Sign in to BugLogin",
                    401,
                    "desktop",
                )
            })
    }

    fn preferred_token(&self) -> Result<Option<Zeroizing<String>>, Problem> {
        let tokens = self
            .tokens
            .read()
            .map_err(|_| client_problem("api_client_unavailable", 503))?;
        Ok(tokens.refreshed.clone().or_else(|| tokens.current.clone()))
    }

    async fn create_managed_profile(
        &self,
        input: Value,
        correlation: &str,
        idem: Option<&str>,
    ) -> Result<Value, Problem> {
        let mut input: ProfileCreateInput =
            serde_json::from_value(input).map_err(|_| bad_input("profile", correlation))?;
        input.name = input.name.trim().to_owned();
        if input.name.is_empty() {
            return Err(bad_input("name", correlation));
        }
        let workspace = input.workspace_id.clone();
        let key = idem.ok_or_else(|| {
            Problem::new(
                "idempotency_key_required",
                "A profile create key is required",
                400,
                correlation,
            )
        })?;
        let expected_id = input
            .id
            .as_deref()
            .map(ProfileId::from_str)
            .transpose()
            .map_err(|_| client_problem("api_request_invalid", 400))?;
        let extra = workspace
            .map(|workspace| vec![("x-buglogin-workspace-id", workspace)])
            .unwrap_or_default();
        let profile =
            serde_json::to_value(input).map_err(|_| client_problem("api_request_invalid", 400))?;
        let created: Versioned<Profile> = self
            .send(
                Method::POST,
                "/v1/profiles",
                Some(profile),
                correlation,
                Some(key),
                None,
                &extra,
            )
            .await?;
        if expected_id
            .as_ref()
            .is_some_and(|expected| created.value.id != *expected)
            || created.value.state != ProfileState::Ready
            || created.value.sync_mode != Some(SyncMode::Managed)
        {
            return Err(Problem::new(
                "api_profile_provision_response_invalid",
                "BugLogin returned a profile that does not match the managed creation request",
                502,
                correlation,
            ));
        }
        profile_record_value(created)
    }
}

fn profile_record_value(profile: Versioned<Profile>) -> Result<Value, Problem> {
    serde_json::to_value(ProfileRecord::new(profile.value, profile.revision))
        .map_err(|_| client_problem("api_response_invalid", 502))
}

#[async_trait::async_trait]
impl DesktopSyncBackend for RemoteApiClient {
    async fn acquire_lease_idempotent(
        &self,
        scope: CoordinationScope<'_>,
        resource: &ResourceId,
        ttl: Duration,
        idem: &str,
    ) -> Result<LeaseGrant, Problem> {
        self.send(
            Method::POST,
            &format!(
                "/v1/workspaces/{}/resources/{}/lease",
                scope.workspace_id.as_str(),
                resource.as_str()
            ),
            Some(json!({"ownerId":scope.auth.actor_id(),"ttlSeconds":ttl.as_secs()})),
            "desktop-lease",
            Some(idem),
            Some(scope.auth),
            &[],
        )
        .await
    }
    async fn renew_lease_idempotent(
        &self,
        scope: CoordinationScope<'_>,
        resource: &ResourceId,
        owner_token: &str,
        ttl: Duration,
        idempotency_key: &str,
    ) -> Result<LeaseGrant, Problem> {
        let grant:LeaseGrant=self.send(Method::PUT,&format!("/v1/workspaces/{}/resources/{}/lease",scope.workspace_id.as_str(),resource.as_str()),Some(json!({"ownerId":scope.auth.actor_id(),"ownerToken":owner_token,"ttlSeconds":ttl.as_secs()})),"desktop-lease",Some(idempotency_key),Some(scope.auth),&[]).await?;
        Ok(grant)
    }
    async fn release_lease_idempotent(
        &self,
        scope: CoordinationScope<'_>,
        resource: &ResourceId,
        owner_token: &str,
        idempotency_key: &str,
    ) -> Result<(), Problem> {
        let _: Value = self
            .send(
                Method::DELETE,
                &format!(
                    "/v1/workspaces/{}/resources/{}/lease",
                    scope.workspace_id.as_str(),
                    resource.as_str()
                ),
                Some(json!({"ownerId":scope.auth.actor_id(),"ownerToken":owner_token})),
                "desktop-lease",
                Some(idempotency_key),
                Some(scope.auth),
                &[],
            )
            .await?;
        Ok(())
    }
    async fn current_manifest(
        &self,
        scope: CoordinationScope<'_>,
        resource: &ResourceId,
    ) -> Result<Option<SyncManifest>, Problem> {
        self.send(
            Method::GET,
            &format!(
                "/v1/workspaces/{}/resources/{}/manifest",
                scope.workspace_id.as_str(),
                resource.as_str()
            ),
            None,
            "desktop-sync",
            None,
            Some(scope.auth),
            &[("x-buglogin-actor-id", scope.auth.actor_id().to_owned())],
        )
        .await
    }

    async fn commit_manifest(
        &self,
        scope: CoordinationScope<'_>,
        resource: &ResourceId,
        owner_token: &str,
        expected_profile_revision: Option<u64>,
        manifest: SyncManifest,
    ) -> Result<(), Problem> {
        self.commit_manifest_request(
            scope,
            resource,
            owner_token,
            expected_profile_revision,
            manifest,
            None,
        )
        .await
    }
}

impl RemoteApiClient {
    pub async fn current_manifest_for_proxy_repair(
        &self,
        scope: CoordinationScope<'_>,
        resource: &ResourceId,
        correlation: &str,
    ) -> Result<Option<SyncManifest>, Problem> {
        self.send(
            Method::GET,
            &format!(
                "/v1/workspaces/{}/resources/{}/manifest?repairStaleProxy=true",
                scope.workspace_id.as_str(),
                resource.as_str()
            ),
            None,
            correlation,
            None,
            Some(scope.auth),
            &[("x-buglogin-actor-id", scope.auth.actor_id().to_owned())],
        )
        .await
    }

    async fn commit_manifest_request(
        &self,
        scope: CoordinationScope<'_>,
        resource: &ResourceId,
        owner_token: &str,
        expected_profile_revision: Option<u64>,
        manifest: SyncManifest,
        key_share_envelope_updates: Option<&[ProfileKeyRotationEnvelopeUpdate]>,
    ) -> Result<(), Problem> {
        let mut extra = vec![
            ("x-buglogin-owner-id", scope.auth.actor_id().to_owned()),
            ("x-buglogin-owner-token", owner_token.to_owned()),
            (
                "x-buglogin-fencing-token",
                manifest.fencing_token.to_string(),
            ),
        ];
        if let Some(revision) = expected_profile_revision {
            extra.push(("x-buglogin-profile-revision", revision.to_string()));
        }
        let body = if let Some(updates) = key_share_envelope_updates {
            json!({
                "manifest": manifest,
                "keyShareEnvelopeUpdates": updates,
            })
        } else {
            serde_json::to_value(manifest.clone())
                .map_err(|_| client_problem("api_request_invalid", 400))?
        };
        let idem = manifest_commit_idempotency_key(
            expected_profile_revision,
            &manifest,
            key_share_envelope_updates,
        )?;
        let _: SyncManifest = self
            .send(
                Method::PUT,
                &format!(
                    "/v1/workspaces/{}/resources/{}/manifest",
                    scope.workspace_id.as_str(),
                    resource.as_str()
                ),
                Some(body),
                "desktop-sync",
                Some(&idem),
                Some(scope.auth),
                &extra,
            )
            .await?;
        Ok(())
    }

    /// Reconcile a retryable manifest commit failure before deciding whether the
    /// stopped profile still needs another sync attempt. A timed-out response may
    /// mean the server activated the generation even though the client saw an error.
    pub async fn commit_manifest_with_reconciliation(
        &self,
        scope: CoordinationScope<'_>,
        resource: &ResourceId,
        owner_token: &str,
        expected_profile_revision: Option<u64>,
        manifest: SyncManifest,
        correlation: &str,
    ) -> Result<(), Problem> {
        self.commit_manifest_with_updates_reconciliation(
            scope,
            resource,
            owner_token,
            expected_profile_revision,
            manifest,
            None,
            correlation,
        )
        .await
    }

    pub async fn commit_manifest_with_key_rotation_reconciliation(
        &self,
        scope: CoordinationScope<'_>,
        resource: &ResourceId,
        owner_token: &str,
        expected_profile_revision: Option<u64>,
        manifest: SyncManifest,
        key_share_envelope_updates: &[ProfileKeyRotationEnvelopeUpdate],
        correlation: &str,
    ) -> Result<(), Problem> {
        self.commit_manifest_with_updates_reconciliation(
            scope,
            resource,
            owner_token,
            expected_profile_revision,
            manifest,
            Some(key_share_envelope_updates),
            correlation,
        )
        .await
    }

    async fn commit_manifest_with_updates_reconciliation(
        &self,
        scope: CoordinationScope<'_>,
        resource: &ResourceId,
        owner_token: &str,
        expected_profile_revision: Option<u64>,
        manifest: SyncManifest,
        key_share_envelope_updates: Option<&[ProfileKeyRotationEnvelopeUpdate]>,
        correlation: &str,
    ) -> Result<(), Problem> {
        match self
            .commit_manifest_request(
                scope.clone(),
                resource,
                owner_token,
                expected_profile_revision,
                manifest.clone(),
                key_share_envelope_updates,
            )
            .await
        {
            Ok(()) => Ok(()),
            Err(commit_error) if commit_error.retryable || commit_error.status >= 500 => {
                match DesktopSyncBackend::current_manifest(self, scope, resource).await {
                    Ok(Some(active))
                        if active.workspace_id == manifest.workspace_id
                            && active.resource_id == manifest.resource_id
                            && active.generation == manifest.generation
                            && active.root_checksum == manifest.root_checksum =>
                    {
                        Ok(())
                    }
                    Ok(Some(active)) if active.generation >= manifest.generation => Err(
                        Problem::new(
                            "sync_manifest_commit_conflict",
                            "A different manifest generation is active",
                            409,
                            correlation,
                        ),
                    ),
                    Ok(_) => Err(commit_error),
                    Err(_) => Err(Problem::new(
                        "sync_manifest_commit_outcome_unknown",
                        "Could not verify whether the profile sync was activated; retry sync to reconcile",
                        503,
                        correlation,
                    )
                    .retryable(true)),
                }
            }
            Err(error) => Err(error),
        }
    }

    pub async fn profile_proxy_assignment(
        &self,
        workspace_id: &WorkspaceId,
        profile_id: &ProfileId,
        correlation: &str,
    ) -> Result<Option<ProfileProxyAssignment>, Problem> {
        match self
            .get(
                &format!(
                    "/v1/workspaces/{}/profiles/{}/proxy-assignment",
                    workspace_id.as_str(),
                    profile_id.as_str()
                ),
                correlation,
            )
            .await
        {
            Ok(assignment) => Ok(Some(assignment)),
            Err(error) if error.code == "profile_proxy_assignment_not_found" => Ok(None),
            Err(error) => Err(error),
        }
    }
}

fn manifest_commit_idempotency_key(
    expected_profile_revision: Option<u64>,
    manifest: &SyncManifest,
    key_share_envelope_updates: Option<&[ProfileKeyRotationEnvelopeUpdate]>,
) -> Result<String, Problem> {
    let identity = serde_json::to_vec(&json!({
        "expectedProfileRevision": expected_profile_revision,
        "manifest": manifest,
        "keyShareEnvelopeUpdates": key_share_envelope_updates,
    }))
    .map_err(|_| client_problem("api_request_invalid", 400))?;
    Ok(format!("manifest-{:x}", Sha256::digest(identity)))
}

async fn read_problem(response: reqwest::Response, correlation: &str, retry_safe: bool) -> Problem {
    let status = response.status().as_u16();
    match response.json::<ApiProblem>().await {
        Ok(value) => Problem {
            problem_type: value
                .problem_type
                .filter(|problem_type| !problem_type.is_empty())
                .unwrap_or_else(|| format!("https://buglogin.local/problems/{}", value.code)),
            title: value.title,
            status: value.status,
            code: value.code,
            detail: value.detail,
            instance: value.instance,
            correlation_id: value
                .correlation_id
                .unwrap_or_else(|| correlation.to_owned()),
            retryable: retry_safe && value.retryable.unwrap_or(status >= 500 || status == 429),
            fields: value.fields.unwrap_or_default(),
        },
        Err(_) => Problem::new(
            "api_request_failed",
            "BugLogin API request failed",
            status,
            correlation,
        )
        .retryable(retry_safe && (status >= 500 || status == 429)),
    }
}

fn request_retry_safe(method: &Method, path: &str, has_idempotency_key: bool) -> bool {
    has_idempotency_key
        || *method == Method::GET
        || *method == Method::HEAD
        || *method == Method::OPTIONS
        || matches!(path, "/v1/auth/device/token" | "/v1/proxies/parse")
}

async fn parse_response(response: reqwest::Response, correlation: &str) -> Result<Value, Problem> {
    let status = response.status();
    if !status.is_success() {
        return Err(read_problem(response, correlation, true).await);
    }
    response.json().await.map_err(|_| {
        Problem::new(
            "api_response_invalid",
            "BugLogin API response is incomplete or invalid",
            502,
            correlation,
        )
        .retryable(true)
    })
}
fn client_problem(code: &'static str, status: u16) -> Problem {
    Problem::new(code, "BugLogin API client error", status, "desktop-api")
}

fn validate_cookie_job_id(job_id: &str) -> Result<(), Problem> {
    let Some(suffix) = job_id.strip_prefix("job_") else {
        return Err(client_problem("cookie_job_id_invalid", 400));
    };
    if suffix.len() != 64 || !suffix.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(client_problem("cookie_job_id_invalid", 400));
    }
    Ok(())
}

fn validate_runtime_dispatch_job_id(job_id: &str) -> Result<(), Problem> {
    let Some(suffix) = job_id.strip_prefix("runtime_") else {
        return Err(client_problem("runtime_job_id_invalid", 400));
    };
    if suffix.len() != 64 || !suffix.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(client_problem("runtime_job_id_invalid", 400));
    }
    Ok(())
}
fn valid_bearer(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= 4096
        && token.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
}
fn bad_input(field: &str, correlation: &str) -> Problem {
    Problem::new(
        "invalid_command_input",
        "Command input is invalid",
        400,
        correlation,
    )
    .with_field("field", field)
}

fn canonical_workspace_update(update: &Value, correlation: &str) -> Result<Value, Problem> {
    let object = update
        .as_object()
        .ok_or_else(|| bad_input("update", correlation))?;
    if object.len() != 2
        || !object.contains_key("workspace")
        || !object.contains_key("expectedRevision")
    {
        return Err(bad_input("update", correlation));
    }
    if !object.get("workspace").is_some_and(Value::is_object)
        || !object
            .get("expectedRevision")
            .and_then(Value::as_u64)
            .is_some_and(|revision| revision > 0)
    {
        return Err(bad_input("update", correlation));
    }
    Ok(update.clone())
}

fn legacy_workspace_update(
    update: &Value,
    record: &Value,
    organization_id: &str,
    workspace_id: &str,
    correlation: &str,
) -> Result<Value, Problem> {
    let update = update
        .as_object()
        .ok_or_else(|| bad_input("update", correlation))?;
    if update.is_empty()
        || update
            .keys()
            .any(|key| !matches!(key.as_str(), "name" | "policy"))
    {
        return Err(bad_input("update", correlation));
    }
    let revision = record
        .get("revision")
        .and_then(Value::as_u64)
        .filter(|revision| *revision > 0)
        .ok_or_else(|| {
            Problem::new(
                "workspace_record_invalid",
                "BugLogin API returned an invalid workspace revision",
                502,
                correlation,
            )
        })?;
    let mut workspace = record
        .get("workspace")
        .and_then(Value::as_object)
        .cloned()
        .ok_or_else(|| {
            Problem::new(
                "workspace_record_invalid",
                "BugLogin API returned an invalid workspace record",
                502,
                correlation,
            )
        })?;
    if workspace.get("id").and_then(Value::as_str) != Some(workspace_id)
        || workspace.get("organizationId").and_then(Value::as_str) != Some(organization_id)
    {
        return Err(Problem::new(
            "workspace_record_invalid",
            "BugLogin API returned a workspace outside the requested scope",
            502,
            correlation,
        ));
    }
    if let Some(name) = update.get("name") {
        let name = name
            .as_str()
            .filter(|name| !name.trim().is_empty())
            .ok_or_else(|| bad_input("name", correlation))?;
        workspace.insert("name".to_owned(), Value::String(name.trim().to_owned()));
    }
    if let Some(policy_update) = update.get("policy") {
        let policy_update = policy_update
            .as_object()
            .ok_or_else(|| bad_input("policy", correlation))?;
        let policy = workspace
            .get_mut("policy")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| {
                Problem::new(
                    "workspace_record_invalid",
                    "BugLogin API returned an invalid workspace policy",
                    502,
                    correlation,
                )
            })?;
        for (key, value) in policy_update {
            let valid = match key.as_str() {
                "maxProfiles" | "maxMembers" => {
                    value.as_u64().is_some_and(|value| value <= u32::MAX as u64)
                }
                "maxStorageBytes" => value.as_u64().is_some(),
                "offlineEntitled" => value.is_boolean(),
                _ => false,
            };
            if !valid {
                return Err(bad_input("policy", correlation));
            }
            policy.insert(key.clone(), value.clone());
        }
    }
    Ok(json!({
        "workspace": Value::Object(workspace),
        "expectedRevision": revision,
    }))
}
fn optional_bool(
    value: &Value,
    key: &str,
    default: bool,
    correlation: &str,
) -> Result<bool, Problem> {
    match value.get(key) {
        None => Ok(default),
        Some(Value::Bool(value)) => Ok(*value),
        Some(_) => Err(bad_input(key, correlation)),
    }
}
fn optional_page_limit(value: &Value, correlation: &str) -> Result<u64, Problem> {
    let Some(value) = value.get("limit") else {
        return Ok(100);
    };
    let limit = value
        .as_u64()
        .filter(|limit| (1..=100).contains(limit))
        .ok_or_else(|| bad_input("limit", correlation))?;
    Ok(limit)
}
fn optional_cursor(value: &Value, correlation: &str) -> Result<Option<String>, Problem> {
    match value.get("cursor") {
        None => Ok(None),
        Some(Value::String(cursor)) if !cursor.is_empty() && cursor.len() <= 128 => {
            Ok(Some(cursor.clone()))
        }
        Some(_) => Err(bad_input("cursor", correlation)),
    }
}
fn required_string<'a>(value: &'a Value, key: &str, correlation: &str) -> Result<&'a str, Problem> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| bad_input(key, correlation))
}
fn path_arg(value: &Value, key: &str, correlation: &str) -> Result<String, Problem> {
    let id = required_string(value, key, correlation)?;
    if id.len() > 128
        || !id.as_bytes()[0].is_ascii_alphanumeric()
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
    {
        return Err(bad_input(key, correlation));
    }
    Ok(id.to_owned())
}
fn command_id(name: &str) -> &str {
    match name {
        "system_health_get" => "system.health.get",
        "account_get" => "account.get",
        "account_list" => "account.list",
        "account_status_update" => "account.status.update",
        "organization_list" => "organization.list",
        "organization_get" => "organization.get",
        "organization_record_get" => "organization.record.get",
        "organization_create" => "organization.create",
        "organization_update" => "organization.update",
        "workspace_list" => "workspace.list",
        "workspace_get" => "workspace.get",
        "workspace_record_get" => "workspace.record.get",
        "workspace_create" => "workspace.create",
        "workspace_update" => "workspace.update",
        "group_list" => "group.list",
        "group_get" => "group.get",
        "group_create" => "group.create",
        "group_update" => "group.update",
        "group_delete" => "group.delete",
        "extension_group_list" => "extension.group.list",
        "extension_group_get" => "extension.group.get",
        "extension_group_create" => "extension.group.create",
        "extension_group_update" => "extension.group.update",
        "extension_group_delete" => "extension.group.delete",
        "profile_extension_group_set" => "profile.extension.group.set",
        "group_share_list" => "group.share.list",
        "group_share_create" => "group.share.create",
        "group_share_revoke" => "group.share.revoke",
        "profile_groups_replace" => "profile.groups.replace",
        "operation_get" => "operation.get",
        "migration_inspect" => "migration.inspect",
        "migration_import" => "migration.import",
        "profile_list" => "profile.list",
        "profile_get" => "profile.get",
        "profile_binding_get" => "profile.binding.get",
        "profile_proxy_assignment_get" => "profile.proxy.assignment.get",
        "profile_proxy_assignment_set" => "profile.proxy.assignment.set",
        "profile_create" => "profile.create",
        "profile_update" => "profile.update",
        "profile_delete" => "profile.delete",
        "profile_archive" => "profile.archive",
        "profile_restore" => "profile.restore",
        "profile_clone" => "profile.clone",
        "profile_share" => "profile.share",
        "profile_share_revoke" => "profile.share.revoke",
        "profile_share_recipientdevicekeys_list" => "profile.share.recipientdevicekeys.list",
        "profile_keyrotation_recipients_list" => "profile.keyrotation.recipients.list",
        "cookie_list" => "cookie.list",
        "cookie_stage" => "cookie.stage",
        "cookie_import" => "cookie.import",
        "cookie_copy" => "cookie.copy",
        "proxy_parse" => "proxy.parse",
        "proxy_list" => "proxy.list",
        "proxy_get" => "proxy.get",
        "proxy_create" => "proxy.create",
        "proxy_update" => "proxy.update",
        "proxy_delete" => "proxy.delete",
        "team_list" => "team.list",
        "team_get" => "team.get",
        "team_create" => "team.create",
        "team_update" => "team.update",
        "team_delete" => "team.delete",
        "member_list" => "member.list",
        "member_get" => "member.get",
        "member_invite" => "member.invite",
        "member_activate" => "member.activate",
        "member_suspend" => "member.suspend",
        "member_role_update" => "member.role.update",
        "resource_list" => "resource.list",
        "resource_get" => "resource.get",
        "resource_lease_get" => "resource.lease.get",
        "profile_runtime_get" => "profile.runtime.get",
        "resource_register" => "resource.register",
        "grant_list" => "grant.list.legacy",
        "grant_list_page" => "grant.list.page",
        "grant_create" => "grant.create",
        "grant_revoke" => "grant.revoke",
        _ => name,
    }
}
fn urlencoding(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) {
            out.push(byte as char)
        } else {
            out.push_str(&format!("%{byte:02X}"))
        }
    }
    out
}
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
#[cfg(test)]
mod tests {
    use super::{
        MAX_AUTH_RESPONSE_BYTES, RemoteApiClient, canonical_workspace_update,
        legacy_workspace_update, manifest_commit_idempotency_key, optional_bool, optional_cursor,
        optional_page_limit, request_retry_safe,
    };
    use crate::clock::unix_time_seconds_at;
    use crate::profile_runtime::DesktopSyncBackend;
    use buglogin_api::services::connected_cookie_jobs::{
        ConnectedCookieFailureCode, ConnectedCookieJob, ConnectedCookieJobState,
        ConnectedCookieReceipt,
    };
    use buglogin_api::services::runtime_dispatch::RuntimeDispatchState;
    use buglogin_domain::entities::{Profile, ProfileState, Runtime, SyncMode};
    use buglogin_domain::ids::{ProfileId, ProxyId, ResourceId, WorkspaceId};
    use buglogin_storage::local::Versioned;
    use buglogin_sync::coordination::{CoordinationAuth, CoordinationScope, LeaseGrant};
    use buglogin_sync::manifest::SyncManifest;
    use serde_json::{Value, json};
    use sha2::{Digest, Sha256};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn legacy_workspace_patch_is_merged_with_scoped_record_revision() {
        let record = json!({
            "workspace": {
                "id": "ws_1",
                "organizationId": "org_1",
                "name": "Before",
                "policy": {
                    "maxProfiles": 30,
                    "maxMembers": 10,
                    "maxStorageBytes": 4096,
                    "offlineEntitled": false
                },
                "createdAt": "2026-10-20T00:00:00Z"
            },
            "revision": 7
        });
        let body = legacy_workspace_update(
            &json!({ "name": "After", "policy": { "maxMembers": 20 } }),
            &record,
            "org_1",
            "ws_1",
            "cor_workspace_update",
        )
        .unwrap();

        assert_eq!(body["expectedRevision"], 7);
        assert_eq!(body["workspace"]["name"], "After");
        assert_eq!(body["workspace"]["policy"]["maxProfiles"], 30);
        assert_eq!(body["workspace"]["policy"]["maxMembers"], 20);
        assert_eq!(body["workspace"]["policy"]["maxStorageBytes"], 4096);
        assert_eq!(body["workspace"]["policy"]["offlineEntitled"], false);
    }

    #[test]
    fn workspace_update_adapter_rejects_invalid_revision_and_wrong_scope() {
        let record = json!({
            "workspace": { "id": "ws_other", "organizationId": "org_1", "policy": {} },
            "revision": 1
        });
        assert_eq!(
            legacy_workspace_update(
                &json!({ "name": "After" }),
                &record,
                "org_1",
                "ws_1",
                "cor_workspace_scope",
            )
            .unwrap_err()
            .code,
            "workspace_record_invalid"
        );
        assert_eq!(
            canonical_workspace_update(
                &json!({ "workspace": {}, "expectedRevision": 0 }),
                "cor_workspace_revision",
            )
            .unwrap_err()
            .code,
            "invalid_command_input"
        );
    }

    #[tokio::test]
    async fn legacy_workspace_update_fetches_revision_then_patches_canonical_record() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let record = json!({
                "workspace": {
                    "id": "ws_1",
                    "organizationId": "org_1",
                    "name": "Before",
                    "policy": {
                        "maxProfiles": 30,
                        "maxMembers": 10,
                        "maxStorageBytes": 4096,
                        "offlineEntitled": false
                    },
                    "createdAt": "2026-10-20T00:00:00Z"
                },
                "revision": 7
            });
            let (mut record_stream, _) = listener.accept().await.unwrap();
            let request = read_http_request(&mut record_stream).await;
            assert!(
                request.starts_with("GET /v1/organizations/org_1/workspaces/ws_1/record HTTP/1.1")
            );
            let response_body = serde_json::to_vec(&record).unwrap();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                response_body.len()
            );
            record_stream.write_all(response.as_bytes()).await.unwrap();
            record_stream.write_all(&response_body).await.unwrap();

            let (mut update_stream, _) = listener.accept().await.unwrap();
            let request = read_http_request(&mut update_stream).await;
            assert!(request.starts_with("PATCH /v1/organizations/org_1/workspaces/ws_1 HTTP/1.1"));
            let (headers, body) = request.split_once("\r\n\r\n").unwrap();
            assert!(
                headers
                    .to_ascii_lowercase()
                    .contains("idempotency-key: workspace-partial-update-0001")
            );
            assert_eq!(
                serde_json::from_str::<Value>(body).unwrap(),
                json!({
                    "workspace": {
                        "id": "ws_1",
                        "organizationId": "org_1",
                        "name": "After",
                        "policy": {
                            "maxProfiles": 30,
                            "maxMembers": 20,
                            "maxStorageBytes": 4096,
                            "offlineEntitled": false
                        },
                        "createdAt": "2026-10-20T00:00:00Z"
                    },
                    "expectedRevision": 7
                })
            );
            let updated = json!({
                "workspace": {
                    "id": "ws_1",
                    "organizationId": "org_1",
                    "name": "After",
                    "policy": {
                        "maxProfiles": 30,
                        "maxMembers": 20,
                        "maxStorageBytes": 4096,
                        "offlineEntitled": false
                    },
                    "createdAt": "2026-10-20T00:00:00Z"
                },
                "revision": 8
            });
            let response_body = serde_json::to_vec(&updated).unwrap();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                response_body.len()
            );
            update_stream.write_all(response.as_bytes()).await.unwrap();
            update_stream.write_all(&response_body).await.unwrap();
        });
        let client =
            RemoteApiClient::new(&format!("http://{address}"), "test-token".into()).unwrap();

        let result = client
            .invoke(
                "workspace_update",
                "workspace.update",
                Some("workspace-partial-update-0001"),
                json!({
                    "organizationId": "org_1",
                    "workspaceId": "ws_1",
                    "update": { "name": "After", "policy": { "maxMembers": 20 } }
                }),
            )
            .await
            .unwrap();

        assert_eq!(result.data["revision"], 8);
        assert_eq!(result.data["workspace"]["name"], "After");
        server.await.unwrap();
    }

    #[test]
    fn manifest_commit_idempotency_key_tracks_the_complete_retry_identity() {
        let manifest = SyncManifest::new(
            WorkspaceId::new("ws_idempotency").unwrap(),
            ResourceId::new("res_idempotency").unwrap(),
            7,
            4,
            Some(3),
            b"same profile root",
            Vec::new(),
        )
        .unwrap();
        let original = manifest_commit_idempotency_key(None, &manifest, None).unwrap();
        let exact_retry = manifest_commit_idempotency_key(None, &manifest, None).unwrap();
        assert_eq!(original, exact_retry);
        assert_eq!(original.len(), 73);

        let mut next_lease = manifest.clone();
        next_lease.fencing_token += 1;
        assert_ne!(
            original,
            manifest_commit_idempotency_key(None, &next_lease, None).unwrap()
        );
        assert_ne!(
            original,
            manifest_commit_idempotency_key(Some(9), &manifest, None).unwrap()
        );
    }

    #[tokio::test]
    async fn profile_delete_sends_only_the_strict_rest_body() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_http_request(&mut stream).await;
            let (headers, body) = request.split_once("\r\n\r\n").unwrap();
            assert!(headers.starts_with("DELETE /v1/profiles/prf_delete HTTP/1.1"));
            assert_eq!(
                serde_json::from_str::<Value>(body).unwrap(),
                json!({
                    "revision": 7,
                    "confirmation": "QUARANTINE_PROFILE_DATA"
                })
            );
            let response_body = r#"{"id":"prf_delete","deleted":true,"recoverable":true}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                response_body.len(),
                response_body
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        });
        let client =
            RemoteApiClient::new(&format!("http://{address}"), "device-token".into()).unwrap();
        let receipt = client
            .invoke(
                "profile_delete",
                "profile-delete-test",
                Some("profile-delete-key-0001"),
                json!({
                    "profileId": "prf_delete",
                    "revision": 7,
                    "confirmation": "QUARANTINE_PROFILE_DATA"
                }),
            )
            .await
            .unwrap();
        assert_eq!(receipt.command_id, "profile.delete");
        assert_eq!(receipt.data["deleted"], true);
        server.await.unwrap();
    }

    #[test]
    fn token_expiry_clock_before_epoch_fails_closed() {
        let invalid_clock = std::time::UNIX_EPOCH - std::time::Duration::from_secs(1);
        assert_eq!(
            unix_time_seconds_at(invalid_clock, "cor").unwrap_err().code,
            "clock_unavailable"
        );
    }

    #[test]
    fn desktop_list_query_rejects_malformed_values_instead_of_silently_defaulting() {
        assert_eq!(
            optional_bool(
                &json!({"includeArchived": "yes"}),
                "includeArchived",
                false,
                "cor"
            )
            .unwrap_err()
            .code,
            "invalid_command_input"
        );
        assert_eq!(
            optional_page_limit(&json!({"limit": "50"}), "cor")
                .unwrap_err()
                .code,
            "invalid_command_input"
        );
        assert_eq!(
            optional_page_limit(&json!({"limit": 101}), "cor")
                .unwrap_err()
                .code,
            "invalid_command_input"
        );
        assert_eq!(
            optional_cursor(&json!({"cursor": 42}), "cor")
                .unwrap_err()
                .code,
            "invalid_command_input"
        );
    }

    #[test]
    fn canonical_api_problem_keeps_type_instance_code_and_retryability() {
        let body = json!({
            "type": "https://buglogin.local/problems/sync_manifest_conflict",
            "title": "Manifest conflict",
            "status": 409,
            "code": "sync_manifest_conflict",
            "instance": "req_123",
            "correlationId": "cor_123",
            "retryable": false,
            "fields": {"generation": 4}
        });

        let parsed: super::ApiProblem = serde_json::from_value(body).unwrap();
        assert_eq!(
            parsed.problem_type.as_deref(),
            Some("https://buglogin.local/problems/sync_manifest_conflict")
        );
        assert_eq!(parsed.instance.as_deref(), Some("req_123"));
        assert_eq!(parsed.code, "sync_manifest_conflict");
        assert_eq!(parsed.retryable, Some(false));
    }

    #[test]
    fn transport_retry_requires_idempotency_or_safe_endpoint() {
        assert!(request_retry_safe(
            &reqwest::Method::GET,
            "/v1/profiles",
            false
        ));
        assert!(request_retry_safe(
            &reqwest::Method::POST,
            "/v1/auth/device/token",
            false
        ));
        assert!(request_retry_safe(
            &reqwest::Method::POST,
            "/v1/profiles",
            true
        ));
        assert!(!request_retry_safe(
            &reqwest::Method::POST,
            "/v1/auth/device/refresh",
            false
        ));
    }

    #[tokio::test]
    async fn desktop_login_sends_direct_credentials_and_parses_device_session() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_http_request(&mut stream).await;
            let (headers, body) = request.split_once("\r\n\r\n").unwrap();
            assert!(headers.starts_with("POST /v1/auth/desktop/login HTTP/1.1"));
            assert!(headers.lines().any(|line| {
                line.eq_ignore_ascii_case("x-correlation-id: cor_desktop_login_test")
            }));
            assert_eq!(
                serde_json::from_str::<Value>(body).unwrap(),
                json!({
                    "identifier": "local-owner@buglogin.test",
                    "password": "Synthetic-E2E-Password-2026!",
                    "deviceId": "device_synthetic_1"
                })
            );

            let response_body = r#"{"accessToken":"access-synthetic","refreshToken":"refresh-synthetic","tokenType":"Bearer","expiresAt":1900000000}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                response_body.len(),
                response_body
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        });

        let client = RemoteApiClient::new_unauthenticated(&format!("http://{address}")).unwrap();
        let session = client
            .desktop_login(
                "local-owner@buglogin.test".to_owned(),
                "Synthetic-E2E-Password-2026!".to_owned(),
                "device_synthetic_1",
                "cor_desktop_login_test",
            )
            .await
            .unwrap();

        assert_eq!(session.access_token, "access-synthetic");
        assert_eq!(session.refresh_token, "refresh-synthetic");
        assert_eq!(session.token_type, "Bearer");
        assert_eq!(session.expires_at, 1_900_000_000);
        let debug = format!("{session:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("access-synthetic"));
        assert!(!debug.contains("refresh-synthetic"));

        drop(session);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn device_key_enrollment_uses_bound_authenticated_idempotent_api_contract() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let public_key = "ab".repeat(32);
        let fingerprint = format!("{:x}", Sha256::digest([0xab; 32]));
        let server_public_key = public_key.clone();
        let server_fingerprint = fingerprint.clone();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_http_request(&mut stream).await;
            let (headers, body) = request.split_once("\r\n\r\n").unwrap();
            assert!(request.starts_with("PUT /v1/auth/device/encryption-key HTTP/1.1"));
            assert!(
                headers
                    .to_ascii_lowercase()
                    .contains("authorization: bearer device-token")
            );
            assert!(
                headers
                    .to_ascii_lowercase()
                    .contains("idempotency-key: device-key-enrollment-test-0001")
            );
            assert_eq!(
                serde_json::from_str::<Value>(body).unwrap(),
                json!({"publicKey": server_public_key})
            );
            let body = serde_json::to_vec(&json!({
                "accountId": "acct_device_enrollment",
                "deviceId": "device_enrollment",
                "schemaVersion": 1,
                "publicKey": server_public_key,
                "fingerprint": server_fingerprint,
                "registeredAt": 123
            }))
            .unwrap();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            stream.write_all(&body).await.unwrap();
        });
        let client =
            RemoteApiClient::new(&format!("http://{address}"), "device-token".into()).unwrap();
        client
            .register_device_encryption_key(
                "device_enrollment",
                &public_key,
                &fingerprint,
                "device-key-enrollment-test-0001",
                "device-enrollment-test",
            )
            .await
            .unwrap();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn recipient_device_key_query_uses_authenticated_share_route() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_http_request(&mut stream).await;
            let (headers, _) = request.split_once("\r\n\r\n").unwrap();
            assert!(request.starts_with(
                "GET /v1/profiles/prf_share_preview/shares/mem_share_preview/device-keys HTTP/1.1"
            ));
            assert!(
                headers
                    .to_ascii_lowercase()
                    .contains("authorization: bearer device-token")
            );
            let body = serde_json::to_vec(&json!([{
                "accountId": "acct_recipient",
                "deviceId": "device_recipient",
                "schemaVersion": 1,
                "publicKey": "ab".repeat(32),
                "fingerprint": "cd".repeat(32),
                "registeredAt": 123
            }]))
            .unwrap();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            stream.write_all(&body).await.unwrap();
        });
        let client =
            RemoteApiClient::new(&format!("http://{address}"), "device-token".into()).unwrap();
        let response = client
            .invoke(
                "profile_share_recipientdevicekeys_list",
                "share-preview-test",
                None,
                json!({
                    "profileId": "prf_share_preview",
                    "memberId": "mem_share_preview"
                }),
            )
            .await
            .unwrap();
        assert_eq!(
            response.command_id,
            "profile.share.recipientdevicekeys.list"
        );
        assert_eq!(response.data[0]["deviceId"], "device_recipient");
        assert_eq!(response.data[0]["fingerprint"], "cd".repeat(32));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn key_rotation_recipient_query_uses_authenticated_profile_route() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_http_request(&mut stream).await;
            let (headers, _) = request.split_once("\r\n\r\n").unwrap();
            assert!(request.starts_with(
                "GET /v1/profiles/prf_rotation_review/key-rotation-recipients HTTP/1.1"
            ));
            assert!(
                headers
                    .to_ascii_lowercase()
                    .contains("authorization: bearer device-token")
            );
            let body = serde_json::to_vec(&json!([{
                "memberId": "mem_rotation_review",
                "deviceKeys": [{
                    "accountId": "acct_rotation_review",
                    "deviceId": "device_rotation_review",
                    "schemaVersion": 1,
                    "publicKey": "ab".repeat(32),
                    "fingerprint": "cd".repeat(32),
                    "registeredAt": 123
                }]
            }]))
            .unwrap();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            stream.write_all(&body).await.unwrap();
        });
        let client =
            RemoteApiClient::new(&format!("http://{address}"), "device-token".into()).unwrap();
        let response = client
            .invoke(
                "profile_keyrotation_recipients_list",
                "rotation-review-test",
                None,
                json!({ "profileId": "prf_rotation_review" }),
            )
            .await
            .unwrap();
        assert_eq!(response.command_id, "profile.keyrotation.recipients.list");
        assert_eq!(response.data[0]["memberId"], "mem_rotation_review");
        assert_eq!(
            response.data[0]["deviceKeys"][0]["fingerprint"],
            "cd".repeat(32)
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn resource_lease_query_derives_actor_from_authenticated_account() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut account_stream, _) = listener.accept().await.unwrap();
            let account_request = read_http_request(&mut account_stream).await;
            let account_headers = account_request.split_once("\r\n\r\n").unwrap().0;
            assert!(account_request.starts_with("GET /v1/account HTTP/1.1"));
            assert!(
                account_headers
                    .to_ascii_lowercase()
                    .contains("authorization: bearer device-token")
            );
            let account_body = br#"{"accountId":"acct_lease_reader"}"#;
            let account_response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                account_body.len()
            );
            account_stream
                .write_all(account_response.as_bytes())
                .await
                .unwrap();
            account_stream.write_all(account_body).await.unwrap();

            let (mut lease_stream, _) = listener.accept().await.unwrap();
            let lease_request = read_http_request(&mut lease_stream).await;
            let lease_headers = lease_request.split_once("\r\n\r\n").unwrap().0;
            assert!(
                lease_request.starts_with("GET /v1/workspaces/ws_1/resources/res_1/lease HTTP/1.1")
            );
            assert!(
                lease_headers
                    .to_ascii_lowercase()
                    .contains("authorization: bearer device-token")
            );
            assert!(
                lease_headers
                    .to_ascii_lowercase()
                    .contains("x-buglogin-actor-id: acct_lease_reader")
            );
            let body = b"null";
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            );
            lease_stream.write_all(response.as_bytes()).await.unwrap();
            lease_stream.write_all(body).await.unwrap();
        });
        let client =
            RemoteApiClient::new(&format!("http://{address}"), "device-token".into()).unwrap();
        let result = client
            .invoke(
                "resource_lease_get",
                "lease-query-test",
                None,
                json!({"workspaceId":"ws_1","resourceId":"res_1"}),
            )
            .await
            .unwrap();
        assert_eq!(result.command_id, "resource.lease.get");
        assert!(result.data.is_null());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn production_control_commands_follow_openapi_routes_and_wire_bodies() {
        struct Invocation {
            name: &'static str,
            command_id: &'static str,
            payload: Value,
            method: &'static str,
            path: &'static str,
            body: Option<Value>,
            idempotency_key: Option<String>,
        }

        let write_key = |name: &str| Some(format!("control-parity-{name}"));
        let invocations = vec![
            Invocation {
                name: "account_get",
                command_id: "account.get",
                payload: json!({}),
                method: "GET",
                path: "/v1/account",
                body: None,
                idempotency_key: None,
            },
            Invocation {
                name: "account_list",
                command_id: "account.list",
                payload: json!({}),
                method: "GET",
                path: "/v1/accounts",
                body: None,
                idempotency_key: None,
            },
            Invocation {
                name: "account_status_update",
                command_id: "account.status.update",
                payload: json!({"accountId":"acct_1","status":"suspended","expectedRevision":4}),
                method: "PUT",
                path: "/v1/accounts/acct_1/status",
                body: Some(json!({"status":"suspended","expectedRevision":4})),
                idempotency_key: write_key("account-status"),
            },
            Invocation {
                name: "operation_get",
                command_id: "operation.get",
                payload: json!({"operationId":"op_profile_launch_1"}),
                method: "GET",
                path: "/v1/operations/op_profile_launch_1",
                body: None,
                idempotency_key: None,
            },
            Invocation {
                name: "organization_get",
                command_id: "organization.get",
                payload: json!({"organizationId":"org_1"}),
                method: "GET",
                path: "/v1/organizations/org_1",
                body: None,
                idempotency_key: None,
            },
            Invocation {
                name: "organization_record_get",
                command_id: "organization.record.get",
                payload: json!({"organizationId":"org_1"}),
                method: "GET",
                path: "/v1/organizations/org_1/record",
                body: None,
                idempotency_key: None,
            },
            Invocation {
                name: "organization_create",
                command_id: "organization.create",
                payload: json!({"organization":{"id":"org_1","accountId":"acct_1","name":"Org","createdAt":"2026-10-14T00:00:00Z"},"policy":{"maxProfiles":20,"maxMembers":10,"maxStorageBytes":1000,"offlineEntitled":false}}),
                method: "POST",
                path: "/v1/organizations",
                body: Some(
                    json!({"organization":{"id":"org_1","accountId":"acct_1","name":"Org","createdAt":"2026-10-14T00:00:00Z"},"policy":{"maxProfiles":20,"maxMembers":10,"maxStorageBytes":1000,"offlineEntitled":false}}),
                ),
                idempotency_key: write_key("organization-create"),
            },
            Invocation {
                name: "organization_update",
                command_id: "organization.update",
                payload: json!({"organizationId":"org_1","update":{"organization":{"id":"org_1","accountId":"acct_1","name":"Org 2","createdAt":"2026-10-14T00:00:00Z"},"policy":{"maxProfiles":20,"maxMembers":10,"maxStorageBytes":1000,"offlineEntitled":false},"expectedRevision":2}}),
                method: "PATCH",
                path: "/v1/organizations/org_1",
                body: Some(
                    json!({"organization":{"id":"org_1","accountId":"acct_1","name":"Org 2","createdAt":"2026-10-14T00:00:00Z"},"policy":{"maxProfiles":20,"maxMembers":10,"maxStorageBytes":1000,"offlineEntitled":false},"expectedRevision":2}),
                ),
                idempotency_key: write_key("organization-update"),
            },
            Invocation {
                name: "workspace_get",
                command_id: "workspace.get",
                payload: json!({"organizationId":"org_1","workspaceId":"ws_1"}),
                method: "GET",
                path: "/v1/organizations/org_1/workspaces/ws_1",
                body: None,
                idempotency_key: None,
            },
            Invocation {
                name: "workspace_record_get",
                command_id: "workspace.record.get",
                payload: json!({"organizationId":"org_1","workspaceId":"ws_1"}),
                method: "GET",
                path: "/v1/organizations/org_1/workspaces/ws_1/record",
                body: None,
                idempotency_key: None,
            },
            Invocation {
                name: "workspace_create",
                command_id: "workspace.create",
                payload: json!({"organizationId":"org_1","workspace":{"id":"ws_1","organizationId":"org_1","name":"Workspace","policy":{"maxProfiles":20,"maxMembers":10,"maxStorageBytes":1000,"offlineEntitled":false},"createdAt":"2026-10-14T00:00:00Z"}}),
                method: "POST",
                path: "/v1/organizations/org_1/workspaces",
                body: Some(
                    json!({"id":"ws_1","organizationId":"org_1","name":"Workspace","policy":{"maxProfiles":20,"maxMembers":10,"maxStorageBytes":1000,"offlineEntitled":false},"createdAt":"2026-10-14T00:00:00Z"}),
                ),
                idempotency_key: write_key("workspace-create"),
            },
            Invocation {
                name: "workspace_update",
                command_id: "workspace.update",
                payload: json!({"organizationId":"org_1","workspaceId":"ws_1","update":{"workspace":{"id":"ws_1","organizationId":"org_1","name":"Workspace 2","policy":{"maxProfiles":20,"maxMembers":10,"maxStorageBytes":1000,"offlineEntitled":false},"createdAt":"2026-10-14T00:00:00Z"},"expectedRevision":2}}),
                method: "PATCH",
                path: "/v1/organizations/org_1/workspaces/ws_1",
                body: Some(
                    json!({"workspace":{"id":"ws_1","organizationId":"org_1","name":"Workspace 2","policy":{"maxProfiles":20,"maxMembers":10,"maxStorageBytes":1000,"offlineEntitled":false},"createdAt":"2026-10-14T00:00:00Z"},"expectedRevision":2}),
                ),
                idempotency_key: write_key("workspace-update"),
            },
            Invocation {
                name: "group_list",
                command_id: "group.list",
                payload: json!({"workspaceId":"ws_1"}),
                method: "GET",
                path: "/v1/workspaces/ws_1/groups",
                body: None,
                idempotency_key: None,
            },
            Invocation {
                name: "group_get",
                command_id: "group.get",
                payload: json!({"workspaceId":"ws_1","groupId":"grp_1"}),
                method: "GET",
                path: "/v1/workspaces/ws_1/groups/grp_1",
                body: None,
                idempotency_key: None,
            },
            Invocation {
                name: "group_create",
                command_id: "group.create",
                payload: json!({"workspaceId":"ws_1","groupId":"grp_1","name":"North America"}),
                method: "POST",
                path: "/v1/workspaces/ws_1/groups",
                body: Some(json!({"workspaceId":"ws_1","groupId":"grp_1","name":"North America"})),
                idempotency_key: write_key("group-create"),
            },
            Invocation {
                name: "group_update",
                command_id: "group.update",
                payload: json!({"workspaceId":"ws_1","groupId":"grp_1","expectedRevision":2,"name":"North America team"}),
                method: "PUT",
                path: "/v1/workspaces/ws_1/groups/grp_1",
                body: Some(
                    json!({"workspaceId":"ws_1","groupId":"grp_1","expectedRevision":2,"name":"North America team"}),
                ),
                idempotency_key: write_key("group-update"),
            },
            Invocation {
                name: "group_delete",
                command_id: "group.delete",
                payload: json!({"workspaceId":"ws_1","groupId":"grp_1","expectedRevision":3}),
                method: "DELETE",
                path: "/v1/workspaces/ws_1/groups/grp_1",
                body: Some(json!({"workspaceId":"ws_1","groupId":"grp_1","expectedRevision":3})),
                idempotency_key: write_key("group-delete"),
            },
            Invocation {
                name: "extension_group_list",
                command_id: "extension.group.list",
                payload: json!({"workspaceId":"ws_1"}),
                method: "GET",
                path: "/v1/workspaces/ws_1/extension-groups",
                body: None,
                idempotency_key: None,
            },
            Invocation {
                name: "extension_group_get",
                command_id: "extension.group.get",
                payload: json!({"workspaceId":"ws_1","extensionGroupId":"exg_1"}),
                method: "GET",
                path: "/v1/workspaces/ws_1/extension-groups/exg_1",
                body: None,
                idempotency_key: None,
            },
            Invocation {
                name: "extension_group_create",
                command_id: "extension.group.create",
                payload: json!({"workspaceId":"ws_1","extensionGroupId":"exg_1","name":"Ads","extensionIds":[]}),
                method: "POST",
                path: "/v1/workspaces/ws_1/extension-groups",
                body: Some(
                    json!({"workspaceId":"ws_1","extensionGroupId":"exg_1","name":"Ads","extensionIds":[]}),
                ),
                idempotency_key: write_key("extension-group-create"),
            },
            Invocation {
                name: "extension_group_update",
                command_id: "extension.group.update",
                payload: json!({"workspaceId":"ws_1","extensionGroupId":"exg_1","expectedRevision":2,"name":"Ads team","extensionIds":[]}),
                method: "PUT",
                path: "/v1/workspaces/ws_1/extension-groups/exg_1",
                body: Some(
                    json!({"workspaceId":"ws_1","extensionGroupId":"exg_1","expectedRevision":2,"name":"Ads team","extensionIds":[]}),
                ),
                idempotency_key: write_key("extension-group-update"),
            },
            Invocation {
                name: "extension_group_delete",
                command_id: "extension.group.delete",
                payload: json!({"workspaceId":"ws_1","extensionGroupId":"exg_1","expectedRevision":3}),
                method: "DELETE",
                path: "/v1/workspaces/ws_1/extension-groups/exg_1",
                body: Some(
                    json!({"workspaceId":"ws_1","extensionGroupId":"exg_1","expectedRevision":3}),
                ),
                idempotency_key: write_key("extension-group-delete"),
            },
            Invocation {
                name: "profile_extension_group_set",
                command_id: "profile.extension.group.set",
                payload: json!({"workspaceId":"ws_1","profileId":"profile_1","expectedRevision":4,"extensionGroupId":"exg_1"}),
                method: "PUT",
                path: "/v1/profiles/profile_1/extension-group",
                body: Some(
                    json!({"workspaceId":"ws_1","profileId":"profile_1","expectedRevision":4,"extensionGroupId":"exg_1"}),
                ),
                idempotency_key: write_key("profile-extension-group-set"),
            },
            Invocation {
                name: "profile_groups_replace",
                command_id: "profile.groups.replace",
                payload: json!({"workspaceId":"ws_1","profileId":"profile_1","expectedRevision":4,"groupIds":["grp_1"]}),
                method: "PUT",
                path: "/v1/profiles/profile_1/groups",
                body: Some(
                    json!({"workspaceId":"ws_1","profileId":"profile_1","expectedRevision":4,"groupIds":["grp_1"]}),
                ),
                idempotency_key: write_key("profile-groups-replace"),
            },
            Invocation {
                name: "team_list",
                command_id: "team.list",
                payload: json!({"workspaceId":"ws_1"}),
                method: "GET",
                path: "/v1/workspaces/ws_1/teams",
                body: None,
                idempotency_key: None,
            },
            Invocation {
                name: "team_get",
                command_id: "team.get",
                payload: json!({"workspaceId":"ws_1","teamId":"team_1"}),
                method: "GET",
                path: "/v1/workspaces/ws_1/teams/team_1",
                body: None,
                idempotency_key: None,
            },
            Invocation {
                name: "team_create",
                command_id: "team.create",
                payload: json!({"workspaceId":"ws_1","team":{"id":"team_1","workspaceId":"ws_1","name":"Team","memberIds":[]}}),
                method: "POST",
                path: "/v1/workspaces/ws_1/teams",
                body: Some(
                    json!({"id":"team_1","workspaceId":"ws_1","name":"Team","memberIds":[]}),
                ),
                idempotency_key: write_key("team-create"),
            },
            Invocation {
                name: "team_update",
                command_id: "team.update",
                payload: json!({"workspaceId":"ws_1","teamId":"team_1","update":{"name":"Team 2","expectedRevision":2}}),
                method: "PUT",
                path: "/v1/workspaces/ws_1/teams/team_1",
                body: Some(json!({"name":"Team 2","expectedRevision":2})),
                idempotency_key: write_key("team-update"),
            },
            Invocation {
                name: "team_delete",
                command_id: "team.delete",
                payload: json!({"workspaceId":"ws_1","teamId":"team_1","expectedRevision":3}),
                method: "DELETE",
                path: "/v1/workspaces/ws_1/teams/team_1",
                body: Some(json!({"expectedRevision":3})),
                idempotency_key: write_key("team-delete"),
            },
            Invocation {
                name: "member_list",
                command_id: "member.list",
                payload: json!({"workspaceId":"ws_1","limit":100}),
                method: "GET",
                path: "/v1/workspaces/ws_1/members/page?limit=100",
                body: None,
                idempotency_key: None,
            },
            Invocation {
                name: "member_get",
                command_id: "member.get",
                payload: json!({"workspaceId":"ws_1","memberId":"mem_1"}),
                method: "GET",
                path: "/v1/workspaces/ws_1/members/mem_1",
                body: None,
                idempotency_key: None,
            },
            Invocation {
                name: "member_invite",
                command_id: "member.invite",
                payload: json!({"workspaceId":"ws_1","member":{"id":"mem_1","accountId":"acct_2","workspaceId":"ws_1","role":"member","status":"invited","inviteExpiresAt":200}}),
                method: "POST",
                path: "/v1/workspaces/ws_1/members",
                body: Some(
                    json!({"id":"mem_1","accountId":"acct_2","workspaceId":"ws_1","role":"member","status":"invited","inviteExpiresAt":200}),
                ),
                idempotency_key: write_key("member-invite"),
            },
            Invocation {
                name: "member_activate",
                command_id: "member.activate",
                payload: json!({"workspaceId":"ws_1","memberId":"mem_1"}),
                method: "PUT",
                path: "/v1/workspaces/ws_1/members/mem_1/activate",
                body: None,
                idempotency_key: write_key("member-activate"),
            },
            Invocation {
                name: "member_suspend",
                command_id: "member.suspend",
                payload: json!({"workspaceId":"ws_1","memberId":"mem_1"}),
                method: "PUT",
                path: "/v1/workspaces/ws_1/members/mem_1/suspend",
                body: None,
                idempotency_key: write_key("member-suspend"),
            },
            Invocation {
                name: "member_role_update",
                command_id: "member.role.update",
                payload: json!({"workspaceId":"ws_1","memberId":"mem_1","role":"operator"}),
                method: "PUT",
                path: "/v1/workspaces/ws_1/members/mem_1/role",
                body: Some(json!({"role":"operator"})),
                idempotency_key: write_key("member-role"),
            },
            Invocation {
                name: "resource_list",
                command_id: "resource.list",
                payload: json!({"workspaceId":"ws_1","limit":100}),
                method: "GET",
                path: "/v1/workspaces/ws_1/resources/page?limit=100",
                body: None,
                idempotency_key: None,
            },
            Invocation {
                name: "resource_get",
                command_id: "resource.get",
                payload: json!({"workspaceId":"ws_1","resourceId":"res_1"}),
                method: "GET",
                path: "/v1/workspaces/ws_1/resources/res_1",
                body: None,
                idempotency_key: None,
            },
            Invocation {
                name: "resource_register",
                command_id: "resource.register",
                payload: json!({"resource":{"id":"res_1","workspaceId":"ws_1","kind":"profile","state":"active","displayName":"Profile"}}),
                method: "POST",
                path: "/v1/workspaces/ws_1/resources",
                body: Some(
                    json!({"id":"res_1","workspaceId":"ws_1","kind":"profile","state":"active","displayName":"Profile"}),
                ),
                idempotency_key: write_key("resource-register"),
            },
        ];
        let server_expectations: Vec<_> = invocations
            .iter()
            .map(|call| {
                (
                    call.method,
                    call.path,
                    call.body.clone(),
                    call.idempotency_key.clone(),
                )
            })
            .collect();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (method, path, expected_body, idempotency_key) in server_expectations {
                let (mut stream, _) = listener.accept().await.unwrap();
                let request = read_http_request(&mut stream).await;
                let headers = request.split_once("\r\n\r\n").unwrap().0;
                assert!(request.starts_with(&format!("{method} {path} HTTP/1.1")));
                assert!(
                    headers
                        .to_ascii_lowercase()
                        .contains("authorization: bearer device-token")
                );
                assert!(
                    headers
                        .to_ascii_lowercase()
                        .contains("x-correlation-id: control-parity-test")
                );
                if let Some(idem) = idempotency_key {
                    assert!(
                        headers
                            .to_ascii_lowercase()
                            .contains(&format!("idempotency-key: {idem}"))
                    );
                }
                let body = request.split_once("\r\n\r\n").unwrap().1;
                match expected_body {
                    Some(expected) => {
                        assert_eq!(serde_json::from_str::<Value>(body).unwrap(), expected)
                    }
                    None => assert!(body.is_empty()),
                }
                let response_body = if path.starts_with("/v1/operations/") {
                    br#"{"operationId":"op_profile_launch_1","commandId":"profile.launch","scope":{"kind":"global"},"status":"completed","progress":1.0,"result":{"profileId":"prf_1"}}"#.as_slice()
                } else if path.contains("/members/page?") || path.contains("/resources/page?") {
                    br#"{"items":[],"nextCursor":null}"#.as_slice()
                } else {
                    b"[]".as_slice()
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    response_body.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
                stream.write_all(response_body).await.unwrap();
            }
        });
        let client =
            RemoteApiClient::new(&format!("http://{address}"), "device-token".into()).unwrap();
        for call in invocations {
            let result = client
                .invoke(
                    call.name,
                    "control-parity-test",
                    call.idempotency_key.as_deref(),
                    call.payload,
                )
                .await
                .unwrap_or_else(|error| panic!("{} failed: {error}", call.name));
            assert_eq!(result.command_id, call.command_id);
            if matches!(call.name, "member_list" | "resource_list") {
                assert_eq!(result.data, json!({"items": [], "nextCursor": null}));
            } else if call.name == "operation_get" {
                assert_eq!(result.data["status"], "completed");
                assert_eq!(result.data["result"]["profileId"], "prf_1");
            }
        }
        server.await.unwrap();
    }

    #[tokio::test]
    async fn profile_actions_bridge_routes_group_proxy_and_trash_mutations() {
        let profile_value = |archived: bool, revision: u64| {
            json!({
                "schemaVersion": 1,
                "value": {
                    "id": "prf_actions_e2e",
                    "name": "Actions E2E",
                    "runtime": "wayfern",
                    "runtimeVersion": null,
                    "runtimeConfig": null,
                    "state": "ready",
                    "archived": archived,
                    "fingerprint": null,
                    "note": null,
                    "proxyId": "pxy_actions_e2e",
                    "vpnId": null,
                    "groupIds": ["grp_destination"],
                    "tags": [],
                    "extensionGroupId": null,
                    "syncMode": "managed",
                    "createdAt": "2026-10-19T00:00:00Z",
                    "updatedAt": "2026-10-19T00:00:00Z"
                },
                "revision": revision
            })
        };
        let invocations = vec![
            (
                "profile_groups_replace",
                "profile.groups.replace",
                json!({"workspaceId":"ws_actions_e2e","profileId":"prf_actions_e2e","expectedRevision":1,"groupIds":["grp_destination"]}),
                "PUT",
                "/v1/profiles/prf_actions_e2e/groups",
                json!({"workspaceId":"ws_actions_e2e","profileId":"prf_actions_e2e","expectedRevision":1,"groupIds":["grp_destination"]}),
                "actions-group-move-0001",
                json!({"workspaceId":"ws_actions_e2e","profileId":"prf_actions_e2e","revision":2,"groupIds":["grp_destination"]}),
            ),
            (
                "proxy_update",
                "proxy.update",
                json!({"workspaceId":"ws_actions_e2e","proxyId":"pxy_actions_e2e","expectedRevision":1,"proxy":{"id":"pxy_actions_e2e","name":"","protocol":"http","host":"proxy-new.example","port":8080,"bypassRules":[],"createdAt":"2026-10-19T00:00:00Z","updatedAt":"2026-10-19T00:01:00Z"}}),
                "PATCH",
                "/v1/workspaces/ws_actions_e2e/proxies/pxy_actions_e2e",
                json!({"workspaceId":"ws_actions_e2e","proxyId":"pxy_actions_e2e","expectedRevision":1,"proxy":{"id":"pxy_actions_e2e","name":"","protocol":"http","host":"proxy-new.example","port":8080,"bypassRules":[],"createdAt":"2026-10-19T00:00:00Z","updatedAt":"2026-10-19T00:01:00Z"}}),
                "actions-proxy-edit-0001",
                json!({"workspaceId":"ws_actions_e2e","revision":2,"proxy":{"id":"pxy_actions_e2e","host":"proxy-new.example","port":8080}}),
            ),
            (
                "profile_proxy_assignment_set",
                "profile.proxy.assignment.set",
                json!({"workspaceId":"ws_actions_e2e","profileId":"prf_actions_e2e","expectedProfileRevision":2,"proxyId":null}),
                "PUT",
                "/v1/workspaces/ws_actions_e2e/profiles/prf_actions_e2e/proxy-assignment",
                json!({"workspaceId":"ws_actions_e2e","profileId":"prf_actions_e2e","expectedProfileRevision":2,"proxyId":null}),
                "actions-proxy-reassign-0001",
                json!({"workspaceId":"ws_actions_e2e","profileId":"prf_actions_e2e","expectedProfileRevision":2,"proxyId":null}),
            ),
            (
                "profile_archive",
                "profile.archive",
                json!({"profileId":"prf_actions_e2e","revision":2}),
                "PUT",
                "/v1/profiles/prf_actions_e2e/archive",
                json!({"revision":2}),
                "actions-profile-archive-0001",
                profile_value(true, 3),
            ),
            (
                "profile_restore",
                "profile.restore",
                json!({"profileId":"prf_actions_e2e","revision":3}),
                "PUT",
                "/v1/profiles/prf_actions_e2e/restore",
                json!({"revision":3}),
                "actions-profile-restore-0001",
                profile_value(false, 4),
            ),
        ];
        let expectations: Vec<_> = invocations
            .iter()
            .map(|(_, _, _, method, path, body, idem, response)| {
                (*method, *path, body.clone(), *idem, response.clone())
            })
            .collect();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (method, path, expected_body, idem, response_body) in expectations {
                let (mut stream, _) = listener.accept().await.unwrap();
                let request = read_http_request(&mut stream).await;
                let (headers, body) = request.split_once("\r\n\r\n").unwrap();
                assert!(request.starts_with(&format!("{method} {path} HTTP/1.1")));
                assert!(
                    headers
                        .to_ascii_lowercase()
                        .contains("authorization: bearer device-token")
                );
                assert!(
                    headers
                        .to_ascii_lowercase()
                        .contains(&format!("idempotency-key: {idem}"))
                );
                assert_eq!(serde_json::from_str::<Value>(body).unwrap(), expected_body);
                let response_body = serde_json::to_vec(&response_body).unwrap();
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    response_body.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
                stream.write_all(&response_body).await.unwrap();
            }
        });
        let client =
            RemoteApiClient::new(&format!("http://{address}"), "device-token".into()).unwrap();
        for (name, command_id, payload, _, _, _, idem, expected_response) in invocations {
            let result = client
                .invoke(name, "profile-actions-e2e", Some(idem), payload)
                .await
                .unwrap_or_else(|error| panic!("{name} failed: {error}"));
            assert_eq!(result.command_id, command_id);
            match name {
                "profile_archive" | "profile_restore" => {
                    assert_eq!(
                        result.data["archived"],
                        expected_response["value"]["archived"]
                    );
                    assert_eq!(result.data["revision"], expected_response["revision"]);
                }
                "profile_groups_replace" => {
                    assert_eq!(result.data["groupIds"], expected_response["groupIds"])
                }
                "profile_proxy_assignment_set" => assert_eq!(result.data["proxyId"], Value::Null),
                "proxy_update" => assert_eq!(result.data["revision"], 2),
                _ => unreachable!("only profile action invocations are configured"),
            }
        }
        server.await.unwrap();
    }

    #[tokio::test]
    async fn ambiguous_blob_upload_is_reconciled_by_presence_check_on_retry() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let body = b"encrypted-profile-chunk".to_vec();
        let checksum = buglogin_storage::blob::checksum(&body);
        let server_body = body.clone();
        let server_checksum = checksum.clone();
        let server = tokio::spawn(async move {
            let mut put_count = 0;
            let (mut put_stream, _) = listener.accept().await.unwrap();
            let request = read_http_request(&mut put_stream).await;
            assert!(request.starts_with(
                "PUT /v1/workspaces/ws_upload_retry/resources/res_upload_retry/blobs?key=Default%2FCookies HTTP/1.1"
            ));
            let (headers, uploaded_body) = request.split_once("\r\n\r\n").unwrap();
            assert_eq!(uploaded_body.as_bytes(), server_body);
            assert!(
                headers
                    .to_ascii_lowercase()
                    .contains(&format!("x-content-sha256: {server_checksum}"))
            );
            put_count += 1;
            // Model a server that durably stored the immutable blob but lost its response.
            drop(put_stream);

            let (mut check_stream, _) = listener.accept().await.unwrap();
            let check_request = read_http_request(&mut check_stream).await;
            assert!(check_request.starts_with(
                "GET /v1/workspaces/ws_upload_retry/resources/res_upload_retry/blobs/check?key=Default%2FCookies&generation=2&checksum="
            ));
            let response_body = br#"{"present":true}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                response_body.len()
            );
            check_stream.write_all(response.as_bytes()).await.unwrap();
            check_stream.write_all(response_body).await.unwrap();
            (put_count, check_request)
        });

        let client =
            RemoteApiClient::new(&format!("http://{address}"), "device-token".into()).unwrap();
        let workspace = WorkspaceId::new("ws_upload_retry").unwrap();
        let resource = ResourceId::new("res_upload_retry").unwrap();
        let auth = CoordinationAuth::new("desktop-upload-retry", "lease-token").unwrap();
        let scope = CoordinationScope {
            workspace_id: &workspace,
            auth: &auth,
        };
        let upload = client
            .upload_blob(
                scope.clone(),
                &resource,
                "Default/Cookies",
                2,
                &checksum,
                "owner-token",
                7,
                "upload-ambiguous-first-attempt",
                body,
                "upload-retry-test",
            )
            .await;
        assert!(upload.is_err());

        assert!(
            client
                .blob_present(
                    scope,
                    &resource,
                    "Default/Cookies",
                    2,
                    &checksum,
                    "owner-token",
                    7,
                    "upload-retry-test",
                )
                .await
                .unwrap()
        );
        let (put_count, check_request) = server.await.unwrap();
        assert_eq!(
            put_count, 1,
            "retry should not upload an already stored blob"
        );
        assert!(
            check_request
                .to_ascii_lowercase()
                .contains("x-buglogin-fencing-token: 7")
        );
    }

    #[tokio::test]
    async fn device_refresh_sends_refresh_credential_without_access_bearer() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_http_request(&mut stream).await;
            let expires_at = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs()
                + 3600;
            let body = serde_json::json!({
                "accessToken": "a".repeat(64),
                "refreshToken": "b".repeat(64),
                "tokenType": "Bearer",
                "expiresAt": expires_at
            });
            let body = serde_json::to_vec(&body).unwrap();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            stream.write_all(&body).await.unwrap();
            request
        });
        let client = RemoteApiClient::new_unauthenticated(&format!("http://{address}")).unwrap();
        let token = client
            .device_session_refresh(&"c".repeat(64), "desktop-device", "refresh-test")
            .await
            .unwrap();
        assert_eq!(token.access_token, "a".repeat(64));
        assert_eq!(token.refresh_token, "b".repeat(64));
        assert!(!format!("{token:?}").contains(&token.refresh_token));
        let request = server.await.unwrap();
        let (headers, body) = request.split_once("\r\n\r\n").unwrap();
        assert!(headers.starts_with("POST /v1/auth/device/refresh HTTP/1.1"));
        assert!(
            headers
                .to_ascii_lowercase()
                .contains("idempotency-key: device-refresh-")
        );
        assert!(!headers.to_ascii_lowercase().contains("authorization:"));
        assert!(body.contains("\"refreshToken\""));
        assert!(body.contains("desktop-device"));
    }

    #[tokio::test]
    async fn device_refresh_keeps_server_errors_retryable_after_same_key_retry() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let request = read_http_request(&mut stream).await;
                assert!(request.starts_with("POST /v1/auth/device/refresh HTTP/1.1"));
                assert!(
                    request
                        .to_ascii_lowercase()
                        .contains("idempotency-key: device-refresh-")
                );
                let body = br#"{"code":"temporary_failure","title":"Temporary failure","status":503,"retryable":false}"#;
                let response = format!(
                    "HTTP/1.1 503 Service Unavailable\r\ncontent-type: application/problem+json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
                stream.write_all(body).await.unwrap();
            }
        });
        let client = RemoteApiClient::new_unauthenticated(&format!("http://{address}")).unwrap();

        let error = client
            .device_session_refresh(&"c".repeat(64), "desktop-device", "refresh-503")
            .await
            .unwrap_err();

        assert!(error.retryable);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn device_refresh_bounds_declared_and_streamed_response_bodies() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let mut idempotency_key = None;
            for attempt in 0..2 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let request = read_http_request(&mut stream).await;
                let header = request
                    .lines()
                    .find(|line| line.to_ascii_lowercase().starts_with("idempotency-key:"))
                    .expect("refresh request has an idempotency key")
                    .to_owned();
                if let Some(previous) = &idempotency_key {
                    assert_eq!(&header, previous);
                } else {
                    idempotency_key = Some(header);
                }

                if attempt == 0 {
                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                        MAX_AUTH_RESPONSE_BYTES + 1
                    );
                    stream.write_all(response.as_bytes()).await.unwrap();
                    continue;
                }

                let oversized = vec![b'x'; MAX_AUTH_RESPONSE_BYTES + 1];
                let response = format!(
                    "HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n{:X}\r\n",
                    oversized.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
                stream.write_all(&oversized).await.unwrap();
                stream.write_all(b"\r\n0\r\n\r\n").await.unwrap();
            }
        });
        let client = RemoteApiClient::new_unauthenticated(&format!("http://{address}")).unwrap();

        let error = client
            .device_session_refresh(&"c".repeat(64), "desktop-device", "refresh-oversized")
            .await
            .unwrap_err();

        assert_eq!(error.code, "api_auth_response_invalid");
        assert!(error.retryable);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn device_refresh_keeps_credentials_retryable_after_truncated_responses() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let mut idempotency_key = None;
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let request = read_http_request(&mut stream).await;
                let header = request
                    .lines()
                    .find(|line| line.to_ascii_lowercase().starts_with("idempotency-key:"))
                    .expect("refresh request has an idempotency key")
                    .to_owned();
                if let Some(previous) = &idempotency_key {
                    assert_eq!(&header, previous);
                } else {
                    idempotency_key = Some(header);
                }
                stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 100\r\nconnection: close\r\n\r\n{\"partial\":",
                    )
                    .await
                    .unwrap();
            }
        });
        let client = RemoteApiClient::new_unauthenticated(&format!("http://{address}")).unwrap();

        let error = client
            .device_session_refresh(&"c".repeat(64), "desktop-device", "refresh-truncated")
            .await
            .unwrap_err();

        assert_eq!(error.code, "api_unavailable");
        assert!(error.retryable);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn device_refresh_keeps_network_failures_retryable() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let mut idempotency_key = None;
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let request = read_http_request(&mut stream).await;
                assert!(request.starts_with("POST /v1/auth/device/refresh HTTP/1.1"));
                let header = request
                    .lines()
                    .find(|line| line.to_ascii_lowercase().starts_with("idempotency-key:"))
                    .expect("refresh request has an idempotency key")
                    .to_owned();
                if let Some(previous) = &idempotency_key {
                    assert_eq!(&header, previous);
                } else {
                    idempotency_key = Some(header);
                }
                // The server receives the credential but loses the response.
                // The desktop must retry with the identical idempotency key.
                drop(stream);
            }
        });
        let client = RemoteApiClient::new_unauthenticated(&format!("http://{address}")).unwrap();

        let error = client
            .device_session_refresh(&"c".repeat(64), "desktop-device", "refresh-disconnect")
            .await
            .unwrap_err();

        assert_eq!(error.code, "api_unavailable");
        assert!(error.retryable);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn connected_cookie_transport_uses_device_bearer_and_metadata_receipt_only() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let workspace = WorkspaceId::new("ws_cookie_transport").unwrap();
        let workspace_text = workspace.as_str();
        let job_id = format!("job_{}", "a".repeat(64));
        let operation_id = "operation_cookie_transport";
        let device_id = "device_cookie_transport";
        let profile_id = "prf_cookie_transport";
        let receipt = ConnectedCookieReceipt {
            operation_id: operation_id.into(),
            job_id: job_id.clone(),
            device_id: device_id.into(),
            profile_id: profile_id.into(),
            previous_generation: 3,
            new_generation: 4,
            created_count: 2,
            replaced_count: 1,
            deleted_count: 0,
            metadata_checksum: "b".repeat(64),
            completed_at: 500,
        };
        let server_job = |state, receipt| ConnectedCookieJob {
            job_id: job_id.clone(),
            operation_id: operation_id.into(),
            command_id: "cookie.import".into(),
            workspace_id: workspace_text.into(),
            device_id: device_id.into(),
            profile_id: profile_id.into(),
            executor_account_id: Some("acct_cookie_transport".into()),
            expected_generation: 3,
            source_workspace_id: None,
            source_profile_id: None,
            source_device_id: None,
            staged_import_ref: Some("opaque_stage_ref".into()),
            requested_by: "acct_cookie_transport".into(),
            idempotency_key: "cookie-idempotency-transport".into(),
            created_at: 100,
            expires_at: 700,
            state,
            receipt,
            failure_code: None,
        };
        let mut failed_job = server_job(ConnectedCookieJobState::Failed, None);
        failed_job.failure_code = Some(ConnectedCookieFailureCode::MaterializationFailed);
        let responses = vec![
            serde_json::json!({"items":[server_job(ConnectedCookieJobState::Accepted, None)]}),
            serde_json::json!({"items":[server_job(ConnectedCookieJobState::Accepted, None)]}),
            serde_json::to_value(server_job(ConnectedCookieJobState::Dispatched, None)).unwrap(),
            serde_json::to_value(server_job(ConnectedCookieJobState::Running, None)).unwrap(),
            serde_json::to_value(server_job(
                ConnectedCookieJobState::Completed,
                Some(receipt.clone()),
            ))
            .unwrap(),
            serde_json::to_value(failed_job).unwrap(),
            serde_json::to_value(server_job(ConnectedCookieJobState::Cancelled, None)).unwrap(),
        ];
        let server = tokio::spawn(async move {
            let mut requests = Vec::new();
            for body in responses {
                let (mut stream, _) = listener.accept().await.unwrap();
                requests.push(read_http_request(&mut stream).await);
                let body = serde_json::to_vec(&body).unwrap();
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
                stream.write_all(&body).await.unwrap();
            }
            requests
        });
        let client =
            RemoteApiClient::new(&format!("http://{address}"), "device-token".into()).unwrap();
        let lease = LeaseGrant {
            resource_id: ResourceId::new("res_cookie_transport").unwrap(),
            workspace_id: workspace.clone(),
            owner_id: "desktop-cookie-transport".into(),
            owner_token: "opaque-cookie-owner-token".into(),
            fencing_token: 9,
            issued_at_ms: 1_000,
            expires_at_ms: 121_000,
        };
        assert_eq!(
            client
                .claim_cookie_job(&workspace, "../unexpected-path", "cookie-invalid-id")
                .await
                .unwrap_err()
                .code,
            "cookie_job_id_invalid"
        );
        let inbox = client
            .cookie_job_inbox(&workspace, "cookie-inbox-test")
            .await
            .unwrap();
        assert_eq!(inbox.len(), 1);
        assert_eq!(inbox[0].job_id, job_id);
        let device_inbox = client
            .cookie_job_inbox_all("cookie-device-inbox-test")
            .await
            .unwrap();
        assert_eq!(device_inbox.len(), 1);
        assert_eq!(device_inbox[0].job_id, job_id);
        assert_eq!(
            client
                .claim_cookie_job(&workspace, &inbox[0].job_id, "cookie-claim-test")
                .await
                .unwrap()
                .state,
            ConnectedCookieJobState::Dispatched
        );
        assert_eq!(
            client
                .acknowledge_cookie_job(&workspace, &inbox[0].job_id, &lease, "cookie-ack-test",)
                .await
                .unwrap()
                .state,
            ConnectedCookieJobState::Running
        );
        assert_eq!(
            client
                .complete_cookie_job(
                    &workspace,
                    &inbox[0].job_id,
                    &receipt,
                    "cookie-complete-test",
                )
                .await
                .unwrap()
                .state,
            ConnectedCookieJobState::Completed
        );
        assert_eq!(
            client
                .fail_cookie_job(
                    &workspace,
                    &inbox[0].job_id,
                    ConnectedCookieFailureCode::MaterializationFailed,
                    "cookie-fail-test",
                )
                .await
                .unwrap()
                .failure_code,
            Some(ConnectedCookieFailureCode::MaterializationFailed)
        );
        assert_eq!(
            client
                .cancel_cookie_job(&workspace, &inbox[0].job_id, "cookie-cancel-test")
                .await
                .unwrap()
                .state,
            ConnectedCookieJobState::Cancelled
        );
        let requests = server.await.unwrap();
        let expected_paths = [
            format!("GET /v1/workspaces/{workspace_text}/cookie-jobs/inbox HTTP/1.1"),
            "GET /v1/device/cookie-jobs/inbox HTTP/1.1".to_owned(),
            format!("POST /v1/cookie-jobs/{job_id}/claim?workspaceId={workspace_text} HTTP/1.1"),
            format!("POST /v1/cookie-jobs/{job_id}/ack?workspaceId={workspace_text} HTTP/1.1"),
            format!("POST /v1/cookie-jobs/{job_id}/complete?workspaceId={workspace_text} HTTP/1.1"),
            format!("POST /v1/cookie-jobs/{job_id}/fail?workspaceId={workspace_text} HTTP/1.1"),
            format!("POST /v1/cookie-jobs/{job_id}/cancel?workspaceId={workspace_text} HTTP/1.1"),
        ];
        for (request, expected_path) in requests.iter().zip(expected_paths) {
            let (headers, body) = request.split_once("\r\n\r\n").unwrap();
            assert!(headers.starts_with(&expected_path));
            assert!(
                headers
                    .to_ascii_lowercase()
                    .contains("authorization: bearer device-token")
            );
            if expected_path.contains("/ack?") {
                assert!(headers.contains("x-buglogin-owner-id: desktop-cookie-transport"));
                assert!(headers.contains("x-buglogin-owner-token: opaque-cookie-owner-token"));
                assert!(headers.contains("x-buglogin-fencing-token: 9"));
            }
            if !body.is_empty() {
                let payload: Value = serde_json::from_str(body).unwrap();
                if expected_path.contains("/fail?") {
                    assert_eq!(
                        payload,
                        serde_json::json!({"failureCode":"materialization_failed"})
                    );
                } else {
                    assert_eq!(payload["metadataChecksum"], "b".repeat(64));
                }
                assert!(payload.get("cookies").is_none());
                assert!(payload.get("value").is_none());
            }
        }
    }

    #[tokio::test]
    async fn managed_lease_uses_versioned_rest_contract_and_idempotency() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let workspace = WorkspaceId::new("ws_lease").unwrap();
        let resource = ResourceId::new("res_lease").unwrap();
        let response_lease = LeaseGrant {
            resource_id: resource.clone(),
            workspace_id: workspace.clone(),
            owner_id: "desktop-1".into(),
            owner_token: "opaque-owner-token".into(),
            fencing_token: 12,
            issued_at_ms: 1_000,
            expires_at_ms: 121_000,
        };
        let response_body = serde_json::to_vec(&response_lease).unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_http_request(&mut stream).await;
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                response_body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            stream.write_all(&response_body).await.unwrap();
            request
        });
        let client =
            RemoteApiClient::new(&format!("http://{address}"), "device-token".into()).unwrap();
        let auth = CoordinationAuth::new("desktop-1", "device-token").unwrap();
        let lease = client
            .acquire_lease_idempotent(
                CoordinationScope {
                    workspace_id: &workspace,
                    auth: &auth,
                },
                &resource,
                std::time::Duration::from_secs(120),
                "lease-create-key-0001",
            )
            .await
            .unwrap();
        let request = server.await.unwrap();
        let (headers, body) = request.split_once("\r\n\r\n").unwrap();
        let request_body: Value = serde_json::from_str(body).unwrap();

        assert!(
            headers.starts_with("POST /v1/workspaces/ws_lease/resources/res_lease/lease HTTP/1.1")
        );
        assert!(
            headers
                .to_ascii_lowercase()
                .contains("authorization: bearer device-token")
        );
        assert!(
            headers
                .to_ascii_lowercase()
                .contains("idempotency-key: lease-create-key-0001")
        );
        assert_eq!(request_body["ownerId"], "desktop-1");
        assert_eq!(request_body["ttlSeconds"], 120);
        assert_eq!(lease, response_lease);
    }

    #[tokio::test]
    async fn blob_presence_check_uses_resource_lease_scope_and_checksum() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_http_request(&mut stream).await;
            let body = br#"{"present":true}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            stream.write_all(body).await.unwrap();
            request
        });
        let client =
            RemoteApiClient::new(&format!("http://{address}"), "device-token".to_owned()).unwrap();
        let auth = CoordinationAuth::new("desktop-1", "lease-token").unwrap();
        let workspace = WorkspaceId::new("ws_retry").unwrap();
        let resource = ResourceId::new("res_retry").unwrap();
        let present = client
            .blob_present(
                CoordinationScope {
                    workspace_id: &workspace,
                    auth: &auth,
                },
                &resource,
                "Default/Preferences",
                4,
                &"a".repeat(64),
                "owner-token",
                7,
                "blob-presence-test",
            )
            .await
            .unwrap();
        let request = server.await.unwrap();
        let headers = request.split_once("\r\n\r\n").unwrap().0;
        assert!(headers.starts_with(
            "GET /v1/workspaces/ws_retry/resources/res_retry/blobs/check?key=Default%2FPreferences&generation=4&checksum="
        ));
        assert!(
            headers
                .to_ascii_lowercase()
                .contains("x-buglogin-owner-id: desktop-1")
        );
        assert!(
            headers
                .to_ascii_lowercase()
                .contains("x-buglogin-fencing-token: 7")
        );
        assert!(present);
    }

    #[tokio::test]
    async fn retryable_manifest_commit_error_is_reconciled_against_active_generation() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let workspace = WorkspaceId::new("ws_reconcile").unwrap();
        let resource = ResourceId::new("res_reconcile").unwrap();
        let manifest = SyncManifest::new(
            workspace.clone(),
            resource.clone(),
            7,
            1,
            None,
            b"canonical profile root",
            Vec::new(),
        )
        .unwrap();
        let active_body = serde_json::to_vec(&manifest).unwrap();
        let server = tokio::spawn(async move {
            let (mut commit_stream, _) = listener.accept().await.unwrap();
            let commit_request = read_http_request(&mut commit_stream).await;
            assert!(commit_request.starts_with(
                "PUT /v1/workspaces/ws_reconcile/resources/res_reconcile/manifest HTTP/1.1"
            ));
            let unavailable = br#"{"code":"api_temporarily_unavailable","title":"Temporary outage","status":503,"retryable":true}"#;
            let response = format!(
                "HTTP/1.1 503 Service Unavailable\r\ncontent-type: application/problem+json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                unavailable.len()
            );
            commit_stream.write_all(response.as_bytes()).await.unwrap();
            commit_stream.write_all(unavailable).await.unwrap();

            let (mut reconcile_stream, _) = listener.accept().await.unwrap();
            let reconcile_request = read_http_request(&mut reconcile_stream).await;
            assert!(reconcile_request.starts_with(
                "GET /v1/workspaces/ws_reconcile/resources/res_reconcile/manifest HTTP/1.1"
            ));
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                active_body.len()
            );
            reconcile_stream
                .write_all(response.as_bytes())
                .await
                .unwrap();
            reconcile_stream.write_all(&active_body).await.unwrap();
            (commit_request, reconcile_request)
        });

        let client =
            RemoteApiClient::new(&format!("http://{address}"), "device-token".into()).unwrap();
        let auth = CoordinationAuth::new("desktop-1", "device-token").unwrap();
        client
            .commit_manifest_with_reconciliation(
                CoordinationScope {
                    workspace_id: &workspace,
                    auth: &auth,
                },
                &resource,
                "lease-owner-token",
                None,
                manifest,
                "sync-reconcile-test",
            )
            .await
            .expect("active generation proves that the commit succeeded");

        let (commit_request, reconcile_request) = server.await.unwrap();
        let idempotency_key = commit_request
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase()
                    .starts_with("idempotency-key: ")
                    .then(|| line.split_once(':').map(|(_, value)| value.trim()))
                    .flatten()
            })
            .expect("manifest request sends an idempotency key");
        assert!(idempotency_key.starts_with("manifest-"));
        assert_eq!(idempotency_key.len(), 73);
        assert!(
            reconcile_request
                .to_ascii_lowercase()
                .contains("authorization: bearer device-token")
        );
    }

    #[tokio::test]
    async fn profile_list_rejects_cyclic_remote_cursor_sequences() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (expected_cursor, next_cursor) in [
                (None, "prf_cursor_a"),
                (Some("prf_cursor_a"), "prf_cursor_b"),
                (Some("prf_cursor_b"), "prf_cursor_a"),
            ] {
                let (mut stream, _) = listener.accept().await.unwrap();
                let request = read_http_request(&mut stream).await;
                let request_line = request.lines().next().unwrap_or_default();
                if let Some(cursor) = expected_cursor {
                    assert!(request_line.contains(&format!("cursor={cursor}")));
                } else {
                    assert!(request_line.starts_with("GET /v1/profiles?limit=100 "));
                }
                let body = format!(r#"{{"items":[],"nextCursor":"{next_cursor}"}}"#);
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            }
        });

        let client =
            RemoteApiClient::new(&format!("http://{address}"), "device-token".into()).unwrap();
        let error = client
            .invoke(
                "profile_list",
                "profile-list-cycle-test",
                None,
                json!({"includeArchived":false}),
            )
            .await
            .expect_err("cursor cycle should fail instead of looping forever");
        assert_eq!(error.code, "api_profile_cursor_invalid");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn managed_profile_create_uses_the_rest_contract_and_redacts_bearer_debug() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let token = "device-token-secret-value".to_owned();
        let expected_id = format!(
            "prf_{}",
            &super::sha256_hex(b"ws_test:profile-create-test-key-0001")[..24]
        );
        let response_profile = Profile {
            id: ProfileId::from_str(&expected_id).unwrap(),
            name: "Browser profile".to_owned(),
            runtime: Runtime::Wayfern,
            runtime_version: Some("152.0".to_owned()),
            runtime_config: None,
            state: ProfileState::Ready,
            archived: false,
            fingerprint: None,
            note: None,
            proxy_id: Some(ProxyId::from_str("proxy_1").unwrap()),
            vpn_id: None,
            group_ids: Vec::new(),
            tags: Vec::new(),
            extension_group_id: None,
            sync_mode: Some(SyncMode::Managed),
            created_at: "2026-10-12T00:00:00Z".to_owned(),
            updated_at: "2026-10-12T00:00:00Z".to_owned(),
        };
        let response_body = serde_json::to_vec(&Versioned {
            schema_version: 1,
            revision: 1,
            value: response_profile,
        })
        .unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_http_request(&mut stream).await;
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                response_body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            stream.write_all(&response_body).await.unwrap();
            request
        });

        let client = RemoteApiClient::new(&format!("http://{address}"), token.clone()).unwrap();
        assert!(!format!("{client:?}").contains(&token));
        let result = client
            .invoke(
                "profile_create",
                "cor_profile_create_test",
                Some("profile-create-test-key-0001"),
                json!({
                    "name":"Browser profile",
                    "runtime":"wayfern",
                    "runtimeVersion":"152.0",
                    "workspaceId":"ws_test",
                    "proxyId":"proxy_1"
                }),
            )
            .await
            .unwrap();
        let request = server.await.unwrap();
        let (headers, body) = request.split_once("\r\n\r\n").unwrap();
        let request_body: Value = serde_json::from_str(body).unwrap();

        assert!(headers.starts_with("POST /v1/profiles HTTP/1.1"));
        assert!(
            headers
                .to_ascii_lowercase()
                .contains("authorization: bearer device-token-secret-value")
        );
        assert!(
            headers
                .to_ascii_lowercase()
                .contains("idempotency-key: profile-create-test-key-0001")
        );
        assert!(
            headers
                .to_ascii_lowercase()
                .contains("x-buglogin-workspace-id: ws_test")
        );
        assert!(request_body.get("id").is_none());
        assert!(request_body.get("note").is_none());
        assert!(request_body.get("syncMode").is_none());
        assert!(request_body.get("state").is_none());
        assert_eq!(request_body["name"], "Browser profile");
        assert_eq!(request_body["runtime"], "wayfern");
        assert_eq!(result.command_id, "profile.create");
        assert_eq!(result.data["id"], expected_id);
    }

    #[tokio::test]
    async fn proxy_sync_fails_closed_when_local_vault_secret_is_missing() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let response_body = br#"{"workspaceId":"ws_proxy_missing_secret","revision":7,"proxy":{"id":"pxy_proxy_missing_secret","protocol":"http","host":"proxy.example","port":8080,"hasCredential":true,"credentialVersion":"credential-test-v1","bypassRules":[]}}"#;
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_http_request(&mut stream).await;
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                response_body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            stream.write_all(response_body).await.unwrap();
            request
        });

        let client =
            RemoteApiClient::new(&format!("http://{address}"), "device-token".into()).unwrap();
        let workspace = WorkspaceId::from_str("ws_proxy_missing_secret").unwrap();
        let proxy = ProxyId::from_str("pxy_proxy_missing_secret").unwrap();
        let error = client
            .proxy_for_sync(&workspace, &proxy, 7, None, "cor_missing_proxy_secret")
            .await
            .expect_err("profile sync must not drop an unavailable proxy credential");
        assert_eq!(error.code, "proxy_secret_sync_unavailable");
        assert_eq!(error.status, 409);
        assert!(error.title.contains("operating system vault"));

        let request = server.await.unwrap();
        assert!(request.starts_with(
            "GET /v1/workspaces/ws_proxy_missing_secret/proxies/pxy_proxy_missing_secret HTTP/1.1"
        ));
    }

    #[tokio::test]
    async fn cookie_mutations_fail_before_any_remote_request() {
        let client = RemoteApiClient::new("http://127.0.0.1:0", "device-token".into()).unwrap();
        let commands = [
            (
                "cookie_stage",
                json!({"profileId":"prf_local","cookies":[{"value":"secret-cookie-value"}]}),
            ),
            (
                "cookie_import",
                json!({"profileId":"prf_local","stageId":"stage_local"}),
            ),
            (
                "cookie_copy",
                json!({"destinationProfileId":"prf_local","sourceProfileId":"prf_source"}),
            ),
        ];

        for (command, payload) in commands {
            let error = client
                .invoke(
                    command,
                    "cor_cookie_dispatch_required",
                    Some("cookie-key-00000001"),
                    payload,
                )
                .await
                .expect_err("unsupported production mutation must fail before transport");
            assert_eq!(error.code, "production_cookie_device_dispatch_required");
            assert_eq!(error.status, 503);
            assert!(error.retryable);
        }
    }

    #[tokio::test]
    async fn runtime_dispatch_transport_uses_device_bound_rest_routes() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let job = json!({
            "jobId": format!("runtime_{}", "a".repeat(64)),
            "operationId": "operation_runtime_dispatch",
            "commandId": "profile.launch",
            "workspaceId": "ws_runtime_dispatch",
            "deviceId": "device_runtime_dispatch",
            "profileId": "prf_runtime_dispatch",
            "requestedBy": "acct_runtime_dispatch",
            "idempotencyKey": "runtime-dispatch-test-key-0001",
            "createdAt": 100,
            "expiresAt": 220,
            "state": "accepted",
            "input": {"headless": false}
        });
        let mut claimed = job.clone();
        claimed["state"] = json!("running");
        let mut completed = claimed.clone();
        completed["state"] = json!("completed");
        completed.as_object_mut().unwrap().remove("input");
        completed["result"] = json!({"state":"running"});
        let mut failed = claimed.clone();
        failed["state"] = json!("failed");
        failed.as_object_mut().unwrap().remove("input");
        failed["failureCode"] = json!("runtime_operation_failed");
        let responses = vec![
            json!({"items":[job]}),
            json!({"job":claimed}),
            json!({"job":completed}),
            json!({"job":failed}),
        ];
        let server = tokio::spawn(async move {
            let mut requests = Vec::new();
            for body in responses {
                let (mut stream, _) = listener.accept().await.unwrap();
                requests.push(read_http_request(&mut stream).await);
                let body = serde_json::to_vec(&body).unwrap();
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
                stream.write_all(&body).await.unwrap();
            }
            requests
        });
        let client =
            RemoteApiClient::new(&format!("http://{address}"), "device-token".into()).unwrap();
        let workspace = WorkspaceId::new("ws_runtime_dispatch").unwrap();
        assert_eq!(
            client
                .claim_runtime_dispatch(&workspace, "../invalid", "runtime-invalid-id")
                .await
                .unwrap_err()
                .code,
            "runtime_job_id_invalid"
        );
        assert_eq!(
            client
                .runtime_dispatch_inbox("runtime-inbox-test")
                .await
                .unwrap()[0]
                .state,
            RuntimeDispatchState::Accepted
        );
        let job_id = format!("runtime_{}", "a".repeat(64));
        assert_eq!(
            client
                .claim_runtime_dispatch(&workspace, &job_id, "runtime-claim-test")
                .await
                .unwrap()
                .state,
            RuntimeDispatchState::Running
        );
        let receipt = json!({
            "jobId":job_id,
            "operationId":"operation_runtime_dispatch",
            "deviceId":"device_runtime_dispatch",
            "profileId":"prf_runtime_dispatch",
            "commandId":"profile.launch",
            "state":"running"
        });
        assert_eq!(
            client
                .complete_runtime_dispatch(&workspace, &job_id, receipt, "runtime-complete-test")
                .await
                .unwrap()
                .state,
            RuntimeDispatchState::Completed
        );
        assert_eq!(
            client
                .fail_runtime_dispatch(
                    &workspace,
                    &job_id,
                    "runtime_operation_failed",
                    "runtime-fail-test"
                )
                .await
                .unwrap()
                .state,
            RuntimeDispatchState::Failed
        );
        let requests = server.await.unwrap();
        assert!(requests[0].starts_with("GET /v1/device/runtime-dispatch/inbox HTTP/1.1"));
        assert!(requests[1].starts_with(&format!(
            "POST /v1/workspaces/ws_runtime_dispatch/runtime-dispatch/{job_id}/claim HTTP/1.1"
        )));
        assert!(requests[2].starts_with(&format!(
            "POST /v1/workspaces/ws_runtime_dispatch/runtime-dispatch/{job_id}/complete HTTP/1.1"
        )));
        assert!(requests[3].starts_with(&format!(
            "POST /v1/workspaces/ws_runtime_dispatch/runtime-dispatch/{job_id}/fail HTTP/1.1"
        )));
    }

    async fn read_http_request(stream: &mut tokio::net::TcpStream) -> String {
        let mut bytes = Vec::new();
        let mut chunk = [0; 2048];
        loop {
            let count = stream.read(&mut chunk).await.unwrap();
            assert!(count > 0, "client closed before sending request");
            bytes.extend_from_slice(&chunk[..count]);
            if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                let body_start = end + 4;
                let headers = String::from_utf8_lossy(&bytes[..end]);
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())
                            .flatten()
                    })
                    .unwrap_or(0);
                if bytes.len() >= body_start + content_length {
                    return String::from_utf8(bytes).unwrap();
                }
            }
        }
    }
}
