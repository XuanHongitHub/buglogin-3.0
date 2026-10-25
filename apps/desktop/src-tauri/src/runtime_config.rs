use std::{collections::BTreeMap, env, fmt, path::PathBuf};

use buglogin_browser::runtime::{
    RuntimeBinary,
    catalog::{Ed25519RuntimeSignatureVerifier, RuntimeCatalog},
};
use buglogin_domain::errors::Problem;
use serde::de::{MapAccess, Visitor};

use crate::adapter::{RuntimeTarget, RuntimeTargets};
use crate::app_instance::AppInstance;
use crate::device_identity::DeviceIdentityVault;

const PRODUCTION_API_BASE_URL: &str = "https://api3.buglogin.com";
const EMBEDDED_RUNTIME_TRUST_KEYS_JSON: &str = include_str!("../runtime-trust-keys.json");
const EMBEDDED_RUNTIME_CATALOG_JSON: &str = include_str!("../runtime-catalog.json");

pub(crate) fn pinned_runtime_verifier(
    correlation_id: &str,
) -> Result<Ed25519RuntimeSignatureVerifier, Problem> {
    let trusted_keys = if let Some(encoded_keys) =
        option_env!("BUGLOGIN_RUNTIME_TRUST_KEYS_JSON").filter(|value| !value.trim().is_empty())
    {
        let RuntimeTrustKeyMap(encoded_keys) = serde_json::from_str(encoded_keys)
            .map_err(|_| runtime_trust_problem(correlation_id, true))?;
        if encoded_keys.is_empty() {
            return Err(runtime_trust_problem(correlation_id, true));
        }
        encoded_keys
            .into_iter()
            .map(|(key_id, encoded_key)| {
                let public_key = decode_runtime_public_key(&encoded_key)
                    .ok_or_else(|| runtime_trust_problem(correlation_id, true))?;
                Ok((key_id, public_key))
            })
            .collect::<Result<BTreeMap<_, _>, Problem>>()?
    } else {
        let legacy_key_id =
            option_env!("BUGLOGIN_RUNTIME_TRUST_KEY_ID").filter(|value| !value.trim().is_empty());
        let legacy_encoded_key =
            option_env!("BUGLOGIN_RUNTIME_PUBLIC_KEY_HEX").filter(|value| !value.trim().is_empty());
        match (legacy_key_id, legacy_encoded_key) {
            (Some(key_id), Some(encoded_key)) => {
                let public_key = decode_runtime_public_key(encoded_key)
                    .ok_or_else(|| runtime_trust_problem(correlation_id, true))?;
                BTreeMap::from([(key_id.to_owned(), public_key)])
            }
            (None, None) => {
                let RuntimeTrustKeyMap(encoded_keys) =
                    serde_json::from_str(EMBEDDED_RUNTIME_TRUST_KEYS_JSON)
                        .map_err(|_| runtime_trust_problem(correlation_id, true))?;
                if encoded_keys.is_empty() {
                    return Err(runtime_trust_problem(correlation_id, true));
                }
                encoded_keys
                    .into_iter()
                    .map(|(key_id, encoded_key)| {
                        let public_key = decode_runtime_public_key(&encoded_key)
                            .ok_or_else(|| runtime_trust_problem(correlation_id, true))?;
                        Ok((key_id, public_key))
                    })
                    .collect::<Result<BTreeMap<_, _>, Problem>>()?
            }
            _ => return Err(runtime_trust_problem(correlation_id, true)),
        }
    };
    Ed25519RuntimeSignatureVerifier::new(trusted_keys)
        .map_err(|_| runtime_trust_problem(correlation_id, true))
}

pub(crate) fn fallback_runtime_catalog(
    verifier: &Ed25519RuntimeSignatureVerifier,
    correlation_id: &str,
) -> Result<RuntimeCatalog, Problem> {
    if let Some(catalog_path) = env::var_os("BUGLOGIN_RUNTIME_CATALOG_PATH") {
        if let Ok(bytes) = std::fs::read(&catalog_path) {
            if let Ok(catalog) = serde_json::from_slice::<RuntimeCatalog>(&bytes) {
                if catalog.verify(verifier).is_ok() {
                    return Ok(catalog);
                }
            }
        }
    }
    let catalog: RuntimeCatalog =
        serde_json::from_str(EMBEDDED_RUNTIME_CATALOG_JSON).map_err(|_| {
            Problem::new(
                "runtime_catalog_invalid",
                "Embedded runtime catalog is invalid",
                500,
                correlation_id,
            )
        })?;
    catalog
        .verify(verifier)
        .map_err(|error| Problem::new(error.code, error.title, error.status, correlation_id))?;
    Ok(catalog)
}

