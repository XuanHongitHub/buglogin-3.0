use std::{
    collections::BTreeMap,
    env, fs,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{Json, Router, routing::get, serve};
use buglogin_api::http::{ApiState, coordination_authorizer, router};
use buglogin_api::mcp::transport::{McpTransportConfig, production_router};
use buglogin_api::services::deployment_config::validate_production_s3_namespace;
use buglogin_api::services::runtime_catalog::PublishedRuntimeCatalog;
use buglogin_migration::MigrationKey;
use buglogin_migration::{package::NeutralPackage, verify::validate_relative_path};
use buglogin_storage::{
    control::{PostgresConfig, PostgresControlRecordStore, SqliteControlRecordStore},
    remote::{LocalRemoteChunkStore, S3Config, S3RemoteChunkStore},
    secrets::SecretValue,
};
use buglogin_sync::coordination::{LocalCoordination, RedisCoordination, RedisCoordinationConfig};
use tokio::net::TcpListener;
use zeroize::Zeroizing;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let production = production_mode()?;
    if let Some(package_dir) = initial_migration_import_argument()? {
        return run_initial_migration_import(production, package_dir).await;
    }
    if production && env::var_os("BUGLOGIN_DATA_ROOT").is_some() {
        return Err("BUGLOGIN_DATA_ROOT is forbidden in production".into());
    }
    let state = match env::var_os("BUGLOGIN_POSTGRES_URL") {
        Some(_) if production => {
            let config = PostgresConfig::from_env()?;
            tokio::task::spawn_blocking(move || ApiState::production(config)).await??
        }
        Some(database_url) => {
            let database_url = database_url
                .into_string()
                .map_err(|_| "BUGLOGIN_POSTGRES_URL must be valid Unicode")?;
            let config = PostgresConfig::new_local_development(
                database_url,
                env::var_os("BUGLOGIN_POSTGRES_TLS_ROOT_CERT").map(PathBuf::from),
            )?;
            let control_store = tokio::task::spawn_blocking(move || {
                let mut store = PostgresControlRecordStore::connect(&config)?;
                store.ensure_schema()?;
                Ok::<_, buglogin_domain::errors::Problem>(store)
            })
            .await??;
            let data_root = local_data_root(env::var_os("BUGLOGIN_DATA_ROOT"))?;
            let chunks = LocalRemoteChunkStore::new(data_root.join("chunks"))?;
            ApiState::local_hosted_with_postgres_control_store(control_store)?
                .with_concurrent_remote_chunks(Box::new(chunks))
        }
        None if production => {
            return Err("BUGLOGIN_POSTGRES_URL is required in production".into());
        }
        None => {
            let data_root = local_data_root(env::var_os("BUGLOGIN_DATA_ROOT"))?;
            let control_store = SqliteControlRecordStore::open(data_root.join("control.sqlite"))?;
            let chunks = LocalRemoteChunkStore::new(data_root.join("chunks"))?;
            ApiState::local_hosted_with_control_store(control_store)?
                .with_concurrent_remote_chunks(Box::new(chunks))
        }
    };
    let state = with_configured_migration_keys(state)?;
    let state = with_configured_runtime_catalog(state)?;
    let state = match env::var("BUGLOGIN_DEVICE_VERIFICATION_URI") {
        Ok(uri) if !production || uri.starts_with("https://") => {
            state.with_device_verification_uri(uri)?
        }
        Ok(_) => {
            return Err("BUGLOGIN_DEVICE_VERIFICATION_URI must use HTTPS in production".into());
        }
        Err(_) if production => {
            return Err("BUGLOGIN_DEVICE_VERIFICATION_URI is required in production".into());
        }
        Err(_) => state,
    };
    let state = match env::var("BUGLOGIN_WEB_ORIGIN") {
        Ok(origin) if !production || origin.starts_with("https://") => {
            state.with_web_origin(origin)?
        }
        Ok(_) => return Err("BUGLOGIN_WEB_ORIGIN must use HTTPS in production".into()),
        Err(_) if production => return Err("BUGLOGIN_WEB_ORIGIN is required in production".into()),
        Err(_) => state,
    };
    let state = match env::var_os("BUGLOGIN_S3_ENDPOINT") {
        Some(_) => {
            if production {
                let prefix = env::var("BUGLOGIN_S3_PREFIX")
                    .map_err(|_| "BUGLOGIN_S3_PREFIX is required in production")?;
                let tenant_id = env::var("BUGLOGIN_TENANT_ID")
                    .map_err(|_| "BUGLOGIN_TENANT_ID is required in production")?;
                validate_production_s3_namespace(&prefix, &tenant_id)?;
            }
            state.with_concurrent_remote_chunks(Box::new(S3RemoteChunkStore::new(
                S3Config::from_env()?,
            )?))
        }
        None if production => return Err("BUGLOGIN_S3_ENDPOINT is required in production".into()),
        None => state,
    };
    let state = match env::var("BUGLOGIN_REDIS_URL") {
        Ok(url) => {
            validate_redis_url(production, &url)?;
            let namespace = env::var("BUGLOGIN_REDIS_NAMESPACE")
                .unwrap_or_else(|_| "buglogin-production".to_owned());
            let mut redis_config = RedisCoordinationConfig::new(url, namespace);
            if let Ok(certificate_path) = env::var("BUGLOGIN_REDIS_CA_CERT") {
                let certificate_pem = std::fs::read(certificate_path)?;
                redis_config = redis_config.with_tls_root_cert(certificate_pem);
            }
            let backend =
                RedisCoordination::connect(redis_config, coordination_authorizer(&state)).await?;
            state.with_coordination(Arc::new(backend))
        }
        Err(_) if production => return Err("BUGLOGIN_REDIS_URL is required in production".into()),
        Err(_) => state.with_coordination(Arc::new(LocalCoordination::new())),
    };
    let now_ms = unix_time_ms()?;
    let recovery = state.recover_stale_sync_migrations(now_ms.saturating_sub(15 * 60 * 1_000))?;
    if recovery.recovered > 0 || recovery.deferred > 0 || recovery.failed > 0 {
        eprintln!(
            "Migration recovery: scanned={}, recovered={}, deferred={}, failed={}",
            recovery.scanned, recovery.recovered, recovery.deferred, recovery.failed
        );
    }
    let address = bind_address(production, env::var("BUGLOGIN_API_BIND").ok())?;
    let listener = TcpListener::bind(address).await?;
    eprintln!("BugLogin API listening on {address}");
    start_sync_migration_recovery_worker(state.clone());
    let app = router(state.clone());
    let app = with_optional_mcp(app, state)?;
    serve(listener, app).await?;
    Ok(())
}

