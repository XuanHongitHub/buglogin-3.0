use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use buglogin_domain::entities::{Proxy, ProxyProtocol};
use buglogin_domain::errors::Problem;
use buglogin_domain::ids::{CredentialId, ProxyId};
use buglogin_network::proxy::{ProxyProtocol as ParsedProtocol, ProxyRow};
use buglogin_storage::local::LocalRepository;
use buglogin_storage::secrets::{OsVault, SecretKey, SecretValue};
use buglogin_storage::vault::NativeVault;
use serde::Serialize;

use crate::proxy_bridge::ProxyBridgeSpec;
use sha2::{Digest, Sha256};

#[derive(Clone)]
pub struct DesktopProxyStore {
    repository: LocalRepository,
    vault: Arc<Mutex<Box<dyn OsVault + Send>>>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ImportedProxy {
    pub id: String,
    pub name: String,
    pub protocol: String,
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProxyLaunch {
    pub server: String,
    pub bypass_rules: Vec<String>,
    pub bridge: Option<ProxyBridgeSpec>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct ProxyImportReport {
    pub imported_count: usize,
    pub skipped_count: usize,
    pub errors: Vec<String>,
    pub proxies: Vec<ImportedProxy>,
}

#[derive(Serialize)]
struct ProxyCredential<'a> {
    username: Option<&'a str>,
    password: Option<&'a str>,
}

impl DesktopProxyStore {
    pub fn native(repository: LocalRepository) -> Self {
        Self::with_vault(repository, Box::new(NativeVault::new()))
    }

    pub fn with_vault(repository: LocalRepository, vault: Box<dyn OsVault + Send>) -> Self {
        Self {
            repository,
            vault: Arc::new(Mutex::new(vault)),
        }
    }

    pub fn import(
        &self,
        rows: Vec<ProxyRow>,
        name_prefix: Option<&str>,
    ) -> Result<ProxyImportReport, Problem> {
        let prefix = name_prefix
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("Imported");
        let named = rows
            .into_iter()
            .enumerate()
            .map(|(index, row)| (row, format!("{prefix} Proxy {}", index + 1)))
            .collect();
        self.import_named(named)
    }

    pub fn import_named(
        &self,
        rows: Vec<(ProxyRow, String)>,
    ) -> Result<ProxyImportReport, Problem> {
        let mut existing = self
            .repository
            .list::<Proxy>()?
            .into_iter()
            .map(|proxy| (endpoint_key(&proxy.value), proxy.value))
            .collect::<BTreeMap<_, _>>();
        let mut report = ProxyImportReport::default();

        for (row, name) in rows {
            let key = row_endpoint_key(&row);
            if existing.contains_key(&key) {
                report.skipped_count += 1;
                continue;
            }
            match self.create(row, name) {
                Ok((proxy, imported)) => {
                    existing.insert(key, proxy);
                    report.imported_count += 1;
                    report.proxies.push(imported);
                }
                Err(error) => report.errors.push(error.code),
            }
        }
        Ok(report)
    }

    pub fn list(&self) -> Result<Vec<ImportedProxy>, Problem> {
        self.repository
            .list::<Proxy>()?
            .into_iter()
            .map(|proxy| {
                let value = proxy.value;
                Ok(ImportedProxy {
                    id: value.id.as_str().to_owned(),
                    name: value.name.clone().unwrap_or_else(|| fallback_name(&value)),
                    protocol: protocol_name(&value.protocol).to_owned(),
                    host: value.host,
                    port: value.port,
                    username: None,
                })
            })
            .collect()
    }

    pub fn launch(&self, id: &ProxyId) -> Result<ProxyLaunch, Problem> {
        let proxy = self
            .repository
            .get::<Proxy>(id.as_str())?
            .ok_or_else(|| Problem::new("proxy_not_found", "Proxy not found", 404, "desktop"))?
            .value;
        let credential = proxy
            .credential_ref
            .as_ref()
            .map(|reference| {
                let key = credential_key(reference)?;
                self.vault
                    .lock()
                    .map_err(|_| vault_unavailable())?
                    .read(&key)
                    .map(|(value, _)| value)
                    .map_err(secret_problem)
            })
            .transpose()?;
        let host = if proxy.host.contains(':') && !proxy.host.starts_with('[') {
            format!("[{}]", proxy.host)
        } else {
            proxy.host.clone()
        };
        let bridge = credential.map(|credential| ProxyBridgeSpec {
            protocol: proxy.protocol.clone(),
            host: proxy.host.clone(),
            port: proxy.port,
            credential: Some(credential),
        });
        Ok(ProxyLaunch {
            server: format!(
                "{}://{}:{}",
                protocol_name(&proxy.protocol),
                host,
                proxy.port
            ),
            bypass_rules: proxy.bypass_rules,
            bridge,
        })
    }

    fn create(&self, row: ProxyRow, name: String) -> Result<(Proxy, ImportedProxy), Problem> {
        let id = proxy_id(&row)?;
        let credential_ref = if row.username.is_some() || row.password.is_some() {
            Some(
                CredentialId::new(format!("credential-{}", id.as_str())).map_err(|_| {
                    Problem::new(
                        "invalid_proxy_credential_id",
                        "Invalid proxy credential id",
                        500,
                        "desktop",
                    )
                })?,
            )
        } else {
            None
        };
        let credential_version = if let Some(reference) = credential_ref.as_ref() {
            let bytes = serde_json::to_vec(&ProxyCredential {
                username: row.username.as_deref(),
                password: row.password.as_deref(),
            })
            .map_err(|_| {
                Problem::new(
                    "proxy_credential_invalid",
                    "Proxy credential is invalid",
                    500,
                    "desktop",
                )
            })?;
            let key = credential_key(reference)?;
            let value = SecretValue::new(bytes).map_err(secret_problem)?;
            Some(
                self.vault
                    .lock()
                    .map_err(|_| vault_unavailable())?
                    .create(&key, value)
                    .map_err(secret_problem)?,
            )
        } else {
            None
        };
        let timestamp = now();
        let proxy = Proxy {
            id: id.clone(),
            name: Some(name.clone()),
            protocol: domain_protocol(&row.protocol),
            host: row.host.clone(),
            port: row.port,
            credential_ref: credential_ref.clone(),
            rotation_url: None,
            bypass_rules: Vec::new(),
            created_at: timestamp.clone(),
            updated_at: timestamp,
        };
        if let Err(error) = self.repository.create(proxy.clone()) {
            if let (Some(reference), Some(version)) = (credential_ref.as_ref(), credential_version)
            {
                let key = credential_key(reference)?;
                self.vault
                    .lock()
                    .map_err(|_| vault_unavailable())?
                    .remove(&key, version)
                    .map_err(secret_problem)?;
            }
            return Err(error);
        }
        let imported = ImportedProxy {
            id: id.as_str().to_owned(),
            name,
            protocol: protocol_name(&proxy.protocol).to_owned(),
            host: proxy.host.clone(),
            port: proxy.port,
            username: row.username,
        };
        Ok((proxy, imported))
    }
}

fn proxy_id(row: &ProxyRow) -> Result<ProxyId, Problem> {
    let digest = format!("{:x}", Sha256::digest(row_endpoint_key(row).as_bytes()));
    ProxyId::new(format!("prx_{}", &digest[..24]))
        .map_err(|_| Problem::new("invalid_proxy_id", "Invalid proxy id", 500, "desktop"))
}

fn credential_key(reference: &CredentialId) -> Result<SecretKey, Problem> {
    SecretKey::new("proxy-credentials", reference.as_str()).map_err(secret_problem)
}

fn endpoint_key(proxy: &Proxy) -> String {
    format!(
        "{}|{}|{}",
        protocol_name(&proxy.protocol),
        proxy.host.trim().to_ascii_lowercase(),
        proxy.port
    )
}

fn row_endpoint_key(row: &ProxyRow) -> String {
    format!(
        "{}|{}|{}",
        parsed_protocol_name(&row.protocol),
        row.host.trim().to_ascii_lowercase(),
        row.port
    )
}

fn fallback_name(proxy: &Proxy) -> String {
    format!(
        "{}://{}:{}",
        protocol_name(&proxy.protocol),
        proxy.host,
        proxy.port
    )
}

fn domain_protocol(protocol: &ParsedProtocol) -> ProxyProtocol {
    match protocol {
        ParsedProtocol::Http => ProxyProtocol::Http,
        ParsedProtocol::Https => ProxyProtocol::Https,
        ParsedProtocol::Socks4 => ProxyProtocol::Socks4,
        ParsedProtocol::Socks5 => ProxyProtocol::Socks5,
    }
}

fn protocol_name(protocol: &ProxyProtocol) -> &'static str {
    match protocol {
        ProxyProtocol::Http => "http",
        ProxyProtocol::Https => "https",
        ProxyProtocol::Socks4 => "socks4",
        ProxyProtocol::Socks5 => "socks5",
    }
}

fn parsed_protocol_name(protocol: &ParsedProtocol) -> &'static str {
    match protocol {
        ParsedProtocol::Http => "http",
        ParsedProtocol::Https => "https",
        ParsedProtocol::Socks4 => "socks4",
        ParsedProtocol::Socks5 => "socks5",
    }
}