fn runtime_trust_problem(correlation_id: &str, invalid: bool) -> Problem {
    let (code, title) = if invalid {
        (
            "runtime_trust_key_invalid",
            "This BugLogin build has an invalid pinned runtime signing key",
        )
    } else {
        (
            "runtime_trust_key_unavailable",
            "This BugLogin build has no pinned runtime signing key",
        )
    };
    Problem::new(code, title, 503, correlation_id)
}

fn decode_runtime_public_key(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let mut key = [0_u8; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        key[index] = (runtime_hex_digit(pair[0])? << 4) | runtime_hex_digit(pair[1])?;
    }
    Some(key)
}

fn runtime_hex_digit(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

struct RuntimeTrustKeyMap(BTreeMap<String, String>);

impl<'de> serde::Deserialize<'de> for RuntimeTrustKeyMap {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct RuntimeTrustKeyMapVisitor;

        impl<'de> Visitor<'de> for RuntimeTrustKeyMapVisitor {
            type Value = RuntimeTrustKeyMap;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a map of runtime key IDs to public-key hex")
            }

            fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut keys = BTreeMap::new();
                while let Some((key_id, public_key)) = map.next_entry::<String, String>()? {
                    if keys.insert(key_id, public_key).is_some() {
                        return Err(serde::de::Error::custom("duplicate runtime trust key ID"));
                    }
                }
                Ok(RuntimeTrustKeyMap(keys))
            }
        }

        deserializer.deserialize_map(RuntimeTrustKeyMapVisitor)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeploymentMode {
    Local,
    Production,
}

#[derive(Clone)]
pub struct DesktopBootstrap {
    pub mode: DeploymentMode,
    pub repository_root: Option<PathBuf>,
    pub profiles_root: PathBuf,
    pub api_base_url: Option<String>,
    pub api_token: Option<String>,
    pub device_id: Option<String>,
    pub runtime_targets: RuntimeTargets,
}

pub fn open() -> Result<DesktopBootstrap, Problem> {
    let app_instance = AppInstance::from_environment()?;
    let mode = if cfg!(debug_assertions) {
        match env::var("BUGLOGIN_DEPLOYMENT_MODE") {
            Ok(value) => DeploymentMode::parse(&value)?,
            Err(_) if env::var_os("BUGLOGIN_API_BASE_URL").is_some() => DeploymentMode::Production,
            Err(_) => DeploymentMode::Local,
        }
    } else {
        if let Ok(value) = env::var("BUGLOGIN_DEPLOYMENT_MODE")
            && DeploymentMode::parse(&value)? != DeploymentMode::Production
        {
            return Err(problem(
                "production_local_mode_forbidden",
                "Packaged desktop builds must use hosted production mode",
                503,
            ));
        }
        DeploymentMode::Production
    };
    let (repository_root, profiles_root, api_base_url, api_token, device_id) = match mode {
        DeploymentMode::Local => {
            let data_root = env::var_os("BUGLOGIN_DATA_ROOT")
                .map(PathBuf::from)
                .or_else(|| dirs::data_local_dir().map(|path| path.join("BugLogin-Local")))
                .ok_or_else(|| problem("data_root_unavailable", "Data root unavailable", 503))?;
            let data_root = app_instance.namespace_path(data_root);
            (
                Some(data_root.join("repository")),
                data_root.join("profiles"),
                None,
                None,
                None,
            )
        }
        DeploymentMode::Production => {
            if env::var_os("BUGLOGIN_DATA_ROOT").is_some() {
                return Err(problem(
                    "production_local_data_forbidden",
                    "Production local data authority is forbidden",
                    503,
                ));
            }
            let api_base_url = env::var("BUGLOGIN_API_BASE_URL")
                .unwrap_or_else(|_| PRODUCTION_API_BASE_URL.to_owned());
            validate_production_api_url(&api_base_url)?;
            let local_development_api = is_local_development_api_url(&api_base_url);
            let api_token = optional_api_token(env::var("BUGLOGIN_API_TOKEN").ok());
            let cache_root = env::var_os("BUGLOGIN_PROFILE_CACHE_ROOT")
                .map(PathBuf::from)
                .or_else(|| {
                    let app_cache_name = if local_development_api {
                        "BugLogin-LocalApi"
                    } else {
                        "BugLogin"
                    };
                    dirs::cache_dir().map(|path| path.join(app_cache_name).join("profiles"))
                })
                .ok_or_else(|| {
                    problem(
                        "profile_cache_root_unavailable",
                        "Profile cache root unavailable",
                        503,
                    )
                })?;
            let cache_root = app_instance.namespace_path(cache_root);
            let identity_vault = DeviceIdentityVault::native();
            let identity_namespace =
                app_instance.device_identity_namespace(if local_development_api {
                    "buglogin-device-identity-local-api"
                } else {
                    "buglogin-device-identity"
                });
            let device_id = identity_vault
                .load_or_create_in_namespace(&identity_namespace, "desktop-bootstrap")?;
            (
                None,
                cache_root,
                Some(api_base_url),
                api_token,
                Some(device_id),
            )
        }
    };
    let allow_runtime_environment_overrides = runtime_environment_overrides_allowed(
        mode,
        api_base_url.as_deref(),
        cfg!(debug_assertions),
    );
    let mut runtime_targets = RuntimeTargets::default();
    if let Some(target) = runtime_target("WAYFERN", allow_runtime_environment_overrides)? {
        runtime_targets.insert(buglogin_domain::entities::Runtime::Wayfern, target);
    }
    if mode == DeploymentMode::Production {
        let runtime_root = runtime_install_root()?;
        if crate::runtime_install::has_install_records(&runtime_root)? {
            let verifier = pinned_runtime_verifier("desktop-runtime-bootstrap")?;
            let installed =
                crate::runtime_install::load_installed_targets(&runtime_root, &verifier)?;
            for target in installed.wayfern.into_values() {
                if !runtime_targets.wayfern.contains_key(&target.binary.version) {
                    runtime_targets.insert(buglogin_domain::entities::Runtime::Wayfern, target);
                }
            }
        }
    }
    Ok(DesktopBootstrap {
        mode,
        repository_root,
        profiles_root,
        api_base_url,
        api_token,
        device_id,
        runtime_targets,
    })
}