fn with_optional_mcp(app: Router, state: ApiState) -> Result<Router, Box<dyn std::error::Error>> {
    let enabled = match env::var("BUGLOGIN_MCP_ENABLED") {
        Ok(value) if value.eq_ignore_ascii_case("true") => true,
        Ok(value) if value.eq_ignore_ascii_case("false") => false,
        Ok(_) => return Err("BUGLOGIN_MCP_ENABLED must be 'true' or 'false'".into()),
        Err(env::VarError::NotPresent) => false,
        Err(error) => return Err(error.into()),
    };
    let app = app.route(
        "/v1/mcp/status",
        get(move || async move {
            Json(serde_json::json!({
                "enabled": enabled,
                "endpoint": "/v1/mcp",
                "authentication": "scoped-api-token"
            }))
        }),
    );
    if !enabled {
        return Ok(app);
    }
    Ok(app.nest(
        "/v1",
        production_router(
            state,
            McpTransportConfig {
                enabled: true,
                ..Default::default()
            },
        ),
    ))
}

fn initial_migration_import_argument() -> Result<Option<PathBuf>, String> {
    let mut arguments = env::args_os().skip(1);
    match (arguments.next(), arguments.next(), arguments.next()) {
        (None, _, _) => Ok(None),
        (Some(flag), Some(path), None) if flag == "--initial-migration-import" => {
            let path = PathBuf::from(path);
            if !path.is_absolute() {
                return Err("initial migration package path must be absolute".into());
            }
            Ok(Some(path))
        }
        _ => Err("usage: buglogin-api [--initial-migration-import <absolute-package-dir>]".into()),
    }
}

async fn run_initial_migration_import(
    production: bool,
    package_dir: PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    if !production
        || env::var("BUGLOGIN_MIGRATION_OPERATOR_MODE").ok().as_deref() != Some("enabled")
    {
        return Err("initial migration import requires production mode and BUGLOGIN_MIGRATION_OPERATOR_MODE=enabled".into());
    }
    if env::var_os("BUGLOGIN_POSTGRES_URL").is_none() {
        return Err("BUGLOGIN_POSTGRES_URL is required for initial migration import".into());
    }
    let files = read_initial_package(&package_dir)?;
    let package = NeutralPackage::from_files(files);
    let config = PostgresConfig::from_env()?;
    let state = tokio::task::spawn_blocking(move || ApiState::production(config)).await??;
    let state = with_configured_migration_keys(state)?;
    let receipt = state.import_initial_signed_migration_package(&package)?;
    println!("{}", serde_json::to_string(&receipt)?);
    Ok(())
}

