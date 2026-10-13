use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    sync::{Arc, RwLock},
    time::Duration,
};

use crate::profile_runtime::DesktopSyncBackend;
use buglogin_api::services::connected_cookie_jobs::{ConnectedCookieJob, ConnectedCookieReceipt};
use buglogin_api::services::profile_proxy_assignment::ProfileProxyAssignment;
use buglogin_domain::{
    entities::{Profile, ProfileCreateInput, ProfileRecord, ProfileState, ProxyProtocol, SyncMode},
    errors::Problem,
    ids::{ProfileId, ProxyId, ResourceId, WorkspaceId},
    profile::ProfileResource,
};
use buglogin_storage::local::Versioned;
use buglogin_storage::secrets::SecretValue;
use buglogin_sync::{
    coordination::{CoordinationAuth, CoordinationScope, LeaseGrant},
    manifest::SyncManifest,
};
use reqwest::{Method, StatusCode, Url, header};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};

#[derive(Clone)]
pub struct RemoteApiClient {
    base_url: Url,
    http: reqwest::Client,
    token: Arc<RwLock<Option<String>>>,
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

impl fmt::Debug for RemoteApiClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RemoteApiClient")
            .field("base_url", &self.base_url)
            .field("token", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeviceAuthorizationStart {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    pub interval: u64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeviceAuthorizationToken {
    pub access_token: String,
    pub token_type: String,
    pub expires_at: u64,
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
}

#[derive(Clone, Debug)]
pub struct RemoteProxySyncSnapshot {
    pub proxy_id: ProxyId,
    pub revision: u64,
    /// Plaintext exists only in Rust memory until ProfileProjection seals it
    /// with the selected profile resource key.
    pub credential: Option<SecretValue>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiProblem {
    code: String,
    title: String,
    status: u16,
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
        let mut base_url = Url::parse(base).map_err(|_| client_problem("api_url_invalid", 400))?;
        let local_http = base_url.scheme() == "http"
            && matches!(base_url.host_str(), Some("127.0.0.1" | "localhost" | "::1"));
        if (base_url.scheme() != "https" && !local_http)
            || base_url.host_str().is_none()
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
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(|_| client_problem("api_client_unavailable", 503))?;
        Ok(Self {
            base_url,
            http,
            token: Arc::new(RwLock::new(token)),
        })
    }

    pub fn has_token(&self) -> Result<bool, Problem> {
        self.token
            .read()
            .map(|v| v.is_some())
            .map_err(|_| client_problem("api_client_unavailable", 503))
    }

    pub fn set_token(&self, token: String) -> Result<(), Problem> {
        if !valid_bearer(&token) {
            return Err(client_problem("api_token_invalid", 400));
        }
        *self
            .token
            .write()
            .map_err(|_| client_problem("api_client_unavailable", 503))? = Some(token);
        Ok(())
    }

    pub fn clear_token(&self) -> Result<(), Problem> {
        *self
            .token
            .write()
            .map_err(|_| client_problem("api_client_unavailable", 503))? = None;
        Ok(())
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
        let mut request = self
            .http
            .request(method, url)
            .header("x-correlation-id", correlation);
        let token = match auth {
            Some(auth) => Some(auth.bearer_token().to_owned()),
            None => self
                .token
                .read()
                .map_err(|_| client_problem("api_client_unavailable", 503))?
                .clone(),
        };
        if let Some(token) = token {
            request = request.bearer_auth(token);
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
            .retryable(error.is_timeout() || error.is_connect())
        })?;
        let status = response.status();
        if !status.is_success() {
            return Err(read_problem(response, correlation).await);
        }
        if status == StatusCode::NO_CONTENT {
            return serde_json::from_value(Value::Null)
                .map_err(|_| client_problem("api_response_invalid", 502));
        }
        response
            .json()
            .await
            .map_err(|_| client_problem("api_response_invalid", 502))
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
        let mut request = self
            .http
            .request(method, url)
            .bearer_auth(auth.bearer_token())
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
            .retryable(error.is_timeout() || error.is_connect())
        })?;
        if !response.status().is_success() {
            return Err(read_problem(response, correlation).await);
        }
        response
            .json()
            .await
            .map_err(|_| client_problem("api_response_invalid", 502))
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
        let response = self
            .http
            .post(url)
            .bearer_auth(self.token_value()?)
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
                .retryable(error.is_timeout() || error.is_connect())
            })?;
        parse_response(response, correlation).await
    }

    async fn get<T: DeserializeOwned>(&self, path: &str, correlation: &str) -> Result<T, Problem> {
        self.send(Method::GET, path, None, correlation, None, None, &[])
            .await
    }

    pub async fn device_authorization_start(
        &self,
        device_id: &str,
        client_name: &str,
        correlation: &str,
    ) -> Result<DeviceAuthorizationStart, Problem> {
        let started: DeviceAuthorizationStart = self
            .send(
                Method::POST,
                "/v1/auth/device/start",
                Some(json!({"deviceId":device_id,"clientName":client_name})),
                correlation,
                None,
                None,
                &[],
            )
            .await?;
        let verification = Url::parse(&started.verification_uri).map_err(|_| {
            Problem::new(
                "api_auth_response_invalid",
                "BugLogin returned an invalid verification URL",
                502,
                correlation,
            )
        })?;
        let local_http = verification.scheme() == "http"
            && matches!(
                verification.host_str(),
                Some("127.0.0.1" | "localhost" | "::1")
            );
        if (verification.scheme() != "https" && !local_http)
            || started.device_code.is_empty()
            || started.user_code.is_empty()
            || started.expires_in == 0
            || started.interval == 0
        {
            return Err(Problem::new(
                "api_auth_response_invalid",
                "BugLogin returned invalid device authorization details",
                502,
                correlation,
            ));
        }
        Ok(started)
    }

    pub async fn device_authorization_token(
        &self,
        device_code: &str,
        correlation: &str,
    ) -> Result<DeviceAuthorizationToken, Problem> {
        let token: DeviceAuthorizationToken = self
            .send(
                Method::POST,
                "/v1/auth/device/token",
                Some(json!({"deviceCode":device_code})),
                correlation,
                None,
                None,
                &[],
            )
            .await?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        if token.access_token.is_empty() || token.token_type != "Bearer" || token.expires_at <= now
        {
            return Err(Problem::new(
                "api_auth_response_invalid",
                "BugLogin returned an invalid device token",
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
        correlation: &str,
    ) -> Result<ConnectedCookieJob, Problem> {
        validate_cookie_job_id(job_id)?;
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
            &[],
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
        let path = format!(
            "/v1/workspaces/{}/resources/{}/blobs?key={}",
            scope.workspace_id.as_str(),
            resource.as_str(),
            urlencoding(key)
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
            return Err(read_problem(response, correlation).await);
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|_| client_problem("api_response_invalid", 502))?;
        if bytes.len() as u64 != expected_size || sha256_hex(&bytes) != checksum {
            return Err(client_problem("api_blob_integrity_failed", 502));
        }
        Ok(bytes.to_vec())
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
            "organization_list" => json!({"items":self.get::<Vec<Value>>("/v1/organizations",correlation).await?,"nextCursor":Value::Null}),
            "workspace_list" => { let org=path_arg(&payload,"organizationId",correlation)?; json!({"items":self.get::<Vec<Value>>(&format!("/v1/organizations/{org}/workspaces"),correlation).await?,"nextCursor":Value::Null}) },
            "profile_list" => {
                let include_archived=payload.get("includeArchived").and_then(Value::as_bool).unwrap_or(false);
                let mut profiles = Vec::new();
                let mut cursor: Option<String> = None;
                let mut seen_cursors = BTreeSet::new();
                loop {
                    let path = match cursor.as_deref() {
                        Some(cursor) => format!("/v1/profiles?limit=100&cursor={}", urlencoding(cursor)),
                        None => "/v1/profiles?limit=100".to_owned(),
                    };
                    let page: VersionedProfilePage = self.get(&path, correlation).await?;
                    let next_cursor = page.next_cursor.clone();
                    profiles.extend(page.items);
                    match next_cursor {
                        Some(next)
                            if next.len() <= 512 && seen_cursors.insert(next.clone()) =>
                        {
                            cursor = Some(next)
                        }
                        Some(_) => return Err(client_problem("api_profile_cursor_invalid", 502)),
                        None => break,
                    }
                }
                let items=profiles.into_iter().filter(|profile|include_archived||!profile.value.archived).map(|profile|{
                    profile_record_value(profile)
                }).collect::<Result<Vec<_>,Problem>>()?;
                json!({"items":items,"nextCursor":Value::Null})
            },
            "profile_get" => { let id=path_arg(&payload,"profileId",correlation)?; let record:Versioned<Profile>=self.get(&format!("/v1/profiles/{id}"),correlation).await?; profile_record_value(record)? },
            "profile_create" => self.create_managed_profile(payload,correlation,idem).await?,
            "profile_delete" => { let id=path_arg(&payload,"profileId",correlation)?; self.send::<Value>(Method::DELETE,&format!("/v1/profiles/{id}"),Some(payload),correlation,idem,None,&[]).await? },
            "profile_archive" | "profile_restore" => { let id=path_arg(&payload,"profileId",correlation)?; let action=if name=="profile_archive" {"archive"} else {"restore"}; let body=json!({"revision":payload.get("revision").and_then(Value::as_u64).ok_or_else(||bad_input("revision",correlation))?}); let updated:Versioned<Profile>=self.send(Method::PUT,&format!("/v1/profiles/{id}/{action}"),Some(body),correlation,idem,None,&[]).await?; profile_record_value(updated)? },
            "profile_clone" => { let id=path_arg(&payload,"sourceProfileId",correlation)?; self.send::<Value>(Method::POST,&format!("/v1/profiles/{id}/clone"),Some(payload),correlation,idem,None,&[]).await? },
            "profile_share" => { let id=path_arg(&payload,"profileId",correlation)?; self.send::<Value>(Method::POST,&format!("/v1/profiles/{id}/shares"),Some(payload),correlation,idem,None,&[]).await? },
            "profile_share_revoke" => { let id=path_arg(&payload,"profileId",correlation)?; let member=path_arg(&payload,"memberId",correlation)?; self.send::<Value>(Method::DELETE,&format!("/v1/profiles/{id}/shares/{member}"),Some(payload),correlation,idem,None,&[]).await? },
            "proxy_parse" => { let text=payload.get("text").and_then(Value::as_str).ok_or_else(||bad_input("text",correlation))?; self.send_text("/v1/proxies/parse",text.to_owned(),correlation).await? },
            "proxy_list" => { let ws=path_arg(&payload,"workspaceId",correlation)?; self.get::<Value>(&format!("/v1/workspaces/{ws}/proxies"),correlation).await? },
            "proxy_get" => { let ws=path_arg(&payload,"workspaceId",correlation)?; let id=path_arg(&payload,"proxyId",correlation)?; self.get::<Value>(&format!("/v1/workspaces/{ws}/proxies/{id}"),correlation).await? },
            "proxy_create" => { let ws=path_arg(&payload,"workspaceId",correlation)?; self.send::<Value>(Method::POST,&format!("/v1/workspaces/{ws}/proxies"),Some(payload),correlation,idem,None,&[]).await? },
            "proxy_update" => { let ws=path_arg(&payload,"workspaceId",correlation)?; let id=path_arg(&payload,"proxyId",correlation)?; self.send::<Value>(Method::PATCH,&format!("/v1/workspaces/{ws}/proxies/{id}"),Some(payload),correlation,idem,None,&[]).await? },
            "proxy_delete" => { let ws=path_arg(&payload,"workspaceId",correlation)?; let id=path_arg(&payload,"proxyId",correlation)?; self.send::<Value>(Method::DELETE,&format!("/v1/workspaces/{ws}/proxies/{id}"),Some(payload),correlation,idem,None,&[]).await? },
            "cookie_list" => { let id=path_arg(&payload,"profileId",correlation)?; let ws=path_arg(&payload,"workspaceId",correlation)?; self.get::<Value>(&format!("/v1/profiles/{id}/cookies?workspaceId={ws}"),correlation).await? },
            "cookie_stage" | "cookie_import" | "cookie_copy" => {
                return Err(Problem::new(
                    "production_cookie_device_dispatch_required",
                    "Cookie mutations must execute on the authorized connected device",
                    503,
                    correlation,
                ).retryable(true));
            }
            "grant_list" => { let ws=path_arg(&payload,"workspaceId",correlation)?; let mut path=format!("/v1/workspaces/{ws}/grants");let mut query=Vec::new();for key in ["resourceId","memberId"]{if let Some(value)=payload.get(key).and_then(Value::as_str){query.push(format!("{key}={}",urlencoding(value)));}}if !query.is_empty(){path.push('?');path.push_str(&query.join("&"));}self.get::<Value>(&path,correlation).await? },
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

    fn token_value(&self) -> Result<String, Problem> {
        self.token
            .read()
            .map_err(|_| client_problem("api_client_unavailable", 503))?
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
    async fn renew_lease(
        &self,
        scope: CoordinationScope<'_>,
        resource: &ResourceId,
        owner_token: &str,
        ttl: Duration,
    ) -> Result<LeaseGrant, Problem> {
        let grant:LeaseGrant=self.send(Method::PUT,&format!("/v1/workspaces/{}/resources/{}/lease",scope.workspace_id.as_str(),resource.as_str()),Some(json!({"ownerId":scope.auth.actor_id(),"ownerToken":owner_token,"ttlSeconds":ttl.as_secs()})),"desktop-lease",Some(&format!("renew-{}",uuid_key())),Some(scope.auth),&[]).await?;
        Ok(grant)
    }
    async fn release_lease(
        &self,
        scope: CoordinationScope<'_>,
        resource: &ResourceId,
        owner_token: &str,
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
                Some(&format!("release-{}", uuid_key())),
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
        let idem = format!(
            "manifest-{}-{}-{}",
            resource.as_str(),
            manifest.generation,
            manifest.root_checksum
        );
        let _: SyncManifest = self
            .send(
                Method::PUT,
                &format!(
                    "/v1/workspaces/{}/resources/{}/manifest",
                    scope.workspace_id.as_str(),
                    resource.as_str()
                ),
                Some(
                    serde_json::to_value(manifest)
                        .map_err(|_| client_problem("api_request_invalid", 400))?,
                ),
                "desktop-sync",
                Some(&idem),
                Some(scope.auth),
                &extra,
            )
            .await?;
        Ok(())
    }
}

impl RemoteApiClient {
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
        match DesktopSyncBackend::commit_manifest(
            self,
            scope.clone(),
            resource,
            owner_token,
            expected_profile_revision,
            manifest.clone(),
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

async fn read_problem(response: reqwest::Response, correlation: &str) -> Problem {
    let status = response.status().as_u16();
    match response.json::<ApiProblem>().await {
        Ok(value) => Problem {
            problem_type: format!("https://buglogin.com/problems/{}", value.code),
            title: value.title,
            status: value.status,
            code: value.code,
            detail: value.detail,
            instance: None,
            correlation_id: value
                .correlation_id
                .unwrap_or_else(|| correlation.to_owned()),
            retryable: value.retryable.unwrap_or(status >= 500 || status == 429),
            fields: value.fields.unwrap_or_default(),
        },
        Err(_) => Problem::new(
            "api_request_failed",
            "BugLogin API request failed",
            status,
            correlation,
        )
        .retryable(status >= 500 || status == 429),
    }
}

async fn parse_response(response: reqwest::Response, correlation: &str) -> Result<Value, Problem> {
    let status = response.status();
    if !status.is_success() {
        return Err(read_problem(response, correlation).await);
    }
    response
        .json()
        .await
        .map_err(|_| client_problem("api_response_invalid", 502))
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
        "organization_list" => "organization.list",
        "workspace_list" => "workspace.list",
        "operation_get" => "operation.get",
        "migration_inspect" => "migration.inspect",
        "migration_import" => "migration.import",
        "profile_list" => "profile.list",
        "profile_get" => "profile.get",
        "profile_create" => "profile.create",
        "profile_delete" => "profile.delete",
        "profile_archive" => "profile.archive",
        "profile_restore" => "profile.restore",
        "profile_clone" => "profile.clone",
        "profile_share" => "profile.share",
        "profile_share_revoke" => "profile.share.revoke",
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
        "grant_list" => "grant.list",
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
fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn uuid_key() -> String {
    format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    )
}

#[cfg(test)]
mod tests {
    use super::RemoteApiClient;
    use crate::profile_runtime::DesktopSyncBackend;
    use buglogin_api::services::connected_cookie_jobs::{
        ConnectedCookieJob, ConnectedCookieJobState, ConnectedCookieReceipt,
    };
    use buglogin_domain::entities::{Profile, ProfileState, Runtime, SyncMode};
    use buglogin_domain::ids::{ProfileId, ProxyId, ResourceId, WorkspaceId};
    use buglogin_storage::local::Versioned;
    use buglogin_sync::coordination::{CoordinationAuth, CoordinationScope, LeaseGrant};
    use buglogin_sync::manifest::SyncManifest;
    use serde_json::{Value, json};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

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
        };
        let responses = vec![
            serde_json::json!({"items":[server_job(ConnectedCookieJobState::Accepted, None)]}),
            serde_json::to_value(server_job(ConnectedCookieJobState::Dispatched, None)).unwrap(),
            serde_json::to_value(server_job(ConnectedCookieJobState::Running, None)).unwrap(),
            serde_json::to_value(server_job(
                ConnectedCookieJobState::Completed,
                Some(receipt.clone()),
            ))
            .unwrap(),
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
                .acknowledge_cookie_job(&workspace, &inbox[0].job_id, "cookie-ack-test")
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
        let requests = server.await.unwrap();
        let expected_paths = [
            format!("GET /v1/workspaces/{workspace_text}/cookie-jobs/inbox HTTP/1.1"),
            format!("POST /v1/cookie-jobs/{job_id}/claim?workspaceId={workspace_text} HTTP/1.1"),
            format!("POST /v1/cookie-jobs/{job_id}/ack?workspaceId={workspace_text} HTTP/1.1"),
            format!("POST /v1/cookie-jobs/{job_id}/complete?workspaceId={workspace_text} HTTP/1.1"),
        ];
        for (request, expected_path) in requests.iter().zip(expected_paths) {
            let (headers, body) = request.split_once("\r\n\r\n").unwrap();
            assert!(headers.starts_with(&expected_path));
            assert!(
                headers
                    .to_ascii_lowercase()
                    .contains("authorization: bearer device-token")
            );
            if !body.is_empty() {
                let payload: Value = serde_json::from_str(body).unwrap();
                assert_eq!(payload["metadataChecksum"], "b".repeat(64));
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
        assert!(
            commit_request
                .to_ascii_lowercase()
                .contains("idempotency-key: manifest-res_reconcile-1-")
        );
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
                (None, "cursor-a"),
                (Some("cursor-a"), "cursor-b"),
                (Some("cursor-b"), "cursor-a"),
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
        let response_body = br#"{"workspaceId":"ws_proxy_missing_secret","revision":7,"proxy":{"id":"pxy_proxy_missing_secret","protocol":"http","host":"proxy.example","port":8080,"hasCredential":true,"bypassRules":[]}}"#;
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