pub(crate) fn runtime_install_root() -> Result<PathBuf, Problem> {
    dirs::data_local_dir()
        .map(|path| path.join("BugLogin").join("runtimes"))
        .ok_or_else(|| {
            problem(
                "runtime_install_path_unavailable",
                "BugLogin runtime installation directory is unavailable",
                503,
            )
        })
}

pub fn migration_executable() -> Option<PathBuf> {
    if let Some(path) = env::var_os("BUGLOGIN_MIGRATION_BINARY").map(PathBuf::from) {
        return Some(path);
    }
    let target = option_env!("BUGLOGIN_TARGET_TRIPLE").filter(|value| !value.is_empty())?;
    let suffix = if cfg!(target_os = "windows") {
        ".exe"
    } else {
        ""
    };
    let name = format!("buglogin-migrate-{target}{suffix}");
    let executable = std::env::current_exe().ok()?;
    let parent = executable.parent()?;
    [
        parent.join(&name),
        parent.join("resources").join(&name),
        parent.join("../Resources").join(&name),
    ]
    .into_iter()
    .find(|candidate| candidate.is_file())
}

fn runtime_environment_overrides_allowed(
    mode: DeploymentMode,
    api_base_url: Option<&str>,
    debug_build: bool,
) -> bool {
    debug_build
        && (mode == DeploymentMode::Local || api_base_url.is_some_and(is_local_development_api_url))
}

fn runtime_target(
    name: &str,
    allow_environment_override: bool,
) -> Result<Option<RuntimeTarget>, Problem> {
    let binary_name = format!("BUGLOGIN_{name}_BINARY");
    let version_name = format!("BUGLOGIN_{name}_VERSION");
    let checksum_name = format!("BUGLOGIN_{name}_SHA256");
    let args_name = format!("BUGLOGIN_{name}_ARGS");
    let binary = env::var_os(&binary_name);
    let version = env::var(&version_name).ok();
    let checksum = env::var(&checksum_name).ok();
    if binary.is_none() && version.is_none() && checksum.is_none() {
        return Ok(None);
    }
    if !allow_environment_override {
        return Err(problem(
            "runtime_environment_override_forbidden",
            "Runtime environment overrides are allowed only in local development builds",
            503,
        ));
    }
    let binary = binary
        .ok_or_else(|| problem("runtime_not_configured", "Runtime binary is required", 503))?;
    let version = version
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| problem("runtime_not_configured", "Runtime version is required", 503))?;
    let expected_checksum = checksum
        .filter(|value| valid_checksum(value))
        .ok_or_else(|| {
            problem(
                "runtime_checksum_invalid",
                "Runtime checksum is invalid",
                503,
            )
        })?;
    let extra_args = env::var(args_name)
        .map(|value| value.split_whitespace().map(str::to_owned).collect())
        .unwrap_or_default();
    Ok(Some(RuntimeTarget {
        binary: RuntimeBinary {
            path: PathBuf::from(binary),
            version,
            expected_checksum,
        },
        extra_args,
    }))
}