fn read_initial_package(root: &Path) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let root_metadata = fs::symlink_metadata(root)
        .map_err(|_| "initial migration package directory cannot be read")?;
    if !root_metadata.is_dir() || root_metadata.file_type().is_symlink() {
        return Err("initial migration package path must be a real directory".into());
    }
    let mut files = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    let mut total_bytes = 0_u64;
    while let Some(directory) = pending.pop() {
        let entries = fs::read_dir(&directory)
            .map_err(|_| "initial migration package directory cannot be read")?;
        for entry in entries {
            let entry = entry.map_err(|_| "initial migration package entry cannot be read")?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)
                .map_err(|_| "initial migration package entry metadata cannot be read")?;
            if metadata.file_type().is_symlink() {
                return Err("initial migration package cannot contain symlinks".into());
            }
            if metadata.is_dir() {
                pending.push(path);
                continue;
            }
            if !metadata.is_file() || files.len() >= 512 {
                return Err(
                    "initial migration package has an unsupported entry or too many files".into(),
                );
            }
            let relative = path
                .strip_prefix(root)
                .map_err(|_| "initial migration package path escaped its root")?;
            let relative = relative
                .to_str()
                .ok_or("initial migration package path is not valid Unicode")?
                .replace('\\', "/");
            validate_relative_path(&relative)
                .map_err(|_| "initial migration package path is invalid")?;
            total_bytes = total_bytes
                .checked_add(metadata.len())
                .filter(|size| *size <= 32 * 1024 * 1024)
                .ok_or("initial migration package exceeds the 32 MiB limit")?;
            let bytes =
                fs::read(&path).map_err(|_| "initial migration package file cannot be read")?;
            files.insert(relative, bytes);
        }
    }
    if files.is_empty() {
        return Err("initial migration package directory is empty".into());
    }
    Ok(files)
}

fn with_configured_migration_keys(state: ApiState) -> Result<ApiState, String> {
    let encryption_key = migration_key_from_env("BUGLOGIN_MIGRATION_ENCRYPTION_KEY_HEX")?;
    let signing_key = migration_key_from_env("BUGLOGIN_MIGRATION_SIGNING_KEY_HEX")?;
    match (encryption_key, signing_key) {
        (None, None) => Ok(state),
        (Some(_), None) | (None, Some(_)) => Err(
            "BUGLOGIN_MIGRATION_ENCRYPTION_KEY_HEX and BUGLOGIN_MIGRATION_SIGNING_KEY_HEX must be configured together".into(),
        ),
        (Some(encryption_key), Some(signing_key)) => {
            if *encryption_key == *signing_key {
                return Err("migration encryption and signing keys must be different".into());
            }
            let signing_key = SecretValue::new(signing_key.to_vec())
                .map_err(|_| "migration signing key is invalid".to_owned())?;
            Ok(state
                .with_migration_key(MigrationKey::from_bytes(*encryption_key))
                .with_migration_signing_key(signing_key))
        }
    }
}

fn with_configured_runtime_catalog(state: ApiState) -> Result<ApiState, String> {
    let catalog_path = env::var_os("BUGLOGIN_RUNTIME_CATALOG_PATH").map(PathBuf::from);
    let trust_keys_path = env::var_os("BUGLOGIN_RUNTIME_TRUST_KEYS_PATH").map(PathBuf::from);
    match (catalog_path, trust_keys_path) {
        (None, None) => Ok(state),
        (Some(_), None) | (None, Some(_)) => Err(
            "BUGLOGIN_RUNTIME_CATALOG_PATH and BUGLOGIN_RUNTIME_TRUST_KEYS_PATH must be configured together".into(),
        ),
        (Some(catalog_path), Some(trust_keys_path)) => {
            let catalog_bytes = read_bounded_config_file(&catalog_path, 2 * 1024 * 1024)?;
            let trust_keys_bytes = read_bounded_config_file(&trust_keys_path, 64 * 1024)?;
            let catalog = PublishedRuntimeCatalog::verify(&catalog_bytes, &trust_keys_bytes)
                .map_err(|error| error.code)?;
            Ok(state.with_runtime_catalog(catalog))
        }
    }
}