fn now() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string()
}

fn secret_problem(error: buglogin_storage::secrets::SecretError) -> Problem {
    Problem::new(
        error.code(),
        "Proxy credential storage failed",
        503,
        "desktop",
    )
}

fn vault_unavailable() -> Problem {
    Problem::new(
        "secret_vault_unavailable",
        "Proxy credential storage unavailable",
        503,
        "desktop",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use buglogin_storage::secrets::{
        RotationReceipt, SecretError, SecretVersion, UnavailableVault,
    };
    use std::collections::HashMap;
    use std::fs;

    #[derive(Clone, Default)]
    struct TestVault {
        values: Arc<Mutex<HashMap<SecretKey, (SecretValue, SecretVersion)>>>,
    }

    impl OsVault for TestVault {
        fn read(&self, key: &SecretKey) -> Result<(SecretValue, SecretVersion), SecretError> {
            self.values
                .lock()
                .unwrap()
                .get(key)
                .cloned()
                .ok_or(SecretError::NotFound)
        }
        fn create(
            &mut self,
            key: &SecretKey,
            value: SecretValue,
        ) -> Result<SecretVersion, SecretError> {
            let mut values = self.values.lock().unwrap();
            if values.contains_key(key) {
                return Err(SecretError::VersionConflict);
            }
            let version = SecretVersion::new(1).unwrap();
            values.insert(key.clone(), (value, version));
            Ok(version)
        }
        fn rotate(
            &mut self,
            _key: &SecretKey,
            _expected: SecretVersion,
            _replacement: SecretValue,
        ) -> Result<RotationReceipt, SecretError> {
            Err(SecretError::OperationFailed)
        }
        fn remove(&mut self, key: &SecretKey, expected: SecretVersion) -> Result<(), SecretError> {
            let mut values = self.values.lock().unwrap();
            if values.get(key).map(|(_, version)| *version) != Some(expected) {
                return Err(SecretError::VersionConflict);
            }
            values.remove(key);
            Ok(())
        }
    }

    fn row(host: &str, username: Option<&str>, password: Option<&str>) -> ProxyRow {
        ProxyRow {
            protocol: ParsedProtocol::Http,
            host: host.to_owned(),
            port: 8080,
            username: username.map(str::to_owned),
            password: password.map(str::to_owned),
        }
    }

    fn repository(name: &str) -> (LocalRepository, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "buglogin-proxy-store-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        (LocalRepository::open(root.clone()).unwrap(), root)
    }

    #[test]
    fn import_deduplicates_endpoint_and_redacts_credentials() {
        let (repository, root) = repository("dedupe");
        let vault = TestVault::default();
        let store = DesktopProxyStore::with_vault(repository.clone(), Box::new(vault.clone()));
        let report = store
            .import(
                vec![
                    row("Proxy.EXAMPLE", Some("alice"), Some("secret")),
                    row("proxy.example", None, None),
                ],
                None,
            )
            .unwrap();
        assert_eq!(report.imported_count, 1);
        assert_eq!(report.skipped_count, 1);
        assert_eq!(report.proxies[0].username.as_deref(), Some("alice"));
        assert!(!serde_json::to_string(&report).unwrap().contains("secret"));
        assert_eq!(repository.list::<Proxy>().unwrap().len(), 1);
        assert_eq!(vault.values.lock().unwrap().len(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn launch_resolves_credential_for_the_loopback_bridge() {
        let (repository, root) = repository("launch");
        let vault = TestVault::default();
        let store = DesktopProxyStore::with_vault(repository, Box::new(vault));
        let direct = store
            .import(vec![row("proxy.example", None, None)], None)
            .unwrap()
            .proxies
            .remove(0);
        let direct_id = ProxyId::new(direct.id).unwrap();
        assert_eq!(
            store.launch(&direct_id).unwrap().server,
            "http://proxy.example:8080"
        );

        let authenticated = store
            .import(
                vec![row("auth.example", Some("alice"), Some("secret"))],
                None,
            )
            .unwrap()
            .proxies
            .remove(0);
        let authenticated_id = ProxyId::new(authenticated.id).unwrap();
        let launch = store.launch(&authenticated_id).unwrap();
        assert!(launch.bridge.is_some());
        let debug = format!("{launch:?}");
        assert!(!debug.contains("alice"));
        assert!(!debug.contains("secret"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn vault_failure_prevents_plaintext_repository_write() {
        let (repository, root) = repository("vault-failure");
        let store = DesktopProxyStore::with_vault(repository.clone(), Box::new(UnavailableVault));
        let report = store
            .import(
                vec![row("proxy.example", Some("alice"), Some("secret"))],
                None,
            )
            .unwrap();
        assert_eq!(report.imported_count, 0);
        assert_eq!(report.errors, vec!["secret_vault_unavailable"]);
        assert!(repository.list::<Proxy>().unwrap().is_empty());
        fs::remove_dir_all(root).unwrap();
    }
}