impl DeploymentMode {
    fn parse(value: &str) -> Result<Self, Problem> {
        match value.trim().to_ascii_lowercase().as_str() {
            "local" => Ok(Self::Local),
            "production" => Ok(Self::Production),
            _ => Err(problem(
                "deployment_mode_invalid",
                "Desktop deployment mode is invalid",
                503,
            )),
        }
    }
}

fn validate_production_api_url(value: &str) -> Result<(), Problem> {
    let url = reqwest::Url::parse(value.trim()).map_err(|_| {
        problem(
            "production_api_invalid",
            "Production API endpoint must use remote HTTPS",
            503,
        )
    })?;
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    let local_development_api = is_local_development_api(&url);
    if local_development_api
        && matches!(url.path(), "/" | "")
        && url.query().is_none()
        && url.fragment().is_none()
        && !url_authority_has_userinfo(value.trim())
        && url.username().is_empty()
        && url.password().is_none()
    {
        return Ok(());
    }
    if url.scheme() != "https"
        || !is_production_api_host(&host)
        || url.port().is_some()
        || !matches!(url.path(), "/" | "")
        || url.query().is_some()
        || url.fragment().is_some()
        || url_authority_has_userinfo(value.trim())
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(problem(
            "production_api_invalid",
            "Production API endpoint must use remote HTTPS",
            503,
        ));
    }
    Ok(())
}

fn is_local_development_api_url(value: &str) -> bool {
    reqwest::Url::parse(value.trim()).is_ok_and(|url| is_local_development_api(&url))
}

fn is_local_development_api(url: &reqwest::Url) -> bool {
    cfg!(debug_assertions)
        && url.scheme() == "http"
        && matches!(
            url.host_str().map(str::to_ascii_lowercase).as_deref(),
            Some("localhost" | "127.0.0.1" | "::1")
        )
        && url.port().is_some()
}

pub(crate) fn is_production_api_host(host: &str) -> bool {
    host == "api3.buglogin.com"
}

pub(crate) fn is_production_device_verification_url(
    api_host: &str,
    verification: &reqwest::Url,
) -> bool {
    is_production_api_host(api_host)
        && verification.scheme() == "https"
        && matches!(verification.host_str(), Some("app3.buglogin.com"))
        && verification.port().is_none()
        && verification.path() == "/device"
        && verification.query().is_none()
}

pub(crate) fn url_authority_has_userinfo(value: &str) -> bool {
    value
        .split_once("://")
        .and_then(|(_, remainder)| remainder.split(['/', '?', '#']).next())
        .is_some_and(|authority| authority.contains('@'))
}

fn problem(code: &'static str, title: &'static str, status: u16) -> Problem {
    Problem::new(code, title, status, "desktop-bootstrap")
}