fn read_bounded_config_file(path: &Path, max_bytes: u64) -> Result<Vec<u8>, String> {
    if !path.is_absolute() {
        return Err("runtime catalog configuration paths must be absolute".into());
    }
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| "runtime catalog configuration file cannot be read".to_owned())?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > max_bytes {
        return Err("runtime catalog configuration file is invalid".into());
    }
    let bytes = fs::read(path)
        .map_err(|_| "runtime catalog configuration file cannot be read".to_owned())?;
    if bytes.len() as u64 > max_bytes {
        return Err("runtime catalog configuration file is too large".into());
    }
    Ok(bytes)
}

fn migration_key_from_env(name: &str) -> Result<Option<Zeroizing<[u8; 32]>>, String> {
    let value = match env::var(name) {
        Ok(value) => value,
        Err(env::VarError::NotPresent) => return Ok(None),
        Err(env::VarError::NotUnicode(_)) => {
            return Err(format!("{name} must be 64 hexadecimal characters"));
        }
    };
    let value = Zeroizing::new(value);
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("{name} must be 64 hexadecimal characters"));
    }
    let mut key = Zeroizing::new([0_u8; 32]);
    for (index, byte) in key.iter_mut().enumerate() {
        let offset = index * 2;
        *byte = u8::from_str_radix(&value[offset..offset + 2], 16)
            .map_err(|_| format!("{name} must be 64 hexadecimal characters"))?;
    }
    if key.iter().all(|byte| *byte == 0) {
        return Err(format!("{name} must not be all zeroes"));
    }
    Ok(Some(key))
}

fn unix_time_ms() -> Result<u64, buglogin_domain::errors::Problem> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| {
            buglogin_domain::errors::Problem::new(
                "clock_unavailable",
                "System clock cannot provide a valid Unix timestamp",
                503,
                "recovery-worker",
            )
            .retryable(true)
        })?
        .as_millis();
    u64::try_from(millis).map_err(|_| {
        buglogin_domain::errors::Problem::new(
            "clock_unavailable",
            "System clock cannot provide a valid Unix timestamp",
            503,
            "recovery-worker",
        )
        .retryable(true)
    })
}

fn validate_redis_url(production: bool, url: &str) -> Result<(), String> {
    let scheme = url.split_once("://").map(|(scheme, _)| scheme);
    if production && !scheme.is_some_and(|scheme| scheme.eq_ignore_ascii_case("rediss")) {
        return Err("BUGLOGIN_REDIS_URL must use rediss:// in production".to_owned());
    }
    Ok(())
}

fn start_sync_migration_recovery_worker(state: ApiState) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        interval.tick().await;
        loop {
            interval.tick().await;
            let state = state.clone();
            let recovery = tokio::task::spawn_blocking(move || {
                let now_ms = unix_time_ms()?;
                let device_authorizations =
                    match state.purge_expired_device_authorizations(now_ms / 1_000) {
                        Ok(purged) => purged,
                        Err(error) => {
                            eprintln!(
                                "Expired device authorization cleanup failed: {}",
                                error.code
                            );
                            0
                        }
                    };
                let sessions = match state.purge_expired_sessions(now_ms / 1_000) {
                    Ok(purged) => purged,
                    Err(error) => {
                        eprintln!("Expired session cleanup failed: {}", error.code);
                        0
                    }
                };
                let purged = state.recover_expired_profile_quarantines(now_ms)?;
                let migrations =
                    state.recover_stale_sync_migrations(now_ms.saturating_sub(15 * 60 * 1_000))?;
                Ok::<_, buglogin_domain::errors::Problem>((
                    purged,
                    device_authorizations,
                    sessions,
                    migrations,
                ))
            })
            .await;
            match recovery {
                Ok(Ok((purged, device_authorizations, sessions, report)))
                    if purged > 0
                        || device_authorizations > 0
                        || sessions > 0
                        || report.recovered > 0
                        || report.failed > 0 =>
                {
                    eprintln!(
                        "Hosted cleanup worker: profiles_purged={}, device_authorizations_purged={}, sessions_purged={}, migration_scanned={}, migration_recovered={}, migration_deferred={}, migration_failed={}",
                        purged,
                        device_authorizations,
                        sessions,
                        report.scanned,
                        report.recovered,
                        report.deferred,
                        report.failed
                    );
                }
                Ok(Ok(_)) => {}
                Ok(Err(error)) => {
                    eprintln!("Hosted cleanup worker unavailable: {}", error.code)
                }
                Err(_) => eprintln!("Hosted cleanup worker task failed"),
            }
        }
    });
}

