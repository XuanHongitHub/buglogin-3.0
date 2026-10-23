use std::time::Duration;

use buglogin_domain::errors::Problem;
use reqwest::{Client, Url};
use serde::Deserialize;
use zeroize::Zeroizing;

const IDP_ORIGIN: &str = "https://id.buglab.vn";
const MAX_PROVIDER_RESPONSE_BYTES: usize = 32 * 1024;

#[derive(Clone, Debug)]
pub(crate) struct BugLabSsoConfig {
    pub app_code: String,
    pub callback_uri: Url,
}

impl BugLabSsoConfig {
    pub fn from_env() -> Result<Option<Self>, Problem> {
        if !matches!(std::env::var("BUGLAB_SSO_ENABLED").as_deref(), Ok("true")) {
            return Ok(None);
        }
        let app_code = std::env::var("BUGLAB_SSO_APP_CODE").ok();
        let callback_uri = std::env::var("BUGLAB_SSO_CALLBACK_URI").ok();
        let (Some(app_code), Some(callback_uri)) = (app_code, callback_uri) else {
            return Ok(None);
        };
        let app_code = app_code.trim();
        if !(2..=64).contains(&app_code.len())
            || !app_code
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(config_invalid());
        }
        let callback_uri = Url::parse(callback_uri.trim()).map_err(|_| config_invalid())?;
        if callback_uri.scheme() != "buglogin"
            || callback_uri.host_str() != Some("sso")
            || callback_uri.path() != "/callback"
            || callback_uri.query().is_some()
            || callback_uri.fragment().is_some()
            || callback_uri.username() != ""
            || callback_uri.password().is_some()
        {
            return Err(config_invalid());
        }
        Ok(Some(Self {
            app_code: app_code.to_owned(),
            callback_uri,
        }))
    }

    pub fn authorization_url(&self, state: &str) -> Result<Url, Problem> {
        let mut url =
            Url::parse(&format!("{IDP_ORIGIN}/sso/login")).map_err(|_| provider_unavailable())?;
        url.query_pairs_mut()
            .append_pair("app", &self.app_code)
            .append_pair("redirect_uri", self.callback_uri.as_str())
            .append_pair("state", state);
        Ok(url)
    }
}

#[derive(Clone)]
pub(crate) struct BugLabSsoClient {
    http: Client,
}

#[derive(Debug, Deserialize)]
pub(crate) struct BugLabExchangeResponse {
    pub success: bool,
    #[serde(default)]
    pub user: Option<BugLabUser>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct BugLabUser {
    #[serde(deserialize_with = "deserialize_subject")]
    pub user_id: String,
    pub name: Option<String>,
    pub email: String,
    pub status: String,
    pub app: String,
    pub old_account: Option<String>,
}

impl BugLabSsoClient {
    pub fn new() -> Result<Self, Problem> {
        let http = Client::builder()
            .https_only(true)
            .timeout(Duration::from_secs(5))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| provider_unavailable())?;
        Ok(Self { http })
    }

    pub async fn exchange(&self, token: &str, app_code: &str) -> Result<BugLabUser, Problem> {
        let token = Zeroizing::new(token.to_owned());
        let endpoint = format!("{IDP_ORIGIN}/api/v1/sso/exchange");
        let response = self
            .http
            .post(endpoint)
            .header(reqwest::header::ACCEPT, "application/json")
            .json(&serde_json::json!({ "token": token.as_str() }))
            .send()
            .await
            .map_err(|_| provider_unavailable())?;
        if !response.status().is_success()
            || response
                .content_length()
                .is_some_and(|length| length > MAX_PROVIDER_RESPONSE_BYTES as u64)
        {
            return Err(provider_rejected());
        }
        use futures_util::StreamExt;
        let mut stream = response.bytes_stream();
        let mut bytes = Vec::with_capacity(MAX_PROVIDER_RESPONSE_BYTES.min(4096));
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| provider_unavailable())?;
            if bytes.len().saturating_add(chunk.len()) > MAX_PROVIDER_RESPONSE_BYTES {
                return Err(provider_rejected());
            }
            bytes.extend_from_slice(&chunk);
        }
        let result: BugLabExchangeResponse =
            serde_json::from_slice(&bytes).map_err(|_| provider_rejected())?;
        if !result.success {
            return Err(provider_rejected());
        }
        let user = result.user.ok_or_else(provider_rejected)?;
        if user.app != app_code
            || user.status != "active"
            || user.user_id.is_empty()
            || user.user_id.len() > 128
            || user.user_id.chars().any(char::is_control)
            || user.email.len() > 254
            || user.email.trim() != user.email
            || user.name.as_ref().is_some_and(|name| name.len() > 256)
            || user
                .old_account
                .as_ref()
                .is_some_and(|account| account.len() > 128 || account.chars().any(char::is_control))
        {
            return Err(provider_rejected());
        }
        Ok(user)
    }
}

fn deserialize_subject<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Subject {
        Text(String),
        Number(u64),
    }
    match Subject::deserialize(deserializer)? {
        Subject::Text(value) => Ok(value),
        Subject::Number(value) => Ok(value.to_string()),
    }
}

fn config_invalid() -> Problem {
    Problem::new(
        "buglab_sso_configuration_invalid",
        "BugLab ID sign-in is not configured correctly",
        503,
        "identity",
    )
}

fn provider_unavailable() -> Problem {
    Problem::new(
        "identity_provider_unavailable",
        "BugLab ID is temporarily unavailable",
        503,
        "identity",
    )
    .retryable(true)
}

fn provider_rejected() -> Problem {
    Problem::new(
        "identity_provider_response_invalid",
        "BugLab ID could not verify this sign-in",
        401,
        "identity",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorization_url_keeps_callback_and_state_as_query_values() {
        let config = BugLabSsoConfig {
            app_code: "buglogin".into(),
            callback_uri: Url::parse("buglogin://sso/callback").unwrap(),
        };

        let url = config.authorization_url(&"ab".repeat(32)).unwrap();
        let pairs: std::collections::BTreeMap<_, _> = url.query_pairs().into_owned().collect();
        let expected_state = "ab".repeat(32);

        assert_eq!(url.scheme(), "https");
        assert_eq!(url.host_str(), Some("id.buglab.vn"));
        assert_eq!(url.path(), "/sso/login");
        assert_eq!(pairs.get("app").map(String::as_str), Some("buglogin"));
        assert_eq!(
            pairs.get("redirect_uri").map(String::as_str),
            Some("buglogin://sso/callback")
        );
        assert_eq!(
            pairs.get("state").map(String::as_str),
            Some(expected_state.as_str())
        );
        assert_eq!(pairs.len(), 3);
    }

    #[test]
    fn provider_subject_accepts_numeric_or_string_id() {
        #[derive(Deserialize)]
        struct SubjectResponse {
            #[serde(deserialize_with = "deserialize_subject")]
            user_id: String,
        }

        assert_eq!(
            serde_json::from_str::<SubjectResponse>(r#"{"user_id":15}"#)
                .unwrap()
                .user_id,
            "15"
        );
        assert_eq!(
            serde_json::from_str::<SubjectResponse>(r#"{"user_id":"user-15"}"#)
                .unwrap()
                .user_id,
            "user-15"
        );
    }
}
