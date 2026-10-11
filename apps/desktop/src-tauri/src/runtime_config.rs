use std::{env, path::PathBuf};

use buglogin_browser::runtime::RuntimeBinary;
use buglogin_domain::errors::Problem;

use crate::adapter::{RuntimeTarget, RuntimeTargets};

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
    let mode = DeploymentMode::parse(&env::var("BUGLOGIN_DEPLOYMENT_MODE").map_err(|_| {
        problem(
            "deployment_mode_required",
            "Desktop deployment mode is required",
            503,
        )
    })?)?;
    let (repository_root, profiles_root, api_base_url, api_token, device_id) = match mode {
        DeploymentMode::Local => {
            let data_root = env::var_os("BUGLOGIN_DATA_ROOT")
                .map(PathBuf::from)
                .or_else(|| dirs::data_local_dir().map(|path| path.join("BugLogin-Local")))
                .ok_or_else(|| problem("data_root_unavailable", "Data root unavailable", 503))?;
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
            let api_base_url = env::var("BUGLOGIN_API_BASE_URL").map_err(|_| {
                problem(
                    "production_api_required",
                    "Production API endpoint is required",
                    503,
                )
            })?;
            validate_production_api_url(&api_base_url)?;
            let api_token = optional_api_token(env::var("BUGLOGIN_API_TOKEN").ok());
            let cache_root = env::var_os("BUGLOGIN_PROFILE_CACHE_ROOT")
                .map(PathBuf::from)
                .or_else(|| dirs::cache_dir().map(|path| path.join("BugLogin").join("profiles")))
                .ok_or_else(|| {
                    problem(
                        "profile_cache_root_unavailable",
                        "Profile cache root unavailable",
                        503,
                    )
                })?;
            let device_id = env::var("BUGLOGIN_DEVICE_ID").map_err(|_| {
                problem(
                    "production_device_id_required",
                    "Production device ID is required",
                    503,
                )
            })?;
            if device_id.trim().is_empty() {
                return Err(problem(
                    "production_device_id_required",
                    "Production device ID is required",
                    503,
                ));
            }
            (
                None,
                cache_root,
                Some(api_base_url),
                api_token,
                Some(device_id),
            )
        }
    };
    let runtime_targets = RuntimeTargets {
        wayfern: runtime_target("WAYFERN")?,
        bugmini: runtime_target("BUGMINI")?,
    };
    if runtime_targets.wayfern.is_none() && runtime_targets.bugmini.is_none() {
        return Err(problem(
            "runtime_not_configured",
            "At least one desktop runtime is required",
            503,
        ));
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

fn runtime_target(name: &str) -> Result<Option<RuntimeTarget>, Problem> {
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
    let local_host = host == "localhost" || host.ends_with(".localhost");
    let local_ip = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .parse::<std::net::IpAddr>()
        .is_ok_and(|address| address.is_loopback() || address.is_unspecified());
    if url.scheme() != "https"
        || host.is_empty()
        || local_host
        || local_ip
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
    use super::{DeploymentMode, optional_api_token, valid_checksum, validate_production_api_url};

    #[test]
    fn checksum_requires_sha256_hex() {
        assert!(valid_checksum(&"a".repeat(64)));
        assert!(!valid_checksum("short"));
        assert!(!valid_checksum(&"g".repeat(64)));
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
        assert!(validate_production_api_url("https://api.buglogin.com").is_ok());
        assert_eq!(
            validate_production_api_url("http://api.buglogin.com")
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
            "https://127.0.0.2",
            "https://0.0.0.0",
            "https://[::1]",
            "https://api.localhost",
            "https://user:password@api.buglogin.com",
        ] {
            assert_eq!(
                validate_production_api_url(endpoint).unwrap_err().code,
                "production_api_invalid"
            );
        }
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
}
