use std::path::{Path, PathBuf};

use axum::{Json, Router, routing::get};
use buglogin_api::http::ApiState;
use buglogin_browser::profile::ProfileService;
use buglogin_domain::errors::Problem;
use buglogin_storage::local::LocalRepository;
use serde::Serialize;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Health {
    pub status: &'static str,
    pub service: &'static str,
    pub version: &'static str,
}

pub fn router() -> Router {
    router_with_data_dir(default_data_dir()).expect("BugLogin data directory must be available")
}

pub fn router_with_data_dir(data_dir: impl AsRef<Path>) -> Result<Router, Problem> {
    let repository = LocalRepository::open(data_dir.as_ref().join("local"))?;
    let state =
        ApiState::new(ProfileService::new(repository)).with_recovery_delivery_from_env(false)?;
    let router = Router::new()
        .route("/health", get(health))
        .merge(buglogin_api::http::router(state.clone()));
    with_optional_mcp(router, state)
}

fn with_optional_mcp(router: Router, state: ApiState) -> Result<Router, Problem> {
    let enabled = std::env::var("BUGLOGIN_MCP_ENABLED")
        .ok()
        .is_some_and(|value| value.eq_ignore_ascii_case("true"));
    if !enabled {
        return Ok(router);
    }
    let bearer = std::env::var("BUGLOGIN_MCP_TOKEN").map_err(|_| {
        Problem::new(
            "missing_mcp_token",
            "MCP transport token is missing",
            500,
            "control-startup",
        )
    })?;
    let control = buglogin_api::mcp::transport::McpTransportControl::default();
    control.activate(&bearer)?;
    Ok(
        router.merge(buglogin_api::mcp::transport::production_router(
            state,
            buglogin_api::mcp::transport::McpTransportConfig {
                enabled: true,
                ..Default::default()
            },
            control,
        )),
    )
}

pub fn router_with_startup_config(
    data_dir: impl AsRef<Path>,
    environment: Option<&str>,
    _postgres_url: Option<String>,
) -> Result<Router, Problem> {
    match environment {
        None | Some("local") => router_with_data_dir(data_dir),
        Some("production") => Err(Problem::new(
            "production_api_entrypoint_unsupported",
            "Hosted production must run the buglogin-api entrypoint that requires PostgreSQL, S3, Redis, and an explicit ingress bind",
            409,
            "control-startup",
        )),
        Some(_) => Err(Problem::new(
            "invalid_environment",
            "BugLogin environment is invalid",
            500,
            "control-startup",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::router_with_startup_config;

    #[test]
    fn secondary_control_entrypoint_cannot_run_hosted_production() {
        let data = tempfile::tempdir().unwrap();
        assert_eq!(
            router_with_startup_config(
                data.path(),
                Some("production"),
                Some("postgres://db/app?sslmode=verify-full".into()),
            )
            .err()
            .unwrap()
            .code,
            "production_api_entrypoint_unsupported"
        );
    }

    #[test]
    fn unknown_environment_never_falls_back_to_memory() {
        let data = tempfile::tempdir().unwrap();
        assert_eq!(
            router_with_startup_config(data.path(), Some("prod"), None)
                .err()
                .unwrap()
                .code,
            "invalid_environment"
        );
    }
}

pub fn default_data_dir() -> PathBuf {
    if let Some(path) = std::env::var_os("BUGLOGIN_DATA_DIR") {
        return PathBuf::from(path);
    }
    #[cfg(target_os = "windows")]
    if let Some(path) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(path).join("BugLogin");
    }
    #[cfg(target_os = "macos")]
    if let Some(path) = std::env::var_os("HOME") {
        return PathBuf::from(path).join("Library/Application Support/BugLogin");
    }
    std::env::temp_dir().join("buglogin-data")
}

async fn health() -> Json<Health> {
    Json(Health {
        status: "ok",
        service: "buglogin-control",
        version: env!("CARGO_PKG_VERSION"),
    })
}
