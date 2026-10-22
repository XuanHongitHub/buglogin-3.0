use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProxyProtocol {
    Http,
    Https,
    Socks4,
    Socks5,
}

#[derive(Clone, Eq, PartialEq, Serialize)]
pub struct ProxyRow {
    pub protocol: ProxyProtocol,
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    #[serde(skip_serializing)]
    pub password: Option<String>,
}

impl Drop for ProxyRow {
    fn drop(&mut self) {
        self.zeroize_credentials();
    }
}

impl ProxyRow {
    fn zeroize_credentials(&mut self) {
        if let Some(username) = &mut self.username {
            username.zeroize();
        }
        if let Some(password) = &mut self.password {
            password.zeroize();
        }
    }
}

impl std::fmt::Debug for ProxyRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProxyRow")
            .field("protocol", &self.protocol)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("username", &self.username.as_ref().map(|_| "<redacted>"))
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::{ProxyProtocol, ProxyRow};

    #[test]
    fn zeroize_credentials_clears_parsed_proxy_secrets() {
        let mut row = ProxyRow {
            protocol: ProxyProtocol::Http,
            host: "proxy.example".to_owned(),
            port: 8080,
            username: Some("user-secret".to_owned()),
            password: Some("password-secret".to_owned()),
        };

        row.zeroize_credentials();

        assert_eq!(row.username.as_deref(), Some(""));
        assert_eq!(row.password.as_deref(), Some(""));
    }
}
