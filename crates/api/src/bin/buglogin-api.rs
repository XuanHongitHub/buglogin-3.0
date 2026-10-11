use std::{
    env,
    net::SocketAddr,
    path::PathBuf,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use axum::serve;
use buglogin_api::http::{ApiState, coordination_authorizer, router};
use buglogin_browser::profile::ProfileService;
use buglogin_storage::{
    control::PostgresConfig,
    local::LocalRepository,
    remote::{S3Config, S3RemoteChunkStore},
};
use buglogin_sync::coordination::{RedisCoordination, RedisCoordinationConfig};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let production = production_mode()?;
    if production && env::var_os("BUGLOGIN_DATA_ROOT").is_some() {
        return Err("BUGLOGIN_DATA_ROOT is forbidden in production".into());
    }
    let state = match env::var_os("BUGLOGIN_POSTGRES_URL") {
        Some(_) => ApiState::production(PostgresConfig::from_env()?)?,
        None if production => {
            return Err("BUGLOGIN_POSTGRES_URL is required in production".into());
        }
        None => {
            let data_root = env::var_os("BUGLOGIN_DATA_ROOT")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(".buglogin-data"));
            let repository = LocalRepository::open(data_root.join("repository"))?;
            ApiState::local(ProfileService::new(repository))
        }
    };
    let state = state.with_recovery_delivery_from_env(production)?;
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
            state.with_remote_chunks(Box::new(S3RemoteChunkStore::new(S3Config::from_env()?)?))
        }
        None if production => return Err("BUGLOGIN_S3_ENDPOINT is required in production".into()),
        None => state,
    };
    let state = match env::var("BUGLOGIN_REDIS_URL") {
        Ok(url) => {
            let namespace = env::var("BUGLOGIN_REDIS_NAMESPACE")
                .unwrap_or_else(|_| "buglogin-production".to_owned());
            let backend = RedisCoordination::connect(
                RedisCoordinationConfig::new(url, namespace),
                coordination_authorizer(&state),
            )
            .await?;
            state.with_coordination(Arc::new(backend))
        }
        Err(_) if production => return Err("BUGLOGIN_REDIS_URL is required in production".into()),
        Err(_) => state,
    };
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_millis()
        .min(u64::MAX as u128) as u64;
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
    serve(listener, router(state)).await?;
    Ok(())
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

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    use std::env;

    use super::{bind_address, production_mode_from};

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
}
