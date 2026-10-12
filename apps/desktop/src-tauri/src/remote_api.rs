use std::{
    collections::BTreeMap,
    fmt,
    sync::{Arc, RwLock},
    time::Duration,
};

use buglogin_domain::{
    entities::{Profile, ProfileState, ProxyProtocol, Runtime, SyncMode},
    errors::Problem,
    ids::{ProfileId, ProxyId, ResourceId, WorkspaceId},
    profile::ProfileResource,
};
use buglogin_storage::local::Versioned;
use buglogin_storage::secrets::SecretValue;
use buglogin_sync::{
    coordination::{
        CoordinationAuth, CoordinationBackend, CoordinationEvent, CoordinationScope, LeaseGrant,
        Presence, PublishedEvent, StreamEvent,
    },
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProxyView {
    id: ProxyId,
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
    pub metadata: Vec<u8>,
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
        let metadata = serde_json::to_vec(&json!({"workspaceId":record.workspace_id,"proxy":{
            "id":record.proxy.id,"name":record.proxy.name,"protocol":record.proxy.protocol,
            "host":record.proxy.host,"port":record.proxy.port,"hasCredential":record.proxy.has_credential,
            "rotationConfigured":record.proxy.rotation_configured,"bypassRules":record.proxy.bypass_rules,
            "createdAt":record.proxy.created_at,"updatedAt":record.proxy.updated_at
        },"revision":record.revision})).map_err(|_| client_problem("api_response_invalid", 502))?;
        Ok(RemoteProxySyncSnapshot {
            proxy_id: id.clone(),
            revision: record.revision,
            metadata,
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
                let profiles:Vec<Versioned<Profile>>=self.get("/v1/profiles",correlation).await?;
                let items=profiles.into_iter().filter(|profile|include_archived||!profile.value.archived).map(|profile|{
                    let revision=profile.revision;
                    let mut value=serde_json::to_value(profile.value).map_err(|_|client_problem("api_response_invalid",502))?;
                    value["revision"]=json!(revision);
                    Ok(value)
                }).collect::<Result<Vec<_>,Problem>>()?;
                json!({"items":items,"nextCursor":Value::Null})
            },
            "profile_get" => { let id=path_arg(&payload,"profileId",correlation)?; self.get::<Value>(&format!("/v1/profiles/{id}"),correlation).await? },
            "profile_create" => self.create_managed_profile(payload,correlation,idem).await?,
            "profile_delete" => { let id=path_arg(&payload,"profileId",correlation)?; self.send::<Value>(Method::DELETE,&format!("/v1/profiles/{id}"),Some(payload),correlation,idem,None,&[]).await? },
            "profile_archive" | "profile_restore" => { let id=path_arg(&payload,"profileId",correlation)?; let action=if name=="profile_archive" {"archive"} else {"restore"}; let body=json!({"revision":payload.get("revision").and_then(Value::as_u64).ok_or_else(||bad_input("revision",correlation))?}); let updated:Versioned<Profile>=self.send(Method::PUT,&format!("/v1/profiles/{id}/{action}"),Some(body),correlation,idem,None,&[]).await?; serde_json::to_value(updated.value).map_err(|_|client_problem("api_response_invalid",502))? },
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
            "cookie_stage" => { let id=path_arg(&payload,"profileId",correlation)?; self.send(Method::POST,&format!("/v1/profiles/{id}/cookies/stage"),Some(payload),correlation,idem,None,&[]).await? },
            "cookie_import" => { let id=path_arg(&payload,"profileId",correlation)?; self.send(Method::POST,&format!("/v1/profiles/{id}/cookies/import"),Some(payload),correlation,idem,None,&[]).await? },
            "cookie_copy" => { let id=path_arg(&payload,"destinationProfileId",correlation)?; self.send(Method::POST,&format!("/v1/profiles/{id}/cookies/copy"),Some(payload),correlation,idem,None,&[]).await? },
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
        let name = input
            .get("name")
            .and_then(Value::as_str)
            .filter(|v| !v.trim().is_empty())
            .ok_or_else(|| bad_input("name", correlation))?
            .trim();
        let runtime: Runtime = serde_json::from_value(
            input
                .get("runtime")
                .cloned()
                .ok_or_else(|| bad_input("runtime", correlation))?,
        )
        .map_err(|_| bad_input("runtime", correlation))?;
        let workspace = input.get("workspaceId").and_then(Value::as_str);
        let key = idem.ok_or_else(|| {
            Problem::new(
                "idempotency_key_required",
                "A profile create key is required",
                400,
                correlation,
            )
        })?;
        let seed = format!("{}:{key}", workspace.unwrap_or_default());
        let id = format!("prf_{}", &sha256_hex(seed.as_bytes())[..24]);
        let expected_id =
            ProfileId::from_str(&id).map_err(|_| client_problem("api_request_invalid", 400))?;
        let timestamp = time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(|_| client_problem("api_clock_invalid", 500))?;
        let profile = json!({"id":id,"name":name,"runtime":runtime,"runtimeVersion":input.get("runtimeVersion"),"runtimeConfig":null,"state":ProfileState::Created,"archived":false,"fingerprint":null,"note":null,"proxyId":input.get("proxyId"),"vpnId":null,"groupIds":[],"tags":[],"extensionGroupId":null,"syncMode":SyncMode::Managed,"createdAt":timestamp,"updatedAt":timestamp});
        let extra = workspace
            .map(|workspace| vec![("x-buglogin-workspace-id", workspace.to_owned())])
            .unwrap_or_default();
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
        if created.value.id != expected_id
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
        serde_json::to_value(created.value).map_err(|_| client_problem("api_response_invalid", 502))
    }
}

#[async_trait::async_trait]
impl CoordinationBackend for RemoteApiClient {
    async fn acquire_lease(
        &self,
        scope: CoordinationScope<'_>,
        resource: &ResourceId,
        ttl: Duration,
    ) -> Result<LeaseGrant, Problem> {
        self.acquire_lease_idempotent(
            scope,
            resource,
            ttl,
            &format!("lease-{}-{}", resource.as_str(), uuid_key()),
        )
        .await
    }
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
    async fn heartbeat_presence(
        &self,
        _: CoordinationScope<'_>,
        _: &str,
        _: BTreeMap<String, String>,
        _: Duration,
    ) -> Result<Presence, Problem> {
        Err(client_problem("api_presence_unsupported", 501))
    }
    async fn list_presence(&self, _: CoordinationScope<'_>) -> Result<Vec<Presence>, Problem> {
        Err(client_problem("api_presence_unsupported", 501))
    }
    async fn publish_event(
        &self,
        _: CoordinationScope<'_>,
        _: &str,
        _: CoordinationEvent,
    ) -> Result<PublishedEvent, Problem> {
        Err(client_problem("api_events_unsupported", 501))
    }
    async fn read_events(
        &self,
        _: CoordinationScope<'_>,
        _: Option<&str>,
        _: usize,
    ) -> Result<Vec<StreamEvent>, Problem> {
        Err(client_problem("api_events_unsupported", 501))
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
        manifest: SyncManifest,
    ) -> Result<(), Problem> {
        let extra = [
            ("x-buglogin-owner-id", scope.auth.actor_id().to_owned()),
            ("x-buglogin-owner-token", owner_token.to_owned()),
            (
                "x-buglogin-fencing-token",
                manifest.fencing_token.to_string(),
            ),
        ];
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
    use buglogin_domain::entities::{Profile, ProfileState, Runtime, SyncMode};
    use buglogin_domain::ids::{ProfileId, ProxyId, ResourceId, WorkspaceId};
    use buglogin_storage::local::Versioned;
    use buglogin_sync::coordination::{CoordinationAuth, CoordinationScope};
    use serde_json::{Value, json};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

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
        assert_eq!(request_body["id"], expected_id);
        assert_eq!(request_body["syncMode"], "managed");
        assert_eq!(request_body["state"], "created");
        assert_eq!(result.command_id, "profile.create");
        assert_eq!(result.data["id"], expected_id);
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
