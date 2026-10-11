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
    let production = env::var("BUGLOGIN_ENV").as_deref() == Ok("production");
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
    let address: SocketAddr = env::var("BUGLOGIN_API_BIND")
        .unwrap_or_else(|_| "127.0.0.1:12343".to_owned())
        .parse()?;
    let listener = TcpListener::bind(address).await?;
    eprintln!("BugLogin API listening on http://{address}");
    serve(listener, router(state)).await?;
    Ok(())
}