fn valid_checksum(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn optional_api_token(value: Option<String>) -> Option<String> {
    value.filter(|token| !token.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::{
        DeploymentMode, fallback_runtime_catalog, is_local_development_api_url,
        is_production_device_verification_url, optional_api_token, pinned_runtime_verifier,
        runtime_environment_overrides_allowed, valid_checksum, validate_production_api_url,
    };

    #[test]
    fn checksum_requires_sha256_hex() {
        assert!(valid_checksum(&"a".repeat(64)));
        assert!(!valid_checksum("short"));
        assert!(!valid_checksum(&"g".repeat(64)));
    }

    #[test]
    fn embedded_runtime_trust_keys_build_a_verifier() {
        let verifier = pinned_runtime_verifier("runtime-trust-key-test").unwrap();
        let catalog = fallback_runtime_catalog(&verifier, "runtime-trust-key-test").unwrap();

        assert!(!catalog.releases.is_empty());
        assert!(catalog.verify(&verifier).is_ok());
        assert!(catalog.releases.iter().any(|release| {
            release.runtime == buglogin_domain::entities::Runtime::Wayfern
                && release.platform == buglogin_domain::entities::RuntimePlatform::Macos
                && release.architecture == buglogin_domain::entities::RuntimeArchitecture::Arm64
                && release.package_format
                    == buglogin_browser::runtime::catalog::RuntimePackageFormat::Dmg
        }));
    }

    #[test]
    fn deployment_mode_never_defaults_or_accepts_unknown_values() {
        assert_eq!(
            DeploymentMode::parse("local").unwrap(),
            DeploymentMode::Local
        );
        assert_eq!(
            DeploymentMode::parse("production").unwrap(),
            DeploymentMode::Production
        );
        assert_eq!(
            DeploymentMode::parse("dev").unwrap_err().code,
            "deployment_mode_invalid"
        );
    }

    #[test]
    fn production_api_requires_remote_https() {
        assert!(validate_production_api_url("https://api3.buglogin.com").is_ok());
        if cfg!(debug_assertions) {
            assert!(validate_production_api_url("http://127.0.0.1:12343").is_ok());
            assert!(validate_production_api_url("http://localhost:12343").is_ok());
        }
        assert_eq!(
            validate_production_api_url("http://api3.buglogin.com")
                .unwrap_err()
                .code,
            "production_api_invalid"
        );
        assert_eq!(
            validate_production_api_url("https://localhost:12343")
                .unwrap_err()
                .code,
            "production_api_invalid"
        );
        for endpoint in [
            "https://api.buglogin.com",
            "https://api2.buglogin.com",
            "https://api3.buglogin.com.evil.example",
            "https://other.api3.buglogin.com",
            "https://api3.buglogin.com:8443",
            "https://api3.buglogin.com/v1",
            "https://api3.buglogin.com/?tenant=other",
            "https://127.0.0.2",
            "https://0.0.0.0",
            "https://[::1]",
            "https://api.localhost",
            "https://user:password@api3.buglogin.com",
        ] {
            assert_eq!(
                validate_production_api_url(endpoint).unwrap_err().code,
                "production_api_invalid"
            );
        }
    }

    #[test]
    fn loopback_api_is_marked_for_development_isolation_only() {
        if cfg!(debug_assertions) {
            assert!(is_local_development_api_url("http://127.0.0.1:12344"));
            assert!(is_local_development_api_url("http://localhost:12344"));
        } else {
            assert!(!is_local_development_api_url("http://127.0.0.1:12344"));
        }
        assert!(!is_local_development_api_url("https://api3.buglogin.com"));
    }

    #[test]
    fn unsigned_runtime_environment_overrides_are_limited_to_debug_and_local_api() {
        assert!(runtime_environment_overrides_allowed(
            DeploymentMode::Local,
            None,
            true
        ));
        assert!(runtime_environment_overrides_allowed(
            DeploymentMode::Production,
            Some("http://127.0.0.1:12344"),
            true
        ));
        assert!(!runtime_environment_overrides_allowed(
            DeploymentMode::Production,
            Some("https://api3.buglogin.com"),
            true
        ));
        assert!(!runtime_environment_overrides_allowed(
            DeploymentMode::Local,
            None,
            false
        ));
        assert!(!runtime_environment_overrides_allowed(
            DeploymentMode::Production,
            Some("http://127.0.0.1:12344"),
            false
        ));
    }

    #[test]
    fn production_api_token_is_optional_for_device_bootstrap() {
        assert_eq!(optional_api_token(None), None);
        assert_eq!(optional_api_token(Some(" ".to_owned())), None);
        assert_eq!(
            optional_api_token(Some("session-token".to_owned())),
            Some("session-token".to_owned())
        );
    }

    #[test]
    fn device_verification_url_is_pinned_to_buglogin_hosts() {
        assert!(is_production_device_verification_url(
            "api3.buglogin.com",
            &reqwest::Url::parse("https://app3.buglogin.com/device").unwrap()
        ));
        for verification_url in [
            "https://evil.example/device",
            "https://app3.buglogin.com.evil.example/device",
            "http://app3.buglogin.com/device",
            "https://app3.buglogin.com/other",
            "https://app3.buglogin.com/device?next=evil",
            "https://app3.buglogin.com:8443/device",
        ] {
            assert!(!is_production_device_verification_url(
                "api3.buglogin.com",
                &reqwest::Url::parse(verification_url).unwrap()
            ));
        }
        assert!(!is_production_device_verification_url(
            "evil.example",
            &reqwest::Url::parse("https://app3.buglogin.com/device").unwrap()
        ));
        assert!(!is_production_device_verification_url(
            "api.buglogin.com",
            &reqwest::Url::parse("https://app3.buglogin.com/device").unwrap()
        ));
    }
}