fn bind_address(production: bool, configured: Option<String>) -> Result<SocketAddr, String> {
    let raw = match configured {
        Some(value) => value,
        None if production => {
            return Err("BUGLOGIN_API_BIND is required in production".to_owned());
        }
        None => "127.0.0.1:12343".to_owned(),
    };
    let address = raw
        .parse::<SocketAddr>()
        .map_err(|_| "BUGLOGIN_API_BIND must be a valid socket address".to_owned())?;
    if address.port() == 0 {
        return Err("BUGLOGIN_API_BIND must use a nonzero port".to_owned());
    }
    if production && address.ip().is_loopback() {
        return Err("BUGLOGIN_API_BIND must be reachable by the production ingress".to_owned());
    }
    Ok(address)
}

fn production_mode() -> Result<bool, String> {
    production_mode_from(env::var("BUGLOGIN_ENV"))
}

fn production_mode_from(value: Result<String, env::VarError>) -> Result<bool, String> {
    match value {
        Ok(value) if value == "production" => Ok(true),
        Ok(value) if value == "local" => Ok(false),
        Ok(_) => Err("BUGLOGIN_ENV must be either local or production".to_owned()),
        Err(env::VarError::NotPresent) => {
            Err("BUGLOGIN_ENV is required; set local or production".to_owned())
        }
        Err(env::VarError::NotUnicode(_)) => Err("BUGLOGIN_ENV must be valid Unicode".to_owned()),
    }
}

fn local_data_root(configured: Option<std::ffi::OsString>) -> Result<PathBuf, String> {
    if let Some(path) = configured {
        return Ok(PathBuf::from(path));
    }
    #[cfg(target_os = "windows")]
    {
        return env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .map(|path| path.join("BugLogin").join("API"))
            .ok_or_else(|| "LOCALAPPDATA is required when BUGLOGIN_DATA_ROOT is unset".into());
    }
    #[cfg(target_os = "macos")]
    {
        return env::var_os("HOME")
            .map(PathBuf::from)
            .map(|path| {
                path.join("Library")
                    .join("Application Support")
                    .join("BugLogin")
                    .join("API")
            })
            .ok_or_else(|| "HOME is required when BUGLOGIN_DATA_ROOT is unset".into());
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        if let Some(path) = env::var_os("XDG_DATA_HOME") {
            return Ok(PathBuf::from(path).join("BugLogin").join("API"));
        }
        env::var_os("HOME")
            .map(PathBuf::from)
            .map(|path| {
                path.join(".local")
                    .join("share")
                    .join("BugLogin")
                    .join("API")
            })
            .ok_or_else(|| "HOME is required when BUGLOGIN_DATA_ROOT is unset".into())
    }
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    use std::env;

    use super::{bind_address, local_data_root, production_mode_from, validate_redis_url};

    #[test]
    fn local_mode_keeps_loopback_default() {
        assert_eq!(
            bind_address(false, None).unwrap(),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 12343)
        );
    }

    #[test]
    fn production_requires_explicit_non_loopback_binding() {
        assert!(bind_address(true, None).is_err());
        assert!(bind_address(true, Some("127.0.0.1:12343".to_owned())).is_err());
        assert!(bind_address(true, Some("0.0.0.0:12343".to_owned())).is_ok());
        assert!(bind_address(true, Some("0.0.0.0:0".to_owned())).is_err());
    }

    #[test]
    fn invalid_environment_never_silently_selects_local_mode() {
        assert!(production_mode_from(Ok("prod".to_owned())).is_err());
        assert!(production_mode_from(Err(env::VarError::NotPresent)).is_err());
        assert!(production_mode_from(Ok("production".to_owned())).unwrap());
    }

    #[test]
    fn production_redis_requires_tls_while_local_allows_plaintext() {
        assert!(validate_redis_url(true, "redis://localhost:6379").is_err());
        assert!(validate_redis_url(true, "rediss://redis.example:6379").is_ok());
        assert!(validate_redis_url(true, "REDISS://redis.example:6379").is_ok());
        assert!(validate_redis_url(false, "redis://localhost:6379").is_ok());
    }

    #[test]
    fn local_data_root_uses_user_application_data_instead_of_working_directory() {
        let path = local_data_root(None).unwrap();
        #[cfg(target_os = "windows")]
        assert!(path.ends_with("BugLogin\\API"));
        #[cfg(target_os = "macos")]
        assert!(path.ends_with("BugLogin/API"));
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        assert!(path.is_absolute());
    }
}
